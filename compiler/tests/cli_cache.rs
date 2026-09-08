// `cache.clear()`/`cache.clear(prefix)`/`cache.stats()` + `X-Cache: HIT|MISS`
// automático (GRAMMAR.md §3.293, PLAN.md §9.24 Fase 2 ítem G4): API
// imperativa sobre el MISMO `CacheStore` que ya sirve `@cache` (§3.144) --
// nunca una copia separada.
//
// Contra el BINARIO real vía `linkc serve`.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

const PROGRAM: &str = r#"
type Stats = { entries: Int, hits: Int, misses: Int }

service Pages {
  @cache("60s")
  rpc home() -> String {
    "rendered"
  }

  @cache("60s")
  rpc about() -> String {
    "about page"
  }
}

service Admin {
  rpc clearAll() -> Void {
    cache.clear()
  }
  rpc clearPrefix(p: String) -> Void {
    cache.clear(p)
  }
  rpc getStats() -> Stats {
    cache.stats()
  }
}
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-cache-{name}-{}-{}",
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

    /// POST crudo -- devuelve (status, body, headers).
    fn post(&self, path: &str) -> (u16, String, Vec<(String, String)>) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("conectar");
        let body = "{}";
        let req = format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            self.port,
            body.len()
        );
        stream.write_all(req.as_bytes()).unwrap();
        stream.flush().ok();
        let mut resp = String::new();
        stream.read_to_string(&mut resp).ok();
        let mut parts = resp.splitn(2, "\r\n\r\n");
        let head = parts.next().unwrap_or_default();
        let body = parts.next().unwrap_or_default().to_string();
        let mut lines = head.lines();
        let status: u16 = lines.next().and_then(|l| l.split_whitespace().nth(1)).and_then(|s| s.parse().ok()).unwrap_or(0);
        let headers = lines
            .filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
            .collect();
        (status, body, headers)
    }

    fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
        headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn x_cache_header_is_miss_then_hit_across_two_calls() {
    let temp = TempDir::new("hitmiss");
    let link_path = temp.write("app.link", PROGRAM);
    let server = Serve::start(&link_path);

    let (status, _, headers) = server.post("/Pages/home");
    assert_eq!(status, 200);
    assert_eq!(Serve::header(&headers, "X-Cache"), Some("MISS"));

    let (status, _, headers) = server.post("/Pages/home");
    assert_eq!(status, 200);
    assert_eq!(Serve::header(&headers, "X-Cache"), Some("HIT"));
}

#[test]
fn a_non_cached_rpc_never_gets_an_x_cache_header() {
    let temp = TempDir::new("nocache");
    let link_path = temp.write("app.link", PROGRAM);
    let server = Serve::start(&link_path);

    let (status, _, headers) = server.post("/Admin/getStats");
    assert_eq!(status, 200);
    assert_eq!(Serve::header(&headers, "X-Cache"), None, "un rpc sin @cache nunca debería tener este header");
}

#[test]
fn stats_reports_correct_entries_hits_and_misses() {
    let temp = TempDir::new("stats");
    let link_path = temp.write("app.link", PROGRAM);
    let server = Serve::start(&link_path);

    server.post("/Pages/home"); // miss
    server.post("/Pages/home"); // hit
    server.post("/Pages/about"); // miss
    server.post("/Pages/missing-does-not-exist"); // 404, no toca el cache

    let (status, body, _) = server.post("/Admin/getStats");
    assert_eq!(status, 200, "{body}");
    let stats: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(stats["entries"], 2, "{body}");
    assert_eq!(stats["hits"], 1, "{body}");
    assert_eq!(stats["misses"], 2, "{body}");
}

#[test]
fn clear_empties_the_cache_and_a_later_call_is_a_miss_again() {
    let temp = TempDir::new("clear");
    let link_path = temp.write("app.link", PROGRAM);
    let server = Serve::start(&link_path);

    server.post("/Pages/home");
    let (_, _, headers) = server.post("/Pages/home");
    assert_eq!(Serve::header(&headers, "X-Cache"), Some("HIT"), "confirmá que había un hit antes de limpiar");

    let (status, _, _) = server.post("/Admin/clearAll");
    assert_eq!(status, 200);

    let (_, _, headers) = server.post("/Pages/home");
    assert_eq!(Serve::header(&headers, "X-Cache"), Some("MISS"), "después de clear(), tiene que volver a ser un miss");
}

#[test]
fn clear_prefix_removes_only_the_matching_rpc() {
    let temp = TempDir::new("clear-prefix");
    let link_path = temp.write("app.link", PROGRAM);
    let server = Serve::start(&link_path);

    server.post("/Pages/home");
    server.post("/Pages/about");

    let mut stream = TcpStream::connect(("127.0.0.1", server.port)).expect("conectar");
    let body = serde_json::json!({"p": "Pages.home"}).to_string();
    let req = format!(
        "POST /Admin/clearPrefix HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        server.port,
        body.len()
    );
    stream.write_all(req.as_bytes()).unwrap();
    stream.flush().ok();
    let mut resp = String::new();
    stream.read_to_string(&mut resp).ok();
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");

    let (_, _, headers) = server.post("/Pages/home");
    assert_eq!(Serve::header(&headers, "X-Cache"), Some("MISS"), "Pages.home fue borrado por el prefix");

    let (_, _, headers) = server.post("/Pages/about");
    assert_eq!(Serve::header(&headers, "X-Cache"), Some("HIT"), "Pages.about NO debería haberse borrado");
}

#[test]
fn setting_the_x_cache_header_manually_is_rejected_at_runtime() {
    // `response.setHeader` (§3.279) tiene 'x-cache' en su lista reservada
    // desde este ítem -- un programa que intente pisarlo a mano falla
    // limpio en RUNTIME (mismo mecanismo ya probado para el resto de la
    // lista reservada), nunca produce un header duplicado.
    let temp = TempDir::new("reserved");
    let src = temp.write(
        "app.link",
        r#"
service S {
  rpc f() -> String {
    response.setHeader("X-Cache", "totally-fake");
    "x"
  }
}
"#,
    );
    let server = Serve::start(&src);
    let (status, body, _) = server.post("/S/f");
    assert_eq!(status, 500, "{body}");
    assert!(body.contains("X-Cache") || body.contains("x-cache"), "{body}");
}
