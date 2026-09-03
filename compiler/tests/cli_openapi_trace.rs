// `GET /openapi.json` servido por `linkc serve` y propagación de
// `traceparent`/`tracestate` a las llamadas `http.*` salientes (GRAMMAR.md
// §3.240, PLAN.md §9.20 Eje H ítem 10): lo que el OTel que instrumenta al
// backend viejo de Skynet necesita para seguir correlacionando cuando un
// tramo pasa por un `.link`. Contra un upstream falso que refleja el header.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-openapi-trace-{name}-{}-{}",
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

/// Upstream falso: responde con el valor del header `traceparent` que
/// recibió (o `none`) como texto plano.
fn start_echo_upstream() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bindear puerto efímero");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    return;
                }
                let mut trace = "none".to_string();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                        break;
                    }
                    if let Some((k, v)) = line.trim().split_once(':') {
                        if k.trim().eq_ignore_ascii_case("traceparent") {
                            trace = v.trim().to_string();
                        }
                    }
                }
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{trace}", trace.len());
                let _ = stream.write_all(response.as_bytes());
            });
        }
    });
    port
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
            if server.request("GET", "/live", "", &[]).is_some() {
                return server;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("'linkc serve' no abrió el puerto {port} a tiempo");
    }

    fn request(&self, method: &str, path: &str, body: &str, headers: &[(&str, &str)]) -> Option<(u16, String, String)> {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).ok()?;
        stream.set_read_timeout(Some(Duration::from_secs(30))).ok()?;
        let mut req = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n", body.len());
        for (k, v) in headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        req.push_str("\r\n");
        req.push_str(body);
        stream.write_all(req.as_bytes()).ok()?;
        let mut reader = BufReader::new(stream);
        let mut status_line = String::new();
        reader.read_line(&mut status_line).ok()?;
        let status: u16 = status_line.split_whitespace().nth(1)?.parse().ok()?;
        let mut content_length = 0usize;
        let mut content_type = String::new();
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
                if k.trim().eq_ignore_ascii_case("content-type") {
                    content_type = v.trim().to_string();
                }
            }
        }
        let mut buf = vec![0u8; content_length];
        reader.read_exact(&mut buf).ok()?;
        Some((status, content_type, String::from_utf8_lossy(&buf).to_string()))
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn openapi_json_is_served_by_the_running_process_and_matches_the_declared_rpcs() {
    let temp = TempDir::new("openapi");
    let src = temp.write("app.link", "type Item = { id: Int, name: String }\ndb { items: Item[] }\nservice Items { rpc list() -> Item[] { db.items.all() } }\n");
    let server = Serve::start(&src, &[]);
    let (status, ct, body) = server.request("GET", "/openapi.json", "", &[]).unwrap();
    assert_eq!(status, 200, "{body}");
    assert!(ct.contains("application/json"), "{ct}");
    let doc: serde_json::Value = serde_json::from_str(&body).expect("JSON válido");
    assert!(doc["openapi"].as_str().unwrap_or("").starts_with("3."), "{body}");
    assert!(doc["paths"].get("/Items/list").is_some(), "{body}");
    assert!(doc["components"]["schemas"].get("Item").is_some(), "{body}");

    // Detrás de --service-api-key como cualquier rpc (no es un probe de
    // liveness): sin la clave, rechazado.
    drop(server);
    let server = Serve::start(&src, &["--service-api-key", "s3cret"]);
    let (status, _, _) = server.request("GET", "/openapi.json", "", &[]).unwrap();
    assert_ne!(status, 200);
    let (status, _, _) = server.request("GET", "/openapi.json", "", &[("X-Service-Api-Key", "s3cret")]).unwrap();
    assert_eq!(status, 200);
}

#[test]
fn traceparent_of_the_incoming_request_is_forwarded_on_outgoing_http_calls() {
    let upstream = start_echo_upstream();
    let temp = TempDir::new("trace");
    let src = temp.write(
        "app.link",
        &format!("service Relay {{ rpc ping() -> String {{ http.get(\"http://127.0.0.1:{upstream}/x\") }} }}\n"),
    );
    let server = Serve::start(&src, &[]);
    let trace = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";
    let (status, _, body) = server.request("POST", "/Relay/ping", "{}", &[("traceparent", trace)]).unwrap();
    assert_eq!(status, 200, "{body}");
    let text: String = serde_json::from_str(&body).expect("un String JSON");
    assert_eq!(text, trace, "el header entrante tiene que llegar al upstream tal cual");

    // Sin header entrante no se inventa ninguno.
    let (status, _, body) = server.request("POST", "/Relay/ping", "{}", &[]).unwrap();
    assert_eq!(status, 200, "{body}");
    let text: String = serde_json::from_str(&body).expect("un String JSON");
    assert_eq!(text, "none");
}
