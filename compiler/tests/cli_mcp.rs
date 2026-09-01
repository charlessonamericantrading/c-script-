// MCP real (GRAMMAR.md §3.203) -- sesión (Pieza A), tools/list/tools/call
// (Pieza B), mcp.sample + streaming bidireccional (Pieza C). Se prueba
// contra el BINARIO real hablando HTTP de verdad, mismo criterio que el
// resto de los tests de este estilo (`cli_service_api_key.rs`): que el
// código compile no prueba que `--mcp-jwt-secret` de verdad habilite
// `/mcp`, que `initialize` de verdad exija un `Authorization: Bearer` real,
// que `@requires` aplique idéntico vía `tools/call`, ni que la correlación
// cross-hilo de `mcp.sample` funcione contra el servidor real (el spike
// aislado de PLAN.md §9.15 ítem 3 ya probó que el MECANISMO funciona bajo
// `tiny_http` -- esto prueba que la INTEGRACIÓN real también).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

const PROGRAM: &str = r#"
enum Role { Admin, Member }

service Auth {
  rpc login() -> String { auth.createSession(Role.Admin {}) }
  rpc loginAsMember() -> String { auth.createSession(Role.Member {}) }
}

service Calc {
  rpc add(a: Int, b: Int) -> Int { a + b }

  @requires(Role.Admin)
  rpc adminOnly() -> String { "solo admin" }

  rpc askLlm(prompt: String) -> String { mcp.sample(prompt) }
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
    // `tools/list`/`tools/call` ya están implementados (Pieza B) -- un
    // método real pero todavía no conectado (Pieza C, streaming
    // bidireccional) es el caso real de "no implementado todavía".
    let temp = TempDir::new("unknown-method");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &["--mcp-jwt-secret", "mcp-s3cr3t"]);
    let (status, _, body) = server.request("POST", "/mcp", r#"{"jsonrpc":"2.0","id":1,"method":"sampling/createMessage"}"#, &[]);
    assert_eq!(status, 501, "body: {body}");
}

// ---- Pieza B: tools/list / tools/call ----

#[test]
fn tools_list_exposes_every_non_stream_non_cron_rpc_with_a_json_schema() {
    let temp = TempDir::new("tools-list");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &["--mcp-jwt-secret", "mcp-s3cr3t"]);
    let (status, _, body) = server.request("POST", "/mcp", r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#, &[]);
    assert_eq!(status, 200, "body: {body}");
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("body debe ser JSON");
    let tools = parsed["result"]["tools"].as_array().expect("result.tools debe ser un array");
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"Calc_add"), "{names:?}");
    assert!(names.contains(&"Calc_adminOnly"), "{names:?}");
    let add_tool = tools.iter().find(|t| t["name"] == "Calc_add").expect("Calc_add tiene que estar en la lista");
    assert_eq!(add_tool["inputSchema"]["properties"]["a"]["type"], "integer");
    assert_eq!(add_tool["inputSchema"]["required"], serde_json::json!(["a", "b"]));
}

#[test]
fn tools_call_without_a_session_is_rejected() {
    let temp = TempDir::new("call-no-session");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &["--mcp-jwt-secret", "mcp-s3cr3t"]);
    let (status, _, body) = server.request(
        "POST",
        "/mcp",
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"Calc_add","arguments":{"a":1,"b":2}}}"#,
        &[],
    );
    assert_eq!(status, 401, "body: {body}");
}

/// Hace el ciclo completo `login` -> `initialize` y devuelve el
/// `Mcp-Session-Id` resultante, para los tests de `tools/call` que
/// necesitan una sesión MCP real ya establecida.
fn login_and_initialize(server: &Serve) -> String {
    let token = login(server);
    let (status, headers, body) = server.request(
        "POST",
        "/mcp",
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
        &[("Authorization", &format!("Bearer {token}"))],
    );
    assert_eq!(status, 200, "body: {body}");
    headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("Mcp-Session-Id")).map(|(_, v)| v.clone()).expect("initialize debe devolver Mcp-Session-Id")
}

