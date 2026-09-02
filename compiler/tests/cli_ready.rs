// `/live` y `/ready` (GRAMMAR.md §3.220, PLAN.md §9.18 Eje E ítem 2):
// liveness y readiness como dos preguntas DISTINTAS, que `/health` (§3.87)
// mezclaba en una sola. Un orquestador/proxy usa `/live` para decidir si
// REINICIAR el proceso (¿responde?) y `/ready` para decidir si ENRUTARLE
// tráfico (¿puede atender AHORA?). Hoy `/ready` = base conectada; es el
// enganche donde el drenado gracioso (Eje E ítem 1) y la saturación del
// pool (Eje B ítem 2) van a sumar sus condiciones sin cambiar el contrato.
//
// Camino feliz contra SQLite, más la forma exacta del JSON y la exención de
// `--service-api-key` (mismo criterio que `/health`: un probe de liveness
// no tiene por qué conocer el secreto del gateway).

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
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-ready-{name}-{}-{}",
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
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

struct Serve {
    child: Child,
    port: u16,
}

impl Serve {
    fn start(link_path: &PathBuf, extra: &[&str]) -> Self {
        let port = free_port();
        let child = Command::new(env!("CARGO_BIN_EXE_linkc"))
            .arg("serve")
            .arg(link_path)
            .arg(port.to_string())
            .args(extra)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("iniciar 'linkc serve'");
        let server = Serve { child, port };
        for _ in 0..200 {
            if server.get("/live", &[]).is_some() {
                return server;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("'linkc serve' no abrió el puerto {port} a tiempo");
    }

    fn get(&self, path: &str, headers: &[(&str, &str)]) -> Option<(u16, String)> {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).ok()?;
        let mut req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
        for (k, v) in headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        req.push_str("\r\n");
        stream.write_all(req.as_bytes()).ok()?;
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
fn live_answers_200_with_only_process_identity_and_no_database_check() {
    let temp = TempDir::new("live");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &[]);

    let (status, body) = server.get("/live", &[]).expect("debería responder");
    assert_eq!(status, 200, "{body}");
    let json: serde_json::Value = serde_json::from_str(&body).unwrap_or_else(|e| panic!("no es JSON ({e}): {body}"));
    assert_eq!(json["status"], "alive", "{json}");
    assert_eq!(json["engine"], "c-script", "{json}");
    assert_eq!(json["version"], env!("CARGO_PKG_VERSION"), "{json}");
    // Liveness NO mira la base: la clave no existe, a propósito -- un
    // proceso vivo con la base caída se reporta vivo acá y no listo en /ready.
    assert!(json.get("database").is_none(), "{json}");
    assert!(json.get("checks").is_none(), "{json}");
}

#[test]
fn ready_answers_200_with_named_checks_against_a_healthy_sqlite_server() {
    let temp = TempDir::new("ready");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &[]);

    let (status, body) = server.get("/ready", &[]).expect("debería responder");
    assert_eq!(status, 200, "{body}");
    let json: serde_json::Value = serde_json::from_str(&body).unwrap_or_else(|e| panic!("no es JSON ({e}): {body}"));
    assert_eq!(json["status"], "ready", "{json}");
    assert_eq!(json["checks"]["database"], "ok", "{json}");
    assert_eq!(json["version"], env!("CARGO_PKG_VERSION"), "{json}");
}

#[test]
fn live_and_ready_are_exempt_from_the_service_api_key_but_rpcs_are_not() {
    let temp = TempDir::new("exempt");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &["--service-api-key", "s3cr3t"]);

    let (live, _) = server.get("/live", &[]).expect("live");
    let (ready, _) = server.get("/ready", &[]).expect("ready");
    assert_eq!(live, 200);
    assert_eq!(ready, 200);

    // Control: la misma protección sigue activa para todo lo demás.
    let (metrics_without_key, _) = server.get("/metrics", &[]).expect("metrics");
    assert_eq!(metrics_without_key, 401);
}
