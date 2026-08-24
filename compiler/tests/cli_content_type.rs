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

    /// Como `get`, pero mandando un body JSON -- para un rpc que toma
    /// parámetros (`escapeHtml` necesita un valor real de la request, no
    /// uno hardcodeado en el `.link`, para probar algo real).
    fn post(&self, path: &str, body: &str) -> (u16, String, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("conectar");
        let request = format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            self.port,
            body.len()
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

    /// Como `post`, pero devuelve el header `Location` en vez del body --
    /// para `response.redirect` (GRAMMAR.md §3.111), donde lo que importa
    /// es el status (301/302) y a dónde apunta, no un body real. `POST`
    /// (no `GET`) porque estos rpcs se llaman por su dirección normal
    /// `/Servicio/rpc`, no un `@route` -- mismo protocolo que `post` de
    /// arriba usa para el resto de este archivo.
    fn post_redirect(&self, path: &str, body: &str) -> (u16, Option<String>) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("conectar");
        let request = format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            self.port,
            body.len()
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

        let mut location = None;
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            let n = reader.read_line(&mut line).expect("header");
            if n == 0 || line.trim().is_empty() {
                break;
            }
            if let Some((k, v)) = line.trim().split_once(':') {
                match k.trim().to_ascii_lowercase().as_str() {
                    "location" => location = Some(v.trim().to_string()),
                    "content-length" => content_length = v.trim().parse().unwrap_or(0),
                    _ => {}
                }
            }
        }
        let mut buf = vec![0u8; content_length];
        reader.read_exact(&mut buf).ok();
        (status, location)
    }

    /// Como `get`, pero devuelve el header `Cache-Control` en vez del body --
    /// para el caso combinado con `@route` (`@cache_control` sobre un rpc
    /// que también se sirve por GET, GRAMMAR.md §3.113).
    fn get_cache_control(&self, path: &str) -> Option<String> {
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

        let mut cache_control = None;
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            let n = reader.read_line(&mut line).expect("header");
            if n == 0 || line.trim().is_empty() {
                break;
            }
            if let Some((k, v)) = line.trim().split_once(':') {
                match k.trim().to_ascii_lowercase().as_str() {
                    "cache-control" => cache_control = Some(v.trim().to_string()),
                    "content-length" => content_length = v.trim().parse().unwrap_or(0),
                    _ => {}
                }
            }
        }
        let mut buf = vec![0u8; content_length];
        reader.read_exact(&mut buf).ok();
        cache_control
    }

    /// Como `post_redirect`, pero devuelve el header `Cache-Control` --
    /// para `@cache_control("...")` (GRAMMAR.md §3.113).
    fn post_cache_control(&self, path: &str, body: &str) -> (u16, Option<String>) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("conectar");
        let request = format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            self.port,
            body.len()
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

        let mut cache_control = None;
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            let n = reader.read_line(&mut line).expect("header");
            if n == 0 || line.trim().is_empty() {
                break;
            }
            if let Some((k, v)) = line.trim().split_once(':') {
                match k.trim().to_ascii_lowercase().as_str() {
                    "cache-control" => cache_control = Some(v.trim().to_string()),
                    "content-length" => content_length = v.trim().parse().unwrap_or(0),
                    _ => {}
                }
            }
        }
        let mut buf = vec![0u8; content_length];
        reader.read_exact(&mut buf).ok();
        (status, cache_control)
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

