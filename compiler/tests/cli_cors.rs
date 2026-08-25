// CORS configurable y headers de seguridad fijos (GRAMMAR.md §3.41).
//
// Antes de esta ronda, `linkc serve` mandaba `Access-Control-Allow-Origin: *`
// SIEMPRE, sin forma de acotarlo -- cualquier página, de cualquier origen,
// podía leer la respuesta de un rpc con un token Bearer válido en manos del
// usuario (el navegador no lo evita solo: sin este header, sí; con `*`,
// no). Tampoco había ningún header de seguridad más allá de eso. Ambas
// cosas se verifican acá contra el BINARIO real, hablando HTTP de verdad --
// que el código compile no prueba que el servidor mande el header correcto
// para cada `Origin` que llega.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

const PROGRAM: &str = r#"
type Item = { id: Int, name: String }
db { items: Item[] }

service Sys {
  rpc ping() -> String {
    "pong"
  }

  @cors("*")
  rpc pingOpen() -> String {
    "pong"
  }

  @cors("https://partner.example.com")
  rpc pingPartner() -> String {
    "pong"
  }

  stream watchAll() -> Item {
    db.items.all()
  }
}
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "linkc-cors-{name}-{}-{}",
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
    fn start(link_path: &PathBuf, extra_args: &[&str], extra_env: &[(&str, &str)]) -> Self {
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

    /// Manda una request cruda y devuelve (status, headers en minúscula,
    /// body). Headers en minúscula porque HTTP los trata sin distinguir
    /// mayúsculas -- comparar así evita falsos negativos por capitalización.
    fn request(&self, method: &str, path: &str, extra_headers: &[(&str, &str)], body: &str) -> (u16, Vec<(String, String)>, String) {
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

        let mut headers = Vec::new();
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            let n = reader.read_line(&mut line).expect("header");
            if n == 0 || line.trim().is_empty() {
                break;
            }
            if let Some((k, v)) = line.trim().split_once(':') {
                let k = k.trim().to_ascii_lowercase();
                let v = v.trim().to_string();
                if k == "content-length" {
                    content_length = v.parse().unwrap_or(0);
                }
                headers.push((k, v));
            }
        }
        // Un `stream` no manda Content-Length (chunked) -- para lo que
        // estos tests necesitan (headers, no el body completo), no hace
        // falta leer más que eso.
        let mut buf = vec![0u8; content_length];
        let _ = reader.read_exact(&mut buf);
        (status, headers, String::from_utf8_lossy(&buf).to_string())
    }

    fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
        headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Los 3 headers de seguridad que van en TODA respuesta, sin importar CORS
/// (GRAMMAR.md §3.41) -- se revisan juntos en cada test para no repetir.
fn assert_security_headers(headers: &[(String, String)]) {
    assert_eq!(Serve::header(headers, "x-content-type-options"), Some("nosniff"), "headers: {headers:?}");
    assert_eq!(Serve::header(headers, "x-frame-options"), Some("DENY"), "headers: {headers:?}");
    assert_eq!(Serve::header(headers, "referrer-policy"), Some("no-referrer"), "headers: {headers:?}");
}

#[test]
fn default_allows_any_origin_and_still_adds_security_headers() {
    let temp = TempDir::new("default");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &[], &[]);

    let (status, headers, _) = server.request("POST", "/Sys/ping", &[("Origin", "https://anything.example.com")], "{}");
    assert_eq!(status, 200);
    assert_eq!(Serve::header(&headers, "access-control-allow-origin"), Some("*"), "headers: {headers:?}");
    assert!(Serve::header(&headers, "vary").is_none(), "con '*' no hace falta Vary: Origin: {headers:?}");
    assert_security_headers(&headers);
}

