// Tests de integración para `@primaryKey(campo1, campo2, ...)` -- claves
// primarias COMPUESTAS (PLAN.md §9.21 Fase 3 ítem 11 resto, GRAMMAR.md
// §3.255). Núcleo CRUD acotado a propósito (all/find/insert/insertMany/
// delete/applyPatch/count) -- el resto de los métodos de `db.<c>` se
// rechazan en compilación con un mensaje claro, ver
// `primary_key_annotation_rejects_an_unsupported_method` más abajo.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-pk-{name}-{}-{}",
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

const ORDER_LINE_PROGRAM: &str = r#"
@primaryKey(orderId, lineNumber)
type OrderLine = {
  orderId: Int,
  lineNumber: Int,
  sku: String,
  qty: Int,
}
db { orderLines: OrderLine[] }
service Orders {
  rpc addLine(orderId: Int, lineNumber: Int, sku: String, qty: Int) -> OrderLine {
    db.orderLines.insert(OrderLine { orderId: orderId, lineNumber: lineNumber, sku: sku, qty: qty })
  }
  rpc getLine(id: { orderId: Int, lineNumber: Int }) -> OrderLine? {
    db.orderLines.find(id)
  }
  rpc listLines() -> OrderLine[] { db.orderLines.all() }
  rpc countLines() -> Int { db.orderLines.count() }
  rpc removeLine(id: { orderId: Int, lineNumber: Int }) -> Bool {
    db.orderLines.delete(id)
  }
  rpc bumpQty(id: { orderId: Int, lineNumber: Int }, patch: Patch<OrderLine>) -> OrderLine {
    db.orderLines.applyPatch(id, patch)
  }
}
"#;

#[test]
fn primary_key_annotation_compiles_and_type_checks_over_the_real_binary() {
    let source = format!("{ORDER_LINE_PROGRAM}\ntest \"dummy\" {{ assert(true, \"ok\"); }}\n");
    let (ok, out) = run_link_tests(&source);
    assert!(ok, "el programa base con @primaryKey debería compilar: {out}");
}

#[test]
fn primary_key_annotation_rejects_fewer_than_two_fields() {
    let bad_program = r#"
@primaryKey(orderId)
type OrderLine = { orderId: Int, sku: String }
db { orderLines: OrderLine[] }
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: '@primaryKey' con un solo campo: {out}");
    assert!(out.contains("al menos 2 campos"), "{out}");
}

#[test]
fn primary_key_annotation_rejects_a_repeated_field() {
    let bad_program = r#"
@primaryKey(orderId, orderId)
type OrderLine = { orderId: Int, sku: String }
db { orderLines: OrderLine[] }
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: '@primaryKey' repite el mismo campo: {out}");
    assert!(out.contains("repite el mismo campo"), "{out}");
}

#[test]
fn primary_key_annotation_rejects_an_unknown_field() {
    let bad_program = r#"
@primaryKey(orderId, doesNotExist)
type OrderLine = { orderId: Int, sku: String }
db { orderLines: OrderLine[] }
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: 'doesNotExist' no es un campo declarado: {out}");
    assert!(out.contains("no es un campo declarado"), "{out}");
}

#[test]
fn primary_key_annotation_rejects_an_unsupported_field_type() {
    let bad_program = r#"
@primaryKey(orderId, flag)
type OrderLine = { orderId: Int, flag: Bool, sku: String }
db { orderLines: OrderLine[] }
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: 'flag' es Bool, no Int/Uuid/String: {out}");
    assert!(out.contains("tiene que ser Int, Uuid o String"), "{out}");
}

#[test]
fn primary_key_annotation_rejects_an_optional_field() {
    let bad_program = r#"
@primaryKey(orderId, lineNumber)
type OrderLine = { orderId: Int, lineNumber: Int?, sku: String }
db { orderLines: OrderLine[] }
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: 'lineNumber' es opcional: {out}");
    assert!(out.contains("tiene que ser requerido"), "{out}");
}

#[test]
fn primary_key_annotation_rejects_declaring_it_twice() {
    let bad_program = r#"
@primaryKey(orderId, lineNumber)
@primaryKey(orderId, sku)
type OrderLine = { orderId: Int, lineNumber: Int, sku: String }
db { orderLines: OrderLine[] }
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: dos '@primaryKey' sobre el mismo type: {out}");
    assert!(out.contains("más de una vez"), "{out}");
}

#[test]
fn primary_key_annotation_rejects_an_unsupported_method() {
    let bad_program = r#"
@primaryKey(orderId, lineNumber)
type OrderLine = { orderId: Int, lineNumber: Int, sku: String }
db { orderLines: OrderLine[] }
service Orders {
  rpc badPage(limit: Int, offset: Int) -> OrderLine[] {
    db.orderLines.page(limit, offset)
  }
}
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: 'page' no está soportado sobre PK compuesta todavía: {out}");
    assert!(out.contains("clave primaria compuesta"), "{out}");
}

#[test]
fn primary_key_annotation_generated_contract_shows_the_composite_id_as_an_inline_struct() {
    let temp = TempDir::new("build");
    let src = temp.write("app.link", ORDER_LINE_PROGRAM);
    let gen_dir = temp.0.join("gen");
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("build").arg(&src).arg(&gen_dir).output().expect("linkc build");
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "linkc build falló: {text}");

    let contract = std::fs::read_to_string(gen_dir.join("contract.d.ts")).expect("leer contract.d.ts");
    assert!(
        contract.contains("orderId: number; lineNumber: number") || contract.contains("orderId: number, lineNumber: number"),
        "getLine tiene que tomar un id inline con los dos campos de la PK: {contract}"
    );
}