#[test]
fn escape_html_neutralizes_a_real_payload_interpolated_into_an_html_page() {
    // GRAMMAR.md §3.45: `.escapeHtml()` es la herramienta que faltaba para
    // armar una página `@content_type("text/html")` con datos que no
    // controla el propio programa (un nombre, un comentario) sin quedar
    // abierta a inyectar HTML/JS ajeno. Se prueba con un payload de XSS de
    // libro, contra el servidor real -- no alcanza con que el método
    // exista, tiene que llegar escapado en la respuesta HTTP de verdad.
    let temp = TempDir::new("escape-html");
    let out = build(
        &temp,
        r#"
service Blog {
  @content_type("text/html; charset=utf-8")
  rpc page(name: String) -> String {
    "<h1>Hola " + name.escapeHtml() + "</h1>"
  }
}
"#,
    );
    assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));

    let server = Serve::start(&temp.0.join("app.link"));
    let payload = r#"<img src=x onerror=alert(1)>"#;
    let (status, content_type, body) = server.post("/Blog/page", &serde_json::json!({"name": payload}).to_string());
    assert_eq!(status, 200);
    assert_eq!(content_type, "text/html; charset=utf-8");
    assert_eq!(body, "<h1>Hola &lt;img src=x onerror=alert(1)&gt;</h1>");
    assert!(!body.contains("<img"), "el payload sin escapar no puede sobrevivir tal cual en la respuesta: {body}");
}

#[test]
fn response_set_status_renders_a_branded_404_page_instead_of_the_json_error_path() {
    // GRAMMAR.md §3.46: la brecha original -- un rpc `@route`+`@content_type`
    // solo podía devolver 200 en el camino de éxito; cualquier "no
    // encontrado" tenía que fallar (panic/Err), y un error SIEMPRE sale como
    // JSON (server.rs), rompiendo justo la página HTML que se quería mostrar.
    // Prueba contra el servidor real: el status HTTP de la respuesta tiene
    // que ser 404 de verdad, con el HTML tal cual lo armó el rpc -- no el
    // `{"error": ...}` de siempre.
    let temp = TempDir::new("response-set-status-404");
    let out = build(
        &temp,
        r#"
type User = { id: Int, name: String }
db { users: User[] }

service Web {
  @route("/users/:id")
  @content_type("text/html")
  rpc userPage(id: Int) -> String {
    let found = db.users.find(id);
    if found == null {
      response.setStatus(404);
      "<h1>404: usuario no encontrado</h1>"
    } else {
      "<h1>encontrado</h1>"
    }
  }
}
"#,
    );
    assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));

    let server = Serve::start(&temp.0.join("app.link"));
    let (status, content_type, body) = server.get("/users/999");
    assert_eq!(status, 404);
    assert_eq!(content_type, "text/html");
    assert_eq!(body, "<h1>404: usuario no encontrado</h1>");
}

#[test]
fn response_set_status_also_works_on_a_plain_json_rpc_for_a_2xx_other_than_200() {
    // No está atado a `@content_type`/HTML -- cualquier rpc puede pedir un
    // status de éxito distinto de 200 (ej. 201 Created para un `create`).
    let temp = TempDir::new("response-set-status-json");
    let out = build(
        &temp,
        r#"
service Web {
  rpc create(name: String) -> String {
    response.setStatus(201);
    "creado: " + name
  }
}
"#,
    );
    assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));

    let server = Serve::start(&temp.0.join("app.link"));
    let (status, content_type, body) = server.post("/Web/create", &serde_json::json!({"name": "Ada"}).to_string());
    assert_eq!(status, 201);
    assert_eq!(content_type, "application/json; charset=utf-8");
    assert_eq!(body, "\"creado: Ada\"");
}

#[test]
fn response_set_status_rejects_a_code_outside_the_valid_http_range() {
    // Validado en RUNTIME (el argumento puede ser cualquier expresión, no
    // solo un literal, así que no hay forma de chequearlo en compilación) --
    // pero SIGUE siendo un error claro, no un status HTTP inválido escrito
    // tal cual al socket.
    let temp = TempDir::new("response-set-status-invalid");
    let out = build(
        &temp,
        r#"
service Web {
  rpc bad() -> String {
    response.setStatus(50);
    "no deberia llegar"
  }
}
"#,
    );
    assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));

    let server = Serve::start(&temp.0.join("app.link"));
    let (status, content_type, body) = server.post("/Web/bad", "{}");
    assert_eq!(status, 500);
    assert_eq!(content_type, "application/json; charset=utf-8");
    assert!(body.contains("un status HTTP válido está entre 100 y 599"), "body inesperado: {body}");
}

