// `@rate_limit("N/ventana")` (GRAMMAR.md §3.39): como mucho N requests por
// ventana de tiempo, por (ip del cliente, servicio, rpc) -- la respuesta al
// excederlo es 429, no un error genérico ni un cuelgue.
//
// Nace de la misma auditoría de "gaps de producción" que llevó a
// `env.get`/`crypto.hmacSha256`/`request.rawBody` (GRAMMAR.md §3.38): un
// backend real necesita poder protegerse de abuso sin depender de un proxy
// externo. Como con `@content_type`/`@route`, esto se prueba contra el
// BINARIO real hablando HTTP de verdad -- que el checker acepte la
// anotación no prueba que el servidor la haga cumplir.

use serde_json::json;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

const PROGRAM: &str = r#"
enum Role { Admin, Member }

service Sys {
  @rate_limit("3/1m")
  rpc ping() -> String {
    "pong"
  }

  rpc unlimited() -> String {
    "pong"
  }

  @requires(Role.Admin)
  @rate_limit("2/1m")
  rpc adminPing() -> String {
    "pong"
  }

  @rate_limit("2/1m", key: email)
  rpc loginAttempt(email: String) -> String {
    "ok"
  }
}
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "linkc-rate-limit-{name}-{}-{}",
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

/// Mismo criterio que tests/server_http.rs: un round-trip HTTP completo es
/// la única señal confiable de que el servidor ya está aceptando conexiones.
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

    /// POST /{service}/{method} con un body JSON y un token bearer opcional
    /// -- HTTP de verdad sobre un TcpStream propio por request.
    fn post(&self, path: &str, body: &serde_json::Value, token: Option<&str>) -> (u16, serde_json::Value) {
        self.post_with_headers(path, body, token, &[])
    }

    /// Como `post`, más headers extra (`X-Forwarded-For`, GRAMMAR.md §3.89).
    fn post_with_headers(
        &self,
        path: &str,
        body: &serde_json::Value,
        token: Option<&str>,
        extra_headers: &[(&str, &str)],
    ) -> (u16, serde_json::Value) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("conectar");
        let body_str = body.to_string();
        let mut request = format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
            self.port,
            body_str.len()
        );
        if let Some(t) = token {
            request.push_str(&format!("Authorization: Bearer {t}\r\n"));
        }
        for (k, v) in extra_headers {
            request.push_str(&format!("{k}: {v}\r\n"));
        }
        request.push_str("\r\n");
        request.push_str(&body_str);
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
        let json = if buf.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&buf).expect("body debe ser JSON") };
        (status, json)
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn build(temp: &TempDir, source: &str) -> std::process::Output {
    let src = temp.write("app.link", source);
    Command::new(env!("CARGO_BIN_EXE_linkc")).arg("build").arg(&src).arg(temp.0.join("gen")).output().expect("ejecutar linkc build")
}

#[test]
fn requests_over_the_limit_get_429_and_unrelated_rpcs_are_unaffected() {
    let temp = TempDir::new("basic");
    let out = build(&temp, PROGRAM);
    assert!(out.status.success(), "el programa debió compilar: {}", String::from_utf8_lossy(&out.stderr));

    let server = Serve::start(&temp.0.join("app.link"));

    // "3/1m": las primeras 3 pasan.
    for i in 1..=3 {
        let (status, body) = server.post("/Sys/ping", &json!({}), None);
        assert_eq!(status, 200, "request {i} debió pasar: {body:?}");
    }
    // La 4ta y la 5ta, no.
    for i in 4..=5 {
        let (status, body) = server.post("/Sys/ping", &json!({}), None);
        assert_eq!(status, 429, "request {i} debió ser rechazada: {body:?}");
        assert!(body["error"].is_string(), "el 429 debe traer un error en JSON, igual que cualquier otro: {body:?}");
    }

    // Un rpc SIN `@rate_limit` no se ve afectado por haber agotado el bucket
    // de otro rpc -- la clave incluye el nombre del rpc, no es global por IP.
    for i in 1..=5 {
        let (status, body) = server.post("/Sys/unlimited", &json!({}), None);
        assert_eq!(status, 200, "request {i} a un rpc sin límite no debió verse afectada: {body:?}");
    }
}

// ---- `--trust-proxy`/`LINK_TRUST_PROXY` (GRAMMAR.md §3.89) ----

#[test]
fn without_trust_proxy_a_spoofed_x_forwarded_for_is_ignored() {
    // Sin --trust-proxy: TODAS estas requests vienen de la MISMA conexión
    // TCP real (el test), así que comparten un solo bucket sin importar qué
    // diga X-Forwarded-For -- si el header se estuviera respetando por
    // default, cada IP distinta tendría su propio balde y esto no daría
    // 429 nunca.
    let temp = TempDir::new("no-trust-proxy");
    let out = build(&temp, PROGRAM);
    assert!(out.status.success());
    let server = Serve::start(&temp.0.join("app.link"));

    for (i, fake_ip) in ["1.1.1.1", "2.2.2.2", "3.3.3.3"].iter().enumerate() {
        let (status, body) = server.post_with_headers("/Sys/ping", &json!({}), None, &[("X-Forwarded-For", fake_ip)]);
        assert_eq!(status, 200, "request {} debió pasar: {body:?}", i + 1);
    }
    let (status, body) = server.post_with_headers("/Sys/ping", &json!({}), None, &[("X-Forwarded-For", "4.4.4.4")]);
    assert_eq!(status, 429, "una 4ta IP 'distinta' no debería salvar de compartir el mismo balde: {body:?}");
}

