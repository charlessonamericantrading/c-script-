// Log de auditoría de autorización estructurado (GRAMMAR.md §3.148,
// PLAN.md §9.5): "quién llamó a qué rpc, con qué rol, y si se permitió o
// denegó". Antes de esta ronda, `log_done` (§3.122) ya logueaba
// method/status/duration_ms, pero nada sobre la DECISIÓN de autorización en
// sí -- auditar quién tuvo acceso (o no) a un rpc protegido exigía cruzar
// el status code con otra fuente. Se verifica acá contra el BINARIO real,
// leyendo su stdout de verdad.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const PROGRAM: &str = r#"
enum Role { Admin, Member }

service Sys {
  rpc ping() -> String {
    "pong"
  }

  rpc loginAsAdmin() -> String {
    auth.createSessionWithId(Role.Admin {}, 42)
  }

  rpc loginAsMember() -> String {
    auth.createSession(Role.Member {})
  }

  @authenticated
  rpc anyAuth() -> String {
    "ok"
  }

  @requires(Role.Admin)
  rpc adminOnly() -> String {
    "ok"
  }
}
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "linkc-auth-audit-{name}-{}-{}",
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

/// POST /{path} con un body JSON y un token bearer opcional, devuelve el
/// body de la respuesta parseado -- necesario acá (a diferencia de
/// cli_log_format.rs, que solo le importa stdout) porque `loginAsAdmin`/
/// `loginAsMember` devuelven el token real que las siguientes requests
/// necesitan mandar.
fn post(port: u16, path: &str, token: Option<&str>) -> serde_json::Value {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("conectar");
    let mut request =
        format!("POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n");
    if let Some(t) = token {
        request.push_str(&format!("Authorization: Bearer {t}\r\n"));
    }
    request.push_str("\r\n{}");
    stream.write_all(request.as_bytes()).expect("escribir request");
    stream.flush().ok();

    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader.read_line(&mut status_line).expect("línea de estado");
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
    reader.read_exact(&mut buf).ok();
    if buf.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&buf).unwrap_or(serde_json::Value::Null) }
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
        cmd.stdout(Stdio::piped()).stderr(Stdio::null());
        let child = cmd.spawn().expect("iniciar 'linkc serve'");
        wait_for_port(port);
        Serve { child, port }
    }

    /// Termina el proceso y devuelve TODO lo que imprimió hasta ahí --
    /// mismo margen chico que `cli_log_format.rs` antes de matar el
    /// proceso, para darle tiempo a la última línea de loguearse.
    fn finish_and_collect_stdout(mut self) -> String {
        std::thread::sleep(Duration::from_millis(100));
        let _ = self.child.kill();
        let output = self.child.wait_with_output().expect("esperar a que 'linkc serve' termine");
        String::from_utf8_lossy(&output.stdout).to_string()
    }
}

fn json_lines(stdout: &str) -> Vec<serde_json::Value> {
    stdout.lines().filter(|l| l.trim_start().starts_with('{')).filter_map(|l| serde_json::from_str(l).ok()).collect()
}

#[test]
fn a_denied_request_logs_the_role_and_allowed_false_in_json_format() {
    let temp = TempDir::new("denied");
    let file = temp.write("app.link", PROGRAM);
    let server = Serve::start(&file, &["--log-format", "json"]);

    let admin_token = post(server.port, "/Sys/loginAsAdmin", None).as_str().expect("token").to_string();
    let member_token = post(server.port, "/Sys/loginAsMember", None).as_str().expect("token").to_string();
    // Member pidiendo un rpc que exige Admin -- 403, denegado.
    post(server.port, "/Sys/adminOnly", Some(&member_token));

    let stdout = server.finish_and_collect_stdout();
    let lines = json_lines(&stdout);
    let denied = lines
        .iter()
        .find(|v| v.get("method") == Some(&serde_json::json!("Sys.adminOnly")) && v["status"] == 403)
        .unwrap_or_else(|| panic!("no se encontró la línea de adminOnly denegado: {stdout}"));
    assert_eq!(denied["auth_role"], "Member", "{denied:?}");
    assert_eq!(denied["auth_allowed"], false, "{denied:?}");
    let _ = admin_token; // usado en el otro test; se loguea igual acá pero no hace falta reusarlo
}

