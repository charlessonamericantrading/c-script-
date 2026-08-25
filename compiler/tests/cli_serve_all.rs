// `linkc serve-all <directorio> --port-base N` (GRAMMAR.md §3.92): UN
// proceso sirviendo TODOS los `.link` de un directorio, cada uno en su
// propio hilo y puerto -- reemplaza N procesos `pm2` separados (el caso
// real citado: 13-17 procesos, uno por `.link`, en IgnisLove) por uno solo.
// También cubre `--restart-backoff` (backoff exponencial ante un bind de
// puerto ocupado o una conexión a Postgres caída), nativo en vez de la capa
// externa (`pm2 --restart-delay`) que mitigaba el mismo incidente hoy.
//
// Se prueba contra el BINARIO real, hablando HTTP de verdad y bindeando
// puertos reales -- que el código compile no prueba que dos servicios
// realmente respondan en paralelo desde un solo proceso, ni que un tercero
// atascado no se lleve a los otros dos por delante.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const ALPHA: &str = r#"
type Item = { id: Int, name: String }
db { items: Item[] }
service Alpha {
  rpc ping() -> String { "alpha" }
}
"#;

const BETA: &str = r#"
type Item = { id: Int, name: String }
db { items: Item[] }
service Beta {
  rpc ping() -> String { "beta" }
}
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "linkc-serve-all-{name}-{}-{}",
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
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0)).expect("bindear puerto efímero").local_addr().unwrap().port()
}

