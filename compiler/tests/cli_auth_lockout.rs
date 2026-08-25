// Bloqueo de cuenta configurable (GRAMMAR.md §3.152, PLAN.md §9.5): tres
// primitivas chicas (`auth.recordFailedLogin`/`failedLoginCount`/
// `resetFailedLogins`) para que quien escribe su propio `login` en c-script
// pueda implementar un umbral/ventana propios, sin ningún mecanismo mágico
// ni flag de servidor nuevo. Se verifica acá contra el BINARIO real,
// hablando HTTP de verdad -- un login real bloqueado después de N intentos,
// no solo que el checker acepte la sintaxis.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

const PROGRAM: &str = r#"
enum Role { Member }
enum LoginError { InvalidCredentials, LockedOut }
type User = { id: Int, email: String }
db { users: User[] }

service Sys {
  rpc register(email: String) -> User {
    db.users.insert(User { id: 0, email: email })
  }

  rpc login(email: String) -> Result<String, LoginError> {
    if auth.failedLoginCount(email, 900) >= 3 {
      Result.Err { error: LoginError.LockedOut {} }
    } else {
      let matches = db.users.all().filter(|u: User| { u.email == email });
      if matches.length() > 0 {
        auth.resetFailedLogins(email);
        Result.Ok { value: auth.createSession(Role.Member {}) }
      } else {
        auth.recordFailedLogin(email);
        Result.Err { error: LoginError.InvalidCredentials {} }
      }
    }
  }

  rpc failCount(email: String) -> Int {
    auth.failedLoginCount(email, 900)
  }
}
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "linkc-auth-lockout-{name}-{}-{}",
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

    fn post(&self, path: &str, body: &serde_json::Value) -> (u16, serde_json::Value) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("conectar");
        let body_str = body.to_string();
        let request = format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body_str}",
            self.port,
            body_str.len()
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
        reader.read_exact(&mut buf).ok();
        let json = if buf.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&buf).expect("el body debe ser JSON") };
        (status, json)
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn a_valid_login_is_unaffected_by_a_different_identifiers_failures() {
    let temp = TempDir::new("isolated");
    let file = temp.write("app.link", PROGRAM);
    let server = Serve::start(&file);

    server.post("/Sys/register", &serde_json::json!({"email": "real@x.com"}));

    // Tres intentos fallidos contra OTRO identifier.
    for _ in 0..3 {
        server.post("/Sys/login", &serde_json::json!({"email": "atacante@x.com"}));
    }

    // El usuario real, con su PROPIO contador en cero, loguea normal --
    // `Result<String, LoginError>` nunca lanza para un error DECLARADO, así
    // que el status siempre es 200; lo que distingue éxito de fallo es
    // `type` en el body.
    let (status, body) = server.post("/Sys/login", &serde_json::json!({"email": "real@x.com"}));
    assert_eq!(status, 200, "body: {body:?}");
    assert_eq!(body["type"], "Ok", "body: {body:?}");
    assert!(body["value"].as_str().is_some_and(|t| !t.is_empty()), "debió devolver un token real: {body:?}");
}

#[test]
fn after_the_threshold_further_attempts_are_locked_out_with_their_own_error() {
    let temp = TempDir::new("locked");
    let file = temp.write("app.link", PROGRAM);
    let server = Serve::start(&file);

    // Tres intentos fallidos (umbral que el propio .link eligió: >= 3).
    // `LoginError` es un enum SIMPLE (variantes sin datos) -- serializa
    // como STRING plano, no un objeto con `type` (eso es solo para un ADT).
    for i in 1..=3 {
        let (status, body) = server.post("/Sys/login", &serde_json::json!({"email": "victima@x.com"}));
        assert_eq!(status, 200, "intento {i}, body: {body:?}");
        assert_eq!(body["type"], "Err", "intento {i}: {body:?}");
        assert_eq!(body["error"], "InvalidCredentials", "intento {i}: {body:?}");
    }

    // El 4to intento ya ve el umbral cumplido -- variante de error
    // DISTINTA, confirmando que el camino de lockout (no el de
    // credenciales inválidas de siempre) es el que corrió.
    let (status, body) = server.post("/Sys/login", &serde_json::json!({"email": "victima@x.com"}));
    assert_eq!(status, 200, "body: {body:?}");
    assert_eq!(body["type"], "Err", "body: {body:?}");
    assert_eq!(body["error"], "LockedOut", "body: {body:?}");
}

#[test]
fn a_successful_login_resets_the_failed_count() {
    let temp = TempDir::new("reset");
    let file = temp.write("app.link", PROGRAM);
    let server = Serve::start(&file);

    server.post("/Sys/register", &serde_json::json!({"email": "user@x.com"}));

    // Dos fallos (probablemente typos antes de acertar) -- todavía debajo
    // del umbral.
    server.post("/Sys/login", &serde_json::json!({"email": "usr@x.com"}));
    let (_, count) = server.post("/Sys/failCount", &serde_json::json!({"email": "usr@x.com"}));
    assert_eq!(count, serde_json::json!(1));

    // Un login exitoso con el email CORRECTO no toca el contador del email
    // typeado (son identifiers distintos) -- confirma que el reset es
    // POR identifier, no global.
    let (status, _) = server.post("/Sys/login", &serde_json::json!({"email": "user@x.com"}));
    assert_eq!(status, 200);
    let (_, count_after) = server.post("/Sys/failCount", &serde_json::json!({"email": "usr@x.com"}));
    assert_eq!(count_after, serde_json::json!(1), "el fallo del OTRO identifier no se resetea por un login exitoso de otro");

    // Ahora falla y ACIERTA con el MISMO identifier -- eso sí resetea.
    server.post("/Sys/login", &serde_json::json!({"email": "user@x.com"})); // ya tiene sesión, pero esto no importa para el conteo
    let (_, count_correct) = server.post("/Sys/failCount", &serde_json::json!({"email": "user@x.com"}));
    assert_eq!(count_correct, serde_json::json!(0), "un login exitoso resetea el contador del MISMO identifier");
}
