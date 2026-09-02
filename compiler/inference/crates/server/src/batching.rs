//! Fase 23, Milestone 2: continuous-batching scheduler for the DECODE step
//! only (prefill stays direct, per-connection, unbatched — see
//! `routes.rs`'s doc comment on its decode loop). One dedicated scheduler
//! thread receives `DecodeJob`s from every connection thread's decode
//! loop, coalesces whatever's queued at each iteration into one
//! `LanguageModel::forward_step_batch` call (grouped by which model it's
//! for, since a batch only makes sense within one model's own weights),
//! and sends each sequence's result back.
//!
//! Built on `std::sync::mpsc`, not a hand-rolled `Mutex`+`Condvar` pair —
//! deliberately, per a lesson already paid for once in this codebase
//! (`tensor_core::worker_pool`'s Fase 12 fix for a real, reproducible
//! lost-wakeup: notifying a `Condvar` without holding the same `Mutex`
//! the waiter's `wait()` uses can land the notify in the gap between the
//! waiter's check and its `wait()` call, and condvars don't queue
//! notifications for a future waiter). A channel sidesteps that whole bug
//! class by construction: `Sender::send` before the receiver calls
//! `recv()`/`try_recv()` is never lost, it just sits in the channel.
//!
//! No artificial "wait a few ms to let more requests arrive" timer,
//! either — `run_scheduler_loop`'s non-blocking drain after the first
//! (blocking) job already coalesces whatever arrived while the PREVIOUS
//! batch's forward pass was running (tens of ms, per the roadmap's own
//! measurements) — that's the natural batching window continuous
//! batching relies on, not a tuned constant.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use model_core::{KvCache, LanguageModel};

/// One connection thread's request for a single decode step on one
/// sequence. `cache` is MOVED in (not borrowed) — the connection thread
/// temporarily gives up ownership for the duration of one batched call
/// and gets it back via `reply`. Moving a `KvCache` is cheap (its
/// `Vec<f32>` buffers relocate as pointer+len+cap, no data copy), so this
/// costs nothing per decode step beyond the channel send/recv itself.
pub struct DecodeJob {
    pub model: Arc<dyn LanguageModel>,
    /// Groups jobs that can share one `forward_step_batch` call — must be
    /// the same for every job whose `model` is the same loaded instance
    /// (the server can have more than one model resident at once).
    pub model_key: String,
    pub token: u32,
    pub cache: KvCache,
    /// Set by the connection thread right before `send()`. Used to derive
    /// `DecodeTiming::queue_ms` — cheap (`Instant::now()` is ~20-40ns) and
    /// always populated, not just under a debug flag, so the timing is
    /// never stale/missing when someone reaches for `SKYNET_DEBUG_BATCH_TIMING`.
    pub submitted_at: Instant,
    pub reply: Sender<(KvCache, Vec<f32>, DecodeTiming)>,
}

/// Phase-level breakdown of one decode step's round trip through the
/// scheduler, for the "Fase 24" investigation into Fase 23 M2's remaining
/// ~23% aggregate-throughput regression (see
/// `docs/ROADMAP-PERF-WAVE3.md` section 11). Two competing hypotheses,
/// unconfirmed: (a) OS thread contention among connection threads +
/// scheduler thread + `tensor_core::worker_pool`'s own threads, or (b)
/// fixed overhead in the channel round-trip itself. `queue_ms` isolates
/// time-to-dequeue (grows with (a) if many jobs are piling up waiting for
/// scheduler CPU time, independent of batch collection); `compute_ms` is
/// the actual `forward_step_batch` call; the connection thread derives
/// `return_ms` itself (total round trip minus these two) — that remainder
/// is scheduler-thread-send + OS wake-up + connection-thread-recv, which
/// isolates hypothesis (b) if it's large even at low concurrency, or
/// hypothesis (a) if it grows specifically as concurrent connections grow.
#[derive(Clone, Copy, Debug)]
pub struct DecodeTiming {
    /// Time from `submitted_at` to when the scheduler thread started
    /// processing the batch this job landed in (i.e. picked it up off the
    /// channel and began `run_batch`) — includes any time spent inside the
    /// `BATCH_COLLECT_WINDOW` waiting for siblings, which is deliberate
    /// (that's the batching benefit), not overhead.
    pub queue_ms: f64,
    /// Wall-clock time of the `forward_step_batch` call itself, shared
    /// identically across every job in the same batch.
    pub compute_ms: f64,
}

/// How long the scheduler blocks on an empty queue before looping back
/// (only relevant for checking shutdown / staying responsive when idle —
/// never adds latency to an actual request, since `recv_timeout` returns
/// immediately the moment a job arrives).
const POLL_TIMEOUT: Duration = Duration::from_millis(200);

