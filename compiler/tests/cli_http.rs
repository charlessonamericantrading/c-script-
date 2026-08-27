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
type Resp = { status: Int, headers: Header[], body: String }

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

  rpc getStatus(url: String) -> Resp {
    http.getWithStatus(url, [])
  }

  rpc postStatus(url: String, body: String) -> Resp {
    http.postWithStatus(url, body, [])
  }

  rpc postRetry(url: String, body: String, maxAttempts: Int) -> String {
    http.postWithRetry(url, body, [], maxAttempts)
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
        Self::start_with_response(200, "OK", br#"{"ok":true}"#, &[])
    }

    /// Como `start`, pero con el status/body/headers de respuesta que pida
    /// el test -- necesario para probar `getWithStatus`/`postWithStatus`
    /// (GRAMMAR.md §3.60), que existen justamente para que un 4xx/5xx real
    /// llegue como DATO, no como error.
    fn start_with_response(status: u16, reason: &'static str, body: &'static [u8], extra_headers: &'static [(&'static str, &'static str)]) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bindear puerto efímero");
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let tx = tx.clone();
                std::thread::spawn(move || handle_one_connection(stream, tx, status, reason, body, extra_headers));
            }
        });
        FakeHttp { port, rx }
    }

    fn recv(&self, timeout: Duration) -> Option<ReceivedRequest> {
        self.rx.recv_timeout(timeout).ok()
    }

    /// GRAMMAR.md §3.160 (`http.postWithRetry`): las primeras `fail_count`
    /// conexiones reciben 500, la siguiente en adelante 200 -- un contador
    /// compartido entre conexiones (`AtomicUsize`, no un `Mutex` porque solo
    /// hace falta incrementar-y-leer atómico) simula un endpoint real que
    /// falla de forma transitoria y se recupera solo.
    fn start_failing_then_succeeding(fail_count: usize) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bindear puerto efímero");
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = channel();
        let seen = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let tx = tx.clone();
                let seen = std::sync::Arc::clone(&seen);
                std::thread::spawn(move || {
                    let n = seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if n < fail_count {
                        handle_one_connection(stream, tx, 500, "Internal Server Error", b"falla transitoria", &[]);
                    } else {
                        handle_one_connection(stream, tx, 200, "OK", br#"{"ok":true}"#, &[]);
                    }
                });
            }
        });
        FakeHttp { port, rx }
    }
}