#[test]
fn tools_call_invokes_the_real_rpc_and_wraps_the_result_in_mcp_content_blocks() {
    let temp = TempDir::new("call-ok");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &["--mcp-jwt-secret", "mcp-s3cr3t"]);
    let mcp_session_id = login_and_initialize(&server);

    let (status, _, body) = server.request(
        "POST",
        "/mcp",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"Calc_add","arguments":{"a":3,"b":4}}}"#,
        &[("Mcp-Session-Id", &mcp_session_id)],
    );
    assert_eq!(status, 200, "body: {body}");
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("body debe ser JSON");
    assert_eq!(parsed["result"]["content"][0]["type"], "text");
    assert_eq!(parsed["result"]["content"][0]["text"], "7");
}

#[test]
fn tools_call_on_an_unknown_tool_is_a_clean_404() {
    let temp = TempDir::new("call-unknown-tool");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &["--mcp-jwt-secret", "mcp-s3cr3t"]);
    let mcp_session_id = login_and_initialize(&server);

    let (status, _, body) = server.request(
        "POST",
        "/mcp",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"NoExiste_foo","arguments":{}}}"#,
        &[("Mcp-Session-Id", &mcp_session_id)],
    );
    assert_eq!(status, 404, "body: {body}");
}

/// El mismo `@requires(Role.Admin)` que ya protege el `rpc` por la vía
/// REST normal (GRAMMAR.md §3.14) tiene que aplicar IDÉNTICO vía
/// `tools/call` -- confirma que no hay un camino de auth paralelo sin
/// auditar para MCP.
#[test]
fn tools_call_respects_the_rpcs_existing_requires_annotation() {
    let temp = TempDir::new("call-requires");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &["--mcp-jwt-secret", "mcp-s3cr3t"]);

    let (status, _, body) = server.request("POST", "/Auth/loginAsMember", "{}", &[]);
    assert_eq!(status, 200, "body: {body}");
    let member_token: String = serde_json::from_str(&body).expect("login debe devolver un token string");
    let (status, headers, body) = server.request(
        "POST",
        "/mcp",
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
        &[("Authorization", &format!("Bearer {member_token}"))],
    );
    assert_eq!(status, 200, "body: {body}");
    let mcp_session_id =
        headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("Mcp-Session-Id")).map(|(_, v)| v.clone()).expect("initialize debe devolver Mcp-Session-Id");

    let (status, _, body) = server.request(
        "POST",
        "/mcp",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"Calc_adminOnly","arguments":{}}}"#,
        &[("Mcp-Session-Id", &mcp_session_id)],
    );
    assert_eq!(status, 403, "un Member no debería poder llamar un tool @requires(Role.Admin): {body}");
}

// ---- Pieza C: mcp.sample + streaming bidireccional real ----

/// `GET /mcp` conectado a mano por un `TcpStream` -- mismo patrón que
/// `StreamClient` en `pg_integration.rs` (`Transfer-Encoding: chunked`,
/// un evento SSE por chunk, `write_chunk` en `runtime/server.rs` nunca
/// parte uno en dos ni junta dos en uno).
struct McpStreamClient {
    reader: BufReader<TcpStream>,
}

impl McpStreamClient {
    fn connect(port: u16, mcp_session_id: &str) -> Self {
        let stream = TcpStream::connect(("127.0.0.1", port)).expect("conectar a GET /mcp");
        stream.set_read_timeout(Some(Duration::from_secs(10))).expect("fijar read timeout");
        let request = format!(
            "GET /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nMcp-Session-Id: {mcp_session_id}\r\nConnection: keep-alive\r\n\r\n"
        );
        let mut stream = stream;
        stream.write_all(request.as_bytes()).expect("escribir GET /mcp");
        stream.flush().ok();

        let mut reader = BufReader::new(stream);
        let mut status_line = String::new();
        reader.read_line(&mut status_line).expect("línea de estado de GET /mcp");
        assert!(status_line.contains("200"), "GET /mcp no arrancó bien: {status_line}");
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).expect("header de GET /mcp");
            if line.trim().is_empty() {
                break;
            }
        }
        McpStreamClient { reader }
    }

    fn next_event(&mut self) -> Option<serde_json::Value> {
        let mut size_line = String::new();
        self.reader.read_line(&mut size_line).ok()?;
        let size = usize::from_str_radix(size_line.trim(), 16).ok()?;
        if size == 0 {
            return None;
        }
        let mut buf = vec![0u8; size];
        self.reader.read_exact(&mut buf).ok()?;
        let mut crlf = [0u8; 2];
        self.reader.read_exact(&mut crlf).ok()?;
        let chunk = String::from_utf8_lossy(&buf);
        let data = chunk.strip_prefix("data: ")?.trim_end_matches(['\n', '\r']);
        serde_json::from_str(data).ok()
    }
}