/// Max time to keep collecting jobs into a batch after the first one
/// arrives, before dispatching whatever's been gathered so far even if
/// `max_batch_size` hasn't been reached. See `run_scheduler_loop`'s
/// comment for the empirical reasoning behind this existing at all.
const BATCH_COLLECT_WINDOW: Duration = Duration::from_millis(8);

/// A running scheduler. Cloning `sender()` into each connection thread is
/// the only interaction point — the thread itself is fire-and-forget,
/// stopped only by every `Sender` clone (including this handle's own)
/// being dropped, which ends the process anyway (server shutdown).
pub struct DecodeScheduler {
    sender: Sender<DecodeJob>,
}

impl DecodeScheduler {
    /// Spawns the scheduler thread. `max_batch_size` bounds how many jobs
    /// one `forward_step_batch` call covers — unbounded growth would let
    /// one huge batch starve latency for every request waiting behind it.
    pub fn start(max_batch_size: usize) -> Self {
        let (sender, receiver) = mpsc::channel::<DecodeJob>();
        std::thread::spawn(move || run_scheduler_loop(receiver, max_batch_size));
        Self { sender }
    }

    pub fn sender(&self) -> Sender<DecodeJob> {
        self.sender.clone()
    }
}

fn run_scheduler_loop(receiver: Receiver<DecodeJob>, max_batch_size: usize) {
    loop {
        let first = match receiver.recv_timeout(POLL_TIMEOUT) {
            Ok(job) => job,
            Err(RecvTimeoutError::Timeout) => continue,
            // Every Sender clone dropped -- server is shutting down.
            Err(RecvTimeoutError::Disconnected) => return,
        };

        // Measured empirically (real HTTP load, 8 concurrent connections):
        // an immediate non-blocking drain right after the first job
        // averaged batch size ~2.2, with 42% of batches landing at size 1
        // (pure scheduling overhead, zero sharing) -- connections' decode
        // requests arrive staggered enough under real request timing that
        // "whatever's already queued" rarely means "everyone who's ready".
        // Waiting up to BATCH_COLLECT_WINDOW past the first job (bounded,
        // small relative to one decode step's ~50-150ms compute) trades a
        // few ms of latency for materially larger, more representative
        // batches -- the standard micro-batching technique real
        // continuous-batching servers (vLLM, TGI) use for the same reason.
        let mut batch = vec![first];
        let deadline = Instant::now() + BATCH_COLLECT_WINDOW;
        while batch.len() < max_batch_size {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match receiver.recv_timeout(remaining) {
                Ok(job) => batch.push(job),
                Err(_) => break,
            }
        }

        run_batch(batch);
    }
}

