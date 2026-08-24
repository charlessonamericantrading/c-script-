// `/health` hace un `SELECT 1` real contra la base (GRAMMAR.md §3.87) --
// hasta esta ronda devolvía 200 fijo sin importar si la base respondía o
// no, inútil para cualquier orquestador (Kubernetes, un load balancer) que
// lo usa para decidir si reiniciar el proceso o sacarlo de rotación. El
// caso de falla real (Postgres caído, reconectado solo) se prueba en
// `pg_integration.rs`, reusando la misma técnica de `pg_terminate_backend`
// que el test de reconexión (GRAMMAR.md §3.40) -- acá solo el camino feliz
// contra SQLite, y la FORMA exacta del JSON que "/health" devuelve.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

const PROGRAM: &str = r#"
type Item = { id: Int, name: String }
db { items: Item[] }

service Items {
  rpc list() -> Item[] { db.items.all() }
}

service Health {
  rpc ping() -> String { "pong" }
}
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "linkc-health-{name}-{}-{}",
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

struct Serve {
    child: Child,
    port: u16,
}

impl Serve {
    fn start(link_path: &PathBuf) -> Self {
        let port = free_port();
        let child = Command::new(env!("CARGO_BIN_EXE_linkc"))
            .arg("serve")
            .arg(link_path)
            .arg(port.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("iniciar 'linkc serve'");
        let server = Serve { child, port };
        server.wait_ready();
        server
    }

    fn wait_ready(&self) {
        for _ in 0..200 {
            if self.try_get_health().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("'linkc serve' no abrió el puerto {} a tiempo", self.port);
    }

    fn try_get_health(&self) -> Option<(u16, String)> {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).ok()?;
        stream.write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").ok()?;
        let mut reader = BufReader::new(stream);
        let mut status_line = String::new();
        reader.read_line(&mut status_line).ok()?;
        let status: u16 = status_line.split_whitespace().nth(1)?.parse().ok()?;

        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            let n = reader.read_line(&mut line).ok()?;
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
        reader.read_exact(&mut buf).ok()?;
        Some((status, String::from_utf8_lossy(&buf).to_string()))
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn health_reports_ok_and_a_working_database_against_a_healthy_sqlite_server() {
    let temp = TempDir::new("ok");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src);

    let (status, body) = server.try_get_health().expect("debería responder");
    assert_eq!(status, 200, "{body}");
    let json: serde_json::Value = serde_json::from_str(&body).unwrap_or_else(|e| panic!("no es JSON válido ({e}): {body}"));

    assert_eq!(json["status"], "ok", "{json}");
    assert_eq!(json["engine"], "c-script", "{json}");
    assert_eq!(json["database"], "ok", "{json}");
    assert_eq!(json["version"], env!("CARGO_PKG_VERSION"), "{json}");

    let services: Vec<&str> = json["services"].as_array().expect("services debe ser un array").iter().map(|s| s.as_str().unwrap()).collect();
    assert!(services.contains(&"Items"), "{services:?}");
    assert!(services.contains(&"Health"), "{services:?}");
}

#[test]
fn all_three_health_aliases_return_the_same_shape() {
    // `/`, `/health`, `/status` son el mismo handler -- probado con las tres
    // rutas para que una futura ronda que solo actualice una de las tres no
    // pase desapercibida.
    let temp = TempDir::new("aliases");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src);

    for path in ["/", "/health", "/status"] {
        let mut stream = TcpStream::connect(("127.0.0.1", server.port)).expect("conectar");
        stream
            .write_all(format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").as_bytes())
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).ok();
        assert!(response.starts_with("HTTP/1.1 200"), "'{path}': {response}");
        assert!(response.contains("\"database\":\"ok\""), "'{path}': {response}");
    }
}
