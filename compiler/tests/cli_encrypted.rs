// `@encrypted` (GRAMMAR.md §3.191): AES-256-GCM sobre un campo `String`/
// `String?`, puramente a nivel de almacenamiento. Contra el BINARIO real:
// `linkc serve` rechaza arrancar sin `--encryption-key`/`LINK_ENCRYPTION_KEY`
// si el programa declara algún campo así marcado; con una clave real, el
// campo viaja en texto plano por HTTP (invisible del lado de `.link`) pero
// se guarda cifrado -- confirmado leyendo la fila física con `linkc db
// shell`, no solo confiando en que el código "debería" cifrar. `findWhere`
// sobre un campo `@encrypted` sigue matcheando correctamente (cae al camino
// interpretado, que descifra antes de comparar -- ver `leaf_condition_sql`,
// `runtime/db.rs`).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const PROGRAM: &str = r#"
type User = { id: Int, name: String, @encrypted ssn: String }
type NewUser = { name: String, ssn: String }
db { users: User[] }
service Users {
  rpc add(name: String, ssn: String) -> User { db.users.insert(NewUser { name: name, ssn: ssn }) }
  rpc get(id: Int) -> User? { db.users.find(id) }
  rpc findBySsn(ssn: String) -> User[] { db.users.findWhere(|u: User| { u.ssn == ssn }) }
}
"#;

// 32 bytes reales en base64 estándar -- clave de prueba fija, nunca usada
// para nada real.
const TEST_KEY: &str = "CDXG1VdLU/xMH3p4PBXLw1C7uW3IyHDJuhbu3WIbPE8=";

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "linkc-encrypted-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&path).expect("crear tempdir");
        Self(path)
    }

    fn write(&self, name: &str, content: &str) -> PathBuf {
        let full = self.0.join(name);
        std::fs::write(&full, content).unwrap();
        full
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0)).expect("bindear puerto efímero").local_addr().unwrap().port()
}

fn wait_for_port(port: u16) {
    let mut buf = [0u8; 1];
    for _ in 0..200 {
        if let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) {
            let ready = stream
                .write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
                .is_ok()
                && matches!(stream.read(&mut buf), Ok(n) if n > 0);
            if ready {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("'linkc serve' no abrió el puerto {port} a tiempo");
}

struct Serve {
    child: Child,
    port: u16,
}

impl Serve {
    fn start_with_args(link_path: &PathBuf, db_path: &PathBuf, extra_args: &[&str]) -> Self {
        let port = free_port();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_linkc"));
        cmd.arg("serve").arg(link_path).arg(port.to_string()).arg("--db").arg(db_path);
        for a in extra_args {
            cmd.arg(a);
        }
        let child = cmd.stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("iniciar 'linkc serve'");
        wait_for_port(port);
        Serve { child, port }
    }

    fn post(&self, path: &str, body: &str) -> (u16, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("conectar");
        let request = format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            self.port,
            body.len()
        );
        stream.write_all(request.as_bytes()).expect("escribir request");
        stream.flush().ok();

        let mut reader = BufReader::new(stream);
        let mut status_line = String::new();
        reader.read_line(&mut status_line).expect("línea de estado");
        let status: u16 = status_line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or_else(|| panic!("estado HTTP inesperado: {status_line:?}"));

        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            let n = reader.read_line(&mut line).expect("header");
            if n == 0 || line.trim().is_empty() {
                break;
            }
            if let Some((k, v)) = line.trim().split_once(':') {
                if k.trim().eq_ignore_ascii_case("content-length") {
                    content_length = v.trim().parse().unwrap_or(0);
                }
            }
        }
        let mut buf = vec![0u8; content_length];
        reader.read_exact(&mut buf).expect("leer body");
        (status, String::from_utf8_lossy(&buf).into_owned())
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn build(temp: &TempDir, source: &str) -> std::process::Output {
    let src = temp.write("app.link", source);
    Command::new(env!("CARGO_BIN_EXE_linkc")).arg("build").arg(&src).arg(temp.0.join("gen")).output().expect("ejecutar linkc build")
}

/// Una sola consulta SQL de solo lectura contra `db_path` vía `linkc db
/// shell` real -- confirma la representación FÍSICA guardada, sin confiar
/// en que el código de escritura "debería" haber cifrado. Versión de un solo
/// disparo del mismo REPL que `cli_db_shell.rs` prueba con más detalle: acá
/// alcanza con una consulta y cerrar stdin.
fn run_one_shell_query(link_path: &PathBuf, db_path: &PathBuf, sql: &str) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("db")
        .arg("shell")
        .arg(link_path)
        .arg("--db")
        .arg(db_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("iniciar 'linkc db shell'");
    {
        let stdin = child.stdin.as_mut().expect("stdin del hijo");
        writeln!(stdin, "{sql}").expect("escribir la consulta");
    }
    let output = child.wait_with_output().expect("esperar 'linkc db shell'");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn serve_refuses_to_start_without_a_key_when_the_program_declares_an_encrypted_field() {
    let temp = TempDir::new("no-key");
    let out = build(&temp, PROGRAM);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let port = free_port();
    let output = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(temp.path("app.link"))
        .arg(port.to_string())
        .arg("--db")
        .arg(temp.path("app.db"))
        .output()
        .expect("ejecutar 'linkc serve'");
    assert!(!output.status.success(), "no debería arrancar sin clave si el programa declara @encrypted");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("@encrypted") && stderr.contains("--encryption-key"), "{stderr}");
    std::thread::sleep(Duration::from_millis(200));
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err(), "no debería haber abierto el puerto");
}

#[test]
fn serve_refuses_a_key_of_the_wrong_length() {
    let temp = TempDir::new("bad-key-length");
    let out = build(&temp, PROGRAM);
    assert!(out.status.success());
    let port = free_port();
    let output = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(temp.path("app.link"))
        .arg(port.to_string())
        .arg("--db")
        .arg(temp.path("app.db"))
        .arg("--encryption-key")
        .arg("dGVzdA==") // decodifica a 4 bytes, no 32
        .output()
        .expect("ejecutar 'linkc serve'");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("32"), "{stderr}");
}

