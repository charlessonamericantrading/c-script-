// `linkc serve-all <directorio> --port-base N --port-registry <archivo.json>`
// (GRAMMAR.md §3.153): mecanismo exacto del incidente real reportado desde
// producción (IgnisLove, 17 servicios): `serve-all` sin este flag asigna
// puerto por orden ALFABÉTICO de archivo -- agregar/borrar/renombrar un
// `.link` corre TODOS los puertos posteriores, y ese reordenamiento puede
// coincidir con un puerto que otra cosa (otro servicio, un proxy externo)
// tenía hardcodeado. Con `--port-registry`, el puerto de cada servicio se
// fija por NOMBRE, leído del archivo si ya existe -- agregar/quitar un
// `.link` en la carpeta ya no mueve el puerto de los que ya estaban.
//
// Se prueba contra el BINARIO real -- que el archivo JSON tenga la forma
// correcta no prueba que dos corridas SUCESIVAS de verdad mantengan el
// mismo puerto para el mismo nombre.

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

const GAMMA: &str = r#"
type Item = { id: Int, name: String }
db { items: Item[] }
service Gamma {
  rpc ping() -> String { "gamma" }
}
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "linkc-port-registry-{name}-{}-{}",
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

// Mismo criterio que cli_serve_all.rs: `serve-all` ocupa un RANGO de
// puertos, así que serializa los tests de este archivo entre sí para evitar
// colisiones de TOCTOU entre ellos.
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

struct ServeAll {
    child: Child,
    stderr_path: PathBuf,
}

