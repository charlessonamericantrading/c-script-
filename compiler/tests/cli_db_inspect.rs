// `linkc db inspect` (GRAMMAR.md §3.175): lista cada colección declarada
// con su estado FÍSICO real -- existe o no, cuántas filas -- SIN ejecutar
// ningún DDL. Se prueba contra el BINARIO real: que compile no prueba que
// distinga "tabla inexistente" de "tabla vacía", ni que cuente filas reales
// después de que otro proceso (`linkc serve`) las insertó de verdad.

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

const PROGRAM: &str = r#"
type Item = { id: Int, name: String }
type Order = { id: Int, itemId: Int, @softDelete deletedAt: Timestamp? = null }
db { items: Item[], orders: Order[] }
service Items {
  rpc add(name: String) -> Item { db.items.insert(Item { id: 0, name: name }) }
}
service Orders {
  rpc add(itemId: Int) -> Order { db.orders.insert(Order { id: 0, itemId: itemId }) }
  rpc remove(id: Int) -> Bool { db.orders.delete(id) }
}
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "linkc-db-inspect-{name}-{}-{}",
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

    fn rpc(&self, method: &str, body: &str) -> serde_json::Value {
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

fn run_inspect(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("db").arg("inspect").args(args).output().expect("ejecutar linkc db inspect");
    (out.status.success(), String::from_utf8_lossy(&out.stdout).to_string(), String::from_utf8_lossy(&out.stderr).to_string())
}

#[test]
fn a_database_that_does_not_exist_yet_reports_every_collection_as_not_created() {
    let temp = TempDir::new("fresh");
    let src = temp.write("app.link", PROGRAM);
    let (success, stdout, stderr) = run_inspect(&[src.to_str().unwrap()]);
    assert!(success, "stdout: {stdout}\nstderr: {stderr}");
    assert!(stdout.contains("items"), "{stdout}");
    assert!(stdout.contains("orders"), "{stdout}");
    assert!(stdout.contains("no existe todavía"), "{stdout}");
    assert!(stdout.contains("2 colección(es) declaradas, 2 sin crear todavía, 0 fila(s) en total"), "{stdout}");
}

/// El caso real: una base ya poblada por `linkc serve` de verdad, no un
/// archivo `.db` armado a mano -- confirma que `db inspect` lee filas REALES,
/// y que `@softDelete` NO se filtra (mismo criterio que `db.tableStats()`,
/// GRAMMAR.md §3.151: conteo FÍSICO, distinto de `count()`).
#[test]
fn a_real_database_populated_by_linkc_serve_reports_real_row_counts_including_soft_deleted_rows() {
    let temp = TempDir::new("populated");
    let src = temp.write("app.link", PROGRAM);
    let db_path = temp.path("app.db");

    let server = Serve::start(&src, &db_path);
    server.rpc("Items/add", r#"{"name":"a"}"#);
    server.rpc("Items/add", r#"{"name":"b"}"#);
    let created = server.rpc("Orders/add", r#"{"itemId":1}"#);
    let id = created["id"].as_i64().expect("id");
    server.rpc("Orders/remove", &format!(r#"{{"id":{id}}}"#));
    drop(server);

    let (success, stdout, stderr) = run_inspect(&[src.to_str().unwrap(), "--db", db_path.to_str().unwrap()]);
    assert!(success, "stdout: {stdout}\nstderr: {stderr}");
    assert!(stdout.contains("items") && stdout.contains("2 fila(s)"), "{stdout}");
    // La fila borrada (soft-delete) sigue existiendo FÍSICAMENTE -- el
    // conteo tiene que seguir siendo 1, no 0.
    assert!(stdout.contains("orders") && stdout.contains("1 fila(s)"), "{stdout}");
    assert!(stdout.contains("2 colección(es) declaradas, 0 sin crear todavía, 3 fila(s) en total"), "{stdout}");
}

#[test]
fn a_db_inspect_call_with_no_arguments_is_a_clean_usage_error() {
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("db").arg("inspect").output().expect("ejecutar linkc db inspect");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("uso: linkc db inspect"), "{stderr}");
}

#[test]
fn an_unknown_db_sub_subcommand_is_a_clean_usage_error() {
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("db").arg("shell").output().expect("ejecutar linkc db shell");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("uso: linkc db inspect"), "{stderr}");
}

#[test]
fn an_unreachable_postgres_url_is_reported_as_a_clean_error_not_a_hang_or_panic() {
    let temp = TempDir::new("bad-postgres");
    let src = temp.write("app.link", PROGRAM);
    let start = std::time::Instant::now();
    let (success, stdout, stderr) = run_inspect(&[src.to_str().unwrap(), "--db", "postgres://user:pass@127.0.0.1:1/db"]);
    assert!(!success, "stdout: {stdout}\nstderr: {stderr}");
    assert!(start.elapsed() < std::time::Duration::from_secs(20), "no debe colgarse esperando una conexión que nunca va a llegar");
    assert!(!stdout.contains("panicked at") && !stderr.contains("panicked at"), "stdout: {stdout}\nstderr: {stderr}");
    assert!(stderr.contains("no se pudo inspeccionar"), "{stderr}");
}
