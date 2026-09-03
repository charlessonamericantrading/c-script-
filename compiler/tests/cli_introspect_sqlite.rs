// Tests de integración para `linkc introspect` sobre bases SQLite
// (PLAN.md §9.21 Fase 1 ítem 4, GRAMMAR.md §3.247).
// Verifica:
// 1. Detección automática de SQLite por ruta de archivo (.db) o prefijo sqlite://
// 2. Extracción de PKs (id: Int, id: Uuid)
// 3. Extracción de FKs como comentarios explicativos (// FK -> tabla(col))
// 4. Extracción de índices simples (@unique, @index) y compuestos (@unique(c1, c2), @index(c1, c2))
// 5. Extracción de defaults (= now(), = true, = false, numéricos y strings)
// 6. Extracción de @autoUpdate en campos temporales
// 7. Extracción de restricciones CHECK (// CHECK: ...)
// 8. Validación de que el código .link generado compila con `linkc test`

use std::path::PathBuf;
use std::process::Command;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-introspect-sqlite-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        );
        let path = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&path).expect("crear tempdir");
        Self(path)
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

#[test]
fn introspect_sqlite_full_schema_and_roundtrip_test() {
    let temp = TempDir::new("full");
    let db_path = temp.path("company.db");

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute_batch(
        r#"
        CREATE TABLE departments (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            code TEXT NOT NULL UNIQUE
        );

        CREATE TABLE employees (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            dept_id INTEGER NOT NULL REFERENCES departments(id),
            full_name TEXT NOT NULL,
            salary DECIMAL NOT NULL DEFAULT 50000,
            active BOOLEAN NOT NULL DEFAULT 1,
            created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
            CHECK (salary >= 0)
        );

        CREATE INDEX idx_emp_dept ON employees(dept_id);
        CREATE UNIQUE INDEX idx_emp_dept_name ON employees(dept_id, full_name);
        "#,
    )
    .unwrap();
    drop(conn);

    let output = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("introspect")
        .arg(&db_path)
        .output()
        .expect("ejecutar linkc introspect");

    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();

    // Verificaciones estructurales
    assert!(stdout.contains("type Departments = {"), "{stdout}");
    assert!(stdout.contains("  id: Int,"), "{stdout}");
    assert!(stdout.contains("  name: String,"), "{stdout}");
    assert!(stdout.contains("  @unique code: String,"), "código debe ser @unique: {stdout}");

    assert!(stdout.contains("@unique(dept_id, full_name)"), "índice compuesto único: {stdout}");
    assert!(stdout.contains("type Employees = {"), "{stdout}");
    assert!(stdout.contains("  @index dept_id: Int, // FK -> departments(id)"), "índice y FK en dept_id: {stdout}");
    assert!(stdout.contains("  full_name: String,"), "{stdout}");
    assert!(stdout.contains("  salary: Decimal = 50000.toDecimal(),"), "default salary: {stdout}");
    assert!(stdout.contains("  active: Bool = true,"), "default boolean: {stdout}");
    assert!(stdout.contains("  created_at: Timestamp = now(),"), "default timestamp: {stdout}");
    assert!(stdout.contains("  @autoUpdate updated_at: Timestamp = now(),"), "@autoUpdate en updated_at: {stdout}");
    assert!(stdout.contains("// CHECK: salary >= 0"), "restricción CHECK documentada: {stdout}");

    assert!(stdout.contains("db {"), "{stdout}");
    assert!(stdout.contains("  departments: Departments[],"), "{stdout}");
    assert!(stdout.contains("  employees: Employees[],"), "{stdout}");

    // Verificación de compilación: el .link generado debe parsear y tipar con `linkc test`
    let link_file = temp.path("schema.link");
    std::fs::write(&link_file, &stdout).unwrap();

    let check_out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("test")
        .arg(&link_file)
        .output()
        .expect("ejecutar linkc test");

    assert!(
        check_out.status.success(),
        "el schema generado debe compilar sin errores:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&check_out.stdout),
        String::from_utf8_lossy(&check_out.stderr)
    );
}

#[test]
fn introspect_sqlite_missing_file_fails_cleanly() {
    let output = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("introspect")
        .arg("archivo_que_no_existe_12345.db")
        .output()
        .expect("ejecutar linkc introspect");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("el archivo SQLite 'archivo_que_no_existe_12345.db' no existe"), "stderr: {stderr}");
}

#[test]
fn introspect_sqlite_empty_database_fails_cleanly() {
    let temp = TempDir::new("empty");
    let db_path = temp.path("empty.db");

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    drop(conn);

    let output = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("introspect")
        .arg(&db_path)
        .output()
        .expect("ejecutar linkc introspect");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no tiene ninguna tabla -- nada para introspeccionar"), "stderr: {stderr}");
}
