// `@startup` (GRAMMAR.md §3.287, PLAN.md §9.24 Fase 2 ítem E6): corre UNA vez
// al arrancar el servidor, antes de aceptar la primera conexión -- el hueco
// que `@cron` (recurrente, intervalo mínimo) no cubre: un seed de datos que
// solo tiene sentido ejecutar exactamente una vez por arranque del proceso
// (el caso real citado por PLAN.md: sembrar `email_notifications`/settings
// SMTP si la base está vacía).
//
// Contra el BINARIO real vía `linkc serve` -- que el checker acepte la
// anotación no prueba que el seed exista para cuando la primera request
// real llega, ni que el rpc quede inalcanzable por su propia dirección.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-startup-{name}-{}-{}",
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

fn rpc_status_and_body(port: u16, method: &str, body: &str) -> (u16, String) {
    match ureq::post(&format!("http://127.0.0.1:{port}/{method}")).set("Content-Type", "application/json").send_string(body) {
        Ok(r) => (r.status(), r.into_string().unwrap_or_default()),
        Err(ureq::Error::Status(status, r)) => (status, r.into_string().unwrap_or_default()),
        Err(e) => panic!("{method} falló de red: {e}"),
    }
}

const PROGRAM: &str = r#"
type Setting = { id: Int, key: String, value: String }
db { settings: Setting[] }

service Boot {
    @startup
    rpc seedA() -> Void {
        db.settings.insert(Setting { id: 0, key: "smtp_host", value: "localhost" });
    }
    @startup
    rpc seedB() -> Void {
        db.settings.insert(Setting { id: 0, key: "smtp_port", value: "587" });
    }
}

service Api {
    rpc listSettings() -> Setting[] {
        db.settings.all()
    }
}
"#;

#[test]
fn seeded_data_from_startup_is_already_present_on_the_very_first_real_request() {
    let temp = TempDir::new("seeded");
    let link_path = temp.write("app.link", PROGRAM);
    let db_path = temp.0.join("app.db");
    let port = free_port();
    let mut child = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&link_path)
        .arg(port.to_string())
        .arg("--db")
        .arg(&db_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("iniciar linkc serve");
    wait_ready(port);

    let (status, body) = rpc_status_and_body(port, "Api/listSettings", "{}");
    assert_eq!(status, 200, "{body}");
    let settings: serde_json::Value = serde_json::from_str(&body).unwrap();
    let settings = settings.as_array().unwrap();
    assert_eq!(settings.len(), 2, "las DOS tareas @startup independientes tienen que haber corrido: {body}");
    assert!(settings.iter().any(|s| s["key"] == "smtp_host"), "{body}");
    assert!(settings.iter().any(|s| s["key"] == "smtp_port"), "{body}");

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn a_direct_hit_on_the_startup_rpc_itself_is_a_clean_404_not_a_second_run() {
    let temp = TempDir::new("direct-hit");
    let link_path = temp.write("app.link", PROGRAM);
    let db_path = temp.0.join("app.db");
    let port = free_port();
    let mut child = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&link_path)
        .arg(port.to_string())
        .arg("--db")
        .arg(&db_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("iniciar linkc serve");
    wait_ready(port);

    let (status, _) = rpc_status_and_body(port, "Boot/seedA", "{}");
    assert_eq!(status, 404, "un rpc @startup nunca es alcanzable por su propia dirección HTTP");

    // Y sobre todo: no corrió una SEGUNDA vez -- sigue habiendo solo 2 filas.
    let (_, body) = rpc_status_and_body(port, "Api/listSettings", "{}");
    let settings: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(settings.as_array().unwrap().len(), 2, "{body}");

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn startup_failure_logs_but_does_not_prevent_the_server_from_starting() {
    // Un `@startup` que siempre falla (violación de constraint) no debería
    // tumbar el arranque entero -- mismo criterio de resiliencia que
    // `@cron`. El programa sigue sirviendo el resto de los rpcs con
    // normalidad.
    let program = r#"
type Setting = { id: Int, key: String }
db { settings: Setting[] }

service Boot {
    @startup
    rpc broken() -> Void {
        db.settings.insert(Setting { id: 0, key: "a" });
        db.settings.insert(Setting { id: 0, key: "a" });
        panic("seed roto a propósito")
    }
}

service Api {
    rpc ping() -> String { "pong" }
}
"#;
    let temp = TempDir::new("broken-startup");
    let link_path = temp.write("app.link", program);
    let db_path = temp.0.join("app.db");
    let port = free_port();
    let mut child = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&link_path)
        .arg(port.to_string())
        .arg("--db")
        .arg(&db_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("iniciar linkc serve");
    wait_ready(port);

    let (status, body) = rpc_status_and_body(port, "Api/ping", "{}");
    assert_eq!(status, 200, "el servidor tiene que arrancar igual pese al @startup roto: {body}");
    assert_eq!(body, "\"pong\"");

    let _ = child.kill();
    let _ = child.wait();
}