fn handle_one_connection(
    stream: TcpStream,
    tx: Sender<ReceivedRequest>,
    status: u16,
    reason: &str,
    response_body: &[u8],
    extra_headers: &[(&str, &str)],
) {
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

    let _ = write!(writer, "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n", response_body.len());
    for (name, value) in extra_headers {
        let _ = write!(writer, "{name}: {value}\r\n");
    }
    let _ = write!(writer, "Connection: close\r\n\r\n");
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
        Self::start_with_args(link_path, &[])
    }

    fn start_with_args(link_path: &PathBuf, extra_args: &[&str]) -> Self {
        let port = free_port();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_linkc"));
        cmd.arg("serve").arg(link_path).arg(port.to_string());
        for a in extra_args {
            cmd.arg(a);
        }
        let child = cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().expect("iniciar 'linkc serve'");
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
fn get_with_status_exposes_the_2xx_status_code_and_response_headers() {
    let upstream = FakeHttp::start_with_response(200, "OK", br#"{"ok":true}"#, &[("X-Request-Id", "abc123")]);
    let temp = TempDir::new("status-2xx");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src);

    let url = format!("http://127.0.0.1:{}/ping", upstream.port);
    let (status, body) = server.post("/Sys/getStatus", &serde_json::json!({"url": url}).to_string());
    assert_eq!(status, 200, "el rpc en sí siempre responde 200: body {body}");
    let resp: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(resp["status"], 200, "status de la respuesta upstream: {resp}");
    assert_eq!(resp["body"], r#"{"ok":true}"#);
    // HTTP no distingue mayúsculas/minúsculas en nombres de header -- `ureq`
    // los normaliza a minúsculas al parsear la respuesta, así que la
    // comparación tiene que serlo también (mismo criterio que
    // `ReceivedRequest::header` ya usa para los headers de la REQUEST).
    let headers = resp["headers"].as_array().expect("headers es una lista");
    assert!(
        headers.iter().any(|h| h["name"].as_str().unwrap_or("").eq_ignore_ascii_case("X-Request-Id") && h["value"] == "abc123"),
        "el header de la respuesta upstream tiene que estar en la lista: {headers:?}"
    );
}

#[test]
fn get_with_status_returns_a_4xx_as_data_not_as_a_runtime_error() {
    // La brecha real que getWithStatus cierra (README "Does not work yet"
    // hasta esta ronda): antes, un 429 de la API llamada se volvía un error
    // de runtime genérico -- imposible reintentar SOLO en ese código.
    let upstream = FakeHttp::start_with_response(429, "Too Many Requests", b"rate limited", &[("Retry-After", "30")]);
    let temp = TempDir::new("status-4xx");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src);

    let url = format!("http://127.0.0.1:{}/v1/charges", upstream.port);
    let (status, body) = server.post("/Sys/getStatus", &serde_json::json!({"url": url}).to_string());
    assert_eq!(status, 200, "el 429 upstream NO tiene que tirar abajo el rpc: body {body}");
    let resp: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(resp["status"], 429, "el codigo real de la API llamada, como dato: {resp}");
    assert_eq!(resp["body"], "rate limited");
    let headers = resp["headers"].as_array().expect("headers es una lista");
    assert!(
        headers.iter().any(|h| h["name"].as_str().unwrap_or("").eq_ignore_ascii_case("Retry-After") && h["value"] == "30"),
        "el header Retry-After tiene que llegar para poder implementar backoff: {headers:?}"
    );
}

#[test]
fn post_with_status_also_exposes_the_real_status_code() {
    let upstream = FakeHttp::start_with_response(201, "Created", br#"{"id":42}"#, &[]);
    let temp = TempDir::new("status-post");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src);

    let url = format!("http://127.0.0.1:{}/v1/charges", upstream.port);
    let (status, body) =
        server.post("/Sys/postStatus", &serde_json::json!({"url": url, "body": "amount=100"}).to_string());
    assert_eq!(status, 200, "body: {body}");
    let resp: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(resp["status"], 201);
    assert_eq!(resp["body"], r#"{"id":42}"#);
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

// ---- `--http-timeout`/`LINK_HTTP_TIMEOUT` (GRAMMAR.md §3.86) ----
//
// Hasta esta ronda, `http.get`/`post`/etc. no tenían NINGÚN timeout de
// lectura/escritura (`ureq` solo trae uno de conexión, 30s, por default) --
// una request saliente a un servidor que ACEPTA la conexión pero nunca
// responde bloqueaba el intérprete (de un solo hilo) para SIEMPRE. Se
// prueba acá contra un servidor de mentira que hace exactamente eso.

/// Acepta la conexión y se queda sin escribir NADA -- ni siquiera una línea
/// de estado -- durante mucho más tiempo del que cualquier timeout de este
/// test configura. El hilo se abandona a propósito al volver (`Drop` no
/// hace falta: el proceso de test termina igual, y el listener se cierra
/// solo con el binding).
struct HangingServer {
    port: u16,
}

impl HangingServer {
    fn start() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bindear puerto efímero");
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                std::thread::sleep(Duration::from_secs(60));
                drop(stream);
            }
        });
        HangingServer { port }
    }
}