#[test]
fn allowlist_echoes_a_matching_origin_and_omits_the_header_for_others() {
    let temp = TempDir::new("allowlist");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &["--cors-origin", "https://app.example.com", "--cors-origin", "https://admin.example.com"], &[]);

    // Origen permitido: se ecoa EXACTO (nunca '*'), con Vary: Origin.
    let (status, headers, _) = server.request("POST", "/Sys/ping", &[("Origin", "https://app.example.com")], "{}");
    assert_eq!(status, 200);
    assert_eq!(Serve::header(&headers, "access-control-allow-origin"), Some("https://app.example.com"), "headers: {headers:?}");
    assert_eq!(Serve::header(&headers, "vary"), Some("Origin"), "headers: {headers:?}");
    assert_security_headers(&headers);

    // Origen NO permitido: la request se procesa igual (200, "pong" real --
    // CORS lo hace cumplir el NAVEGADOR sobre la respuesta, no el server
    // rechazando la request), pero sin el header -- así el navegador de
    // quien sí lo respeta bloquea la lectura.
    let (status, headers, body) = server.request("POST", "/Sys/ping", &[("Origin", "https://evil.example.com")], "{}");
    assert_eq!(status, 200);
    assert_eq!(body, "\"pong\"");
    assert!(Serve::header(&headers, "access-control-allow-origin").is_none(), "headers: {headers:?}");
    assert_security_headers(&headers);

    // Preflight (OPTIONS) sigue el mismo criterio.
    let (status, headers, _) = server.request(
        "OPTIONS",
        "/Sys/ping",
        &[("Origin", "https://app.example.com"), ("Access-Control-Request-Method", "POST")],
        "",
    );
    assert_eq!(status, 204);
    assert_eq!(Serve::header(&headers, "access-control-allow-origin"), Some("https://app.example.com"), "headers: {headers:?}");
}

#[test]
fn link_cors_origins_env_var_is_a_comma_separated_allowlist() {
    let temp = TempDir::new("env");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &[], &[("LINK_CORS_ORIGINS", "https://a.example.com, https://b.example.com")]);

    let (status, headers, _) = server.request("POST", "/Sys/ping", &[("Origin", "https://b.example.com")], "{}");
    assert_eq!(status, 200);
    assert_eq!(Serve::header(&headers, "access-control-allow-origin"), Some("https://b.example.com"), "headers: {headers:?}");

    let (_, headers, _) = server.request("POST", "/Sys/ping", &[("Origin", "https://c.example.com")], "{}");
    assert!(Serve::header(&headers, "access-control-allow-origin").is_none(), "headers: {headers:?}");
}

#[test]
fn cli_flags_take_precedence_over_the_env_var() {
    let temp = TempDir::new("precedence");
    let src = temp.write("app.link", PROGRAM);
    // El flag dice SOLO a.example.com; el env var dice otra cosa --
    // mismo criterio de precedencia que --db/LINK_DATABASE_URL.
    let server =
        Serve::start(&src, &["--cors-origin", "https://a.example.com"], &[("LINK_CORS_ORIGINS", "https://b.example.com")]);

    let (_, headers, _) = server.request("POST", "/Sys/ping", &[("Origin", "https://a.example.com")], "{}");
    assert_eq!(Serve::header(&headers, "access-control-allow-origin"), Some("https://a.example.com"), "headers: {headers:?}");

    let (_, headers, _) = server.request("POST", "/Sys/ping", &[("Origin", "https://b.example.com")], "{}");
    assert!(Serve::header(&headers, "access-control-allow-origin").is_none(), "el flag debe ganarle al env var: {headers:?}");
}

#[test]
fn security_headers_are_present_on_error_responses_too() {
    let temp = TempDir::new("errors");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &[], &[]);

    let (status, headers, _) = server.request("POST", "/Sys/doesNotExist", &[], "{}");
    assert_eq!(status, 500, "un rpc inexistente en un servicio real da 500 (runtime error), no un 404 genérico");
    assert_security_headers(&headers);
    assert_eq!(Serve::header(&headers, "access-control-allow-origin"), Some("*"), "headers: {headers:?}");
}

#[test]
fn a_stream_response_carries_the_same_cors_policy_as_a_normal_rpc() {
    // `stream` arma su preámbulo HTTP a mano (runtime/server.rs::sse_preamble),
    // sin pasar por el mismo builder que un rpc normal -- si las dos rutas
    // divergieran, un stream podría filtrar datos a un origen que la
    // allowlist de un rpc normal ya rechaza. Se prueba explícitamente para
    // que esa divergencia no pase desapercibida.
    let temp = TempDir::new("stream");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &["--cors-origin", "https://app.example.com"], &[]);

    let (status, headers, _) = server.request("POST", "/Sys/watchAll", &[("Origin", "https://app.example.com")], "{}");
    assert_eq!(status, 200);
    assert_eq!(Serve::header(&headers, "access-control-allow-origin"), Some("https://app.example.com"), "headers: {headers:?}");
    assert_eq!(Serve::header(&headers, "vary"), Some("Origin"), "headers: {headers:?}");
    assert_security_headers(&headers);

    let (status, headers, _) = server.request("POST", "/Sys/watchAll", &[("Origin", "https://evil.example.com")], "{}");
    assert_eq!(status, 200);
    assert!(Serve::header(&headers, "access-control-allow-origin").is_none(), "headers: {headers:?}");
}

