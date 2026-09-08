// `log.info/warn/error(msg, meta?)` (GRAMMAR.md §3.291, PLAN.md §9.24 Fase 2
// ítem G5): trazas de negocio/`[AUDIT]` desde código de usuario, respetando
// el MISMO `--log-format`/`--log-level` que el resto de las líneas de log
// del servidor -- no un `println!` suelto sin filtro ni forma JSON.
//
// Contra el BINARIO real vía `linkc serve`, leyendo el stdout capturado.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const PROGRAM: &str = r#"
type Meta = { userId: Int, action: String }

service S {
  rpc doThing() -> String {
    log.info("accion completada", Meta { userId: 42, action: "purge" });
    log.warn("algo raro paso", null);
    log.error("fallo real", null);
    "ok"
  }
}
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-log-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        );
        let path = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&path).expect("crear tempdir");
        Self(path)
    }

    fn write(&self, name: &str, content: &str) -> PathBuf {
        let p = self.0.join(name);
        std::fs::write(&p, content).expect("escribir archivo");
        p
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

fn post(port: u16, path: &str) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("conectar");
    let body = "{}";
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).unwrap();
    stream.flush().ok();
    let mut resp = String::new();
    stream.read_to_string(&mut resp).ok();
}

struct Serve {
    child: Child,
    port: u16,
}

impl Serve {
    fn start(link_path: &PathBuf, extra_args: &[&str]) -> Self {
        let port = free_port();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_linkc"));
        cmd.arg("serve").arg(link_path).arg(port.to_string());
        for a in extra_args {
            cmd.arg(a);
        }
        cmd.stdout(Stdio::piped()).stderr(Stdio::null());
        let child = cmd.spawn().expect("iniciar linkc serve");
        wait_for_port(port);
        Serve { child, port }
    }

    /// Mata el proceso y lee TODO su stdout acumulado hasta ese punto --
    /// mismo criterio que `cli_graceful_drain.rs`: el log real es lo único
    /// que prueba qué imprimió el servidor, no una suposición.
    fn kill_and_read_stdout(mut self) -> String {
        let mut stdout = self.child.stdout.take().expect("stdout capturado");
        let _ = self.child.kill();
        let _ = self.child.wait();
        let mut out = String::new();
        let _ = stdout.read_to_string(&mut out);
        out
    }
}

#[test]
fn log_lines_appear_in_text_format_with_the_right_level_and_meta() {
    let temp = TempDir::new("text");
    let link_path = temp.write("app.link", PROGRAM);
    let server = Serve::start(&link_path, &[]);
    let port = server.port;
    post(port, "/S/doThing");
    std::thread::sleep(Duration::from_millis(200));
    let out = server.kill_and_read_stdout();

    assert!(out.contains(r#"[log] level=info msg="accion completada" meta={"action":"purge","userId":42}"#), "{out}");
    assert!(out.contains(r#"[log] level=warn msg="algo raro paso""#), "{out}");
    assert!(out.contains(r#"[log] level=error msg="fallo real""#), "{out}");
}

#[test]
fn log_lines_appear_as_valid_json_when_log_format_is_json() {
    let temp = TempDir::new("json");
    let link_path = temp.write("app.link", PROGRAM);
    let server = Serve::start(&link_path, &["--log-format", "json"]);
    let port = server.port;
    post(port, "/S/doThing");
    std::thread::sleep(Duration::from_millis(200));
    let out = server.kill_and_read_stdout();

    let info_line = out.lines().find(|l| l.contains(r#""level":"info""#)).unwrap_or_else(|| panic!("no se encontró la línea info: {out}"));
    let parsed: serde_json::Value = serde_json::from_str(info_line).expect("línea JSON válida");
    assert_eq!(parsed["msg"], "accion completada");
    assert_eq!(parsed["meta"]["userId"], 42);
    assert_eq!(parsed["meta"]["action"], "purge");
}

#[test]
fn log_level_filters_out_lower_severity_log_calls_same_as_request_logging() {
    let temp = TempDir::new("level");
    let link_path = temp.write("app.link", PROGRAM);
    let server = Serve::start(&link_path, &["--log-level", "warn"]);
    let port = server.port;
    post(port, "/S/doThing");
    std::thread::sleep(Duration::from_millis(200));
    let out = server.kill_and_read_stdout();

    assert!(!out.contains("accion completada"), "log.info tiene que suprimirse con --log-level warn: {out}");
    assert!(out.contains("algo raro paso"), "log.warn SÍ tiene que pasar el filtro: {out}");
    assert!(out.contains("fallo real"), "log.error SÍ tiene que pasar el filtro: {out}");
}

#[test]
fn log_without_meta_omits_the_meta_field_entirely_never_null_or_empty_braces() {
    let temp = TempDir::new("nometa");
    let link_path = temp.write("app.link", PROGRAM);
    let server = Serve::start(&link_path, &[]);
    let port = server.port;
    post(port, "/S/doThing");
    std::thread::sleep(Duration::from_millis(200));
    let out = server.kill_and_read_stdout();

    let warn_line = out.lines().find(|l| l.starts_with("[log] level=warn")).unwrap_or_else(|| panic!("{out}"));
    assert_eq!(warn_line, r#"[log] level=warn msg="algo raro paso""#, "sin meta, la línea de texto no debería tener ' meta='");
}
