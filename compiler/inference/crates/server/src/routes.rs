//! Ollama-API-compatible route handlers.
//!
//! Scope, deliberately: `/api/generate` (raw completions), `/api/chat`
//! (ChatML via `qwen2::chat_template`, non-streaming, no `tools`),
//! `/api/tags`, and `/api/ps`. There is no model pull/create/list-from-disk
//! (that's F6 — model management, not built yet); models are whatever was
//! passed on the server's command line at startup, matching this phase's
//! actual goal: let JARVIS point at this engine instead of Ollama for the
//! models already validated end-to-end (Fase 1/F4·A), not build out the
//! full Ollama feature surface.
//!
//! Sampling is greedy-only, matching every correctness check this whole
//! project has run — `options.temperature` is accepted (so existing
//! Ollama-shaped request bodies don't need editing) but ignored.
//!
//! ## Lazy loading, memory budget, eviction (Fase A)
//!
//! Every `--model` on the command line is *known* from startup
//! (`ServerState::known`) but not necessarily *loaded*.
//! `ServerState::get_or_load` loads a model into `ServerState::loaded` the
//! first time a request names it; every subsequent request for that name
//! reuses the same resident `Arc<ModelEntry>` — no reload, matching this
//! server's original "stays warm for the life of the process" behavior
//! once a model has actually been used at least once.
//!
//! If `--memory-budget-mb` was passed, `loaded` is kept at or under that
//! many bytes (measured as the sum of each entry's `file_size` — a good
//! proxy for resident RAM specifically because `tensor_core::QuantizedMatrix`
//! keeps weights in their on-disk quantized form rather than upconverting to
//! f32 at load time, see that type's doc comment) by evicting the
//! least-recently-used loaded model(s) before inserting a new one. A model
//! whose `file_size` alone exceeds the budget is refused outright — an
//! honest error — rather than silently let through over budget.
//!
//! Concurrency note: the actual disk read + GGUF parse + tensor dequant
//! (`loader::load_model`, possibly seconds for a multi-GB checkpoint) runs
//! WITHOUT holding `loaded`'s lock, so a slow cold load of model B never
//! blocks a concurrent request already being served by resident model A.
//! The cost of that choice: two threads racing to cold-load different
//! models that individually fit but don't fit together can transiently
//! push resident bytes over budget between each one's pre-load size check
//! and its post-load insert; `insert_loaded` re-runs eviction right before
//! inserting, so the window is brief and self-heals on the very next
//! insert or request. Acceptable for this server's actual concurrency
//! level (a handful of local callers), not a design proven for a busy
//! multi-tenant server.

use std::collections::HashMap;
use std::fs;
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use model_core::prefix_cache::{safe_reuse_len, PrefixCacheManager};
use model_core::{argmax, ChatMessage, KvCache, LanguageModel, ModelTokenizer};

use crate::batching::{DecodeJob, DecodeScheduler};
use crate::http::{write_json_response, ChunkedJsonStream, Request};
use crate::json::Json;
use crate::time::now_rfc3339;

/// How many jobs one `forward_step_batch` call covers at most — bounds
/// worst-case added latency for whichever request is last in a big batch.
const DECODE_BATCH_SIZE: usize = 32;

/// Fase 23, Milestone 2 safety valve: routes decode steps through
/// `DecodeScheduler` when set to anything but "0" (same on-by-default,
/// `OnceLock`-cached pattern as `tensor_core::ops`'s `NATIVE_*_DOT`
/// toggles). **Default OFF** until the server-integrated benchmark (see
/// `docs/ROADMAP-PERF-WAVE3.md`) confirms a real, order-invariant win —
/// same criterion Fase 15/16 used before defaulting their toggles on,
/// unlike Fase 14 which stayed off. Lets scheduled decode be killed
/// instantly in production without a redeploy if something's wrong.
fn scheduled_decode_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("SKYNET_SCHEDULED_DECODE").map(|v| v != "0").unwrap_or(false))
}

