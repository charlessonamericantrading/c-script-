// `linkc db export` (GRAMMAR.md §3.185): vuelca cada colección declarada a
// un archivo JSON, byte-idéntico al wire real (mismo `value_to_json` que
// `db.<c>.all()` ya usa por HTTP). Se prueba contra el BINARIO real: que
// compile no prueba que exporte filas soft-deleted, ni que un `.link` con
// una colección que la base nunca creó exporte un array vacío en vez de
// fallar.

use serde_json::Value;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

const PROGRAM: &str = r#"
type Item = { id: Int, name: String, price: Decimal, tag: Role, @softDelete deletedAt: Timestamp? = null }
enum Role { Regular, Vip }
db { items: Item[] }
service Items {
  rpc add(name: String, price: Decimal, tag: Role) -> Item { db.items.insert(Item { id: 0, name: name, price: price, tag: tag }) }
  rpc remove(id: Int) -> Bool { db.items.delete(id) }
  rpc all() -> Item[] { db.items.all() }
}
"#;

/// `PROGRAM` más una colección extra (`orders`) que nunca se sirve --
/// para ejercitar de verdad "colección declarada, tabla nunca creada"
/// contra una base que SÍ existe (a diferencia del caso trivial de una
/// `.db` inexistente): un `linkc serve` normal crea tablas para TODA
/// colección declarada, así que hace falta un `.link` más grande que el
/// que de verdad sirvió la base para reproducir esto.
const PROGRAM_WITH_EXTRA_COLLECTION: &str = r#"
type Item = { id: Int, name: String, price: Decimal, tag: Role, @softDelete deletedAt: Timestamp? = null }
type Order = { id: Int, itemId: Int }
enum Role { Regular, Vip }
db { items: Item[], orders: Order[] }
service Items {
  rpc add(name: String, price: Decimal, tag: Role) -> Item { db.items.insert(Item { id: 0, name: name, price: price, tag: tag }) }
}
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "linkc-db-export-{name}-{}-{}",
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

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
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

struct Serve {
    child: Child,
    port: u16,
}

