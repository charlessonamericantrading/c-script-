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
    run_linkc_test_with_args(entry, &[])
}

fn run_linkc_test_with_args(entry: &std::path::Path, extra_args: &[&str]) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_linkc"));
    cmd.arg("test").arg(entry);
    for a in extra_args {
        cmd.arg(a);
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).output().expect("no se pudo iniciar 'linkc test'")
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

// ---- `--filter <nombre>` (PLAN.md §9.7, GRAMMAR.md §3.82) ----

const THREE_TESTS: &str = r#"
test "crear usuario exitoso" {
    assert(true);
}

test "actualizar usuario exitoso" {
    assert(true);
}

test "borrar item" {
    assert(true);
}
"#;

#[test]
fn filter_runs_only_tests_whose_name_contains_the_substring() {
    let project = TempDir::new("filter-match");
    let entry = project.write("main.link", THREE_TESTS);

    let output = run_linkc_test_with_args(&entry, &["--filter", "usuario"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "stderr: {stderr}");
    assert!(stdout.contains("running 2 tests (filtro: 'usuario')"), "stdout: {stdout}");
    assert!(stdout.contains("test result: ok. 2 passed; 0 failed"), "stdout: {stdout}");
    assert!(!stdout.contains("borrar item"), "el test que no matchea no debería ni mencionarse: {stdout}");
}

#[test]
fn filter_matching_no_test_name_runs_zero_cleanly_not_an_error() {
    let project = TempDir::new("filter-empty");
    let entry = project.write("main.link", THREE_TESTS);

    let output = run_linkc_test_with_args(&entry, &["--filter", "esto-no-matchea-nada"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "un filtro que no matchea nada no debe ser un error: stderr: {stderr}");
    assert!(stdout.contains("running 0 tests (filtro: 'esto-no-matchea-nada')"), "stdout: {stdout}");
    assert!(stdout.contains("test result: ok. 0 tests run"), "stdout: {stdout}");
}

#[test]
fn filter_is_substring_not_exact_match() {
    // "usuario" (sin "exitoso") tiene que matchear los dos primeros tests
    // igual -- mismo criterio que `cargo test <substring>`, no un nombre
    // exacto.
    let project = TempDir::new("filter-substring");
    let entry = project.write("main.link", THREE_TESTS);

    let output = run_linkc_test_with_args(&entry, &["--filter", "crear"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("running 1 tests"), "stdout: {stdout}");
}

#[test]
fn filter_without_a_value_is_a_clean_cli_error() {
    let project = TempDir::new("filter-noval");
    let entry = project.write("main.link", THREE_TESTS);

    let output = run_linkc_test_with_args(&entry, &["--filter"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("--filter"), "stderr: {stderr}");
    assert!(!stderr.contains("panicked at"), "un flag mal usado es un error de uso, no un panic: {stderr}");
}

#[test]
fn filter_combined_with_a_snapshot_path_is_a_clean_cli_error() {
    // `--filter` solo tiene sentido contra los bloques `test "..."`
    // integrados -- combinarlo con el testing de contrato (que SÍ toma un
    // segundo path posicional, el snapshot) es un uso confuso, rechazado
    // en vez de ignorado en silencio.
    let project = TempDir::new("filter-snapshot");
    let entry = project.write("main.link", THREE_TESTS);
    let snap = project.0.join("main.snap");

    let output = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("test")
        .arg(&entry)
        .arg(&snap)
        .arg("--filter")
        .arg("usuario")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("no se pudo iniciar 'linkc test'");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--filter"), "stderr: {stderr}");
    assert!(!std::path::Path::new(&snap).exists(), "no debería haber llegado a crear ningún snapshot");
}

// ---- regresión: `while` real dentro de `test { }` (GRAMMAR.md §3.15) ----

#[test]
fn a_while_loop_that_finishes_in_a_handful_of_iterations_passes_inside_a_test_block() {
    // Bug real encontrado 26/08/2026 verificando un reporte de un colaborador:
    // `run_tests_core` inicializaba el contador compartido de iteraciones de
    // `while` en `Cell::new(1_000_000)` (el propio tope MAX_WHILE_ITERATIONS)
    // en vez de `Cell::new(0)` -- el camino de rpc normal (invoke_rpc_with_sessions)
    // sí arrancaba en 0. Efecto: CUALQUIER `while` que se ejecutara al menos
    // una vez dentro de un `test { }` fallaba de inmediato en su primera
    // iteración real con "límite de 1000000 iteraciones de 'while' excedido",
    // sin importar cuán corto fuera el loop -- el ejemplo canónico de §3.15
    // (sumar una lista con un `while`) nunca pasaba dentro de un test, aunque
    // el mismo código funcionaba perfecto invocado vía `serve`.
    let project = TempDir::new("while-in-test-block");
    let entry = project.write(
        "main.link",
        r#"
        test "while suma una lista sin colgarse" {
            let xs = [1, 2, 3, 4, 5];
            let mut total = 0;
            let mut i = 0;
            while i < xs.length() {
                total = total + xs[i];
                i = i + 1;
            }
            assert(total == 15, "la suma debe ser 15");
        }
        "#,
    );

    let output = run_linkc_test(&entry);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "stdout: {stdout} -- stderr: {stderr}");
    assert!(stdout.contains("test result: ok. 1 passed; 0 failed"), "stdout: {stdout}");
}

#[test]
fn a_genuinely_infinite_while_inside_a_test_block_still_hits_the_real_cap() {
    // La contracara del fix de arriba: no basta con que el contador arranque
    // en 0, tiene que seguir cortando un loop que de verdad nunca termina --
    // si no, el fix de la regresión anterior podría haberse hecho subiendo
    // el tope en vez de corrigiendo el punto de partida.
    let project = TempDir::new("while-true-in-test-block");
    let entry = project.write(
        "main.link",
        r#"
        test "while true real se corta" {
            let mut i = 0;
            while true {
                i = i + 1;
            }
            assert(i > 0);
        }
        "#,
    );

    let output = run_linkc_test(&entry);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(!output.status.success(), "un while true real no debe pasar nunca");
    assert!(
        stdout.contains("límite de 1000000 iteraciones de 'while' excedido"),
        "stdout: {stdout}"
    );
}