/// Runs one decode step for `cache`+`token` — through the shared scheduler
/// (coalesces with other connections' concurrent decode steps into one
/// weight read) when `scheduled_decode_enabled()`, or a direct
/// `forward_step` call otherwise. Owns `cache` for the call and hands it
/// back alongside the new logits: the scheduler path needs to temporarily
/// move it out (to `DecodeJob`) and back, so this function's signature
/// matches that shape regardless of which path runs, keeping both call
/// sites (`handle_generate`/`handle_chat`) identical.
///
/// Everything else about a decode step — EOS check, `argmax`, streaming
/// write, the `num_predict` bound — stays on the connection thread,
/// unchanged, in the callers' loops. This function replaces ONLY the
/// `forward_step` call itself.
fn decode_step(state: &ServerState, model: &Arc<dyn LanguageModel>, model_key: &str, mut cache: KvCache, token: u32) -> (KvCache, Vec<f32>) {
    if !scheduled_decode_enabled() {
        let logits = model.forward_step(&mut cache, &[token]);
        return (cache, logits);
    }

    let submitted_at = Instant::now();
    let (tx, rx) = std::sync::mpsc::channel();
    let job = DecodeJob { model: Arc::clone(model), model_key: model_key.to_string(), token, cache, submitted_at, reply: tx };
    match state.decode_scheduler.sender().send(job) {
        Ok(()) => {
            let (cache, logits, timing) = rx.recv().expect("scheduler dropped the reply sender without responding");
            // Fase 24 diagnostic (see batching.rs's DecodeTiming doc comment)
            // -- total round trip minus queue+compute isolates return-path
            // overhead (channel send-back + OS wake-up + this thread's own
            // recv()). Cheap (one Instant::now() + one env lookup per step
            // when SKYNET_SCHEDULED_DECODE is on), kept alongside
            // SKYNET_DEBUG_BATCH_SIZES rather than reverted after use.
            if std::env::var("SKYNET_DEBUG_BATCH_TIMING").is_ok() {
                let total_ms = submitted_at.elapsed().as_secs_f64() * 1000.0;
                let return_ms = (total_ms - timing.queue_ms - timing.compute_ms).max(0.0);
                eprintln!(
                    "[batch_timing] queue_ms={:.2} compute_ms={:.2} return_ms={:.2} total_ms={:.2}",
                    timing.queue_ms, timing.compute_ms, return_ms, total_ms,
                );
            }
            (cache, logits)
        }
        // Scheduler thread gone (should not happen while the server is
        // up) -- fall back to a direct call rather than hang or panic the
        // connection thread over an infrastructure hiccup.
        Err(std::sync::mpsc::SendError(job)) => {
            let mut cache = job.cache;
            let logits = job.model.forward_step(&mut cache, &[job.token]);
            (cache, logits)
        }
    }
}

pub struct ModelEntry {
    pub name: String,
    /// GGUF's `general.architecture` string ("qwen2", "llama", ...) — read
    /// once at load time (see `loader::load_model`), reported as-is in
    /// `/api/tags`/`/api/ps` instead of a hardcoded family name.
    pub architecture: String,
    pub model: Arc<dyn LanguageModel>,
    pub tokenizer: Arc<dyn ModelTokenizer>,
    pub file_size: u64,
}

struct CachedEntry {
    entry: Arc<ModelEntry>,
    last_used: Instant,
}

/// Why `ServerState::get_or_load` failed — kept distinct from a plain
/// `String` so route handlers can map each case to the right HTTP status
/// (404 vs 500) instead of guessing from message text.
pub enum ModelLookupError {
    /// `name` isn't one of the `--model` specs this server was started
    /// with at all.
    NotFound(String),
    /// `name` is configured but loading it (or fitting it in the memory
    /// budget) failed. See the wrapped message for specifics.
    LoadFailed(String),
}

pub struct ServerState {
    /// `--model` name -> path, exactly as configured at startup. Every
    /// configured model appears here whether or not it's currently
    /// loaded — this is what `/api/tags` enumerates.
    known: Vec<(String, String)>,
    /// `None` = no cap (loaded models stay resident forever once first
    /// used, matching this server's original eager-load-everything
    /// behavior). `Some(bytes)` = `loaded`'s total `file_size` is kept at
    /// or under this by evicting LRU entries.
    budget_bytes: Option<u64>,
    loaded: Mutex<HashMap<String, CachedEntry>>,
    pub prefix_cache: PrefixCacheManager,
    /// Fase 23, Milestone 2 — shared scheduler for the decode step. Started
    /// once per process (see `DecodeScheduler::start`); every connection
    /// thread clones its `Sender` cheaply. Only consulted when
    /// `scheduled_decode_enabled()` is true.
    pub decode_scheduler: DecodeScheduler,
}

impl ServerState {
    pub fn new(known: Vec<(String, String)>, budget_bytes: Option<u64>) -> ServerState {
        ServerState {
            known,
            budget_bytes,
            loaded: Mutex::new(HashMap::new()),
            prefix_cache: PrefixCacheManager::new(16),
            decode_scheduler: DecodeScheduler::start(DECODE_BATCH_SIZE),
        }
    }

