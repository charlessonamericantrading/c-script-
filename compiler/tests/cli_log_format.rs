// `--log-format`/`--log-level` (GRAMMAR.md §3.122).
//
// Antes de esta ronda, `linkc serve` imprimía SIEMPRE dos líneas de texto
// por request (recibida + completada), sin forma de pedir JSON ni de
// reducir el volumen en producción con tráfico real. Se verifica acá
// contra el BINARIO real, leyendo su stdout de verdad -- que el código
// compile no prueba que la línea impresa tenga la forma exacta que un
// colector de logs esperaría.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const PROGRAM: &str = r#"
type Item = { id: Int, name: String }
db { items: Item[] }

service Sys {
  rpc ping() -> String {
    "pong"
  }
}
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "linkc-log-format-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&path).expect("crear tempdir");
        Self(path)
    }

    fn write(&self, name: &str, content: &str) -> PathBuf {
        let full = self.0.join(name);
        std::fs::write(&full, content).unwrap();
        full
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0)).expect("bindear puerto efímero").local_addr().unwrap().port()
}

fn wait_for_port(port: u16) {
    let mut buf = [0u8; 1];
    for _ in 0..200 {
        if let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) {
            let ready = stream
                .write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
                .is_ok()
                && matches!(stream.read(&mut buf), Ok(n) if n > 0);
            if ready {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("'linkc serve' no abrió el puerto {port} a tiempo");
}

/// Manda una request cruda por `path` y descarta la respuesta -- estos
/// tests solo les importa lo que el servidor IMPRIME, no lo que devuelve.
fn hit(port: u16, path: &str) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("conectar");
    let request = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).expect("escribir request");
    stream.flush().ok();
    // Drena la respuesta -- sin esto, `Connection: close` del lado del
    // servidor puede quedar esperando el shutdown del socket dependiendo
    // del SO, y el próximo `hit()` arrancaría antes de que el anterior
    // terminó de verdad de loguear.
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf);
}

/// Arranca `linkc serve` con stdout REDIRIGIDO (a diferencia de otros
/// tests de CLI que lo mandan a `Stdio::null()` -- acá es justo lo que se
/// verifica), manda las requests que el test necesite, mata el proceso y
/// devuelve TODO lo que imprimió hasta ese momento. `wait_with_output`
/// bloquea hasta que el proceso termina de verdad -- por eso el `kill()`
/// previo, no alcanza con pedir el output de un proceso que sigue vivo.
fn run_and_collect_stdout(link_path: &PathBuf, extra_args: &[&str], hits: &[&str]) -> String {
    let port = free_port();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_linkc"));
    cmd.arg("serve").arg(link_path).arg(port.to_string());
    for a in extra_args {
        cmd.arg(a);
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child: Child = cmd.spawn().expect("iniciar 'linkc serve'");
    wait_for_port(port);
    for path in hits {
        hit(port, path);
    }
    // Un margen chico para que el hilo del servidor termine de imprimir
    // antes del kill -- log_done corre en el mismo hilo que ya escribió la
    // respuesta, así que en la práctica ya pasó, pero no hay una señal
    // explícita de "terminaste de loguear" que esperar.
    std::thread::sleep(Duration::from_millis(100));
    let _ = child.kill();
    let output = child.wait_with_output().expect("esperar a que 'linkc serve' termine");
    String::from_utf8_lossy(&output.stdout).to_string()
}

#[test]
fn default_text_format_prints_the_received_and_done_lines_for_a_normal_request() {
    let temp = TempDir::new("default-text");
    let file = temp.write("app.link", PROGRAM);
    let stdout = run_and_collect_stdout(&file, &[], &["/Sys/ping"]);

    assert!(stdout.contains("/Sys/ping"), "stdout: {stdout}");
    assert!(stdout.lines().any(|l| l.contains("status=") && l.contains("duration_ms=")), "stdout: {stdout}");
}

/// `--log-format json`: cada línea de un request tiene que parsear como
/// JSON de verdad, no solo "parece JSON" -- confirma los campos reales que
/// GRAMMAR.md §3.122 documenta.
#[test]
fn log_format_json_emits_one_valid_json_object_per_line() {
    let temp = TempDir::new("json-format");
    let file = temp.write("app.link", PROGRAM);
    let stdout = run_and_collect_stdout(&file, &["--log-format", "json"], &["/Sys/ping"]);

    let json_lines: Vec<serde_json::Value> =
        stdout.lines().filter(|l| l.trim_start().starts_with('{')).map(|l| serde_json::from_str(l).expect("línea JSON inválida")).collect();
    assert!(json_lines.len() >= 2, "se esperaban al menos 2 líneas JSON (recibida + completada): {stdout}");

    // `wait_for_port` (arriba) también manda un GET /health para chequear
    // que el servidor ya escucha -- CON stdout capturado (a diferencia del
    // resto de los tests de este repo, que lo mandan a `Stdio::null()`),
    // esa request de arranque también queda logueada, así que hay que
    // filtrar por el path real que le interesa a este test, no tomar la
    // primera línea con un campo "path" a secas.
    let received = json_lines.iter().find(|v| v["path"] == "/Sys/ping").expect("línea de request recibida para /Sys/ping");
    assert!(received["req_id"].is_u64());

    let done = json_lines.iter().find(|v| v.get("method") == Some(&serde_json::json!("Sys.ping"))).expect("línea de request completada");
    assert_eq!(done["status"], 200);
    assert!(done["duration_ms"].is_u64());
}

/// `--log-level warn`: una request exitosa (2xx) no deja NINGUNA línea --
/// mismo criterio "reducir volumen en producción" que motivó el ítem
/// (PLAN.md §9.8).
#[test]
fn log_level_warn_suppresses_lines_for_a_successful_request() {
    let temp = TempDir::new("warn-level-success");
    let file = temp.write("app.link", PROGRAM);
    let stdout = run_and_collect_stdout(&file, &["--log-level", "warn"], &["/Sys/ping"]);

    assert!(!stdout.contains("/Sys/ping"), "no debería haber ninguna línea sobre una request exitosa: {stdout}");
}

/// El mismo `--log-level warn`, pero con un 404 -- SÍ tiene que aparecer:
/// "solo lo que amerita mirar" no significa "todo silenciado". Un solo
/// segmento (sin `/Servicio/rpc`) es lo que da 404 -- `/Servicio/rpc` con
/// un `Servicio` inexistente resuelve distinto (500, "service
/// desconocido"), no sirve para este test.
#[test]
fn log_level_warn_still_shows_a_4xx() {
    let temp = TempDir::new("warn-level-4xx");
    let file = temp.write("app.link", PROGRAM);
    let stdout = run_and_collect_stdout(&file, &["--log-level", "warn"], &["/no-tiene-la-forma-correcta"]);

    assert!(stdout.lines().any(|l| l.contains("status=404")), "stdout: {stdout}");
}

#[test]
fn an_invalid_log_format_is_rejected_with_a_clear_message() {
    let temp = TempDir::new("bad-format");
    let file = temp.write("app.link", PROGRAM);
    let port = free_port();
    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&file)
        .arg(port.to_string())
        .arg("--log-format")
        .arg("xml")
        .output()
        .expect("ejecutar linkc");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--log-format"));
}

#[test]
fn an_invalid_log_level_is_rejected_with_a_clear_message() {
    let temp = TempDir::new("bad-level");
    let file = temp.write("app.link", PROGRAM);
    let port = free_port();
    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&file)
        .arg(port.to_string())
        .arg("--log-level")
        .arg("verbose")
        .output()
        .expect("ejecutar linkc");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--log-level"));
}
