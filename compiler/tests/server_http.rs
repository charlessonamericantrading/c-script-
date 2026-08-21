// Tests de integración: `linkc serve` como SUBPROCESO REAL hablando HTTP de
// verdad por un TcpStream (sin cliente ts/node de por medio) -- mismo
// espíritu que tests/lsp_stdio.rs para el protocolo LSP, acá para el
// servidor RPC.
//
// Motivo concreto de esta ronda (auditoría post-push): el demo insignia
// (`frontend/src/main.ts`) llamaba a `Users.update`/`Users.remove` sin
// haberse logueado antes -- quedó desactualizado cuando auth v0
// (GRAMMAR.md §3.14) agregó `@requires(Role.Admin)` a esos dos rpc, y
// ningún test existente lo detectaba porque los tests unitarios de
// `runtime/mod.rs` invocan `invoke_rpc_with_sessions` in-process (con un
// token ya resuelto a mano), nunca a través del servidor HTTP real con el
// gate de autorización (`check_auth_gate` en `runtime/server.rs`) en el
// medio. Este archivo fija ese contrato contra el BINARIO real: sin token
// es 401, con el token de un login real es 200.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

/// Pide al SO un puerto libre y lo suelta de inmediato -- mismo patrón
/// ("bind a :0, leé el puerto, soltalo") que usan crates como `portpicker`;
/// la ventana de carrera entre soltar el listener acá y que el subproceso
/// lo tome es la misma que ya asume cualquier test de este estilo, y no se
/// evita sin capturar el puerto real que `tiny_http` eligió desde DENTRO
/// del proceso servidor (que hoy no expone -- ver `serve()` en
/// `runtime/server.rs`, imprime el puerto que recibió por parámetro, no el
/// que `Server::http` bindeó).
fn free_port() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bindear un puerto efímero");
    listener.local_addr().expect("local_addr").port()
}

