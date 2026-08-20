// `@content_type("...")` (GRAMMAR.md §3.35): un rpc que devuelve `String`
// puede declarar el Content-Type de su respuesta, y entonces el cuerpo se
// escribe TAL CUAL -- sin las comillas de JSON alrededor.
//
// Es lo que hace posible servir HTML, un sitemap XML o un CSV desde un
// programa c-script. Antes de esto el Content-Type estaba literal en el
// binario (`application/json` para rpcs, `text/event-stream` para streams) y
// no existía forma de devolver una página, así que un proyecto con páginas
// públicas no podía usar el lenguaje para servirlas.
//
// Este archivo lo prueba donde importa: contra el BINARIO real, hablando HTTP
// de verdad. Que el checker acepte la anotación no prueba que el servidor
// mande el header, y que el servidor lo mande no prueba que el cliente
// generado sepa leerlo (la primera versión llamaba `res.json()` sobre el HTML
// y reventaba en el primer `<`).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

const PROGRAM: &str = r#"
type Article = { id: Int, slug: String, title: String }

db { articles: Article[], }

service Site {
  @content_type("text/html; charset=utf-8")
  rpc home() -> String {
    "<!doctype html><h1>Hola</h1>"
  }

  @content_type("application/xml")
  rpc sitemap() -> String {
    "<urlset></urlset>"
  }

  rpc list() -> Article[] {
    db.articles.all()
  }
}
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "linkc-content-type-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
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
    TcpListener::bind(("127.0.0.1", 0))
        .expect("bindear puerto efímero")
        .local_addr()
        .unwrap()
        .port()
}

/// Mismo criterio que tests/server_http.rs: un round-trip HTTP completo es la
/// única señal confiable de que el servidor ya está sirviendo (el backlog del
/// socket acepta la conexión antes de que el proceso llame `accept()`).
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
            .expect("iniciar 'linkc serve'");
        wait_for_port(port);
        Serve { child, port }
    }

    /// Devuelve (status, content-type, body crudo) -- el body SIN parsear,
    /// porque justamente lo que se está probando es que no sea JSON.
    fn get(&self, path: &str) -> (u16, String, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("conectar");
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
            self.port
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

        let mut content_type = String::new();
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            let n = reader.read_line(&mut line).expect("header");
            if n == 0 || line.trim().is_empty() {
                break;
            }
            if let Some((k, v)) = line.trim().split_once(':') {
                match k.trim().to_ascii_lowercase().as_str() {
                    "content-type" => content_type = v.trim().to_string(),
                    "content-length" => content_length = v.trim().parse().unwrap_or(0),
                    _ => {}
                }
            }
        }
        let mut buf = vec![0u8; content_length];
        reader.read_exact(&mut buf).expect("body");
        (status, content_type, String::from_utf8_lossy(&buf).to_string())
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
    Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("build")
        .arg(&src)
        .arg(temp.0.join("gen"))
        .output()
        .expect("ejecutar linkc build")
}

#[test]
fn declared_content_type_is_served_verbatim_and_json_rpcs_are_untouched() {
    let temp = TempDir::new("serve");
    let out = build(&temp, PROGRAM);
    assert!(out.status.success(), "el programa debió compilar: {}", String::from_utf8_lossy(&out.stderr));

    let server = Serve::start(&temp.0.join("app.link"));

    let (status, content_type, body) = server.get("/Site/home");
    assert_eq!(status, 200);
    assert_eq!(content_type, "text/html; charset=utf-8");
    // Lo importante: el body es el String TAL CUAL. Como JSON habría llegado
    // envuelto en comillas y con los `<` escapados donde corresponda.
    assert_eq!(body, "<!doctype html><h1>Hola</h1>");
    assert!(!body.starts_with('"'), "el HTML no debe salir envuelto en comillas de JSON");

    let (status, content_type, body) = server.get("/Site/sitemap");
    assert_eq!(status, 200);
    assert_eq!(content_type, "application/xml");
    assert_eq!(body, "<urlset></urlset>");

    // Un rpc sin la anotación sigue siendo JSON, byte por byte igual que antes.
    let (status, content_type, body) = server.get("/Site/list");
    assert_eq!(status, 200);
    assert_eq!(content_type, "application/json; charset=utf-8");
    assert_eq!(body, "[]");
}

