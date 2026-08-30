// `--db-schema`/`LINK_DATABASE_SCHEMA` (GRAMMAR.md §3.193): validación de la
// forma del identificador y el rechazo contra un target SQLite -- las dos
// cosas fallan ANTES de intentar ninguna conexión real, así que se prueban
// acá sin necesitar Postgres. El resto (crear el schema, namespacing real
// entre dos programas, etc.) vive en `pg_integration.rs`.

use std::path::PathBuf;
use std::process::Command;

const PROGRAM: &str = r#"
type Item = { id: Int, name: String }
db { items: Item[] }
service Items {
  rpc add(name: String) -> Item { db.items.insert(Item { id: 0, name: name }) }
}
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "linkc-db-schema-{name}-{}-{}",
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

fn run_doctor(link_path: &PathBuf, extra_args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("doctor").arg(link_path).args(extra_args).output().expect("ejecutar linkc doctor");
    (out.status.success(), String::from_utf8_lossy(&out.stdout).to_string(), String::from_utf8_lossy(&out.stderr).to_string())
}

#[test]
fn an_invalid_schema_identifier_is_a_clean_cli_error() {
    let temp = TempDir::new("invalid-identifier");
    let src = temp.write("app.link", PROGRAM);
    // Empieza con un dígito -- no es un identificador SQL válido.
    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&src)
        .arg("0")
        .arg("--db-schema")
        .arg("9bad")
        .output()
        .expect("ejecutar linkc serve");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--db-schema") && stderr.contains("identificador"), "{stderr}");
}

#[test]
fn a_schema_name_with_sql_injection_shaped_input_is_rejected_cleanly() {
    let temp = TempDir::new("injection-shaped");
    let src = temp.write("app.link", PROGRAM);
    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&src)
        .arg("0")
        .arg("--db-schema")
        .arg("foo\"; DROP TABLE users; --")
        .output()
        .expect("ejecutar linkc serve");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--db-schema"), "{stderr}");
}

#[test]
fn db_schema_combined_with_a_sqlite_target_is_rejected_cleanly() {
    let temp = TempDir::new("sqlite-target");
    let src = temp.write("app.link", PROGRAM);
    // Sin --db, el target por default es SQLite -- --db-schema no debería
    // aceptarse en silencio como si tuviera efecto.
    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&src)
        .arg("0")
        .arg("--db")
        .arg(temp.path("app.db"))
        .arg("--db-schema")
        .arg("myschema")
        .output()
        .expect("ejecutar linkc serve");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--db-schema") && stderr.contains("SQLite"), "{stderr}");
}

#[test]
fn db_schema_without_a_db_flag_at_all_is_rejected_as_sqlite_too() {
    // Sin --db/LINK_DATABASE_URL, el default es SQLite -- mismo rechazo que
    // con --db apuntando explícitamente a un archivo.
    let temp = TempDir::new("no-db-flag");
    let src = temp.write("app.link", PROGRAM);
    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&src)
        .arg("0")
        .arg("--db-schema")
        .arg("myschema")
        .output()
        .expect("ejecutar linkc serve");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--db-schema") && stderr.contains("SQLite"), "{stderr}");
}

#[test]
fn doctor_reports_the_invalid_schema_as_an_error_line_without_crashing() {
    let temp = TempDir::new("doctor-invalid");
    let src = temp.write("app.link", PROGRAM);
    let (success, stdout, stderr) = run_doctor(&src, &["--db-schema", "9bad"]);
    // `doctor` nunca "crashea" -- reporta [ERROR] y sigue, el código de
    // salida refleja que hubo al menos un error real.
    assert!(!success, "stdout: {stdout}\nstderr: {stderr}");
    assert!(stdout.contains("[ERROR]") && stdout.contains("--db-schema"), "{stdout}");
}

#[test]
fn linkc_serve_all_rejects_db_schema_up_front() {
    let temp = TempDir::new("serve-all-rejects");
    temp.write("alpha.link", PROGRAM);
    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve-all")
        .arg(&temp.0)
        .arg("--port-base")
        .arg("0")
        .arg("--db-schema")
        .arg("myschema")
        .output()
        .expect("ejecutar linkc serve-all");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--db-schema") && stderr.contains("serve-all"), "{stderr}");
}
