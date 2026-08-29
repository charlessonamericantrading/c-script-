// `linkc db import` (GRAMMAR.md §3.185): lee un archivo de `db export` y
// escribe sus filas contra un target, PRESERVANDO el id original de cada
// fila. Se prueba contra el BINARIO real: que compile no prueba que un
// import a un target vacío (el caso "seed") deje la secuencia de ids lista
// para que un `insert()` normal posterior no choque, ni que un choque de
// id de verdad revierta TODO el import sin dejar nada a medias.

use serde_json::Value;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

const PROGRAM: &str = r#"
type Item = { id: Int, name: String, price: Decimal }
db { items: Item[] }
service Items {
  rpc add(name: String, price: Decimal) -> Item { db.items.insert(Item { id: 0, name: name, price: price }) }
  rpc get(id: Int) -> Item? { db.items.find(id) }
}
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "linkc-db-import-{name}-{}-{}",
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

struct Serve {
    child: Child,
    port: u16,
}

impl Serve {
    fn start(link_path: &PathBuf, db_path: &PathBuf) -> Self {
        let port = free_port();
        let child = Command::new(env!("CARGO_BIN_EXE_linkc"))
            .arg("serve")
            .arg(link_path)
            .arg(port.to_string())
            .arg("--db")
            .arg(db_path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("iniciar 'linkc serve'");
        let server = Serve { child, port };
        server.wait_ready();
        server
    }

    fn wait_ready(&self) {
        for _ in 0..200 {
            if ureq::get(&format!("http://127.0.0.1:{}/health", self.port)).call().is_ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("'linkc serve' no abrió el puerto {} a tiempo", self.port);
    }

    fn rpc(&self, method: &str, body: &str) -> Value {
        let text = ureq::post(&format!("http://127.0.0.1:{}/{method}", self.port))
            .set("Content-Type", "application/json")
            .send_string(body)
            .unwrap_or_else(|e| panic!("{method} falló: {e}"))
            .into_string()
            .expect("leer el body");
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{method} no devolvió JSON ({e}): {text}"))
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn run(sub: &str, args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("db").arg(sub).args(args).output().unwrap_or_else(|e| panic!("ejecutar linkc db {sub}: {e}"));
    (out.status.success(), String::from_utf8_lossy(&out.stdout).to_string(), String::from_utf8_lossy(&out.stderr).to_string())
}

fn export_from(src: &PathBuf, db_path: &PathBuf, out_path: &PathBuf) {
    let (success, stdout, stderr) = run("export", &[src.to_str().unwrap(), out_path.to_str().unwrap(), "--db", db_path.to_str().unwrap()]);
    assert!(success, "export falló -- stdout: {stdout}\nstderr: {stderr}");
}

fn row_count(db_path: &PathBuf, src: &PathBuf) -> i64 {
    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("db")
        .arg("inspect")
        .arg(src)
        .arg("--db")
        .arg(db_path)
        .output()
        .expect("ejecutar linkc db inspect");
    let stdout = String::from_utf8_lossy(&out.stdout);
    // "  items       2 columna(s) declaradas  N fila(s)" -- extrae N.
    stdout
        .lines()
        .find(|l| l.trim_start().starts_with("items"))
        .and_then(|l| l.split_whitespace().rev().nth(1))
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no se pudo leer el conteo de filas de: {stdout}"))
}

/// El caso "seed" (PLAN.md §9.7 ítem 2): importar contra un target VACÍO
/// es exactamente lo mismo que poblar una base nueva desde un fichero --
/// sin código aparte. Confirma también que el id ORIGINAL sobrevive (no
/// se reasigna) y que la secuencia queda lista: un `insert()` normal
/// posterior (vía RPC real) no choca con ningún id importado.
#[test]
fn importing_into_a_fresh_target_seeds_it_preserving_original_ids_and_resyncing_the_sequence() {
    let temp = TempDir::new("seed");
    let src = temp.write("app.link", PROGRAM);
    let source_db = temp.path("source.db");
    let server = Serve::start(&src, &source_db);
    server.rpc("Items/add", r#"{"name":"Widget","price":"19.9900"}"#);
    server.rpc("Items/add", r#"{"name":"Gadget","price":"5.5000"}"#);
    drop(server);

    let export_path = temp.path("export.json");
    export_from(&src, &source_db, &export_path);

    let target_db = temp.path("target.db");
    assert!(!target_db.exists());
    let (success, stdout, stderr) = run("import", &[src.to_str().unwrap(), export_path.to_str().unwrap(), "--db", target_db.to_str().unwrap()]);
    assert!(success, "stdout: {stdout}\nstderr: {stderr}");
    assert!(stdout.contains("2 fila(s) importadas"), "{stdout}");

    let server = Serve::start(&src, &target_db);
    let fetched = server.rpc("Items/get", r#"{"id":1}"#);
    assert_eq!(fetched["name"], "Widget", "id ORIGINAL preservado: {fetched}");
    let created = server.rpc("Items/add", r#"{"name":"Thingamajig","price":"1.0000"}"#);
    assert_eq!(created["id"], serde_json::json!(3), "la secuencia resincronizada no choca con los ids importados: {created}");
}

/// Cruce de entornos: importar contra un target que YA fue servido antes
/// (mismo esquema, con SUS PROPIAS filas previas) -- DDL idempotente
/// (`CREATE TABLE IF NOT EXISTS`), sigue derecho a los datos, ninguna fila
/// previa se pierde ni se toca. El archivo a importar trae un id EXPLÍCITO
/// alto (500) a propósito, sin pasar por `db export`/`Serve` -- dos
/// entornos servidos independientemente arrancan su autoincremento en el
/// mismo id (1) por diseño, así que un choque de verdad ahí sería
/// esperado (ver el test de choque de id, abajo), no lo que ESTE test
/// quiere ejercitar: DDL idempotente + no pérdida de datos previos.
#[test]
fn importing_into_an_already_served_target_is_idempotent_and_keeps_its_prior_rows() {
    let temp = TempDir::new("cross-env");
    let src = temp.write("app.link", PROGRAM);
    let export_path = temp.write(
        "export.json",
        r#"{"linkc_version":"0","exported_at":"","collections":{"items":[{"id":500,"name":"FromSource","price":"1.0000"}]}}"#,
    );

    let target_db = temp.path("target.db");
    let server = Serve::start(&src, &target_db);
    server.rpc("Items/add", r#"{"name":"AlreadyThere","price":"2.0000"}"#);
    drop(server);
    assert_eq!(row_count(&target_db, &src), 1);

    let (success, stdout, stderr) = run("import", &[src.to_str().unwrap(), export_path.to_str().unwrap(), "--db", target_db.to_str().unwrap()]);
    assert!(success, "stdout: {stdout}\nstderr: {stderr}");
    assert_eq!(row_count(&target_db, &src), 2, "la fila previa del target sigue ahí, más la importada");

    let server = Serve::start(&src, &target_db);
    let prior = server.rpc("Items/get", r#"{"id":1}"#);
    assert_eq!(prior["name"], "AlreadyThere", "la fila previa del target no se tocó: {prior}");
    let imported = server.rpc("Items/get", r#"{"id":500}"#);
    assert_eq!(imported["name"], "FromSource", "la fila importada llegó con su id explícito intacto: {imported}");
}

/// Un choque de id (la fila ya existe en el target) cancela y revierte
/// TODO el import -- nunca deja datos a medias. Importar el MISMO archivo
/// dos veces al mismo target reproduce esto de forma determinística.
#[test]
fn importing_the_same_file_twice_fails_loud_on_id_collision_and_leaves_the_target_unchanged() {
    let temp = TempDir::new("collision");
    let src = temp.write("app.link", PROGRAM);
    let source_db = temp.path("source.db");
    let server = Serve::start(&src, &source_db);
    server.rpc("Items/add", r#"{"name":"Widget","price":"1.0000"}"#);
    server.rpc("Items/add", r#"{"name":"Gadget","price":"2.0000"}"#);
    drop(server);
    let export_path = temp.path("export.json");
    export_from(&src, &source_db, &export_path);

    let target_db = temp.path("target.db");
    let (success, ..) = run("import", &[src.to_str().unwrap(), export_path.to_str().unwrap(), "--db", target_db.to_str().unwrap()]);
    assert!(success);
    assert_eq!(row_count(&target_db, &src), 2);

    let (success, stdout, stderr) = run("import", &[src.to_str().unwrap(), export_path.to_str().unwrap(), "--db", target_db.to_str().unwrap()]);
    assert!(!success, "stdout: {stdout}\nstderr: {stderr}");
    assert!(stderr.contains("id=1") || stderr.contains("id="), "el error nombra la colección/id en choque: {stderr}");
    assert!(stderr.contains("cancelado"), "{stderr}");
    assert_eq!(row_count(&target_db, &src), 2, "el segundo import (fallido) no dejó nada a medias ni duplicó filas");
}

/// Una colección en el archivo que el `.link` ACTUAL no declara es un
/// error duro ANTES de tocar cualquier fila -- ni siquiera se conecta con
/// el target (si conectara, el archivo `.db`/esquema ya existiría aunque
/// ninguna fila se hubiera escrito, lo que haría impreciso decir "nada se
/// escribió").
#[test]
fn an_unknown_collection_in_the_file_is_a_clean_error_and_touches_nothing() {
    let temp = TempDir::new("unknown-collection");
    let src = temp.write("app.link", PROGRAM);
    let bad_export = temp.write(
        "bad.json",
        r#"{"linkc_version":"0","exported_at":"","collections":{"items":[],"ghosts":[{"id":1}]}}"#,
    );
    let target_db = temp.path("target.db");
    let (success, stdout, stderr) = run("import", &[src.to_str().unwrap(), bad_export.to_str().unwrap(), "--db", target_db.to_str().unwrap()]);
    assert!(!success, "stdout: {stdout}\nstderr: {stderr}");
    assert!(stderr.contains("ghosts"), "{stderr}");
    assert!(!target_db.exists(), "ni siquiera el archivo .db se debe crear -- nada se escribió de verdad");
}

#[test]
fn a_db_import_call_missing_the_input_path_is_a_clean_usage_error() {
    let temp = TempDir::new("missing-arg");
    let src = temp.write("app.link", PROGRAM);
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("db").arg("import").arg(&src).output().expect("ejecutar linkc db import");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("uso: linkc db import"), "{stderr}");
}
