// `--argon2-memory-kib`/`--argon2-iterations` (GRAMMAR.md §3.55): el costo
// de `crypto.hashPassword` era fijo al default de la crate `argon2` (~19 MiB,
// 2 iteraciones) sin ninguna forma de subirlo desde el lenguaje ni desde la
// línea de comandos -- un servicio con requisitos de seguridad más altos no
// tenía cómo pedirlo. Este archivo prueba, contra el binario real, que los
// flags cambian el costo REAL embebido en el hash resultante (formato PHC:
// `$argon2id$v=19$m=<mem>,t=<iter>,p=<par>$...`), no solo que se acepten sin
// error.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

const PROGRAM: &str = r#"
service Auth {
  rpc hash(pwd: String) -> String {
    crypto.hashPassword(pwd)
  }
}
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "linkc-argon2-{name}-{}-{}",
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
    fn start(link_path: &PathBuf, extra_args: &[&str]) -> Self {
        let port = free_port();
        let child = Command::new(env!("CARGO_BIN_EXE_linkc"))
            .arg("serve")
            .arg(link_path)
            .arg(port.to_string())
            .args(extra_args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("iniciar 'linkc serve'");
        wait_for_port(port);
        Serve { child, port }
    }

    fn post(&self, path: &str, body: &str) -> (u16, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("conectar");
        let request = format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            self.port,
            body.len()
        );
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
        reader.read_exact(&mut buf).expect("body");
        (status, String::from_utf8_lossy(&buf).to_string())
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn without_flags_hashpassword_uses_the_crate_default_cost() {
    let temp = TempDir::new("default");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &[]);

    let (status, body) = server.post("/Auth/hash", r#"{"pwd":"correo-horse-battery"}"#);
    assert_eq!(status, 200);
    assert!(body.contains("$argon2id$"), "body inesperado: {body}");
    assert!(body.contains("m=19456"), "default de la crate esperado (19456 KiB): {body}");
    assert!(body.contains("t=2"), "default de la crate esperado (2 iteraciones): {body}");
}

#[test]
fn the_flags_change_the_real_cost_embedded_in_the_hash() {
    let temp = TempDir::new("custom");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &["--argon2-memory-kib", "8192", "--argon2-iterations", "3"]);

    let (status, body) = server.post("/Auth/hash", r#"{"pwd":"correo-horse-battery"}"#);
    assert_eq!(status, 200);
    assert!(body.contains("m=8192"), "no se aplicó --argon2-memory-kib: {body}");
    assert!(body.contains("t=3"), "no se aplicó --argon2-iterations: {body}");
}

#[test]
fn an_invalid_value_fails_fast_with_a_clear_message_instead_of_starting() {
    let temp = TempDir::new("invalid");
    let src = temp.write("app.link", PROGRAM);
    let port = free_port();
    let output = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&src)
        .arg(port.to_string())
        .arg("--argon2-memory-kib")
        .arg("no-es-un-numero")
        .output()
        .expect("ejecutar 'linkc serve'");
    assert!(!output.status.success(), "un valor no numérico debió rechazarse antes de arrancar");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("argon2-memory-kib"), "mensaje inesperado: {stderr}");
}
