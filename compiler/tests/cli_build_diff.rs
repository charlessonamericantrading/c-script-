// Test de integración: `linkc build --diff <archivo-anterior>` como
// SUBPROCESO REAL contra el binario compilado (PLAN.md §9.3, GRAMMAR.md
// §3.79) -- "qué cambió en el contrato TypeScript generado entre dos
// versiones del .link, para revisión de PR". Reusa el mismo `diff_lines`
// (LCS) que `linkc test` ya tenía, pero se llama desde `linkc build` en vez
// de `linkc test`, y NUNCA hace fallar el build -- es puramente informativo.

use std::io::Write;
use std::process::{Command, Stdio};

struct TempDir(std::path::PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("cscript-cli-build-diff-{tag}-{}", std::process::id()));
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

fn run_build(args: &[&std::ffi::OsStr]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("build")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("no se pudo iniciar 'linkc build'")
}

#[test]
fn diff_against_an_earlier_contract_shows_exactly_the_added_lines() {
    let project = TempDir::new("added-field");
    let entry_v1 = project.write("a.link", "type Task = { id: Int, title: String }\nfn f() -> Task { Task { id: 1, title: \"x\" } }\n");
    let outdir_v1 = project.0.join("gen-v1");
    let out1 = run_build(&[entry_v1.as_os_str(), outdir_v1.as_os_str()]);
    assert!(out1.status.success(), "el primer build debe tener éxito -- stderr: {}", String::from_utf8_lossy(&out1.stderr));
    let old_contract = outdir_v1.join("contract.d.ts");
    assert!(old_contract.exists());

    let entry_v2 = project.write(
        "a.link",
        "type Task = { id: Int, title: String, priority: Int }\nfn f() -> Task { Task { id: 1, title: \"x\", priority: 1 } }\n",
    );
    let outdir_v2 = project.0.join("gen-v2");
    let out2 = run_build(&[
        entry_v2.as_os_str(),
        outdir_v2.as_os_str(),
        std::ffi::OsStr::new("--diff"),
        old_contract.as_os_str(),
    ]);
    assert!(out2.status.success(), "--diff nunca debe hacer fallar un build que por lo demás compiló bien");
    let stdout = String::from_utf8_lossy(&out2.stdout);
    assert!(stdout.contains("el contrato cambió"), "{stdout}");
    assert!(stdout.contains("+  priority: number;") || stdout.contains("+   priority: number;"), "{stdout}");
}

#[test]
fn diff_against_an_identical_contract_reports_no_change() {
    let project = TempDir::new("no-change");
    let entry = project.write("a.link", "type Task = { id: Int, title: String }\nfn f() -> Task { Task { id: 1, title: \"x\" } }\n");
    let outdir = project.0.join("gen");
    let out1 = run_build(&[entry.as_os_str(), outdir.as_os_str()]);
    assert!(out1.status.success());
    let contract = outdir.join("contract.d.ts");

    // Mismo programa, mismo outdir -- el contrato recién generado tiene que
    // coincidir EXACTO consigo mismo.
    let out2 = run_build(&[entry.as_os_str(), outdir.as_os_str(), std::ffi::OsStr::new("--diff"), contract.as_os_str()]);
    assert!(out2.status.success());
    let stdout = String::from_utf8_lossy(&out2.stdout);
    assert!(stdout.contains("el contrato no cambió"), "{stdout}");
}

#[test]
fn diff_against_a_missing_file_warns_but_still_succeeds() {
    let project = TempDir::new("missing-file");
    let entry = project.write("a.link", "type Task = { id: Int }\nfn f() -> Task { Task { id: 1 } }\n");
    let outdir = project.0.join("gen");
    let missing = project.0.join("no-existe.d.ts");

    let out = run_build(&[entry.as_os_str(), outdir.as_os_str(), std::ffi::OsStr::new("--diff"), missing.as_os_str()]);
    // El BUILD en sí tuvo éxito -- --diff es informativo, un archivo de
    // comparación inválido no debe tirar abajo un build que por lo demás
    // funcionó.
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no se pudo leer"), "{stderr}");
    assert!(outdir.join("contract.d.ts").exists(), "el build en sí debe haber generado los archivos igual");
}

#[test]
fn build_without_diff_still_works_exactly_as_before() {
    let project = TempDir::new("no-diff-flag");
    let entry = project.write("a.link", "type Task = { id: Int }\nfn f() -> Task { Task { id: 1 } }\n");
    let outdir = project.0.join("gen");
    let out = run_build(&[entry.as_os_str(), outdir.as_os_str()]);
    assert!(out.status.success());
    assert!(outdir.join("contract.d.ts").exists());
}
