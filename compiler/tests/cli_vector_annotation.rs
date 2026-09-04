// Tests de integración para `Vector<N>` + `db.<c>.nearest(...)` -- búsqueda
// semántica nativa (PLAN.md §9.21 Fase 4 ítem 13, GRAMMAR.md §3.254).
//
// La búsqueda pushdown a pgvector real (Postgres, `<=>`) se prueba en
// `compiler/tests/pg_integration.rs`. Este archivo cubre lo que SÍ es
// chequeable sin Postgres: rechazos del checker/parser, los artefactos de
// codegen (contract.d.ts/validators.ts/schemas.ts/openapi.json), y un
// servidor real sobre SQLite ejercitando insert/all/find/nearest de punta a
// punta -- SIN `test { }`, porque `Vector<N>` no tiene sintaxis de literal
// en el lenguaje (GRAMMAR.md §3.254, "Límites honestos"): un valor solo
// nace de un parámetro de rpc o de una lectura de `db`, así que ningún
// `test { }` puede construir uno para pasarle a `insert`/`nearest`.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-vector-{name}-{}-{}",
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

const VECTOR_PROGRAM: &str = r#"
type Note = {
  id: Int,
  embedding: Vector<3>,
  text: String,
}
db { notes: Note[] }
service Notes {
  rpc add(text: String, embedding: Vector<3>) -> Note {
    db.notes.insert(Note { id: 0, embedding: embedding, text: text })
  }
  rpc list() -> Note[] { db.notes.all() }
  rpc get(id: Int) -> Note? { db.notes.find(id) }
  rpc closest(query: Vector<3>, k: Int) -> Note[] {
    db.notes.nearest(|n: Note| { n.embedding }, query, k)
  }
}
"#;

#[test]
fn vector_annotation_compiles_and_type_checks_over_the_real_binary() {
    let source = format!("{VECTOR_PROGRAM}\ntest \"dummy\" {{ assert(true, \"ok\"); }}\n");
    let (ok, out) = run_link_tests(&source);
    assert!(ok, "el programa base con Vector<3>/nearest debería compilar: {out}");
}

#[test]
fn vector_type_rejects_a_missing_dimension_argument() {
    let bad_program = r#"
type Note = {
  id: Int,
  embedding: Vector,
  text: String,
}
db { notes: Note[] }
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: 'Vector' sin dimensión: {out}");
    assert!(out.contains("Vector<N>") && out.contains("1 argumento"), "{out}");
}

#[test]
fn vector_type_rejects_a_type_argument_instead_of_an_integer() {
    let bad_program = r#"
type Note = {
  id: Int,
  embedding: Vector<Int>,
  text: String,
}
db { notes: Note[] }
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: 'Vector<Int>' -- N tiene que ser un entero literal: {out}");
    assert!(out.contains("entero literal"), "{out}");
}

#[test]
fn vector_type_rejects_a_zero_dimension() {
    let bad_program = r#"
type Note = {
  id: Int,
  embedding: Vector<0>,
  text: String,
}
db { notes: Note[] }
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: 'Vector<0>': {out}");
    assert!(out.contains("mayor que 0"), "{out}");
}

#[test]
fn vector_two_different_dimensions_are_incompatible_types() {
    let bad_program = r#"
type Note = {
  id: Int,
  embedding: Vector<3>,
  text: String,
}
db { notes: Note[] }
service Notes {
  rpc closest(query: Vector<5>, k: Int) -> Note[] {
    db.notes.nearest(|n: Note| { n.embedding }, query, k)
  }
}
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: Vector<5> contra un campo Vector<3>: {out}");
    assert!(out.contains("Vector<3>"), "{out}");
}

#[test]
fn nearest_rejects_a_selector_pointing_to_a_non_vector_field() {
    let bad_program = r#"
type Note = {
  id: Int,
  embedding: Vector<3>,
  text: String,
}
db { notes: Note[] }
service Notes {
  rpc closest(query: Vector<3>, k: Int) -> Note[] {
    db.notes.nearest(|n: Note| { n.text }, query, k)
  }
}
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: selector sobre 'text' (String), no un Vector<N>: {out}");
    assert!(out.contains("Vector<N>"), "{out}");
}

