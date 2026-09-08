// `@cron` con `initialDelay` y con una expresión cron real de 5 campos
// (GRAMMAR.md §9.24 Fase 2 ítem F1) -- lo que el intervalo fijo `Ns`/`Nm`/
// `Nh`/`Nd` (ya probado en `cli_metrics.rs::metrics_reports_real_cron_runs_and_failures`)
// no cubría: retrasar la PRIMERA corrida (para que varias tareas pesadas no
// arranquen todas juntas en el instante 0), y horarios de calendario reales
// ("todos los días a las 4am") en vez de solo "cada N unidades de tiempo".
//
// Contra el BINARIO real -- que el checker acepte la sintaxis no prueba que
// el scheduler realmente respete la demora, ni que una expresión cron real
// dispare en el minuto de pared correcto.

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-cron-schedule-{name}-{}-{}",
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

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn wait_ready(port: u16) {
    for _ in 0..200 {
        if ureq::get(&format!("http://127.0.0.1:{port}/live")).call().is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("el servidor no levantó a tiempo");
}

fn cron_run_count(port: u16, method: &str) -> u64 {
    let body = ureq::get(&format!("http://127.0.0.1:{port}/metrics")).call().expect("GET /metrics").into_string().unwrap();
    body.lines()
        .find(|l| l.starts_with("linkc_cron_runs_total") && l.contains(&format!("method=\"{method}\"")))
        .and_then(|l| l.rsplit(' ').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

#[test]
fn initial_delay_postpones_the_first_run_past_what_the_bare_interval_would_give() {
    let program = r#"
service Jobs {
    @cron("1s", initialDelay: "3s")
    rpc tick() -> Void { }
}
"#;
    let temp = TempDir::new("delay");
    let link_path = temp.write("app.link", program);
    let port = free_port();
    let mut child = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&link_path)
        .arg(port.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("iniciar linkc serve");
    wait_ready(port);
    let start = Instant::now();

    // Sin `initialDelay`, un `@cron("1s")` ya habría corrido varias veces
    // para este punto (`cli_metrics.rs` espera >= 2 corridas en 2.5s). Con
    // `initialDelay: "3s")`, a 1.5s NO debería haber corrido todavía ni una
    // vez -- eso es lo que prueba que la demora realmente se está
    // respetando, no solo que el flag se acepta.
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(cron_run_count(port, "Jobs.tick"), 0, "a 1.5s con initialDelay=3s todavía no debería haber corrido ninguna vez");

    // Pasada la demora + al menos un tick, ya tiene que haber corrido.
    while start.elapsed() < Duration::from_millis(4500) && cron_run_count(port, "Jobs.tick") == 0 {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(cron_run_count(port, "Jobs.tick") >= 1, "pasados 4.5s (3s de demora + al menos 1 tick de 1s) ya debería haber corrido");

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn a_real_cron_expression_fires_at_the_correct_wall_clock_minute() {
    // Construye una expresión que matchea el PRÓXIMO minuto de pared exacto
    // (nunca el actual, para no depender de en qué segundo del minuto
    // arranca el test) -- la prueba real de que una expresión de 5 campos
    // (no un intervalo) efectivamente se agenda contra el reloj de pared,
    // no una duración fija reinterpretada.
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
    let current_minute_of_hour = (now.as_secs() / 60) % 60;
    let next_minute = (current_minute_of_hour + 1) % 60;
    let seconds_into_current_minute = now.as_secs() % 60;
    // Cuánto falta para que empiece el próximo minuto -- el peor caso real
    // (arrancar el test justo después de que empezó el minuto actual) es
    // casi 60s de espera; sumamos margen para el arranque del servidor.
    let wait_budget = Duration::from_secs(60 - seconds_into_current_minute + 15);

    let program = format!(
        r#"
service Jobs {{
    @cron("{next_minute} * * * *")
    rpc tick() -> Void {{ }}
}}
"#
    );
    let temp = TempDir::new("expression");
    let link_path = temp.write("app.link", &program);
    let port = free_port();
    let mut child = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&link_path)
        .arg(port.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("iniciar linkc serve");
    wait_ready(port);

    let start = Instant::now();
    while start.elapsed() < wait_budget && cron_run_count(port, "Jobs.tick") == 0 {
        std::thread::sleep(Duration::from_millis(250));
    }
    assert!(cron_run_count(port, "Jobs.tick") >= 1, "la expresión cron para el minuto {next_minute} tendría que haber disparado dentro de {wait_budget:?}");

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn a_cron_expression_with_an_out_of_range_field_is_a_clean_compile_error() {
    let temp = TempDir::new("badexpr");
    let src = temp.write(
        "app.link",
        r#"
service Jobs {
    @cron("60 4 * * *")
    rpc tick() -> Void { }
}
"#,
    );
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("build").arg(&src).arg(temp.0.join("gen")).output().expect("linkc build");
    assert!(!out.status.success(), "minuto 60 no existe -- tiene que rechazar la compilación");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("panicked"), "{stderr}");
}

#[test]
fn cron_initial_delay_with_an_invalid_format_is_a_clean_compile_error() {
    let temp = TempDir::new("baddelay");
    let src = temp.write(
        "app.link",
        r#"
service Jobs {
    @cron("5m", initialDelay: "not-a-duration")
    rpc tick() -> Void { }
}
"#,
    );
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("build").arg(&src).arg(temp.0.join("gen")).output().expect("linkc build");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("panicked"), "{stderr}");
}

#[test]
fn cron_second_argument_other_than_initial_delay_is_rejected_at_parse_time() {
    let temp = TempDir::new("badkeyword");
    let src = temp.write(
        "app.link",
        r#"
service Jobs {
    @cron("5m", delay: "1s")
    rpc tick() -> Void { }
}
"#,
    );
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("build").arg(&src).arg(temp.0.join("gen")).output().expect("linkc build");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("initialDelay"), "{stderr}");
}
