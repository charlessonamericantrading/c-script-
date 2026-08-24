// `--max-body-bytes <N>`/`LINK_MAX_BODY_BYTES` (GRAMMAR.md §3.85): hasta esta
// ronda `linkc serve` leía el body de CUALQUIER request entero a memoria sin
// ningún límite (`request.as_reader().read_to_string(&mut body)`) -- un
// vector real de agotamiento de memoria, un solo body enorme (a propósito o
// no) se leía completo antes de que auth/rate-limit/forma del JSON tuvieran
// oportunidad de rechazarlo. Se prueba acá contra el binario real, hablando
// HTTP de verdad -- mismo criterio que `cli_cors.rs` (helper `Serve` casi
// textual).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

const PROGRAM: &str = r#"
service Echo {
  rpc len(s: String) -> Int {
    s.length()
  }
}
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "linkc-max-body-{name}-{}-{}",
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
    fn start(link_path: &PathBuf, extra_args: &[&str], extra_env: &[(&str, &str)]) -> Self {
        let port = free_port();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_linkc"));
        cmd.arg("serve").arg(link_path).arg(port.to_string());
        for a in extra_args {
            cmd.arg(a);
        }
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        let child = cmd.spawn().expect("iniciar 'linkc serve'");
        wait_for_port(port);
        Serve { child, port }
    }

    /// Manda un POST con `body` CRUDO (ya armado por el caller, para poder
    /// controlar su tamaño exacto en bytes) y devuelve (status, body de la
    /// respuesta). Siempre `Connection: close` -- cada test abre su propia
    /// conexión nueva, sin depender de keep-alive.
    fn post_raw(&self, path: &str, body: &[u8]) -> (u16, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("conectar");
        let request = format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            self.port,
            body.len()
        );
        stream.write_all(request.as_bytes()).expect("escribir headers");
        stream.write_all(body).expect("escribir body");
        stream.flush().ok();

        let mut reader = BufReader::new(stream);
        let mut status_line = String::new();
        reader.read_line(&mut status_line).expect("línea de estado");
        let status: u16 = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| panic!("estado HTTP inesperado: {status_line:?}"));

        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            let n = reader.read_line(&mut line).expect("header");
            if n == 0 || line.trim().is_empty() {
                break;
            }
            if let Some((k, v)) = line.trim().split_once(':') {
                if k.trim().eq_ignore_ascii_case("content-length") {
                    content_length = v.trim().parse().unwrap_or(0);
                }
            }
        }
        let mut buf = vec![0u8; content_length];
        let _ = reader.read_exact(&mut buf);
        (status, String::from_utf8_lossy(&buf).to_string())
    }

    /// Body JSON `{"s": "<n bytes de 'a'>"}` -- devuelve (body, longitud
    /// exacta en bytes del body completo), para poder fijar
    /// `--max-body-bytes` a un valor exacto en los tests de borde.
    fn json_body_of_total_len(total_len: usize) -> Vec<u8> {
        // `{"s":""}` mide 9 bytes -- el resto se rellena con 'a' dentro del
        // string. Falla en compilación del test (no en runtime) si alguien
        // pide un total_len menor a ese piso, así que no hace falta
        // chequearlo acá.
        let overhead = b"{\"s\":\"\"}".len();
        let padding = total_len - overhead;
        format!("{{\"s\":\"{}\"}}", "a".repeat(padding)).into_bytes()
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn a_body_under_the_default_limit_is_accepted() {
    let temp = TempDir::new("default-ok");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &[], &[]);

    let (status, body) = server.post_raw("/Echo/len", br#"{"s":"hola"}"#);
    assert_eq!(status, 200, "{body}");
    assert_eq!(body, "4");
}

#[test]
fn a_body_exactly_at_the_configured_limit_is_accepted() {
    let temp = TempDir::new("exact-limit");
    let src = temp.write("app.link", PROGRAM);
    let body = Serve::json_body_of_total_len(1000);
    assert_eq!(body.len(), 1000);
    let server = Serve::start(&src, &["--max-body-bytes", "1000"], &[]);

    let (status, resp) = server.post_raw("/Echo/len", &body);
    assert_eq!(status, 200, "un body EXACTO al límite debe aceptarse: {resp}");
}

#[test]
fn a_body_one_byte_over_the_configured_limit_is_rejected_with_413() {
    let temp = TempDir::new("over-limit");
    let src = temp.write("app.link", PROGRAM);
    let body = Serve::json_body_of_total_len(1001);
    assert_eq!(body.len(), 1001);
    let server = Serve::start(&src, &["--max-body-bytes", "1000"], &[]);

    let (status, resp) = server.post_raw("/Echo/len", &body);
    assert_eq!(status, 413, "{resp}");
    assert!(resp.contains("error"), "{resp}");
    assert!(resp.contains("1000"), "el mensaje debería nombrar el límite configurado: {resp}");
}

#[test]
fn a_much_larger_body_is_also_rejected_with_413_not_read_in_full() {
    let temp = TempDir::new("much-larger");
    let src = temp.write("app.link", PROGRAM);
    // ~2 MiB contra un límite de 1000 bytes -- si el servidor lo estuviera
    // leyendo entero de todas formas, este test seguiría pasando (solo
    // probaría el chequeo POSTERIOR a la lectura) pero mucho más lento; el
    // punto real de `.take(...)` es que la lectura misma se corta temprano.
    let body = Serve::json_body_of_total_len(2 * 1024 * 1024);
    let server = Serve::start(&src, &["--max-body-bytes", "1000"], &[]);

    let (status, resp) = server.post_raw("/Echo/len", &body);
    assert_eq!(status, 413, "{resp}");
}

#[test]
fn link_max_body_bytes_env_var_is_honored() {
    let temp = TempDir::new("env");
    let src = temp.write("app.link", PROGRAM);
    let body = Serve::json_body_of_total_len(1001);
    let server = Serve::start(&src, &[], &[("LINK_MAX_BODY_BYTES", "1000")]);

    let (status, resp) = server.post_raw("/Echo/len", &body);
    assert_eq!(status, 413, "{resp}");
}

#[test]
fn cli_flag_takes_precedence_over_the_env_var() {
    let temp = TempDir::new("precedence");
    let src = temp.write("app.link", PROGRAM);
    // El flag permite hasta 1_000_000 bytes; el env var dice un límite
    // mucho más chico -- si el env var ganara, este body (1001 bytes) se
    // rechazaría.
    let body = Serve::json_body_of_total_len(1001);
    let server = Serve::start(&src, &["--max-body-bytes", "1000000"], &[("LINK_MAX_BODY_BYTES", "10")]);

    let (status, resp) = server.post_raw("/Echo/len", &body);
    assert_eq!(status, 200, "el flag debe ganarle al env var: {resp}");
}

#[test]
fn a_max_body_bytes_flag_with_a_non_numeric_value_is_a_clean_cli_error() {
    let temp = TempDir::new("badvalue");
    let src = temp.write("app.link", PROGRAM);
    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&src)
        .arg(free_port().to_string())
        .arg("--max-body-bytes")
        .arg("not-a-number")
        .output()
        .expect("ejecutar linkc serve");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--max-body-bytes"), "el mensaje debe nombrar el flag: {stderr}");
    assert!(!stderr.contains("panicked at"), "un flag mal usado es un error de uso, no un panic: {stderr}");
}

