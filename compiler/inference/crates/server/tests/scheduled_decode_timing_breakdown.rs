//! Fase 24 diagnostic: phase-level timing breakdown for `DecodeScheduler`
//! under real concurrent HTTP load, to distinguish between Fase 23 M2's two
//! unconfirmed hypotheses for its ~23% aggregate-throughput regression
//! (see `docs/ROADMAP-PERF-WAVE3.md` section 11):
//!   (a) OS thread contention among connection threads + scheduler thread +
//!       `tensor_core::worker_pool`'s own threads
//!   (b) fixed overhead in the channel round-trip itself
//!
//! Spawns the real server binary with both `SKYNET_SCHEDULED_DECODE=1` and
//! `SKYNET_DEBUG_BATCH_TIMING=1`, captures its stderr (instead of the usual
//! `Stdio::null()`), fires concurrent requests, and parses the
//! `[batch_timing] queue_ms=.. compute_ms=.. return_ms=.. total_ms=..`
//! lines `routes.rs::decode_step` emits per decode step.
//!
//! `#[ignore]`d by default — same reasons as `scheduled_decode_correctness`
//! and `scheduled_decode_load_benchmark` (needs the real checkpoint, real
//! HTTP, real concurrency — not a unit test). Run:
//! `cargo test -p server --release --test scheduled_decode_timing_breakdown -- --ignored --nocapture`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use server::json::{self, Json};

const MODEL_PATH: &str = "C:/Users/repre/.ollama/models/blobs/sha256-c5396e06af294bd101b30dce59131a76d2b773e76950acc870eda801d3ab0515";
const NUM_PREDICT: usize = 48;
const PROMPTS: [&str; 8] = [
    "Explica que es una lista enlazada en una frase.",
    "Escribe un haiku sobre el otoño.",
    "¿Cuál es la capital de Francia?",
    "Cuenta hasta cinco.",
    "Describe un gato en una frase.",
    "¿Qué es la fotosíntesis?",
    "Nombra tres planetas del sistema solar.",
    "Escribe un refrán corto.",
];

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

/// One parsed `[batch_timing]` line.
#[derive(Debug, Clone, Copy)]
struct Timing {
    queue_ms: f64,
    compute_ms: f64,
    return_ms: f64,
    total_ms: f64,
}

fn parse_timing_line(line: &str) -> Option<Timing> {
    if !line.starts_with("[batch_timing]") {
        return None;
    }
    let get = |key: &str| -> Option<f64> {
        line.split_whitespace()
            .find(|tok| tok.starts_with(key))
            .and_then(|tok| tok.split('=').nth(1))
            .and_then(|v| v.parse::<f64>().ok())
    };
    Some(Timing {
        queue_ms: get("queue_ms=")?,
        compute_ms: get("compute_ms=")?,
        return_ms: get("return_ms=")?,
        total_ms: get("total_ms=")?,
    })
}

/// Spawns the server with scheduled decode + timing diagnostic on, piping
/// stderr through a background reader thread that parses `[batch_timing]`
/// lines and forwards them over `mpsc`. The receiver end is returned
/// alongside the handle so the caller can drain it after the load round.
fn spawn_server_with_timing(port: u16) -> (ServerHandle, mpsc::Receiver<Timing>) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_inference-server"))
        .args(["--port", &port.to_string(), "--model", &format!("qwen2.5:0.5b={MODEL_PATH}")])
        .env("SKYNET_SCHEDULED_DECODE", "1")
        .env("SKYNET_DEBUG_BATCH_TIMING", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn inference-server");

    let stderr = child.stderr.take().expect("stderr was piped");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines().map_while(Result::ok) {
            if let Some(t) = parse_timing_line(&line) {
                let _ = tx.send(t);
            }
        }
    });

    assert!(wait_for_port(port, Duration::from_secs(30)), "server on port {port} did not start listening in time");
    (ServerHandle { child }, rx)
}

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
    stream.set_read_timeout(Some(Duration::from_secs(120))).unwrap();
    let request = format!(
        "POST /api/generate HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).unwrap();
    stream.flush().unwrap();

    let mut raw = String::new();
    stream.read_to_string(&mut raw).expect("read response");
    let body_start = raw.find("\r\n\r\n").expect("no header/body separator") + 4;
    let parsed = json::parse(&raw[body_start..]).expect("bad JSON response");
    parsed.get("response").and_then(Json::as_str).expect("no response field").to_string()
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx]
}

fn summarize(label: &str, mut values: Vec<f64>) {
    if values.is_empty() {
        println!("{label}: no samples");
        return;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let sum: f64 = values.iter().sum();
    let avg = sum / values.len() as f64;
    println!(
        "{label}: n={} avg={:.2}ms p50={:.2}ms p90={:.2}ms max={:.2}ms",
        values.len(),
        avg,
        percentile(&values, 0.5),
        percentile(&values, 0.9),
        values.last().unwrap(),
    );
}

#[test]
#[ignore]
fn timing_breakdown_low_vs_high_concurrency() {
    // N=1 baseline first (isolates hypothesis (b): channel overhead should
    // be visible here even with zero contention), then N=8 (adds hypothesis
    // (a): if return_ms specifically grows from N=1 to N=8, that's
    // consistent with OS thread-scheduling contention on the wake-up path).
    for &concurrency in &[1usize, 8] {
        let port = 18530 + concurrency as u16;
        let (_server, rx) = spawn_server_with_timing(port);
        generate(port, "warmup"); // pay one-time GGUF parse + dequant cost

        let prompts: Vec<&str> = PROMPTS.iter().take(concurrency).copied().collect();
        let handles: Vec<_> = prompts.iter().map(|&p| std::thread::spawn(move || generate(port, p))).collect();
        for h in handles {
            h.join().unwrap();
        }

        // Drain whatever timing lines arrived (non-blocking after the round
        // is done -- the reader thread has already forwarded everything the
        // child wrote by the time all requests completed).
        let mut queue = Vec::new();
        let mut compute = Vec::new();
        let mut ret = Vec::new();
        let mut total = Vec::new();
        while let Ok(t) = rx.try_recv() {
            queue.push(t.queue_ms);
            compute.push(t.compute_ms);
            ret.push(t.return_ms);
            total.push(t.total_ms);
        }

        println!("--- concurrency={concurrency} ---");
        summarize("queue_ms", queue);
        summarize("compute_ms", compute);
        summarize("return_ms", ret);
        summarize("total_ms", total);
    }
}