#[test]
fn response_redirect_sets_the_real_status_and_location_header() {
    // GRAMMAR.md §3.111: `response.redirect(url, permanent)` -- `permanent:
    // false` tiene que dar 302 de verdad (no solo un body que lo mencione),
    // `permanent: true` tiene que dar 301, y los dos tienen que llevar el
    // header `Location` REAL con la URL exacta que el rpc pidió.
    let temp = TempDir::new("response-redirect");
    let out = build(
        &temp,
        r#"
service Web {
  rpc temporary() -> Void { response.redirect("/nueva-ubicacion", false) }
  rpc permanent() -> Void { response.redirect("https://example.com/nuevo", true) }
}
"#,
    );
    assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));

    let server = Serve::start(&temp.0.join("app.link"));

    let (status, location) = server.post_redirect("/Web/temporary", "{}");
    assert_eq!(status, 302);
    assert_eq!(location.as_deref(), Some("/nueva-ubicacion"));

    let (status, location) = server.post_redirect("/Web/permanent", "{}");
    assert_eq!(status, 301);
    assert_eq!(location.as_deref(), Some("https://example.com/nuevo"));
}

#[test]
fn cache_control_annotation_sets_the_real_header_only_on_success() {
    // GRAMMAR.md §3.113: `@cache_control("...")` tiene que aparecer como
    // header HTTP real (no solo un valor que el checker haya aceptado), y
    // TIENE que faltar en una respuesta de error -- una falla nunca debe
    // quedar cacheada con la política pensada para el camino de éxito.
    let temp = TempDir::new("cache-control");
    let out = build(
        &temp,
        r#"
service Web {
  @cache_control("public, max-age=3600")
  rpc cached() -> String { "ok" }

  @cache_control("public, max-age=60")
  rpc alwaysFails() -> Void { panic("siempre falla") }

  rpc uncached() -> String { "ok" }
}
"#,
    );
    assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));

    let server = Serve::start(&temp.0.join("app.link"));

    let (status, cache_control) = server.post_cache_control("/Web/cached", "{}");
    assert_eq!(status, 200);
    assert_eq!(cache_control.as_deref(), Some("public, max-age=3600"));

    // Sin la anotación: sin header, como siempre.
    let (status, cache_control) = server.post_cache_control("/Web/uncached", "{}");
    assert_eq!(status, 200);
    assert_eq!(cache_control, None);

    // CON la anotación, pero el rpc falla: el header NO debe aparecer.
    let (status, cache_control) = server.post_cache_control("/Web/alwaysFails", "{}");
    assert_eq!(status, 500);
    assert_eq!(cache_control, None, "una respuesta de error no debe llevar el Cache-Control del camino de éxito");
}

#[test]
fn cache_control_combines_with_route_and_content_type_for_a_real_sitemap() {
    // El caso real que motiva esto: un sitemap.xml servido con `@route` +
    // `@content_type`, más `@cache_control` para que un CDN/crawler no lo
    // vuelva a pedir en cada visita -- las tres anotaciones combinadas,
    // GET real (no `/Servicio/rpc`).
    let temp = TempDir::new("cache-control-route");
    let out = build(
        &temp,
        r#"
service Site {
  @route("/sitemap.xml")
  @content_type("application/xml")
  @cache_control("public, max-age=86400")
  rpc sitemap() -> String { "<urlset></urlset>" }
}
"#,
    );
    assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));

    let server = Serve::start(&temp.0.join("app.link"));
    let (status, content_type, body) = server.get("/sitemap.xml");
    assert_eq!(status, 200);
    assert_eq!(content_type, "application/xml");
    assert_eq!(body, "<urlset></urlset>");
    assert_eq!(server.get_cache_control("/sitemap.xml").as_deref(), Some("public, max-age=86400"));
}