/// Groups by `model_key` (usually one group in practice — the server
/// mostly serves one active model at a time) and calls
/// `forward_step_batch` once per group.
fn run_batch(jobs: Vec<DecodeJob>) {
    let dequeued_at = Instant::now();
    let mut groups: HashMap<String, Vec<DecodeJob>> = HashMap::new();
    for job in jobs {
        groups.entry(job.model_key.clone()).or_default().push(job);
    }

    for (_key, group) in groups {
        if std::env::var("SKYNET_DEBUG_BATCH_SIZES").is_ok() {
            eprintln!("[batch_size] {}", group.len());
        }
        let model = Arc::clone(&group[0].model);

        let mut tokens = Vec::with_capacity(group.len());
        let mut caches = Vec::with_capacity(group.len());
        let mut replies = Vec::with_capacity(group.len());
        let mut queue_mss = Vec::with_capacity(group.len());
        for job in group {
            tokens.push(job.token);
            caches.push(job.cache);
            replies.push(job.reply);
            queue_mss.push(dequeued_at.saturating_duration_since(job.submitted_at).as_secs_f64() * 1000.0);
        }

        let mut refs: Vec<&mut KvCache> = caches.iter_mut().collect();
        let compute_start = Instant::now();
        let all_logits = model.forward_step_batch(&mut refs, &tokens);
        let compute_ms = compute_start.elapsed().as_secs_f64() * 1000.0;
        drop(refs); // release the borrow so `caches` can move below

        for (((cache, logits), reply), queue_ms) in caches.into_iter().zip(all_logits).zip(replies).zip(queue_mss) {
            // Send failure means the connection is gone (client
            // disconnected mid-generation) -- nothing to do with that
            // sequence's result, drop it.
            let _ = reply.send((cache, logits, DecodeTiming { queue_ms, compute_ms }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use model_core::cache::CacheShape;
    use std::sync::{Barrier, Mutex};

    /// Records the batch size it was actually called with each time —
    /// lets tests observe the scheduler's real grouping behavior instead
    /// of just checking output shape.
    struct RecordingModel {
        seen_batch_sizes: Mutex<Vec<usize>>,
    }
    impl RecordingModel {
        fn new() -> Self {
            Self { seen_batch_sizes: Mutex::new(Vec::new()) }
        }
    }
    impl LanguageModel for RecordingModel {
        fn forward_step(&self, _cache: &mut KvCache, new_tokens: &[u32]) -> Vec<f32> {
            vec![new_tokens[0] as f32]
        }
        fn cache_shape(&self) -> CacheShape {
            CacheShape { n_layers: 1, n_kv_heads: 1, head_dim: 1, context_length: 8, per_layer_head_dim: None }
        }
        fn vocab_size(&self) -> usize {
            256
        }
        fn forward_step_batch(&self, caches: &mut [&mut KvCache], new_tokens: &[u32]) -> Vec<Vec<f32>> {
            self.seen_batch_sizes.lock().unwrap().push(caches.len());
            // Deliberately slow enough that concurrent submissions from
            // the test below have a real chance to queue up behind one
            // another and get coalesced into the SAME scheduler batch --
            // not just serialized one-by-one.
            std::thread::sleep(Duration::from_millis(20));
            new_tokens.iter().map(|&t| vec![t as f32]).collect()
        }
    }

    fn tiny_cache() -> KvCache {
        KvCache::new(&CacheShape { n_layers: 1, n_kv_heads: 1, head_dim: 1, context_length: 8, per_layer_head_dim: None })
    }

    #[test]
    fn sequential_submissions_each_get_their_own_correct_reply() {
        let model: Arc<dyn LanguageModel> = Arc::new(RecordingModel::new());
        let scheduler = DecodeScheduler::start(8);

        for tok in [3u32, 7, 42] {
            let (tx, rx) = mpsc::channel();
            scheduler
                .sender()
                .send(DecodeJob { model: Arc::clone(&model), model_key: "m".into(), token: tok, cache: tiny_cache(), submitted_at: Instant::now(), reply: tx })
                .unwrap();
            let (_cache, logits, timing) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(logits, vec![tok as f32], "reply for token {tok} carried the wrong logits");
            assert!(timing.compute_ms >= 0.0 && timing.queue_ms >= 0.0, "timing fields should be non-negative: {timing:?}");
        }
    }

    #[test]
    fn concurrent_submissions_get_coalesced_into_a_batch_larger_than_one() {
        let model = Arc::new(RecordingModel::new());
        let dyn_model: Arc<dyn LanguageModel> = model.clone() as Arc<dyn LanguageModel>;
        let scheduler = Arc::new(DecodeScheduler::start(8));

        const N: usize = 6;
        let barrier = Arc::new(Barrier::new(N));
        let handles: Vec<_> = (0..N)
            .map(|i| {
                let scheduler = Arc::clone(&scheduler);
                let model = Arc::clone(&dyn_model);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait(); // all N threads submit at (as close to) the same instant
                    let (tx, rx) = mpsc::channel();
                    scheduler
                        .sender()
                        .send(DecodeJob { model, model_key: "m".into(), token: i as u32, cache: tiny_cache(), submitted_at: Instant::now(), reply: tx })
                        .unwrap();
                    let (_cache, logits, _timing) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    assert_eq!(logits, vec![i as f32]);
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }

        let seen = model.seen_batch_sizes.lock().unwrap();
        let max_batch = seen.iter().copied().max().unwrap_or(0);
        let total: usize = seen.iter().sum();
        assert_eq!(total, N, "every submitted job should have been processed exactly once: {seen:?}");
        assert!(max_batch > 1, "expected at least one batch >1 from {N} concurrent submissions, got batch sizes {seen:?}");
    }

    #[test]
    fn different_model_keys_are_never_batched_together() {
        let model = Arc::new(RecordingModel::new());
        let dyn_model: Arc<dyn LanguageModel> = model.clone() as Arc<dyn LanguageModel>;
        let scheduler = Arc::new(DecodeScheduler::start(8));

        const N: usize = 6;
        let barrier = Arc::new(Barrier::new(N));
        let handles: Vec<_> = (0..N)
            .map(|i| {
                let scheduler = Arc::clone(&scheduler);
                let model = Arc::clone(&dyn_model);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    let (tx, rx) = mpsc::channel();
                    // Every job gets its OWN model_key -- no two should
                    // ever land in the same forward_step_batch call.
                    scheduler
                        .sender()
                        .send(DecodeJob { model, model_key: format!("m{i}"), token: i as u32, cache: tiny_cache(), submitted_at: Instant::now(), reply: tx })
                        .unwrap();
                    rx.recv_timeout(Duration::from_secs(5)).unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }

        let seen = model.seen_batch_sizes.lock().unwrap();
        assert!(seen.iter().all(|&sz| sz == 1), "distinct model_keys leaked into the same batch: {seen:?}");
        assert_eq!(seen.len(), N, "expected exactly {N} single-job batches, one per distinct model_key");
    }
}