#[test]
fn a_hanging_upstream_times_out_instead_of_blocking_the_server_forever() {
    let upstream = HangingServer::start();
    let temp = TempDir::new("timeout");
    let src = temp.write("app.link", PROGRAM);
    // 1s -- mucho más corto que los 60s que `HangingServer` se queda sin
    // responder, así que si el timeout de verdad se aplica esto vuelve
    // rápido; si no se aplicara (regresión), colgaría hasta el timeout de
    // conexión de 30s de `ureq` como mínimo -- de cualquier forma, mucho
    // más que el presupuesto generoso que este test se da.
    let server = Serve::start_with_args(&src, &["--http-timeout", "1s"]);

    let url = format!("http://127.0.0.1:{}/", upstream.port);
    let start = std::time::Instant::now();
    let (status, body) = server.post("/Sys/plainGet", &serde_json::json!({"url": url}).to_string());
    let elapsed = start.elapsed();

    assert_eq!(status, 500, "un upstream que nunca responde es un error de runtime, no un 200: {body}");
    assert!(!body.contains("panicked"), "{body}");
    assert!(
        elapsed < Duration::from_secs(10),
        "debería haber cortado cerca de 1s configurado, tardó {elapsed:?} -- ¿se está bloqueando en vez de cortar?"
    );
}

#[test]
fn link_http_timeout_env_var_is_honored() {
    let upstream = HangingServer::start();
    let temp = TempDir::new("timeout-env");
    let src = temp.write("app.link", PROGRAM);
    let port = free_port();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_linkc"));
    cmd.arg("serve").arg(&src).arg(port.to_string()).env("LINK_HTTP_TIMEOUT", "1s");
    let child = cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().expect("iniciar 'linkc serve'");
    let server = Serve { child, port };
    wait_for_port(port);

    let url = format!("http://127.0.0.1:{}/", upstream.port);
    let start = std::time::Instant::now();
    let (status, body) = server.post("/Sys/plainGet", &serde_json::json!({"url": url}).to_string());
    assert_eq!(status, 500, "body: {body}");
    assert!(start.elapsed() < Duration::from_secs(10), "tardó {:?}", start.elapsed());
}

/// GRAMMAR.md §3.114: el flujo OAuth2 "client credentials" (servidor a
/// servidor, sin login de usuario -- el que usan Google APIs/Microsoft
/// Graph/Salesforce/HubSpot para autenticación de máquina a máquina) NO es
/// un gap del lenguaje: `http.postWithHeaders` para pedir el token,
/// `json.parse(...).campo` (`Type::Dynamic`, ya asignable a `String` sin
/// cast explícito) para extraer `access_token` de la respuesta, y
/// `http.getWithHeaders` con `Authorization: Bearer <token>` para la
/// llamada real -- las tres piezas ya existían, ninguna nueva. Distinto del
/// login-con-usuario (OAuth2 authorization code, PLAN.md §9.12, bloqueado
/// porque verificarlo de punta a punta necesita un proveedor de identidad
/// real con una app de prueba registrada).
const OAUTH2_PROGRAM: &str = r#"
type Header = { name: String, value: String }

service Api {
  rpc callProtectedApi(tokenUrl: String, clientId: String, clientSecret: String, apiUrl: String) -> String {
    let tokenBody = "grant_type=client_credentials&client_id=" + clientId + "&client_secret=" + clientSecret;
    let tokenResponse = http.postWithHeaders(tokenUrl, tokenBody, [
      Header { name: "Content-Type", value: "application/x-www-form-urlencoded" },
    ]);
    let parsed = json.parse(tokenResponse);
    let token = parsed.access_token;
    http.getWithHeaders(apiUrl, [
      Header { name: "Authorization", value: "Bearer " + token },
    ])
  }
}
"#;

