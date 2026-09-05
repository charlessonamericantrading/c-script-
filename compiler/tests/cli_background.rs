// Tests de integración para `@background` + `background.status` (PLAN.md
// §9.18 Eje F ítem 3 / §9.22 ítem 2, GRAMMAR.md §3.262) contra un `linkc
// serve` real sobre SQLite -- esta feature es explícitamente "un proceso,
// sin cola distribuida" (PLAN.md), así que no necesita Postgres para
// verificarse de punta a punta (a diferencia de `@readReplica`).

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-background-{name}-{}-{}",
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
    for _ in 0..100 {
        if ureq::get(&format!("http://127.0.0.1:{port}/live")).call().is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("el servidor no levantó a tiempo");
}

fn rpc(port: u16, method: &str, body: &str) -> serde_json::Value {
    let text = ureq::post(&format!("http://127.0.0.1:{port}/{method}"))
        .set("Content-Type", "application/json")
        .send_string(body)
        .unwrap_or_else(|e| panic!("{method} falló: {e}"))
        .into_string()
        .expect("leer el body");
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{method} no devolvió JSON ({e}): {text}"))
}

fn rpc_with_auth(port: u16, method: &str, body: &str, token: &str) -> serde_json::Value {
    let text = ureq::post(&format!("http://127.0.0.1:{port}/{method}"))
        .set("Content-Type", "application/json")
        .set("Authorization", &format!("Bearer {token}"))
        .send_string(body)
        .unwrap_or_else(|e| panic!("{method} falló: {e}"))
        .into_string()
        .expect("leer el body");
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{method} no devolvió JSON ({e}): {text}"))
}

