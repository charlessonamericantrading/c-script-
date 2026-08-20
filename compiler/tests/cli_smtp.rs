// `smtp.send(to, subject, body)` (GRAMMAR.md §3.43): un módulo builtin para
// mandar email por SMTP, leyendo la conexión (`LINK_SMTP_URL`) y el
// remitente (`LINK_SMTP_FROM`) del ENTORNO del proceso, nunca de argumentos
// del rpc -- así un `.link` no puede hardcodear ni filtrar credenciales de
// un relay, y ningún caller puede spoofear el remitente con datos de la
// request.
//
// Se prueba contra un servidor SMTP de mentira escrito a mano en este mismo
// archivo -- sin depender de nada externo al binario de test: lo único que
// necesita es hablar lo mínimo del protocolo (EHLO/MAIL FROM/RCPT TO/DATA)
// para que `lettre` complete un envío real de punta a punta, hablando SMTP
// de verdad sobre un TcpStream real.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

const PROGRAM: &str = r#"
service Sys {
  rpc notify(to: String, subject: String, body: String) -> String {
    smtp.send(to, subject, body);
    "enviado"
  }
}
"#;

#[derive(Debug)]
struct ReceivedMail {
    mail_from: String,
    rcpt_to: String,
    data: String,
}

/// Servidor SMTP mínimo: acepta conexiones y hace lo justo del protocolo
/// (EHLO, MAIL FROM, RCPT TO, DATA) para que un cliente real complete un
/// envío -- no pretende ser un servidor SMTP completo, solo lo suficiente
/// para confirmar que `smtp.send` habla el protocolo de verdad, no un mock
/// interno.
struct FakeSmtp {
    port: u16,
    rx: Receiver<ReceivedMail>,
}

impl FakeSmtp {
    fn start() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bindear puerto efímero");
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let tx = tx.clone();
                std::thread::spawn(move || handle_one_connection(stream, tx));
            }
        });
        FakeSmtp { port, rx }
    }

    fn recv(&self, timeout: Duration) -> Option<ReceivedMail> {
        self.rx.recv_timeout(timeout).ok()
    }
}

