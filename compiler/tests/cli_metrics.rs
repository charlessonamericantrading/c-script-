// `GET /metrics` en formato de exposición de Prometheus (GRAMMAR.md
// §3.149, PLAN.md §9.8). Se verifica contra el BINARIO real: que el código
// compile no prueba que la línea impresa tenga la forma exacta que
// Prometheus espera, ni que el conteo de suscriptores de un stream sea el
// real (no uno inventado en memoria aparte).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

const PROGRAM: &str = r#"
type Task = { id: Int, title: String }
db { tasks: Task[] }

service Sys {
  rpc ping() -> String {
    "pong"
  }

  rpc create(title: String) -> Task {
    db.tasks.insert(Task { id: 0, title: title })
  }

  stream watch() -> Task {
    while true {
      db.tasks.subscribe()
    }
  }
}
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "linkc-metrics-{name}-{}-{}",
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

    fn request(&self, method: &str, path: &str, body: &str, extra_headers: &[(&str, &str)]) -> (u16, String) {
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
        let status: u16 = status_line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);

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
        let _ = reader.read_exact(&mut buf);
        (status, String::from_utf8_lossy(&buf).to_string())
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Abre una conexión de `stream` real y la mantiene VIVA (el socket sigue
/// abierto mientras este struct no se dropee) -- para probar
/// `linkc_stream_subscribers` con un suscriptor de verdad conectado, no uno
/// simulado.
struct OpenStream {
    _reader: BufReader<TcpStream>,
}

impl OpenStream {
    fn connect(port: u16, path: &str) -> Self {
        let stream = TcpStream::connect(("127.0.0.1", port)).expect("conectar al stream");
        let body = "{}";
        let request = format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let mut stream = stream;
        stream.write_all(request.as_bytes()).expect("escribir request");
        stream.flush().ok();

        let mut reader = BufReader::new(stream);
        let mut status_line = String::new();
        reader.read_line(&mut status_line).expect("línea de estado del stream");
        assert!(status_line.contains("200"), "el stream no arrancó bien: {status_line}");
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).expect("header del stream");
            if line.trim().is_empty() {
                break;
            }
        }
        OpenStream { _reader: reader }
    }
}

const RATE_LIMITED_PROGRAM: &str = r#"
type Task = { id: Int, title: String }
db { tasks: Task[] }

service Sys {
  @rate_limit("1/1h")
  rpc limited() -> String {
    "ok"
  }
}
"#;

fn build(temp: &TempDir, source: &str) -> std::process::Output {
    let src = temp.write("app.link", source);
    Command::new(env!("CARGO_BIN_EXE_linkc")).arg("build").arg(&src).arg(temp.0.join("gen")).output().expect("ejecutar linkc build")
}

#[test]
fn metrics_reports_request_count_and_duration_per_method() {
    let temp = TempDir::new("counts");
    let out = build(&temp, PROGRAM);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let server = Serve::start(&temp.0.join("app.link"), &[]);

    server.request("POST", "/Sys/ping", "{}", &[]);
    server.request("POST", "/Sys/ping", "{}", &[]);

    let (status, body) = server.request("GET", "/metrics", "", &[]);
    assert_eq!(status, 200, "body: {body}");
    assert!(body.contains("linkc_http_requests_total{method=\"Sys.ping\"} 2"), "body: {body}");
    assert!(body.contains("linkc_http_request_duration_seconds_sum{method=\"Sys.ping\"}"), "body: {body}");
    // Un TYPE/HELP por métrica, formato de exposición real.
    assert!(body.contains("# TYPE linkc_http_requests_total counter"), "body: {body}");
}

