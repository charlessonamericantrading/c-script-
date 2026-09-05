// Tests de integración para `@searchable` + `db.<c>.search(query)` --
// búsqueda de texto completo (PLAN.md §9.22 ítem 4, GRAMMAR.md §3.257).
//
// El pushdown a `tsvector`/`plainto_tsquery` real de Postgres se prueba en
// `compiler/tests/pg_integration.rs`. Este archivo cubre lo que SÍ es
// chequeable sin Postgres: rechazos del checker/parser, comportamiento
// completo vía `test { }` (a diferencia de `Vector<N>`/`@tenant`, `String`
// SÍ tiene sintaxis de literal y `search` no depende de ningún claim JWT,
// así que el fallback SQLite se puede ejercitar de punta a punta sin
// levantar un servidor real) y un servidor real sobre SQLite confirmando el
// camino HTTP completo.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-searchable-{name}-{}-{}",
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

fn run_link_tests(source: &str) -> (bool, String) {
    let temp = TempDir::new("run");
    let src = temp.write("app.link", source);
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("test").arg(&src).output().expect("linkc test");
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    (out.status.success(), text)
}

const SEARCH_PROGRAM: &str = r#"
type Article = {
  id: Int,
  @searchable
  title: String,
  @searchable
  body: String,
  views: Int,
}
db { articles: Article[] }
service Articles {
  rpc add(title: String, body: String, views: Int) -> Article {
    db.articles.insert(Article { id: 0, title: title, body: body, views: views })
  }
  rpc find(query: String) -> Article[] { db.articles.search(query) }
}
"#;

#[test]
fn searchable_annotation_compiles_over_the_real_binary() {
    let source = format!("{SEARCH_PROGRAM}\ntest \"dummy\" {{ assert(true, \"ok\"); }}\n");
    let (ok, out) = run_link_tests(&source);
    assert!(ok, "el programa base con @searchable/search debería compilar: {out}");
}

#[test]
fn searchable_rejects_a_non_string_field() {
    let bad_program = r#"
type Article = {
  id: Int,
  @searchable
  views: Int,
}
db { articles: Article[] }
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: '@searchable' sobre un campo Int: {out}");
    assert!(out.contains("String"), "{out}");
}

#[test]
fn searchable_repeated_on_the_same_field_is_rejected() {
    let bad_program = r#"
type Article = {
  id: Int,
  @searchable
  @searchable
  title: String,
}
db { articles: Article[] }
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: '@searchable' repetido sobre el mismo campo: {out}");
}

#[test]
fn searchable_is_incompatible_with_encrypted_on_the_same_field() {
    let bad_program = r#"
type Article = {
  id: Int,
  @searchable
  @encrypted
  title: String,
}
db { articles: Article[] }
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: '@searchable' + '@encrypted' en el mismo campo: {out}");
    assert!(out.contains("encrypted"), "{out}");
}

#[test]
fn search_rejects_a_collection_with_no_searchable_field_declared() {
    let bad_program = r#"
type Article = {
  id: Int,
  title: String,
}
db { articles: Article[] }
service Articles {
  rpc find(query: String) -> Article[] { db.articles.search(query) }
}
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: ningún campo '@searchable' declarado: {out}");
    assert!(out.contains("@searchable"), "{out}");
}

#[test]
fn search_rejects_a_non_string_argument() {
    let bad_program = r#"
type Article = {
  id: Int,
  @searchable
  title: String,
}
db { articles: Article[] }
service Articles {
  rpc find(query: Int) -> Article[] { db.articles.search(query) }
}
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: 'search' con un Int en vez de String: {out}");
}

#[test]
fn searchable_on_an_enum_variant_field_is_rejected() {
    let bad_program = r#"
enum Shape {
  Circle {
    @searchable
    label: String,
  },
}
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: '@searchable' sobre un campo de variante de enum: {out}");
    assert!(out.contains("JSON"), "{out}");
}

#[test]
fn search_over_multiple_searchable_fields_combines_them_with_and_semantics_over_a_real_sqlite_test_block() {
    let source = format!(
        r#"{SEARCH_PROGRAM}
test "search combina title+body con semántica AND" {{
  Articles.add("Rust en producción", "una guía práctica de despliegue", 10);
  Articles.add("Guía de TypeScript", "tipos avanzados y genéricos", 5);
  Articles.add("Otra nota", "sin relación con ninguno de los dos temas", 1);

  let both = Articles.find("guía");
  assert(both.length() == 2, "las dos filas con 'guía' en título o cuerpo (case-insensitive)");

  let rust_only = Articles.find("rust despliegue");
  assert(rust_only.length() == 1, "AND entre dos términos: solo la fila que tiene los dos");
  assert(rust_only[0].title == "Rust en producción", "es la fila correcta");

  let none = Articles.find("inexistente");
  assert(none.length() == 0, "ningún término matchea -> lista vacía, no un error");
}}
"#
    );
    let (ok, out) = run_link_tests(&source);
    assert!(ok, "el test block de comportamiento debería pasar sobre SQLite real: {out}");
}

/// Espera a que `/live` responda, con timeout -- mismo criterio que
/// `cli_vector_annotation.rs`.
fn wait_ready(port: u16) {
    for _ in 0..100 {
        if ureq::get(&format!("http://127.0.0.1:{port}/live")).call().is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("el servidor no levantó a tiempo");
}

fn rpc_status_and_body(port: u16, method: &str, body: &str) -> (u16, String) {
    match ureq::post(&format!("http://127.0.0.1:{port}/{method}")).set("Content-Type", "application/json").send_string(body) {
        Ok(r) => (r.status(), r.into_string().unwrap_or_default()),
        Err(ureq::Error::Status(status, r)) => (status, r.into_string().unwrap_or_default()),
        Err(e) => panic!("{method} falló de red: {e}"),
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

#[test]
fn search_works_end_to_end_over_a_real_http_server_against_sqlite() {
    let temp = TempDir::new("http");
    let link_path = temp.write("app.link", SEARCH_PROGRAM);
    let db_path = temp.0.join("app.db");
    let port = free_port();

    let mut child = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&link_path)
        .arg(port.to_string())
        .arg("--db")
        .arg(&db_path)
        .spawn()
        .expect("spawn linkc serve");
    wait_ready(port);

    let (status, _) = rpc_status_and_body(port, "Articles/add", r#"{"title":"Manual de c-script","body":"contrato tipado end to end","views":42}"#);
    assert_eq!(status, 200);
    let (status, _) = rpc_status_and_body(port, "Articles/add", r#"{"title":"Recetario de cocina","body":"nada que ver con lenguajes","views":3}"#);
    assert_eq!(status, 200);

    let (status, body) = rpc_status_and_body(port, "Articles/find", r#"{"query":"c-script"}"#);
    assert_eq!(status, 200, "body: {body}");
    let found: serde_json::Value = serde_json::from_str(&body).unwrap();
    let found = found.as_array().unwrap();
    assert_eq!(found.len(), 1, "{body}");
    assert_eq!(found[0]["title"], "Manual de c-script", "{body}");

    let (status, empty_body) = rpc_status_and_body(port, "Articles/find", r#"{"query":"palabraQueNoExiste"}"#);
    assert_eq!(status, 200, "body: {empty_body}");
    let empty: serde_json::Value = serde_json::from_str(&empty_body).unwrap();
    assert_eq!(empty.as_array().unwrap().len(), 0, "{empty_body}");

    let _ = child.kill();
    let _ = child.wait();
}