fn handle_one_connection(stream: TcpStream, tx: Sender<ReceivedMail>) {
    let mut writer = stream.try_clone().expect("clonar el stream");
    let mut reader = BufReader::new(stream);
    let _ = write!(writer, "220 fake-smtp ready\r\n");

    let mut mail_from = String::new();
    let mut rcpt_to = String::new();
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let cmd = line.trim();
        let upper = cmd.to_ascii_uppercase();
        if upper.starts_with("EHLO") || upper.starts_with("HELO") {
            let _ = write!(writer, "250 hola\r\n");
        } else if upper.starts_with("MAIL FROM:") {
            mail_from = cmd["MAIL FROM:".len()..].trim().to_string();
            let _ = write!(writer, "250 OK\r\n");
        } else if upper.starts_with("RCPT TO:") {
            rcpt_to = cmd["RCPT TO:".len()..].trim().to_string();
            let _ = write!(writer, "250 OK\r\n");
        } else if upper.starts_with("DATA") {
            let _ = write!(writer, "354 adelante\r\n");
            let mut data = String::new();
            loop {
                let mut data_line = String::new();
                if reader.read_line(&mut data_line).unwrap_or(0) == 0 {
                    break;
                }
                if data_line.trim_end_matches(['\r', '\n']) == "." {
                    break;
                }
                data.push_str(&data_line);
            }
            let _ = write!(writer, "250 OK: mensaje aceptado\r\n");
            let _ = tx.send(ReceivedMail { mail_from: mail_from.clone(), rcpt_to: rcpt_to.clone(), data });
        } else if upper.starts_with("QUIT") {
            let _ = write!(writer, "221 bye\r\n");
            return;
        } else {
            let _ = write!(writer, "250 OK\r\n");
        }
    }
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "linkc-smtp-{name}-{}-{}",
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
            use std::io::Read;
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
    fn start(link_path: &PathBuf, extra_env: &[(&str, &str)]) -> Self {
        let port = free_port();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_linkc"));
        cmd.arg("serve").arg(link_path).arg(port.to_string()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        let child = cmd.spawn().expect("iniciar 'linkc serve'");
        wait_for_port(port);
        Serve { child, port }
    }

    fn post(&self, path: &str, body: &str) -> (u16, String) {
        use std::io::Read;
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
fn smtp_send_delivers_a_real_message_over_a_real_smtp_conversation() {
    let smtp = FakeSmtp::start();
    let temp = TempDir::new("send");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(
        &src,
        &[("LINK_SMTP_URL", &format!("smtp://127.0.0.1:{}", smtp.port)), ("LINK_SMTP_FROM", "remitente@example.com")],
    );

    let (status, body) =
        server.post("/Sys/notify", r#"{"to":"destino@example.com","subject":"Hola desde c-script","body":"Cuerpo del mensaje."}"#);
    assert_eq!(status, 200, "body: {body}");
    assert_eq!(body, "\"enviado\"");

    let mail = smtp.recv(Duration::from_secs(5)).expect("el servidor SMTP de mentira debió recibir un mensaje");
    assert_eq!(mail.mail_from, "<remitente@example.com>", "mail_from: {mail:?}");
    assert_eq!(mail.rcpt_to, "<destino@example.com>", "rcpt_to: {mail:?}");
    assert!(mail.data.contains("Subject: Hola desde c-script"), "data: {}", mail.data);
    assert!(mail.data.contains("Cuerpo del mensaje."), "data: {}", mail.data);
}

#[test]
fn smtp_send_without_link_smtp_url_fails_cleanly_not_with_a_panic() {
    let temp = TempDir::new("no-url");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &[]);

    let (status, body) = server.post("/Sys/notify", r#"{"to":"a@example.com","subject":"x","body":"y"}"#);
    assert_eq!(status, 500, "body: {body}");
    assert!(body.contains("LINK_SMTP_URL"), "el mensaje debe nombrar la variable que falta: {body}");
}

#[test]
fn smtp_send_without_link_smtp_from_fails_cleanly() {
    let smtp = FakeSmtp::start();
    let temp = TempDir::new("no-from");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(&src, &[("LINK_SMTP_URL", &format!("smtp://127.0.0.1:{}", smtp.port))]);

    let (status, body) = server.post("/Sys/notify", r#"{"to":"a@example.com","subject":"x","body":"y"}"#);
    assert_eq!(status, 500, "body: {body}");
    assert!(body.contains("LINK_SMTP_FROM"), "el mensaje debe nombrar la variable que falta: {body}");
}

#[test]
fn smtp_send_with_an_invalid_recipient_address_fails_cleanly() {
    let smtp = FakeSmtp::start();
    let temp = TempDir::new("bad-address");
    let src = temp.write("app.link", PROGRAM);
    let server = Serve::start(
        &src,
        &[("LINK_SMTP_URL", &format!("smtp://127.0.0.1:{}", smtp.port)), ("LINK_SMTP_FROM", "remitente@example.com")],
    );

    let (status, body) = server.post("/Sys/notify", r#"{"to":"esto-no-es-un-email","subject":"x","body":"y"}"#);
    assert_eq!(status, 500, "body: {body}");
    assert!(!body.contains("panicked"), "una dirección inválida es un error normal, no un panic: {body}");
}

#[test]
fn smtp_send_against_an_unreachable_host_fails_cleanly_not_with_a_panic() {
    let temp = TempDir::new("unreachable");
    let src = temp.write("app.link", PROGRAM);
    // Puerto libre, nada escuchando -- simula el relay caído.
    let dead_port = free_port();
    let server =
        Serve::start(&src, &[("LINK_SMTP_URL", &format!("smtp://127.0.0.1:{dead_port}")), ("LINK_SMTP_FROM", "remitente@example.com")]);

    let (status, body) = server.post("/Sys/notify", r#"{"to":"a@example.com","subject":"x","body":"y"}"#);
    assert_eq!(status, 500, "body: {body}");
    assert!(!body.contains("panicked"), "una conexión caída es una condición operativa normal, no un panic: {body}");
}
