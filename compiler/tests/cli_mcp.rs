// MCP real (GRAMMAR.md §3.203) -- Pieza A: sesión (`initialize`/`DELETE`).
// Se prueba contra el BINARIO real hablando HTTP de verdad, mismo criterio
// que el resto de los tests de este estilo (`cli_service_api_key.rs`): que
// el código compile no prueba que `--mcp-jwt-secret` de verdad habilite
// `/mcp`, que `initialize` de verdad exija un `Authorization: Bearer` real
// y devuelva un `Mcp-Session-Id` usable, ni que `DELETE` de verdad revoque
// esa sesión.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

const PROGRAM: &str = r#"
enum Role { Admin }

service Auth {
  rpc login() -> String { auth.createSession(Role.Admin {}) }
}
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "linkc-mcp-{name}-{}-{}",
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
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_linkc"));
        cmd.arg("serve").arg(link_path).arg(port.to_string());
        for a in extra_args {
            cmd.arg(a);
        }
        cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        let child = cmd.spawn().expect("iniciar 'linkc serve'");
        wait_for_port(port);
        Serve { child, port }
    }

    /// Request cruda con método/body/headers arbitrarios -- devuelve
    /// status, headers de RESPUESTA (para leer `Mcp-Session-Id`) y el body
    /// crudo como string.
    fn request(&self, method: &str, path: &str, body: &str, extra_headers: &[(&str, &str)]) -> (u16, Vec<(String, String)>, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("conectar");
        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
            self.port,
            body.len()
        );
        for (k, v) in extra_headers {
            request.push_str(&format!("{k}: {v}\r\n"));
        }
        request.push_str("\r\n");
        request.push_str(body);
        stream.write_all(request.as_bytes()).unwrap();
        stream.flush().ok();
        let mut resp = String::new();
        stream.read_to_string(&mut resp).ok();
        let (head, tail) = resp.split_once("\r\n\r\n").unwrap_or((resp.as_str(), ""));
        let mut lines = head.lines();
        let status: u16 = lines.next().and_then(|l| l.split_whitespace().nth(1)).and_then(|s| s.parse().ok()).unwrap_or(0);
        let headers: Vec<(String, String)> = lines
            .filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
            .collect();
        (status, headers, tail.to_string())
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Login real (mismo patrón que `server_http.rs`) para conseguir un token
/// de sesión válido, sin fabricar ninguno a mano.
fn login(server: &Serve) -> String {
    let (status, _, body) = server.request("POST", "/Auth/login", "{}", &[]);
    assert_eq!(status, 200, "body: {body}");
    serde_json::from_str::<String>(&body).expect("login debe devolver un token string")
}

#[test]
fn without_the_flag_mcp_endpoint_does_not_exist() {
    let temp = TempDir::new("off");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &[]);
    let (status, _, body) = server.request("POST", "/mcp", r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#, &[]);
    assert_eq!(status, 404, "sin --mcp-jwt-secret, /mcp no debería existir: {body}");
}

#[test]
fn initialize_without_a_bearer_token_is_rejected() {
    let temp = TempDir::new("init-no-token");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &["--mcp-jwt-secret", "mcp-s3cr3t"]);
    let (status, _, body) = server.request("POST", "/mcp", r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#, &[]);
    assert_eq!(status, 401, "body: {body}");
}

#[test]
fn initialize_with_a_real_login_token_returns_a_usable_mcp_session_id() {
    let temp = TempDir::new("init-ok");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &["--mcp-jwt-secret", "mcp-s3cr3t"]);
    let token = login(&server);

    let (status, headers, body) = server.request(
        "POST",
        "/mcp",
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
        &[("Authorization", &format!("Bearer {token}"))],
    );
    assert_eq!(status, 200, "body: {body}");
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("body debe ser JSON");
    assert_eq!(parsed["result"]["protocolVersion"], serde_json::json!("2025-06-18"), "body: {body}");
    let mcp_session_id = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("Mcp-Session-Id"))
        .map(|(_, v)| v.clone())
        .expect("initialize exitoso tiene que devolver el header Mcp-Session-Id");
    assert!(!mcp_session_id.is_empty());
}

#[test]
fn delete_without_the_session_header_is_a_clean_400() {
    let temp = TempDir::new("delete-no-header");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &["--mcp-jwt-secret", "mcp-s3cr3t"]);
    let (status, _, body) = server.request("DELETE", "/mcp", "", &[]);
    assert_eq!(status, 400, "body: {body}");
}

#[test]
fn delete_with_an_unknown_session_id_is_a_clean_404() {
    let temp = TempDir::new("delete-unknown");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &["--mcp-jwt-secret", "mcp-s3cr3t"]);
    let (status, _, body) = server.request("DELETE", "/mcp", "", &[("Mcp-Session-Id", "not-a-real-jwt")]);
    assert_eq!(status, 404, "body: {body}");
}

#[test]
fn a_session_terminated_by_delete_is_rejected_by_a_later_request() {
    let temp = TempDir::new("delete-then-use");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &["--mcp-jwt-secret", "mcp-s3cr3t"]);
    let token = login(&server);

    let (status, headers, body) = server.request(
        "POST",
        "/mcp",
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
        &[("Authorization", &format!("Bearer {token}"))],
    );
    assert_eq!(status, 200, "body: {body}");
    let mcp_session_id = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("Mcp-Session-Id")).map(|(_, v)| v.clone()).unwrap();

    let (status, _, body) = server.request("DELETE", "/mcp", "", &[("Mcp-Session-Id", &mcp_session_id)]);
    assert_eq!(status, 204, "body: {body}");

    // Revocar de nuevo la MISMA sesión ya terminada -- 404, no un segundo
    // 204 (la sesión ya no existe desde el punto de vista de este store).
    let (status, _, body) = server.request("DELETE", "/mcp", "", &[("Mcp-Session-Id", &mcp_session_id)]);
    assert_eq!(status, 404, "revocar dos veces la misma sesión: {body}");
}

#[test]
fn an_unknown_mcp_method_gets_a_clean_501_not_a_crash() {
    let temp = TempDir::new("unknown-method");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &["--mcp-jwt-secret", "mcp-s3cr3t"]);
    let (status, _, body) = server.request("POST", "/mcp", r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#, &[]);
    assert_eq!(status, 501, "body: {body}");
}
