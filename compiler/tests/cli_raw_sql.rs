// `@rawSql` / `db.query` / `db.execute` (GRAMMAR.md §3.283, PLAN.md §9.24.5
// ítem 2 -- la escapatoria de SQL crudo). Estos tests corren contra el
// backend SQLite real (el que usa `linkc serve` por default sin `--db`);
// el equivalente contra Postgres real -- que además prueba una window
// function, exclusiva de ese motor -- vive en
// `raw_sql_query_and_execute_work_against_real_postgres_including_a_window_function`
// en `pg_integration.rs` (necesita `LINK_TEST_PG_URL`).
//
// Lo que un test de checker (`checker.rs`) NO puede probar: que el
// placeholder `$1` realmente se traduzca a la sintaxis de SQLite, que
// `db.execute` empuje una escritura de verdad, y sobre todo que la conexión
// de solo-lectura de `db.query` (`with_reader`, ver `Db::raw_sql_query` en
// runtime/db.rs) rechace una escritura contra el motor real -- no solo que
// el checker la deje pasar o no según la anotación.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

struct TempDir(PathBuf);

static TEMP_DIR_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl TempDir {
    fn new(name: &str) -> Self {
        let n = TEMP_DIR_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "linkc-rawsql-{name}-{}-{}-{n}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
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

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn wait_ready(port: u16) {
    for _ in 0..200 {
        if ureq::get(&format!("http://127.0.0.1:{port}/live")).call().is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
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

fn rpc_json(port: u16, method: &str, body: &str) -> serde_json::Value {
    let (status, text) = rpc_status_and_body(port, method, body);
    assert_eq!(status, 200, "{method} devolvió {status}: {text}");
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{method} no devolvió JSON ({e}): {text}"))
}

const PROGRAM: &str = r#"
type Item = { id: Int, name: String, price: Float }
db { items: Item[] }

type Row = { id: Int, name: String, price: Float }

service S {
    rpc seed() -> Void {
        db.items.insert(Item { id: 0, name: "widget", price: 9.99 });
        db.items.insert(Item { id: 0, name: "gadget", price: 19.99 });
    }

    @rawSql
    rpc listByMinPrice(minPrice: Float) -> Row[] {
        db.query("SELECT id, name, price FROM items WHERE price >= $1 ORDER BY id", [minPrice])
    }

    @rawSql
    rpc bumpPrice(id: Int, delta: Float) -> Int {
        db.execute("UPDATE items SET price = price + $1 WHERE id = $2", [delta, id])
    }

    @rawSql
    rpc attemptWriteViaQuery(id: Int) -> Row[] {
        db.query("DELETE FROM items WHERE id = $1", [id])
    }
}
"#;

fn start_server(temp: &TempDir) -> (std::process::Child, u16) {
    let link_path = temp.write("app.link", PROGRAM);
    let db_path = temp.0.join("app.db");
    let port = free_port();
    let child = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&link_path)
        .arg(port.to_string())
        .arg("--db")
        .arg(&db_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn linkc serve");
    wait_ready(port);
    (child, port)
}

#[test]
fn db_query_selects_rows_with_positional_params_against_real_sqlite() {
    let temp = TempDir::new("select");
    let (mut child, port) = start_server(&temp);

    rpc_json(port, "S/seed", "{}");
    let rows = rpc_json(port, "S/listByMinPrice", r#"{"minPrice": 10.0}"#);
    let rows = rows.as_array().expect("listByMinPrice devuelve una lista");
    assert_eq!(rows.len(), 1, "solo el gadget (19.99) pasa el filtro >= 10: {rows:?}");
    assert_eq!(rows[0]["name"], "gadget");
    assert_eq!(rows[0]["price"], 19.99);

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn db_execute_updates_and_returns_the_affected_row_count_against_real_sqlite() {
    let temp = TempDir::new("execute");
    let (mut child, port) = start_server(&temp);

    rpc_json(port, "S/seed", "{}");
    let all = rpc_json(port, "S/listByMinPrice", r#"{"minPrice": 0}"#);
    let id = all.as_array().unwrap()[0]["id"].as_i64().unwrap();

    let affected = rpc_json(port, "S/bumpPrice", &format!(r#"{{"id": {id}, "delta": 5.0}}"#));
    assert_eq!(affected, 1, "el UPDATE afecta exactamente una fila");

    let after = rpc_json(port, "S/listByMinPrice", r#"{"minPrice": 0}"#);
    let after = after.as_array().unwrap();
    let bumped = after.iter().find(|r| r["id"] == id).expect("la fila sigue estando");
    assert_eq!(bumped["price"], 14.99, "9.99 + 5: {bumped:?}");

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn db_query_read_only_connection_rejects_a_write_against_real_sqlite() {
    let temp = TempDir::new("readonly");
    let (mut child, port) = start_server(&temp);

    rpc_json(port, "S/seed", "{}");
    let all = rpc_json(port, "S/listByMinPrice", r#"{"minPrice": 0}"#);
    let before_count = all.as_array().unwrap().len();
    let id = all.as_array().unwrap()[0]["id"].as_i64().unwrap();

    let (status, body) = rpc_status_and_body(port, "S/attemptWriteViaQuery", &format!(r#"{{"id": {id}}}"#));
    assert_ne!(status, 200, "un DELETE vía db.query tiene que fallar -- la conexión es de solo lectura: {body}");
    assert!(
        body.to_lowercase().contains("readonly") || body.to_lowercase().contains("read-only") || body.to_lowercase().contains("read only"),
        "el error tiene que dejar claro que la conexión es de solo lectura: {body}"
    );

    let after = rpc_json(port, "S/listByMinPrice", r#"{"minPrice": 0}"#);
    assert_eq!(after.as_array().unwrap().len(), before_count, "el DELETE rechazado no borró nada: {after:?}");

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn db_query_outside_at_raw_sql_is_rejected_at_build_time() {
    let temp = TempDir::new("gate");
    let link_path = temp.write(
        "app.link",
        r#"
type Row = { n: Int }
db { items: Row[] }
service S {
    rpc leak() -> Row[] { db.query("SELECT 1 AS n", []) }
}
"#,
    );
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("build").arg(&link_path).arg(temp.0.join("gen")).output().expect("ejecutar linkc build");
    assert!(!out.status.success(), "db.query sin @rawSql tiene que rechazar la compilación");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("rawSql") || stderr.contains("raw_sql") || stderr.contains("@rawSql"), "mensaje inesperado: {stderr}");
}