#[test]
fn oauth2_client_credentials_flow_works_end_to_end_with_only_existing_builtins() {
    // Dos servidores de mentira DISTINTOS -- uno hace de endpoint de token,
    // el otro de API protegida -- para confirmar que el token que devuelve
    // el primero es EXACTAMENTE el que el segundo recibe en su header
    // `Authorization`, no solo que las dos llamadas salieron.
    let token_server = FakeHttp::start_with_response(200, "OK", br#"{"access_token":"tok-xyz-789","expires_in":3600}"#, &[]);
    let api_server = FakeHttp::start_with_response(200, "OK", br#"{"result":"secreto"}"#, &[]);
    let temp = TempDir::new("oauth2-client-credentials");
    let src = temp.write("app.link", OAUTH2_PROGRAM);
    let server = Serve::start(&src);

    let token_url = format!("http://127.0.0.1:{}/oauth/token", token_server.port);
    let api_url = format!("http://127.0.0.1:{}/v1/protected", api_server.port);
    let (status, body) = server.post(
        "/Api/callProtectedApi",
        &serde_json::json!({"tokenUrl": token_url, "clientId": "client-1", "clientSecret": "secret-1", "apiUrl": api_url}).to_string(),
    );
    assert_eq!(status, 200, "body: {body}");
    assert_eq!(body, "\"{\\\"result\\\":\\\"secreto\\\"}\"");

    let token_req = token_server.recv(Duration::from_secs(5)).expect("el servidor de token debió recibir la request");
    assert_eq!(token_req.method, "POST");
    assert_eq!(token_req.header("Content-Type"), Some("application/x-www-form-urlencoded"));
    assert_eq!(token_req.body, "grant_type=client_credentials&client_id=client-1&client_secret=secret-1");

    let api_req = api_server.recv(Duration::from_secs(5)).expect("el servidor de API debió recibir la request");
    assert_eq!(api_req.method, "GET");
    assert_eq!(
        api_req.header("Authorization"),
        Some("Bearer tok-xyz-789"),
        "el token extraído de la respuesta del primer servidor tiene que llegar EXACTO al segundo: {:?}",
        api_req.headers
    );
}

// ---- `http.postWithRetry` (GRAMMAR.md §3.160) ----

#[test]
fn post_with_retry_succeeds_after_transient_failures_within_its_budget() {
    let upstream = FakeHttp::start_failing_then_succeeding(2);
    let temp = TempDir::new("retry-succeeds");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src);

    let url = format!("http://127.0.0.1:{}/webhook", upstream.port);
    let (status, body) = server.post("/Sys/postRetry", &serde_json::json!({"url": url, "body": "evento", "maxAttempts": 3}).to_string());
    assert_eq!(status, 200, "2 fallas transitorias con presupuesto de 3 intentos tienen que terminar en éxito: body: {body}");
    assert_eq!(body, "\"{\\\"ok\\\":true}\"");
}

#[test]
fn post_with_retry_gives_up_after_exhausting_max_attempts() {
    // Un 500 persistente (nunca se recupera) -- con solo 2 intentos de
    // presupuesto, el tercer intento que arreglaría todo nunca llega a
    // pasar. Prueba que esto falla LIMPIO (un runtime error real, no un
    // loop infinito ni un panic) en vez de reintentar para siempre.
    let upstream = FakeHttp::start_with_response(500, "Internal Server Error", b"caido de verdad", &[]);
    let temp = TempDir::new("retry-exhausted");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src);

    let url = format!("http://127.0.0.1:{}/webhook", upstream.port);
    let (status, body) = server.post("/Sys/postRetry", &serde_json::json!({"url": url, "body": "evento", "maxAttempts": 2}).to_string());
    assert_eq!(status, 500, "agotar los 2 intentos contra un 500 persistente tiene que fallar limpio: body: {body}");
}

#[test]
fn post_with_retry_rejects_a_non_positive_max_attempts() {
    let upstream = FakeHttp::start();
    let temp = TempDir::new("retry-bad-max-attempts");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src);

    let url = format!("http://127.0.0.1:{}/webhook", upstream.port);
    let (status, body) = server.post("/Sys/postRetry", &serde_json::json!({"url": url, "body": "evento", "maxAttempts": 0}).to_string());
    assert_eq!(status, 500, "maxAttempts=0 tiene que ser un error de runtime claro, no un no-op ni un panic: body: {body}");
    assert!(upstream.recv(Duration::from_millis(200)).is_none(), "con maxAttempts inválido no debería haberse mandado ninguna request real");
}

#[test]
fn an_http_timeout_flag_with_an_invalid_duration_is_a_clean_cli_error() {
    let temp = TempDir::new("badvalue");
    let src = temp.write("app.link", PROGRAM);
    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&src)
        .arg(free_port().to_string())
        .arg("--http-timeout")
        .arg("not-a-duration")
        .output()
        .expect("ejecutar linkc serve");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("panicked at"), "un flag mal usado es un error de uso, no un panic: {stderr}");
}
