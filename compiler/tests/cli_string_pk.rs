// Tests de integración para `id: String` -- tercera forma de PK, pensada
// para adoptar una tabla existente cuya columna id es VARCHAR/TEXT en vez
// del tipo nativo `uuid` (PLAN.md §9.21 Fase 3 ítem 11, GRAMMAR.md §3.251).

use std::path::PathBuf;
use std::process::Command;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-strpk-{name}-{}-{}",
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

const PROGRAM: &str = r#"
type Session = {
  id: String,
  token: String,
}

db {
  sessions: Session[],
}

service Auth {
  rpc create(token: String) -> Session {
    db.sessions.insert(Session { id: "unused", token: token })
  }

  rpc byId(id: String) -> Session? {
    db.sessions.find(id)
  }

  rpc remove(id: String) -> Bool {
    db.sessions.delete(id)
  }

  rpc idOnly() -> String[] {
    db.sessions.select(|s: Session| { s.id })
  }
}
"#;

#[test]
fn string_pk_full_crud_cycle_generates_a_uuid_shaped_id() {
    let source = format!(
        "{PROGRAM}\n{}",
        r#"
test "id: String genera un uuid y funciona en el ciclo completo" {
  let s1 = Auth.create("tok-a");
  let s2 = Auth.create("tok-b");
  assert(s1.id != s2.id, "ids distintos generados");
  assert(s1.id.length() == 36, "forma de uuid (36 caracteres)");

  let found = Auth.byId(s1.id);
  assert(found.isSome(), "encontrado por id string");
  match found {
    s: Session => assert(s.token == "tok-a", "token correcto"),
    null => assert(false, "unreachable"),
  }

  let ids = Auth.idOnly();
  assert(ids.length() == 2, "select proyecta la columna id como String");

  let removed = Auth.remove(s1.id);
  assert(removed, "borrado exitoso");
  assert(Auth.byId(s1.id).isNone(), "ya no existe");
}
"#
    );
    let (ok, out) = run_link_tests(&source);
    assert!(ok, "test falló: {out}");
}

#[test]
fn string_pk_rejects_page_after() {
    let bad_program = r#"
type Session = { id: String, token: String }
db { sessions: Session[] }
service Auth {
  rpc list(cursor: Int?, limit: Int) -> Session[] {
    db.sessions.pageAfter(cursor, limit)
  }
}
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: pageAfter no soportado sobre id: String: {out}");
    assert!(out.contains("'pageAfter' no está soportado sobre una colección con 'id: String'"), "{out}");
}

#[test]
fn string_pk_generated_postgres_ddl_uses_varchar_not_native_uuid() {
    let temp = TempDir::new("build");
    let src = temp.write("app.link", PROGRAM);
    let gen_dir = temp.0.join("gen");
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("build").arg(&src).arg(&gen_dir).output().expect("linkc build");
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "linkc build falló: {text}");

    let schema = std::fs::read_to_string(gen_dir.join("schema.postgres.sql")).expect("leer schema.postgres.sql");
    assert!(schema.contains("\"id\" VARCHAR PRIMARY KEY"), "id: String tiene que emitir VARCHAR, no el tipo nativo uuid: {schema}");
    assert!(!schema.contains("UUID PRIMARY KEY"), "id: String NUNCA debe emitir el tipo nativo UUID: {schema}");
}

#[test]
fn string_pk_introspect_sqlite_maps_a_varchar_column_to_string() {
    let temp = TempDir::new("introspect");
    let db_path = temp.0.join("legacy.db");
    let conn = rusqlite::Connection::open(&db_path).expect("abrir sqlite");
    conn.execute_batch("CREATE TABLE users (id VARCHAR PRIMARY KEY, name TEXT NOT NULL);").expect("crear tabla legacy");
    drop(conn);

    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("introspect").arg(&db_path).output().expect("linkc introspect");
    assert!(out.status.success(), "introspect falló: {}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("id: String,"), "una PK VARCHAR tiene que mapear a 'id: String': {text}");
    assert!(!text.contains("id: Int,"), "{text}");
}

#[test]
fn string_pk_adopt_existing_reads_a_real_legacy_varchar_row_over_sqlite() {
    let temp = TempDir::new("adopt");
    let db_path = temp.0.join("legacy.db");
    let conn = rusqlite::Connection::open(&db_path).expect("abrir sqlite");
    conn.execute_batch("CREATE TABLE users (id VARCHAR PRIMARY KEY, name TEXT NOT NULL);").expect("crear tabla legacy");
    conn.execute("INSERT INTO users (id, name) VALUES ('legacy-uuid-123', 'Ada')", []).expect("insertar fila legacy");
    drop(conn);

    let program = r#"
type Users = {
  id: String,
  name: String,
}
db { users: Users[] }
service Svc {
  rpc list() -> Users[] {
    db.users.all()
  }
}
"#;
    let link_path = temp.write("app.link", program);

    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut child = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&link_path)
        .arg(port.to_string())
        .arg("--db")
        .arg(&db_path)
        .arg("--adopt-existing")
        .spawn()
        .expect("spawn linkc serve");

    let mut ready = false;
    for _ in 0..100 {
        if ureq::get(&format!("http://127.0.0.1:{port}/live")).call().is_ok() {
            ready = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(ready, "el servidor no levantó a tiempo");

    let resp = ureq::post(&format!("http://127.0.0.1:{port}/Svc/list"))
        .set("Content-Type", "application/json")
        .send_string("{}")
        .expect("Svc/list debe responder 200")
        .into_string()
        .unwrap();
    let val: serde_json::Value = serde_json::from_str(&resp).unwrap();
    let rows = val.as_array().unwrap();
    assert_eq!(rows.len(), 1, "la fila legacy se lee tal cual: {resp}");
    assert_eq!(rows[0]["id"], "legacy-uuid-123");
    assert_eq!(rows[0]["name"], "Ada");

    let _ = child.kill();
    let _ = child.wait();
}
