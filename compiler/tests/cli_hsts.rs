// `--hsts <valor>`/`LINK_HSTS` (GRAMMAR.md §3.143): el header
// `Strict-Transport-Security` que `linkc serve` manda en toda respuesta,
// SOLO si se configura explícitamente -- `linkc serve` nunca termina TLS por
// sí solo, así que sin este opt-in no hay forma de saber que la respuesta de
// verdad viaja sobre HTTPS. Mismo criterio que el resto de las features de
// runtime de esta sesión: se prueba contra el BINARIO real hablando HTTP de
// verdad, no solo que el checker/parser acepten el flag.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

const PROGRAM: &str = r#"
service Sys {
  rpc ping() -> String { "pong" }
  stream events() -> Int { [1, 2] }
}
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "linkc-hsts-{name}-{}-{}",
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

    /// GET /health -- devuelve (status, header Strict-Transport-Security si
    /// vino). El health check nunca requiere body/auth, así que alcanza
    /// para confirmar que el header viaja en TODA respuesta, sin depender
    /// de ningún rpc de usuario.
    fn get_hsts(&self, path: &str) -> (u16, Option<String>) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("conectar");
        let request = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n", self.port);
        stream.write_all(request.as_bytes()).expect("escribir request");
        stream.flush().ok();

        let mut reader = BufReader::new(stream);
        let mut status_line = String::new();
        reader.read_line(&mut status_line).expect("línea de estado");
        let status: u16 = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| panic!("estado HTTP inesperado: {status_line:?}"));

        let mut hsts = None;
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            let n = reader.read_line(&mut line).expect("header");
            if n == 0 || line.trim().is_empty() {
                break;
            }
            if let Some((k, v)) = line.trim().split_once(':') {
                match k.trim().to_ascii_lowercase().as_str() {
                    "strict-transport-security" => hsts = Some(v.trim().to_string()),
                    "content-length" => content_length = v.trim().parse().unwrap_or(0),
                    _ => {}
                }
            }
        }
        let mut buf = vec![0u8; content_length];
        reader.read_exact(&mut buf).ok();
        (status, hsts)
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
fn without_hsts_no_header_is_sent() {
    let temp = TempDir::new("off");
    let out = build(&temp, PROGRAM);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let server = Serve::start_with_args(&temp.0.join("app.link"), &[]);

    let (status, hsts) = server.get_hsts("/health");
    assert_eq!(status, 200);
    assert_eq!(hsts, None, "sin --hsts, comportamiento idéntico al de siempre -- ningún header");
}

#[test]
fn with_hsts_flag_the_literal_value_is_sent_on_every_response() {
    let temp = TempDir::new("flag");
    let out = build(&temp, PROGRAM);
    assert!(out.status.success());
    let server = Serve::start_with_args(&temp.0.join("app.link"), &["--hsts", "max-age=63072000; includeSubDomains"]);

    let (status, hsts) = server.get_hsts("/health");
    assert_eq!(status, 200);
    assert_eq!(hsts.as_deref(), Some("max-age=63072000; includeSubDomains"));

    // También en el camino de un rpc real (POST /Sys/ping), no solo /health.
    let mut stream = TcpStream::connect(("127.0.0.1", server.port)).expect("conectar");
    let body = "{}";
    let request = format!(
        "POST /Sys/ping HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        server.port,
        body.len()
    );
    stream.write_all(request.as_bytes()).expect("escribir request");
    let mut response = String::new();
    stream.read_to_string(&mut response).ok();
    assert!(
        response.to_ascii_lowercase().contains("strict-transport-security: max-age=63072000; includesubdomains"),
        "respuesta: {response}"
    );
}

#[test]
fn with_hsts_flag_a_stream_response_also_carries_the_header() {
    // `sse_preamble` es un segundo lugar que arma headers a mano (server.rs
    // no puede reusar el builder de tiny_http para SSE) -- confirma que los
    // dos caminos no divergieron.
    let temp = TempDir::new("stream");
    let out = build(&temp, PROGRAM);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let server = Serve::start_with_args(&temp.0.join("app.link"), &["--hsts", "max-age=300"]);

    let mut stream = TcpStream::connect(("127.0.0.1", server.port)).expect("conectar");
    let body = "{}";
    let request = format!(
        "POST /Sys/events HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        server.port,
        body.len()
    );
    stream.write_all(request.as_bytes()).expect("escribir request");
    let mut response = String::new();
    stream.read_to_string(&mut response).ok();
    assert!(
        response.to_ascii_lowercase().contains("strict-transport-security: max-age=300"),
        "respuesta: {response}"
    );
}

#[test]
fn link_hsts_env_var_is_honored() {
    let temp = TempDir::new("env");
    let out = build(&temp, PROGRAM);
    assert!(out.status.success());
    let port = free_port();
    let child = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(temp.0.join("app.link"))
        .arg(port.to_string())
        .env("LINK_HSTS", "max-age=300")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("iniciar 'linkc serve'");
    let server = Serve { child, port };
    wait_for_port(port);

    let (status, hsts) = server.get_hsts("/health");
    assert_eq!(status, 200);
    assert_eq!(hsts.as_deref(), Some("max-age=300"));
}
