// `--host <dirección>`/`LINK_HOST` (GRAMMAR.md §3.81): hasta esta ronda
// `linkc serve` siempre escuchaba en `0.0.0.0` (todas las interfaces), sin
// ninguna alternativa -- un gap de seguridad real, no solo de conveniencia,
// para un proceso que solo necesita aceptar conexiones locales (detrás de
// un proxy en el mismo host, por ejemplo) y terminaba dependiendo
// ÚNICAMENTE del firewall del sistema operativo como capa de defensa.
//
// Se prueba contra el binario real como subproceso, igual que
// `cli_cors.rs` (mismo helper `Serve`, casi textual). La forma más portable
// de probar que el valor de `--host` de verdad se usa para bindear (sin
// depender de que la máquina de test tenga más de una interfaz de red
// configurada) es pedirle que bindee una dirección que NO le pertenece a
// ninguna interfaz local -- `192.0.2.1` es TEST-NET-1 (RFC 5737),
// reservada para documentación, nunca asignada a un host real.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

const PROGRAM: &str = r#"
service Sys {
  rpc ping() -> String {
    "pong"
  }
}
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "linkc-host-{name}-{}-{}",
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
    fn start(link_path: &PathBuf, extra_args: &[&str], extra_env: &[(&str, &str)]) -> Self {
        let port = free_port();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_linkc"));
        cmd.arg("serve").arg(link_path).arg(port.to_string());
        for a in extra_args {
            cmd.arg(a);
        }
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        let child = cmd.spawn().expect("iniciar 'linkc serve'");
        wait_for_port(port);
        Serve { child, port }
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn default_host_still_accepts_a_loopback_connection() {
    // Sin `--host`: comportamiento de siempre (`0.0.0.0`), que entre otras
    // cosas acepta conexiones por loopback -- no debe romperse por esta
    // ronda.
    let temp = TempDir::new("default");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &[], &[]);

    let mut stream = TcpStream::connect(("127.0.0.1", server.port)).expect("conectar por loopback");
    stream
        .write_all(b"POST /Sys/ping HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
        .unwrap();
    let mut status_line = String::new();
    BufReader::new(stream).read_line(&mut status_line).expect("línea de estado");
    assert!(status_line.starts_with("HTTP/1.1 200"), "{status_line}");
}

#[test]
fn explicit_host_flag_still_serves_on_that_same_address() {
    let temp = TempDir::new("explicit");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &["--host", "127.0.0.1"], &[]);

    let stream = TcpStream::connect(("127.0.0.1", server.port)).expect("conectar por loopback tras --host 127.0.0.1");
    drop(stream);
}

#[test]
fn link_host_env_var_is_honored() {
    let temp = TempDir::new("env");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &[], &[("LINK_HOST", "127.0.0.1")]);

    let stream = TcpStream::connect(("127.0.0.1", server.port)).expect("conectar por loopback tras LINK_HOST");
    drop(stream);
}

#[test]
fn a_host_that_belongs_to_no_local_interface_fails_to_bind_instead_of_being_silently_ignored() {
    // Prueba, sin depender de que la máquina de test tenga una segunda
    // interfaz real configurada, que `--host` de verdad se usa para
    // bindear: `192.0.2.1` (TEST-NET-1, RFC 5737) nunca le pertenece a
    // ninguna interfaz local real, así que el intento de bind falla -- si
    // el flag se estuviera ignorando silenciosamente (cayendo siempre a
    // `0.0.0.0`), el proceso arrancaría igual y este test no detectaría la
    // regresión.
    let temp = TempDir::new("unbindable");
    let src = temp.write("app.link", PROGRAM);
    let port = free_port();
    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&src)
        .arg(port.to_string())
        .arg("--host")
        .arg("192.0.2.1")
        .output()
        .expect("ejecutar linkc serve");
    assert!(!out.status.success(), "bindear una dirección que no es local debe fallar, no arrancar igual en 0.0.0.0");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("192.0.2.1"), "el mensaje de error debe nombrar la dirección pedida: {stderr}");
}

#[test]
fn a_host_flag_without_a_value_is_a_clean_cli_error() {
    let temp = TempDir::new("badflag");
    let src = temp.write("app.link", PROGRAM);
    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&src)
        .arg(free_port().to_string())
        .arg("--host")
        .output()
        .expect("ejecutar linkc serve");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--host"), "el mensaje debe nombrar el flag: {stderr}");
    assert!(!stderr.contains("panicked at"), "un flag mal usado es un error de uso, no un panic: {stderr}");
}

#[test]
fn an_empty_host_flag_is_rejected_instead_of_silently_binding_everywhere() {
    let temp = TempDir::new("emptyflag");
    let src = temp.write("app.link", PROGRAM);
    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&src)
        .arg(free_port().to_string())
        .arg("--host")
        .arg("")
        .output()
        .expect("ejecutar linkc serve");
    assert!(!out.status.success(), "'--host \"\"' no debe caer silenciosamente al default");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--host"), "{stderr}");
}

#[test]
fn cli_flag_takes_precedence_over_the_env_var() {
    // El flag pide una dirección inalcanzable a propósito; el env var pide
    // loopback -- si el env var ganara, el bind tendría éxito. Mismo
    // criterio de precedencia que el resto de los flags de `serve`
    // (`--cors-origin`/`LINK_CORS_ORIGINS`, etc.).
    let temp = TempDir::new("precedence");
    let src = temp.write("app.link", PROGRAM);
    let port = free_port();
    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&src)
        .arg(port.to_string())
        .arg("--host")
        .arg("192.0.2.1")
        .env("LINK_HOST", "127.0.0.1")
        .output()
        .expect("ejecutar linkc serve");
    assert!(!out.status.success(), "el flag debe ganarle al env var");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("192.0.2.1"), "{stderr}");
}
