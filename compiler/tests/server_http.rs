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

    /// Como `post`, pero mandando además un header `Idempotency-Key`
    /// (GRAMMAR.md §3.140) -- duplica el armado/lectura de la request en
    /// vez de generalizar `post` (usado por decenas de tests de este
    /// archivo que no necesitan tocar headers), para no arriesgar ninguno
    /// de ellos con un cambio de firma compartido.
    fn post_with_idempotency_key(&self, path: &str, body: &Value, key: &str) -> (u16, Value) {
        let mut stream =
            TcpStream::connect(("127.0.0.1", self.port)).expect("conectar al servidor 'linkc serve' real");
        let body_str = body.to_string();
        let request = format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nIdempotency-Key: {key}\r\nConnection: close\r\n\r\n{body_str}",
            self.port,
            body_str.len()
        );
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

    /// Como `post`, pero deja elegir si mandar `Accept-Encoding: gzip` y
    /// devuelve los headers crudos + el body SIN decodificar -- necesario
    /// para probar compresión (GRAMMAR.md §3.180), donde `post` (que asume
    /// que el body siempre es JSON de texto plano) no sirve: un body
    /// comprimido no parsea como JSON hasta pasar por `flate2::read::GzDecoder`.
    fn post_raw(&self, path: &str, body: &Value, accept_gzip: bool) -> (u16, Vec<(String, String)>, Vec<u8>) {
        let mut stream =
            TcpStream::connect(("127.0.0.1", self.port)).expect("conectar al servidor 'linkc serve' real");
        let body_str = body.to_string();
        let mut request = format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
            self.port,
            body_str.len()
        );
        if accept_gzip {
            request.push_str("Accept-Encoding: gzip\r\n");
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
        let mut headers = Vec::new();
        loop {
            let mut line = String::new();
            let n = reader.read_line(&mut line).expect("leer un header de la respuesta");
            if n == 0 || line.trim().is_empty() {
                break;
            }
            if let Some((k, v)) = line.trim().split_once(':') {
                let k = k.trim().to_string();
                let v = v.trim().to_string();
                if k.eq_ignore_ascii_case("content-length") {
                    content_length = v.parse().unwrap_or(0);
                }
                headers.push((k, v));
            }
        }
        let mut buf = vec![0u8; content_length];
        reader.read_exact(&mut buf).expect("leer el body de la respuesta");
        (status, headers, buf)
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

const CURRENT_ROLE_PROGRAM: &str = r#"
enum Role { Admin, Agent, Member }

service Auth {
  rpc loginAs(role: Role) -> String {
    auth.createSession(role)
  }
}

service Dashboard {
  @requires(Role.Admin | Role.Agent)
  rpc sharedPanel() -> String {
    let role = auth.currentRole();
    if role == "Admin" {
      "panel de administrador"
    } else {
      "panel de agente"
    }
  }

  rpc whoAmI() -> String? {
    auth.currentRole()
  }
}
"#;

#[test]
fn current_role_lets_a_shared_endpoint_behave_differently_per_role_over_a_real_subprocess() {
    // GRAMMAR.md §3.51: la brecha real que motivó esto -- "bloquea
    // cualquier endpoint que hoy se comporte distinto según si eres agent
    // o admin, no solo permitido/denegado". `sharedPanel` acepta los dos
    // roles (§3.49) pero responde DISTINTO según cuál autenticó -- eso es
    // lo nuevo que `auth.currentRole()` habilita.
    let server = ServeProcess::start_with_program("current-role", CURRENT_ROLE_PROGRAM);

    let (_, admin_token) = server.post("/Auth/loginAs", &json!({"role": "Admin"}), None);
    let admin_token = admin_token.as_str().unwrap().to_string();
    let (_, agent_token) = server.post("/Auth/loginAs", &json!({"role": "Agent"}), None);
    let agent_token = agent_token.as_str().unwrap().to_string();

    let (status, body) = server.post("/Dashboard/sharedPanel", &json!({}), Some(&admin_token));
    assert_eq!(status, 200);
    assert_eq!(body, "panel de administrador");
    let (status, body) = server.post("/Dashboard/sharedPanel", &json!({}), Some(&agent_token));
    assert_eq!(status, 200);
    assert_eq!(body, "panel de agente");

    // Disponible SIN ninguna anotación de auth en el rpc que lo llama --
    // `whoAmI` no tiene ni `@requires` ni `@authenticated`.
    let (status, body) = server.post("/Dashboard/whoAmI", &json!({}), Some(&admin_token));
    assert_eq!(status, 200);
    assert_eq!(body, "Admin");

    // Sin token, y con un token que nunca existió, dan lo mismo: null --
    // mismo criterio de indistinguibilidad que ya rige `role_for` (§3.50).
    let (status, body) = server.post("/Dashboard/whoAmI", &json!({}), None);
    assert_eq!(status, 200);
    assert_eq!(body, Value::Null);
    let (status, body) = server.post("/Dashboard/whoAmI", &json!({}), Some("un-token-que-nunca-existio"));
    assert_eq!(status, 200);
    assert_eq!(body, Value::Null);

    server.shutdown();
}

const CURRENT_USER_ID_PROGRAM: &str = r#"
enum Role { Admin, Member }

service Auth {
  rpc loginWithId(role: Role, userId: Int) -> String {
    auth.createSessionWithId(role, userId)
  }

  rpc loginWithoutId(role: Role) -> String {
    auth.createSession(role)
  }
}

service Users {
  rpc currentUserId() -> Int? {
    auth.currentUserId()
  }

  rpc currentRole() -> String? {
    auth.currentRole()
  }

  @authenticated
  rpc myProfile() -> String {
    let uid = auth.currentUserId();
    if uid == 42 {
      "perfil de usuario 42"
    } else {
      "otro usuario"
    }
  }
}
"#;

#[test]
fn current_user_id_returns_stored_user_id_over_a_real_subprocess() {
    // GRAMMAR.md §3.53: persistir y consultar la identidad (userId) del caller.
    let server = ServeProcess::start_with_program("current-user-id", CURRENT_USER_ID_PROGRAM);

    let (_, token_with_id) = server.post("/Auth/loginWithId", &json!({"role": "Member", "userId": 42}), None);
    let token_with_id = token_with_id.as_str().unwrap().to_string();

    let (_, token_without_id) = server.post("/Auth/loginWithoutId", &json!({"role": "Admin"}), None);
    let token_without_id = token_without_id.as_str().unwrap().to_string();

    // Con token que incluye userId:
    let (status, body) = server.post("/Users/currentUserId", &json!({}), Some(&token_with_id));
    assert_eq!(status, 200);
    assert_eq!(body, json!(42));

    let (status, body) = server.post("/Users/currentRole", &json!({}), Some(&token_with_id));
    assert_eq!(status, 200);
    assert_eq!(body, "Member");

    let (status, body) = server.post("/Users/myProfile", &json!({}), Some(&token_with_id));
    assert_eq!(status, 200);
    assert_eq!(body, "perfil de usuario 42");

    // Con token sin userId:
    let (status, body) = server.post("/Users/currentUserId", &json!({}), Some(&token_without_id));
    assert_eq!(status, 200);
    assert_eq!(body, Value::Null);

    let (status, body) = server.post("/Users/currentRole", &json!({}), Some(&token_without_id));
    assert_eq!(status, 200);
    assert_eq!(body, "Admin");

    // Sin token o con token inexistente:
    let (status, body) = server.post("/Users/currentUserId", &json!({}), None);
    assert_eq!(status, 200);
    assert_eq!(body, Value::Null);

    let (status, body) = server.post("/Users/currentUserId", &json!({}), Some("token-fantasma"));
    assert_eq!(status, 200);
    assert_eq!(body, Value::Null);

    server.shutdown();
}

// GRAMMAR.md §3.84: revocar TODAS las sesiones de un usuario a la vez
// (`auth.destroyAllSessions(userId)`), no solo la sesión que ya autenticó la
// request actual (eso es `destroySession`, sin argumentos). Gateado con
// `@requires(Role.Admin)` acá -- ese gate lo decide quien escribe el
// `.link`, `destroyAllSessions` en sí mismo no impone ninguna política.
const REVOKE_ALL_SESSIONS_PROGRAM: &str = r#"
enum Role { Admin, Member }

service Auth {
  rpc login(role: Role, userId: Int) -> String {
    auth.createSessionWithId(role, userId)
  }

  @requires(Role.Admin)
  rpc revokeUser(userId: Int) -> Int {
    auth.destroyAllSessions(userId)
  }
}

service Users {
  @authenticated
  rpc whoAmI() -> Int? {
    auth.currentUserId()
  }
}
"#;

#[test]
fn destroy_all_sessions_revokes_two_tokens_of_the_same_user_over_a_real_subprocess() {
    let server = ServeProcess::start_with_program("revoke-all-sessions", REVOKE_ALL_SESSIONS_PROGRAM);

    let admin_token = server.post("/Auth/login", &json!({"role": "Admin", "userId": 1}), None).1.as_str().unwrap().to_string();
    let victim_a = server.post("/Auth/login", &json!({"role": "Member", "userId": 7}), None).1.as_str().unwrap().to_string();
    let victim_b = server.post("/Auth/login", &json!({"role": "Member", "userId": 7}), None).1.as_str().unwrap().to_string();
    let survivor = server.post("/Auth/login", &json!({"role": "Member", "userId": 8}), None).1.as_str().unwrap().to_string();

    // Las tres sesiones funcionan antes de revocar nada.
    for tok in [&victim_a, &victim_b, &survivor] {
        let (status, _) = server.post("/Users/whoAmI", &json!({}), Some(tok));
        assert_eq!(status, 200, "token {tok} debería estar autenticado todavía");
    }

    let (status, count) = server.post("/Auth/revokeUser", &json!({"userId": 7}), Some(&admin_token));
    assert_eq!(status, 200);
    assert_eq!(count, json!(2), "user 7 tenía exactamente 2 sesiones abiertas");

    // Las DOS sesiones de user 7 dejan de autenticar -- mismo 401 que
    // cualquier token inexistente/vencido (GRAMMAR.md §3.50).
    for tok in [&victim_a, &victim_b] {
        let (status, _) = server.post("/Users/whoAmI", &json!({}), Some(tok));
        assert_eq!(status, 401, "token {tok} debería haber quedado revocado");
    }

    // La sesión de OTRO usuario no se toca.
    let (status, uid) = server.post("/Users/whoAmI", &json!({}), Some(&survivor));
    assert_eq!(status, 200);
    assert_eq!(uid, json!(8));

    server.shutdown();
}

// GRAMMAR.md §3.64: auth externo -- confiar en un JWT HS256 ya emitido por un
// backend existente, sin pasar por `auth.createSession(WithId)`. `--jwt-secret`
// (extra_args de `ServeProcess`) es lo único que hace falta para habilitarlo.

const JWT_PROGRAM: &str = r#"
enum Role { Admin, Member }

service Secured {
  @requires(Role.Admin)
  rpc adminOnly() -> String {
    "solo admin"
  }

  @authenticated
  rpc anyAuth() -> String {
    "cualquier rol autenticado"
  }

  rpc whoAmI() -> String? {
    auth.currentRole()
  }

  rpc myId() -> Int? {
    auth.currentUserId()
  }
}
"#;

/// Arma un JWT HS256 DE VERDAD -- mismo algoritmo que produciría
/// `jsonwebtoken` de Node o `PyJWT` de Python, no un atajo interno de este
/// repo -- para probar interoperabilidad real, no un round-trip contra el
/// propio código de este proyecto.
fn make_jwt(secret: &str, alg: &str, claims_json: &str) -> String {
    use base64::Engine;
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let header = format!(r#"{{"alg":"{alg}","typ":"JWT"}}"#);
    let header_b64 = engine.encode(header.as_bytes());
    let payload_b64 = engine.encode(claims_json.as_bytes());
    let signing_input = format!("{header_b64}.{payload_b64}");
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(signing_input.as_bytes());
    let sig_b64 = engine.encode(mac.finalize().into_bytes());
    format!("{signing_input}.{sig_b64}")
}

#[test]
fn a_jwt_with_the_right_role_satisfies_requires_over_a_real_subprocess() {
    let server = ServeProcess::start_with_program_and_args("jwt-role-ok", JWT_PROGRAM, &["--jwt-secret", "shh"]);

    let jwt = make_jwt("shh", "HS256", r#"{"role":"Admin","sub":7}"#);
    let (status, body) = server.post("/Secured/adminOnly", &json!({}), Some(&jwt));
    assert_eq!(status, 200, "body: {body:?}");
    assert_eq!(body, "solo admin");

    server.shutdown();
}

#[test]
fn a_jwt_with_the_wrong_role_is_rejected_over_a_real_subprocess() {
    let server = ServeProcess::start_with_program_and_args("jwt-role-bad", JWT_PROGRAM, &["--jwt-secret", "shh"]);

    let jwt = make_jwt("shh", "HS256", r#"{"role":"Member","sub":7}"#);
    let (status, _) = server.post("/Secured/adminOnly", &json!({}), Some(&jwt));
    assert_eq!(status, 403, "rol válido, pero no el que pide @requires(Role.Admin)");

    server.shutdown();
}

#[test]
fn a_jwt_satisfies_authenticated_regardless_of_its_role() {
    let server = ServeProcess::start_with_program_and_args("jwt-authenticated", JWT_PROGRAM, &["--jwt-secret", "shh"]);

    let jwt = make_jwt("shh", "HS256", r#"{"role":"Member","sub":1}"#);
    let (status, body) = server.post("/Secured/anyAuth", &json!({}), Some(&jwt));
    assert_eq!(status, 200, "body: {body:?}");
    assert_eq!(body, "cualquier rol autenticado");

    server.shutdown();
}

#[test]
fn auth_current_role_and_current_user_id_read_jwt_claims_over_a_real_subprocess() {
    let server = ServeProcess::start_with_program_and_args("jwt-claims", JWT_PROGRAM, &["--jwt-secret", "shh"]);

    let jwt = make_jwt("shh", "HS256", r#"{"role":"Admin","sub":"123"}"#);
    let (status, body) = server.post("/Secured/whoAmI", &json!({}), Some(&jwt));
    assert_eq!(status, 200);
    assert_eq!(body, "Admin");

    let (status, body) = server.post("/Secured/myId", &json!({}), Some(&jwt));
    assert_eq!(status, 200, "body: {body:?}");
    assert_eq!(body, json!(123), "'sub' como string de dígitos tiene que parsear a Int");

    server.shutdown();
}

#[test]
fn a_jwt_with_an_invalid_signature_is_rejected_over_a_real_subprocess() {
    let server = ServeProcess::start_with_program_and_args("jwt-bad-sig", JWT_PROGRAM, &["--jwt-secret", "shh"]);

    let jwt = make_jwt("secreto-equivocado", "HS256", r#"{"role":"Admin","sub":1}"#);
    let (status, _) = server.post("/Secured/adminOnly", &json!({}), Some(&jwt));
    assert_eq!(status, 401, "firma que no matchea: indistinguible de 'sin token' a propósito");

    server.shutdown();
}

#[test]
fn without_jwt_secret_configured_a_jwt_shaped_token_is_just_unauthenticated() {
    // Sin --jwt-secret: comportamiento IDÉNTICO al de antes de esta ronda --
    // un token con forma de JWT no se verifica nunca, cae a "desconocido".
    let server = ServeProcess::start_with_program("jwt-not-configured", JWT_PROGRAM);

    let jwt = make_jwt("cualquier-secreto", "HS256", r#"{"role":"Admin","sub":1}"#);
    let (status, _) = server.post("/Secured/adminOnly", &json!({}), Some(&jwt));
    assert_eq!(status, 401);

    server.shutdown();
}

/// AUDIT-2026-08-27.md #13: `--jwt-secret ""` (valor vacío explícito por
/// flag) activaba la verificación de JWT con un secreto vacío en vez de
/// comportarse como "no configurado" -- mismo filtro que ya aplicaba del
/// lado de la env var, ahora también del lado del flag.
#[test]
fn an_empty_string_jwt_secret_flag_behaves_like_it_was_never_configured() {
    let server = ServeProcess::start_with_program_and_args("jwt-empty-flag", JWT_PROGRAM, &["--jwt-secret", ""]);

    // Un JWT firmado con la clave vacía -- si el flag vacío se hubiera
    // tomado en serio, esto verificaría.
    let jwt = make_jwt("", "HS256", r#"{"role":"Admin","sub":1}"#);
    let (status, _) = server.post("/Secured/adminOnly", &json!({}), Some(&jwt));
    assert_eq!(status, 401, "un --jwt-secret vacío no debería activar la verificación de JWT");

    server.shutdown();
}

/// `@idempotent` (GRAMMAR.md §3.140): `create` inserta una fila real -- el
/// contador (`count`) es lo que prueba que un reintento con la MISMA clave
/// nunca corre el cuerpo dos veces, no solo que devuelve un valor parecido.
const IDEMPOTENT_PROGRAM: &str = r#"
    type Order = { id: Int, total: Int }
    db { orders: Order[] }
    service Orders {
        @idempotent
        rpc create(total: Int) -> Order { db.orders.insert(Order { id: 0, total: total }) }
        rpc count() -> Int { db.orders.all().length() }
    }
"#;

#[test]
fn idempotent_replays_the_stored_result_on_a_retry_with_the_same_key_over_a_real_subprocess() {
    let server = ServeProcess::start_with_program("idempotent-replay", IDEMPOTENT_PROGRAM);

    let (status1, body1) = server.post_with_idempotency_key("/Orders/create", &json!({"total": 10}), "key-1");
    assert_eq!(status1, 200, "body: {body1:?}");
    let (status2, body2) = server.post_with_idempotency_key("/Orders/create", &json!({"total": 10}), "key-1");
    assert_eq!(status2, 200, "body: {body2:?}");
    assert_eq!(body1, body2, "un reintento con la misma clave tiene que devolver EXACTAMENTE el mismo resultado");

    let (_, count) = server.post("/Orders/count", &json!({}), None);
    assert_eq!(count, json!(1), "el segundo POST no debe haber insertado una segunda fila");

    server.shutdown();
}

#[test]
fn idempotent_without_a_key_runs_the_body_every_time_over_a_real_subprocess() {
    let server = ServeProcess::start_with_program("idempotent-no-key", IDEMPOTENT_PROGRAM);

    server.post("/Orders/create", &json!({"total": 10}), None);
    server.post("/Orders/create", &json!({"total": 10}), None);
    let (_, count) = server.post("/Orders/count", &json!({}), None);
    assert_eq!(count, json!(2), "sin 'Idempotency-Key' el rpc corre normal, sin ninguna deduplicación");

    server.shutdown();
}

#[test]
fn idempotent_rejects_the_same_key_reused_with_a_different_body_over_a_real_subprocess() {
    let server = ServeProcess::start_with_program("idempotent-conflict", IDEMPOTENT_PROGRAM);

    let (status1, _) = server.post_with_idempotency_key("/Orders/create", &json!({"total": 10}), "key-1");
    assert_eq!(status1, 200);
    let (status2, body2) = server.post_with_idempotency_key("/Orders/create", &json!({"total": 20}), "key-1");
    assert_eq!(status2, 409, "misma clave, body distinto: 409 -- body: {body2:?}");

    let (_, count) = server.post("/Orders/count", &json!({}), None);
    assert_eq!(count, json!(1), "el intento en conflicto no debe haber insertado nada");

    server.shutdown();
}

/// AUDIT-2026-08-27.md #4/GRAMMAR.md §3.166: antes del fix, `lookup`+`store`
/// eran dos candados separados con el cuerpo del rpc corriendo sin ninguno
/// sostenido entre medio -- requests concurrentes con la MISMA clave veían
/// todas un `Miss` y todas corrían el cuerpo. Acá se lanzan 30 requests
/// reales, con hilos del sistema operativo reales, contra un `linkc serve`
/// real -- exactamente el escenario que reprodujo el bug (30 concurrentes
/// insertaron 2 filas antes del fix). Con el fix, como mucho UNA gana la
/// reserva y corre el cuerpo; el resto recibe 200 (con la respuesta
/// repetida, si llegaron después de que la primera terminara) o 409
/// (`in-flight`, si llegaron mientras la primera todavía corría) -- nunca
/// una segunda inserción.
#[test]
fn idempotent_never_runs_the_body_twice_under_real_concurrent_requests_with_the_same_key() {
    let server = ServeProcess::start_with_program("idempotent-concurrent", IDEMPOTENT_PROGRAM);

    std::thread::scope(|scope| {
        for _ in 0..30 {
            scope.spawn(|| {
                let (status, _) = server.post_with_idempotency_key("/Orders/create", &json!({"total": 10}), "race-key");
                assert!(status == 200 || status == 409, "status inesperado: {status}");
            });
        }
    });

    let (_, count) = server.post("/Orders/count", &json!({}), None);
    assert_eq!(count, json!(1), "30 requests concurrentes con la misma clave tienen que insertar UNA sola fila, no más");

    server.shutdown();
}

/// `@cache("60s")` (GRAMMAR.md §3.144): `summary` inserta una fila real cada
/// vez que CORRE de verdad -- `rowCount` (sin `@cache`) es lo que prueba que
/// un segundo POST dentro del TTL nunca ejecutó el cuerpo de nuevo, no solo
/// que devolvió un número parecido.
const CACHE_PROGRAM: &str = r#"
    type Stat = { id: Int, n: Int }
    db { stats: Stat[] }
    service Stats {
        @cache("60s")
        rpc summary() -> Int {
            db.stats.insert(Stat { id: 0, n: 1 });
            db.stats.all().length()
        }
        rpc rowCount() -> Int { db.stats.all().length() }
    }
"#;

#[test]
fn cache_replays_the_stored_result_within_the_ttl_without_rerunning_the_body_over_a_real_subprocess() {
    let server = ServeProcess::start_with_program("cache-replay", CACHE_PROGRAM);

    let (status1, body1) = server.post("/Stats/summary", &json!({}), None);
    assert_eq!(status1, 200, "body: {body1:?}");
    assert_eq!(body1, json!(1));

    let (status2, body2) = server.post("/Stats/summary", &json!({}), None);
    assert_eq!(status2, 200, "body: {body2:?}");
    assert_eq!(body2, json!(1), "un hit de cache repite EXACTAMENTE el resultado grabado");

    let (_, row_count) = server.post("/Stats/rowCount", &json!({}), None);
    assert_eq!(row_count, json!(1), "el segundo POST no debió haber insertado una segunda fila");

    server.shutdown();
}

#[test]
fn a_rpc_without_cache_runs_its_body_every_time_over_a_real_subprocess() {
    let server = ServeProcess::start_with_program("cache-none", CACHE_PROGRAM);

    server.post("/Stats/rowCount", &json!({}), None);
    // `rowCount` no tiene `@cache` -- llamarlo dos veces no inserta nada (no
    // muta), pero confirma que un rpc sin la anotación nunca pasa por el
    // camino de cache (status 200 estable, sin efectos raros).
    let (status, body) = server.post("/Stats/rowCount", &json!({}), None);
    assert_eq!(status, 200, "body: {body:?}");
    assert_eq!(body, json!(0));

    server.shutdown();
}

// ---- `@cron("Ns"/"Nm"/"Nh"/"Nd")` (GRAMMAR.md §3.159) ----

const CRON_PROGRAM: &str = r#"
    type Counter = { id: Int, hits: Int }
    db { counters: Counter[] }
    service Jobs {
        @cron("1s")
        rpc tick() -> Void {
            let rows = db.counters.all();
            if (rows.length() == 0) {
                db.counters.insert(Counter { id: 0, hits: 1 });
            } else {
                db.counters.increment(rows[0].id, |c: Counter| { c.hits }, 1);
            }
        }
        rpc getHits() -> Int {
            let rows = db.counters.all();
            if (rows.length() == 0) { 0 } else { rows[0].hits }
        }
    }
"#;

#[test]
fn a_cron_rpc_runs_on_its_own_without_any_http_request_triggering_it() {
    let server = ServeProcess::start_with_program("cron-fires", CRON_PROGRAM);
    // El scheduler duerme el intervalo COMPLETO antes de la primera corrida
    // (mismo criterio que setInterval de JS, ver runtime/server.rs::serve)
    // -- 2.5s alcanza para dos vueltas de un intervalo de 1s sin ser un test
    // frágil por un margen demasiado ajustado.
    std::thread::sleep(Duration::from_millis(2500));
    let (status, hits) = server.post("/Jobs/getHits", &json!({}), None);
    assert_eq!(status, 200, "body: {hits:?}");
    assert!(hits.as_i64().unwrap_or(0) >= 2, "esperaba al menos 2 corridas de @cron(\"1s\") en 2.5s, dio: {hits:?}");
    server.shutdown();
}

#[test]
fn a_cron_rpc_is_never_reachable_over_http_even_at_its_default_path() {
    let server = ServeProcess::start_with_program("cron-unreachable", CRON_PROGRAM);
    // El checker ya garantiza que `@cron` nunca coexiste con `@route`, pero
    // el path por DEFECTO (`POST /{Service}/{rpc}`) encuentra cualquier rpc
    // por nombre -- esto prueba que server.rs lo bloquea ahí también, antes
    // de que is_cron_member exista solo en el papel.
    let (status, body) = server.post("/Jobs/tick", &json!({}), None);
    assert_eq!(status, 404, "un rpc @cron no puede ser invocado por HTTP, ni siquiera en su path por defecto: {body:?}");
    server.shutdown();
}

// ---- Compresión GZIP de la respuesta HTTP (GRAMMAR.md §3.180) ----

/// El string se arma en Rust (no con un método de stdlib de c-script como
/// `.repeat()`, que puede no existir) y se incrusta como literal en el
/// código fuente del programa de prueba -- 2000 bytes supera con margen el
/// umbral `GZIP_MIN_BODY_BYTES` (1024) de `runtime/server.rs`.
fn big_body_program() -> String {
    let big = "x".repeat(2000);
    format!(
        r#"
        service Big {{
            rpc bigString() -> String {{ "{big}" }}
            rpc smallString() -> String {{ "hola" }}
        }}
        "#
    )
}

#[test]
fn a_large_response_is_gzip_compressed_when_the_client_accepts_it_over_a_real_subprocess() {
    let server = ServeProcess::start_with_program("gzip-large-accepted", &big_body_program());

    let (status, headers, raw_body) = server.post_raw("/Big/bigString", &json!({}), true);
    assert_eq!(status, 200);
    let content_encoding = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("Content-Encoding")).map(|(_, v)| v.as_str());
    assert_eq!(content_encoding, Some("gzip"), "un body grande con Accept-Encoding: gzip debe comprimirse -- headers: {headers:?}");

    use std::io::Read;
    let mut decoder = flate2::read::GzDecoder::new(&raw_body[..]);
    let mut decoded = String::new();
    decoder.read_to_string(&mut decoded).expect("el body debe ser un stream GZIP válido");
    let value: Value = serde_json::from_str(&decoded).expect("descomprimido, el body debe ser el JSON esperado");
    assert_eq!(value.as_str().unwrap().len(), 2000, "el contenido real tiene que sobrevivir el viaje comprimido/descomprimido");

    server.shutdown();
}

#[test]
fn a_large_response_is_not_compressed_when_the_client_does_not_accept_gzip_over_a_real_subprocess() {
    let server = ServeProcess::start_with_program("gzip-large-not-accepted", &big_body_program());

    let (status, headers, raw_body) = server.post_raw("/Big/bigString", &json!({}), false);
    assert_eq!(status, 200);
    assert!(
        headers.iter().all(|(k, _)| !k.eq_ignore_ascii_case("Content-Encoding")),
        "sin Accept-Encoding: gzip no debe agregarse Content-Encoding -- headers: {headers:?}"
    );
    let value: Value = serde_json::from_slice(&raw_body).expect("sin Accept-Encoding, el body es JSON de texto plano sin comprimir");
    assert_eq!(value.as_str().unwrap().len(), 2000);

    server.shutdown();
}

#[test]
fn a_small_response_is_not_compressed_even_when_the_client_accepts_gzip_over_a_real_subprocess() {
    let server = ServeProcess::start_with_program("gzip-small-body", &big_body_program());

    let (status, headers, raw_body) = server.post_raw("/Big/smallString", &json!({}), true);
    assert_eq!(status, 200);
    assert!(
        headers.iter().all(|(k, _)| !k.eq_ignore_ascii_case("Content-Encoding")),
        "un body chico no debe comprimirse aunque el cliente lo acepte -- GZIP_MIN_BODY_BYTES, headers: {headers:?}"
    );
    let value: Value = serde_json::from_slice(&raw_body).expect("body sin comprimir debe ser JSON de texto plano");
    assert_eq!(value, "hola");

    server.shutdown();
}

