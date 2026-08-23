// `--adopt-existing`/`LINK_ADOPT_EXISTING` (GRAMMAR.md §3.67): antes de esto,
// la única forma de que `linkc serve` abriera una colección era crearla (si
// faltaba) o auto-migrarla (columnas opcionales agregadas con ALTER TABLE) --
// un chequeo de schema EXACTO que falla fuerte ante cualquier columna física
// de más. Eso bloqueaba adoptar una tabla legacy con columnas que el
// programa todavía no modela, o una base donde el rol de la app no tiene
// permiso de DDL (CREATE/ALTER), ambos casos reales al migrar un sistema
// existente. Este archivo prueba, contra el binario real (dos corridas de
// `linkc serve` seguidas sobre el MISMO archivo SQLite), que --adopt-existing
// nunca toca DDL y solo exige que las columnas DECLARADAS existan.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "linkc-adopt-{name}-{}-{}",
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

    fn db_path(&self, name: &str) -> PathBuf {
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
    fn start(link_path: &PathBuf, db_path: &PathBuf, extra_args: &[&str]) -> Self {
        let port = free_port();
        let child = Command::new(env!("CARGO_BIN_EXE_linkc"))
            .arg("serve")
            .arg(link_path)
            .arg(port.to_string())
            .arg("--db")
            .arg(db_path)
            .args(extra_args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("iniciar 'linkc serve'");
        wait_for_port(port);
        Serve { child, port }
    }

    fn post(&self, path: &str, body: &str) -> (u16, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("conectar");
        let request = format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            self.port,
            body.len()
        );
        stream.write_all(request.as_bytes()).expect("escribir request");
        stream.flush().ok();

        let mut reader = BufReader::new(stream);
        let mut status_line = String::new();
        reader.read_line(&mut status_line).expect("línea de estado");
        let status: u16 = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| panic!("estado HTTP inesperado: {status_line:?}"));

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
        reader.read_exact(&mut buf).expect("body");
        (status, String::from_utf8_lossy(&buf).to_string())
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn adopt_existing_ignores_a_physical_column_the_link_file_does_not_model() {
    let temp = TempDir::new("extra-column");
    let db_path = temp.db_path("app.db");

    // Corrida 1, normal: crea la tabla con una columna que la corrida 2 NO va a declarar.
    let seed_link = temp.write(
        "seed.link",
        r#"
        type Item = { id: Int, name: String, legacyNote: String }
        db { items: Item[] }
        service Items { rpc add(name: String, legacyNote: String) -> Item { db.items.insert(Item { id: 0, name: name, legacyNote: legacyNote }) } }
        "#,
    );
    {
        let server = Serve::start(&seed_link, &db_path, &[]);
        let (status, _) = server.post("/Items/add", r#"{"name":"Ada","legacyNote":"columna que la corrida 2 no conoce"}"#);
        assert_eq!(status, 200);
    } // el Drop mata este proceso, liberando el archivo

    // Corrida 2, --adopt-existing: el .link NO declara "legacyNote". Sin la
    // flag, esto fallaría con "schema incompatible que no se puede migrar
    // automáticamente" (la tabla física tiene una columna de más). Con la
    // flag, tiene que arrancar igual e ignorar esa columna.
    let adopt_link = temp.write(
        "adopt.link",
        r#"
        type Item = { id: Int, name: String }
        db { items: Item[] }
        service Items { rpc all() -> Item[] { db.items.all() } }
        "#,
    );
    let server = Serve::start(&adopt_link, &db_path, &["--adopt-existing"]);
    let (status, body) = server.post("/Items/all", "{}");
    assert_eq!(status, 200, "body: {body}");
    assert!(body.contains("\"name\":\"Ada\""), "la fila preexistente tiene que seguir ahí: {body}");
    assert!(!body.contains("legacyNote"), "una columna no declarada no debe filtrarse a la respuesta: {body}");
}

#[test]
fn adopt_existing_fails_fast_when_the_table_does_not_exist_instead_of_creating_it() {
    let temp = TempDir::new("missing-table");
    let db_path = temp.db_path("app.db"); // nunca se crea

    let link = temp.write(
        "app.link",
        "type Item = { id: Int, name: String }\ndb { items: Item[] }\nservice Items { rpc all() -> Item[] { db.items.all() } }",
    );
    let port = free_port();
    let output = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&link)
        .arg(port.to_string())
        .arg("--db")
        .arg(&db_path)
        .arg("--adopt-existing")
        .output()
        .expect("ejecutar 'linkc serve'");
    assert!(!output.status.success(), "--adopt-existing sobre una tabla inexistente no debería arrancar el servidor");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no existe como tabla"), "mensaje inesperado: {stderr}");
}

#[test]
fn adopt_existing_fails_fast_when_a_declared_column_is_missing_even_if_optional() {
    let temp = TempDir::new("missing-column");
    let db_path = temp.db_path("app.db");

    // Corrida 1: crea la tabla SIN "note".
    let seed_link = temp.write(
        "seed.link",
        "type Item = { id: Int, name: String }\ndb { items: Item[] }\nservice Items { rpc add(name: String) -> Item { db.items.insert(Item { id: 0, name: name }) } }",
    );
    {
        let server = Serve::start(&seed_link, &db_path, &[]);
        let (status, _) = server.post("/Items/add", r#"{"name":"Ada"}"#);
        assert_eq!(status, 200);
    }

    // Corrida 2, --adopt-existing, declara "note?" (opcional): en modo
    // normal esto se auto-migraría con ALTER TABLE ADD COLUMN sin drama.
    // En modo adopción tiene que fallar igual, porque el punto es no
    // ejecutar NINGÚN DDL.
    let adopt_link = temp.write(
        "adopt.link",
        "type Item = { id: Int, name: String, note?: String }\ndb { items: Item[] }\nservice Items { rpc all() -> Item[] { db.items.all() } }",
    );
    let port = free_port();
    let output = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&adopt_link)
        .arg(port.to_string())
        .arg("--db")
        .arg(&db_path)
        .arg("--adopt-existing")
        .output()
        .expect("ejecutar 'linkc serve'");
    assert!(!output.status.success(), "una columna declarada faltante tiene que fallar incluso si es opcional");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("faltan columnas"), "mensaje inesperado: {stderr}");
}