/// A diferencia de `linkc serve` (que solo ocupa UN puerto), `serve-all`
/// necesita un RANGO -- `--port-base` más uno por cada `.link` -- así que
/// el TOCTOU de "bindear-y-soltar" de `free_port()` (tolerable con un solo
/// puerto, como en el resto de los tests CLI de este repo) tiene mucha más
/// superficie de colisión acá entre los tests de ESTE archivo corriendo en
/// paralelo (más aún con `restart_backoff_recovers...`, que mantiene un
/// puerto ocupado a propósito por más de un segundo). Serializa los tests
/// de este archivo entre sí -- no toca la paralelización ENTRE archivos de
/// test, cada uno ya corre en su propio proceso.
fn port_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn wait_for_port(port: u16) -> bool {
    let mut buf = [0u8; 1];
    for _ in 0..200 {
        if let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) {
            let ready = stream
                .write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
                .is_ok()
                && matches!(stream.read(&mut buf), Ok(n) if n > 0);
            if ready {
                return true;
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

fn ping(port: u16, service: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("conectar");
    let body = "{}";
    let request = format!(
        "POST /{service}/ping HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).unwrap();
    stream.flush().ok();
    let mut resp = String::new();
    stream.read_to_string(&mut resp).ok();
    let status: u16 = resp.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let body = resp.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    (status, body)
}

struct ServeAll {
    child: Child,
    stderr_path: PathBuf,
}

impl ServeAll {
    fn start(dir: &PathBuf, port_base: u16, extra_args: &[&str]) -> Self {
        let stderr_path = dir.join("__stderr.log");
        let stderr_file = std::fs::File::create(&stderr_path).unwrap();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_linkc"));
        cmd.arg("serve-all").arg(dir).arg("--port-base").arg(port_base.to_string()).arg("--host").arg("127.0.0.1");
        for a in extra_args {
            cmd.arg(a);
        }
        cmd.stdout(Stdio::null()).stderr(Stdio::from(stderr_file));
        let child = cmd.spawn().expect("iniciar 'linkc serve-all'");
        ServeAll { child, stderr_path }
    }

    fn stderr(&self) -> String {
        std::fs::read_to_string(&self.stderr_path).unwrap_or_default()
    }
}

impl Drop for ServeAll {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn serves_every_link_file_in_a_directory_on_sequential_ports_from_one_process() {
    let _guard = port_guard();
    let dir = TempDir::new("basic");
    dir.write("alpha.link", ALPHA);
    dir.write("beta.link", BETA);
    let port_base = free_port();
    let server = ServeAll::start(&dir.0, port_base, &[]);

    assert!(wait_for_port(port_base), "alpha (puerto base) no abrió a tiempo: {}", server.stderr());
    assert!(wait_for_port(port_base + 1), "beta (puerto base + 1) no abrió a tiempo: {}", server.stderr());

    // Orden alfabético: alpha.link < beta.link, así que alpha va en el
    // puerto base y beta en base+1.
    let (status, body) = ping(port_base, "Alpha");
    assert_eq!(status, 200);
    assert_eq!(body, "\"alpha\"");

    let (status, body) = ping(port_base + 1, "Beta");
    assert_eq!(status, 200);
    assert_eq!(body, "\"beta\"");

    // Cada servicio conserva su propio SQLite -- ni un proceso compartido
    // ni una base compartida, solo el conteo de PROCESOS colapsa.
    assert!(dir.0.join("alpha.db").exists(), "alpha debe tener su propio .db");
    assert!(dir.0.join("beta.db").exists(), "beta debe tener su propio .db");
}

/// GRAMMAR.md §3.107: `--port-map-out` escribe la asignación real
/// (nombre de archivo sin `.link` -> puerto) a un JSON, para que un
/// gateway externo no tenga que replicar a mano la regla de orden
/// alfabético -- el caso real: `server/cscript-gateway.ts` de IgnisLove
/// hardcodeaba ese mapa, con el riesgo admitido en su propio comentario de
/// desactualizarse si se agrega/quita/renombra un `.link`.
#[test]
fn port_map_out_writes_the_real_assignment_before_serving() {
    let _guard = port_guard();
    let dir = TempDir::new("port-map-out");
    dir.write("alpha.link", ALPHA);
    dir.write("beta.link", BETA);
    let port_base = free_port();
    let map_path = dir.0.join("ports.json");
    let server = ServeAll::start(&dir.0, port_base, &["--port-map-out", map_path.to_str().unwrap()]);

    assert!(wait_for_port(port_base), "alpha no abrió a tiempo: {}", server.stderr());
    assert!(wait_for_port(port_base + 1), "beta no abrió a tiempo: {}", server.stderr());

    let contents = std::fs::read_to_string(&map_path).expect("--port-map-out debe haber escrito el archivo");
    let parsed: serde_json::Value = serde_json::from_str(&contents).expect("tiene que ser JSON válido");
    assert_eq!(parsed["alpha"], serde_json::json!(port_base), "{contents}");
    assert_eq!(parsed["beta"], serde_json::json!(port_base + 1), "{contents}");
}

#[test]
fn port_map_out_to_an_unwritable_path_fails_clean_before_starting_any_service() {
    let _guard = port_guard();
    let dir = TempDir::new("port-map-out-fails");
    dir.write("alpha.link", ALPHA);
    let port_base = free_port();
    // Un directorio padre que no existe -- `fs::write` falla de entrada,
    // antes de arrancar ningún hilo de servicio.
    let bad_path = dir.0.join("no_existe").join("ports.json");
    let server = ServeAll::start(&dir.0, port_base, &["--port-map-out", bad_path.to_str().unwrap()]);
    // Un solo intento de conexión, no un loop de reintentos (mismo criterio
    // que `a_type_error_in_one_link_file_aborts_the_whole_workspace_before_starting_anything`,
    // arriba en este archivo): si el `.link` NUNCA llegó a arrancar,
    // conectar tiene que fallar de una -- reintentar `wait_for_port` contra
    // un puerto que jamás va a abrir solo alarga la espera sin agregar
    // señal, y en este entorno un `connect()` a un puerto sin nada
    // escuchando puede tardar bastante más que instantáneo.
    std::thread::sleep(Duration::from_millis(300));
    assert!(TcpStream::connect(("127.0.0.1", port_base)).is_err(), "no debería haber arrancado ningún servicio si --port-map-out falló");
    let stderr = server.stderr();
    assert!(stderr.contains("port-map-out"), "{stderr}");
}

#[test]
fn rejects_a_shared_db_flag_up_front() {
    let _guard = port_guard();
    let dir = TempDir::new("shared-db-flag");
    dir.write("alpha.link", ALPHA);
    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve-all")
        .arg(&dir.0)
        .arg("--port-base")
        .arg(free_port().to_string())
        .arg("--db")
        .arg("shared.db")
        .output()
        .expect("ejecutar linkc serve-all");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--db"), "{stderr}");
    assert!(!stderr.contains("panicked at"), "{stderr}");
}

#[test]
fn rejects_the_shared_database_url_env_var_too() {
    let _guard = port_guard();
    let dir = TempDir::new("shared-db-env");
    dir.write("alpha.link", ALPHA);
    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve-all")
        .arg(&dir.0)
        .arg("--port-base")
        .arg(free_port().to_string())
        .env("LINK_DATABASE_URL", "postgres://user:pass@localhost/shared")
        .output()
        .expect("ejecutar linkc serve-all");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("LINK_DATABASE_URL"), "{stderr}");
}

#[test]
fn fails_cleanly_with_no_link_files_in_the_directory() {
    let _guard = port_guard();
    let dir = TempDir::new("empty");
    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve-all")
        .arg(&dir.0)
        .arg("--port-base")
        .arg(free_port().to_string())
        .output()
        .expect("ejecutar linkc serve-all");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(".link"), "{stderr}");
}

#[test]
fn requires_port_base() {
    let _guard = port_guard();
    let dir = TempDir::new("no-port-base");
    dir.write("alpha.link", ALPHA);
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("serve-all").arg(&dir.0).output().expect("ejecutar linkc serve-all");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--port-base"), "{stderr}");
}

#[test]
fn a_type_error_in_one_link_file_aborts_the_whole_workspace_before_starting_anything() {
    let _guard = port_guard();
    let dir = TempDir::new("bad-file");
    dir.write("alpha.link", ALPHA);
    dir.write("broken.link", "service Broken { rpc bad() -> Int { \"not an int\" } }");
    let port_base = free_port();
    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve-all")
        .arg(&dir.0)
        .arg("--port-base")
        .arg(port_base.to_string())
        .output()
        .expect("ejecutar linkc serve-all");
    assert!(!out.status.success());
    // alpha nunca debería haber llegado a aceptar conexiones -- un
    // workspace a medio arrancar es peor que ninguno.
    assert!(TcpStream::connect(("127.0.0.1", port_base)).is_err(), "alpha no debería haber arrancado");
}

#[test]
fn a_bind_failure_in_one_service_does_not_take_down_the_others() {
    let _guard = port_guard();
    let dir = TempDir::new("one-down");
    dir.write("alpha.link", ALPHA);
    dir.write("beta.link", BETA);
    let port_base = free_port();

    // Ocupa el puerto de alpha ANTES de arrancar -- alpha nunca puede
    // bindear, mientras beta (puerto base+1, libre) sí debería.
    let _hog = TcpListener::bind(("127.0.0.1", port_base)).expect("ocupar el puerto de alpha");

    let server = ServeAll::start(&dir.0, port_base, &[]);
    assert!(wait_for_port(port_base + 1), "beta no abrió a tiempo pese a que alpha no pudo bindear: {}", server.stderr());

    let (status, body) = ping(port_base + 1, "Beta");
    assert_eq!(status, 200);
    assert_eq!(body, "\"beta\"");

    // Le da tiempo al hilo de alpha a terminar y loguear el fallo.
    std::thread::sleep(Duration::from_millis(300));
    let stderr = server.stderr();
    assert!(stderr.contains("alpha.link"), "el log debe nombrar el servicio caído: {stderr}");
    assert!(!stderr.contains("panicked at"), "un bind ocupado es un fallo operativo esperado, no un panic: {stderr}");
}

#[test]
fn restart_backoff_recovers_once_the_port_frees_up_while_the_other_service_stays_up() {
    let _guard = port_guard();
    let dir = TempDir::new("backoff-recovers");
    dir.write("alpha.link", ALPHA);
    dir.write("beta.link", BETA);
    let port_base = free_port();

    // Ocupa el puerto de alpha por un rato corto, en otro hilo -- simula el
    // arranque en frío real (el puerto tarda en liberarse, no está ocupado
    // para siempre).
    let hog_port = port_base;
    let hog = std::thread::spawn(move || {
        let listener = TcpListener::bind(("127.0.0.1", hog_port)).expect("ocupar el puerto de alpha");
        std::thread::sleep(Duration::from_millis(1500));
        drop(listener);
    });

    let server = ServeAll::start(&dir.0, port_base, &["--restart-backoff", "1s"]);
    assert!(wait_for_port(port_base + 1), "beta no abrió a tiempo: {}", server.stderr());
    let (status, _) = ping(port_base + 1, "Beta");
    assert_eq!(status, 200, "beta debe seguir sano mientras alpha reintenta");

    hog.join().unwrap();
    assert!(wait_for_port(port_base), "alpha debería recuperarse una vez libre el puerto: {}", server.stderr());
    let (status, body) = ping(port_base, "Alpha");
    assert_eq!(status, 200);
    assert_eq!(body, "\"alpha\"");

    let stderr = server.stderr();
    assert!(stderr.contains("reintentando en"), "{stderr}");
}

/// GRAMMAR.md §3.153, extensión de §3.93: `--service-api-key` es un flag
/// GLOBAL a toda la corrida de `serve-all` -- el landmine real que este
/// test cierra es que, hasta esta ronda, no había forma de que UN servicio
/// del workspace tuviera una política distinta al resto sin sacarlo de
/// `serve-all` por completo. `--service-api-key-exempt <nombre>` deja a ese
/// servicio puntual afuera del chequeo, sin tocar el resto.
fn ping_with_header(port: u16, service: &str, header: Option<(&str, &str)>) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("conectar");
    let body = "{}";
    let header_line = header.map(|(k, v)| format!("{k}: {v}\r\n")).unwrap_or_default();
    let request = format!(
        "POST /{service}/ping HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{header_line}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).unwrap();
    stream.flush().ok();
    let mut resp = String::new();
    stream.read_to_string(&mut resp).ok();
    let status: u16 = resp.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let body = resp.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    (status, body)
}

#[test]
fn service_api_key_exempt_lets_one_service_skip_the_check_while_the_other_still_requires_it() {
    let _guard = port_guard();
    let dir = TempDir::new("api-key-exempt");
    dir.write("alpha.link", ALPHA);
    dir.write("beta.link", BETA);
    let port_base = free_port();
    let server = ServeAll::start(&dir.0, port_base, &["--service-api-key", "s3cr3t", "--service-api-key-exempt", "alpha"]);

    assert!(wait_for_port(port_base), "alpha no abrió a tiempo: {}", server.stderr());
    assert!(wait_for_port(port_base + 1), "beta no abrió a tiempo: {}", server.stderr());

    // alpha (exento): responde SIN el header.
    let (status, body) = ping_with_header(port_base, "Alpha", None);
    assert_eq!(status, 200, "alpha debería estar exento del chequeo: {body}");
    assert_eq!(body, "\"alpha\"");

    // beta (no exento): sigue exigiendo el header de siempre.
    let (status, _) = ping_with_header(port_base + 1, "Beta", None);
    assert_eq!(status, 401, "beta NO está exenta, debe seguir exigiendo la clave");
    let (status, body) = ping_with_header(port_base + 1, "Beta", Some(("X-Service-Api-Key", "s3cr3t")));
    assert_eq!(status, 200, "beta con la clave correcta debe responder normal: {body}");
    assert_eq!(body, "\"beta\"");
}

#[test]
fn service_api_key_exempt_naming_an_unknown_service_fails_clean_before_starting_anything() {
    let _guard = port_guard();
    let dir = TempDir::new("api-key-exempt-unknown");
    dir.write("alpha.link", ALPHA);
    let port_base = free_port();
    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve-all")
        .arg(&dir.0)
        .arg("--port-base")
        .arg(port_base.to_string())
        .arg("--service-api-key")
        .arg("s3cr3t")
        .arg("--service-api-key-exempt")
        .arg("gamma")
        .output()
        .expect("ejecutar linkc serve-all");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("gamma"), "{stderr}");
    assert!(stderr.contains("alpha"), "debe listar los servicios reales conocidos: {stderr}");
    assert!(!stderr.contains("panicked at"), "{stderr}");
    std::thread::sleep(Duration::from_millis(200));
    assert!(TcpStream::connect(("127.0.0.1", port_base)).is_err(), "no debería haber arrancado ningún servicio con un nombre exento inválido");
}

#[test]
fn service_api_key_exempt_without_service_api_key_is_a_clean_usage_error() {
    let _guard = port_guard();
    let dir = TempDir::new("api-key-exempt-no-key");
    dir.write("alpha.link", ALPHA);
    let port_base = free_port();
    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve-all")
        .arg(&dir.0)
        .arg("--port-base")
        .arg(port_base.to_string())
        .arg("--service-api-key-exempt")
        .arg("alpha")
        .output()
        .expect("ejecutar linkc serve-all");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("service-api-key-exempt"), "{stderr}");
    assert!(!stderr.contains("panicked at"), "{stderr}");
}

#[test]
fn a_restart_backoff_flag_without_a_value_is_a_clean_cli_error() {
    let _guard = port_guard();
    let dir = TempDir::new("bad-backoff-flag");
    let src_dir = dir.0.clone();
    dir.write("alpha.link", ALPHA);
    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(src_dir.join("alpha.link"))
        .arg(free_port().to_string())
        .arg("--restart-backoff")
        .output()
        .expect("ejecutar linkc serve");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--restart-backoff"), "{stderr}");
    assert!(!stderr.contains("panicked at"), "{stderr}");
}

