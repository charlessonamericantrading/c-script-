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

// GRAMMAR.md §3.42: `@route` con MÁS de un parámetro, en cualquier
// posición -- v0 (§3.37) solo permitía uno, y tenía que ser el último.

const MULTI_PARAM_PROGRAM: &str = r#"
service Blog {
  @route("/blog/:categoria/:slug")
  rpc page(slug: String, categoria: String) -> String {
    categoria + "/" + slug
  }

  @route("/blog/featured/:slug")
  rpc featuredInCategory(slug: String) -> String {
    "FEATURED/" + slug
  }
}
"#;

// GRAMMAR.md §3.42 (ronda catch-all): `:nombre*` como ÚLTIMO segmento
// captura cero o más segmentos restantes, unidos con "/".

const CATCHALL_PROGRAM: &str = r#"
service Docs {
  @route("/docs/:rest*")
  rpc page(rest: String) -> String {
    rest
  }

  @route("/docs/changelog")
  rpc changelog() -> String {
    "EL-CHANGELOG"
  }
}
"#;

// GRAMMAR.md §3.62: cualquier parámetro del rpc que NO esté en el path se
// lee de la query string por nombre -- `String`/`Int` obligatorio, o
// `String?`/`Int?` si puede estar ausente sin que eso sea un error.

const QUERY_PROGRAM: &str = r#"
type SearchResult = { q: String, page: Int? }
type PostResult = { slug: String, src: String? }

service Search {
  @route("/search")
  rpc search(q: String, page: Int?) -> SearchResult {
    SearchResult { q: q, page: page }
  }

  @route("/blog/:slug")
  rpc post(slug: String, src: String?) -> PostResult {
    PostResult { slug: slug, src: src }
  }
}
"#;

#[test]
fn a_route_with_multiple_params_captures_each_by_name() {
    let temp = TempDir::new("multi-param");
    let src = temp.write("app.link", MULTI_PARAM_PROGRAM);
    let server = Serve::start(&src);

    // El orden de los parámetros del RPC (slug, categoria) es distinto al
    // orden en que aparecen en la ruta (categoria, slug) -- a propósito,
    // para probar que el binding es por NOMBRE, no por posición.
    let (status, _, body) = server.get("/blog/rust/hola-mundo");
    assert_eq!(status, 200);
    assert_eq!(body, "\"rust/hola-mundo\"");
}

#[test]
fn a_route_with_one_more_literal_segment_wins_over_a_fully_dynamic_one() {
    // `/blog/featured/:slug` (1 segmento literal) y `/blog/:categoria/:slug`
    // (0 literales) matchean las dos `/blog/featured/algo` -- la más
    // específica gana, generalizando la precedencia de un solo parámetro
    // (§3.37) a cualquier cantidad de segmentos.
    let temp = TempDir::new("more-specific-wins");
    let src = temp.write("app.link", MULTI_PARAM_PROGRAM);
    let server = Serve::start(&src);

    let (status, _, body) = server.get("/blog/featured/algo");
    assert_eq!(status, 200);
    assert_eq!(body, "\"FEATURED/algo\"", "la ruta con el segmento literal fijo tiene que ganar");

    // Cualquier otra categoría sigue yendo al rpc totalmente dinámico.
    let (status, _, body) = server.get("/blog/rust/algo");
    assert_eq!(status, 200);
    assert_eq!(body, "\"rust/algo\"");
}

#[test]
fn a_catchall_route_captures_zero_or_more_trailing_segments() {
    let temp = TempDir::new("catchall-basic");
    let src = temp.write("app.link", CATCHALL_PROGRAM);
    let server = Serve::start(&src);

    // Varios segmentos restantes, unidos con "/".
    let (status, _, body) = server.get("/docs/api/v2/users");
    assert_eq!(status, 200);
    assert_eq!(body, "\"api/v2/users\"");

    // Un solo segmento restante.
    let (status, _, body) = server.get("/docs/intro");
    assert_eq!(status, 200);
    assert_eq!(body, "\"intro\"");

    // Cero segmentos restantes: el catch-all captura string vacío, no deja
    // de matchear.
    let (status, _, body) = server.get("/docs");
    assert_eq!(status, 200);
    assert_eq!(body, "\"\"");
}

