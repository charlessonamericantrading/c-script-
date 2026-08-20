// `@route("/blog/:slug")` (GRAMMAR.md §3.37): un rpc puede declarar una URL
// alternativa, amigable para un crawler, ADEMÁS de su dirección normal
// `/Service/rpc` -- ninguna reemplaza a la otra.
//
// Motivación: `@content_type` (GRAMMAR.md §3.35) ya permite devolver HTML
// desde un rpc, pero el ruteo siguió siendo siempre `/Service/rpc` -- para
// contenido pensado para SEO (un blog, una ficha de producto) eso significa
// una URL fea, sin nada que un crawler pueda indexar de forma legible. Este
// archivo prueba, contra el binario real hablando HTTP de verdad, que la URL
// linda funciona, que la de siempre SIGUE funcionando, y sobre todo que la
// PRECEDENCIA entre una ruta literal y una dinámica es la correcta -- ese fue
// justamente el primer bug real que este mismo archivo encontró durante el
// desarrollo: `/blog/featured` (literal) resolvía al rpc de `/blog/:slug`
// (dinámico) por orden de declaración, no al literal.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

const PROGRAM: &str = r#"
enum Role { Admin }

service Blog {
  @content_type("text/html; charset=utf-8")
  @route("/blog/:slug")
  rpc page(slug: String) -> String {
    "<h1>" + slug + "</h1>"
  }

  @route("/blog/featured")
  @content_type("text/html; charset=utf-8")
  rpc featured() -> String {
    "<h1>ESTA-ES-LA-LITERAL</h1>"
  }

  @content_type("application/xml")
  @route("/sitemap.xml")
  rpc sitemap() -> String {
    "<urlset></urlset>"
  }

  @route("/producto/:id")
  rpc product(id: Int) -> String {
    "producto"
  }

  @requires(Role.Admin)
  @content_type("text/html; charset=utf-8")
  @route("/admin/panel")
  rpc panel() -> String {
    "<h1>Panel</h1>"
  }
}
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "linkc-route-{name}-{}-{}",
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

/// Mismo criterio que tests/server_http.rs y tests/cli_content_type.rs: un
/// round-trip HTTP completo es la única señal confiable de que el servidor
/// ya está sirviendo.
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

    /// GET crudo -- (status, content-type, body) sin parsear. A propósito
    /// GET y no POST: el caso de uso de `@route` es un crawler, que nunca
    /// manda un body.
    fn get(&self, path: &str) -> (u16, String, String) {
        self.request("GET", path, "", None)
    }

    fn post(&self, path: &str, body: &str) -> (u16, String, String) {
        self.request("POST", path, body, None)
    }

    fn get_with_token(&self, path: &str, token: &str) -> (u16, String, String) {
        self.request("GET", path, "", Some(token))
    }

    fn request(&self, method: &str, path: &str, body: &str, token: Option<&str>) -> (u16, String, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("conectar");
        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Length: {}\r\nConnection: close\r\n",
            self.port,
            body.len()
        );
        if let Some(t) = token {
            request.push_str(&format!("Authorization: Bearer {t}\r\n"));
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
    Command::new(env!("CARGO_BIN_EXE_linkc")).arg("build").arg(&src).arg(temp.0.join("gen")).output().expect("ejecutar linkc build")
}

#[test]
fn a_string_param_route_serves_html_over_a_plain_get() {
    let temp = TempDir::new("string-param");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src);

    let (status, content_type, body) = server.get("/blog/hola-mundo");
    assert_eq!(status, 200);
    assert_eq!(content_type, "text/html; charset=utf-8");
    assert_eq!(body, "<h1>hola-mundo</h1>");
}

#[test]
fn a_literal_route_wins_over_a_param_route_with_the_same_prefix() {
    // El bug real que este archivo encontró la primera vez: sin precedencia
    // explícita, `/blog/featured` resolvía al rpc de `/blog/:slug` (el
    // primero declarado en el .link), tratando "featured" como si fuera un
    // slug cualquiera -- nunca llegaba a ejecutar el rpc de la ruta literal.
    let temp = TempDir::new("literal-precedence");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src);

    let (status, _, body) = server.get("/blog/featured");
    assert_eq!(status, 200);
    assert_eq!(body, "<h1>ESTA-ES-LA-LITERAL</h1>", "la ruta literal tiene que ganar, no el rpc de :slug");

    // Y un slug real (no "featured") sigue yendo al rpc dinámico.
    let (status, _, body) = server.get("/blog/otra-cosa");
    assert_eq!(status, 200);
    assert_eq!(body, "<h1>otra-cosa</h1>");
}

