// Tests de integración para `@readReplica` -- réplicas de lectura (PLAN.md
// §9.22 ítem 1, GRAMMAR.md §3.260).
//
// El enrutamiento real contra una segunda conexión Postgres vive en
// `compiler/tests/pg_integration.rs` (necesita `LINK_TEST_PG_URL`). Este
// archivo cubre lo que SÍ es chequeable sin Postgres real: el rechazo de
// escritura en compilación, el fallback al backend primario sobre SQLite
// (sin ninguna réplica configurada, que es el caso común de dev/test), y
// que `--read-replica-url` con una base primaria SQLite falla limpio al
// arrancar -- nunca un panic, nunca un servidor que arranca a medias.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-read-replica-{name}-{}-{}",
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

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

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

const READ_REPLICA_PROGRAM: &str = r#"
type Task = { id: Int, done: Bool }
db { tasks: Task[] }
service Tasks {
    @readReplica
    rpc list() -> Task[] { db.tasks.all() }

    rpc create() -> Task { db.tasks.insert(Task { id: 0, done: false }) }
}
"#;

#[test]
fn read_replica_falls_back_to_primary_over_sqlite_without_the_flag() {
    // Caso común de dev/test: `@readReplica` sin `--read-replica-url`
    // configurado no rompe nada -- degrada al backend primario, mismo
    // criterio de "nunca un servidor que no arranca" que `@cache`/
    // `@rate_limit` distribuidos.
    let temp = TempDir::new("fallback");
    let link_path = temp.write("app.link", READ_REPLICA_PROGRAM);
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

    let (status, body) = rpc_status_and_body(port, "Tasks/create", "{}");
    assert_eq!(status, 200, "body: {body}");

    let (status, body) = rpc_status_and_body(port, "Tasks/list", "{}");
    assert_eq!(status, 200, "body: {body}");
    let list: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1, "{body}");

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn read_replica_url_with_a_sqlite_primary_is_rejected_at_startup_not_a_panic() {
    let temp = TempDir::new("sqlite-rejects-replica");
    let link_path = temp.write("app.link", READ_REPLICA_PROGRAM);
    let db_path = temp.0.join("app.db");
    let port = free_port();

    let output = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&link_path)
        .arg(port.to_string())
        .arg("--db")
        .arg(&db_path)
        .arg("--read-replica-url")
        .arg("postgres://user:pass@127.0.0.1:5999/nope")
        .output()
        .expect("ejecutar linkc serve");

    assert!(!output.status.success(), "tiene que rechazar el arranque, no arrancar a medias");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("panicked"), "tiene que ser un error limpio, no un panic: {stderr}");
    assert!(stderr.contains("read-replica-url") || stderr.contains("SQLite"), "mensaje inesperado: {stderr}");
}

#[test]
fn read_replica_rejects_a_write_method_inside_the_body_over_the_real_binary() {
    let temp = TempDir::new("rejects-write");
    let src = r#"
type Task = { id: Int, done: Bool }
db { tasks: Task[] }
service Tasks {
    @readReplica
    rpc create() -> Task { db.tasks.insert(Task { id: 0, done: false }) }
}
"#;
    let link_path = temp.write("app.link", src);
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("test").arg(&link_path).output().expect("linkc test");
    assert!(!out.status.success(), "un insert dentro de un rpc @readReplica no debería compilar");
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(text.contains("insert") && text.contains("readReplica"), "mensaje inesperado: {text}");
}
