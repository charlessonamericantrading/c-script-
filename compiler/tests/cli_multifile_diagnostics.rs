// Test de integración: `linkc <archivo>` como SUBPROCESO REAL, para la
// identidad de archivo en errores de tipos multi-archivo (GRAMMAR.md
// §3.21, "Not done yet" hasta esta ronda). Antes, `main.rs::
// report_check_errors` solo podía renderizar un snippet real cuando
// `touched.len() == 1` -- cualquier programa con imports caía al `Display`
// plano de `CheckError` para TODOS sus errores, sin línea ni columna,
// aunque el error estuviera perfectamente localizado. Ahora `CheckError`
// carga la identidad de archivo real (`checker::CheckError.file`,
// estampada por `check_program_full` a partir de `item_files` -- ver
// `modules::load_program_with_overlay`), así que el snippet se renderiza
// para CUALQUIER archivo tocado, no solo el de entrada.

use std::io::Write;
use std::process::{Command, Stdio};

struct TempDir(std::path::PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("cscript-cli-multifile-diag-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
    fn write(&self, rel: &str, contents: &str) -> std::path::PathBuf {
        let path = self.0.join(rel);
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

fn run_linkc_check(path: &std::path::Path) -> (bool, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .expect("no se pudo iniciar 'linkc'");
    (output.status.success(), String::from_utf8_lossy(&output.stderr).into_owned())
}

#[test]
fn a_type_error_in_the_entry_file_of_a_multifile_program_gets_a_real_snippet_over_a_real_subprocess() {
    let dir = TempDir::new("entry-file-error");
    dir.write("b.link", "type Unused = { n: Int }\n");
    let path_a = dir.write(
        "a.link",
        "import { Unused } from \"./b.link\";\ntype Point = { x: Int, y: Int }\nfn origin() -> Point { Point { x: \"nope\", y: 0 } }\n",
    );

    let (ok, stderr) = run_linkc_check(&path_a);
    assert!(!ok, "un programa con un error de tipos real debe fallar: {stderr}");
    assert!(stderr.contains("a.link"), "debe nombrar a.link, el archivo donde está el error: {stderr}");
    assert!(stderr.contains(":3:"), "debe dar la línea real (3) del error, no una posición degradada: {stderr}");
    assert!(stderr.contains('^'), "debe incluir un caret real, no solo el mensaje plano: {stderr}");
}

#[test]
fn a_type_error_in_an_imported_file_names_that_file_with_a_real_snippet_over_a_real_subprocess() {
    // El caso más difícil: el error está en b.link (importado), no en
    // a.link (el archivo de entrada que se le pasa a `linkc`) -- antes de
    // esta ronda, `touched.len() == 2` hacía caer esto al `Display` plano
    // SIN ninguna posición, ni siquiera nombrando el archivo. Ahora debe
    // nombrar b.link específicamente, con su propio snippet real.
    let dir = TempDir::new("imported-file-error");
    dir.write("b.link", "type Point = { x: Int, y: Int }\nfn bad() -> Point { Point { x: \"nope\", y: 0 } }\n");
    let path_a = dir.write("a.link", "import { Point } from \"./b.link\";\nfn origin() -> Point { Point { x: 0, y: 0 } }\n");

    let (ok, stderr) = run_linkc_check(&path_a);
    assert!(!ok, "un programa con un error de tipos real debe fallar: {stderr}");
    assert!(stderr.contains("b.link"), "debe nombrar b.link, el archivo IMPORTADO donde está el error real: {stderr}");
    assert!(!stderr.contains("a.link:"), "el error no está en a.link -- no debe atribuirse ahí: {stderr}");
    assert!(stderr.contains(":2:"), "debe dar la línea real (2, dentro de b.link) del error: {stderr}");
    assert!(stderr.contains('^'), "debe incluir un caret real sobre b.link, no solo nombrarlo en un mensaje plano: {stderr}");
}
