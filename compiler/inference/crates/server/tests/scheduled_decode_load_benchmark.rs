//! Fase 23, Milestone 2 — real HTTP load benchmark: aggregate tokens/s
//! across N concurrent `/api/generate` requests with `SKYNET_SCHEDULED_DECODE`
//! ON vs OFF, against the real server binary (not the M1 in-process
//! microbenchmark, which only timed the decode loop in isolation without
//! prefill, HTTP, or connection overhead). Interleaved A/B, round order
//! alternated (per the Fase 18 lesson: a "consistent" win that flips sign
//! under reversal is thermal/system bias, not a real effect). Reports
//! aggregate throughput (never mixed with single-connection latency, per
//! the roadmap's own explicit rule) AND a separate N=1 latency check to
//! confirm the scheduler's channel round-trip doesn't regress the common
//! case.
//!
//! `#[ignore]`d by default — same reasons as `scheduled_decode_correctness`.
//! Run: `cargo test -p server --release --test
//! scheduled_decode_load_benchmark -- --ignored --nocapture`.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command};
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

fn spawn_server(port: u16, scheduled_decode: bool) -> ServerHandle {
    let child = Command::new(env!("CARGO_BIN_EXE_inference-server"))
        .args(["--port", &port.to_string(), "--model", &format!("qwen2.5:0.5b={MODEL_PATH}")])
        .env("SKYNET_SCHEDULED_DECODE", if scheduled_decode { "1" } else { "0" })
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("failed to spawn inference-server");
    assert!(wait_for_port(port, Duration::from_secs(30)), "server on port {port} did not start listening in time");
    ServerHandle { child }
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

/// Fires all `PROMPTS` concurrently against `port`, returns wall-clock ms.
fn concurrent_round(port: u16) -> u128 {
    let start = Instant::now();
    let handles: Vec<_> = PROMPTS.iter().map(|&p| std::thread::spawn(move || generate(port, p))).collect();
    for h in handles {
        h.join().unwrap();
    }
    start.elapsed().as_millis()
}

#[test]
#[ignore]
fn scheduled_decode_aggregate_throughput_vs_baseline() {
    // Held for their Drop impl (kills the child process at end of scope),
    // not read directly.
    let _off = spawn_server(18520, false);
    let _on = spawn_server(18521, true);
    // Warm both (first request pays GGUF parse + tensor dequant, ~seconds
    // for this checkpoint) so the timed rounds below measure steady-state.
    generate(18520, "warmup");
    generate(18521, "warmup");

    const ROUNDS: usize = 5;
    let total_tokens = (PROMPTS.len() * NUM_PREDICT) as f64;
    println!("round,order,off_ms,on_ms,off_tok_s,on_tok_s");
    let mut ratios = Vec::new();
    for r in 0..ROUNDS {
        let normal = r % 2 == 0;
        // Which SERVER runs first alternates by round; the resulting
        // (off_ms, on_ms) labeling is unambiguous either way since each
        // branch names its own results explicitly.
        let (off_ms, on_ms) = if normal {
            let off_ms = concurrent_round(18520);
            let on_ms = concurrent_round(18521);
            (off_ms, on_ms)
        } else {
            let on_ms = concurrent_round(18521);
            let off_ms = concurrent_round(18520);
            (off_ms, on_ms)
        };
        let off_tok_s = total_tokens / (off_ms as f64 / 1000.0);
        let on_tok_s = total_tokens / (on_ms as f64 / 1000.0);
        ratios.push(on_tok_s / off_tok_s);
        println!("{r},{},{off_ms},{on_ms},{off_tok_s:.1},{on_tok_s:.1}", if normal { "normal" } else { "reversed" });
    }
    let avg_ratio = ratios.iter().sum::<f64>() / ratios.len() as f64;
    println!("avg on/off throughput ratio: {avg_ratio:.3}");

    // -- Single-connection (N=1) latency check: scheduler's channel
    // round-trip must not meaningfully regress the common case.
    const N1_ROUNDS: usize = 5;
    let mut off_n1 = Vec::new();
    let mut on_n1 = Vec::new();
    for r in 0..N1_ROUNDS {
        if r % 2 == 0 {
            off_n1.push(time_single(18520));
            on_n1.push(time_single(18521));
        } else {
            on_n1.push(time_single(18521));
            off_n1.push(time_single(18520));
        }
    }
    let off_n1_avg = off_n1.iter().sum::<u128>() as f64 / off_n1.len() as f64;
    let on_n1_avg = on_n1.iter().sum::<u128>() as f64 / on_n1.len() as f64;
    println!("N=1 latency: off_ms_avg={off_n1_avg:.0} on_ms_avg={on_n1_avg:.0} (on/off ratio={:.3})", on_n1_avg / off_n1_avg);
}

fn time_single(port: u16) -> u128 {
    let start = Instant::now();
    generate(port, PROMPTS[0]);
    start.elapsed().as_millis()
}