#[test]
fn metrics_reports_the_real_database_size_in_bytes() {
    let temp = TempDir::new("db-size");
    let out = build(&temp, PROGRAM);
    assert!(out.status.success());
    let server = Serve::start(&temp.0.join("app.link"), &[]);

    server.request("POST", "/Sys/create", r#"{"title":"algo"}"#, &[]);

    let (_, body) = server.request("GET", "/metrics", "", &[]);
    let line = body.lines().find(|l| l.starts_with("linkc_db_size_bytes ")).unwrap_or_else(|| panic!("body: {body}"));
    let size: i64 = line.trim_start_matches("linkc_db_size_bytes ").trim().parse().expect("tamaño numérico");
    assert!(size > 0, "el tamaño de una base SQLite real con al menos una fila no puede ser 0: {size}");
}

#[test]
fn metrics_reports_the_real_number_of_connected_stream_subscribers() {
    let temp = TempDir::new("stream-subs");
    let out = build(&temp, PROGRAM);
    assert!(out.status.success());
    let server = Serve::start(&temp.0.join("app.link"), &[]);

    // Sin nadie conectado: sin ninguna línea de esta colección (mismo
    // criterio que el resto de las métricas -- nunca inventar un 0 para
    // algo que ni siquiera se sabe que existe).
    let (_, body) = server.request("GET", "/metrics", "", &[]);
    assert!(!body.contains("collection=\"tasks\""), "body: {body}");

    let _watcher_a = OpenStream::connect(server.port, "/Sys/watch");
    let _watcher_b = OpenStream::connect(server.port, "/Sys/watch");

    let (_, body) = server.request("GET", "/metrics", "", &[]);
    assert!(body.contains("linkc_stream_subscribers{collection=\"tasks\"} 2"), "body: {body}");
}

/// GRAMMAR.md §3.39, landmine del barrido de "límites honestos": el
/// `RateLimiter` es por PROCESO -- correr N réplicas detrás de un
/// balanceador diluye el límite real sin ningún aviso. Este contador no
/// arregla la dilución (necesitaría estado compartido entre procesos,
/// fuera de alcance), pero hace el rechazo real VISIBLE en `/metrics`, el
/// mismo lugar que un operador ya mira -- agregable entre réplicas en
/// Prometheus para notar cuándo el 429 real está pasando de verdad.
#[test]
fn metrics_reports_real_rate_limit_rejections_per_rpc() {
    let temp = TempDir::new("rate-limit-rejections");
    let out = build(&temp, RATE_LIMITED_PROGRAM);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let server = Serve::start(&temp.0.join("app.link"), &[]);

    // Sin ningún rechazo todavía: la métrica no aparece (mismo criterio
    // "nunca inventar un 0" que el resto de las métricas condicionales).
    let (_, body) = server.request("GET", "/metrics", "", &[]);
    assert!(!body.contains("linkc_rate_limit_rejections_total"), "body: {body}");

    // Primera pasa (cupo de 1/1h), la segunda y tercera se rechazan de
    // verdad con 429 -- no un valor inventado.
    let (status, _) = server.request("POST", "/Sys/limited", "{}", &[]);
    assert_eq!(status, 200);
    let (status, _) = server.request("POST", "/Sys/limited", "{}", &[]);
    assert_eq!(status, 429);
    let (status, _) = server.request("POST", "/Sys/limited", "{}", &[]);
    assert_eq!(status, 429);

    let (_, body) = server.request("GET", "/metrics", "", &[]);
    assert!(body.contains("linkc_rate_limit_rejections_total{method=\"Sys.limited\"} 2"), "body: {body}");
}

#[test]
fn metrics_is_not_exempt_from_the_service_api_key_unlike_health() {
    let temp = TempDir::new("api-key");
    let out = build(&temp, PROGRAM);
    assert!(out.status.success());
    let server = Serve::start(&temp.0.join("app.link"), &["--service-api-key", "secreto"]);

    // /health SIGUE exento (comportamiento de siempre).
    let (status, _) = server.request("GET", "/health", "", &[]);
    assert_eq!(status, 200);

    // /metrics, sin la clave, rechazado -- es más sensible que /health.
    let (status, body) = server.request("GET", "/metrics", "", &[]);
    assert_eq!(status, 401, "body: {body}");

    // Con la clave, sirve normal.
    let (status, body) = server.request("GET", "/metrics", "", &[("X-Service-Api-Key", "secreto")]);
    assert_eq!(status, 200, "body: {body}");
    assert!(body.contains("linkc_http_requests_total"), "body: {body}");
}
