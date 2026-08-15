use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-fmt-lint-test-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let path = std::env::temp_dir().join(unique);
        fs::create_dir_all(&path).expect("no se pudo crear tempdir para test");
        Self(path)
    }

    fn write(&self, relative: &str, content: &str) -> PathBuf {
        let full = self.0.join(relative);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&full, content).unwrap();
        full
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn linkc_fmt_formats_file_in_place_and_respects_check_flag() {
    let temp = TempDir::new("fmt-test");
    let unformatted = "fn calculate(a:Int,b:Int)->Int{let sum=a+b;sum}\n";
    let file = temp.write("calc.link", unformatted);

    // 1. Con --check debe fallar porque no está formateado
    let res_check = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("fmt")
        .arg(&file)
        .arg("--check")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(!res_check.status.success());

    // 2. Ejecutar formato in-place
    let res_fmt = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("fmt")
        .arg(&file)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(res_fmt.status.success());

    let content_after = fs::read_to_string(&file).unwrap();
    assert!(content_after.contains("fn calculate(a: Int, b: Int) -> Int {\n  let sum = a + b;\n  sum\n}"));

    // 3. Ahora con --check debe salir 0 exitoso
    let res_check2 = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("fmt")
        .arg(&file)
        .arg("--check")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(res_check2.status.success());
}

#[test]
fn linkc_lint_detects_unused_variables_and_empty_tests() {
    let temp = TempDir::new("lint-test");
    let src = r#"
        fn run(x: Int) -> Int {
            let unused_val = 123;
            let mut never_mutated = 456;
            never_mutated + x
        }
        test "placeholder" { }
    "#;
    let file = temp.write("app.link", src);

    let res = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("lint")
        .arg(&file)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();

    assert!(res.status.success());
    let stdout = String::from_utf8_lossy(&res.stdout);
    assert!(stdout.contains("unused-var"), "{stdout}");
    assert!(stdout.contains("unused-mut"), "{stdout}");
    assert!(stdout.contains("empty-test"), "{stdout}");
}