impl Serve {
    fn start(link_path: &PathBuf, db_path: &PathBuf) -> Self {
        let port = free_port();
        let child = Command::new(env!("CARGO_BIN_EXE_linkc"))
            .arg("serve")
            .arg(link_path)
            .arg(port.to_string())
            .arg("--db")
            .arg(db_path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("iniciar 'linkc serve'");
        let server = Serve { child, port };
        server.wait_ready();
        server
    }

    fn wait_ready(&self) {
        for _ in 0..200 {
            if ureq::get(&format!("http://127.0.0.1:{}/health", self.port)).call().is_ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("'linkc serve' no abrió el puerto {} a tiempo", self.port);
    }

    fn rpc(&self, method: &str, body: &str) -> Value {
        let text = ureq::post(&format!("http://127.0.0.1:{}/{method}", self.port))
            .set("Content-Type", "application/json")
            .send_string(body)
            .unwrap_or_else(|e| panic!("{method} falló: {e}"))
            .into_string()
            .expect("leer el body");
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{method} no devolvió JSON ({e}): {text}"))
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn run_export(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("db").arg("export").args(args).output().expect("ejecutar linkc db export");
    (out.status.success(), String::from_utf8_lossy(&out.stdout).to_string(), String::from_utf8_lossy(&out.stderr).to_string())
}

#[test]
fn a_database_that_does_not_exist_yet_exports_every_collection_as_an_empty_array() {
    let temp = TempDir::new("fresh");
    let src = temp.write("app.link", PROGRAM);
    let out_path = temp.path("export.json");
    let (success, stdout, stderr) = run_export(&[src.to_str().unwrap(), out_path.to_str().unwrap()]);
    assert!(success, "stdout: {stdout}\nstderr: {stderr}");
    let file: Value = serde_json::from_str(&std::fs::read_to_string(&out_path).unwrap()).unwrap();
    assert_eq!(file["collections"]["items"], serde_json::json!([]), "{file}");
}

/// El caso real: una base ya poblada por `linkc serve` de verdad, con una
/// fila soft-deleted -- confirma que exporta filas REALES y que NO filtra
/// `@softDelete` (mismo criterio que `db.tableStats()`/`db inspect`:
/// verdad física, a propósito distinto de `all()`/`count()`).
#[test]
fn a_real_database_populated_by_linkc_serve_exports_real_rows_including_soft_deleted_ones() {
    let temp = TempDir::new("populated");
    let src = temp.write("app.link", PROGRAM);
    let db_path = temp.path("app.db");

    let server = Serve::start(&src, &db_path);
    server.rpc("Items/add", r#"{"name":"Widget","price":"19.9900","tag":"Vip"}"#);
    let created = server.rpc("Items/add", r#"{"name":"Gadget","price":"5.5000","tag":"Regular"}"#);
    let id = created["id"].as_i64().expect("id");
    server.rpc("Items/remove", &format!(r#"{{"id":{id}}}"#));
    drop(server);

    let out_path = temp.path("export.json");
    let (success, stdout, stderr) = run_export(&[src.to_str().unwrap(), out_path.to_str().unwrap(), "--db", db_path.to_str().unwrap()]);
    assert!(success, "stdout: {stdout}\nstderr: {stderr}");
    let file: Value = serde_json::from_str(&std::fs::read_to_string(&out_path).unwrap()).unwrap();
    let items = file["collections"]["items"].as_array().expect("items array");
    assert_eq!(items.len(), 2, "las dos filas, incluida la soft-deleted: {items:?}");
    assert!(items.iter().any(|r| r["name"] == "Widget" && r["price"] == "19.9900" && r["tag"] == "Vip"), "{items:?}");
    let gadget = items.iter().find(|r| r["name"] == "Gadget").expect("la fila soft-deleted sigue exportándose");
    assert!(gadget["deletedAt"].is_string(), "@softDelete real, con fecha puesta: {gadget}");
}

/// Un `.link` con una colección (`orders`) que la base FÍSICA nunca creó
/// (servida con un programa más chico) exporta esa colección como array
/// vacío, no un error -- mismo criterio de "declarada != creada" que
/// `db inspect` ya establece, ejercitado acá contra una base que SÍ existe.
#[test]
fn a_collection_declared_but_never_created_exports_as_an_empty_array() {
    let temp = TempDir::new("partial");
    let small_src = temp.write("small.link", PROGRAM);
    let db_path = temp.path("app.db");
    let server = Serve::start(&small_src, &db_path);
    server.rpc("Items/add", r#"{"name":"Widget","price":"1.0000","tag":"Regular"}"#);
    drop(server);

    let big_src = temp.write("big.link", PROGRAM_WITH_EXTRA_COLLECTION);
    let out_path = temp.path("export.json");
    let (success, stdout, stderr) = run_export(&[big_src.to_str().unwrap(), out_path.to_str().unwrap(), "--db", db_path.to_str().unwrap()]);
    assert!(success, "stdout: {stdout}\nstderr: {stderr}");
    let file: Value = serde_json::from_str(&std::fs::read_to_string(&out_path).unwrap()).unwrap();
    assert_eq!(file["collections"]["items"].as_array().unwrap().len(), 1, "{file}");
    assert_eq!(file["collections"]["orders"], serde_json::json!([]), "declarada pero nunca creada -- vacío, no error: {file}");
}

/// El export tiene que ser BYTE-IDÉNTICO a lo que un cliente real ya
/// recibe de `all()` -- mismo `value_to_json`, sin encoding paralelo que
/// pueda divergir (Decimal, enum, Timestamp incluidos).
#[test]
fn exported_rows_match_the_real_rpc_response_byte_for_byte() {
    let temp = TempDir::new("wire-match");
    let src = temp.write("app.link", PROGRAM);
    let db_path = temp.path("app.db");
    let server = Serve::start(&src, &db_path);
    server.rpc("Items/add", r#"{"name":"Widget","price":"19.9900","tag":"Vip"}"#);
    let via_rpc = server.rpc("Items/all", "{}");
    drop(server);

    let out_path = temp.path("export.json");
    let (success, stdout, stderr) = run_export(&[src.to_str().unwrap(), out_path.to_str().unwrap(), "--db", db_path.to_str().unwrap()]);
    assert!(success, "stdout: {stdout}\nstderr: {stderr}");
    let file: Value = serde_json::from_str(&std::fs::read_to_string(&out_path).unwrap()).unwrap();
    assert_eq!(&file["collections"]["items"], &via_rpc, "export tiene que calzar EXACTO con la respuesta RPC real");
}

#[test]
fn a_db_export_call_missing_the_output_path_is_a_clean_usage_error() {
    let temp = TempDir::new("missing-arg");
    let src = temp.write("app.link", PROGRAM);
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("db").arg("export").arg(&src).output().expect("ejecutar linkc db export");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("uso: linkc db export"), "{stderr}");
}

#[test]
fn an_unreachable_postgres_url_is_reported_as_a_clean_error_not_a_hang_or_panic() {
    let temp = TempDir::new("bad-postgres");
    let src = temp.write("app.link", PROGRAM);
    let out_path = temp.path("export.json");
    let start = std::time::Instant::now();
    let (success, stdout, stderr) =
        run_export(&[src.to_str().unwrap(), out_path.to_str().unwrap(), "--db", "postgres://user:pass@127.0.0.1:1/db"]);
    assert!(!success, "stdout: {stdout}\nstderr: {stderr}");
    assert!(start.elapsed() < Duration::from_secs(20), "no debe colgarse esperando una conexión que nunca va a llegar");
    assert!(!stdout.contains("panicked at") && !stderr.contains("panicked at"), "stdout: {stdout}\nstderr: {stderr}");
    assert!(stderr.contains("no se pudo exportar"), "{stderr}");
    assert!(!out_path.exists(), "no se debe escribir el archivo de salida si la exportación falló");
}