#[test]
fn mcp_sample_without_an_open_get_connection_is_a_clean_runtime_error() {
    let temp = TempDir::new("sample-no-connection");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &["--mcp-jwt-secret", "mcp-s3cr3t"]);
    let mcp_session_id = login_and_initialize(&server);

    // Sin ningún GET /mcp abierto para esta sesión -- mcp.sample tiene que
    // fallar limpio, no colgarse.
    let (status, _, body) = server.request(
        "POST",
        "/mcp",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"Calc_askLlm","arguments":{"prompt":"hola"}}}"#,
        &[("Mcp-Session-Id", &mcp_session_id)],
    );
    assert_eq!(status, 500, "body: {body}");
    assert!(body.contains("no hay ninguna conexión"), "body: {body}");
}

#[test]
fn mcp_sample_full_round_trip_over_a_real_get_connection_and_a_real_post_response() {
    let temp = TempDir::new("sample-round-trip");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &["--mcp-jwt-secret", "mcp-s3cr3t"]);
    let mcp_session_id = login_and_initialize(&server);

    // La conexión GET tiene que estar abierta ANTES de que tools/call
    // dispare mcp.sample, para no correr una carrera real.
    let mut stream_client = McpStreamClient::connect(server.port, &mcp_session_id);

    // `std::thread::scope` (no `std::thread::spawn`): `Serve::request` toma
    // `&self`, así que el hilo de `tools/call` puede pedir prestado
    // `&server` directo, sin `Arc` ni ningún truco -- `tools/call` bloquea
    // (mcp.sample espera la respuesta correlacionada) mientras el hilo
    // PRINCIPAL lee el evento SSE y lo responde.
    std::thread::scope(|scope| {
        let call_thread = scope.spawn(|| {
            server.request(
                "POST",
                "/mcp",
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"Calc_askLlm","arguments":{"prompt":"¿cuánto es 2+2?"}}}"#,
                &[("Mcp-Session-Id", &mcp_session_id)],
            )
        });

        let event = stream_client.next_event().expect("tiene que llegar un evento sampling/createMessage");
        assert_eq!(event["method"], "sampling/createMessage", "evento inesperado: {event}");
        let sample_id = event["id"].clone();
        let sample_prompt = event["params"]["messages"][0]["content"]["text"].as_str().unwrap_or_default();
        assert_eq!(sample_prompt, "¿cuánto es 2+2?");

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": sample_id,
            "result": { "content": [{ "type": "text", "text": "4" }] },
        })
        .to_string();
        let (status, _, body) = server.request("POST", "/mcp", &response, &[]);
        assert_eq!(status, 200, "entrega de la respuesta correlacionada: {body}");

        let (status, _, body) = call_thread.join().expect("el hilo de tools/call no debería panickear");
        assert_eq!(status, 200, "body: {body}");
        let parsed: serde_json::Value = serde_json::from_str(&body).expect("body debe ser JSON");
        assert_eq!(parsed["result"]["content"][0]["text"], "4", "body: {body}");
    });
}

