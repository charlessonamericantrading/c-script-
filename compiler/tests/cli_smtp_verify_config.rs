// `smtp.verifyConfig(config)` (GRAMMAR.md §3.288, PLAN.md §9.24 Fase 2 ítem
// E4): "el admin prueba credenciales SMTP sin mandar nada" -- conecta + EHLO
// + AUTH (`lettre::SmtpTransport::test_connection`, NOOP tras autenticar),
// nunca llega a `MAIL FROM`/`DATA`.
//
// Límite honesto, MISMO que `sendWithConfig` ya documenta (GRAMMAR.md
// §3.265): `secure: true` es TLS implícito y `secure: false` es STARTTLS --
// las dos formas que expone son SIEMPRE cifradas, así que un servidor de
// mentira en texto plano (como el de `cli_smtp.rs`) no puede completar una
// conexión real con ninguna de las dos, y un fake server que hable TLS de
// verdad queda fuera de este alcance. Lo que SÍ se prueba acá contra el
// BINARIO real: los fallos alcanzables ANTES de necesitar un handshake TLS
// completo (host inalcanzable) y el rechazo en compilación de un `config`
// con forma incorrecta.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

const PROGRAM: &str = r#"
service Sys {
  rpc checkSmtp(host: String, port: Int, user: String, pass: String, secure: Bool) -> String {
    smtp.verifyConfig({ host: host, port: port, user: user, pass: pass, secure: secure });
    "ok"
  }
}
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-smtp-verify-{name}-{}-{}",
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
            .expect("iniciar linkc serve");
        wait_for_port(port);
        Serve { child, port }
    }

    fn post(&self, path: &str, body: &str) -> (u16, String) {
        match ureq::post(&format!("http://127.0.0.1:{}/{path}", self.port)).set("Content-Type", "application/json").send_string(body) {
            Ok(r) => (r.status(), r.into_string().unwrap_or_default()),
            Err(ureq::Error::Status(status, r)) => (status, r.into_string().unwrap_or_default()),
            Err(e) => panic!("{path} falló de red: {e}"),
        }
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn verify_config_against_an_unreachable_host_fails_cleanly_not_with_a_panic() {
    let temp = TempDir::new("unreachable");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src);
    let dead_port = free_port();

    let (status, body) =
        server.post("Sys/checkSmtp", &serde_json::json!({"host": "127.0.0.1", "port": dead_port, "user": "u", "pass": "p", "secure": false}).to_string());
    assert_eq!(status, 500, "{body}");
    assert!(!body.contains("panicked"), "una conexión caída es una condición operativa normal, no un panic: {body}");
    assert!(body.contains("verifyConfig"), "{body}");
}

#[test]
fn verify_config_against_an_unreachable_host_with_implicit_tls_also_fails_cleanly() {
    // Mismo caso pero `secure: true` (TLS implícito, el otro camino del
    // builder) -- las dos ramas de `verify_smtp_config` tienen que fallar
    // limpio ante un host inalcanzable, no solo la de STARTTLS.
    let temp = TempDir::new("unreachable-tls");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src);
    let dead_port = free_port();

    let (status, body) =
        server.post("Sys/checkSmtp", &serde_json::json!({"host": "127.0.0.1", "port": dead_port, "user": "u", "pass": "p", "secure": true}).to_string());
    assert_eq!(status, 500, "{body}");
    assert!(!body.contains("panicked"), "{body}");
}

#[test]
fn verify_config_rejects_a_config_missing_a_field_at_compile_time() {
    let temp = TempDir::new("badshape");
    let src = temp.write(
        "app.link",
        r#"
service Sys {
  rpc bad() -> Void {
    smtp.verifyConfig({ host: "x", port: 465, user: "u", pass: "p" })
  }
}
"#,
    );
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("build").arg(&src).arg(temp.0.join("gen")).output().expect("linkc build");
    assert!(!out.status.success(), "falta 'secure' -- tiene que rechazar la compilación");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("panicked"), "{stderr}");
}

#[test]
fn verify_config_rejects_wrong_argument_count() {
    let temp = TempDir::new("badargs");
    let src = temp.write(
        "app.link",
        r#"
service Sys {
  rpc bad() -> Void {
    smtp.verifyConfig({ host: "x", port: 465, user: "u", pass: "p", secure: true }, "extra")
  }
}
"#,
    );
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("build").arg(&src).arg(temp.0.join("gen")).output().expect("linkc build");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("verifyConfig"), "{stderr}");
}