/// Sondea `checkStatus` hasta que deje de ser "pending"/"running" -- el pool
/// de workers hace polling cada 100ms (GRAMMAR.md §3.262), así que un job
/// trivial siempre termina en un puñado de ciclos; el límite de 3s cubre
/// una máquina de CI genuinamente lenta sin dejar el test colgado para
/// siempre si algo real se rompió.
fn wait_for_job(port: u16, service: &str, job_id: &str) -> serde_json::Value {
    for _ in 0..60 {
        let status = rpc(port, &format!("{service}/checkStatus"), &format!(r#"{{"jobId":"{job_id}"}}"#));
        if status.as_str() != Some("pending") && status.as_str() != Some("running") {
            return status;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("el job '{job_id}' no terminó a tiempo");
}

const BACKGROUND_PROGRAM: &str = r#"
type Task = { id: Int, done: Bool }
db { tasks: Task[] }
service Tasks {
    @background
    rpc process(id: Int) -> Task { db.tasks.applyPatch(id, { done: true }) }

    rpc create() -> Task { db.tasks.insert(Task { id: 0, done: false }) }
    rpc get(id: Int) -> Task? { db.tasks.find(id) }
    rpc checkStatus(jobId: String) -> String { background.status(jobId).status }
    rpc checkError(jobId: String) -> String? { background.status(jobId).error }
}
"#;

#[test]
fn background_rpc_responds_with_a_job_id_immediately_not_the_real_result() {
    let temp = TempDir::new("immediate");
    let link_path = temp.write("app.link", BACKGROUND_PROGRAM);
    let port = free_port();
    let mut child = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&link_path)
        .arg(port.to_string())
        .arg("--db")
        .arg(temp.0.join("app.db"))
        .spawn()
        .expect("spawn linkc serve");
    wait_ready(port);

    let created = rpc(port, "Tasks/create", "{}");
    assert_eq!(created["done"], serde_json::json!(false));

    let response = rpc(port, "Tasks/process", &format!(r#"{{"id":{}}}"#, created["id"]));
    assert!(response.get("jobId").and_then(|j| j.as_str()).is_some(), "se esperaba {{jobId}}, se recibió: {response:?}");
    assert!(response.get("done").is_none(), "un @background NUNCA devuelve la forma real del rpc de inmediato: {response:?}");

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn background_job_runs_asynchronously_and_status_reflects_completion() {
    let temp = TempDir::new("completes");
    let link_path = temp.write("app.link", BACKGROUND_PROGRAM);
    let port = free_port();
    let mut child = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&link_path)
        .arg(port.to_string())
        .arg("--db")
        .arg(temp.0.join("app.db"))
        .spawn()
        .expect("spawn linkc serve");
    wait_ready(port);

    let created = rpc(port, "Tasks/create", "{}");
    let id = created["id"].as_i64().unwrap();
    let job = rpc(port, "Tasks/process", &format!(r#"{{"id":{id}}}"#));
    let job_id = job["jobId"].as_str().unwrap();

    let status = wait_for_job(port, "Tasks", job_id);
    assert_eq!(status, serde_json::json!("done"), "status inesperado: {status:?}");

    // El worker corrió el cuerpo de verdad -- la fila real cambió.
    let fetched = rpc(port, "Tasks/get", &format!(r#"{{"id":{id}}}"#));
    assert_eq!(fetched["done"], serde_json::json!(true), "{fetched:?}");

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn background_job_that_fails_reports_failed_status_and_the_real_error() {
    let temp = TempDir::new("fails");
    let link_path = temp.write("app.link", BACKGROUND_PROGRAM);
    let port = free_port();
    let mut child = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&link_path)
        .arg(port.to_string())
        .arg("--db")
        .arg(temp.0.join("app.db"))
        .spawn()
        .expect("spawn linkc serve");
    wait_ready(port);

    // id 999 no existe -- el cuerpo del job (`applyPatch`) falla de verdad.
    let job = rpc(port, "Tasks/process", r#"{"id":999}"#);
    let job_id = job["jobId"].as_str().unwrap();

    let status = wait_for_job(port, "Tasks", job_id);
    assert_eq!(status, serde_json::json!("failed"), "status inesperado: {status:?}");

    let error = rpc(port, "Tasks/checkError", &format!(r#"{{"jobId":"{job_id}"}}"#));
    assert!(error.as_str().is_some_and(|e| e.contains("999")), "mensaje de error inesperado: {error:?}");

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn background_status_on_an_unknown_job_id_is_not_found_not_an_error() {
    let temp = TempDir::new("not-found");
    let link_path = temp.write("app.link", BACKGROUND_PROGRAM);
    let port = free_port();
    let mut child = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&link_path)
        .arg(port.to_string())
        .arg("--db")
        .arg(temp.0.join("app.db"))
        .spawn()
        .expect("spawn linkc serve");
    wait_ready(port);

    let status = rpc(port, "Tasks/checkStatus", r#"{"jobId":"no-existe-este-id"}"#);
    assert_eq!(status, serde_json::json!("not_found"));

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn background_job_replays_the_original_callers_token_so_auth_currentrole_works() {
    // El worker corre el cuerpo MINUTOS después, en otro hilo, sin ninguna
    // request HTTP real de por medio -- este test confirma que
    // `auth.currentRole()` DENTRO del job ve el mismo rol que el caller
    // original tenía al encolarlo (GRAMMAR.md §3.262: el token se guarda
    // junto con el job y se reproduce tal cual al ejecutar).
    let temp = TempDir::new("auth-replay");
    let src = r#"
enum Role { Admin }
service Auth {
    rpc login() -> String { auth.createSession(Role.Admin {}) }
}
type Echo = { id: Int, role: String? }
db { echoes: Echo[] }
service Jobs {
    @background
    rpc whoAmI() -> Echo { db.echoes.insert(Echo { id: 0, role: auth.currentRole() }) }
    rpc get(id: Int) -> Echo? { db.echoes.find(id) }
    rpc checkStatus(jobId: String) -> String { background.status(jobId).status }
}
"#;
    let link_path = temp.write("app.link", src);
    let port = free_port();
    let mut child = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&link_path)
        .arg(port.to_string())
        .arg("--db")
        .arg(temp.0.join("app.db"))
        .spawn()
        .expect("spawn linkc serve");
    wait_ready(port);

    let token = rpc(port, "Auth/login", "{}");
    let token = token.as_str().expect("login devuelve un token String");

    let job = rpc_with_auth(port, "Jobs/whoAmI", "{}", token);
    let job_id = job["jobId"].as_str().unwrap();
    let status = wait_for_job(port, "Jobs", job_id);
    assert_eq!(status, serde_json::json!("done"), "status inesperado: {status:?}");

    let echo = rpc(port, "Jobs/get", "{\"id\":1}");
    assert_eq!(echo["role"], serde_json::json!("Admin"), "auth.currentRole() dentro del job tiene que ver el rol del caller original: {echo:?}");

    let _ = child.kill();
    let _ = child.wait();
}