/// Conexión GET abierta pero NADIE responde -- `mcp.sample` tiene que
/// cortar limpio al timeout (30s, `mcp.rs::SAMPLE_TIMEOUT`), nunca
/// quedarse colgado para siempre. Test real, no simulado -- deliberadamente
/// más lento que el resto de la suite (mismo trade-off que cualquier test
/// de un timeout real de producción: probarlo de verdad cuesta esperar).
#[test]
fn mcp_sample_that_never_gets_a_response_times_out_cleanly() {
    let temp = TempDir::new("sample-timeout");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &["--mcp-jwt-secret", "mcp-s3cr3t"]);
    let mcp_session_id = login_and_initialize(&server);
    let _stream_client = McpStreamClient::connect(server.port, &mcp_session_id);

    let start = std::time::Instant::now();
    let (status, _, body) = server.request(
        "POST",
        "/mcp",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"Calc_askLlm","arguments":{"prompt":"nadie va a responder"}}}"#,
        &[("Mcp-Session-Id", &mcp_session_id)],
    );
    let elapsed = start.elapsed();
    assert_eq!(status, 500, "body: {body}");
    assert!(body.contains("no respondió"), "body: {body}");
    assert!(elapsed >= Duration::from_secs(29), "cortó demasiado rápido ({elapsed:?}) -- ¿el timeout dejó de aplicar?");
    assert!(elapsed < Duration::from_secs(45), "tardó demasiado ({elapsed:?}) -- ¿quedó colgado en vez de cortar al timeout?");
}

/// Auditoría del lenguaje (2026-09-01), GRAMMAR.md §3.204: `tools/list`
/// aplana `(service, rpc)` a `"{service}_{rpc}"` -- un espacio de nombres
/// plano exigido por MCP, sin separador real. Dos pares distintos con
/// guiones bajos propios pueden generar el MISMO nombre de tool
/// (`service A_B { rpc c() }` y `service A { rpc B_c() }` ambos dan
/// `"A_B_c"`) -- antes de este fix, `resolve_tool_name` enrutaba
/// SILENCIOSAMENTE al primero en orden de declaración, sin importar cuál el
/// cliente MCP realmente pretendía. Ahora `--mcp-jwt-secret` rechaza
/// arrancar, nombrando los dos service/rpc en colisión -- no usa `Serve::
/// start` (que espera a que el puerto abra y hace panic si no) porque acá
/// se prueba justamente que el puerto NUNCA abre.
#[test]
fn mcp_startup_rejects_two_service_rpc_pairs_that_collide_on_the_same_tool_name() {
    let temp = TempDir::new("tool-collision");
    let src = temp.write(
        "app.link",
        r#"
        service A_B {
            rpc c() -> Int { 1 }
        }
        service A {
            rpc B_c() -> Int { 2 }
        }
        "#,
    );
    let port = free_port();
    let output = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&src)
        .arg(port.to_string())
        .arg("--mcp-jwt-secret")
        .arg("mcp-s3cr3t")
        .output()
        .expect("ejecutar 'linkc serve'");
    assert!(!output.status.success(), "debería salir con error, no arrancar el servidor");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("A_B.c"), "{stderr}");
    assert!(stderr.contains("A.B_c"), "{stderr}");
    assert!(stderr.contains("A_B_c"), "{stderr}");
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err(), "el puerto no debería haber quedado abierto");
}

/// Mismo chequeo, pero confirma que NO hay falso positivo: dos service/rpc
/// con nombres parecidos pero que NO colisionan arrancan sin problema.
#[test]
fn mcp_startup_allows_similar_but_non_colliding_service_rpc_names() {
    let temp = TempDir::new("no-collision");
    let src = temp.write(
        "app.link",
        r#"
        service A {
            rpc b() -> Int { 1 }
        }
        service A_b {
            rpc c() -> Int { 2 }
        }
        "#,
    );
    let server = Serve::start(&src, &["--mcp-jwt-secret", "mcp-s3cr3t"]);
    let (status, _, body) = server.request("POST", "/mcp", r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#, &[]);
    // Sin token todavía -- solo confirma que el servidor arrancó de verdad
    // (rechazo de auth, no un 404 de "el puerto nunca abrió").
    assert_ne!(status, 0, "body: {body}");
}