impl ServeAll {
    fn start(dir: &PathBuf, port_base: u16, extra_args: &[&str]) -> Self {
        let stderr_path = dir.join(format!("__stderr-{}.log", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
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

fn read_registry(path: &PathBuf) -> serde_json::Value {
    let contents = std::fs::read_to_string(path).expect("el registro debe existir");
    serde_json::from_str(&contents).expect("el registro debe ser JSON válido")
}

#[test]
fn a_fresh_registry_gets_sequential_ports_same_as_without_the_flag() {
    let _guard = port_guard();
    let dir = TempDir::new("fresh");
    dir.write("alpha.link", ALPHA);
    dir.write("beta.link", BETA);
    let port_base = free_port();
    let registry_path = dir.0.join("registry.json");
    let server = ServeAll::start(&dir.0, port_base, &["--port-registry", registry_path.to_str().unwrap()]);

    assert!(wait_for_port(port_base), "alpha no abrió a tiempo: {}", server.stderr());
    assert!(wait_for_port(port_base + 1), "beta no abrió a tiempo: {}", server.stderr());

    let registry = read_registry(&registry_path);
    assert_eq!(registry["alpha"], serde_json::json!(port_base));
    assert_eq!(registry["beta"], serde_json::json!(port_base + 1));
}

#[test]
fn adding_a_new_link_file_never_moves_an_already_registered_services_port() {
    let _guard = port_guard();
    let dir = TempDir::new("add-new");
    dir.write("alpha.link", ALPHA);
    dir.write("beta.link", BETA);
    let port_base = free_port();
    let registry_path = dir.0.join("registry.json");

    // Primera corrida: alpha en port_base, beta en port_base+1.
    {
        let server = ServeAll::start(&dir.0, port_base, &["--port-registry", registry_path.to_str().unwrap()]);
        assert!(wait_for_port(port_base), "alpha no abrió a tiempo: {}", server.stderr());
        assert!(wait_for_port(port_base + 1), "beta no abrió a tiempo: {}", server.stderr());
    }

    // Un nuevo archivo "gamma.link" ordena ANTES que "beta.link" en orden
    // alfabético puro -- sin --port-registry, esto correría el puerto de
    // beta. Con el registro, alpha y beta conservan el suyo; gamma (nuevo)
    // recibe el próximo puerto libre.
    dir.write("gamma.link", GAMMA);
    let server = ServeAll::start(&dir.0, port_base, &["--port-registry", registry_path.to_str().unwrap()]);
    assert!(wait_for_port(port_base), "alpha debe seguir en su puerto de siempre: {}", server.stderr());
    assert!(wait_for_port(port_base + 1), "beta debe seguir en su puerto de siempre: {}", server.stderr());
    assert!(wait_for_port(port_base + 2), "gamma (nuevo) debe abrir en el próximo puerto libre: {}", server.stderr());

    let registry = read_registry(&registry_path);
    assert_eq!(registry["alpha"], serde_json::json!(port_base), "alpha no debe moverse");
    assert_eq!(registry["beta"], serde_json::json!(port_base + 1), "beta no debe moverse");
    assert_eq!(registry["gamma"], serde_json::json!(port_base + 2));
}

#[test]
fn a_removed_services_port_is_never_silently_reused_by_a_different_new_service() {
    let _guard = port_guard();
    let dir = TempDir::new("removed");
    let alpha_path = dir.write("alpha.link", ALPHA);
    dir.write("beta.link", BETA);
    let port_base = free_port();
    let registry_path = dir.0.join("registry.json");

    // Primera corrida: alpha=port_base, beta=port_base+1.
    {
        let server = ServeAll::start(&dir.0, port_base, &["--port-registry", registry_path.to_str().unwrap()]);
        assert!(wait_for_port(port_base), "alpha no abrió a tiempo: {}", server.stderr());
        assert!(wait_for_port(port_base + 1), "beta no abrió a tiempo: {}", server.stderr());
    }

    // Se borra alpha.link y se agrega gamma.link -- gamma NO debe heredar
    // el puerto que alpha tenía (port_base), aunque sea el más "temprano"
    // en orden alfabético entre los archivos que quedan.
    std::fs::remove_file(&alpha_path).unwrap();
    dir.write("gamma.link", GAMMA);

    let server = ServeAll::start(&dir.0, port_base, &["--port-registry", registry_path.to_str().unwrap()]);
    assert!(wait_for_port(port_base + 1), "beta debe seguir en su puerto de siempre: {}", server.stderr());
    // gamma recibe el siguiente puerto libre DESPUÉS de port_base+1 (el de
    // beta), nunca port_base (el que "quedó libre" al borrar alpha).
    assert!(TcpStream::connect(("127.0.0.1", port_base)).is_err(), "nada debe estar escuchando en el puerto viejo de alpha (borrado)");
    assert!(wait_for_port(port_base + 2), "gamma debe recibir un puerto nuevo, nunca el de alpha: {}", server.stderr());

    let registry = read_registry(&registry_path);
    assert_eq!(registry["alpha"], serde_json::json!(port_base), "la entrada de alpha permanece en el registro aunque el .link ya no exista");
    assert_eq!(registry["beta"], serde_json::json!(port_base + 1));
    assert_eq!(registry["gamma"], serde_json::json!(port_base + 2), "gamma no debe heredar el puerto liberado por alpha");
}

#[test]
fn an_invalid_registry_file_fails_clean_before_starting_any_service() {
    let _guard = port_guard();
    let dir = TempDir::new("invalid-json");
    dir.write("alpha.link", ALPHA);
    let port_base = free_port();
    let registry_path = dir.0.join("registry.json");
    std::fs::write(&registry_path, "esto no es JSON").unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve-all")
        .arg(&dir.0)
        .arg("--port-base")
        .arg(port_base.to_string())
        .arg("--port-registry")
        .arg(&registry_path)
        .output()
        .expect("ejecutar linkc serve-all");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("port-registry"), "{stderr}");
    assert!(!stderr.contains("panicked at"), "{stderr}");
    std::thread::sleep(Duration::from_millis(200));
    assert!(TcpStream::connect(("127.0.0.1", port_base)).is_err(), "no debería haber arrancado ningún servicio con un registro inválido");
}
