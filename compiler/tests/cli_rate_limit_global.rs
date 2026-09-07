// `--rate-limit-global <N/ventana>`/`LINK_RATE_LIMIT_GLOBAL` (GRAMMAR.md
// §3.281): un tope para TODO el sitio, una sola clave por IP sin importar
// qué rpc golpee -- a diferencia de `@rate_limit`, que protege por rpc. Se
// prueba contra el BINARIO real hablando HTTP de verdad, mismo criterio que
// `cli_hsts.rs`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

const PROGRAM: &str = r#"
service Sys {
  rpc ping() -> String { "pong" }
}
"#;

struct TempDir(PathBuf);

/// Desambigua nombres de tempdir dentro de este mismo proceso -- la
/// resolución del reloj de Windows puede ser tan gruesa como ~15ms, así que
/// dos tests corriendo en paralelo (mismo PID) pueden pedir
/// `SystemTime::now()` dentro de la MISMA ventana y obtener el mismo valor
/// en nanosegundos pese a la precisión nominal de la API -- un CI real de
/// este repo (run 34158505367, windows-latest) mostró un test leyendo el
/// `.link` de OTRO test por esa colisión. Un contador atómico por proceso
/// hace la colisión imposible sin depender de ninguna resolución de reloj.
static TEMP_DIR_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl TempDir {
    fn new(name: &str) -> Self {
        let n = TEMP_DIR_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "linkc-rlg-{name}-{}-{}-{n}",
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

struct Serve {
    child: Child,
    port: u16,
}

impl Serve {
    fn start_with_args(link_path: &PathBuf, extra_args: &[&str]) -> Self {
        let port = free_port();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_linkc"));
        cmd.arg("serve").arg(link_path).arg(port.to_string());
        for a in extra_args {
            cmd.arg(a);
        }
        let child = cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().expect("iniciar 'linkc serve'");
        wait_for_port(port);
        Serve { child, port }
    }

    /// GET/POST crudo -- devuelve el status de la respuesta.
    fn request(&self, method: &str, path: &str) -> u16 {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("conectar");
        let body = if method == "POST" { "{}" } else { "" };
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            self.port,
            body.len()
        );
        stream.write_all(request.as_bytes()).expect("escribir request");
        stream.flush().ok();
        let mut reader = BufReader::new(stream);
        let mut status_line = String::new();
        reader.read_line(&mut status_line).expect("línea de estado");
        status_line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or_else(|| panic!("estado HTTP inesperado: {status_line:?}"))
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn build(temp: &TempDir, source: &str) -> std::process::Output {
    let src = temp.write("app.link", source);
    Command::new(env!("CARGO_BIN_EXE_linkc")).arg("build").arg(&src).arg(temp.0.join("gen")).output().expect("ejecutar linkc build")
}

#[test]
fn without_the_flag_a_burst_of_requests_all_succeed() {
    let temp = TempDir::new("off");
    let out = build(&temp, PROGRAM);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let server = Serve::start_with_args(&temp.0.join("app.link"), &[]);

    for _ in 0..10 {
        assert_eq!(server.request("POST", "/Sys/ping"), 200, "sin --rate-limit-global, comportamiento idéntico al de siempre");
    }
}

#[test]
fn with_the_flag_the_request_past_the_limit_gets_429() {
    let temp = TempDir::new("on");
    let out = build(&temp, PROGRAM);
    assert!(out.status.success());
    let server = Serve::start_with_args(&temp.0.join("app.link"), &["--rate-limit-global", "2/1m"]);

    assert_eq!(server.request("POST", "/Sys/ping"), 200);
    assert_eq!(server.request("POST", "/Sys/ping"), 200);
    assert_eq!(server.request("POST", "/Sys/ping"), 429, "la 3ra request supera el tope global de 2/1m");
}

#[test]
fn live_stays_exempt_even_when_the_global_bucket_is_exhausted() {
    let temp = TempDir::new("live-exempt");
    let out = build(&temp, PROGRAM);
    assert!(out.status.success());
    let server = Serve::start_with_args(&temp.0.join("app.link"), &["--rate-limit-global", "1/1m"]);

    assert_eq!(server.request("POST", "/Sys/ping"), 200);
    assert_eq!(server.request("POST", "/Sys/ping"), 429, "bucket global ya agotado");
    // Mismo criterio de exención que --service-api-key/--max-concurrency: un
    // orquestador haciendo liveness probing no debería poder quedar
    // bloqueado por el tráfico real de otros clientes.
    assert_eq!(server.request("GET", "/live"), 200, "/live nunca cuenta contra el tope global");
}

#[test]
fn invalid_format_is_rejected_at_startup() {
    let temp = TempDir::new("invalid");
    let out = build(&temp, PROGRAM);
    assert!(out.status.success());
    let port = free_port();
    let output = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(temp.0.join("app.link"))
        .arg(port.to_string())
        .arg("--rate-limit-global")
        .arg("bogus")
        .output()
        .expect("ejecutar 'linkc serve'");
    assert!(!output.status.success(), "un formato inválido tiene que rechazar el arranque, no arrancar igual");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("rate-limit-global") || stderr.contains("RATE_LIMIT_GLOBAL"), "mensaje inesperado: {stderr}");
}

#[test]
fn link_rate_limit_global_env_var_is_honored() {
    let temp = TempDir::new("env");
    let out = build(&temp, PROGRAM);
    assert!(out.status.success());
    let port = free_port();
    let child = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(temp.0.join("app.link"))
        .arg(port.to_string())
        .env("LINK_RATE_LIMIT_GLOBAL", "1/1m")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("iniciar 'linkc serve'");
    let server = Serve { child, port };
    wait_for_port(port);

    assert_eq!(server.request("POST", "/Sys/ping"), 200);
    assert_eq!(server.request("POST", "/Sys/ping"), 429);
}
