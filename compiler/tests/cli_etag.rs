// `ETag` débil + `If-None-Match` → `304` en toda respuesta 200 a un GET, y
// `Vary: Accept-Encoding` cuando el body es candidato a gzip (GRAMMAR.md
// §3.221, PLAN.md §9.18 Eje B ítem 4 / Eje D ítem 3). Verificado por socket
// crudo contra un `linkc serve` real -- headers leídos del wire, no
// inferidos.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

const PROGRAM: &str = r#"
service Pages {
  @route("/page")
  @content_type("text/html; charset=utf-8")
  @cache_control("public, max-age=60")
  rpc page() -> String { "<h1>hola</h1>" }

  @route("/big")
  @content_type("text/plain; charset=utf-8")
  rpc big() -> String { "a".padEnd(3000, "b") }

  rpc data() -> Int { 1 }
}
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-etag-{name}-{}-{}",
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
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

struct Serve {
    child: Child,
    port: u16,
}

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
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
        let server = Serve { child, port };
        for _ in 0..200 {
            if server.request("GET", "/live", &[], "").is_some() {
                return server;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("'linkc serve' no abrió el puerto {port} a tiempo");
    }

    /// Una request cruda con `Connection: close`; la respuesta se lee hasta
    /// EOF y se parte en status/headers/body -- para un 304 el body es vacío
    /// por definición del wire, no porque este parser lo asuma.
    fn request(&self, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> Option<Reply> {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).ok()?;
        let mut req = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
        for (k, v) in headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        if !body.is_empty() {
            req.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", body.len()));
        }
        req.push_str("\r\n");
        req.push_str(body);
        stream.write_all(req.as_bytes()).ok()?;
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).ok()?;
        let split = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
        let head = String::from_utf8_lossy(&raw[..split]).to_string();
        let body = raw[split + 4..].to_vec();
        let mut lines = head.lines();
        let status: u16 = lines.next()?.split_whitespace().nth(1)?.parse().ok()?;
        let headers = lines
            .filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
            .collect();
        Some(Reply { status, headers, body })
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn a_get_route_carries_a_weak_etag_and_a_matching_if_none_match_gets_a_bodyless_304() {
    let temp = TempDir::new("etag");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src);

    let first = server.request("GET", "/page", &[], "").expect("responde");
    assert_eq!(first.status, 200);
    let etag = first.header("ETag").expect("un GET 200 lleva ETag").to_string();
    assert!(etag.starts_with("W/\""), "ETag débil, calculado sobre el body sin comprimir: {etag}");
    assert_eq!(String::from_utf8_lossy(&first.body), "<h1>hola</h1>");

    // Mismo body → mismo ETag (determinista, sin reloj ni nonce).
    let again = server.request("GET", "/page", &[], "").expect("responde");
    assert_eq!(again.header("ETag"), Some(etag.as_str()));

    let revalidate = server.request("GET", "/page", &[("If-None-Match", &etag)], "").expect("responde");
    assert_eq!(revalidate.status, 304, "{}", String::from_utf8_lossy(&revalidate.body));
    assert!(revalidate.body.is_empty(), "un 304 nunca lleva body");
    assert_eq!(revalidate.header("ETag"), Some(etag.as_str()), "el 304 repite el ETag");
    assert_eq!(revalidate.header("Cache-Control"), Some("public, max-age=60"), "el 304 conserva @cache_control");
    assert!(revalidate.header("Content-Encoding").is_none());

    // Un ETag ajeno (o la forma fuerte del mismo valor) NO coincide → 200 normal.
    let stale = server.request("GET", "/page", &[("If-None-Match", "\"otro\"")], "").expect("responde");
    assert_eq!(stale.status, 200);
    let strong_same = server.request("GET", "/page", &[("If-None-Match", etag.trim_start_matches("W/"))], "").expect("responde");
    assert_eq!(strong_same.status, 304, "comparación débil: W/\"x\" y \"x\" son el mismo recurso");
}

#[test]
fn a_post_rpc_carries_no_etag_and_ignores_if_none_match() {
    let temp = TempDir::new("post");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src);

    let reply = server.request("POST", "/Pages/data", &[("If-None-Match", "*")], "{}").expect("responde");
    assert_eq!(reply.status, 200, "{}", String::from_utf8_lossy(&reply.body));
    assert!(reply.header("ETag").is_none(), "un POST no es cacheable: sin ETag");
}

#[test]
fn a_gzip_candidate_body_varies_by_accept_encoding_and_keeps_the_same_etag_compressed_or_not() {
    let temp = TempDir::new("vary");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src);

    let plain = server.request("GET", "/big", &[], "").expect("responde");
    assert_eq!(plain.status, 200);
    assert!(plain.header("Content-Encoding").is_none());
    assert_eq!(plain.body.len(), 3000);
    let vary = plain.header("Vary").unwrap_or("").to_string();
    assert!(vary.contains("Accept-Encoding"), "Vary: {vary}");

    let gz = server.request("GET", "/big", &[("Accept-Encoding", "gzip")], "").expect("responde");
    assert_eq!(gz.header("Content-Encoding"), Some("gzip"));
    assert!(gz.body.len() < 3000, "comprimido de verdad: {} bytes", gz.body.len());
    assert_eq!(gz.header("ETag"), plain.header("ETag"), "el ETag débil es del body sin comprimir: igual en las dos representaciones");
    assert!(gz.header("Vary").unwrap_or("").contains("Accept-Encoding"));

    // Y la revalidación funciona igual aunque el cliente acepte gzip.
    let etag = plain.header("ETag").unwrap().to_string();
    let revalidate = server.request("GET", "/big", &[("Accept-Encoding", "gzip"), ("If-None-Match", &etag)], "").expect("responde");
    assert_eq!(revalidate.status, 304);
    assert!(revalidate.body.is_empty());

    // Un body chico (bajo el umbral de gzip) no varía por encoding.
    let small = server.request("GET", "/page", &[("Accept-Encoding", "gzip")], "").expect("responde");
    assert!(!small.header("Vary").unwrap_or("").contains("Accept-Encoding"), "Vary: {:?}", small.header("Vary"));
}