#[test]
fn an_allowed_request_logs_the_role_user_id_and_allowed_true() {
    let temp = TempDir::new("allowed");
    let file = temp.write("app.link", PROGRAM);
    let server = Serve::start(&file, &["--log-format", "json"]);

    let admin_token = post(server.port, "/Sys/loginAsAdmin", None).as_str().expect("token").to_string();
    post(server.port, "/Sys/adminOnly", Some(&admin_token));

    let stdout = server.finish_and_collect_stdout();
    let lines = json_lines(&stdout);
    let allowed = lines
        .iter()
        .find(|v| v.get("method") == Some(&serde_json::json!("Sys.adminOnly")) && v["status"] == 200)
        .unwrap_or_else(|| panic!("no se encontró la línea de adminOnly permitido: {stdout}"));
    assert_eq!(allowed["auth_role"], "Admin", "{allowed:?}");
    assert_eq!(allowed["auth_user_id"], 42, "{allowed:?}");
    assert_eq!(allowed["auth_allowed"], true, "{allowed:?}");
}

#[test]
fn a_request_without_a_token_logs_a_null_role_and_allowed_false() {
    let temp = TempDir::new("no-token");
    let file = temp.write("app.link", PROGRAM);
    let server = Serve::start(&file, &["--log-format", "json"]);

    post(server.port, "/Sys/anyAuth", None);

    let stdout = server.finish_and_collect_stdout();
    let lines = json_lines(&stdout);
    let denied = lines
        .iter()
        .find(|v| v.get("method") == Some(&serde_json::json!("Sys.anyAuth")) && v["status"] == 401)
        .unwrap_or_else(|| panic!("no se encontró la línea de anyAuth sin token: {stdout}"));
    assert!(denied["auth_role"].is_null(), "{denied:?}");
    assert_eq!(denied["auth_allowed"], false, "{denied:?}");
}

/// Un rpc PÚBLICO (sin `@authenticated`/`@requires`) no genera ningún ruido
/// de auditoría -- no hay ninguna decisión de autorización que registrar.
#[test]
fn a_public_rpc_never_carries_auth_fields() {
    let temp = TempDir::new("public");
    let file = temp.write("app.link", PROGRAM);
    let server = Serve::start(&file, &["--log-format", "json"]);

    post(server.port, "/Sys/ping", None);

    let stdout = server.finish_and_collect_stdout();
    let lines = json_lines(&stdout);
    let done = lines
        .iter()
        .find(|v| v.get("method") == Some(&serde_json::json!("Sys.ping")) && v["status"] == 200)
        .unwrap_or_else(|| panic!("no se encontró la línea de ping: {stdout}"));
    assert!(done.get("auth_role").is_none(), "{done:?}");
    assert!(done.get("auth_allowed").is_none(), "{done:?}");
}

/// Mismo criterio en modo texto (default) -- `auth_role=...`/
/// `auth_user_id=...`/`auth_allowed=...` como pares clave=valor, mismo
/// estilo que el resto de la línea.
#[test]
fn text_format_includes_the_same_three_fields_as_key_value_pairs() {
    let temp = TempDir::new("text-format");
    let file = temp.write("app.link", PROGRAM);
    let server = Serve::start(&file, &[]);

    let admin_token = post(server.port, "/Sys/loginAsAdmin", None).as_str().expect("token").to_string();
    post(server.port, "/Sys/adminOnly", Some(&admin_token));

    let stdout = server.finish_and_collect_stdout();
    let line = stdout
        .lines()
        .find(|l| l.contains("method=Sys.adminOnly") && l.contains("status=200"))
        .unwrap_or_else(|| panic!("no se encontró la línea de adminOnly permitido: {stdout}"));
    assert!(line.contains("auth_role=\"Admin\""), "{line}");
    assert!(line.contains("auth_user_id=42"), "{line}");
    assert!(line.contains("auth_allowed=true"), "{line}");
}
