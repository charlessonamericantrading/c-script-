//! `inference-server` — an Ollama-API-compatible HTTP server (see
//! `server::routes` for exactly what's implemented) fronting this
//! project's own from-scratch inference engine (Qwen2, Llama 3.x, Gemma4,
//! Phi-3, Qwen3/Qwen3MoE, PhiMoE — see `server::loader::load_model` for
//! the registered-architectures list). Point JARVIS's base URL at this
//! instead of `localhost:11434` and its existing HTTP client code needs
//! no changes, for the routes this server actually supports.
//!
//! Usage:
//!   inference-server --port 11500 --model qwen2.5:0.5b
//!   inference-server --port 11500 --model "qwen2.5:0.5b=C:\path\to\model.gguf"
//!   inference-server --port 11500 --model a --model b --memory-budget-mb 8192
//!
//! `--model NAME` (no `=`) auto-resolves NAME against Ollama's own manifest
//! storage (`server::ollama_resolve`) — the common case, since this server
//! is meant to sit next to an existing Ollama installation and serve models
//! already pulled there. `--model NAME=PATH` bypasses that and points
//! directly at a GGUF file, for models not registered with Ollama at all.
//!
//! Every `--model` is *configured* at startup (this process checks the
//! file exists and fails loudly if not — a typo'd path shouldn't wait
//! until the first request to surface) but not *loaded* — the actual GGUF
//! read + tensor dequant happens lazily, on that model's first request,
//! and the result stays resident for the life of the process from then on
//! (see `server::routes`' module doc comment for the full lazy-
//! loading/eviction design). `--memory-budget-mb` is optional; when unset,
//! nothing is ever evicted (every model that's been requested at least
//! once just stays warm, matching this server's original behavior before
//! lazy loading existed — the only thing that changed is *when* the load
//! happens). When set, resident models are kept within that budget by
//! evicting the least-recently-used one(s) before loading a new one.

use std::env;
use std::fs;
use std::net::{TcpListener, TcpStream};
use std::process::ExitCode;
use std::sync::Arc;

use server::http::read_request;
use server::ollama_resolve::resolve_blob_path;
use server::routes::{self, ServerState};

struct Args {
    port: u16,
    models: Vec<(String, String)>,
    memory_budget_bytes: Option<u64>,
}

fn parse_args() -> Result<Args, String> {
    let mut port = 11500u16;
    let mut models = Vec::new();
    let mut memory_budget_bytes = None;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--port" => {
                let v = args.next().ok_or("--port needs a value")?;
                port = v.parse().map_err(|_| format!("invalid --port value: {v}"))?;
            }
            "--model" => {
                let v = args.next().ok_or("--model needs a value: either NAME or NAME=PATH")?;
                let (name, path) = match v.split_once('=') {
                    Some((name, path)) => (name.to_string(), path.to_string()),
                    None => {
                        let blob = resolve_blob_path(&v).map_err(|e| format!("could not resolve model '{v}' via Ollama: {e}"))?;
                        (v.clone(), blob.to_string_lossy().into_owned())
                    }
                };
                models.push((name, path));
            }
            "--memory-budget-mb" => {
                let v = args.next().ok_or("--memory-budget-mb needs a value")?;
                let mb: u64 = v.parse().map_err(|_| format!("invalid --memory-budget-mb value: {v}"))?;
                memory_budget_bytes = Some(mb * 1024 * 1024);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    if models.is_empty() {
        return Err("at least one --model NAME (or NAME=PATH) is required".to_string());
    }
    Ok(Args { port, models, memory_budget_bytes })
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: {e}");
            eprintln!("usage: inference-server --port 11500 --model \"NAME=PATH\" [--model \"NAME=PATH\" ...] [--memory-budget-mb N]");
            return ExitCode::FAILURE;
        }
    };

    // Cheap existence check only (a stat() call per model, no file read) --
    // full GGUF parsing/tensor loading is deferred to each model's first
    // request. Still fails loudly at startup for the common mistake (a
    // typo'd path) rather than only on first use.
    for (name, path) in &args.models {
        if let Err(e) = fs::metadata(path) {
            eprintln!("error: model '{name}' at {path}: {e}");
            return ExitCode::FAILURE;
        }
    }

    let addr = format!("127.0.0.1:{}", args.port);
    let listener = match TcpListener::bind(&addr) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("error: could not bind {addr}: {e}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!("[listening on http://{addr}]");
    for (name, _) in &args.models {
        eprintln!("  model configured: {name} (loads on first request)");
    }
    if let Some(budget) = args.memory_budget_bytes {
        eprintln!("  memory budget: {} MB", budget / 1024 / 1024);
    }

    let state = Arc::new(ServerState::new(args.models, args.memory_budget_bytes));

    for incoming in listener.incoming() {
        let stream = match incoming {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[accept error: {e}]");
                continue;
            }
        };
        let state = Arc::clone(&state);
        std::thread::spawn(move || handle_connection(&state, stream));
    }

    ExitCode::SUCCESS
}

fn handle_connection(state: &ServerState, stream: TcpStream) {
    let peer = stream.peer_addr().map(|a| a.to_string()).unwrap_or_else(|_| "?".to_string());
    let req = match read_request(&stream) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[{peer}] request read error: {e}");
            return;
        }
    };
    eprintln!("[{peer}] {} {}", req.method, req.path);
    routes::handle(state, &req, &stream);
}