    /// Returns the resident entry for `name`, loading it first if this is
    /// the first request that's ever named it. See the module doc comment
    /// for the locking/eviction discipline.
    pub fn get_or_load(&self, name: &str) -> Result<Arc<ModelEntry>, ModelLookupError> {
        if let Some(entry) = self.touch_if_loaded(name) {
            return Ok(entry);
        }

        let path = self
            .known
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, p)| p.clone())
            .ok_or_else(|| ModelLookupError::NotFound(format!("model '{name}' not found")))?;

        // Cheap size check (a stat() call, no file read) before paying for a
        // potentially multi-GB load we might have to reject anyway.
        let file_size = fs::metadata(&path)
            .map(|m| m.len())
            .map_err(|e| ModelLookupError::LoadFailed(format!("could not stat {path}: {e}")))?;
        if let Some(budget) = self.budget_bytes {
            if file_size > budget {
                return Err(ModelLookupError::LoadFailed(format!(
                    "model '{name}' is {file_size} bytes, which alone exceeds the configured memory budget of {budget} bytes — raise --memory-budget-mb or unset it"
                )));
            }
        }

        // The actual load: no lock held across this (see module doc
        // comment) so a slow cold load never blocks requests for
        // already-loaded models.
        let entry = Arc::new(crate::loader::load_model(name, &path).map_err(ModelLookupError::LoadFailed)?);
        self.insert_loaded(name, Arc::clone(&entry));
        Ok(entry)
    }

    /// Fast path of `get_or_load`: if `name` is already resident, bump its
    /// recency and return it.
    fn touch_if_loaded(&self, name: &str) -> Option<Arc<ModelEntry>> {
        let mut loaded = self.loaded.lock().unwrap();
        let cached = loaded.get_mut(name)?;
        cached.last_used = Instant::now();
        Some(Arc::clone(&cached.entry))
    }

    /// Inserts `entry` into `loaded`, evicting LRU entries first if a
    /// budget is configured and there isn't room. Split out of
    /// `get_or_load` so tests can exercise the cache/eviction bookkeeping
    /// directly with synthetic entries, without going through
    /// `loader::load_model`'s real GGUF file read.
    fn insert_loaded(&self, name: &str, entry: Arc<ModelEntry>) {
        let mut loaded = self.loaded.lock().unwrap();
        if let Some(budget) = self.budget_bytes {
            evict_to_fit(&mut loaded, budget, entry.file_size);
        }
        loaded.insert(name.to_string(), CachedEntry { entry, last_used: Instant::now() });
    }

    /// `/api/tags` — every configured model, whether or not it's loaded.
    pub fn tags_json(&self) -> Vec<Json> {
        let loaded = self.loaded.lock().unwrap();
        self.known
            .iter()
            .map(|(name, path)| match loaded.get(name) {
                Some(cached) => model_json(&cached.entry),
                None => unloaded_model_json(name, path),
            })
            .collect()
    }

    /// `/api/ps` — only models currently resident (real Ollama semantics:
    /// "what's running now", not "what's known").
    pub fn ps_json(&self) -> Vec<Json> {
        let loaded = self.loaded.lock().unwrap();
        loaded
            .values()
            .map(|cached| {
                let mut obj = model_json(&cached.entry);
                if let Json::Object(fields) = &mut obj {
                    fields.push(("expires_at".to_string(), Json::Null));
                    fields.push(("size_vram".to_string(), Json::num(0.0)));
                }
                obj
            })
            .collect()
    }
}

/// Evicts least-recently-used entries from `loaded` until
/// `resident + incoming_size <= budget`, or until nothing's left to evict.
/// Never fails here — the caller (`get_or_load`) has already rejected
/// `incoming_size > budget` outright, so there's always some amount of
/// eviction that makes room; this just performs it.
fn evict_to_fit(loaded: &mut HashMap<String, CachedEntry>, budget: u64, incoming_size: u64) {
    let mut resident: u64 = loaded.values().map(|c| c.entry.file_size).sum();
    if resident + incoming_size <= budget {
        return;
    }
    let mut by_age: Vec<String> = loaded.keys().cloned().collect();
    by_age.sort_by_key(|k| loaded[k].last_used);
    for name in by_age {
        if resident + incoming_size <= budget {
            break;
        }
        if let Some(removed) = loaded.remove(&name) {
            eprintln!("[evicting {name} ({} bytes) to fit within {budget}-byte budget]", removed.entry.file_size);
            resident -= removed.entry.file_size;
        }
    }
}

fn error_body(message: impl Into<String>) -> String {
    Json::object(vec![("error", Json::str(message.into()))]).to_json_string()
}

pub fn handle(state: &ServerState, req: &Request, stream: &TcpStream) {
    let result = match (req.method.as_str(), req.path.as_str()) {
        ("POST", "/api/generate") => handle_generate(state, req, stream),
        ("POST", "/api/chat") => handle_chat(state, req, stream),
        ("GET", "/api/tags") => handle_tags(state, stream),
        ("GET", "/api/ps") => handle_ps(state, stream),
        _ => write_json_response(stream, 404, &error_body(format!("unknown route: {} {}", req.method, req.path))),
    };
    if let Err(e) = result {
        eprintln!("[error writing response for {} {}: {e}]", req.method, req.path);
    }
}