#[test]
fn the_generated_client_reads_text_not_json_for_those_rpcs() {
    let temp = TempDir::new("client");
    let out = build(&temp, PROGRAM);
    assert!(out.status.success());

    let client = std::fs::read_to_string(temp.0.join("gen").join("client.ts")).expect("client.ts");
    let home = client
        .split("async home(")
        .nth(1)
        .expect("el cliente debe tener el método home")
        .split("async ")
        .next()
        .unwrap();
    assert!(home.contains("res.text()"), "home() debe leer texto:\n{home}");
    assert!(!home.contains("res.json()"), "home() no debe parsear JSON:\n{home}");

    // Y el rpc normal sigue validando el JSON como siempre.
    let list = client.split("async list(").nth(1).expect("método list").split("async ").next().unwrap();
    assert!(list.contains("res.json()"), "list() debe seguir parseando JSON:\n{list}");

    // El OpenAPI tiene que declarar lo mismo que manda el servidor: si dijera
    // application/json, cualquier cliente generado desde el spec parsearía mal.
    let spec = std::fs::read_to_string(temp.0.join("gen").join("openapi.json")).expect("openapi.json");
    assert!(spec.contains("text/html; charset=utf-8"), "el spec debe declarar el Content-Type real");
}

#[test]
fn the_checker_rejects_the_combinations_that_cannot_work() {
    let temp = TempDir::new("rejects");

    // 1. Sobre un rpc que no devuelve String: el cuerpo se escribe tal cual,
    //    y una lista de structs no es texto.
    let out = build(
        &temp,
        r#"
type A = { id: Int }
db { items: A[], }
service S {
  @content_type("text/html")
  rpc bad() -> A[] { db.items.all() }
}
"#,
    );
    let stderr = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(!out.status.success(), "debió fallar");
    assert!(stderr.contains("tiene que devolver `String`"), "mensaje inesperado: {stderr}");

    // 2. Sobre un stream: SSE tiene su propio Content-Type por protocolo.
    let out = build(
        &temp,
        r#"
type A = { id: Int }
db { items: A[], }
service S {
  @content_type("text/html")
  stream bad() -> A { while true { db.items.subscribe() } }
}
"#,
    );
    let stderr = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(!out.status.success(), "debió fallar");
    assert!(stderr.contains("Server-Sent Events"), "mensaje inesperado: {stderr}");

    // 3. Dos veces: una respuesta tiene un solo Content-Type.
    let out = build(
        &temp,
        r#"
service S {
  @content_type("text/html")
  @content_type("application/xml")
  rpc bad() -> String { "x" }
}
"#,
    );
    let stderr = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(!out.status.success(), "debió fallar");
    assert!(stderr.contains("más de una vez"), "mensaje inesperado: {stderr}");
}

#[test]
fn an_html_page_can_be_behind_auth() {
    // El motivo de haber pasado las anotaciones de una sola a una lista: un
    // panel de administración es HTML *y* está protegido. Con el modelo
    // anterior ("a lo sumo UNA anotación") esto era inexpresable.
    let temp = TempDir::new("auth");
    let out = build(
        &temp,
        r#"
enum Role { Admin, Member }

service Panel {
  @requires(Role.Admin)
  @content_type("text/html; charset=utf-8")
  rpc dashboard() -> String {
    "<h1>Panel</h1>"
  }
}
"#,
    );
    assert!(
        out.status.success(),
        "un rpc puede declarar auth y Content-Type a la vez: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let server = Serve::start(&temp.0.join("app.link"));
    // Sin token, la respuesta es el 401 en JSON de siempre -- NO una página
    // de error en HTML: el cliente generado espera {"error": ...} para
    // cualquier status >= 400.
    let (status, content_type, body) = server.get("/Panel/dashboard");
    assert_eq!(status, 401);
    assert_eq!(content_type, "application/json; charset=utf-8");
    assert!(body.contains("error"), "el body de error debe ser JSON: {body}");
}
