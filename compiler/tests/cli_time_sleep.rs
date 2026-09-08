// `time.sleep(ms)` (GRAMMAR.md §3.286, PLAN.md §9.24 Fase 2 ítem F5): la
// única operación de `time` -- espaciar llamadas salientes en un lote (el
// caso real que la motiva es IndexNow, PLAN.md ítem F5: 500ms entre lotes,
// 1s entre motores) sin necesitar un `@cron` artificial por cada pausa.
//
// Contra el BINARIO real vía `linkc test`: que el checker acepte la firma no
// prueba que efectivamente bloquee el tiempo pedido -- eso solo lo prueba
// medir el reloj real alrededor de la llamada.

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-time-sleep-{name}-{}-{}",
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

fn run_link_tests(source: &str) -> (bool, String) {
    let temp = TempDir::new("run");
    let src = temp.write("app.link", source);
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("test").arg(&src).output().expect("linkc test");
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    (out.status.success(), text)
}

#[test]
fn sleep_actually_blocks_for_roughly_the_requested_duration() {
    let program = r#"
service S {
  rpc wait(ms: Int) -> Int { time.sleep(ms); ms }
}
"#;
    let temp = TempDir::new("timing");
    let src = temp.write("app.link", program);
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("build").arg(&src).arg(temp.0.join("gen")).output().expect("linkc build");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

    // No hay forma de invocar un rpc individual con timing preciso vía
    // `linkc test` (los `test{}` corren todos juntos) -- se mide contra un
    // `linkc serve` real en su lugar, un solo request cronometrado.
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let mut child = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&src)
        .arg(port.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("iniciar linkc serve");
    for _ in 0..200 {
        if ureq::get(&format!("http://127.0.0.1:{port}/live")).call().is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }

    let start = Instant::now();
    let resp = ureq::post(&format!("http://127.0.0.1:{port}/S/wait")).set("Content-Type", "application/json").send_string(r#"{"ms": 200}"#);
    let elapsed = start.elapsed();
    let _ = child.kill();
    let _ = child.wait();

    assert!(resp.is_ok(), "{resp:?}");
    assert_eq!(resp.unwrap().into_string().unwrap(), "200");
    assert!(elapsed >= Duration::from_millis(190), "durmió menos de lo pedido: {elapsed:?}");
    assert!(elapsed < Duration::from_secs(5), "durmió mucho más de lo pedido (o colgó): {elapsed:?}");
}

#[test]
fn a_negative_or_absurdly_large_value_is_rejected_cleanly() {
    let program = r#"
service S {
  rpc wait(ms: Int) -> Int { time.sleep(ms); ms }
}
test "un valor negativo falla" { S.wait(-1); }
"#;
    let (ok, text) = run_link_tests(program);
    assert!(!ok, "{text}");
    assert!(text.contains("entre 0 y 300000"), "{text}");

    let program_big = r#"
service S {
  rpc wait(ms: Int) -> Int { time.sleep(ms); ms }
}
test "un valor absurdo falla" { S.wait(999999999); }
"#;
    let (ok, text) = run_link_tests(program_big);
    assert!(!ok, "{text}");
    assert!(text.contains("entre 0 y 300000"), "{text}");
}

#[test]
fn sleep_zero_is_a_no_op_not_an_error() {
    let program = r#"
service S {
  rpc wait() -> Int { time.sleep(0); 1 }
}
test "cero es valido" { assert(S.wait() == 1); }
"#;
    let (ok, text) = run_link_tests(program);
    assert!(ok, "{text}");
    assert!(text.contains("1 passed"), "{text}");
}

#[test]
fn wrong_argument_shape_is_a_compile_error() {
    let program = r#"
service S {
  rpc bad() -> Void { time.sleep("not a number") }
}
"#;
    let temp = TempDir::new("badargs");
    let src = temp.write("app.link", program);
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("build").arg(&src).arg(temp.0.join("gen")).output().expect("linkc build");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("panicked"), "{stderr}");
}