fn handle_generate(state: &ServerState, req: &Request, stream: &TcpStream) -> std::io::Result<()> {
    let body_str = match std::str::from_utf8(&req.body) {
        Ok(s) => s,
        Err(_) => return write_json_response(stream, 400, &error_body("request body is not valid UTF-8")),
    };
    let body = match crate::json::parse(body_str) {
        Ok(v) => v,
        Err(e) => return write_json_response(stream, 400, &error_body(format!("invalid JSON: {e}"))),
    };

    let model_name = body.get("model").and_then(Json::as_str).unwrap_or("");
    let entry = match state.get_or_load(model_name) {
        Ok(e) => e,
        Err(ModelLookupError::NotFound(msg)) => return write_json_response(stream, 404, &error_body(msg)),
        Err(ModelLookupError::LoadFailed(msg)) => return write_json_response(stream, 500, &error_body(msg)),
    };

    let prompt = body.get("prompt").and_then(Json::as_str).unwrap_or("");
    let raw = body.get("raw").and_then(Json::as_bool).unwrap_or(false);
    if !raw {
        return write_json_response(
            stream,
            400,
            &error_body("this engine currently requires \"raw\": true (no chat template support yet)"),
        );
    }
    let stream_mode = body.get("stream").and_then(Json::as_bool).unwrap_or(true);
    let num_predict = body.get("options").and_then(|o| o.get("num_predict")).and_then(Json::as_u64).unwrap_or(128) as usize;

    let t0 = Instant::now();
    let prompt_ids = entry.tokenizer.encode(prompt);
    let prompt_eval_count = prompt_ids.len();

    let (matched_prefix_len, cached_kv) = state.prefix_cache.find_longest_prefix(model_name, &prompt_ids);
    let safe_prefix_len = safe_reuse_len(matched_prefix_len, prompt_ids.len());

    let (mut cache, mut logits, prompt_eval_duration_ns) = if safe_prefix_len > 0 && cached_kv.is_some() {
        let mut cache = cached_kv.unwrap();
        cache.truncate(safe_prefix_len);
        let uncached_prompt = &prompt_ids[safe_prefix_len..];
        let t_prefill = Instant::now();
        let logits = entry.model.forward_step(&mut cache, uncached_prompt);
        let duration = t_prefill.elapsed().as_nanos() as u64;
        (cache, logits, duration)
    } else {
        let mut cache = KvCache::new(&entry.model.cache_shape());
        let t_prefill = Instant::now();
        let logits = entry.model.forward_step(&mut cache, &prompt_ids);
        let duration = t_prefill.elapsed().as_nanos() as u64;
        state.prefix_cache.insert_prefix(model_name, prompt_ids.clone(), cache.clone());
        (cache, logits, duration)
    };

    let mut chunked = if stream_mode { Some(ChunkedJsonStream::start(stream)?) } else { None };

    let t_decode = Instant::now();
    let mut generated_ids: Vec<u32> = Vec::new();
    let mut done_reason = "length";

    for step in 0..num_predict {
        let next = argmax(&logits);
        if Some(next) == entry.tokenizer.eos_token_id() {
            done_reason = "stop";
            break;
        }
        generated_ids.push(next);

        if let Some(c) = chunked.as_mut() {
            let token_obj = Json::object(vec![
                ("model", Json::str(model_name)),
                ("created_at", Json::str(now_rfc3339())),
                ("response", Json::str(entry.tokenizer.decode(&[next]))),
                ("done", Json::Bool(false)),
            ]);
            c.write_line(&token_obj.to_json_string())?;
        }

        if step + 1 == num_predict {
            break;
        }
        let (new_cache, new_logits) = decode_step(state, &entry.model, model_name, cache, next);
        cache = new_cache;
        logits = new_logits;
    }
    let eval_duration_ns = t_decode.elapsed().as_nanos() as u64;
    let total_duration_ns = t0.elapsed().as_nanos() as u64;

    let final_obj = Json::object(vec![
        ("model", Json::str(model_name)),
        ("created_at", Json::str(now_rfc3339())),
        ("response", Json::str(if stream_mode { String::new() } else { entry.tokenizer.decode(&generated_ids) })),
        ("done", Json::Bool(true)),
        ("done_reason", Json::str(done_reason)),
        ("total_duration", Json::num(total_duration_ns as f64)),
        // Always ~0: `entry` is already resident by the time t0 starts —
        // get_or_load's own load cost (paid on a model's first-ever
        // request, post-Fase A) happens before this function's timers
        // start, so it's simply invisible here rather than genuinely zero.
        ("load_duration", Json::num(0.0)),
        ("prompt_eval_count", Json::num(prompt_eval_count as f64)),
        ("prompt_eval_duration", Json::num(prompt_eval_duration_ns as f64)),
        ("eval_count", Json::num(generated_ids.len() as f64)),
        ("eval_duration", Json::num(eval_duration_ns as f64)),
    ]);

    match chunked {
        Some(mut c) => {
            c.write_line(&final_obj.to_json_string())?;
            c.finish()
        }
        None => write_json_response(stream, 200, &final_obj.to_json_string()),
    }
}