#[test]
fn nearest_rejects_a_selector_with_a_derived_expression() {
    let bad_program = r#"
type Note = {
  id: Int,
  embedding: Vector<3>,
  text: String,
}
db { notes: Note[] }
service Notes {
  rpc closest(query: Vector<3>, k: Int) -> Note[] {
    db.notes.nearest(|n: Note| { n.embedding[0] }, query, k)
  }
}
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: selector con una expresión derivada, no un acceso de campo simple: {out}");
}

#[test]
fn nearest_rejects_an_unknown_field_name() {
    let bad_program = r#"
type Note = {
  id: Int,
  embedding: Vector<3>,
  text: String,
}
db { notes: Note[] }
service Notes {
  rpc closest(query: Vector<3>, k: Int) -> Note[] {
    db.notes.nearest(|n: Note| { n.doesNotExist }, query, k)
  }
}
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: 'doesNotExist' no es un campo de Note: {out}");
}

#[test]
fn vector_annotation_generated_typescript_and_json_schema_artifacts() {
    let temp = TempDir::new("build");
    let src = temp.write("app.link", VECTOR_PROGRAM);
    let gen_dir = temp.0.join("gen");
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("build").arg(&src).arg(&gen_dir).output().expect("linkc build");
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "linkc build falló: {text}");

    let contract = std::fs::read_to_string(gen_dir.join("contract.d.ts")).expect("leer contract.d.ts");
    assert!(contract.contains("embedding: number[]"), "el campo Vector<3> tiene que emitir number[]: {contract}");

    let validators = std::fs::read_to_string(gen_dir.join("validators.ts")).expect("leer validators.ts");
    assert!(validators.contains("embedding"), "isNote tiene que chequear 'embedding': {validators}");
    assert!(validators.contains(".length === 3"), "el chequeo tiene que fijar el largo exacto: {validators}");
    assert!(validators.contains("Number.isFinite"), "el chequeo tiene que rechazar NaN/Infinity: {validators}");

    let schemas = std::fs::read_to_string(gen_dir.join("schemas.ts")).expect("leer schemas.ts");
    assert!(schemas.contains("z.array(z.number().finite()).length(3)"), "el schema Zod de 'embedding': {schemas}");

    let openapi = std::fs::read_to_string(gen_dir.join("openapi.json")).expect("leer openapi.json");
    let openapi: serde_json::Value = serde_json::from_str(&openapi).expect("openapi.json debe ser JSON válido");
    let embedding_schema = &openapi["components"]["schemas"]["Note"]["properties"]["embedding"];
    assert_eq!(embedding_schema["type"], "array", "{openapi}");
    assert_eq!(embedding_schema["minItems"], 3, "{openapi}");
    assert_eq!(embedding_schema["maxItems"], 3, "{openapi}");
}