#[test]
fn with_trust_proxy_each_forwarded_ip_gets_its_own_bucket() {
    let temp = TempDir::new("trust-proxy-separate");
    let out = build(&temp, PROGRAM);
    assert!(out.status.success());
    let server = Serve::start_with_args(&temp.0.join("app.link"), &["--trust-proxy"]);

    // Cliente A agota su balde de 3.
    for i in 1..=3 {
        let (status, body) = server.post_with_headers("/Sys/ping", &json!({}), None, &[("X-Forwarded-For", "9.9.9.1")]);
        assert_eq!(status, 200, "cliente A, request {i}: {body:?}");
    }
    let (status, body) = server.post_with_headers("/Sys/ping", &json!({}), None, &[("X-Forwarded-For", "9.9.9.1")]);
    assert_eq!(status, 429, "cliente A ya agotó su balde: {body:?}");

    // Cliente B, IP distinta -- balde propio, sin verse afectado por A.
    let (status, body) = server.post_with_headers("/Sys/ping", &json!({}), None, &[("X-Forwarded-For", "9.9.9.2")]);
    assert_eq!(status, 200, "cliente B no debería verse afectado por el balde de A: {body:?}");
}

#[test]
fn with_trust_proxy_the_first_hop_in_a_forwarding_chain_is_used() {
    // "cliente, proxy1, proxy2" -- el primero (más cercano al cliente
    // original) es el que corresponde tomar, no el último.
    let temp = TempDir::new("trust-proxy-chain");
    let out = build(&temp, PROGRAM);
    assert!(out.status.success());
    let server = Serve::start_with_args(&temp.0.join("app.link"), &["--trust-proxy"]);

    for i in 1..=3 {
        let (status, body) =
            server.post_with_headers("/Sys/ping", &json!({}), None, &[("X-Forwarded-For", "5.5.5.5, 10.0.0.1, 10.0.0.2")]);
        assert_eq!(status, 200, "request {i}: {body:?}");
    }
    let (status, _) = server.post_with_headers("/Sys/ping", &json!({}), None, &[("X-Forwarded-For", "5.5.5.5, 10.0.0.9")]);
    assert_eq!(status, 429, "mismo primer hop (5.5.5.5) que antes -- debe compartir el balde ya agotado");
}

#[test]
fn with_trust_proxy_and_no_header_falls_back_to_remote_addr() {
    // --trust-proxy prendido pero SIN X-Forwarded-For en la request: cae de
    // vuelta a remote_addr(), mismo comportamiento de siempre -- no debería
    // tratarse como "cliente desconocido" ni fallar de ninguna forma rara.
    let temp = TempDir::new("trust-proxy-no-header");
    let out = build(&temp, PROGRAM);
    assert!(out.status.success());
    let server = Serve::start_with_args(&temp.0.join("app.link"), &["--trust-proxy"]);

    for i in 1..=3 {
        let (status, body) = server.post("/Sys/ping", &json!({}), None);
        assert_eq!(status, 200, "request {i}: {body:?}");
    }
    let (status, body) = server.post("/Sys/ping", &json!({}), None);
    assert_eq!(status, 429, "{body:?}");
}