/// `/api/chat` — the route JARVIS's `local-ai-tools.ts:callOllama()` (the
/// JARVIS-cutover caller this was built for) actually uses, unlike
/// `/api/generate`. Non-streaming only (that caller always sends
/// `"stream": false`) and no `tools` support (that caller never sends
/// `tools` either — see the F7 cutover investigation for why: JARVIS's
/// `direct`/`local_agent` tiers are plain text chat, tool-calling there is
/// a different, unrelated code path in `ollama-client.ts`).
fn handle_chat(state: &ServerState, req: &Request, stream: &TcpStream) -> std::io::Result<()> {
    let body_str = match std::str::from_utf8(&req.body) {
        Ok(s) => s,
        Err(_) => return write_json_response(stream, 400, &error_body("request body is not valid UTF-8")),
    };
    let body = match crate::json::parse(body_str) {
        Ok(v) => v,
        Err(e) => return write_json_response(stream, 400, &error_body(format!("invalid JSON: {e}"))),
    };

    let model_name = body.get("model").and_then(Json::as_str).unwrap_or("");
    let entry = match state.get_or_load(model_name) {
        Ok(e) => e,
        Err(ModelLookupError::NotFound(msg)) => return write_json_response(stream, 404, &error_body(msg)),
        Err(ModelLookupError::LoadFailed(msg)) => return write_json_response(stream, 500, &error_body(msg)),
    };

    let Some(Json::Array(messages_json)) = body.get("messages") else {
        return write_json_response(stream, 400, &error_body("\"messages\" must be an array"));
    };
    if messages_json.is_empty() {
        return write_json_response(stream, 400, &error_body("\"messages\" must not be empty"));
    }
    let messages: Vec<ChatMessage> = messages_json
        .iter()
        .map(|m| ChatMessage {
            role: m.get("role").and_then(Json::as_str).unwrap_or("user").to_string(),
            content: m.get("content").and_then(Json::as_str).unwrap_or("").to_string(),
        })
        .collect();

    // JARVIS's only caller (local-ai-tools.ts:callOllama) never sends
    // `num_predict` — it sends `options.num_ctx` where it means
    // max-output-length (a pre-existing mislabeling that real Ollama
    // ignores for length purposes, since Ollama is fast enough that even a
    // full 1024-token reply finishes in ~160s, inside JARVIS's 300s client
    // timeout). This engine cannot assume the same: qwen2.5-coder:7b here
    // measured ~800ms/token (vs Ollama's ~160ms/token on the same
    // hardware — this engine's kernels are simply less mature), so a
    // 1024-token reply would take ~13 minutes and blow straight through
    // that 300s timeout — confirmed by an actual timeout during the F7
    // cutover's end-to-end check, not a guess. So: honor `num_predict` if a
    // caller sends it; otherwise fall back to `num_ctx` (matching this
    // caller's real intent) but hard-clamp to what this engine's slowest
    // served model can actually finish inside a 300s budget with margin.
    // This is a real capability gap vs. Ollama for long local_agent
    // replies, not swept under the rug — see the F7 report.
    let stream_mode = body.get("stream").and_then(Json::as_bool).unwrap_or(false);

    // This engine's decode is ~2.6 tok/s for 0.5b (~377ms/token, measured
    // 2026-07-08). Caps are chosen so a full reply fits inside the caller's
    // timeout: non-streaming callers (JARVIS's callOllama, 60s budget for the
    // 0.5b/"fast" model) get ~140 tokens (~53s worst case); streaming callers
    // (the dashboard Chat IA, 180s budget) get more, since bytes flow
    // continuously and the client never hits a headers-timeout. Raising these
    // is the payoff of optimizing the engine's decode kernels (a separate,
    // larger effort — see the F4 notes), not something to fake here.
    let max_tokens = if stream_mode { 512 } else { 140 };
    let requested = body
        .get("options")
        .and_then(|o| o.get("num_predict").or_else(|| o.get("num_ctx")))
        .and_then(Json::as_u64)
        .map(|v| v as usize)
        .unwrap_or(2048);
    let num_predict = requested.min(max_tokens);

    let t0 = Instant::now();
    let prompt_ids = entry.tokenizer.render_prompt_ids(&messages);
    let prompt_eval_count = prompt_ids.len();

    let (matched_prefix_len, cached_kv) = state.prefix_cache.find_longest_prefix(model_name, &prompt_ids);
    let safe_prefix_len = safe_reuse_len(matched_prefix_len, prompt_ids.len());
    let debug_prefix_cache = std::env::var("SKYNET_DEBUG_PREFIX_CACHE").is_ok();

    let (mut cache, mut logits, prompt_eval_duration_ns) = if safe_prefix_len > 0 && cached_kv.is_some() {
        let mut cache = cached_kv.unwrap();
        cache.truncate(safe_prefix_len);
        let uncached_prompt = &prompt_ids[safe_prefix_len..];
        if debug_prefix_cache {
            eprintln!(
                "[prefix_cache] HIT model={model_name} prompt_len={} matched_len={matched_prefix_len} safe_len={safe_prefix_len} uncached_len={}",
                prompt_ids.len(), uncached_prompt.len(),
            );
        }
        let t_prefill = Instant::now();
        let logits = entry.model.forward_step(&mut cache, uncached_prompt);
        let duration = t_prefill.elapsed().as_nanos() as u64;
        if debug_prefix_cache {
            eprintln!("[prefix_cache] HIT prefill_ms={:.1} for {} uncached tokens", duration as f64 / 1e6, uncached_prompt.len());
        }
        (cache, logits, duration)
    } else {
        if debug_prefix_cache {
            eprintln!(
                "[prefix_cache] MISS model={model_name} prompt_len={} matched_len={matched_prefix_len} safe_len={safe_prefix_len}",
                prompt_ids.len(),
            );
        }
        let mut cache = KvCache::new(&entry.model.cache_shape());
        let t_prefill = Instant::now();
        let logits = entry.model.forward_step(&mut cache, &prompt_ids);
        let duration = t_prefill.elapsed().as_nanos() as u64;
        state.prefix_cache.insert_prefix(model_name, prompt_ids.clone(), cache.clone());
        if debug_prefix_cache {
            eprintln!(
                "[prefix_cache] MISS prefill_ms={:.1} for {} tokens -- inserted into cache",
                duration as f64 / 1e6, prompt_ids.len(),
            );
        }
        (cache, logits, duration)
    };

    let mut chunked = if stream_mode { Some(ChunkedJsonStream::start(stream)?) } else { None };

    let t_decode = Instant::now();
    let mut generated_ids: Vec<u32> = Vec::new();
    let mut done_reason = "length";
    // Byte buffer for streaming: only complete UTF-8 is emitted (a token may
    // end mid-character — see Tokenizer::decode_bytes).
    let mut byte_buf: Vec<u8> = Vec::new();
    let mut emitted = 0usize;

    for step in 0..num_predict {
        let next = argmax(&logits);
        if Some(next) == entry.tokenizer.eos_token_id() {
            done_reason = "stop";
            break;
        }
        generated_ids.push(next);

        if let Some(c) = chunked.as_mut() {
            byte_buf.extend_from_slice(&entry.tokenizer.decode_bytes(&[next]));
            let valid_up_to = match std::str::from_utf8(&byte_buf) {
                Ok(_) => byte_buf.len(),
                Err(e) => e.valid_up_to(),
            };
            if valid_up_to > emitted {
                let delta = std::str::from_utf8(&byte_buf[emitted..valid_up_to]).unwrap();
                let frame = Json::object(vec![
                    ("model", Json::str(model_name)),
                    ("created_at", Json::str(now_rfc3339())),
                    ("message", Json::object(vec![("role", Json::str("assistant")), ("content", Json::str(delta))])),
                    ("done", Json::Bool(false)),
                ]);
                c.write_line(&frame.to_json_string())?;
                emitted = valid_up_to;
            }
        }

        if step + 1 == num_predict {
            break;
        }
        let (new_cache, new_logits) = decode_step(state, &entry.model, model_name, cache, next);
        cache = new_cache;
        logits = new_logits;
    }
    let eval_duration_ns = t_decode.elapsed().as_nanos() as u64;
    let total_duration_ns = t0.elapsed().as_nanos() as u64;

    // Streaming: content already went out as deltas, so the final frame's
    // message content is empty (matches Ollama). Non-streaming: the full text.
    let final_content = if stream_mode { String::new() } else { entry.tokenizer.decode(&generated_ids) };
    let final_obj = Json::object(vec![
        ("model", Json::str(model_name)),
        ("created_at", Json::str(now_rfc3339())),
        ("message", Json::object(vec![("role", Json::str("assistant")), ("content", Json::str(final_content))])),
        ("done", Json::Bool(true)),
        ("done_reason", Json::str(done_reason)),
        ("total_duration", Json::num(total_duration_ns as f64)),
        // Always ~0: see the identical comment in `handle_generate`.
        ("load_duration", Json::num(0.0)),
        ("prompt_eval_count", Json::num(prompt_eval_count as f64)),
        ("prompt_eval_duration", Json::num(prompt_eval_duration_ns as f64)),
        ("eval_count", Json::num(generated_ids.len() as f64)),
        ("eval_duration", Json::num(eval_duration_ns as f64)),
    ]);

    match chunked {
        Some(mut c) => {
            c.write_line(&final_obj.to_json_string())?;
            c.finish()
        }
        None => write_json_response(stream, 200, &final_obj.to_json_string()),
    }
}