#[test]
fn link_adopt_existing_env_var_has_the_same_effect_as_the_flag() {
    let temp = TempDir::new("env-var");
    let db_path = temp.db_path("app.db");

    let seed_link = temp.write(
        "seed.link",
        r#"type Item = { id: Int, name: String, legacyNote: String }
db { items: Item[] }
service Items { rpc add(name: String, legacyNote: String) -> Item { db.items.insert(Item { id: 0, name: name, legacyNote: legacyNote }) } }"#,
    );
    {
        let server = Serve::start(&seed_link, &db_path, &[]);
        let (status, _) = server.post("/Items/add", r#"{"name":"Beto","legacyNote":"x"}"#);
        assert_eq!(status, 200);
    }

    let adopt_link = temp.write(
        "adopt.link",
        "type Item = { id: Int, name: String }\ndb { items: Item[] }\nservice Items { rpc all() -> Item[] { db.items.all() } }",
    );
    let port = free_port();
    let child = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&adopt_link)
        .arg(port.to_string())
        .arg("--db")
        .arg(&db_path)
        .env("LINK_ADOPT_EXISTING", "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("iniciar 'linkc serve'");
    let mut server = Serve { child, port };
    wait_for_port(server.port);
    let (status, body) = server.post("/Items/all", "{}");
    assert_eq!(status, 200, "body: {body}");
    assert!(body.contains("\"name\":\"Beto\""), "body: {body}");
    let _ = server.child.kill();
    let _ = server.child.wait();
}
