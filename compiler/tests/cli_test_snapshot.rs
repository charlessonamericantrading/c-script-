// Test de integración: `linkc test` como SUBPROCESO REAL contra el binario
// compilado (PLAN.md §5, "tests de contrato -- que el .d.ts generado no
// rompa sin querer"). Cubre las cuatro ramas reales del comando: primera
// corrida (crea el snapshot), corrida sin cambios (OK), corrida con un
// cambio real en el programa (falla y el diff muestra la línea que cambió),
// y `--update` (acepta el cambio como la nueva base).

use std::io::Write;
use std::process::{Command, Stdio};

struct TempDir(std::path::PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("cscript-cli-test-snapshot-{tag}-{}", std::process::id()));
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

fn run_test(entry: &std::path::Path, snap: &std::path::Path, extra: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("test")
        .arg(entry)
        .arg(snap)
        .args(extra)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("no se pudo iniciar 'linkc test'")
}

#[test]
fn first_run_creates_the_snapshot_and_succeeds() {
    let project = TempDir::new("first-run");
    let entry = project.write("a.link", "type Point = { x: Int, y: Int }\nfn origin() -> Point { Point { x: 0, y: 0 } }\n");
    let snap = project.0.join("a.snap");

    assert!(!snap.exists());
    let output = run_test(&entry, &snap, &[]);
    assert!(
        output.status.success(),
        "la primera corrida debe crear el snapshot y salir OK -- stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(snap.exists(), "linkc test debe haber escrito el archivo .snap");
    let content = std::fs::read_to_string(&snap).unwrap();
    assert!(content.contains("=== contract.d.ts ==="));
    assert!(content.contains("Point"), "el snapshot debe contener el contrato real emitido");
}

#[test]
fn unchanged_program_matches_the_existing_snapshot() {
    let project = TempDir::new("unchanged");
    let entry = project.write("a.link", "type Point = { x: Int, y: Int }\nfn origin() -> Point { Point { x: 0, y: 0 } }\n");
    let snap = project.0.join("a.snap");

    let first = run_test(&entry, &snap, &[]);
    assert!(first.status.success());
    let snapshot_after_first_run = std::fs::read_to_string(&snap).unwrap();

    let second = run_test(&entry, &snap, &[]);
    assert!(
        second.status.success(),
        "una segunda corrida sin cambios debe seguir saliendo OK -- stderr: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(std::fs::read_to_string(&snap).unwrap(), snapshot_after_first_run, "no debe reescribir el snapshot si no cambió nada");
    assert!(String::from_utf8_lossy(&second.stdout).contains("OK"));
}

#[test]
fn changed_contract_fails_and_shows_the_real_diff() {
    let project = TempDir::new("changed");
    let entry = project.write("a.link", "type Point = { x: Int, y: Int }\nfn origin() -> Point { Point { x: 0, y: 0 } }\n");
    let snap = project.0.join("a.snap");
    assert!(run_test(&entry, &snap, &[]).status.success());

    // Cambio real y observable: renombrar un campo del struct público.
    project.write("a.link", "type Point = { renamed: Int, y: Int }\nfn origin() -> Point { Point { renamed: 0, y: 0 } }\n");

    let output = run_test(&entry, &snap, &[]);
    assert!(!output.status.success(), "un contrato distinto al snapshot debe fallar, no pasar en silencio");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("CAMBIÓ"), "el mensaje debe decir explícitamente que el contrato cambió: {stderr}");
    assert!(stderr.contains("renamed"), "el diff debe mostrar el campo nuevo real: {stderr}");
    assert!(stderr.contains("--update"), "el mensaje debe indicar cómo aceptar el cambio a propósito: {stderr}");
}

#[test]
fn update_flag_accepts_the_new_contract_as_the_baseline() {
    let project = TempDir::new("update-flag");
    let entry = project.write("a.link", "type Point = { x: Int, y: Int }\nfn origin() -> Point { Point { x: 0, y: 0 } }\n");
    let snap = project.0.join("a.snap");
    assert!(run_test(&entry, &snap, &[]).status.success());

    project.write("a.link", "type Point = { renamed: Int, y: Int }\nfn origin() -> Point { Point { renamed: 0, y: 0 } }\n");
    assert!(!run_test(&entry, &snap, &[]).status.success(), "sin --update, un cambio real debe seguir fallando");

    let updated = run_test(&entry, &snap, &["--update"]);
    assert!(
        updated.status.success(),
        "--update debe aceptar el nuevo contrato y salir OK -- stderr: {}",
        String::from_utf8_lossy(&updated.stderr)
    );
    let content = std::fs::read_to_string(&snap).unwrap();
    assert!(content.contains("renamed"), "el snapshot actualizado debe reflejar el contrato nuevo");

    // Y ahora vuelve a matchear sin --update, porque el snapshot ya es la nueva base.
    assert!(run_test(&entry, &snap, &[]).status.success());
}
