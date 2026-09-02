//! Skynet Native Worker Thread Pool.
//!
//! A lightweight, condvar-based thread pool designed specifically for
//! parallel matrix multiplication and tensor operations in `tensor-core`.
//! Replaces external dependencies like `rayon` with 100% self-contained Rust code.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

#[derive(Clone, Copy)]
pub struct SendPtr<T>(pub *mut T);
unsafe impl<T> Send for SendPtr<T> {}
unsafe impl<T> Sync for SendPtr<T> {}

struct TaskJob {
    func: Box<dyn Fn(usize, usize) + Send + Sync>,
    total_items: usize,
    chunk_size: usize,
    current_index: AtomicUsize,
    completed_items: AtomicUsize,
}

/// Active jobs live in a `Vec`, not a single slot: `global_pool()` is a
/// process-wide singleton and the HTTP server spawns one OS thread per
/// connection (`crates/server/src/bin/server.rs`), so two concurrent
/// requests can both be mid-decode and both call `par_for` at the same
/// time. A single-slot design lets the second submitter overwrite the
/// first submitter's job before any worker claims it; the first caller
/// then waits on a job no worker will ever touch again — a silent,
/// permanent hang under concurrent load. A `Vec` means a second job can
/// never evict a first one; both simply wait to be picked up.
struct PoolState {
    jobs: Vec<Arc<TaskJob>>,
    shutdown: bool,
}

pub struct WorkerPool {
    #[allow(dead_code)]
    threads: Vec<JoinHandle<()>>,
    state: Arc<(Mutex<PoolState>, Condvar)>,
    done_cond: Arc<Condvar>,
}

impl WorkerPool {
    /// Creates a worker thread pool matching available CPU cores.
    pub fn new() -> Self {
        let num_threads = thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);

        let state = Arc::new((
            Mutex::new(PoolState {
                jobs: Vec::new(),
                shutdown: false,
            }),
            Condvar::new(),
        ));
        let done_cond = Arc::new(Condvar::new());

        let mut threads = Vec::with_capacity(num_threads);

        for _ in 0..num_threads {
            let state_clone = Arc::clone(&state);
            let done_cond_clone = Arc::clone(&done_cond);

            let handle = thread::spawn(move || {
                loop {
                    // Snapshot every currently active job under the lock,
                    // then drain them without re-locking per chunk. A
                    // concurrent `par_for` call can push a new job while
                    // this snapshot is being worked through; this thread
                    // will pick it up next time it loops back here. Since
                    // there are several worker threads and each cycles
                    // back frequently (chunk-sized units), a new job is
                    // never starved for long, and never lost.
                    let jobs = {
                        let (lock, cvar) = &*state_clone;
                        let mut guard = lock.lock().unwrap();

                        while guard.jobs.is_empty() && !guard.shutdown {
                            guard = cvar.wait(guard).unwrap();
                        }

                        if guard.shutdown && guard.jobs.is_empty() {
                            break;
                        }

                        guard.jobs.clone()
                    };

                    for job in &jobs {
                        loop {
                            let idx = job.current_index.fetch_add(job.chunk_size, Ordering::SeqCst);
                            if idx >= job.total_items {
                                break;
                            }
                            let end = (idx + job.chunk_size).min(job.total_items);
                            (job.func)(idx, end);

                            let done = job.completed_items.fetch_add(end - idx, Ordering::SeqCst) + (end - idx);
                            if done >= job.total_items {
                                // Must hold the same mutex the waiter checks
                                // under, not just bump the atomic and notify
                                // freely: otherwise this can land in the gap
                                // between the waiter's `completed_items`
                                // check and its `wait()` call, where the
                                // notify finds no one listening yet and is
                                // silently dropped -- condvars don't queue
                                // notifications for a future waiter. Taking
                                // the lock here forces this to happen either
                                // before the waiter checks (so it observes
                                // "done" directly and never calls `wait`) or
                                // while the waiter is already asleep inside
                                // `wait()` (so the notify actually reaches
                                // it). Confirmed as a real, reproducible hang
                                // (not theoretical) via the pre-existing
                                // Fase-3 stress test
                                // `linear_quantized_concurrent_calls_from_many_threads_do_not_panic`.
                                let (lock, _) = &*state_clone;
                                let _guard = lock.lock().unwrap();
                                done_cond_clone.notify_all();
                            }
                        }
                    }
                }
            });

            threads.push(handle);
        }

