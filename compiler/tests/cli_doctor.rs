// `linkc doctor` (GRAMMAR.md §3.100): diagnóstico de entorno antes de un
// despliegue -- versión, que el `.link` de entrada resuelva/parse/tipe,
// permiso de escritura en su directorio, y conectividad de solo lectura a la
// base configurada. Se prueba contra el BINARIO real: que el código compile
// no prueba que el exit code sea el correcto, ni que un `--db` que apunta a
// un puerto cerrado se reporte como error de conectividad en vez de colgarse
// o entrar en pánico.

use std::path::PathBuf;
use std::process::Command;

const PROGRAM: &str = r#"
type Item = { id: Int, name: String }
db { items: Item[] }
service Sys {
  rpc ping() -> String { "pong" }
}
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "linkc-doctor-{name}-{}-{}",
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
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run_doctor(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("doctor").args(args).output().expect("ejecutar linkc doctor");
    (out.status.success(), String::from_utf8_lossy(&out.stdout).to_string(), String::from_utf8_lossy(&out.stderr).to_string())
}

#[test]
fn a_valid_link_file_with_no_db_flag_passes_every_check() {
    let temp = TempDir::new("valid-sqlite");
    let src = temp.write("app.link", PROGRAM);
    let (success, stdout, stderr) = run_doctor(&[src.to_str().unwrap()]);
    assert!(success, "stdout: {stdout}\nstderr: {stderr}");
    assert!(stdout.contains("[OK]    versión de linkc:"), "{stdout}");
    assert!(stdout.contains("parsea y tipa correctamente"), "{stdout}");
    assert!(stdout.contains("permiso de escritura"), "{stdout}");
    assert!(stdout.contains("SQLite embebido"), "{stdout}");
    assert!(!stdout.contains("[ERROR]"), "{stdout}");
    assert!(stdout.contains("0 error(es)"), "{stdout}");
}

#[test]
fn a_missing_file_is_reported_as_an_error_but_the_other_checks_still_run() {
    let temp = TempDir::new("missing-file");
    let missing = temp.0.join("does_not_exist.link");
    let (success, stdout, _stderr) = run_doctor(&[missing.to_str().unwrap()]);
    assert!(!success);
    assert!(stdout.contains("[ERROR]"), "{stdout}");
    // Los otros chequeos (versión, permisos, DB) no dependen de que el
    // archivo exista -- deben seguir corriendo e imprimirse igual.
    assert!(stdout.contains("versión de linkc"), "{stdout}");
    assert!(stdout.contains("permiso de escritura"), "{stdout}");
    assert!(stdout.contains("SQLite embebido"), "{stdout}");
}

#[test]
fn a_syntax_error_fails_the_parse_check_and_prints_the_real_diagnostic() {
    let temp = TempDir::new("syntax-error");
    let src = temp.write("broken.link", "this is not a valid program at all");
    let (success, stdout, stderr) = run_doctor(&[src.to_str().unwrap()]);
    assert!(!success);
    assert!(stdout.contains("[ERROR]"), "{stdout}");
    assert!(stderr.contains("se esperaba"), "{stderr}");
}

#[test]
fn an_unreachable_postgres_url_is_reported_as_a_connectivity_error_not_a_hang_or_panic() {
    let temp = TempDir::new("bad-postgres");
    let src = temp.write("app.link", PROGRAM);
    let start = std::time::Instant::now();
    // Puerto 1 en loopback: connection refused casi instantáneo en
    // cualquier SO, sin necesitar una base real levantada para este caso.
    let (success, stdout, stderr) = run_doctor(&[src.to_str().unwrap(), "--db", "postgres://user:pass@127.0.0.1:1/db"]);
    assert!(!success, "stdout: {stdout}\nstderr: {stderr}");
    assert!(start.elapsed() < std::time::Duration::from_secs(20), "no debe colgarse esperando una conexión que nunca va a llegar");
    assert!(stdout.contains("[ERROR] conectividad a PostgreSQL"), "{stdout}");
    assert!(!stdout.contains("panicked at"), "{stdout}");
    assert!(!stderr.contains("panicked at"), "{stderr}");
    // La URL se muestra para diagnóstico, pero la credencial nunca en texto plano.
    assert!(stdout.contains("postgres://***@127.0.0.1:1/db"), "{stdout}");
    assert!(!stdout.contains("user:pass"), "{stdout}");
}

#[test]
fn a_malformed_postgres_url_is_a_clean_error_not_a_panic() {
    let temp = TempDir::new("malformed-url");
    let src = temp.write("app.link", PROGRAM);
    let (success, stdout, stderr) = run_doctor(&[src.to_str().unwrap(), "--db", "postgres://not a valid url"]);
    assert!(!success, "stdout: {stdout}\nstderr: {stderr}");
    assert!(stdout.contains("[ERROR] conectividad a PostgreSQL"), "{stdout}");
    assert!(!stdout.contains("panicked at"), "{stdout}");
}

#[test]
fn a_doctor_call_with_no_arguments_is_a_clean_usage_error() {
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("doctor").output().expect("ejecutar linkc doctor");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("uso: linkc doctor"), "{stderr}");
}

#[test]
fn link_database_url_env_var_is_honored_the_same_as_the_flag() {
    let temp = TempDir::new("env-var");
    let src = temp.write("app.link", PROGRAM);
    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("doctor")
        .arg(&src)
        .env("LINK_DATABASE_URL", "postgres://user:pass@127.0.0.1:1/db")
        .output()
        .expect("ejecutar linkc doctor");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success());
    assert!(stdout.contains("PostgreSQL configurada"), "{stdout}");
    assert!(stdout.contains("[ERROR] conectividad a PostgreSQL"), "{stdout}");
}
