// `http.get`/`http.post` (existían sin ningún test propio hasta esta
// ronda) y `http.getWithHeaders`/`http.postWithHeaders` (GRAMMAR.md §3.47):
// la brecha real que motivó esta última pareja era que ninguna llamada
// saliente podía llevar un header -- sin eso, autenticarse contra CUALQUIER
// API de terceros real (Stripe, GitHub, ...) que exija `Authorization` era
// imposible, aunque `http.get`/`http.post` ya existieran.
//
// Se prueba contra un servidor HTTP de mentira escrito a mano en este mismo
// archivo -- captura el método, la URL, los headers y el body EXACTOS que
// `ureq` mandó de verdad sobre un TcpStream real, no un mock interno del
// intérprete.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

const PROGRAM: &str = r#"
type Header = { name: String, value: String }

service Sys {
  rpc plainGet(url: String) -> String {
    http.get(url)
  }

  rpc plainPost(url: String, body: String) -> String {
    http.post(url, body)
  }

  rpc getWithAuth(url: String, token: String) -> String {
    http.getWithHeaders(url, [
      Header { name: "Authorization", value: token },
      Header { name: "X-Custom", value: "abc" },
    ])
  }

  rpc postWithAuth(url: String, body: String, token: String) -> String {
    http.postWithHeaders(url, body, [
      Header { name: "Authorization", value: token },
      Header { name: "Content-Type", value: "application/json" },
    ])
  }
}
"#;

#[derive(Debug)]
struct ReceivedRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl ReceivedRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// Servidor HTTP mínimo: parsea lo justo (línea de pedido, headers,
/// `Content-Length`) para capturar una request real y devolver un 200 con
/// un body fijo -- no pretende ser un servidor HTTP completo.
struct FakeHttp {
    port: u16,
    rx: Receiver<ReceivedRequest>,
}

impl FakeHttp {
    fn start() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bindear puerto efímero");
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let tx = tx.clone();
                std::thread::spawn(move || handle_one_connection(stream, tx));
            }
        });
        FakeHttp { port, rx }
    }

    fn recv(&self, timeout: Duration) -> Option<ReceivedRequest> {
        self.rx.recv_timeout(timeout).ok()
    }
}

fn handle_one_connection(stream: TcpStream, tx: Sender<ReceivedRequest>) {
    let mut writer = stream.try_clone().expect("clonar el stream");
    let mut reader = BufReader::new(stream);

    let mut request_line = String::new();
    if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();

    let mut headers = Vec::new();
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = line.trim_end().split_once(':') {
            let (k, v) = (k.trim().to_string(), v.trim().to_string());
            if k.eq_ignore_ascii_case("content-length") {
                content_length = v.parse().unwrap_or(0);
            }
            headers.push((k, v));
        }
    }
    let mut body_buf = vec![0u8; content_length];
    if content_length > 0 {
        let _ = reader.read_exact(&mut body_buf);
    }
    let body = String::from_utf8_lossy(&body_buf).to_string();

    let response_body = br#"{"ok":true}"#;
    let _ = write!(
        writer,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response_body.len()
    );
    let _ = writer.write_all(response_body);

    let _ = tx.send(ReceivedRequest { method, path, headers, body });
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "linkc-http-{name}-{}-{}",
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
        wait_for_port(port);
        Serve { child, port }
    }

    fn post(&self, path: &str, body: &str) -> (u16, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("conectar");
        let request = format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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
fn http_get_and_post_reach_a_real_server_over_a_real_socket() {
    let upstream = FakeHttp::start();
    let temp = TempDir::new("plain");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src);

    let url = format!("http://127.0.0.1:{}/ping", upstream.port);
    let (status, body) = server.post("/Sys/plainGet", &serde_json::json!({"url": url}).to_string());
    assert_eq!(status, 200, "body: {body}");
    assert_eq!(body, "\"{\\\"ok\\\":true}\"");
    let req = upstream.recv(Duration::from_secs(5)).expect("el servidor de mentira debió recibir un GET");
    assert_eq!(req.method, "GET");
    assert_eq!(req.path, "/ping");

    let url = format!("http://127.0.0.1:{}/create", upstream.port);
    let (status, _) = server.post("/Sys/plainPost", &serde_json::json!({"url": url, "body": "hola=mundo"}).to_string());
    assert_eq!(status, 200);
    let req = upstream.recv(Duration::from_secs(5)).expect("el servidor de mentira debió recibir un POST");
    assert_eq!(req.method, "POST");
    assert_eq!(req.body, "hola=mundo");
}

#[test]
fn get_with_headers_sends_every_declared_header_on_a_real_request() {
    // La brecha real (GRAMMAR.md §3.47): sin esto, ningún rpc podía
    // autenticarse contra una API de terceros -- `Authorization` nunca
    // llegaba, aunque `http.get` en sí ya funcionara.
    let upstream = FakeHttp::start();
    let temp = TempDir::new("get-headers");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src);

    let url = format!("http://127.0.0.1:{}/v1/charges", upstream.port);
    let (status, _) =
        server.post("/Sys/getWithAuth", &serde_json::json!({"url": url, "token": "Bearer sk_test_123"}).to_string());
    assert_eq!(status, 200);

    let req = upstream.recv(Duration::from_secs(5)).expect("el servidor de mentira debió recibir la request");
    assert_eq!(req.method, "GET");
    assert_eq!(req.header("Authorization"), Some("Bearer sk_test_123"), "headers: {:?}", req.headers);
    assert_eq!(req.header("X-Custom"), Some("abc"), "headers: {:?}", req.headers);
}

#[test]
fn post_with_headers_sends_headers_and_body_together_on_a_real_request() {
    let upstream = FakeHttp::start();
    let temp = TempDir::new("post-headers");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src);

    let url = format!("http://127.0.0.1:{}/v1/checkout/sessions", upstream.port);
    let (status, _) = server.post(
        "/Sys/postWithAuth",
        &serde_json::json!({"url": url, "body": "amount=1000&currency=usd", "token": "Bearer sk_test_456"}).to_string(),
    );
    assert_eq!(status, 200);

    let req = upstream.recv(Duration::from_secs(5)).expect("el servidor de mentira debió recibir la request");
    assert_eq!(req.method, "POST");
    assert_eq!(req.header("Authorization"), Some("Bearer sk_test_456"), "headers: {:?}", req.headers);
    assert_eq!(req.header("Content-Type"), Some("application/json"), "headers: {:?}", req.headers);
    assert_eq!(req.body, "amount=1000&currency=usd");
}

#[test]
fn http_get_with_headers_against_an_unreachable_host_fails_cleanly_not_with_a_panic() {
    let temp = TempDir::new("unreachable");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src);

    // Puerto libre, nada escuchando -- simula el host caído.
    let dead_port = free_port();
    let url = format!("http://127.0.0.1:{dead_port}/");
    let (status, body) = server.post("/Sys/getWithAuth", &serde_json::json!({"url": url, "token": "x"}).to_string());
    assert_eq!(status, 500, "body: {body}");
    assert!(!body.contains("panicked"), "una conexión caída es una condición operativa normal, no un panic: {body}");
}