#[test]
fn a_max_body_bytes_flag_without_a_value_is_a_clean_cli_error() {
    let temp = TempDir::new("noval");
    let src = temp.write("app.link", PROGRAM);
    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&src)
        .arg(free_port().to_string())
        .arg("--max-body-bytes")
        .output()
        .expect("ejecutar linkc serve");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--max-body-bytes"), "{stderr}");
}

#[test]
fn security_and_cors_headers_are_still_present_on_a_413_response() {
    let temp = TempDir::new("headers");
    let src = temp.write("app.link", PROGRAM);
    let body = Serve::json_body_of_total_len(1001);
    let server = Serve::start(&src, &["--max-body-bytes", "1000"], &[]);

    let mut stream = TcpStream::connect(("127.0.0.1", server.port)).expect("conectar");
    let request = format!(
        "POST /Echo/len HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nOrigin: https://example.com\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        server.port,
        body.len()
    );
    stream.write_all(request.as_bytes()).unwrap();
    stream.write_all(&body).unwrap();
    stream.flush().ok();

    let mut response = String::new();
    stream.read_to_string(&mut response).ok();
    assert!(response.starts_with("HTTP/1.1 413"), "{response}");
    assert!(response.to_lowercase().contains("access-control-allow-origin"), "{response}");
    assert!(response.to_lowercase().contains("x-content-type-options"), "{response}");
}
