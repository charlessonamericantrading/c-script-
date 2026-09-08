// `--metrics-token`/`LINK_METRICS_TOKEN` (GRAMMAR.md §3.285, PLAN.md §9.24
// Fase 2 ítem D6): un secreto DEDICADO a `GET /metrics`, independiente de
// `--service-api-key` (que ya puede cubrir `/metrics` con su propio header
// custom `X-Service-Api-Key`). Prometheus mismo habla nativamente
// `Authorization: Bearer <token>` -- este flag existe para no forzar a un
// operador que solo quiere proteger el scrape a levantar todo un secreto
// servidor-a-servidor que no necesita para nada más.
//
// Se prueba contra el BINARIO real, hablando HTTP de verdad -- mismo
// criterio que `cli_service_api_key.rs`.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

const PROGRAM: &str = r#"
service Sys {
  rpc ping() -> String { "pong" }
}
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "linkc-metrics-token-{name}-{}-{}",
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

    fn get(&self, path: &str, extra_headers: &[(&str, &str)]) -> (u16, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("conectar");
        let mut request = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n", self.port);
        for (k, v) in extra_headers {
            request.push_str(&format!("{k}: {v}\r\n"));
        }
        request.push_str("\r\n");
        stream.write_all(request.as_bytes()).unwrap();
        stream.flush().ok();
        let mut resp = String::new();
        stream.read_to_string(&mut resp).ok();
        let status: u16 = resp.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
        let body = resp.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
        (status, body)
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn without_the_flag_metrics_is_public_same_as_always() {
    let temp = TempDir::new("off");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &[]);
    let (status, body) = server.get("/metrics", &[]);
    assert_eq!(status, 200, "{body}");
}

#[test]
fn a_request_without_the_bearer_header_is_rejected() {
    let temp = TempDir::new("missing-auth");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &["--metrics-token", "s3cr3t"]);
    let (status, body) = server.get("/metrics", &[]);
    assert_eq!(status, 401, "{body}");
    assert!(body.contains("metrics-token") || body.contains("Bearer"), "{body}");
}

#[test]
fn a_request_with_the_wrong_bearer_token_is_rejected() {
    let temp = TempDir::new("wrong-token");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &["--metrics-token", "s3cr3t"]);
    let (status, _) = server.get("/metrics", &[("Authorization", "Bearer wrong")]);
    assert_eq!(status, 401);
}

#[test]
fn a_request_with_the_right_bearer_token_succeeds() {
    let temp = TempDir::new("right-token");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &["--metrics-token", "s3cr3t"]);
    let (status, body) = server.get("/metrics", &[("Authorization", "Bearer s3cr3t")]);
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("linkc_"), "{body}");
}

#[test]
fn service_api_key_and_metrics_token_are_independent_layers() {
    // Configurar los DOS a la vez: --service-api-key sigue exigiendo su
    // propio header (ya cubre /metrics desde antes de este ítem), Y
    // --metrics-token exige el Bearer -- ninguno de los dos reemplaza al
    // otro, ambos tienen que pasar.
    let temp = TempDir::new("both");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &["--service-api-key", "gw-secret", "--metrics-token", "prom-token"]);

    let (status, _) = server.get("/metrics", &[("Authorization", "Bearer prom-token")]);
    assert_eq!(status, 401, "falta X-Service-Api-Key, tiene que seguir rechazando");

    let (status, _) = server.get("/metrics", &[("X-Service-Api-Key", "gw-secret")]);
    assert_eq!(status, 401, "falta el Bearer, tiene que seguir rechazando");

    let (status, body) = server.get("/metrics", &[("X-Service-Api-Key", "gw-secret"), ("Authorization", "Bearer prom-token")]);
    assert_eq!(status, 200, "con las dos, tiene que pasar: {body}");
}

#[test]
fn health_stays_reachable_without_the_token() {
    let temp = TempDir::new("health-exempt");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &["--metrics-token", "s3cr3t"]);
    assert_eq!(server.get("/health", &[]).0, 200);
}

#[test]
fn link_metrics_token_env_var_works_the_same_as_the_flag() {
    let temp = TempDir::new("env-var");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start_with_env(&src, &[], &[("LINK_METRICS_TOKEN", "from-env")]);
    assert_eq!(server.get("/metrics", &[]).0, 401);
    assert_eq!(server.get("/metrics", &[("Authorization", "Bearer from-env")]).0, 200);
}

#[test]
fn an_empty_string_flag_value_behaves_like_the_flag_was_never_passed() {
    let temp = TempDir::new("empty-flag");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &["--metrics-token", ""]);
    assert_eq!(server.get("/metrics", &[]).0, 200);
}