/// Espera a que `/live` responda, con timeout -- mismo criterio que
/// `cli_vector_annotation.rs`/`cli_ref_annotation.rs`.
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
fn primary_key_annotation_full_crud_cycle_over_real_sqlite() {
    let temp = TempDir::new("crud");
    let link_path = temp.write("app.link", ORDER_LINE_PROGRAM);
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

    let (status, a) = rpc_status_and_body(port, "Orders/addLine", r#"{"orderId":1,"lineNumber":1,"sku":"A","qty":10}"#);
    assert_eq!(status, 200, "body: {a}");
    let (status, b) = rpc_status_and_body(port, "Orders/addLine", r#"{"orderId":1,"lineNumber":2,"sku":"B","qty":5}"#);
    assert_eq!(status, 200, "body: {b}");
    let (status, c) = rpc_status_and_body(port, "Orders/addLine", r#"{"orderId":2,"lineNumber":1,"sku":"C","qty":1}"#);
    assert_eq!(status, 200, "body: {c}");

    let (status, list_body) = rpc_status_and_body(port, "Orders/listLines", "{}");
    assert_eq!(status, 200, "body: {list_body}");
    let list: serde_json::Value = serde_json::from_str(&list_body).unwrap();
    assert_eq!(list.as_array().unwrap().len(), 3, "{list_body}");

    let (status, count_body) = rpc_status_and_body(port, "Orders/countLines", "{}");
    assert_eq!(status, 200, "body: {count_body}");
    assert_eq!(count_body, "3");

    let (status, get_body) = rpc_status_and_body(port, "Orders/getLine", r#"{"id":{"orderId":1,"lineNumber":2}}"#);
    assert_eq!(status, 200, "body: {get_body}");
    let got: serde_json::Value = serde_json::from_str(&get_body).unwrap();
    assert_eq!(got["sku"], "B", "{get_body}");

    let (status, missing_body) = rpc_status_and_body(port, "Orders/getLine", r#"{"id":{"orderId":99,"lineNumber":99}}"#);
    assert_eq!(status, 200, "body: {missing_body}");
    assert_eq!(missing_body, "null", "un id compuesto inexistente da null, no un error: {missing_body}");

    // applyPatch: cambia un campo normal, IGNORA un intento de tocar la
    // propia PK (mismo criterio que "id" en el camino escalar).
    let (status, patched_body) =
        rpc_status_and_body(port, "Orders/bumpQty", r#"{"id":{"orderId":1,"lineNumber":1},"patch":{"qty":999}}"#);
    assert_eq!(status, 200, "body: {patched_body}");
    let patched: serde_json::Value = serde_json::from_str(&patched_body).unwrap();
    assert_eq!(patched["qty"], 999, "{patched_body}");

    let (status, sneaky_body) =
        rpc_status_and_body(port, "Orders/bumpQty", r#"{"id":{"orderId":1,"lineNumber":1},"patch":{"orderId":777}}"#);
    assert_eq!(status, 200, "body: {sneaky_body}");
    let sneaky: serde_json::Value = serde_json::from_str(&sneaky_body).unwrap();
    assert_eq!(sneaky["orderId"], 1, "un patch nunca puede reasignar un campo de la PK compuesta: {sneaky_body}");

    // delete: existente da true, ya-borrado da false (idempotente).
    let (status, del1) = rpc_status_and_body(port, "Orders/removeLine", r#"{"id":{"orderId":2,"lineNumber":1}}"#);
    assert_eq!(status, 200, "body: {del1}");
    assert_eq!(del1, "true", "{del1}");
    let (status, del2) = rpc_status_and_body(port, "Orders/removeLine", r#"{"id":{"orderId":2,"lineNumber":1}}"#);
    assert_eq!(status, 200, "body: {del2}");
    assert_eq!(del2, "false", "borrar una fila que ya no existe da false, no un error: {del2}");

    let (status, final_count) = rpc_status_and_body(port, "Orders/countLines", "{}");
    assert_eq!(status, 200);
    assert_eq!(final_count, "2", "3 insertadas, 1 borrada: {final_count}");

    // El DDL físico es un PRIMARY KEY compuesto de verdad, no una
    // simulación -- confirma que 'create_table_sql' tomó la rama nueva.
    let conn = rusqlite::Connection::open(&db_path).expect("abrir la base sqlite");
    let sql: String = conn
        .query_row("SELECT sql FROM sqlite_master WHERE name = 'orderLines'", [], |r| r.get(0))
        .expect("leer el DDL físico de 'orderLines'");
    assert!(sql.contains("PRIMARY KEY (\"orderId\", \"lineNumber\")"), "DDL real: {sql}");
    assert!(!sql.to_uppercase().contains("AUTOINCREMENT"), "una PK compuesta nunca autoincrementa: {sql}");

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn primary_key_annotation_survives_a_server_restart_against_the_same_sqlite_file() {
    let temp = TempDir::new("restart");
    let link_path = temp.write("app.link", ORDER_LINE_PROGRAM);
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
    let (status, _) = rpc_status_and_body(port1, "Orders/addLine", r#"{"orderId":5,"lineNumber":1,"sku":"persisted","qty":1}"#);
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
    let (status, list_body) = rpc_status_and_body(port2, "Orders/listLines", "{}");
    assert_eq!(status, 200, "el schema con PRIMARY KEY compuesto tiene que seguir matcheando en el segundo arranque: {list_body}");
    let list: serde_json::Value = serde_json::from_str(&list_body).unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1, "{list_body}");
    assert_eq!(list[0]["sku"], "persisted");

    let _ = child2.kill();
    let _ = child2.wait();
}