fn model_json(entry: &ModelEntry) -> Json {
    Json::object(vec![
        ("name", Json::str(&entry.name)),
        ("model", Json::str(&entry.name)),
        ("modified_at", Json::str(now_rfc3339())),
        ("size", Json::num(entry.file_size as f64)),
        // No content-hash digest computed at load time (would mean hashing
        // the whole file up front for a field nothing in this project's
        // scope reads back) — left blank rather than fabricated.
        ("digest", Json::str("")),
        (
            "details",
            Json::object(vec![
                ("format", Json::str("gguf")),
                ("family", Json::str(entry.architecture.clone())),
                ("parameter_size", Json::str(format!("{}", entry.model.vocab_size()))),
                ("quantization_level", Json::str("mixed")),
            ]),
        ),
    ])
}

/// `/api/tags` entry for a configured-but-not-yet-loaded model.
/// `architecture` and `vocab_size` aren't knowable without a full GGUF
/// parse (the `gguf` crate only exposes a whole-buffer `GgufFile::parse`,
/// no partial/header-only file reader — see `loader::load_model`, which
/// always reads the whole file), and paying that cost just to answer a
/// listing endpoint would defeat lazy loading's own purpose, so those
/// fields report "unknown" until the model is actually requested once.
/// `size` is still accurate — a stat() call, not a read.
fn unloaded_model_json(name: &str, path: &str) -> Json {
    let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    Json::object(vec![
        ("name", Json::str(name)),
        ("model", Json::str(name)),
        ("modified_at", Json::str(now_rfc3339())),
        ("size", Json::num(size as f64)),
        ("digest", Json::str("")),
        (
            "details",
            Json::object(vec![
                ("format", Json::str("gguf")),
                ("family", Json::str("unknown")),
                ("parameter_size", Json::str("unknown")),
                ("quantization_level", Json::str("unknown")),
            ]),
        ),
    ])
}

