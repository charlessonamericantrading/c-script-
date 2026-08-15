use std::io::Write;
use std::process::{Command, Stdio};

struct TempDir(std::path::PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("cscript-cli-test-runner-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
    fn write(&self, rel: &str, contents: &str) -> std::path::PathBuf {
        let path = self.0.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        path
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run_linkc_test(entry: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("test")
        .arg(entry)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("no se pudo iniciar 'linkc test'")
}

#[test]
fn test_runner_cli_runs_passing_tests_and_returns_success() {
    let project = TempDir::new("passing-tests");
    let entry = project.write(
        "main.link",
        r#"
        type User = { id: Int, name: String }
        db { users: User[] }

        service UsersService {
            rpc add(name: String) -> User {
                db.users.insert(User { id: 0, name: name })
            }
        }

        test "crear usuario exitoso" {
            let u = UsersService.add("Ada");
            assert(u.name == "Ada", "nombre coincide");
        }

        test "otra asercion simple" {
            assert(10 > 5);
        }
        "#,
    );

    let output = run_linkc_test(&entry);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "debe salir con exit code 0 -- stderr: {stderr}");
    assert!(stdout.contains("running 2 tests"), "stdout: {stdout}");
    assert!(stdout.contains("test result: ok. 2 passed; 0 failed"), "stdout: {stdout}");
}

#[test]
fn test_runner_cli_fails_when_assertion_fails() {
    let project = TempDir::new("failing-tests");
    let entry = project.write(
        "main.link",
        r#"
        test "test que pasa" {
            assert(true);
        }

        test "test que falla" {
            assert(false, "esperaba valor verdadero");
        }
        "#,
    );

    let output = run_linkc_test(&entry);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(!output.status.success(), "debe fallar con exit code no-cero");
    assert!(stdout.contains("running 2 tests"), "stdout: {stdout}");
    assert!(stdout.contains("test \"test que falla\" ... FAILED: asercion fallida: esperaba valor verdadero"), "stdout: {stdout}");
    assert!(stdout.contains("test result: FAILED. 1 passed; 1 failed"), "stdout: {stdout}");
}