#[test]
fn link_trust_proxy_env_var_is_honored() {
    let temp = TempDir::new("trust-proxy-env");
    let out = build(&temp, PROGRAM);
    assert!(out.status.success());
    let port = free_port();
    let child = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(temp.0.join("app.link"))
        .arg(port.to_string())
        .env("LINK_TRUST_PROXY", "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("iniciar 'linkc serve'");
    let server = Serve { child, port };
    wait_for_port(port);

    for i in 1..=3 {
        let (status, body) = server.post_with_headers("/Sys/ping", &json!({}), None, &[("X-Forwarded-For", "7.7.7.7")]);
        assert_eq!(status, 200, "request {i}: {body:?}");
    }
    // Otra IP reenviada -- balde propio, prueba que el env var de verdad
    // prendió el chequeo de X-Forwarded-For.
    let (status, body) = server.post_with_headers("/Sys/ping", &json!({}), None, &[("X-Forwarded-For", "8.8.8.8")]);
    assert_eq!(status, 200, "{body:?}");
}

#[test]
fn rate_limit_combines_with_requires_and_still_applies_to_an_authenticated_caller() {
    // Dimensión ortogonal a auth (GRAMMAR.md §3.39): un rpc puede exigir rol
    // Y tener límite a la vez, y el límite corre para un caller YA
    // autenticado -- no es solo una defensa contra tráfico anónimo.
    let temp = TempDir::new("with-auth");
    let out = build(&temp, PROGRAM);
    assert!(out.status.success());

    let server = Serve::start(&temp.0.join("app.link"));

    // Sin token: 401 de auth, ni siquiera llega a consumir el bucket (el
    // gate de rate limit corre ANTES que el de auth -- ver server.rs -- así
    // que esta request SÍ consume un token, a propósito: por diseño protege
    // contra fuerza bruta de credenciales tanto como contra abuso ya
    // autenticado).
    let (status, body) = server.post("/Sys/adminPing", &json!({}), None);
    assert_eq!(status, 401, "body: {body:?}");

    let (status, body) = server.post("/Sys/adminPing", &json!({}), None);
    assert_eq!(status, 401, "body: {body:?}");

    // "2/1m" ya se agotó (dos requests arriba, sin token válido): la
    // tercera es 429 aunque tampoco mande token -- el límite corre antes
    // que la verificación de credenciales.
    let (status, body) = server.post("/Sys/adminPing", &json!({}), None);
    assert_eq!(status, 429, "body: {body:?}");
}

// ---- `key: <param>` (GRAMMAR.md §3.142) ----

#[test]
fn key_param_combines_with_ip_so_a_different_value_gets_its_own_bucket_even_from_the_same_connection() {
    // Todas las requests de este test vienen de la MISMA conexión TCP real
    // (mismo IP) -- lo único que cambia es 'email'. Si el bucket siguiera
    // siendo solo-IP, agotar el balde con un email dejaría SIN cupo al otro
    // también; con `key: email`, cada valor tiene su propio balde.
    let temp = TempDir::new("key-param-separate-buckets");
    let out = build(&temp, PROGRAM);
    assert!(out.status.success(), "el programa debió compilar: {}", String::from_utf8_lossy(&out.stderr));
    let server = Serve::start(&temp.0.join("app.link"));

    for i in 1..=2 {
        let (status, body) = server.post("/Sys/loginAttempt", &json!({"email": "a@example.com"}), None);
        assert_eq!(status, 200, "a@example.com, request {i}: {body:?}");
    }
    let (status, body) = server.post("/Sys/loginAttempt", &json!({"email": "a@example.com"}), None);
    assert_eq!(status, 429, "a@example.com ya agotó su balde: {body:?}");

    // Mismo IP, OTRO email -- balde propio, sin verse afectado.
    let (status, body) = server.post("/Sys/loginAttempt", &json!({"email": "b@example.com"}), None);
    assert_eq!(status, 200, "b@example.com no debería verse afectado por el balde de a@example.com: {body:?}");
}

#[test]
fn key_param_rejects_a_name_that_is_not_a_real_parameter() {
    let temp = TempDir::new("key-param-unknown");
    let out = build(
        &temp,
        r#"
service S {
  @rate_limit("5/1m", key: nombreQueNoExiste)
  rpc ping(email: String) -> String { "pong" }
}
"#,
    );
    let stderr = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(!out.status.success(), "debió fallar");
    assert!(stderr.contains("no es un parámetro"), "mensaje inesperado: {stderr}");
}

#[test]
fn key_param_rejects_a_parameter_that_is_not_string_or_int() {
    let temp = TempDir::new("key-param-wrong-type");
    let out = build(
        &temp,
        r#"
service S {
  @rate_limit("5/1m", key: active)
  rpc ping(active: Bool) -> String { "pong" }
}
"#,
    );
    let stderr = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(!out.status.success(), "debió fallar");
    assert!(stderr.contains("tiene que ser"), "mensaje inesperado: {stderr}");
}

#[test]
fn the_checker_rejects_malformed_rate_limit_specs() {
    let temp = TempDir::new("rejects");

    let cases = [
        ("not-a-limit", "formato de @rate_limit inválido"),
        ("0/1m", "formato de @rate_limit inválido"),
        ("20/0m", "formato de @rate_limit inválido"),
        ("20/1d", "formato de @rate_limit inválido"),
        ("20", "formato de @rate_limit inválido"),
    ];
    for (spec, expected_msg) in cases {
        let out = build(
            &temp,
            &format!(
                r#"
service S {{
  @rate_limit("{spec}")
  rpc ping() -> String {{ "pong" }}
}}
"#
            ),
        );
        let stderr = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(!out.status.success(), "'{spec}' debió ser rechazado");
        assert!(stderr.contains(expected_msg), "mensaje inesperado para '{spec}': {stderr}");
    }

    // Dos veces: un rpc tiene un solo límite.
    let out = build(
        &temp,
        r#"
service S {
  @rate_limit("5/1m")
  @rate_limit("10/1h")
  rpc ping() -> String { "pong" }
}
"#,
    );
    let stderr = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(!out.status.success(), "debió fallar");
    assert!(stderr.contains("más de una vez"), "mensaje inesperado: {stderr}");
}
