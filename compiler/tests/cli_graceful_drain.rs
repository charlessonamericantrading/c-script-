// Drenado gracioso ante SIGTERM/Ctrl-C (GRAMMAR.md §3.282, PLAN.md §9.18 Eje
// E ítem 1 / §9.24 Fase 1 ítem C14). Se prueba contra el BINARIO real,
// mandándole la señal real que un `pm2 restart`/`systemctl stop` mandaría --
// `Child::kill()` de std es SIGKILL en Unix (nunca SIGTERM), así que no sirve
// para esto; cada plataforma necesita su propio mecanismo:
//   - Unix: `libc::kill(pid, SIGTERM)` real.
//   - Windows: `GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid)` -- el único
//     evento de consola que SÍ se puede dirigir a un proceso hijo puntual
//     (CTRL_C_EVENT lo recibe TODO el grupo de consola, incluido este test).
//     Requiere que el hijo se cree con `CREATE_NEW_PROCESS_GROUP`, si no
//     `GenerateConsoleCtrlEvent` no tiene a quién apuntarle.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

const PROGRAM: &str = r#"
service Slow {
  rpc wait() -> String { http.get("http://127.0.0.1:{{UPSTREAM_PORT}}/") }
  rpc fast() -> Int { 1 }
}
"#;

struct TempDir(PathBuf);

static TEMP_DIR_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl TempDir {
    fn new(name: &str) -> Self {
        let n = TEMP_DIR_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "linkc-drain-{name}-{}-{}-{n}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
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
    TcpListener::bind(("127.0.0.1", 0)).expect("bindear puerto efímero").local_addr().unwrap().port()
}

/// Servidor TCP crudo de una sola conexión por vez que tarda `delay` en
/// responder -- el "upstream lento" que `Slow.wait()` llama, para tener una
/// request genuinamente en vuelo cuando llega la señal.
fn start_slow_upstream(delay: Duration) -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bindear upstream");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let delay = delay;
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                std::thread::sleep(delay);
                let body = b"slow-ok";
                let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.write_all(body);
            });
        }
    });
    port
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

/// GET/POST crudo -- devuelve (status, body).
fn request(port: u16, method: &str, path: &str, body: &str) -> std::io::Result<(u16, String)> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))?;
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes())?;
    stream.flush().ok();
    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader.read_line(&mut status_line)?;
    let status: u16 = status_line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
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
    reader.read_exact(&mut buf)?;
    Ok((status, String::from_utf8_lossy(&buf).to_string()))
}

#[cfg(windows)]
mod signal {
    use std::os::windows::process::CommandExt;
    pub const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CTRL_BREAK_EVENT: u32 = 1;

    #[link(name = "kernel32")]
    extern "system" {
        fn GenerateConsoleCtrlEvent(dw_ctrl_event: u32, dw_process_group_id: u32) -> i32;
    }

    pub fn spawn_flags(cmd: &mut std::process::Command) {
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }

    /// El hijo fue creado con `CREATE_NEW_PROCESS_GROUP` (arriba), así que su
    /// PID ES el id de su propio grupo de consola -- `GenerateConsoleCtrlEvent`
    /// apunta a ESE grupo nada más, nunca al proceso de este test.
    pub fn send_graceful_shutdown(pid: u32) {
        let ok = unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid) };
        assert_ne!(ok, 0, "GenerateConsoleCtrlEvent falló (error {})", std::io::Error::last_os_error());
    }
}

#[cfg(unix)]
mod signal {
    pub fn spawn_flags(_cmd: &mut std::process::Command) {}

    pub fn send_graceful_shutdown(pid: u32) {
        let ret = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        assert_eq!(ret, 0, "kill(SIGTERM) falló: {}", std::io::Error::last_os_error());
    }
}

struct Serve {
    child: Child,
    port: u16,
}

