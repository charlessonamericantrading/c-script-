// `--max-concurrency <N>` (GRAMMAR.md §3.241, PLAN.md §9.18 Eje B ítem 2,
// prerrequisito de §9.20 con el motor en proceso): la request N+1 recibe
// 503 + `Retry-After` en vez de un hilo; `/live` nunca cuenta ni se
// rechaza; `/metrics` cuenta los rechazos; y sin el flag no hay tope. Un
// rpc "lento" se fabrica con un `http.get` contra un upstream falso que
// tarda 2 s en responder -- el lenguaje no tiene `sleep`, y así el tiempo
// lo gasta una llamada saliente real.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-maxconc-{name}-{}-{}",
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

/// Upstream falso que tarda 2 s en responder "slow".
fn start_slow_upstream() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bindear puerto efímero");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 0 {
                    if line.trim().is_empty() {
                        break;
                    }
                    line.clear();
                }
                std::thread::sleep(Duration::from_secs(2));
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 4\r\nConnection: close\r\n\r\nslow");
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
            if request(port, "GET", "/live", "").is_some() {
                return server;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("'linkc serve' no abrió el puerto {port} a tiempo");
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// `(status, headers en minúscula, body)`.
type Reply = (u16, Vec<(String, String)>, String);

fn request(port: u16, method: &str, path: &str, body: &str) -> Option<Reply> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(30))).ok()?;
    let req = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len());
    stream.write_all(req.as_bytes()).ok()?;
    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader.read_line(&mut status_line).ok()?;
    let status: u16 = status_line.split_whitespace().nth(1)?.parse().ok()?;
    let mut headers = Vec::new();
    let mut content_length = 0usize;
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
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    let mut buf = vec![0u8; content_length];
    reader.read_exact(&mut buf).ok()?;
    Some((status, headers, String::from_utf8_lossy(&buf).to_string()))
}

#[test]
fn the_request_over_the_limit_gets_503_with_retry_after_while_live_still_answers() {
    let upstream = start_slow_upstream();
    let temp = TempDir::new("limit");
    let src = temp.write(
        "app.link",
        &format!("service Slow {{ rpc wait() -> String {{ http.get(\"http://127.0.0.1:{upstream}/\") }} rpc fast() -> Int {{ 1 }} }}\n"),
    );
    let server = Serve::start(&src, &["--max-concurrency", "1"]);
    let port = server.port;

    // Una request lenta ocupa el único slot durante ~2 s.
    let slow = std::thread::spawn(move || request(port, "POST", "/Slow/wait", "{}"));
    std::thread::sleep(Duration::from_millis(300));

    // La siguiente NO espera: 503 inmediato con Retry-After.
    let started = std::time::Instant::now();
    let (status, headers, body) = request(port, "POST", "/Slow/fast", "{}").unwrap();
    assert_eq!(status, 503, "{body}");
    assert!(started.elapsed() < Duration::from_millis(1500), "el rechazo tiene que ser inmediato, no encolarse");
    assert!(headers.iter().any(|(k, v)| k == "retry-after" && v == "1"), "{headers:?}");
    assert!(body.contains("--max-concurrency") && body.contains("§3.241"), "{body}");

    // /live sigue respondiendo aunque el proceso esté saturado.
    let (status, _, _) = request(port, "GET", "/live", "").unwrap();
    assert_eq!(status, 200);

    // Cuando la lenta termina, el slot se libera.
    let (status, _, body) = slow.join().unwrap().unwrap();
    assert_eq!(status, 200, "{body}");
    let (status, _, body) = request(port, "POST", "/Slow/fast", "{}").unwrap();
    assert_eq!(status, 200, "{body}");

    // Y /metrics cuenta el rechazo.
    let (status, _, metrics) = request(port, "GET", "/metrics", "").unwrap();
    assert_eq!(status, 200);
    assert!(metrics.contains("linkc_http_saturated_total 1"), "{metrics}");
}

#[test]
fn without_the_flag_there_is_no_limit_and_a_bad_value_is_rejected_at_startup() {
    let temp = TempDir::new("nolimit");
    let src = temp.write("app.link", "service S { rpc fast() -> Int { 1 } }\n");
    let server = Serve::start(&src, &[]);
    let port = server.port;
    let handles: Vec<_> = (0..8).map(|_| std::thread::spawn(move || request(port, "POST", "/S/fast", "{}").map(|r| r.0))).collect();
    for h in handles {
        assert_eq!(h.join().unwrap(), Some(200));
    }
    let (_, _, metrics) = request(port, "GET", "/metrics", "").unwrap();
    assert!(!metrics.contains("linkc_http_saturated_total"), "sin tope no hay serie inventada: {metrics}");
    drop(server);

    for bad in ["0", "-3", "muchos"] {
        let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
            .arg("serve")
            .arg(&src)
            .arg(free_port().to_string())
            .arg("--max-concurrency")
            .arg(bad)
            .output()
            .expect("ejecutar linkc");
        assert!(!out.status.success(), "{bad}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("--max-concurrency"), "{bad}");
    }
}
