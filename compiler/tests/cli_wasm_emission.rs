use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-wasm-test-{name}-{}-{}",
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

fn run_wasm_cli(input: &Path, output: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("wasm")
        .arg(input)
        .arg(output)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("no se pudo ejecutar 'linkc wasm'")
}

#[test]
fn wasm_compiles_math_and_recursive_algorithms() {
    let temp = TempDir::new("algorithms");
    let src = r#"
        fn fact(n: Int) -> Int {
            if n <= 1 {
                1
            } else {
                n * fact(n - 1)
            }
        }

        fn fib(n: Int) -> Int {
            let mut a = 0;
            let mut b = 1;
            let mut i = 0;
            while i < n {
                let temp = a + b;
                a = b;
                b = temp;
                i = i + 1;
            }
            a
        }

        fn hypotenuse(a: Float, b: Float) -> Float {
            let a2 = a * a;
            let b2 = b * b;
            a2 + b2
        }

        fn intToFloat(n: Int) -> Float {
            n.toFloat()
        }
    "#;

    let in_file = temp.write("math.link", src);
    let out_file = temp.0.join("math.wasm");

    let res = run_wasm_cli(&in_file, &out_file);
    assert!(
        res.status.success(),
        "linkc wasm debió salir exitoso -- stderr: {}",
        String::from_utf8_lossy(&res.stderr)
    );

    assert!(out_file.exists(), "el archivo math.wasm debió haberse generado");
    let bytes = fs::read(&out_file).unwrap();
    // Validar cabecera mágica de WebAssembly: \0asm (\x00\x61\x73\x6d)
    assert_eq!(&bytes[0..4], &[0x00, 0x61, 0x73, 0x6d]);
    assert_eq!(&bytes[4..8], &[0x01, 0x00, 0x00, 0x00]); // WASM version 1
}

#[test]
fn wasm_compiles_service_rpcs_into_exported_functions() {
    let temp = TempDir::new("service");
    let src = r#"
        service Calculator {
            rpc add(a: Int, b: Int) -> Int {
                a + b
            }
            rpc multiply(a: Int, b: Int) -> Int {
                a * b
            }
        }
    "#;

    let in_file = temp.write("calc.link", src);
    let out_file = temp.0.join("calc.wasm");

    let res = run_wasm_cli(&in_file, &out_file);
    assert!(res.status.success(), "linkc wasm debió compilar el servicio");
    assert!(out_file.exists());
    let bytes = fs::read(&out_file).unwrap();
    assert_eq!(&bytes[0..4], &[0x00, 0x61, 0x73, 0x6d]);
}