// ---- `@cors("...")` override por ruta (GRAMMAR.md §3.147) ----

#[test]
fn a_route_with_cors_star_ignores_the_global_allowlist_and_is_open_to_any_origin() {
    let temp = TempDir::new("cors-override-star");
    let src = temp.write("app.link", PROGRAM);
    // Global: allowlist restrictivo que NO incluye evil.example.com.
    let server = Serve::start(&src, &["--cors-origin", "https://app.example.com"], &[]);

    // El rpc SIN override sigue respetando el allowlist global.
    let (_, headers, _) = server.request("POST", "/Sys/ping", &[("Origin", "https://evil.example.com")], "{}");
    assert!(Serve::header(&headers, "access-control-allow-origin").is_none(), "headers: {headers:?}");

    // El rpc CON @cors("*") ignora el allowlist global -- abierto a
    // cualquier origen, mismo criterio que CorsConfig::Any.
    let (status, headers, body) = server.request("POST", "/Sys/pingOpen", &[("Origin", "https://evil.example.com")], "{}");
    assert_eq!(status, 200);
    assert_eq!(body, "\"pong\"");
    assert_eq!(Serve::header(&headers, "access-control-allow-origin"), Some("*"), "headers: {headers:?}");
    assert_security_headers(&headers);
}

#[test]
fn a_route_with_a_cors_allowlist_override_ignores_the_global_config() {
    let temp = TempDir::new("cors-override-allowlist");
    let src = temp.write("app.link", PROGRAM);
    // Global: abierto a cualquier origen (default, sin --cors-origin).
    let server = Serve::start(&src, &[], &[]);

    // El rpc SIN override sigue abierto (comportamiento global de siempre).
    let (_, headers, _) = server.request("POST", "/Sys/ping", &[("Origin", "https://evil.example.com")], "{}");
    assert_eq!(Serve::header(&headers, "access-control-allow-origin"), Some("*"), "headers: {headers:?}");

    // `pingPartner` tiene @cors("https://partner.example.com") -- ignora el
    // global abierto, se comporta como un allowlist propio de un origen.
    let (status, headers, _) = server.request("POST", "/Sys/pingPartner", &[("Origin", "https://partner.example.com")], "{}");
    assert_eq!(status, 200);
    assert_eq!(Serve::header(&headers, "access-control-allow-origin"), Some("https://partner.example.com"), "headers: {headers:?}");
    assert_eq!(Serve::header(&headers, "vary"), Some("Origin"), "headers: {headers:?}");

    let (status, headers, body) = server.request("POST", "/Sys/pingPartner", &[("Origin", "https://otro.example.com")], "{}");
    assert_eq!(status, 200);
    assert_eq!(body, "\"pong\"", "CORS lo hace cumplir el navegador, no el server rechazando la request");
    assert!(Serve::header(&headers, "access-control-allow-origin").is_none(), "headers: {headers:?}");
}

#[test]
fn the_cors_override_also_applies_to_the_options_preflight() {
    // Crítico: si el preflight no anuncia el override, el navegador nunca
    // manda la request real -- el override tiene que aplicar a los DOS.
    let temp = TempDir::new("cors-override-preflight");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &["--cors-origin", "https://app.example.com"], &[]);

    let (status, headers, _) = server.request("OPTIONS", "/Sys/pingPartner", &[("Origin", "https://partner.example.com")], "");
    assert_eq!(status, 204);
    assert_eq!(Serve::header(&headers, "access-control-allow-origin"), Some("https://partner.example.com"), "headers: {headers:?}");
}

#[test]
fn a_cors_origin_flag_without_a_value_is_a_clean_cli_error() {
    let temp = TempDir::new("badflag");
    let src = temp.write("app.link", PROGRAM);
    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&src)
        .arg(free_port().to_string())
        .arg("--cors-origin")
        .output()
        .expect("ejecutar linkc serve");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--cors-origin"), "el mensaje debe nombrar el flag: {stderr}");
    assert!(!stderr.contains("panicked at"), "un flag mal usado es un error de uso, no un panic: {stderr}");
}
