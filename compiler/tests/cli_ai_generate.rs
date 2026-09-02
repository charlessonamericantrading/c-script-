// `ai.generate`/`ai.chat` contra un modelo REAL (GRAMMAR.md §3.235, PLAN.md
// §9.20 Eje G ítem 3), a través del binario y de HTTP -- el motor embebido
// cargando un GGUF de verdad, sin Ollama ni ningún proceso externo. Mismo
// criterio que `pg_integration.rs`: se salta (no falla) si la máquina no
// tiene un modelo. `LINK_TEST_AI_MODEL` es una spec de `ai { }` (nombre de
// Ollama ya descargado, ej. `qwen2.5:0.5b`, o ruta a un .gguf); en el MSI
// de desarrollo el modelo de 0.5B carga en ~1s y responde en menos de 5s.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-ai-gen-{name}-{}-{}",
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
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

struct Serve {
    child: Child,
    port: u16,
}

impl Serve {
    fn start(link_path: &PathBuf) -> Self {
        let port = free_port();
        let child = Command::new(env!("CARGO_BIN_EXE_linkc"))
            .arg("serve")
            .arg(link_path)
            .arg(port.to_string())
            .arg("--ai-timeout")
            .arg("120s")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .expect("iniciar 'linkc serve'");
        let server = Serve { child, port };
        for _ in 0..200 {
            if server.request("GET", "/live", "").is_some() {
                return server;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("'linkc serve' no abrió el puerto {port} a tiempo");
    }

    fn request(&self, method: &str, path: &str, body: &str) -> Option<(u16, String)> {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).ok()?;
        stream.set_read_timeout(Some(Duration::from_secs(180))).ok()?;
        let req = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(req.as_bytes()).ok()?;
        let mut reader = BufReader::new(stream);
        let mut status_line = String::new();
        reader.read_line(&mut status_line).ok()?;
        let status: u16 = status_line.split_whitespace().nth(1)?.parse().ok()?;
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            let n = reader.read_line(&mut line).ok()?;
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
        reader.read_exact(&mut buf).ok()?;
        Some((status, String::from_utf8_lossy(&buf).to_string()))
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn generate_and_chat_run_a_real_local_model_through_the_embedded_engine() {
    let Ok(spec) = std::env::var("LINK_TEST_AI_MODEL") else {
        eprintln!("saltado: LINK_TEST_AI_MODEL no está definida (ej. qwen2.5:0.5b)");
        return;
    };
    let program = format!(
        r#"
ai {{ m: "{spec}" }}
service Ai {{
  rpc models() -> String[] {{ ai.models() }}
  rpc complete(p: String) -> String {{ ai.generate("m", p, 12) }}
  rpc chat(q: String) -> String {{ ai.chat("m", [AiMessage {{ role: "user", content: q }}], 24) }}
  rpc bad() -> String {{ ai.generate("nadie", "x", 4) }}
}}
"#
    );
    let temp = TempDir::new("real");
    let src = temp.write("app.link", &program);
    let server = Serve::start(&src);

    let (status, body) = server.request("POST", "/Ai/models", "{}").expect("models");
    assert_eq!(status, 200, "{body}");
    assert_eq!(body.trim(), r#"["m"]"#, "{body}");

    // Un prompt crudo cuya continuación greedy es estable en cualquier
    // modelo de lenguaje decente: la lista sigue con "4".
    let (status, body) = server.request("POST", "/Ai/complete", r#"{"p":"1, 2, 3,"}"#).expect("complete");
    assert_eq!(status, 200, "{body}");
    let text: String = serde_json::from_str(&body).expect("un String JSON");
    assert!(!text.trim().is_empty(), "el modelo tiene que generar algo: {body}");
    assert!(text.contains('4'), "la continuación de '1, 2, 3,' tiene que traer un 4: {text:?}");

    let (status, body) = server.request("POST", "/Ai/chat", r#"{"q":"Reply with the single word OK."}"#).expect("chat");
    assert_eq!(status, 200, "{body}");
    let text: String = serde_json::from_str(&body).expect("un String JSON");
    assert!(!text.trim().is_empty(), "{body}");

    let (status, body) = server.request("POST", "/Ai/bad", "{}").expect("bad");
    assert_ne!(status, 200, "{body}");
    assert!(body.contains("'nadie'") && body.contains("[m]"), "{body}");

    // El proceso sigue vivo después del error.
    let (status, _) = server.request("POST", "/Ai/models", "{}").expect("models otra vez");
    assert_eq!(status, 200);
}

/// Lee la respuesta SSE entera (Connection: close) y devuelve los JSON de
/// cada línea `data: ...`, ignorando el framing chunked (las líneas de
/// tamaño en hex nunca empiezan por `data:`).
fn read_sse(port: u16, path: &str, body: &str) -> Vec<serde_json::Value> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("conectar");
    stream.set_read_timeout(Some(Duration::from_secs(180))).unwrap();
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).unwrap();
    let mut raw = Vec::new();
    let _ = stream.read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw);
    assert!(text.starts_with("HTTP/1.1 200"), "{text}");
    text.lines().filter_map(|l| l.strip_prefix("data: ")).map(|j| serde_json::from_str(j).expect("JSON por evento")).collect()
}

#[test]
fn a_stream_whose_body_is_ai_stream_emits_one_sse_event_per_token_from_a_real_model() {
    let Ok(spec) = std::env::var("LINK_TEST_AI_MODEL") else {
        eprintln!("saltado: LINK_TEST_AI_MODEL no está definida (ej. qwen2.5:0.5b)");
        return;
    };
    let program = format!(
        r#"
ai {{ m: "{spec}" }}
service Ai {{
  stream reply(q: String) -> AiToken {{ ai.stream("m", [AiMessage {{ role: "user", content: q }}], 16) }}
  stream broken(q: String) -> AiToken {{ ai.stream("nadie", [AiMessage {{ role: "user", content: q }}], 4) }}
}}
"#
    );
    let temp = TempDir::new("stream");
    let src = temp.write("app.link", &program);
    let server = Serve::start(&src);

    let events = read_sse(server.port, "/Ai/reply", r#"{"q":"Count from one to five."}"#);
    assert!(events.len() >= 3, "al menos dos tokens y el cierre: {events:?}");
    let last = events.last().unwrap();
    assert_eq!(last["done"], true, "{events:?}");
    assert!(last.get("error").is_none(), "{events:?}");
    assert!(events[..events.len() - 1].iter().all(|e| e["done"] == false && e["token"].is_string()), "{events:?}");
    let text: String = events.iter().filter_map(|e| e["token"].as_str()).collect();
    assert!(!text.trim().is_empty(), "{events:?}");

    // Un alias desconocido: el error llega ANTES del primer token, como
    // respuesta de error normal (no un 200 con evento de error).
    let mut stream = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    let body = r#"{"q":"x"}"#;
    stream
        .write_all(format!("POST /Ai/broken HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes())
        .unwrap();
    let mut raw = Vec::new();
    let _ = stream.read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw);
    assert!(text.starts_with("HTTP/1.1 400"), "{text}");
    assert!(text.contains("\"error\"") && text.contains("'nadie'") && text.contains("[m]"), "{text}");
}