#[test]
fn an_encrypted_field_round_trips_to_the_exact_plaintext_over_http() {
    let temp = TempDir::new("round-trip");
    let out = build(&temp, PROGRAM);
    assert!(out.status.success());
    let server = Serve::start_with_args(&temp.path("app.link"), &temp.path("app.db"), &["--encryption-key", TEST_KEY]);

    let (status, body) = server.post("/Users/add", r#"{"name":"Ada","ssn":"123-45-6789"}"#);
    assert_eq!(status, 200, "{body}");
    let created: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(created["ssn"], "123-45-6789", "el wire nunca ve el cifrado -- sigue siendo el String plano: {body}");
    let id = created["id"].as_i64().unwrap();

    let (status, body) = server.post("/Users/get", &format!(r#"{{"id":{id}}}"#));
    assert_eq!(status, 200, "{body}");
    let fetched: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(fetched["ssn"], "123-45-6789");
}

#[test]
fn the_raw_sqlite_storage_is_ciphertext_never_the_plaintext() {
    let temp = TempDir::new("raw-storage");
    let out = build(&temp, PROGRAM);
    assert!(out.status.success());
    let db_path = temp.path("app.db");
    let link_path = temp.path("app.link");
    let server = Serve::start_with_args(&link_path, &db_path, &["--encryption-key", TEST_KEY]);
    let (status, _) = server.post("/Users/add", r#"{"name":"Ada","ssn":"123-45-6789"}"#);
    assert_eq!(status, 200);
    drop(server);

    let raw = run_one_shell_query(&link_path, &db_path, "SELECT ssn FROM users;");
    assert!(!raw.contains("123-45-6789"), "el ssn NUNCA debe aparecer en texto plano en la fila física: {raw:?}");
    // Confirma que sí hay ALGO guardado (no una columna vacía por accidente
    // de este test) -- el ciphertext base64 de un valor no vacío siempre
    // tiene largo > 0.
    assert!(raw.lines().any(|l| l.trim().len() > 10), "debería haber un valor cifrado real guardado: {raw:?}");
}

#[test]
fn find_where_on_an_encrypted_field_still_matches_by_falling_back_to_interpreted_filtering() {
    let temp = TempDir::new("find-where-fallback");
    let out = build(&temp, PROGRAM);
    assert!(out.status.success());
    let server = Serve::start_with_args(&temp.path("app.link"), &temp.path("app.db"), &["--encryption-key", TEST_KEY]);

    server.post("/Users/add", r#"{"name":"Ada","ssn":"123-45-6789"}"#);
    server.post("/Users/add", r#"{"name":"Grace","ssn":"987-65-4321"}"#);

    let (status, body) = server.post("/Users/findBySsn", r#"{"ssn":"123-45-6789"}"#);
    assert_eq!(status, 200, "{body}");
    let matches: serde_json::Value = serde_json::from_str(&body).unwrap();
    let arr = matches.as_array().unwrap();
    assert_eq!(arr.len(), 1, "debe encontrar exactamente la fila con ese ssn en texto plano: {body}");
    assert_eq!(arr[0]["name"], "Ada");
    assert_eq!(arr[0]["ssn"], "123-45-6789");
}

#[test]
fn link_encryption_key_env_var_is_honored() {
    let temp = TempDir::new("env-var");
    let out = build(&temp, PROGRAM);
    assert!(out.status.success());
    let port = free_port();
    let child = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(temp.path("app.link"))
        .arg(port.to_string())
        .arg("--db")
        .arg(temp.path("app.db"))
        .env("LINK_ENCRYPTION_KEY", TEST_KEY)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("iniciar 'linkc serve'");
    let server = Serve { child, port };
    wait_for_port(port);

    let (status, body) = server.post("/Users/add", r#"{"name":"Ada","ssn":"123-45-6789"}"#);
    assert_eq!(status, 200, "{body}");
    let created: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(created["ssn"], "123-45-6789");
}

#[test]
fn db_export_refuses_a_program_with_an_encrypted_field() {
    let temp = TempDir::new("export-refuses");
    let out = build(&temp, PROGRAM);
    assert!(out.status.success());
    let export_out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("db")
        .arg("export")
        .arg(temp.path("app.link"))
        .arg(temp.path("export.json"))
        .arg("--db")
        .arg(temp.path("app.db"))
        .output()
        .expect("ejecutar 'linkc db export'");
    assert!(!export_out.status.success());
    let stderr = String::from_utf8_lossy(&export_out.stderr);
    assert!(stderr.contains("@encrypted"), "{stderr}");
    assert!(!temp.path("export.json").exists(), "no debería haber escrito ningún archivo: {stderr}");
}
