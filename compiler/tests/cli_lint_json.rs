// `linkc lint --diagnostics-json` (GRAMMAR.md §3.224, PLAN.md §9.18 Eje C
// ítem 5): las advertencias de lint con la MISMA forma JSON que los errores
// de carga/tipos de §3.208 (`[{file, line, column, message, code}]`), con
// `code` = nombre de la regla -- un agente que ya parsea `linkc test
// --diagnostics-json` parsea esto sin un segundo formato.

use std::path::PathBuf;
use std::process::Command;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-lintjson-{name}-{}-{}",
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

const WITH_WARNINGS: &str = r#"
fn f() -> Int {
  let unused = 1;
  let mut alsoUnused = 2;
  3
}
"#;

#[test]
fn lint_with_diagnostics_json_prints_one_object_per_warning_with_the_rule_as_code() {
    let temp = TempDir::new("warnings");
    let src = temp.write("app.link", WITH_WARNINGS);
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("lint").arg(&src).arg("--diagnostics-json").output().expect("linkc lint");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "lint con advertencias sigue saliendo 0, igual que sin el flag:\n{stdout}\n{stderr}");
    assert!(stderr.trim().is_empty(), "nada humano a stderr con el flag: {stderr}");
    assert!(!stdout.contains("advertencia(s)"), "el texto humano no debe mezclarse con el JSON: {stdout}");

    let diags: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("stdout no es JSON ({e}): {stdout}"));
    let arr = diags.as_array().expect("array");
    assert!(arr.len() >= 2, "{arr:?}");
    let codes: Vec<&str> = arr.iter().map(|d| d["code"].as_str().unwrap()).collect();
    assert!(codes.contains(&"unused-var"), "{codes:?}");
    assert!(arr.iter().all(|d| d["file"].as_str().unwrap().ends_with("app.link")), "{arr:?}");
    let unused = arr.iter().find(|d| d["code"] == "unused-var").unwrap();
    assert_eq!(unused["line"], 3, "{unused}");
    assert!(unused["column"].as_u64().is_some(), "{unused}");
    assert!(unused["message"].as_str().unwrap().contains("unused"), "{unused}");
}

#[test]
fn lint_with_diagnostics_json_prints_an_empty_array_when_there_is_nothing_to_report() {
    let temp = TempDir::new("clean");
    let src = temp.write("app.link", "fn f() -> Int { 3 }\n");
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("--diagnostics-json").arg("lint").arg(&src).output().expect("linkc lint");
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "[]");
}

#[test]
fn lint_with_diagnostics_json_still_reports_a_type_error_in_the_same_shape() {
    // Un programa que no tipa nunca llega al linter -- el flag global ya
    // cubre ese camino (§3.208); acá solo se confirma que `lint` no lo
    // esquiva y que la forma es la misma.
    let temp = TempDir::new("type-error");
    let src = temp.write("app.link", "fn f() -> Int { \"no\" }\n");
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("lint").arg(&src).arg("--diagnostics-json").output().expect("linkc lint");
    assert!(!out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let diags: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("stdout no es JSON ({e}): {stdout}"));
    assert_eq!(diags.as_array().unwrap().len(), 1, "{diags}");
    assert_eq!(diags[0]["line"], 1);
}