impl Serve {
    fn start_with_args(link_path: &PathBuf, extra_args: &[&str]) -> Self {
        let port = free_port();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_linkc"));
        cmd.arg("serve").arg(link_path).arg(port.to_string());
        for a in extra_args {
            cmd.arg(a);
        }
        signal::spawn_flags(&mut cmd);
        let child = cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().expect("iniciar 'linkc serve'");
        wait_for_port(port);
        Serve { child, port }
    }

    fn pid(&self) -> u32 {
        self.child.id()
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

#[test]
fn graceful_shutdown_lets_an_in_flight_request_finish_then_exits_cleanly() {
    let upstream_port = start_slow_upstream(Duration::from_secs(2));
    let temp = TempDir::new("finish");
    let source = PROGRAM.replace("{{UPSTREAM_PORT}}", &upstream_port.to_string());
    let out = build(&temp, &source);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let mut server = Serve::start_with_args(&temp.0.join("app.link"), &["--drain-timeout", "8s"]);
    let port = server.port;
    let pid = server.pid();

    // /ready normal antes de la señal.
    let (status, body) = request(port, "GET", "/ready", "").unwrap();
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("\"draining\":false"), "{body}");

    // Request lenta EN VUELO cuando llega la señal.
    let slow = std::thread::spawn(move || request(port, "POST", "/Slow/wait", "{}"));
    std::thread::sleep(Duration::from_millis(300));

    signal::send_graceful_shutdown(pid);

    // /ready pasa a draining=true casi de inmediato -- el proceso todavía
    // no salió (la request lenta sigue corriendo), pero ya avisa que no hay
    // que enrutarle tráfico nuevo.
    std::thread::sleep(Duration::from_millis(200));
    if let Ok((status, body)) = request(port, "GET", "/ready", "") {
        assert_eq!(status, 503, "{body}");
        assert!(body.contains("\"draining\":true"), "{body}");
    }

    // La request lenta, YA EN VUELO antes de la señal, termina con éxito --
    // el drenado la deja completar en vez de cortarla.
    let (status, body) = slow.join().unwrap().expect("la request lenta debería completar, no fallar la conexión");
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("slow-ok"), "{body}");

    // Y el proceso sale solo, con código 0, sin que este test tenga que
    // matarlo -- confirma que `serve()` realmente retorna `Ok(())` tras
    // drenar, no que se cuelga esperando para siempre.
    let status = server.child.wait().expect("esperar la salida del proceso");
    assert!(status.success(), "el proceso debería salir con código 0 tras un drenado exitoso: {status:?}");
}

#[test]
fn drain_timeout_forces_exit_even_if_a_request_is_still_running() {
    // El upstream tarda MÁS que el drain-timeout -- el drenado tiene que
    // cortar la espera igual, no colgarse para siempre.
    let upstream_port = start_slow_upstream(Duration::from_secs(6));
    let temp = TempDir::new("timeout");
    let source = PROGRAM.replace("{{UPSTREAM_PORT}}", &upstream_port.to_string());
    let out = build(&temp, &source);
    assert!(out.status.success());
    let mut server = Serve::start_with_args(&temp.0.join("app.link"), &["--drain-timeout", "1s"]);
    let port = server.port;
    let pid = server.pid();

    let slow = std::thread::spawn(move || request(port, "POST", "/Slow/wait", "{}"));
    std::thread::sleep(Duration::from_millis(300));

    let started = std::time::Instant::now();
    signal::send_graceful_shutdown(pid);

    let status = server.child.wait().expect("esperar la salida del proceso");
    let elapsed = started.elapsed();
    assert!(status.success(), "sale con código 0 aunque el drenado se agote: {status:?}");
    assert!(
        elapsed < Duration::from_secs(4),
        "el drenado tiene que cortar la espera cerca de --drain-timeout (1s), no esperar los 6s del upstream: tardó {elapsed:?}"
    );
    // La request lenta, cortada a mitad, termina en error de conexión (el
    // proceso ya cerró) -- resultado esperado, no se verifica su valor.
    let _ = slow.join();
}
