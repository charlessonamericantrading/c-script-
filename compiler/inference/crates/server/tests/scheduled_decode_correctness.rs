//! Fase 23, Milestone 2 — end-to-end correctness check through the REAL
//! `inference-server` binary and REAL HTTP, not just unit-level plumbing.
//!
//! Sampling in this server is greedy-only (see `routes.rs`'s module doc
//! comment — `options.temperature` is accepted but ignored), so output is
//! fully deterministic for a given prompt+model: `SKYNET_SCHEDULED_DECODE`
//! must never change what text comes back, only how the decode steps
//! that produce it get grouped internally. This test proves that bar:
//! K different prompts, run SEQUENTIALLY with the toggle OFF (one
//! connection at a time — the reference, unchanged code path) vs run
//! CONCURRENTLY with the toggle ON (K threads at once, against the real
//! scheduler) — text must match prompt-for-prompt.
//!
//! `#[ignore]`d by default: needs a real qwen2.5:0.5b GGUF on disk and
//! spawns real server processes on real ports — not hermetic, matching
//! how every other real-checkpoint check in this project (`diff-
//! snapshot.mjs`) already stays outside the default `cargo test` run.
//! Run explicitly: `cargo test -p server --release --test
//! scheduled_decode_correctness -- --ignored --nocapture`.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use server::json::{self, Json};

const MODEL_PATH: &str = "C:/Users/repre/.ollama/models/blobs/sha256-c5396e06af294bd101b30dce59131a76d2b773e76950acc870eda801d3ab0515";
const PROMPTS: [&str; 4] =
    ["Explica que es una lista enlazada en una frase.", "Escribe un haiku sobre el otoño.", "¿Cuál es la capital de Francia?", "Cuenta hasta cinco."];
const NUM_PREDICT: usize = 24;

struct ServerHandle {
    child: Child,
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn wait_for_port(port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn spawn_server(port: u16, scheduled_decode: bool) -> ServerHandle {
    let child = Command::new(env!("CARGO_BIN_EXE_inference-server"))
        .args(["--port", &port.to_string(), "--model", &format!("qwen2.5:0.5b={MODEL_PATH}")])
        .env("SKYNET_SCHEDULED_DECODE", if scheduled_decode { "1" } else { "0" })
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("failed to spawn inference-server -- is the binary built? (cargo build -p server --release)");
    assert!(wait_for_port(port, Duration::from_secs(30)), "server on port {port} did not start listening in time");
    ServerHandle { child }
}

/// Minimal raw HTTP/1.1 POST -- this server's own `http.rs` is equally
/// hand-rolled on the receiving end, no client library needed.
fn generate(port: u16, prompt: &str) -> String {
    let body = Json::object(vec![
        ("model", Json::str("qwen2.5:0.5b")),
        ("prompt", Json::str(prompt)),
        ("raw", Json::Bool(true)),
        ("stream", Json::Bool(false)),
        ("options", Json::object(vec![("num_predict", Json::num(NUM_PREDICT as f64))])),
    ])
    .to_json_string();

    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    let request = format!(
        "POST /api/generate HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).unwrap();
    stream.flush().unwrap();

    let mut raw = String::new();
    stream.read_to_string(&mut raw).expect("read response");
    let body_start = raw.find("\r\n\r\n").expect("no header/body separator in response") + 4;
    let response_body = &raw[body_start..];
    let parsed = json::parse(response_body).unwrap_or_else(|e| panic!("bad JSON response: {e:?}\nraw body: {response_body}"));
    parsed.get("response").and_then(Json::as_str).unwrap_or_else(|| panic!("no \"response\" field in: {response_body}")).to_string()
}

#[test]
#[ignore]
fn scheduled_decode_matches_sequential_reference_prompt_for_prompt() {
    // -- Reference: toggle OFF, one connection at a time (today's behavior, unchanged).
    let reference_port = 18500;
    let reference = spawn_server(reference_port, false);
    let expected: Vec<String> = PROMPTS.iter().map(|p| generate(reference_port, p)).collect();
    drop(reference);

    for (prompt, text) in PROMPTS.iter().zip(expected.iter()) {
        assert!(!text.is_empty(), "reference run produced empty text for prompt {prompt:?} -- test setup is broken, not the thing under test");
    }

    // -- Scheduled: toggle ON, all K prompts fired concurrently against the real scheduler.
    let scheduled_port = 18501;
    let scheduled = spawn_server(scheduled_port, true);
    let handles: Vec<_> = PROMPTS
        .iter()
        .map(|&prompt| std::thread::spawn(move || (prompt, generate(scheduled_port, prompt))))
        .collect();
    let mut actual: std::collections::HashMap<&str, String> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    drop(scheduled);

    for (prompt, expected_text) in PROMPTS.iter().zip(expected.iter()) {
        let actual_text = actual.remove(prompt).unwrap_or_else(|| panic!("no concurrent response recorded for prompt {prompt:?}"));
        assert_eq!(&actual_text, expected_text, "prompt {prompt:?}: scheduled-concurrent output diverged from sequential-unscheduled reference");
    }
}