#[test]
fn a_literal_route_wins_over_a_catchall_that_could_also_match() {
    // "/docs/changelog" (2 segmentos literales) y "/docs/:rest*" (1
    // segmento literal fijo, el resto es catch-all) matchean las dos
    // "/docs/changelog" -- la más específica (más segmentos literales)
    // tiene que ganar, mismo criterio que ya vale entre un literal y un
    // `:param` normal (§3.37/§3.42).
    let temp = TempDir::new("catchall-precedence");
    let src = temp.write("app.link", CATCHALL_PROGRAM);
    let server = Serve::start(&src);

    let (status, _, body) = server.get("/docs/changelog");
    assert_eq!(status, 200);
    assert_eq!(body, "\"EL-CHANGELOG\"", "la ruta literal tiene que ganarle al catch-all");
}

#[test]
fn a_required_and_an_optional_query_param_are_read_by_name() {
    let temp = TempDir::new("query-basic");
    let src = temp.write("app.link", QUERY_PROGRAM);
    let server = Serve::start(&src);

    let (status, _, body) = server.get("/search?q=rust&page=2");
    assert_eq!(status, 200);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["q"], "rust");
    assert_eq!(json["page"], 2);

    // El query param opcional, ausente: `null`, no un error.
    let (status, _, body) = server.get("/search?q=rust");
    assert_eq!(status, 200, "body: {body}");
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["q"], "rust");
    assert!(json["page"].is_null(), "page ausente tiene que ser null: {json}");
}

#[test]
fn a_missing_required_query_param_is_a_400_and_a_bad_int_too() {
    let temp = TempDir::new("query-errors");
    let src = temp.write("app.link", QUERY_PROGRAM);
    let server = Serve::start(&src);

    let (status, _, body) = server.get("/search");
    assert_eq!(status, 400);
    assert!(body.contains("'q'"), "el error tiene que nombrar el parámetro que falta: {body}");

    let (status, _, body) = server.get("/search?q=rust&page=no-es-un-numero");
    assert_eq!(status, 400);
    assert!(body.contains("'page'") && body.contains("entero"), "mensaje inesperado: {body}");
}

#[test]
fn a_query_string_alongside_a_path_param_does_not_corrupt_the_captured_segment() {
    // El bug real que motivó separar la query string ANTES de partir en
    // segmentos: sin eso, "/blog/hola-mundo?utm_source=twitter" -- una URL
    // perfectamente normal, cualquier link compartido en redes trae
    // parámetros de tracking -- capturaba "hola-mundo?utm_source=twitter"
    // ENTERO como el valor de :slug.
    let temp = TempDir::new("query-no-corrupt");
    let src = temp.write("app.link", QUERY_PROGRAM);
    let server = Serve::start(&src);

    // "utm_source" no es un parámetro declarado del rpc -- tiene que
    // ignorarse sin error, exactamente como cualquier query param
    // desconocido que un navegador/crawler agregue por su cuenta.
    let (status, _, body) = server.get("/blog/hola-mundo?src=twitter&utm_source=twitter_ads");
    assert_eq!(status, 200, "body: {body}");
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["slug"], "hola-mundo", "la query string no puede colarse en el segmento capturado: {json}");
    assert_eq!(json["src"], "twitter", "un query param desconocido (utm_source) no debe pisar uno real");

    // Sin query string en absoluto: el camino de siempre, sin regresión.
    let (status, _, body) = server.get("/blog/hola-mundo");
    assert_eq!(status, 200);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["slug"], "hola-mundo");
    assert!(json["src"].is_null());
}