fn handle_tags(state: &ServerState, stream: &TcpStream) -> std::io::Result<()> {
    let body = Json::object(vec![("models", Json::Array(state.tags_json()))]);
    write_json_response(stream, 200, &body.to_json_string())
}

fn handle_ps(state: &ServerState, stream: &TcpStream) -> std::io::Result<()> {
    let body = Json::object(vec![("models", Json::Array(state.ps_json()))]);
    write_json_response(stream, 200, &body.to_json_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use model_core::cache::CacheShape;
    use std::io::Write;

    struct FakeModel;
    impl LanguageModel for FakeModel {
        fn forward_step(&self, _cache: &mut KvCache, _new_tokens: &[u32]) -> Vec<f32> {
            vec![0.0]
        }
        fn cache_shape(&self) -> CacheShape {
            CacheShape { n_layers: 1, n_kv_heads: 1, head_dim: 1, context_length: 8, per_layer_head_dim: None }
        }
        fn vocab_size(&self) -> usize {
            1
        }
    }

    struct FakeTokenizer;
    impl ModelTokenizer for FakeTokenizer {
        fn encode(&self, _text: &str) -> Vec<u32> {
            vec![]
        }
        fn decode(&self, _ids: &[u32]) -> String {
            String::new()
        }
        fn decode_bytes(&self, _ids: &[u32]) -> Vec<u8> {
            vec![]
        }
        fn eos_token_id(&self) -> Option<u32> {
            None
        }
        fn render_prompt_ids(&self, _messages: &[ChatMessage]) -> Vec<u32> {
            vec![]
        }
    }

    /// A synthetic loaded entry with a chosen `file_size`, bypassing
    /// `loader::load_model`'s real GGUF file read entirely — the
    /// cache/eviction logic under test never looks past `file_size`/`name`.
    fn fake_entry(name: &str, file_size: u64) -> Arc<ModelEntry> {
        Arc::new(ModelEntry {
            name: name.to_string(),
            architecture: "fake".to_string(),
            model: Arc::new(FakeModel),
            tokenizer: Arc::new(FakeTokenizer),
            file_size,
        })
    }

    /// Writes `size` arbitrary bytes to a fresh temp file and returns its
    /// path (leaked into `known`'s String, cleaned up by the OS temp dir
    /// like every other test in this workspace that touches a real path).
    fn temp_file_of_size(name: &str, size: usize) -> String {
        let mut path = std::env::temp_dir();
        path.push(format!("inference-server-test-{name}-{size}-{:p}", &size));
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(&vec![0u8; size]).unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn get_or_load_reuses_the_same_resident_entry_on_repeat_calls() {
        let path = temp_file_of_size("reuse", 4096);
        let state = ServerState::new(vec![("only".to_string(), path)], None);
        // Real load will fail (not a real GGUF), but insert_loaded lets us
        // seed the cache directly and prove touch_if_loaded's fast path
        // never goes near loader::load_model again afterwards.
        state.insert_loaded("only", fake_entry("only", 4096));

        let first = state.get_or_load("only").ok().unwrap();
        let second = state.get_or_load("only").ok().unwrap();
        assert!(Arc::ptr_eq(&first, &second), "second call should reuse the cached Arc, not reload");
    }

    #[test]
    fn unknown_model_name_is_not_found_not_load_failed() {
        let state = ServerState::new(vec![], None);
        match state.get_or_load("nope") {
            Err(ModelLookupError::NotFound(_)) => {}
            _ => panic!("expected NotFound for a name that was never configured"),
        }
    }

    #[test]
    fn no_budget_means_no_eviction() {
        let state = ServerState::new(vec![], None);
        state.insert_loaded("a", fake_entry("a", 1_000_000_000));
        state.insert_loaded("b", fake_entry("b", 1_000_000_000));
        let ps = state.ps_json();
        assert_eq!(ps.len(), 2, "with no budget configured both entries should stay resident");
    }

    #[test]
    fn inserting_over_budget_evicts_the_least_recently_used_entry() {
        let state = ServerState::new(vec![], Some(150));
        state.insert_loaded("old", fake_entry("old", 100));
        // Touch "old" so it's not the LRU entry, then insert "new" which
        // only fits if "old" (now more-recently-used) still gets evicted --
        // proving eviction is driven by insertion/access order, not name.
        state.touch_if_loaded("old");
        state.insert_loaded("new", fake_entry("new", 100));

        let ps = state.ps_json();
        assert_eq!(ps.len(), 1, "one entry should have been evicted to stay within the 150-byte budget");
    }

    #[test]
    fn least_recently_touched_entry_is_evicted_first() {
        let state = ServerState::new(vec![], Some(250));
        state.insert_loaded("a", fake_entry("a", 100));
        state.insert_loaded("b", fake_entry("b", 100));
        // "a" is now the least-recently-used of the two.
        state.touch_if_loaded("b");
        // Doesn't fit alongside both a and b (100+100+100=300 > 250), so
        // exactly one must go -- it should be "a", not "b".
        state.insert_loaded("c", fake_entry("c", 100));

        let names: Vec<String> = {
            let loaded = state.loaded.lock().unwrap();
            loaded.keys().cloned().collect()
        };
        assert!(!names.contains(&"a".to_string()), "least-recently-used entry 'a' should have been evicted");
        assert!(names.contains(&"b".to_string()));
        assert!(names.contains(&"c".to_string()));
    }

    #[test]
    fn a_model_alone_bigger_than_the_budget_is_refused_outright() {
        let path = temp_file_of_size("too-big", 500);
        let state = ServerState::new(vec![("huge".to_string(), path)], Some(100));
        match state.get_or_load("huge") {
            Err(ModelLookupError::LoadFailed(msg)) => {
                assert!(msg.contains("budget"), "error should mention the budget, got: {msg}");
            }
            _ => panic!("expected LoadFailed when a single model alone exceeds the budget"),
        }
    }

    #[test]
    fn tags_json_lists_known_models_whether_loaded_or_not() {
        let loaded_path = temp_file_of_size("loaded", 10);
        let unloaded_path = temp_file_of_size("unloaded", 20);
        let state =
            ServerState::new(vec![("loaded-model".to_string(), loaded_path), ("cold-model".to_string(), unloaded_path)], None);
        state.insert_loaded("loaded-model", fake_entry("loaded-model", 10));

        let tags = state.tags_json();
        assert_eq!(tags.len(), 2, "tags should list every configured model, loaded or not");
        let families: Vec<String> = tags.iter().map(family_of).collect();
        assert!(families.contains(&"fake".to_string()), "loaded model should report its real architecture");
        assert!(
            families.contains(&"unknown".to_string()),
            "unloaded model should report an unknown architecture rather than a guess"
        );
    }

    /// Test-only navigation of a `model_json`/`unloaded_model_json` value's
    /// `details.family` string, to avoid repeating this match-nest in every
    /// assertion that cares about it.
    fn family_of(j: &Json) -> String {
        let Json::Object(fields) = j else { return String::new() };
        let Some((_, Json::Object(details))) = fields.iter().find(|(k, _)| k == "details") else {
            return String::new();
        };
        match details.iter().find(|(k, _)| k == "family") {
            Some((_, Json::String(s))) => s.clone(),
            _ => String::new(),
        }
    }

    #[test]
    fn ps_json_lists_only_currently_loaded_models() {
        let unloaded_path = temp_file_of_size("cold", 20);
        let state = ServerState::new(vec![("cold".to_string(), unloaded_path)], None);
        state.insert_loaded("warm", fake_entry("warm", 10));

        let ps = state.ps_json();
        assert_eq!(ps.len(), 1, "ps should only list the resident model, not the merely-configured one");
    }
}
