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

#[test]
fn linkc_lint_fix_applies_autofixes_in_place() {
    let temp = TempDir::new("lint-fix-test");
    let src = "fn run(x: Int) -> Int {\n    let unused_val = 123;\n    let mut never_mutated = 456;\n    never_mutated + x\n}\n";
    let file = temp.write("app.link", src);

    let res = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("lint")
        .arg(&file)
        .arg("--fix")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();

    assert!(res.status.success());
    let fixed = fs::read_to_string(&file).unwrap();
    assert!(fixed.contains("let _unused_val = 123;"), "{fixed}");
    assert!(fixed.contains("let never_mutated = 456;"), "{fixed}");
}

#[test]
fn linkc_doc_generates_interactive_html() {
    let temp = TempDir::new("doc-test");
    let src = r#"
    type User = { id: Int, name: String, email: String }
    enum Role { Admin, Member }
    service UserService {
        @authenticated
        rpc get(id: Int) -> User? { null }
    }
    "#;
    let file = temp.write("api.link", src);
    let out_dir = temp.0.join("apidocs");

    let res = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("doc")
        .arg(&file)
        .arg(&out_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();

    assert!(res.status.success());
    let html_file = out_dir.join("index.html");
    assert!(html_file.exists());
    let html = fs::read_to_string(&html_file).unwrap();
    assert!(html.contains("UserService"));
    assert!(html.contains("User"));
    assert!(html.contains("Role"));
    assert!(html.contains("@authenticated"));
}