/// Espera a que `/live` responda, con timeout -- mismo criterio que
/// `cli_ref_annotation.rs`.
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
fn vector_annotation_full_crud_and_nearest_ordering_over_real_sqlite() {
    let temp = TempDir::new("crud-nearest");
    let link_path = temp.write("app.link", VECTOR_PROGRAM);
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

    let (status, a) = rpc_status_and_body(port, "Notes/add", r#"{"text":"a","embedding":[1.0,0.0,0.0]}"#);
    assert_eq!(status, 200, "body: {a}");
    let (status, b) = rpc_status_and_body(port, "Notes/add", r#"{"text":"b","embedding":[0.0,1.0,0.0]}"#);
    assert_eq!(status, 200, "body: {b}");
    let (status, c) = rpc_status_and_body(port, "Notes/add", r#"{"text":"c","embedding":[0.9,0.1,0.0]}"#);
    assert_eq!(status, 200, "body: {c}");

    let (status, list_body) = rpc_status_and_body(port, "Notes/list", "{}");
    assert_eq!(status, 200, "body: {list_body}");
    let list: serde_json::Value = serde_json::from_str(&list_body).unwrap();
    assert_eq!(list.as_array().unwrap().len(), 3, "{list_body}");

    // Vector [1,0,0]: 'a' (idéntico) tiene que ganarle a 'c' (casi
    // idéntico), y los dos le ganan a 'b' (ortogonal, coseno = 1.0).
    let (status, near_body) = rpc_status_and_body(port, "Notes/closest", r#"{"query":[1.0,0.0,0.0],"k":2}"#);
    assert_eq!(status, 200, "body: {near_body}");
    let near: serde_json::Value = serde_json::from_str(&near_body).unwrap();
    let near = near.as_array().unwrap();
    assert_eq!(near.len(), 2, "{near_body}");
    assert_eq!(near[0]["text"], "a", "la fila idéntica tiene que salir primero: {near_body}");
    assert_eq!(near[1]["text"], "c", "la fila casi-idéntica tiene que salir segunda: {near_body}");

    // Vector [0,1,0]: 'b' es el único cercano.
    let (status, near_b_body) = rpc_status_and_body(port, "Notes/closest", r#"{"query":[0.0,1.0,0.0],"k":1}"#);
    assert_eq!(status, 200, "body: {near_b_body}");
    let near_b: serde_json::Value = serde_json::from_str(&near_b_body).unwrap();
    assert_eq!(near_b[0]["text"], "b", "{near_b_body}");

    // Dimensión equivocada -> 400, nunca un 500 ni un resultado silencioso.
    let (status, bad_body) = rpc_status_and_body(port, "Notes/closest", r#"{"query":[1.0,0.0],"k":1}"#);
    assert_eq!(status, 400, "un Vector<3> con 2 componentes tiene que ser 400: {bad_body}");

    // El campo se guarda como BLOB físico, no como JSON -- confirma que
    // 'native_sql_type'/'ColumnPlan::kind' no lo mandó por el camino JSON.
    let conn = rusqlite::Connection::open(&db_path).expect("abrir la base sqlite");
    let decl_type: String = conn
        .query_row("SELECT type FROM pragma_table_info('notes') WHERE name = 'embedding'", [], |r| r.get(0))
        .expect("leer el tipo declarado de 'embedding'");
    assert_eq!(decl_type, "BLOB", "la columna Vector<3> tiene que ser BLOB en SQLite");

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn vector_annotation_survives_a_server_restart_against_the_same_sqlite_file() {
    let temp = TempDir::new("restart");
    let link_path = temp.write("app.link", VECTOR_PROGRAM);
    let db_path = temp.0.join("app.db");
    let port1 = free_port();

    let mut child1 = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&link_path)
        .arg(port1.to_string())
        .arg("--db")
        .arg(&db_path)
        .spawn()
        .expect("spawn linkc serve (1st run)");
    wait_ready(port1);
    let (status, _) = rpc_status_and_body(port1, "Notes/add", r#"{"text":"persisted","embedding":[0.1,0.2,0.3]}"#);
    assert_eq!(status, 200);
    let _ = child1.kill();
    let _ = child1.wait();

    let port2 = free_port();
    let mut child2 = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&link_path)
        .arg(port2.to_string())
        .arg("--db")
        .arg(&db_path)
        .spawn()
        .expect("spawn linkc serve (2nd run)");
    wait_ready(port2);
    let (status, list_body) = rpc_status_and_body(port2, "Notes/list", "{}");
    assert_eq!(status, 200, "el schema BLOB tiene que seguir matcheando en el segundo arranque: {list_body}");
    let list: serde_json::Value = serde_json::from_str(&list_body).unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1, "{list_body}");
    assert_eq!(list[0]["text"], "persisted");

    let _ = child2.kill();
    let _ = child2.wait();
}
