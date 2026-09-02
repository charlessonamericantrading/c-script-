// `--fallback-upstream <url>` (GRAMMAR.md §3.238, PLAN.md §9.18 Eje E ítem 3,
// prerrequisito del estrangulamiento de §9.20): toda request que este
// `.link` NO declara va tal cual al backend viejo y vuelve con su status,
// Content-Type y body; lo que el `.link` sí declara se sirve local; sin el
// flag, el 404 de siempre. Contra un upstream falso en un puerto efímero
// que devuelve método y path para poder afirmar qué recibió.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

const PROGRAM: &str = r#"
type Item = { id: Int, name: String }
db { items: Item[] }
service Items {
  rpc list() -> Item[] { db.items.all() }
}
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-fallback-{name}-{}-{}",
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

/// Upstream falso: responde `upstream:<MÉTODO> <path>|<body>` como texto,
/// 200 salvo `/old/fail` (503) -- y refleja el header `X-Trace` si vino.
fn start_fake_upstream() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bindear puerto efímero");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    return;
                }
                let mut content_length = 0usize;
                let mut trace = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                        break;
                    }
                    if let Some((k, v)) = line.trim().split_once(':') {
                        if k.trim().eq_ignore_ascii_case("content-length") {
                            content_length = v.trim().parse().unwrap_or(0);
                        }
                        if k.trim().eq_ignore_ascii_case("x-trace") {
                            trace = v.trim().to_string();
                        }
                    }
                }
                let mut body = vec![0u8; content_length];
                let _ = reader.read_exact(&mut body);
                let mut parts = request_line.split_whitespace();
                let method = parts.next().unwrap_or("?");
                let path = parts.next().unwrap_or("/");
                let status = if path.starts_with("/old/fail") { "503 Service Unavailable" } else { "200 OK" };
                let text = format!("upstream:{method} {path}|{}|{trace}", String::from_utf8_lossy(&body));
                let response = format!("HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len());
                let _ = stream.write_all(response.as_bytes());
            });
        }
    });
    port
}

struct Serve {
    child: Child,
    port: u16,
}

