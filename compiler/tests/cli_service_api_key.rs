// `--service-api-key`/`LINK_SERVICE_API_KEY` (GRAMMAR.md §3.93): un secreto
// compartido que autentica al LLAMADOR (server-to-server), distinto de
// `@requires`/JWT (que autentican a un usuario final). Caso real citado por
// el usuario: un gateway Node.js hace `fetch` sin autenticación contra 13
// procesos `linkc serve` en el mismo VPS, confiando solo en que el puerto no
// sea alcanzable desde afuera -- cualquier OTRO proceso corriendo en esa
// misma máquina con acceso a loopback puede llamarlos igual.
//
// Se prueba contra el BINARIO real, hablando HTTP de verdad -- que el
// código compile no prueba que un caller sin el header se rechace con 401
// antes de llegar a ejecutar ningún rpc, ni que /health siga siendo
// alcanzable sin el secreto.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

const PROGRAM: &str = r#"
type Item = { id: Int, name: String }
db { items: Item[] }
service Sys {
  rpc ping() -> String { "pong" }
}
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "linkc-service-key-{name}-{}-{}",
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
        Self::start_with_env(link_path, extra_args, &[])
    }

    fn start_with_env(link_path: &PathBuf, extra_args: &[&str], extra_env: &[(&str, &str)]) -> Self {
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

    fn request(&self, path: &str, extra_headers: &[(&str, &str)]) -> (u16, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("conectar");
        let body = "{}";
        let mut request = format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
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
        let status: u16 = resp.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
        let body = resp.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
        (status, body)
    }

    fn get(&self, path: &str) -> u16 {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("conectar");
        let request = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n", self.port);
        stream.write_all(request.as_bytes()).unwrap();
        stream.flush().ok();
        let mut resp = String::new();
        stream.read_to_string(&mut resp).ok();
        resp.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0)
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn without_the_flag_no_key_is_required_at_all() {
    let temp = TempDir::new("off");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &[]);
    let (status, body) = server.request("/Sys/ping", &[]);
    assert_eq!(status, 200);
    assert_eq!(body, "\"pong\"");
}

/// AUDIT-2026-08-27.md #13: `--service-api-key ""` (valor vacío explícito
/// por flag) activaba la capa entera con un secreto vacío -- ahora se
/// filtra igual que un valor de env var vacío, comportándose IDÉNTICO a no
/// pasar el flag en absoluto.
#[test]
fn an_empty_string_flag_value_behaves_like_the_flag_was_never_passed() {
    let temp = TempDir::new("empty-flag");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &["--service-api-key", ""]);
    let (status, body) = server.request("/Sys/ping", &[]);
    assert_eq!(status, 200, "body: {body:?}");
    assert_eq!(body, "\"pong\"");
}

#[test]
fn a_request_without_the_header_is_rejected_before_it_reaches_the_rpc() {
    let temp = TempDir::new("missing-header");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &["--service-api-key", "s3cr3t"]);
    let (status, body) = server.request("/Sys/ping", &[]);
    assert_eq!(status, 401);
    assert!(body.contains("X-Service-Api-Key"), "{body}");
}

#[test]
fn a_request_with_the_wrong_key_is_rejected() {
    let temp = TempDir::new("wrong-key");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &["--service-api-key", "s3cr3t"]);
    let (status, _) = server.request("/Sys/ping", &[("X-Service-Api-Key", "wrong")]);
    assert_eq!(status, 401);
}

#[test]
fn a_request_with_the_right_key_reaches_the_rpc() {
    let temp = TempDir::new("right-key");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &["--service-api-key", "s3cr3t"]);
    let (status, body) = server.request("/Sys/ping", &[("X-Service-Api-Key", "s3cr3t")]);
    assert_eq!(status, 200);
    assert_eq!(body, "\"pong\"");
}

#[test]
fn health_stays_reachable_without_the_key() {
    let temp = TempDir::new("health-exempt");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &["--service-api-key", "s3cr3t"]);
    assert_eq!(server.get("/health"), 200);
    assert_eq!(server.get("/"), 200);
    assert_eq!(server.get("/status"), 200);
}

#[test]
fn link_service_api_key_env_var_works_the_same_as_the_flag() {
    let temp = TempDir::new("env-var");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start_with_env(&src, &[], &[("LINK_SERVICE_API_KEY", "from-env")]);
    let (status, _) = server.request("/Sys/ping", &[]);
    assert_eq!(status, 401);
    let (status, body) = server.request("/Sys/ping", &[("X-Service-Api-Key", "from-env")]);
    assert_eq!(status, 200);
    assert_eq!(body, "\"pong\"");
}

#[test]
fn a_service_api_key_flag_without_a_value_is_a_clean_cli_error() {
    let temp = TempDir::new("badflag");
    let src = temp.write("app.link", PROGRAM);
    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&src)
        .arg(free_port().to_string())
        .arg("--service-api-key")
        .output()
        .expect("ejecutar linkc serve");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--service-api-key"), "{stderr}");
    assert!(!stderr.contains("panicked at"), "{stderr}");
}
