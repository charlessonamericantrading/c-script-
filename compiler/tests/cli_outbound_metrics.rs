// `linkc_http_outbound_total`/`linkc_http_outbound_duration_seconds_sum`
// en `GET /metrics` (GRAMMAR.md §3.223, PLAN.md §9.18 Eje F ítem 5): cada
// llamada `http.*` SALIENTE contada por host y clase de status. Verificado
// contra un `linkc serve` real hablándole a un upstream de mentira real por
// socket -- el conteo sale del wire, no de un mock del intérprete.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-outbound-{name}-{}-{}",
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

/// Upstream de mentira: `/ok` → 200, cualquier otro path → 500. Una
/// conexión por request, `Connection: close`, `Content-Length` correcto --
/// lo mínimo para que `ureq` lo acepte como respuesta HTTP/1.1 válida.
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
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                        break;
                    }
                }
                let path = request_line.split_whitespace().nth(1).unwrap_or("/");
                let (status, body) = if path == "/ok" { ("200 OK", "fine") } else { ("500 Internal Server Error", "boom") };
                let response = format!("HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
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
            if server.request("GET", "/live", "").is_some() {
                return server;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("'linkc serve' no abrió el puerto {port} a tiempo");
    }

    fn request(&self, method: &str, path: &str, body: &str) -> Option<(u16, String)> {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).ok()?;
        let mut req = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
        if !body.is_empty() {
            req.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", body.len()));
        }
        req.push_str("\r\n");
        req.push_str(body);
        stream.write_all(req.as_bytes()).ok()?;
        let mut raw = String::new();
        stream.read_to_string(&mut raw).ok()?;
        let (head, body) = raw.split_once("\r\n\r\n")?;
        let status: u16 = head.lines().next()?.split_whitespace().nth(1)?.parse().ok()?;
        Some((status, body.to_string()))
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn metric_value(metrics: &str, name: &str, labels: &str) -> Option<f64> {
    let prefix = format!("{name}{{{labels}}} ");
    metrics.lines().find(|l| l.starts_with(&prefix)).and_then(|l| l[prefix.len()..].trim().parse().ok())
}

#[test]
fn metrics_counts_outbound_http_calls_by_host_and_status_class_with_their_duration() {
    let upstream = start_fake_upstream();
    let program = format!(
        r#"
service Probe {{
  rpc ok() -> String {{ http.get("http://127.0.0.1:{upstream}/ok") }}
  rpc failStatus() -> Int {{ http.getWithStatus("http://127.0.0.1:{upstream}/fail", []).status }}
  rpc failHard() -> String {{ http.get("http://127.0.0.1:{upstream}/fail") }}
}}
"#
    );
    let temp = TempDir::new("count");
    let src = temp.write("app.link", &program);
    let server = Serve::start(&src);

    // Dos 200 por `http.get`, un 500 leído como DATO (`getWithStatus`) y un
    // 500 que `http.get` convierte en error del rpc: los cuatro tienen que
    // contarse, incluido el que terminó en un 500 del propio rpc.
    assert_eq!(server.request("POST", "/Probe/ok", "{}").unwrap().0, 200);
    assert_eq!(server.request("POST", "/Probe/ok", "{}").unwrap().0, 200);
    let (status, body) = server.request("POST", "/Probe/failStatus", "{}").unwrap();
    assert_eq!(status, 200, "{body}");
    assert_eq!(body.trim(), "500");
    let (status, _) = server.request("POST", "/Probe/failHard", "{}").unwrap();
    assert_eq!(status, 500, "http.get sobre un 500 sigue siendo un error del rpc, sin cambios");

    let (status, metrics) = server.request("GET", "/metrics", "").unwrap();
    assert_eq!(status, 200);
    let host = format!("host=\"127.0.0.1:{upstream}\"");
    assert_eq!(metric_value(&metrics, "linkc_http_outbound_total", &format!("{host},status=\"2xx\"")), Some(2.0), "{metrics}");
    assert_eq!(metric_value(&metrics, "linkc_http_outbound_total", &format!("{host},status=\"5xx\"")), Some(2.0), "{metrics}");
    let secs_2xx = metric_value(&metrics, "linkc_http_outbound_duration_seconds_sum", &format!("{host},status=\"2xx\"")).expect("suma de duración presente");
    assert!(secs_2xx > 0.0 && secs_2xx < 5.0, "duración real acumulada, en segundos: {secs_2xx}");
    assert!(!metrics.contains("status=\"error\""), "ninguna llamada falló a nivel de red: {metrics}");
    assert!(metrics.contains("# TYPE linkc_http_outbound_total counter"), "{metrics}");
}

#[test]
fn metrics_omits_the_outbound_block_entirely_when_the_program_never_called_out() {
    let temp = TempDir::new("none");
    let src = temp.write("app.link", "service Quiet {\n  rpc ping() -> String { \"pong\" }\n}\n");
    let server = Serve::start(&src);
    assert_eq!(server.request("POST", "/Quiet/ping", "{}").unwrap().0, 200);
    let (_, metrics) = server.request("GET", "/metrics", "").unwrap();
    assert!(!metrics.contains("linkc_http_outbound"), "sin llamadas salientes no hay series inventadas en 0: {metrics}");
}