impl Serve {
    fn start(link_path: &PathBuf, extra: &[&str]) -> Self {
        let port = free_port();
        let child = Command::new(env!("CARGO_BIN_EXE_linkc"))
            .arg("serve")
            .arg(link_path)
            .arg(port.to_string())
            .args(extra)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("iniciar 'linkc serve'");
        let server = Serve { child, port };
        for _ in 0..200 {
            if server.request("GET", "/live", "", &[]).is_some() {
                return server;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("'linkc serve' no abrió el puerto {port} a tiempo");
    }

    fn request(&self, method: &str, path: &str, body: &str, headers: &[(&str, &str)]) -> Option<(u16, String, String)> {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).ok()?;
        stream.set_read_timeout(Some(Duration::from_secs(30))).ok()?;
        let mut req = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n", body.len());
        for (k, v) in headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        req.push_str("\r\n");
        req.push_str(body);
        stream.write_all(req.as_bytes()).ok()?;
        let mut reader = BufReader::new(stream);
        let mut status_line = String::new();
        reader.read_line(&mut status_line).ok()?;
        let status: u16 = status_line.split_whitespace().nth(1)?.parse().ok()?;
        let mut content_length = 0usize;
        let mut content_type = String::new();
        loop {
            let mut line = String::new();
            let n = reader.read_line(&mut line).ok()?;
            if n == 0 || line.trim().is_empty() {
                break;
            }
            if let Some((k, v)) = line.trim().split_once(':') {
                if k.trim().eq_ignore_ascii_case("content-length") {
                    content_length = v.trim().parse().unwrap_or(0);
                }
                if k.trim().eq_ignore_ascii_case("content-type") {
                    content_type = v.trim().to_string();
                }
            }
        }
        let mut buf = vec![0u8; content_length];
        reader.read_exact(&mut buf).ok()?;
        Some((status, content_type, String::from_utf8_lossy(&buf).to_string()))
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn undeclared_paths_and_rpcs_go_to_the_upstream_while_declared_ones_stay_local() {
    let upstream = start_fake_upstream();
    let temp = TempDir::new("proxy");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &["--fallback-upstream", &format!("http://127.0.0.1:{upstream}/")]);

    // Lo declarado se sirve local, sin tocar el upstream.
    let (status, ct, body) = server.request("POST", "/Items/list", "{}", &[]).unwrap();
    assert_eq!(status, 200, "{body}");
    assert!(ct.contains("application/json"), "{ct}");
    assert_eq!(body.trim(), "[]", "{body}");

    // Un path sin forma /Service/rpc: proxy, método, path con query, body y
    // headers propios (X-Trace) intactos; status y Content-Type del upstream.
    let (status, ct, body) = server.request("POST", "/api/v1/users?page=2", r#"{"q":1}"#, &[("X-Trace", "abc")]).unwrap();
    assert_eq!(status, 200, "{body}");
    assert!(ct.starts_with("text/plain"), "el Content-Type es el del upstream: {ct}");
    assert_eq!(body, r#"upstream:POST /api/v1/users?page=2|{"q":1}|abc"#, "{body}");

    // Un GET sin body también.
    let (status, _, body) = server.request("GET", "/old/report", "", &[]).unwrap();
    assert_eq!(status, 200, "{body}");
    assert_eq!(body, "upstream:GET /old/report||", "{body}");

    // /Service/rpc con la forma correcta pero que este .link no declara.
    let (status, _, body) = server.request("POST", "/Legacy/doThing", "{}", &[]).unwrap();
    assert_eq!(status, 200, "{body}");
    assert!(body.starts_with("upstream:POST /Legacy/doThing|"), "{body}");

    // El status del upstream se copia tal cual (503 sigue siendo 503).
    let (status, _, body) = server.request("GET", "/old/fail", "", &[]).unwrap();
    assert_eq!(status, 503, "{body}");

    // /health, /live y /metrics siguen siendo de este proceso.
    let (status, _, body) = server.request("GET", "/live", "", &[]).unwrap();
    assert_eq!(status, 200);
    assert!(!body.starts_with("upstream:"), "{body}");
    let (status, _, metrics) = server.request("GET", "/metrics", "", &[]).unwrap();
    assert_eq!(status, 200);
    assert!(metrics.contains(&format!("linkc_http_outbound_total{{host=\"127.0.0.1:{upstream}\",status=\"2xx\"}}")), "el proxy cuenta como llamada saliente: {metrics}");
}

#[test]
fn an_unreachable_upstream_is_a_502_with_the_reason_and_without_the_flag_it_is_the_usual_404() {
    let dead = free_port();
    let temp = TempDir::new("dead");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &["--fallback-upstream", &format!("http://127.0.0.1:{dead}")]);
    let (status, _, body) = server.request("GET", "/old/report", "", &[]).unwrap();
    assert_eq!(status, 502, "{body}");
    assert!(body.contains("--fallback-upstream") && body.contains("no respondió"), "{body}");
    drop(server);

    let server = Serve::start(&src, &[]);
    let (status, _, _) = server.request("GET", "/old/report", "", &[]).unwrap();
    assert_eq!(status, 404);
    let (status, _, _) = server.request("POST", "/Legacy/doThing", "{}", &[]).unwrap();
    assert_eq!(status, 404);
}

#[test]
fn a_malformed_upstream_url_is_rejected_at_startup() {
    let temp = TempDir::new("bad");
    let src = temp.write("app.link", PROGRAM);
    for bad in ["localhost:3000", "http://host/with/path", "ftp://x"] {
        let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
            .arg("serve")
            .arg(&src)
            .arg(free_port().to_string())
            .arg("--fallback-upstream")
            .arg(bad)
            .output()
            .expect("ejecutar linkc");
        assert!(!out.status.success(), "{bad}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("--fallback-upstream"), "{bad}: {err}");
    }
}