        WorkerPool {
            threads,
            state,
            done_cond,
        }
    }

    /// Executes `func(start, end)` in parallel across worker threads for
    /// `total_items`. Safe to call concurrently from multiple threads on
    /// the same pool: each call gets its own `TaskJob` with its own
    /// counters, so concurrent callers wait on independent conditions
    /// rather than racing over shared mutable state.
    pub fn par_for<F>(&self, total_items: usize, chunk_size: usize, func: F)
    where
        F: Fn(usize, usize) + Send + Sync + 'static,
    {
        if total_items == 0 {
            return;
        }

        let job = Arc::new(TaskJob {
            func: Box::new(func),
            total_items,
            chunk_size: chunk_size.max(1),
            current_index: AtomicUsize::new(0),
            completed_items: AtomicUsize::new(0),
        });

        {
            let (lock, cvar) = &*self.state;
            let mut guard = lock.lock().unwrap();
            guard.jobs.push(Arc::clone(&job));
            cvar.notify_all();
        }

        // Wait until this specific job is completed. Other jobs'
        // completions also notify `done_cond` (one condvar shared by all
        // in-flight jobs) -- harmless, since the loop condition re-checks
        // only this job's own counter and keeps waiting if a different
        // job was the one that just finished.
        let (lock, _) = &*self.state;
        let mut guard = lock.lock().unwrap();
        while job.completed_items.load(Ordering::SeqCst) < total_items {
            guard = self.done_cond.wait(guard).unwrap();
        }
        guard.jobs.retain(|j| !Arc::ptr_eq(j, &job));
    }
}

static GLOBAL_POOL: std::sync::OnceLock<WorkerPool> = std::sync::OnceLock::new();

pub fn global_pool() -> &'static WorkerPool {
    GLOBAL_POOL.get_or_init(WorkerPool::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_worker_pool_par_for() {
        let pool = global_pool();
        let counter = Arc::new(AtomicUsize::new(0));

        let counter_clone = Arc::clone(&counter);
        pool.par_for(100, 10, move |_start, _end| {
            counter_clone.fetch_add(1, Ordering::SeqCst);
        });

        assert_eq!(counter.load(Ordering::SeqCst), 10);
    }

    /// Regression test for the single-slot design this replaced: with a
    /// single `Option<Arc<TaskJob>>` in shared state, a second concurrent
    /// `par_for` call could overwrite a first caller's job before any
    /// worker claimed it, hanging the first caller forever. Many threads
    /// hammering `par_for` concurrently (mirroring N HTTP connections each
    /// mid-decode) must all complete with the exact chunk coverage they
    /// asked for.
    #[test]
    fn test_worker_pool_concurrent_par_for_calls_do_not_starve() {
        let pool = global_pool();
        let total_calls = Arc::new(AtomicUsize::new(0));

        let handles: Vec<_> = (0..16)
            .map(|_| {
                let total_calls = Arc::clone(&total_calls);
                thread::spawn(move || {
                    for _ in 0..20 {
                        let covered = Arc::new(AtomicUsize::new(0));
                        let covered_clone = Arc::clone(&covered);
                        pool.par_for(37, 1, move |start, end| {
                            covered_clone.fetch_add(end - start, Ordering::SeqCst);
                        });
                        assert_eq!(covered.load(Ordering::SeqCst), 37);
                        total_calls.fetch_add(1, Ordering::SeqCst);
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(total_calls.load(Ordering::SeqCst), 16 * 20);
    }
}