#[test]
fn a_purely_literal_route_needs_no_params() {
    let temp = TempDir::new("literal-no-params");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src);

    let (status, content_type, body) = server.get("/sitemap.xml");
    assert_eq!(status, 200);
    assert_eq!(content_type, "application/xml");
    assert_eq!(body, "<urlset></urlset>");
}

#[test]
fn an_int_param_route_parses_the_segment_and_rejects_a_bad_one_cleanly() {
    let temp = TempDir::new("int-param");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src);

    // `product` no declara `@content_type`, así que sigue respondiendo JSON
    // normal (con comillas) -- una `@route` no cambia esa parte.
    let (status, _, body) = server.get("/producto/42");
    assert_eq!(status, 200);
    assert_eq!(body, "\"producto\"");

    // Un segmento que no parsea como entero es un 400 con un mensaje claro,
    // nunca un 500 ni -- mucho menos -- un panic que tire el servidor.
    let (status, content_type, body) = server.get("/producto/no-es-un-numero");
    assert_eq!(status, 400);
    assert_eq!(content_type, "application/json; charset=utf-8", "un error siempre es JSON, aunque la ruta sea de una página HTML");
    assert!(body.contains("':id'"), "el error tiene que nombrar el parámetro: {body}");
    assert!(body.contains("entero"), "el error tiene que decir qué tipo esperaba: {body}");
}

#[test]
fn the_normal_service_rpc_address_still_works_alongside_the_route() {
    // `@route` es un alias, NUNCA un reemplazo: el cliente TypeScript
    // generado sigue llamando a /Service/rpc con un body JSON, y eso tiene
    // que seguir funcionando exactamente igual para un rpc que además
    // declaró una ruta linda.
    let temp = TempDir::new("alias-not-replace");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src);

    let (status, content_type, body) = server.post("/Blog/page", r#"{"slug":"via-rpc-normal"}"#);
    assert_eq!(status, 200);
    assert_eq!(content_type, "text/html; charset=utf-8");
    assert_eq!(body, "<h1>via-rpc-normal</h1>");
}

#[test]
fn a_route_can_stack_with_auth_and_errors_stay_json_even_for_an_html_route() {
    let temp = TempDir::new("auth-route");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src);

    // Sin token: 401, y en JSON -- no una página de error en HTML, aunque
    // el rpc protegido declare @content_type("text/html").
    let (status, content_type, body) = server.get("/admin/panel");
    assert_eq!(status, 401);
    assert_eq!(content_type, "application/json; charset=utf-8");
    assert!(body.contains("error"), "el body de error debe ser JSON: {body}");

    // Con un token de un rol equivocado: 403, mismo criterio.
    let (status, _, _) = server.get_with_token("/admin/panel", "un-token-cualquiera-invalido");
    assert_eq!(status, 401, "un token que no corresponde a ninguna sesión real sigue siendo 401, no 403");
}

#[test]
fn an_unmatched_path_falls_back_to_the_normal_404() {
    let temp = TempDir::new("no-match");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src);

    let (status, _, _) = server.get("/esto/no/existe/en/ningun/lado");
    assert_eq!(status, 404);
}

#[test]
fn the_checker_rejects_what_cannot_work() {
    let temp = TempDir::new("checker-rejects");

    // Dos rutas con la misma forma (misma para nombrar-agnóstica) son
    // indistinguibles al despachar -- rechazado en compilación.
    let out = build(
        &temp,
        r#"
service A { @route("/blog/:slug") rpc p(slug: String) -> String { slug } }
service B { @route("/blog/:id") rpc q(id: String) -> String { id } }
"#,
    );
    let stderr = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(!out.status.success(), "dos rutas con la misma forma debieron rechazarse");
    assert!(stderr.contains("misma forma"), "mensaje inesperado: {stderr}");

    // Un parámetro que no es el último segmento -- v0 no lo soporta.
    let out = build(&temp, r#"service A { @route("/:cat/:slug") rpc p(cat: String, slug: String) -> String { slug } }"#);
    let stderr = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(!out.status.success());
    assert!(stderr.contains("ÚLTIMO"), "mensaje inesperado: {stderr}");

    // Un tipo que no puede venir de un segmento de URL.
    let out = build(&temp, r#"service A { @route("/x/:on") rpc p(on: Bool) -> String { "x" } }"#);
    let stderr = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(!out.status.success());
    assert!(stderr.contains("String") && stderr.contains("Int"), "mensaje inesperado: {stderr}");

    // @route sobre un stream.
    let out = build(
        &temp,
        r#"
type X = { id: Int }
db { xs: X[], }
service A { @route("/feed") stream watch() -> X { while true { db.xs.subscribe() } } }
"#,
    );
    let stderr = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(!out.status.success());
    assert!(stderr.contains("stream"), "mensaje inesperado: {stderr}");
}