/// No alcanza con que `TcpStream::connect` haya funcionado: el backlog de
/// un socket en `listen()` puede aceptar la conexión a nivel de SO ANTES
/// de que el proceso servidor haya llegado a llamar `accept()` de verdad
/// (semántica POSIX estándar, no un bug de este servidor) -- confirmado
/// como la causa real de un "Connection reset by peer" intermitente en CI
/// (`ubuntu-latest`, bajo más carga que en desarrollo local) cuando un
/// test mandaba su request real apenas `connect()` daba `Ok`. Un
/// round-trip HTTP completo (mandar una request real, leer al menos un
/// byte de respuesta) es la única señal confiable de que el servidor ya
/// está listo para servir, así que eso es lo que se reintenta acá.
fn wait_for_port(port: u16) {
    let mut buf = [0u8; 1];
    for _ in 0..200 {
        if let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) {
            let ready = stream
                .write_all(b"GET /Users/list HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
                .is_ok()
                && matches!(stream.read(&mut buf), Ok(n) if n > 0);
            if ready {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("'linkc serve' no abrió el puerto {port} a tiempo (o nunca completó un round-trip HTTP real)");
}

struct ServeProcess {
    child: Child,
    port: u16,
}

impl ServeProcess {
    /// Copia `examples/users.link` a un directorio temporal propio de este
    /// test -- así cada test arranca con una base SQLite (GRAMMAR.md §3.17)
    /// completamente vacía y aislada de cualquier otro test o corrida
    /// manual, en vez de pelear por `examples/users.db` compartido.
    fn start_with_flagship_example(dir_suffix: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("cscript-server-http-integration-{dir_suffix}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("crear directorio temporal");
        let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../examples/users.link");
        let link_path = dir.join("users.link");
        std::fs::copy(&src, &link_path).expect("copiar examples/users.link al directorio temporal");
        Self::start_at_path(link_path, &[])
    }

    /// Como `start_with_flagship_example`, pero con un programa propio en
    /// vez de `examples/users.link` -- para casos (ej. OR de roles en
    /// `@requires`, GRAMMAR.md §3.49) que necesitan un `enum`/`service`
    /// específico del test, no el del demo insignia.
    fn start_with_program(dir_suffix: &str, source: &str) -> Self {
        Self::start_with_program_and_args(dir_suffix, source, &[])
    }

    /// Como `start_with_program`, pero con flags extra después del puerto
    /// (ej. `--session-ttl 2s`, GRAMMAR.md §3.50) -- para casos que
    /// necesitan configurar `linkc serve` más allá de programa+puerto.
    fn start_with_program_and_args(dir_suffix: &str, source: &str, extra_args: &[&str]) -> Self {
        let dir = std::env::temp_dir().join(format!("cscript-server-http-integration-{dir_suffix}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("crear directorio temporal");
        let link_path = dir.join("app.link");
        std::fs::write(&link_path, source).expect("escribir el programa del test");
        Self::start_at_path(link_path, extra_args)
    }

    fn start_at_path(link_path: PathBuf, extra_args: &[&str]) -> Self {
        let port = free_port();
        let child = Command::new(env!("CARGO_BIN_EXE_linkc"))
            .arg("serve")
            .arg(&link_path)
            .arg(port.to_string())
            .args(extra_args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("no se pudo iniciar 'linkc serve'");

        wait_for_port(port);
        ServeProcess { child, port }
    }

    /// POST /{service}/{method} con un body JSON y un token bearer
    /// opcional -- HTTP de verdad sobre un TcpStream propio por request
    /// (Connection: close), sin ningún cliente ts/node de por medio.
    fn post(&self, path: &str, body: &Value, token: Option<&str>) -> (u16, Value) {
        let mut stream =
            TcpStream::connect(("127.0.0.1", self.port)).expect("conectar al servidor 'linkc serve' real");
        let body_str = body.to_string();
        let mut request = format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
            self.port,
            body_str.len()
        );
        if let Some(t) = token {
            request.push_str(&format!("Authorization: Bearer {t}\r\n"));
        }
        request.push_str("\r\n");
        request.push_str(&body_str);
        stream.write_all(request.as_bytes()).expect("escribir la request HTTP");
        stream.flush().ok();

        let mut reader = BufReader::new(stream);
        let mut status_line = String::new();
        reader.read_line(&mut status_line).expect("leer la línea de estado HTTP");
        let status: u16 = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| panic!("línea de estado HTTP inesperada: {status_line:?}"));

        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            let n = reader.read_line(&mut line).expect("leer un header de la respuesta");
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
        reader.read_exact(&mut buf).expect("leer el body de la respuesta");
        let json = if buf.is_empty() { Value::Null } else { serde_json::from_slice(&buf).expect("el body debe ser JSON") };
        (status, json)
    }

    /// Termina el proceso hijo por su PID exacto (`Child::kill`, jamás un
    /// kill por nombre de imagen) -- `serve()` corre un loop infinito sobre
    /// `incoming_requests()` sin ningún camino de apagado limpio por señal
    /// o EOF, a diferencia de `linkc lsp` (que sí cierra solo al ver EOF en
    /// stdin).
    fn shutdown(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn list_does_not_require_auth_over_a_real_subprocess() {
    let server = ServeProcess::start_with_flagship_example("list-no-auth");
    let (status, body) = server.post("/Users/list", &json!({}), None);
    assert_eq!(status, 200, "list no tiene @requires ni @authenticated -- body: {body:?}");
    assert_eq!(body, json!([]), "una base recién creada debe estar vacía");
    server.shutdown();
}

#[test]
fn first_user_created_in_an_empty_database_is_admin_over_a_real_subprocess() {
    let server = ServeProcess::start_with_flagship_example("first-user-admin");
    let (status, body) =
        server.post("/Users/create", &json!({"input": {"name": "Ada", "email": "ada@example.com", "createdAt": "2026-01-01T00:00:00.000Z"}}), None);
    assert_eq!(status, 200, "body: {body:?}");
    assert_eq!(body["type"], "Ok", "body: {body:?}");
    assert_eq!(
        body["value"]["role"], "Admin",
        "el primer usuario de una base vacía debe ser Admin (ver 'validate' en examples/users.link): {body:?}"
    );
    server.shutdown();
}

#[test]
fn admin_gated_rpcs_reject_without_a_token_and_succeed_with_a_real_login_token_over_a_real_subprocess() {
    let server = ServeProcess::start_with_flagship_example("admin-gate");

    let (status, created) =
        server.post("/Users/create", &json!({"input": {"name": "Ada", "email": "ada@example.com", "createdAt": "2026-01-01T00:00:00.000Z"}}), None);
    assert_eq!(status, 200, "body: {created:?}");
    let id = created["value"]["id"].as_i64().expect("el usuario creado debe tener id");

    // Sin token: 401 en ambos rpc protegidos -- el bug real de esta ronda
    // era exactamente que el demo TS nunca pasaba por esta rama con éxito
    // porque tampoco mandaba token, pero el servidor SIEMPRE la exigió
    // correctamente desde que auth v0 se agregó.
    let (status, body) = server.post("/Users/update", &json!({"id": id, "patch": {"name": "Ada L."}}), None);
    assert_eq!(status, 401, "update sin token debe rechazar: {body:?}");
    let (status, body) = server.post("/Users/remove", &json!({"id": 999}), None);
    assert_eq!(status, 401, "remove sin token debe rechazar: {body:?}");

    // Login real (busca por email, devuelve un token de sesión opaco con
    // el rol real del usuario -- GRAMMAR.md §3.14) en vez de fabricar un
    // token a mano.
    let (status, token_json) = server.post("/Users/login", &json!({"email": "ada@example.com"}), None);
    assert_eq!(status, 200, "body: {token_json:?}");
    let token = token_json.as_str().expect("login debe devolver un token string cuando el email matchea").to_string();

    let (status, updated) =
        server.post("/Users/update", &json!({"id": id, "patch": {"name": "Ada L."}}), Some(&token));
    assert_eq!(status, 200, "update con un token de Admin real debe aceptar: {updated:?}");
    assert_eq!(updated["name"], "Ada L.");

    let (status, removed) = server.post("/Users/remove", &json!({"id": 999}), Some(&token));
    assert_eq!(status, 200, "remove con un token de Admin real debe aceptar: {removed:?}");
    assert_eq!(removed, json!(false), "999 no existe -- remove debe devolver false, no fallar");

    server.shutdown();
}

const OR_ROLES_PROGRAM: &str = r#"
enum Role { Admin, Agent, Member }

service Auth {
  rpc loginAs(role: Role) -> String {
    auth.createSession(role)
  }
}

service Dashboard {
  @requires(Role.Admin | Role.Agent)
  rpc sharedPanel() -> String { "panel compartido" }

  @requires(Role.Admin)
  rpc adminOnly() -> String { "solo admin" }
}
"#;

#[test]
fn requires_with_or_of_roles_accepts_any_named_role_and_rejects_the_rest() {
    // GRAMMAR.md §3.49: `@requires(Role.Admin | Role.Agent)` -- antes de
    // esta ronda, un endpoint compartido entre dos roles (ej. un dashboard
    // que ven tanto Admin como Agent) no tenía forma de expresarse sin
    // duplicar el rpc entero para cada rol. Contra el servidor real, no
    // solo el checker: dos logins reales con roles distintos, ambos
    // aceptados por el mismo `@requires`, un tercer rol rechazado, y el
    // `@requires` de un solo rol (`adminOnly`) sin cambios de
    // comportamiento -- Agent sigue sin poder entrar ahí.
    let server = ServeProcess::start_with_program("or-roles", OR_ROLES_PROGRAM);

    let (_, admin_token) = server.post("/Auth/loginAs", &json!({"role": "Admin"}), None);
    let admin_token = admin_token.as_str().unwrap().to_string();
    let (_, agent_token) = server.post("/Auth/loginAs", &json!({"role": "Agent"}), None);
    let agent_token = agent_token.as_str().unwrap().to_string();
    let (_, member_token) = server.post("/Auth/loginAs", &json!({"role": "Member"}), None);
    let member_token = member_token.as_str().unwrap().to_string();

    let (status, body) = server.post("/Dashboard/sharedPanel", &json!({}), Some(&admin_token));
    assert_eq!(status, 200, "Admin es una de las alternativas: {body:?}");
    let (status, body) = server.post("/Dashboard/sharedPanel", &json!({}), Some(&agent_token));
    assert_eq!(status, 200, "Agent es la OTRA alternativa: {body:?}");
    let (status, body) = server.post("/Dashboard/sharedPanel", &json!({}), Some(&member_token));
    assert_eq!(status, 403, "Member no está en ninguna alternativa: {body:?}");
    let (status, body) = server.post("/Dashboard/sharedPanel", &json!({}), None);
    assert_eq!(status, 401, "sin token, ni siquiera llega a evaluar el rol: {body:?}");

    // `@requires(Role.Admin)` de un solo rol sigue funcionando exactamente
    // igual que siempre -- esta ronda no le cambió nada a ese caso.
    let (status, body) = server.post("/Dashboard/adminOnly", &json!({}), Some(&agent_token));
    assert_eq!(status, 403, "adminOnly no incluye Agent, sin OR de por medio: {body:?}");
    let (status, body) = server.post("/Dashboard/adminOnly", &json!({}), Some(&admin_token));
    assert_eq!(status, 200, "body: {body:?}");

    server.shutdown();
}

const TTL_PROGRAM: &str = r#"
enum Role { Admin }

service Auth {
  rpc loginAs(role: Role) -> String {
    auth.createSession(role)
  }
}

service Protected {
  @requires(Role.Admin)
  rpc secret() -> String { "shh" }
}
"#;

#[test]
fn a_session_created_under_session_ttl_expires_on_its_own_over_a_real_subprocess() {
    // GRAMMAR.md §3.50: `--session-ttl` -- antes de esta ronda, una sesión
    // vivía hasta `destroySession()` o hasta reiniciar el proceso, sin
    // forma de expresar "sesión válida 7 días". Contra el binario real,
    // con un TTL corto: el token sirve enseguida, y después de vencido el
    // servidor lo trata igual que "nunca hubo token" (401), sin que haga
    // falta llamar `destroySession` a mano.
    let server = ServeProcess::start_with_program_and_args("session-ttl", TTL_PROGRAM, &["--session-ttl", "2s"]);

    let (_, token) = server.post("/Auth/loginAs", &json!({"role": "Admin"}), None);
    let token = token.as_str().unwrap().to_string();

    let (status, body) = server.post("/Protected/secret", &json!({}), Some(&token));
    assert_eq!(status, 200, "recién logueado, el token todavía es válido: {body:?}");

    std::thread::sleep(Duration::from_secs(3));

    let (status, body) = server.post("/Protected/secret", &json!({}), Some(&token));
    assert_eq!(status, 401, "pasado el TTL, el mismo token deja de servir: {body:?}");

    server.shutdown();
}