#[test]
fn query_values_are_percent_and_plus_decoded() {
    let temp = TempDir::new("query-decode");
    let src = temp.write("app.link", QUERY_PROGRAM);
    let server = Serve::start(&src);

    let (status, _, body) = server.get("/search?q=hello+world&page=1");
    assert_eq!(status, 200, "body: {body}");
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["q"], "hello world", "'+' en query string significa espacio: {json}");

    let (status, _, body) = server.get("/search?q=caf%C3%A9&page=1");
    assert_eq!(status, 200);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["q"], "café", "%XX se decodifica igual que en un segmento de path: {json}");
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
    assert!(stderr.contains("conflicto"), "mensaje inesperado: {stderr}");

    // Dos rutas de FORMA distinta (ninguna es same_shape de la otra) pero
    // que igual podrían matchear el mismo path real, empatadas en
    // especificidad -- GRAMMAR.md §3.42, el caso que motivó no comparar
    // solo "misma forma" sino "podrían pisarse Y ninguna es más específica".
    let out = build(
        &temp,
        r#"
service A { @route("/blog/:categoria/ultimo") rpc p(categoria: String) -> String { categoria } }
service B { @route("/blog/destacado/:slug") rpc q(slug: String) -> String { slug } }
"#,
    );
    let stderr = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(!out.status.success(), "las dos matchean /blog/destacado/ultimo, y ninguna es más específica");
    assert!(stderr.contains("conflicto"), "mensaje inesperado: {stderr}");

    // Un nombre de parámetro repetido DENTRO de la misma ruta.
    let out = build(&temp, r#"service A { @route("/:slug/comentarios/:slug") rpc p(slug: String) -> String { slug } }"#);
    let stderr = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(!out.status.success());
    assert!(stderr.contains("más de una vez"), "mensaje inesperado: {stderr}");

    // Un tipo que no puede venir de un segmento de URL.
    let out = build(&temp, r#"service A { @route("/x/:on") rpc p(on: Bool) -> String { "x" } }"#);
    let stderr = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(!out.status.success());
    assert!(stderr.contains("String") && stderr.contains("Int"), "mensaje inesperado: {stderr}");

    // Un catch-all que NO es el último segmento -- inalcanzable siempre.
    let out = build(&temp, r#"service A { @route("/x/:rest*/y") rpc p(rest: String) -> String { rest } }"#);
    let stderr = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(!out.status.success(), "un catch-all en medio de la ruta debió rechazarse");
    assert!(stderr.contains("último segmento"), "mensaje inesperado: {stderr}");

    // Un catch-all tipado `Int` -- captura texto arbitrario, puede traer "/".
    let out = build(&temp, r#"service A { @route("/x/:rest*") rpc p(rest: Int) -> String { "x" } }"#);
    let stderr = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(!out.status.success(), "un catch-all como Int debió rechazarse");
    assert!(stderr.contains("catch-all") && stderr.contains("String"), "mensaje inesperado: {stderr}");

    // Un parámetro extra (query string, §3.62) con un tipo que no puede
    // venir de texto -- Bool no es String/Int/String?/Int?.
    let out = build(&temp, r#"service A { @route("/x") rpc p(activo: Bool) -> String { "x" } }"#);
    let stderr = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(!out.status.success(), "un query param Bool debió rechazarse");
    assert!(stderr.contains("query string"), "mensaje inesperado: {stderr}");

    // Un rpc con MENOS parámetros que los que la ruta pide sigue rechazado
    // -- de más ahora se acepta (query string), de menos nunca.
    let out = build(&temp, r#"service A { @route("/x/:id/:slug") rpc p(id: Int) -> String { "x" } }"#);
    let stderr = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(!out.status.success(), "le falta ':slug' al rpc, tiene que rechazarse");
    assert!(stderr.contains("le faltan") || stderr.contains("faltan"), "mensaje inesperado: {stderr}");

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
