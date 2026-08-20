// `env.get(name) -> String?` y `request.rawBody()`/`request.header(name)`
// (GRAMMAR.md §3.38): lo mínimo que un rpc necesita para leer un secreto de
// configuración y para verificar la firma de un webhook entrante (HMAC
// sobre el body CRUDO, no sobre lo que el parser de JSON reconstruya --
// cualquier reserialización puede no ser byte-a-byte igual al original que
// firmó el remitente).
//
// `env.get` se prueba contra el PROCESO real (`Command::env`), no con
// `std::env::set_var` en este mismo test binario: los tests de un mismo
// binario corren en threads paralelos, y una variable de entorno es estado
// del PROCESO entero -- setearla acá pisaría (o la pisarían a ella) tests
// que corren al mismo tiempo. `request.rawBody`/`header` se prueban contra
// el servidor HTTP real por el mismo motivo de siempre: que el checker
// tipe la anotación no prueba que el servidor la sirva.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

const PROGRAM: &str = r#"
service Sys {
  rpc readSecret() -> String? {
    env.get("LINKC_TEST_SECRET")
  }

  rpc echoBody() -> String {
    request.rawBody()
  }

  rpc echoHeader() -> String? {
    request.header("X-Custom-Header")
  }
}
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "linkc-env-request-{name}-{}-{}",
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
    /// `extra_env`: variables adicionales para el PROCESO hijo -- así cada
    /// test controla exactamente qué ve `env.get` sin tocar el entorno de
    /// este proceso de test (que corre en paralelo con otros).
    fn start(link_path: &PathBuf, extra_env: &[(&str, &str)]) -> Self {
        let port = free_port();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_linkc"));
        cmd.arg("serve").arg(link_path).arg(port.to_string()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        let child = cmd.spawn().expect("iniciar 'linkc serve'");
        wait_for_port(port);
        Serve { child, port }
    }

    /// POST /{service}/{method} con headers extra y devuelve (status,
    /// content-type, body crudo sin parsear como JSON) -- `echoBody`
    /// necesita ver el body EXACTO que se mandó.
    fn post_raw(&self, path: &str, body: &str, extra_headers: &[(&str, &str)]) -> (u16, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("conectar");
        let mut request = format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
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

fn build(temp: &TempDir, source: &str) -> std::process::Output {
    let src = temp.write("app.link", source);
    Command::new(env!("CARGO_BIN_EXE_linkc")).arg("build").arg(&src).arg(temp.0.join("gen")).output().expect("ejecutar linkc build")
}

#[test]
fn env_get_reads_a_real_process_variable_and_is_null_when_absent() {
    let temp = TempDir::new("env");
    let out = build(&temp, PROGRAM);
    assert!(out.status.success(), "el programa debió compilar: {}", String::from_utf8_lossy(&out.stderr));

    let server = Serve::start(&temp.0.join("app.link"), &[("LINKC_TEST_SECRET", "s3cr3t-value")]);
    let (status, body) = server.post_raw("/Sys/readSecret", "{}", &[]);
    assert_eq!(status, 200);
    assert_eq!(body, "\"s3cr3t-value\"", "env.get debe devolver el valor real del proceso: {body}");

    drop(server);

    // Un segundo proceso, SIN esa variable seteada: `env.get` debe dar
    // `null` (Optional vacío), no un error ni un string vacío.
    let server = Serve::start(&temp.0.join("app.link"), &[]);
    let (status, body) = server.post_raw("/Sys/readSecret", "{}", &[]);
    assert_eq!(status, 200);
    assert_eq!(body, "null", "sin la variable seteada, env.get debe dar null: {body}");
}

#[test]
fn request_raw_body_and_header_expose_the_real_incoming_request() {
    let temp = TempDir::new("request");
    let out = build(&temp, PROGRAM);
    assert!(out.status.success());

    let server = Serve::start(&temp.0.join("app.link"), &[]);

    // Body con forma de webhook real: JSON válido (así pasa el parseo de
    // argumentos, que corre antes de que el rpc vea nada -- limitación v0
    // conocida, GRAMMAR.md §3.38) pero con más campos de los que este rpc
    // usa, como manda cualquier proveedor real (Stripe, GitHub, etc.).
    let webhook_body = r#"{"event":"payment.succeeded","amount_cents":4599,"currency":"usd"}"#;
    let (status, body) = server.post_raw("/Sys/echoBody", webhook_body, &[]);
    assert_eq!(status, 200);
    // El rpc devuelve un String -- sale envuelto en comillas JSON. Lo que
    // importa es que ADENTRO esté el body EXACTO, no una reserialización.
    let decoded: String = serde_json::from_str(&body).expect("respuesta JSON válida");
    assert_eq!(decoded, webhook_body, "rawBody debe ser el body tal cual llegó, byte a byte");

    let (status, body) = server.post_raw("/Sys/echoHeader", "{}", &[("X-Custom-Header", "hola-header")]);
    assert_eq!(status, 200);
    assert_eq!(body, "\"hola-header\"");

    // Sin ese header: `None`, no un error.
    let (status, body) = server.post_raw("/Sys/echoHeader", "{}", &[]);
    assert_eq!(status, 200);
    assert_eq!(body, "null");

    // La lectura de headers no distingue mayúsculas/minúsculas, como manda
    // el estándar HTTP -- cualquier proveedor de webhooks puede mandar el
    // nombre del header con la capitalización que quiera.
    let (status, body) = server.post_raw("/Sys/echoHeader", "{}", &[("x-custom-header", "otra-capitalizacion")]);
    assert_eq!(status, 200);
    assert_eq!(body, "\"otra-capitalizacion\"");
}

#[test]
fn each_request_only_sees_its_own_body_and_headers() {
    // El contexto vive en un RefCell sobre `Db`, sobreescrito al principio
    // de CADA request (runtime/server.rs) -- esto prueba que dos requests
    // consecutivas, secuenciales sobre la misma conexión de proceso, no se
    // mezclan (lo que confirmaría un bug de estado compartido mal limpiado).
    let temp = TempDir::new("isolation");
    let out = build(&temp, PROGRAM);
    assert!(out.status.success());

    let server = Serve::start(&temp.0.join("app.link"), &[]);

    let (_, body_a) = server.post_raw("/Sys/echoBody", r#"{"who":"a"}"#, &[]);
    let (_, body_b) = server.post_raw("/Sys/echoBody", r#"{"who":"b"}"#, &[]);
    assert_eq!(body_a, "\"{\\\"who\\\":\\\"a\\\"}\"");
    assert_eq!(body_b, "\"{\\\"who\\\":\\\"b\\\"}\"");
    assert_ne!(body_a, body_b, "cada request debe ver SU PROPIO body, no el de la anterior");
}
