//! Forward-pass kernels. `linear` operates on fully dequantized `Matrix`
//! weights (Fase 1's approach — correctness first); `linear_quantized` is
//! Fase 2's memory-efficient counterpart, consuming `QuantizedMatrix`
//! weights block-at-a-time. The three dot-product/SAXPY-shaped hot loops
//! (`linear_quantized`'s per-block accumulate, `causal_gqa_attention`'s
//! Q·K, and its P·V accumulation) go through `simd::dot_product`/
//! `simd::axpy_inplace`, which auto-detect AVX2 and fall back to scalar —
//! everything else here (RMSNorm, softmax, RoPE, SiLU) is still plain
//! scalar, since none of them were the bottleneck this pass.

use crate::dequant::{dequantize_block_into, MAX_BLOCK_ELEMS};
use crate::matrix::Matrix;
use crate::qmatrix::QuantizedMatrix;
use crate::quant::{dot_q4_k_super_block, dot_q5_0_q8_block, dot_q6_k_super_block, dot_q8_0_q8_block, quantize_row_q8, QBLOCK};
use crate::simd::{axpy_inplace, dot_product, sum_squared, vec_add_inplace, vec_mul_inplace, vec_scale_inplace};
use gguf::GgmlType;

// Shared by `linear_quantized_into` and `linear_quantized_dual_into` (Fase
// 21) -- hoisted to module level so the two dispatch functions can't drift
// out of sync on the threading threshold. See `linear_quantized_into`'s own
// doc comment for the re-measurement history behind this value.
const MIN_WORK_FOR_THREADING: usize = 2_000_000;
const OVERPARTITION_FACTOR: usize = 4;

/// `y = x @ w^T (+ bias)`, i.e. a standard `nn.Linear`: `x` is `(seq, in)`,
/// `w` is `(out, in)` — GGUF/ggml's own convention for a 2-D weight tensor,
/// where each of the `out` rows is `in` contiguous elements (see
/// `gguf::tensor_info` for why that's the right way to read the file's
/// dimension order). Returns `(seq, out)`.
pub fn linear(x: &Matrix, w: &Matrix, bias: Option<&[f32]>) -> Matrix {
    assert_eq!(x.cols, w.cols, "linear: input width {} != weight in-features {}", x.cols, w.cols);
    if let Some(b) = bias {
        assert_eq!(b.len(), w.rows, "linear: bias length != out-features");
    }

    let mut out = Matrix::zeros(x.rows, w.rows);
    for s in 0..x.rows {
        let xr = x.row(s);
        let out_row = out.row_mut(s);
        for o in 0..w.rows {
            let wr = w.row(o);
            let mut acc = 0f32;
            for i in 0..x.cols {
                acc += xr[i] * wr[i];
            }
            out_row[o] = acc + bias.map_or(0.0, |b| b[o]);
        }
    }
    out
}

/// The Fase 2 counterpart to `linear`: same `y = x @ w^T (+ bias)`
/// semantics, but `w` stays in its on-disk quantized form the whole time —
/// no `Matrix` of dequantized f32 weights ever exists. For a 0.5B model
/// that's the difference between ~400MB resident (the GGUF file's own
/// size) and multiple GB (every quantized weight expanded to 4 bytes/value)
/// — see `qwen2::model` for the loader that switched over to this.
///
/// Loop order is deliberate: for each output feature, every quantized
/// block along that row is dequantized *once* into a reusable stack buffer
/// and then dotted against every sequence position that needs it, rather
/// than re-dequantizing the same block once per position. A block never
/// exists as a heap allocation — only `out` (the actual result) does.
///
/// Parallelized across output features (`w.rows`) — each output feature's
/// row is independent of every other, so this is embarrassingly parallel.
/// `out` is `(seq, out_features)` row-major, which means a *column* range
/// (an output-feature range) isn't a contiguous slice of `out.data` — so
/// each thread computes its assigned range into its own small dense
/// `Matrix` and the results get copied into `out`'s columns afterward
/// rather than writing into disjoint slices of one shared buffer directly.
/// That copy is O(seq × out_features) — negligible next to the O(seq ×
/// out_features × in_features) multiply-adds it's copying the result of.
/// Inicializa el pool global de rayon exactamente una vez por proceso.
/// `ThreadPoolBuilder::build_global()` entra en pánico si se llama dos
/// veces — importante dado el lazy/LRU model loading del server (commit


pub fn linear_quantized(x: &Matrix, w: &QuantizedMatrix, bias: Option<&[f32]>) -> Matrix {
    let mut out = Matrix::zeros(x.rows, w.rows);
    linear_quantized_into(x, w, bias, &mut out);
    out
}

/// Same as `linear_quantized`, but writes into a caller-provided `out`
/// instead of allocating a fresh `Matrix` on every call. Fase 7 of the
/// optimization roadmap: `forward_step` calls this ~7 times per layer
/// (Q/K/V/O + gate/up/down); a scratch buffer reused across all layers of
/// one `forward_step` call turns ~168 large allocations per decoded token
/// (7 matmuls × 24 layers, qwen2's layer count) into a handful allocated
/// once before the layer loop. `out` must already be `(x.rows, w.rows)` —
/// deliberately NOT auto-resized here, since a silent resize would defeat
/// the point (the caller is expected to size it once, outside any loop).
///
/// Fase 19: the thread-pool path used to allocate one `Matrix::zeros`
/// "partial" per chunk (48 on this hardware — `Matrix::zeros` both
/// reserves *and* zero-fills memory the kernel overwrites entirely right
/// after), plus the `Vec<Matrix>` holding them, plus a final copy-back
/// loop from every partial into `out`. Each chunk now writes straight into
/// `out`'s own buffer instead: every chunk owns a column range `[start,
/// end)` of `w.rows` that is disjoint from every other chunk's, so handing
/// each worker a raw pointer to the *shared* `out.data` plus its own
/// `[start, end)` is safe by construction — same disjoint-region argument
/// as the read-side `x`/`w`/`bias` raw-pointer idiom just below, applied
/// to the write side. Eliminates the 48 allocations, the zeroing, and the
/// copy-back loop in one go.
pub fn linear_quantized_into(x: &Matrix, w: &QuantizedMatrix, bias: Option<&[f32]>, out: &mut Matrix) {
    assert_eq!(x.cols, w.cols, "linear_quantized: input width {} != weight in-features {}", x.cols, w.cols);
    if let Some(b) = bias {
        assert_eq!(b.len(), w.rows, "linear_quantized: bias length != out-features");
    }
    assert_eq!((out.rows, out.cols), (x.rows, w.rows), "linear_quantized_into: out shape must be (x.rows, w.rows)");

    // Below this many total multiply-adds, dispatching to the thread pool
    // costs more than it saves. Carried forward unchanged from the
    // spawn/join and rayon eras, but re-measured (not just inherited)
    // after the Fase 12 swap to the native worker pool: interleaved A/B
    // on qwen2.5:0.5b (smallest matmuls, where this threshold matters
    // most) against 50_000 (3 rounds: 88-98ms/tok here vs 125-137ms/tok
    // there, ~40-55% worse) and against 5_000_000 (3 rounds: 89-97ms/tok
    // here vs 101-107ms/tok there, ~8-15% worse) both showed this value
    // is a local optimum, not an accident of history -- the condvar/mutex
    // dispatch in the new pool costs more per call than rayon's
    // work-stealing did, so shrinking this threshold (more, smaller
    // parallel dispatches) is a regression here where it might not have
    // been for rayon.
    let total_work = x.rows * w.rows * w.cols;

    // Quantize `x` ONCE here, before any dispatch, when the native-dot
    // path will actually use it -- not inside linear_quantized_range,
    // which (when threading is active, the common case for any matmul
    // big enough to matter) runs once per thread-pool chunk. Before this,
    // each of those chunks re-quantized the same `x` from scratch:
    // `available_parallelism() * OVERPARTITION_FACTOR` (48 on this
    // machine) redundant quantizations of identical data per single
    // matmul call, discovered profiling why measured decode time was
    // still ~5-6x above an isolated-kernel roofline estimate.
    let x_quantized_owned: Option<Vec<crate::quant::QuantizedRow>> =
        native_dot_active_for(w.ty).then(|| (0..x.rows).map(|s| quantize_row_q8(x.row(s))).collect());

    if total_work < MIN_WORK_FOR_THREADING {
        let out_stride = out.cols;
        let out_ptr = out.data.as_mut_ptr();
        linear_quantized_range(x, w, bias, 0, w.rows, out_ptr, out_stride, x_quantized_owned.as_deref());
        return;
    }

    let available = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let n_chunks = (available * OVERPARTITION_FACTOR).clamp(1, w.rows.max(1));

    let chunk = w.rows.div_ceil(n_chunks);
    let ranges: Vec<(usize, usize)> = (0..w.rows).step_by(chunk).map(|start| (start, (start + chunk).min(w.rows))).collect();

    // `out_addr`/`out_stride` below: raw pointer to `out`'s own backing
    // buffer, not a per-chunk scratch copy. Safe because `ranges` above
    // partitions `[0, w.rows)` into disjoint, non-overlapping pieces — two
    // chunks never write the same `(row, column)` cell of `out`, which is
    // exactly the property `test_linear_quantized_into_threaded_path_matches_dense_reference`
    // below exists to catch a regression in (an off-by-one here would
    // silently corrupt or drop output columns, not just race on them).
    let out_stride = out.cols;
    let out_addr = out.data.as_mut_ptr() as usize;
    let n_ranges = ranges.len();
    let x_addr = x as *const Matrix as usize;
    let w_addr = w as *const QuantizedMatrix as usize;
    let bias_info = bias.map(|b| (b.as_ptr() as usize, b.len()));
    // Same raw-pointer-across-threads idiom as x_addr/w_addr/bias_info
    // above: `x_quantized_owned` outlives this call (par_for is
    // synchronous, blocks until every chunk completes), so the pointer
    // never dangles.
    let xq_info = x_quantized_owned.as_ref().map(|v| (v.as_ptr() as usize, v.len()));

    // Fase 19 follow-up: `ranges` moves into the closure directly instead
    // of being cloned first -- `n_ranges` above is captured by value
    // (a `usize`, Copy) so `par_for`'s length argument doesn't need
    // `ranges` to still be alive outside the closure.
    crate::worker_pool::global_pool().par_for(n_ranges, 1, move |i_start, i_end| {
        let x_ref = unsafe { &*(x_addr as *const Matrix) };
        let w_ref = unsafe { &*(w_addr as *const QuantizedMatrix) };
        let bias_ref = bias_info.map(|(p, len)| unsafe { std::slice::from_raw_parts(p as *const f32, len) });
        let xq_ref = xq_info.map(|(p, len)| unsafe { std::slice::from_raw_parts(p as *const crate::quant::QuantizedRow, len) });
        let out_ptr = out_addr as *mut f32;

        for i in i_start..i_end {
            let (start, end) = ranges[i];
            linear_quantized_range(x_ref, w_ref, bias_ref, start, end, out_ptr, out_stride, xq_ref);
        }
    });
}

/// Fase 21: fuses two `linear_quantized_into` calls against the SAME input
/// `x` (the FFN `gate`/`up` projections of a SwiGLU-style block, which both
/// read the post-attention-residual hidden state and produce
/// `(x.rows, w1.rows)`/`(x.rows, w2.rows)` outputs — see each architecture's
/// `forward.rs`, e.g. `h += down(silu(gate(f)) * up(f))`). Two independent
/// calls to `linear_quantized_into` do this correctly today but pay for it
/// twice: `x` gets quantized twice (identical result both times, since
/// `quantize_row_q8` doesn't depend on the weight matrix at all) and the
/// thread pool gets dispatched twice (`par_for`'s own per-dispatch
/// synchronization cost, ~31µs measured — see the roadmap's bandwidth
/// investigation), instead of once for the combined work.
///
/// `w1`/`w2` are NOT required to share a quantization type or row count —
/// each chunk still calls the correct `linear_quantized_range` for its own
/// matrix — but they must share `x.cols` as their `.cols` (both project the
/// same input vector), which is guaranteed by construction for gate/up.
#[allow(clippy::too_many_arguments)]
pub fn linear_quantized_dual_into(
    x: &Matrix,
    w1: &QuantizedMatrix,
    w2: &QuantizedMatrix,
    bias1: Option<&[f32]>,
    bias2: Option<&[f32]>,
    out1: &mut Matrix,
    out2: &mut Matrix,
) {
    assert_eq!(x.cols, w1.cols, "linear_quantized_dual: input width {} != w1 in-features {}", x.cols, w1.cols);
    assert_eq!(x.cols, w2.cols, "linear_quantized_dual: input width {} != w2 in-features {}", x.cols, w2.cols);
    if let Some(b) = bias1 {
        assert_eq!(b.len(), w1.rows, "linear_quantized_dual: bias1 length != w1 out-features");
    }
    if let Some(b) = bias2 {
        assert_eq!(b.len(), w2.rows, "linear_quantized_dual: bias2 length != w2 out-features");
    }
    assert_eq!((out1.rows, out1.cols), (x.rows, w1.rows), "linear_quantized_dual: out1 shape must be (x.rows, w1.rows)");
    assert_eq!((out2.rows, out2.cols), (x.rows, w2.rows), "linear_quantized_dual: out2 shape must be (x.rows, w2.rows)");

    // One shared quantization of `x`, reused for whichever of w1/w2 (or
    // both — the common case, gate/up are almost always the same ggml
    // type in practice) actually needs it. `quantize_row_q8` doesn't take
    // the weight type as input, so the SAME `Vec<QuantizedRow>` is valid
    // input to both `linear_quantized_range` calls below regardless of
    // whether w1's type matches w2's.
    let need_q = native_dot_active_for(w1.ty) || native_dot_active_for(w2.ty);
    let x_quantized_owned: Option<Vec<crate::quant::QuantizedRow>> =
        need_q.then(|| (0..x.rows).map(|s| quantize_row_q8(x.row(s))).collect());
    let xq1 = x_quantized_owned.as_deref().filter(|_| native_dot_active_for(w1.ty));
    let xq2 = x_quantized_owned.as_deref().filter(|_| native_dot_active_for(w2.ty));

    let total_work = x.rows * (w1.rows + w2.rows) * x.cols;
    if total_work < MIN_WORK_FOR_THREADING {
        let out1_stride = out1.cols;
        let out1_ptr = out1.data.as_mut_ptr();
        let out2_stride = out2.cols;
        let out2_ptr = out2.data.as_mut_ptr();
        linear_quantized_range(x, w1, bias1, 0, w1.rows, out1_ptr, out1_stride, xq1);
        linear_quantized_range(x, w2, bias2, 0, w2.rows, out2_ptr, out2_stride, xq2);
        return;
    }

    // Two disjoint chunk lists, one per matrix — each entry says which
    // matrix it belongs to so the closure below can route it to the right
    // `linear_quantized_range`/`out` pair. Chunked exactly like the
    // single-matrix path (`available_parallelism() * OVERPARTITION_FACTOR`
    // chunks EACH, not shared between w1/w2), so this stays correct even
    // when w1.rows != w2.rows.
    let available = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let n_chunks1 = (available * OVERPARTITION_FACTOR).clamp(1, w1.rows.max(1));
    let n_chunks2 = (available * OVERPARTITION_FACTOR).clamp(1, w2.rows.max(1));
    let chunk1 = w1.rows.div_ceil(n_chunks1);
    let chunk2 = w2.rows.div_ceil(n_chunks2);

    #[derive(Clone, Copy)]
    enum Target { First, Second }
    let mut ranges: Vec<(Target, usize, usize)> = (0..w1.rows)
        .step_by(chunk1)
        .map(|start| (Target::First, start, (start + chunk1).min(w1.rows)))
        .collect();
    ranges.extend((0..w2.rows).step_by(chunk2).map(|start| (Target::Second, start, (start + chunk2).min(w2.rows))));
    let n_ranges = ranges.len();

    // Same raw-pointer-across-threads idiom as `linear_quantized_into` —
    // `Target::First`/`Target::Second` chunks each write only into their
    // own `out1`/`out2` buffer, and within one buffer the `[start, end)`
    // ranges built above are disjoint (same argument as the single-matrix
    // path), so no two chunks — same-target or cross-target — ever write
    // the same `(row, column)` cell of either output.
    let out1_stride = out1.cols;
    let out1_addr = out1.data.as_mut_ptr() as usize;
    let out2_stride = out2.cols;
    let out2_addr = out2.data.as_mut_ptr() as usize;
    let x_addr = x as *const Matrix as usize;
    let w1_addr = w1 as *const QuantizedMatrix as usize;
    let w2_addr = w2 as *const QuantizedMatrix as usize;
    let bias1_info = bias1.map(|b| (b.as_ptr() as usize, b.len()));
    let bias2_info = bias2.map(|b| (b.as_ptr() as usize, b.len()));
    let xq1_info = xq1.map(|s| (s.as_ptr() as usize, s.len()));
    let xq2_info = xq2.map(|s| (s.as_ptr() as usize, s.len()));

    crate::worker_pool::global_pool().par_for(n_ranges, 1, move |i_start, i_end| {
        let x_ref = unsafe { &*(x_addr as *const Matrix) };
        let w1_ref = unsafe { &*(w1_addr as *const QuantizedMatrix) };
        let w2_ref = unsafe { &*(w2_addr as *const QuantizedMatrix) };
        let bias1_ref = bias1_info.map(|(p, len)| unsafe { std::slice::from_raw_parts(p as *const f32, len) });
        let bias2_ref = bias2_info.map(|(p, len)| unsafe { std::slice::from_raw_parts(p as *const f32, len) });
        let xq1_ref = xq1_info.map(|(p, len)| unsafe { std::slice::from_raw_parts(p as *const crate::quant::QuantizedRow, len) });
        let xq2_ref = xq2_info.map(|(p, len)| unsafe { std::slice::from_raw_parts(p as *const crate::quant::QuantizedRow, len) });
        let out1_ptr = out1_addr as *mut f32;
        let out2_ptr = out2_addr as *mut f32;

        for i in i_start..i_end {
            let (target, start, end) = ranges[i];
            match target {
                Target::First => linear_quantized_range(x_ref, w1_ref, bias1_ref, start, end, out1_ptr, out1_stride, xq1_ref),
                Target::Second => linear_quantized_range(x_ref, w2_ref, bias2_ref, start, end, out2_ptr, out2_stride, xq2_ref),
            }
        }
    });
}

/// Fase 9 del roadmap de optimizacion: toggle para el kernel nativo Q5_0,
/// leido una sola vez (mismo patron OnceLock que `ensure_thread_pool`).
/// Activo por defecto tras validacion exhaustiva (ver hidden-marinating-
/// thimble.md); `NATIVE_Q5_0_DOT=0` lo desactiva sin necesidad de
/// revertir codigo si algo va mal en produccion.
fn native_q5_0_dot_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("NATIVE_Q5_0_DOT").map(|v| v != "0").unwrap_or(true))
}

/// Fase 10: mismo patron de toggle, para el kernel nativo Q4_K.
/// `NATIVE_Q4_K_DOT=0` lo desactiva.
fn native_q4_k_dot_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("NATIVE_Q4_K_DOT").map(|v| v != "0").unwrap_or(true))
}

/// Fase 14 shipped this kernel widening-to-i32 (simpler to verify first,
/// deliberate), defaulted OFF because the A/B benchmark was inconclusive
/// (~-3.7% on average, noisy, no consistent direction). Fase 14's own
/// follow-up note called for a maddubs-based rewrite before trusting a
/// default -- done: same abs/sign trick as Q5_0/Q8_0, but at 128-bit (16
/// elements) rather than 256-bit (32) width, because Q6_K's two adjacent
/// 16-element sub-blocks always carry DIFFERENT scales (`sc[is]` vs
/// `sc[is+2]`), so packing them under one 256-bit maddubs call with one
/// scale multiply would be mathematically wrong, not just imprecise --
/// 128-bit width respects the format's actual granularity. Re-benchmarked
/// on coder:7b (26.7% Q6_K): 5 interleaved rounds, all 5 faster (-8.9%,
/// -6.8%, -0.6%, -12.6%, -5.0%), averaging ~6.8% -- consistent, unlike the
/// old kernel's mixed result. Now defaults to `true`, same as every other
/// native-dot toggle. `NATIVE_Q6_K_DOT=0` disables it if needed.
fn native_q6_k_dot_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("NATIVE_Q6_K_DOT").map(|v| v != "0").unwrap_or(true))
}

/// Fase 15: toggle para el kernel nativo Q8_0. Empieza en `true` (como
/// Q5_0/Q4_K, a diferencia de Q6_K): mismo truco maddubs+abs/sign ya
/// probado por Q5_0, sin desempaquetado de nibbles -- formato mas simple
/// de todos. A diferencia de Q6_K, el benchmark A/B intercalado en
/// qwen2.5:0.5b (37.3% Q8_0, el checkpoint objetivo) SI confirmo mejora
/// real: 5 rondas, -6.7%, -17.3%, -5.3%, -0.4%, +0.1% (mas rapido en las 4
/// primeras, esencialmente plano en la 5ª -- ninguna ronda con regresion
/// clara, a diferencia del patron mixto de Q6_K), promedio ~6.1% mas
/// rapido. `NATIVE_Q8_0_DOT=0` lo desactiva si hiciera falta.
fn native_q8_0_dot_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("NATIVE_Q8_0_DOT").map(|v| v != "0").unwrap_or(true))
}

/// Whether `linear_quantized_range` will take one of the 4 native-dot
/// branches below for `ty` -- single source of truth for "does this call
/// need a pre-quantized `x`", used by `linear_quantized_into` to decide
/// whether to quantize `x` up front (once per matmul) instead of leaving
/// each native branch to quantize it internally (which, before this,
/// happened once per THREAD-POOL CHUNK -- up to `available_parallelism()
/// * OVERPARTITION_FACTOR` times per single matmul call, not once, since
/// `linear_quantized_range` runs once per chunk under the hood).
fn native_dot_active_for(ty: GgmlType) -> bool {
    match ty {
        GgmlType::Q5_0 => native_q5_0_dot_enabled(),
        GgmlType::Q8_0 => native_q8_0_dot_enabled(),
        GgmlType::Q4K => native_q4_k_dot_enabled(),
        GgmlType::Q6K => native_q6_k_dot_enabled(),
        _ => false,
    }
}

/// Computes output features `[o_start, o_end)` into `out_ptr`, a raw
/// pointer to the first element of the *real, full-width* output buffer
/// (row-major, `out_stride` == `w.rows` elements per row) -- not a
/// compact per-chunk buffer. Both `o` (the loop variable, global) and
/// `bias` are indexed globally; `out_ptr`/`out_stride` let every write
/// land directly at `out_ptr[s * out_stride + o]` with no local-to-global
/// column translation. Caller (`linear_quantized_into`) guarantees
/// `[o_start, o_end)` is disjoint from whatever range any concurrently-
/// running call is writing, which is what makes writing through a raw
/// pointer sound here without any synchronization.
///
/// `x_quantized`, when the native-dot path is active for `w.ty`
/// (`native_dot_active_for`), must be `Some` with one `QuantizedRow` per
/// row of `x` -- computed ONCE by the caller (`linear_quantized_into`,
/// before any thread-pool dispatch), not here: this function runs once
/// per thread-pool chunk when threading is active, so quantizing `x`
/// inside it would repeat that work `available_parallelism() *
/// OVERPARTITION_FACTOR` times per matmul instead of once.
fn linear_quantized_range(x: &Matrix, w: &QuantizedMatrix, bias: Option<&[f32]>, o_start: usize, o_end: usize, out_ptr: *mut f32, out_stride: usize, x_quantized: Option<&[crate::quant::QuantizedRow]>) {
    let (block_elems, block_bytes) = w.ty.block_info().expect("QuantizedMatrix::from_raw already validated this type is supported");
    let (block_elems, block_bytes) = (block_elems as usize, block_bytes as usize);

    // Fase 9: Q5_0 es el formato dominante real que ffn_gate/ffn_up usan
    // (44.2% de los bytes de peso de qwen2.5:0.5b, auditado con
    // gguf-inspect contra los 3 checkpoints reales -- ver el plan). El
    // camino nativo cuantiza cada fila de x UNA VEZ (no una vez por
    // output feature `o`) y hace un dot product entero directo contra los
    // bytes de peso todavia empaquetados -- elimina el paso
    // dequant-a-f32 que el camino generico de abajo si hace. Bloques
    // parciales (n < 32, no deberian ocurrir en la practica ya que ggml
    // exige que los tensores cuantizados sean multiplos del tamano de
    // bloque, pero por robustez) caen al camino generico dequant+f32 para
    // ESE bloque especifico.
    if w.ty == GgmlType::Q5_0 && native_q5_0_dot_enabled() {
        let x_quantized = x_quantized.expect("linear_quantized_into always pre-quantizes x when native_dot_active_for(w.ty) is true");
        let mut acc = vec![0f32; x.rows];
        let mut block_buf = [0f32; MAX_BLOCK_ELEMS];

        for o in o_start..o_end {
            acc.iter_mut().for_each(|a| *a = 0.0);
            let row_bytes = w.row_bytes(o);
            let n_blocks = row_bytes.len() / block_bytes;
            let mut col_off = 0usize;
            for b in 0..n_blocks {
                let block_raw = &row_bytes[b * block_bytes..(b + 1) * block_bytes];
                let n = block_elems.min(w.cols - col_off);
                if n == QBLOCK {
                    let block_idx = col_off / block_elems;
                    for (s, a) in acc.iter_mut().enumerate() {
                        let xq = &x_quantized[s];
                        let x_vals = &xq.values[col_off..col_off + QBLOCK];
                        *a += dot_q5_0_q8_block(block_raw, x_vals, xq.scales[block_idx]);
                    }
                } else {
                    let block_out = &mut block_buf[..block_elems];
                    dequantize_block_into(w.ty, block_raw, block_out);
                    for (s, a) in acc.iter_mut().enumerate() {
                        let xr = &x.row(s)[col_off..col_off + n];
                        *a += dot_product(xr, &block_out[..n]);
                    }
                }
                col_off += n;
            }
            let bias_o = bias.map_or(0.0, |b| b[o]);
            for (s, &a) in acc.iter().enumerate() {
                // SAFETY: `[o_start, o_end)` is disjoint from every other
                // concurrently-running chunk's range (see doc comment
                // above), so this write never races or overlaps another
                // thread's write into the same `out` buffer.
                unsafe { *out_ptr.add(s * out_stride + o) = a + bias_o };
            }
        }
        return;
    }

    // Fase 15: Q8_0 es el segundo formato dominante real de qwen2.5:0.5b
    // (37.3% de los bytes de peso -- auditoria de la Fase 8), 0% en
    // coder:7b/gemma4:e4b (mismo perfil que Q5_0), y NO tenia camino
    // nativo -- confirmado no habia rama para GgmlType::Q8_0 aqui,
    // a diferencia de todos los demas formatos con peso real en algun
    // checkpoint. Formato mas simple de todos (qs ya son i8 sin
    // empaquetar, sin bias afin) -- mismo truco maddubs+abs/sign que Q5_0,
    // sin paso de desempaquetado.
    if w.ty == GgmlType::Q8_0 && native_q8_0_dot_enabled() {
        let x_quantized = x_quantized.expect("linear_quantized_into always pre-quantizes x when native_dot_active_for(w.ty) is true");
        let mut acc = vec![0f32; x.rows];
        let mut block_buf = [0f32; MAX_BLOCK_ELEMS];

        for o in o_start..o_end {
            acc.iter_mut().for_each(|a| *a = 0.0);
            let row_bytes = w.row_bytes(o);
            let n_blocks = row_bytes.len() / block_bytes;
            let mut col_off = 0usize;
            for b in 0..n_blocks {
                let block_raw = &row_bytes[b * block_bytes..(b + 1) * block_bytes];
                let n = block_elems.min(w.cols - col_off);
                if n == QBLOCK {
                    let block_idx = col_off / block_elems;
                    for (s, a) in acc.iter_mut().enumerate() {
                        let xq = &x_quantized[s];
                        let x_vals = &xq.values[col_off..col_off + QBLOCK];
                        *a += dot_q8_0_q8_block(block_raw, x_vals, xq.scales[block_idx]);
                    }
                } else {
                    let block_out = &mut block_buf[..block_elems];
                    dequantize_block_into(w.ty, block_raw, block_out);
                    for (s, a) in acc.iter_mut().enumerate() {
                        let xr = &x.row(s)[col_off..col_off + n];
                        *a += dot_product(xr, &block_out[..n]);
                    }
                }
                col_off += n;
            }
            let bias_o = bias.map_or(0.0, |b| b[o]);
            for (s, &a) in acc.iter().enumerate() {
                // SAFETY: `[o_start, o_end)` is disjoint from every other
                // concurrently-running chunk's range (see doc comment
                // above), so this write never races or overlaps another
                // thread's write into the same `out` buffer.
                unsafe { *out_ptr.add(s * out_stride + o) = a + bias_o };
            }
        }
        return;
    }

    // Fase 10: Q4_K es el unico formato presente con peso no trivial en
    // los 3 checkpoints reales auditados (7.5% / 73.3% / 20.2% -- ver el
    // plan), a diferencia de Q5_0/Q8_0 (0% fuera de qwen2.5:0.5b). Q4_K
    // es AFIN (w = d*q - m, no simetrico como Q5_0), asi que necesita su
    // propia familia de kernel -- ver quant::dot_q4_k_super_block. Igual
    // que Q5_0: x se cuantiza una vez por llamada, super-bloques
    // parciales (256 - block_elems para Q4_K, no deberian ocurrir en la
    // practica) caen al camino generico dequant+f32.
    if w.ty == GgmlType::Q4K && native_q4_k_dot_enabled() {
        let x_quantized = x_quantized.expect("linear_quantized_into always pre-quantizes x when native_dot_active_for(w.ty) is true");
        let mut acc = vec![0f32; x.rows];
        let mut block_buf = [0f32; MAX_BLOCK_ELEMS];

        for o in o_start..o_end {
            acc.iter_mut().for_each(|a| *a = 0.0);
            let row_bytes = w.row_bytes(o);
            let n_blocks = row_bytes.len() / block_bytes;
            let mut col_off = 0usize;
            for b in 0..n_blocks {
                let block_raw = &row_bytes[b * block_bytes..(b + 1) * block_bytes];
                let n = block_elems.min(w.cols - col_off);
                if n == block_elems {
                    for (s, a) in acc.iter_mut().enumerate() {
                        *a += dot_q4_k_super_block(block_raw, &x_quantized[s], col_off);
                    }
                } else {
                    let block_out = &mut block_buf[..block_elems];
                    dequantize_block_into(w.ty, block_raw, block_out);
                    for (s, a) in acc.iter_mut().enumerate() {
                        let xr = &x.row(s)[col_off..col_off + n];
                        *a += dot_product(xr, &block_out[..n]);
                    }
                }
                col_off += n;
            }
            let bias_o = bias.map_or(0.0, |b| b[o]);
            for (s, &a) in acc.iter().enumerate() {
                // SAFETY: `[o_start, o_end)` is disjoint from every other
                // concurrently-running chunk's range (see doc comment
                // above), so this write never races or overlaps another
                // thread's write into the same `out` buffer.
                unsafe { *out_ptr.add(s * out_stride + o) = a + bias_o };
            }
        }
        return;
    }

    // Fase 14: Q6_K esta presente en los 3 checkpoints reales con peso no
    // trivial (10.9% / 26.7% / 10.7% -- auditoria de la Fase 8), a
    // diferencia de Q5_0 (0% fuera de 0.5b). No es una copia de Q4_K:
    // aunque tambien es afin (bias fijo -32 sobre el valor de 6 bits sin
    // signo, no una resta de un `min` variable por sub-bloque como Q4_K),
    // su granularidad de sub-bloque es 16 elementos, no 32 -- ver
    // quant::dot_q6_k_super_block. Igual que Q4_K/Q5_0: x se cuantiza una
    // vez por llamada, super-bloques parciales caen al camino generico.
    if w.ty == GgmlType::Q6K && native_q6_k_dot_enabled() {
        let x_quantized = x_quantized.expect("linear_quantized_into always pre-quantizes x when native_dot_active_for(w.ty) is true");
        let mut acc = vec![0f32; x.rows];
        let mut block_buf = [0f32; MAX_BLOCK_ELEMS];

        for o in o_start..o_end {
            acc.iter_mut().for_each(|a| *a = 0.0);
            let row_bytes = w.row_bytes(o);
            let n_blocks = row_bytes.len() / block_bytes;
            let mut col_off = 0usize;
            for b in 0..n_blocks {
                let block_raw = &row_bytes[b * block_bytes..(b + 1) * block_bytes];
                let n = block_elems.min(w.cols - col_off);
                if n == block_elems {
                    for (s, a) in acc.iter_mut().enumerate() {
                        *a += dot_q6_k_super_block(block_raw, &x_quantized[s], col_off);
                    }
                } else {
                    let block_out = &mut block_buf[..block_elems];
                    dequantize_block_into(w.ty, block_raw, block_out);
                    for (s, a) in acc.iter_mut().enumerate() {
                        let xr = &x.row(s)[col_off..col_off + n];
                        *a += dot_product(xr, &block_out[..n]);
                    }
                }
                col_off += n;
            }
            let bias_o = bias.map_or(0.0, |b| b[o]);
            for (s, &a) in acc.iter().enumerate() {
                // SAFETY: `[o_start, o_end)` is disjoint from every other
                // concurrently-running chunk's range (see doc comment
                // above), so this write never races or overlaps another
                // thread's write into the same `out` buffer.
                unsafe { *out_ptr.add(s * out_stride + o) = a + bias_o };
            }
        }
        return;
    }

    let mut acc = vec![0f32; x.rows];
    let mut block_buf = [0f32; MAX_BLOCK_ELEMS];

    for o in o_start..o_end {
        acc.iter_mut().for_each(|a| *a = 0.0);

        let row_bytes = w.row_bytes(o);
        let n_blocks = row_bytes.len() / block_bytes;
        let mut col_off = 0usize;
        for b in 0..n_blocks {
            let block_raw = &row_bytes[b * block_bytes..(b + 1) * block_bytes];
            let n = block_elems.min(w.cols - col_off);
            let block_out = &mut block_buf[..block_elems];
            dequantize_block_into(w.ty, block_raw, block_out);

            for (s, a) in acc.iter_mut().enumerate() {
                let xr = &x.row(s)[col_off..col_off + n];
                *a += dot_product(xr, &block_out[..n]);
            }
            col_off += n;
        }

        let bias_o = bias.map_or(0.0, |b| b[o]);
        for (s, &a) in acc.iter().enumerate() {
            // SAFETY: see the disjoint-range argument in this function's
            // doc comment above -- same reasoning as the native-dot
            // branches above this generic fallback.
            unsafe { *out_ptr.add(s * out_stride + o) = a + bias_o };
        }
    }
}

/// RMSNorm, per row: `x_i / sqrt(mean(x^2) + eps) * weight_i`. Note this is
/// *not* the more familiar LayerNorm — no mean-subtraction, no bias.
pub fn rmsnorm(x: &Matrix, weight: &[f32], eps: f32) -> Matrix {
    let mut out = Matrix::zeros(x.rows, x.cols);
    rmsnorm_into(x, weight, eps, &mut out);
    out
}

/// Same as `rmsnorm`, but writes into a caller-provided `out` instead of
/// allocating a fresh `Matrix` — Fase 7 of the optimization roadmap, same
/// rationale as `linear_quantized_into`. `out` must already be
/// `(x.rows, x.cols)`.
pub fn rmsnorm_into(x: &Matrix, weight: &[f32], eps: f32, out: &mut Matrix) {
    assert_eq!(x.cols, weight.len());
    assert_eq!((out.rows, out.cols), (x.rows, x.cols), "rmsnorm_into: out shape must match x");
    for s in 0..x.rows {
        let row = x.row(s);
        let mean_sq = row.iter().map(|v| v * v).sum::<f32>() / row.len() as f32;
        let scale = 1.0 / (mean_sq + eps).sqrt();
        let out_row = out.row_mut(s);
        for i in 0..row.len() {
            out_row[i] = row[i] * scale * weight[i];
        }
    }
}

/// RMSNorm without a learned scale — plain `x_i / sqrt(mean(x^2) + eps)`.
/// VERIFIED (not assumed) that Gemma4 needs this as a DISTINCT operation,
/// not just "rmsnorm with weight=[1.0; n]": `src/models/gemma4.cpp`
/// normalizes V with a bare `ggml_rms_norm(ctx0, Vcur, eps)` — no
/// `attn_v_norm` weight tensor exists in the GGUF at all for V (unlike Q/K,
/// which go through `build_norm(..., attn_q_norm/attn_k_norm, ...)`, a
/// genuinely learned per-head scale). Reusing `rmsnorm` with an all-ones
/// weight vector would happen to produce the same numeric result, but would
/// wrongly imply a `Vec<f32>` weight tensor is expected to exist per model.
pub fn rmsnorm_no_weight(x: &Matrix, eps: f32) -> Matrix {
    let mut out = Matrix::zeros(x.rows, x.cols);
    rmsnorm_no_weight_into(x, eps, &mut out);
    out
}

/// Same as `rmsnorm_no_weight`, but writes into a caller-provided `out` —
/// see `rmsnorm_into`.
pub fn rmsnorm_no_weight_into(x: &Matrix, eps: f32, out: &mut Matrix) {
    assert_eq!((out.rows, out.cols), (x.rows, x.cols), "rmsnorm_no_weight_into: out shape must match x");
    for s in 0..x.rows {
        let row = x.row(s);
        let mean_sq = sum_squared(row) / row.len() as f32;
        let scale = 1.0 / (mean_sq + eps).sqrt();
        let out_row = out.row_mut(s);
        out_row.copy_from_slice(row);
        vec_scale_inplace(out_row, scale);
    }
}

/// Reinterprets `[rows, n_groups*group_size]` as `[rows*n_groups,
/// group_size]`, applies `rmsnorm` (with `weight.len() == group_size`),
/// reshapes back. Valid because `Matrix` is a flat row-major buffer
/// (`Matrix`'s own doc comment) — splitting each row into `n_groups`
/// consecutive `group_size`-wide chunks is exactly the same bytes as
/// `rows*n_groups` separate `group_size`-wide rows, no data movement
/// needed. Originally built for Gemma4's QK-norm (one shared `head_dim`-
/// long weight applied independently to each of `n_heads`/`n_kv_heads`
/// per row) and its per-layer-embedding projection norm (`n_groups` =
/// layer count instead of head count — same reshape trick, different
/// grouping meaning); promoted here once Qwen3 needed the exact same
/// per-head QK-norm operation, proving it's architecture-agnostic math,
/// not a Gemma4-specific detail.
pub fn grouped_norm(x: &Matrix, weight: &[f32], eps: f32, n_groups: usize) -> Matrix {
    let group_size = weight.len();
    debug_assert_eq!(x.cols, n_groups * group_size);
    let reshaped = Matrix { rows: x.rows * n_groups, cols: group_size, data: x.data.clone() };
    let normed = rmsnorm(&reshaped, weight, eps);
    Matrix { rows: x.rows, cols: n_groups * group_size, data: normed.data }
}

/// Same reshape trick as `grouped_norm`, without a learned scale — see
/// `rmsnorm_no_weight`'s doc comment for why that's a distinct primitive
/// rather than `grouped_norm` with an all-ones weight.
pub fn grouped_norm_no_weight(x: &Matrix, eps: f32, n_groups: usize, group_size: usize) -> Matrix {
    debug_assert_eq!(x.cols, n_groups * group_size);
    let reshaped = Matrix { rows: x.rows * n_groups, cols: group_size, data: x.data.clone() };
    let normed = rmsnorm_no_weight(&reshaped, eps);
    Matrix { rows: x.rows, cols: n_groups * group_size, data: normed.data }
}

/// Numerically-stable softmax over a single row, in place.
pub fn softmax_inplace(row: &mut [f32]) {
    let max = row.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let mut sum = 0.0f32;
    for v in row.iter_mut() {
        *v = (*v - max).exp();
        sum += *v;
    }
    for v in row.iter_mut() {
        *v /= sum;
    }
}

/// MoE routing: for each token (row of `logits`, shape `[n_tokens,
/// n_expert]`), select the `n_expert_used` highest-probability experts and
/// renormalize just their probabilities to sum to 1.
///
/// VERIFIED against `llm_graph_context::build_moe_ffn` (`src/llama-graph.cpp`,
/// ggml-org/llama.cpp commit e3546c7948e3af463d0b401e6421d5a4c2faf565),
/// specifically the path Gemma4 actually takes (`gating_op=SOFTMAX`, no
/// expert-selection bias, no DeepSeek-V3-style expert groups, `norm_w=true`
/// — see `src/models/gemma4.cpp`'s `build_moe_ffn(...)` call). That function
/// is a large shared dispatcher covering many MoE variants (DeepSeek's
/// expert groups, Llama4's pre-FFN weighting, GroveMoE); this implements
/// only the branch Gemma4's call site actually reaches, not the general case.
///
/// Order matters here and is easy to get backwards: softmax runs over ALL
/// `n_expert` experts FIRST, and top-k selects indices from that full
/// distribution — NOT top-k-by-raw-logit followed by a softmax over just
/// the selected `n_expert_used`. Those give different weights whenever a
/// non-selected expert's logit would have meaningfully affected the
/// normalizing sum.
///
/// Returns, per token, the selected `(expert_index, weight)` pairs sorted by
/// descending weight — `n_expert_used` entries, weights summing to 1
/// (barring the same near-zero clamp `build_moe_ffn` uses to avoid dividing
/// by zero, `6.103515625e-5` — the smallest positive normal `f16` value,
/// ggml's own choice of epsilon here, not a number picked independently).
pub fn moe_route(logits: &Matrix, n_expert_used: usize) -> Vec<Vec<(usize, f32)>> {
    let mut result = Vec::with_capacity(logits.rows);
    for t in 0..logits.rows {
        let mut probs = logits.row(t).to_vec();
        softmax_inplace(&mut probs);

        let mut ranked: Vec<(usize, f32)> = probs.into_iter().enumerate().collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).expect("router logits must not be NaN"));
        ranked.truncate(n_expert_used);

        let sum: f32 = ranked.iter().map(|(_, w)| *w).sum::<f32>().max(6.103515625e-5);
        for (_, w) in ranked.iter_mut() {
            *w /= sum;
        }
        result.push(ranked);
    }
    result
}

/// SiLU / swish: `x * sigmoid(x)`.
pub fn silu(x: f32) -> f32 {
    x / (1.0 + (-x).exp())
}

pub fn silu_inplace(m: &mut Matrix) {
    for v in m.data.iter_mut() {
        *v = silu(*v);
    }
}

/// GELU, tanh approximation — VERIFIED against `ggml_gelu_f32` (not
/// recalled from memory): `ggml/src/ggml-cpu/vec.h`, fetched 2026-07-12
/// from ggml-org/llama.cpp commit e3546c7948e3af463d0b401e6421d5a4c2faf565.
/// `0.5*x*(1 + tanh(sqrt(2/pi)*x*(1 + 0.044715*x^3)))`. Needed for Gemma's
/// GeGLU MLP (Gemma uses GELU where Qwen2/Llama use SiLU — see
/// `LLM_FFN_GELU` in `src/models/gemma4.cpp`).
///
/// Deliberately the exact f32 formula, not ggml's default runtime path:
/// ggml's `ggml_vec_gelu_f32` round-trips through a precomputed FP16 lookup
/// table (`ggml_table_gelu_f16`, `GGML_GELU_FP16` defined by default) rather
/// than evaluating this formula directly in f32. That's a deliberate
/// engineering choice on ggml's side, not part of the mathematical
/// definition — the FP16-quantization error it introduces isn't something
/// worth reproducing here.
const GELU_SQRT_2_OVER_PI: f32 = 0.79788456080286535587989211986876;
const GELU_COEF_A: f32 = 0.044715;

pub fn gelu(x: f32) -> f32 {
    0.5 * x * (1.0 + (GELU_SQRT_2_OVER_PI * x * (1.0 + GELU_COEF_A * x * x)).tanh())
}

pub fn gelu_inplace(m: &mut Matrix) {
    for v in m.data.iter_mut() {
        *v = gelu(*v);
    }
}

pub fn add_inplace(a: &mut Matrix, b: &Matrix) {
    assert_eq!((a.rows, a.cols), (b.rows, b.cols));
    vec_add_inplace(&mut a.data, &b.data);
}

pub fn mul_inplace(a: &mut Matrix, b: &Matrix) {
    assert_eq!((a.rows, a.cols), (b.rows, b.cols));
    vec_mul_inplace(&mut a.data, &b.data);
}

/// Rotary position embedding, ggml's `GGML_ROPE_TYPE_NEOX` convention:
/// dimension `k` is rotated together with `k + head_dim/2` (a "split-half"
/// pairing), as opposed to ggml's other, confusingly-named `NORMAL` mode
/// which pairs *adjacent* dimensions `2k`/`2k+1`. This is the convention
/// Qwen2 (and Gemma, GPT-NeoX, Falcon, ...) use — verified against
/// `llama_model_rope_type()` in llama.cpp's `src/llama-model.cpp`, since
/// guessing wrong here produces plausible-looking but silently incorrect
/// output. See `ggml_compute_forward_rope_flt` / `rotate_pairs` in
/// `ggml/src/ggml-cpu/ops.cpp` for the reference this mirrors.
///
/// `x` is `(seq, n_heads*head_dim)`; `positions[s]` is the absolute sequence
/// position of row `s` (identity when there's no KV-cache offset, as in
/// Fase 1's full-recompute design).
pub fn rope_inplace(x: &mut Matrix, n_heads: usize, head_dim: usize, freq_base: f32, positions: &[usize]) {
    rope_inplace_partial(x, n_heads, head_dim, head_dim, freq_base, positions);
}

/// Generalizes `rope_inplace` for architectures with a partial rotary
/// dimension (`n_rot < head_dim`, e.g. Phi-3's `partial_rotary_factor`) —
/// only the first `n_rot` columns of each head are rotated; the remaining
/// `head_dim - n_rot` are passed through untouched. `rope_inplace` is the
/// `n_rot == head_dim` case (every column rotated), byte-identical to this
/// function's previous non-partial implementation.
///
/// VERIFIED against `ggml_compute_forward_rope_flt`'s `GGML_ROPE_TYPE_NEOX`
/// case, ggml-org/llama.cpp commit e3546c7948e3af463d0b401e6421d5a4c2faf565,
/// `ggml/src/ggml-cpu/ops.cpp`: `theta_scale` is computed from `n_dims`
/// (this function's `n_rot`), not `ne0` (`head_dim`) — `powf(freq_base,
/// -2.0/n_dims)` — and pairs `(i, i + n_dims/2)` are rotated for `i` in
/// `0..n_dims/2` (`rotate_pairs(n_dims, n_dims/2, ...)`); everything from
/// `n_dims` to `ne0` is explicitly copied through unchanged ("fill the
/// remain channels with data from src tensor"). For an in-place op that
/// copy-through is a no-op — simply never touching those columns has the
/// same effect, which is what this function does.
pub fn rope_inplace_partial(x: &mut Matrix, n_heads: usize, head_dim: usize, n_rot: usize, freq_base: f32, positions: &[usize]) {
    rope_inplace_with_freq_factors(x, n_heads, head_dim, n_rot, freq_base, positions, None);
}

/// Generalizes `rope_inplace_partial` with Gemma4's "proportional RoPE"
/// mechanism (`layer.rope_freqs`, present only on some full-attention
/// layers of some checkpoints — `freq_factors` here, `None` recovers
/// `rope_inplace_partial`'s exact previous behavior byte-for-byte).
///
/// VERIFIED against `ggml_rope_cache_init` + `rope_yarn`
/// (`ggml/src/ggml-cpu/ops.cpp`, same commit as `rope_inplace_partial`'s
/// doc comment) for the specific case every checkpoint inspected this
/// session actually has — `ext_factor == 0.0` (no YaRN interpolation
/// ramp) and `freq_scale == 1.0` (no linear context-scaling), both
/// confirmed via `llama-context.cpp`'s resolution of the `-1.0` "not set"
/// sentinel: `cparams.yarn_ext_factor = rope_scaling_type ==
/// LLAMA_ROPE_SCALING_TYPE_YARN ? 1.0 : 0.0`, and no GGUF this session
/// inspected declares `rope.scaling.type` at all (so it defaults to
/// `LLAMA_ROPE_SCALING_TYPE_NONE`). Under those two conditions
/// `rope_yarn` collapses to `theta = theta_extrap = theta_base / ff`
/// (its `if (ext_factor != 0.0)` branch, the only place `freq_scale` and
/// the YaRN ramp/`mscale` amplitude adjustment would otherwise apply, is
/// skipped entirely) — i.e. plain per-dimension-pair division by the
/// checkpoint's own `freq_factors[k]` before computing cos/sin, applied
/// at the exact same point in the position-dependent `theta *=
/// theta_scale` recurrence `rope_inplace_partial` already has. A
/// checkpoint that DOES declare `rope.scaling.type` (YaRN or linear) would
/// need the fuller `rope_yarn` formula this function does not implement —
/// an open gap, not silently assumed away, same as this crate's other
/// documented simplifications.
pub fn rope_inplace_with_freq_factors(
    x: &mut Matrix,
    n_heads: usize,
    head_dim: usize,
    n_rot: usize,
    freq_base: f32,
    positions: &[usize],
    freq_factors: Option<&[f32]>,
) {
    rope_inplace_with_freq_factors_and_scale(x, n_heads, head_dim, n_rot, freq_base, positions, freq_factors, 1.0);
}

/// Generalizes `rope_inplace_with_freq_factors` with an `attn_factor`
/// amplitude scale — Phi-3's LongRoPE mechanism needs both freq_factors
/// (choosing between its `rope_long`/`rope_short` tables — see
/// `phi3::model`'s doc comment for the load-time long-vs-short selection)
/// AND this scale (`attn_factor` == 1.0 for every checkpoint without
/// LongRoPE, recovering `rope_inplace_with_freq_factors` exactly).
///
/// VERIFIED against `rope_yarn` (same source as
/// `rope_inplace_with_freq_factors`'s doc comment): `mscale` (this
/// function's `attn_factor`) multiplies BOTH `cos_theta` and `sin_theta`
/// unconditionally, not just inside the `ext_factor != 0.0` YaRN-ramp
/// branch — Phi-3 also has `ext_factor == 0.0` (its GGUF never declares
/// `rope.scaling.type` either, confirmed in `conversion/phi.py`: only
/// `add_rope_scaling_orig_ctx_len`/`add_rope_scaling_attn_factors` are
/// called, never `add_rope_scaling_type`), so the ramp branch is skipped
/// but the unconditional `* mscale` still applies. Implemented as a
/// post-multiply on the ROTATED output pair rather than pre-scaling `cos`/
/// `sin`: `(x0*(cos*s) - x1*(sin*s), x0*(sin*s) + x1*(cos*s)) == s *
/// (x0*cos - x1*sin, x0*sin + x1*cos)` for any shared scalar `s` — same
/// result, smaller diff against `rope_inplace_with_freq_factors`.
#[allow(clippy::too_many_arguments)]
pub fn rope_inplace_with_freq_factors_and_scale(
    x: &mut Matrix,
    n_heads: usize,
    head_dim: usize,
    n_rot: usize,
    freq_base: f32,
    positions: &[usize],
    freq_factors: Option<&[f32]>,
    attn_factor: f32,
) {
    assert_eq!(x.cols, n_heads * head_dim);
    assert_eq!(x.rows, positions.len());
    assert_eq!(n_rot % 2, 0);
    assert!(n_rot <= head_dim);

    let half = n_rot / 2;
    if let Some(ff) = freq_factors {
        assert_eq!(ff.len(), half, "freq_factors must have one entry per rotated dimension-pair (n_rot/2)");
    }
    let theta_scale = freq_base.powf(-2.0 / n_rot as f32);

    let mut cos = vec![0f32; half];
    let mut sin = vec![0f32; half];

    for (s, &pos) in positions.iter().enumerate() {
        // ggml rebuilds this cache once per sequence position and reuses it
        // across every attention head at that position; mirrored here for
        // the same reason (cheap, and keeps the float sequence identical).
        let mut theta = pos as f32;
        for k in 0..half {
            let adjusted = match freq_factors {
                Some(ff) => theta / ff[k],
                None => theta,
            };
            cos[k] = adjusted.cos();
            sin[k] = adjusted.sin();
            theta *= theta_scale;
        }

        let row = x.row_mut(s);
        for h in 0..n_heads {
            let off = h * head_dim;
            for k in 0..half {
                let x0 = row[off + k];
                let x1 = row[off + k + half];
                row[off + k] = (x0 * cos[k] - x1 * sin[k]) * attn_factor;
                row[off + k + half] = (x0 * sin[k] + x1 * cos[k]) * attn_factor;
            }
            // columns [n_rot, head_dim) are left untouched -- the partial-
            // rotary pass-through described above.
        }
    }
}

/// Causal (autoregressive) grouped-query attention, KV-cache aware.
///
/// `q` is `(seq_new, n_q_heads*head_dim)` — only the *newly computed*
/// positions (the whole prompt on a prefill call, one token on every decode
/// step after). `k`/`v` are `(kv_offset + seq_new, n_kv_heads*head_dim)` —
/// the *full* history, i.e. the KV-cache with this step's new rows already
/// appended (see `qwen2::cache::KvCache`). `kv_offset` is the absolute
/// sequence position of `q`'s first row, so local query row `s` attends
/// causally over key/value rows `0..=(kv_offset+s)`.
///
/// Passing `kv_offset=0` with `q.rows == k.rows == v.rows` recovers the
/// "recompute everything every step" behavior — still correct, just the
/// asymptotically worse path a cache exists to avoid.
///
/// Query head `h` attends using KV head `h / (n_q_heads/n_kv_heads)` — the
/// GQA grouping.
///
/// Returns `(seq_new, n_q_heads*head_dim)` — concatenated per-head outputs,
/// *before* the model's output projection (that's a separate `linear_quantized` call).
pub fn causal_gqa_attention(q: &Matrix, k: &Matrix, v: &Matrix, n_q_heads: usize, n_kv_heads: usize, head_dim: usize, kv_offset: usize) -> Matrix {
    causal_gqa_attention_scaled(q, k, v, n_q_heads, n_kv_heads, head_dim, kv_offset, 1.0 / (head_dim as f32).sqrt())
}

/// Same as `causal_gqa_attention`, but with an explicit attention scale
/// instead of the standard `1/sqrt(head_dim)`. Needed for Gemma4, which
/// VERIFIED (not assumed) uses a fixed `1.0` scale — no pre-attention
/// scaling at all (`src/models/gemma4.cpp`: "Gemma4 uses self.scaling = 1.0
/// (no pre-attn scaling)"). `causal_gqa_attention` is a thin wrapper over
/// this with the standard scale — same computation, zero behavior change
/// for existing Qwen2/Llama callers.
pub fn causal_gqa_attention_scaled(q: &Matrix, k: &Matrix, v: &Matrix, n_q_heads: usize, n_kv_heads: usize, head_dim: usize, kv_offset: usize, scale: f32) -> Matrix {
    assert_eq!(q.cols, n_q_heads * head_dim);
    assert_eq!(k.cols, n_kv_heads * head_dim);
    assert_eq!(v.cols, n_kv_heads * head_dim);
    assert_eq!(k.rows, v.rows);
    assert_eq!(k.rows, kv_offset + q.rows, "k/v must cover exactly the history up to and including q's new rows");
    assert_eq!(n_q_heads % n_kv_heads, 0, "n_q_heads must be a multiple of n_kv_heads for GQA grouping");

    let seq_new = q.rows;
    let kv_len = k.rows;
    let group = n_q_heads / n_kv_heads;

    let mut out = Matrix::zeros(seq_new, n_q_heads * head_dim);
    let mut scores = vec![0f32; kv_len];

    for qh in 0..n_q_heads {
        let kvh = qh / group;
        let q_off = qh * head_dim;
        let kv_off = kvh * head_dim;

        for s in 0..seq_new {
            let abs_pos = kv_offset + s;
            let q_vec = &q.row(s)[q_off..q_off + head_dim];
            let window = &mut scores[..=abs_pos];
            for (t, slot) in window.iter_mut().enumerate() {
                let k_vec = &k.row(t)[kv_off..kv_off + head_dim];
                *slot = dot_product(q_vec, k_vec) * scale;
            }
            softmax_inplace(window);

            let out_row = &mut out.row_mut(s)[q_off..q_off + head_dim];
            for (t, &w) in window.iter().enumerate() {
                let v_vec = &v.row(t)[kv_off..kv_off + head_dim];
                axpy_inplace(out_row, w, v_vec);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_matches_hand_computed() {
        // x: (1,2) = [1, 2]; w: (2,2) rows are out-features -> [[1,0],[0,1]] (identity)
        let x = Matrix::from_vec(1, 2, vec![1.0, 2.0]);
        let w = Matrix::from_vec(2, 2, vec![1.0, 0.0, 0.0, 1.0]);
        let y = linear(&x, &w, None);
        assert_eq!(y.data, vec![1.0, 2.0]);

        // w: (1,2) = [3,4] (a single output row) -> y = 1*3+2*4 = 11
        let w2 = Matrix::from_vec(1, 2, vec![3.0, 4.0]);
        let y2 = linear(&x, &w2, Some(&[10.0]));
        assert_eq!(y2.data, vec![21.0]);
    }

    #[test]
    fn linear_quantized_concurrent_calls_from_many_threads_do_not_panic() {
        // Fase 3 (thread pool persistente via rayon): ensure_thread_pool()
        // usa OnceLock::get_or_init, que garantiza que
        // rayon::ThreadPoolBuilder::build_global() -- el cual entra en
        // panico si se llama dos veces -- solo se ejecuta una vez incluso
        // bajo llamadas concurrentes desde varios hilos. Esto simula el
        // escenario real que motivo el cuidado (server con lazy/LRU model
        // loading: varias cargas de modelo, potencialmente concurrentes,
        // cada una disparando linear_quantized por primera vez).
        use crate::qmatrix::QuantizedMatrix;
        use gguf::GgmlType;

        // Dimensiones que superan MIN_WORK_FOR_THREADING (2_000_000) para
        // forzar la ruta con el worker pool, no el camino de 1 hilo.
        let in_features = 1024;
        let out_features = 2048;
        let mut raw = Vec::with_capacity(out_features * in_features * 4);
        for i in 0..out_features * in_features {
            let v = ((i as f32) * 0.001).sin() * 0.1;
            raw.extend_from_slice(&v.to_le_bytes());
        }
        let w = std::sync::Arc::new(QuantizedMatrix::from_raw(out_features, in_features, GgmlType::F32, raw).unwrap());
        let x = std::sync::Arc::new(Matrix::from_vec(1, in_features, (0..in_features).map(|i| (i as f32) * 0.01).collect()));

        let handles: Vec<_> = (0..8)
            .map(|_| {
                let w = std::sync::Arc::clone(&w);
                let x = std::sync::Arc::clone(&x);
                std::thread::spawn(move || {
                    let mut results = Vec::new();
                    for _ in 0..5 {
                        results.push(linear_quantized(&x, &w, None));
                    }
                    results
                })
            })
            .collect();

        let all_results: Vec<Vec<Matrix>> = handles.into_iter().map(|h| h.join().expect("outer thread panicked")).collect();

        let first = &all_results[0][0];
        for per_thread in &all_results {
            for m in per_thread {
                assert_eq!(m.data, first.data, "concurrent linear_quantized calls produced different results");
            }
        }
    }

    #[test]
    fn linear_quantized_into_threaded_path_matches_dense_reference() {
        // Fase 19: the threaded chunk path in `linear_quantized_into` used
        // to allocate a private per-chunk `partial: Matrix`, then copy it
        // back into `out`; now every chunk writes straight into `out`'s
        // own buffer through a raw pointer at its own disjoint `[start,
        // end)` column range. The test above this one only checks that
        // repeated calls with the SAME input agree with EACH OTHER --
        // that would NOT catch a deterministic boundary bug (say, always
        // dropping the last column of every chunk), since it would still
        // agree with itself every time. This test instead checks the
        // threaded path's actual VALUES against `linear`'s independently-
        // computed dense reference, at the same scale (2048 out-features,
        // 1024 in-features) that clears MIN_WORK_FOR_THREADING and spans
        // every thread-pool chunk on this hardware -- exactly where a
        // start/end off-by-one in the new pointer arithmetic would show up.
        use crate::qmatrix::QuantizedMatrix;
        use gguf::GgmlType;

        let in_features = 1024;
        let out_features = 2048;
        let mut raw = Vec::with_capacity(out_features * in_features * 4);
        let mut dense = Vec::with_capacity(out_features * in_features);
        for i in 0..out_features * in_features {
            let v = ((i as f32) * 0.001).cos() * 0.37 - 0.05;
            raw.extend_from_slice(&v.to_le_bytes());
            dense.push(v);
        }
        let qm = QuantizedMatrix::from_raw(out_features, in_features, GgmlType::F32, raw).unwrap();
        let w_dense = Matrix::from_vec(out_features, in_features, dense);

        let x = Matrix::from_vec(1, in_features, (0..in_features).map(|i| ((i as f32) * 0.013).sin() * 0.2).collect());
        let bias: Vec<f32> = (0..out_features).map(|o| (o as f32) * 0.0001 - 0.1).collect();

        let y_dense = linear(&x, &w_dense, Some(&bias));
        let y_quant = linear_quantized(&x, &qm, Some(&bias));

        assert_eq!(y_dense.rows, y_quant.rows);
        assert_eq!(y_dense.cols, y_quant.cols);
        for (o, (a, b)) in y_dense.data.iter().zip(&y_quant.data).enumerate() {
            assert!((a - b).abs() < 1e-4, "threaded linear_quantized diverged from dense reference at output feature {o}: {a} vs {b}");
        }
    }

    #[test]
    fn linear_quantized_matches_full_dequant_linear() {
        use crate::dequant::dequantize;
        use crate::qmatrix::QuantizedMatrix;
        use gguf::GgmlType;

        // 3 output rows x 40 input cols of Q4_0 (2 blocks/row: 32 + 8,
        // exercising the "partial final block" path) with varied,
        // non-trivial bit patterns and scales per row.
        //
        // Q4_0 specifically (not Q8_0, which this test used before Fase
        // 15): this test's whole point is confirming the GENERIC
        // per-block dequant-then-f32-dot path is exact, with no lossy
        // second quantization of the activation side muddying the
        // comparison -- Q4_0 is audited (Fase 8) as 0% real usage across
        // all 3 checkpoints and explicitly has no native kernel, so it
        // stays on the fully-generic path both now and if some future
        // phase adds native kernels for other formats. Q8_0 stopped being
        // a safe choice here the moment Fase 15 gave it a native (lossy)
        // path -- this exact test started failing at 1e-4 for that reason,
        // caught by cargo test, not assumed away. Q8_0's own native-path
        // nuance is covered by its dedicated
        // linear_quantized_q8_0_native_path_matches_full_dequant_linear
        // test below, with the wider tolerance that comparison actually
        // needs.
        let cols = 40;
        let rows = 3;
        let mut raw = Vec::new();
        for r in 0..rows {
            for block in 0..2 {
                let d = 0x3C00u16 + (r as u16 * 4) + (block as u16 * 2); // varying fp16 scale
                raw.extend_from_slice(&d.to_le_bytes());
                for j in 0..16 {
                    raw.push(((r * 17 + block * 5 + j * 3) % 256) as u8);
                }
            }
        }
        let qm = QuantizedMatrix::from_raw(rows, cols, GgmlType::Q4_0, raw.clone()).unwrap();

        // Build the equivalent fully-dequantized Matrix by dequantizing
        // each row's exact byte range independently (mirrors what Fase 1's
        // loader did).
        let row_bytes = 18 * 2; // 2 blocks * (2 header + 16 packed-nibble) bytes
        let mut dense = Vec::with_capacity(rows * cols);
        for r in 0..rows {
            let row_raw = &raw[r * row_bytes..(r + 1) * row_bytes];
            dense.extend(dequantize(GgmlType::Q4_0, row_raw, cols).unwrap());
        }
        let w_dense = Matrix::from_vec(rows, cols, dense);

        let x = Matrix::from_vec(2, cols, (0..2 * cols).map(|i| (i as f32) * 0.01 - 0.3).collect());
        let bias = vec![1.5, -2.0, 0.25];

        let y_dense = linear(&x, &w_dense, Some(&bias));
        let y_quant = linear_quantized(&x, &qm, Some(&bias));

        assert_eq!(y_dense.rows, y_quant.rows);
        assert_eq!(y_dense.cols, y_quant.cols);
        for (a, b) in y_dense.data.iter().zip(&y_quant.data) {
            assert!((a - b).abs() < 1e-4, "fused quantized matmul diverged from full-dequant matmul: {a} vs {b}");
        }
    }

    #[test]
    fn linear_quantized_q5_0_native_path_matches_full_dequant_linear() {
        // Fase 9: exercises linear_quantized's actual public entry point
        // with real Q5_0 bytes (not just the isolated dot_q5_0_q8_block
        // kernel in quant.rs) -- confirms the native-dot dispatch inside
        // linear_quantized_range is wired correctly, including the
        // partial-final-block fallback path (cols=40 -> one full 32-block
        // via the native kernel, one partial 8-block via dequant+f32,
        // same "40 cols, 2 blocks" shape as the Q8_0 test above).
        use crate::qmatrix::QuantizedMatrix;
        use gguf::GgmlType;

        let cols = 40;
        let rows = 3;
        let mut raw = Vec::new();
        for r in 0..rows {
            for block in 0..2 {
                let d = 0x3400u16 + (r as u16 * 6) + (block as u16 * 3); // varying small fp16 scale
                raw.extend_from_slice(&d.to_le_bytes());
                for i in 0..4 {
                    raw.push(((r * 53 + block * 11 + i * 7) % 256) as u8); // qh
                }
                for i in 0..16 {
                    raw.push(((r * 29 + block * 17 + i * 13) % 256) as u8); // qs
                }
            }
        }
        let qm = QuantizedMatrix::from_raw(rows, cols, GgmlType::Q5_0, raw.clone()).unwrap();

        let row_bytes = 22 * 2;
        let mut dense = Vec::with_capacity(rows * cols);
        for r in 0..rows {
            let row_raw = &raw[r * row_bytes..(r + 1) * row_bytes];
            dense.extend(crate::dequant::dequantize(GgmlType::Q5_0, row_raw, cols).unwrap());
        }
        let w_dense = Matrix::from_vec(rows, cols, dense);

        let x = Matrix::from_vec(2, cols, (0..2 * cols).map(|i| (i as f32) * 0.017 - 0.4).collect());
        let bias = vec![0.5, -1.0, 2.25];

        let y_dense = linear(&x, &w_dense, Some(&bias));
        let y_quant = linear_quantized(&x, &qm, Some(&bias));

        assert_eq!((y_dense.rows, y_dense.cols), (y_quant.rows, y_quant.cols));
        // Wider tolerance than the Q8_0 test: this path genuinely
        // quantizes x a second time (lossy), unlike Q8_0's path which
        // still dequantizes weights to f32 and dots against the exact x.
        // Bound derived the same way as quant.rs's own tolerance test:
        // per-block activation quantization step times the weight row's
        // L1 norm, summed loosely across ~2 blocks/row.
        for (a, b) in y_dense.data.iter().zip(&y_quant.data) {
            let scale = a.abs().max(1.0);
            assert!((a - b).abs() / scale < 0.05, "Q5_0 native-dot matmul diverged beyond quantization tolerance: {a} vs {b}");
        }
    }

    #[test]
    fn linear_quantized_q8_0_native_path_matches_full_dequant_linear() {
        // Fase 15: same shape as linear_quantized_matches_full_dequant_linear
        // above (cols=40 -> one full 32-block via the new native kernel,
        // one partial 8-block via dequant+f32 fallback) but exercised
        // through the actual public linear_quantized entry point with the
        // native dispatch active, confirming it's wired correctly
        // end-to-end -- not just the isolated dot_q8_0_q8_block kernel.
        use crate::qmatrix::QuantizedMatrix;
        use gguf::GgmlType;

        let cols = 40;
        let rows = 3;
        let mut raw = Vec::new();
        for r in 0..rows {
            for block in 0..2 {
                let d = 0x3400u16 + (r as u16 * 6) + (block as u16 * 3);
                raw.extend_from_slice(&d.to_le_bytes());
                for j in 0..32 {
                    raw.push(((r * 41 + block * 19 + j * 5) % 256) as u8);
                }
            }
        }
        let qm = QuantizedMatrix::from_raw(rows, cols, GgmlType::Q8_0, raw.clone()).unwrap();

        let row_bytes = 34 * 2;
        let mut dense = Vec::with_capacity(rows * cols);
        for r in 0..rows {
            let row_raw = &raw[r * row_bytes..(r + 1) * row_bytes];
            dense.extend(crate::dequant::dequantize(GgmlType::Q8_0, row_raw, cols).unwrap());
        }
        let w_dense = Matrix::from_vec(rows, cols, dense);

        let x = Matrix::from_vec(2, cols, (0..2 * cols).map(|i| (i as f32) * 0.019 - 0.45).collect());
        let bias = vec![0.4, -0.8, 1.6];

        let y_dense = linear(&x, &w_dense, Some(&bias));
        let y_quant = linear_quantized(&x, &qm, Some(&bias));

        assert_eq!((y_dense.rows, y_dense.cols), (y_quant.rows, y_quant.cols));
        for (a, b) in y_dense.data.iter().zip(&y_quant.data) {
            let scale = a.abs().max(1.0);
            assert!((a - b).abs() / scale < 0.05, "Q8_0 native-dot matmul diverged beyond quantization tolerance: {a} vs {b}");
        }
    }

    #[test]
    fn linear_quantized_q4_k_native_path_matches_full_dequant_linear() {
        // Fase 10: exercises linear_quantized's actual public entry point
        // with real Q4_K bytes -- confirms the affine (d*q - m) native-dot
        // dispatch is wired correctly across MULTIPLE super-blocks per row
        // (cols=512 -> 2 full 256-element super-blocks, no partial-block
        // fallback needed since real ggml tensors are always exact
        // multiples of 256 for this format).
        use crate::qmatrix::QuantizedMatrix;
        use gguf::GgmlType;

        let cols = 512;
        let rows = 3;
        let mut raw = Vec::new();
        for r in 0..rows {
            for sb in 0..2 {
                let d = 0x3400u16 + (r as u16 * 6) + (sb as u16 * 3);
                let dmin = 0x3200u16 + (r as u16 * 4) + (sb as u16 * 2);
                raw.extend_from_slice(&d.to_le_bytes());
                raw.extend_from_slice(&dmin.to_le_bytes());
                for i in 0..12 {
                    raw.push(((r * 89 + sb * 37 + i * 5) % 256) as u8);
                }
                for i in 0..128 {
                    raw.push(((r * 53 + sb * 29 + i * 13) % 256) as u8);
                }
            }
        }
        let qm = QuantizedMatrix::from_raw(rows, cols, GgmlType::Q4K, raw.clone()).unwrap();

        let row_bytes = 144 * 2;
        let mut dense = Vec::with_capacity(rows * cols);
        for r in 0..rows {
            let row_raw = &raw[r * row_bytes..(r + 1) * row_bytes];
            dense.extend(crate::dequant::dequantize(GgmlType::Q4K, row_raw, cols).unwrap());
        }
        let w_dense = Matrix::from_vec(rows, cols, dense);

        let x = Matrix::from_vec(2, cols, (0..2 * cols).map(|i| (i as f32) * 0.011 - 0.6).collect());
        let bias = vec![0.75, -0.5, 1.5];

        let y_dense = linear(&x, &w_dense, Some(&bias));
        let y_quant = linear_quantized(&x, &qm, Some(&bias));

        assert_eq!((y_dense.rows, y_dense.cols), (y_quant.rows, y_quant.cols));
        // Same reasoning as the Q5_0 integration test: genuinely lossy
        // (x quantized a second time), tolerance scales with the data.
        for (a, b) in y_dense.data.iter().zip(&y_quant.data) {
            let scale = a.abs().max(1.0);
            assert!((a - b).abs() / scale < 0.05, "Q4_K native-dot matmul diverged beyond quantization tolerance: {a} vs {b}");
        }
    }

    #[test]
    fn linear_quantized_q6_k_native_path_matches_full_dequant_linear() {
        // Fase 14: same rationale as the Q4_K integration test above --
        // exercises linear_quantized's actual public entry point with
        // real Q6_K bytes, confirming the native-dot dispatch is wired
        // correctly across MULTIPLE super-blocks per row (cols=512 -> 2
        // full 256-element super-blocks).
        use crate::qmatrix::QuantizedMatrix;
        use gguf::GgmlType;

        let cols = 512;
        let rows = 3;
        let mut raw = Vec::new();
        for r in 0..rows {
            for sb in 0..2 {
                for i in 0..128 {
                    raw.push(((r * 89 + sb * 37 + i * 5) % 256) as u8); // ql
                }
                for i in 0..64 {
                    raw.push(((r * 53 + sb * 29 + i * 13) % 256) as u8); // qh
                }
                for i in 0..16 {
                    raw.push(((r * 71 + sb * 17 + i * 19) % 256) as u8); // scales (signed)
                }
                let d = 0x3400u16 + (r as u16 * 6) + (sb as u16 * 3);
                raw.extend_from_slice(&d.to_le_bytes());
            }
        }
        let qm = QuantizedMatrix::from_raw(rows, cols, GgmlType::Q6K, raw.clone()).unwrap();

        let row_bytes = 210 * 2;
        let mut dense = Vec::with_capacity(rows * cols);
        for r in 0..rows {
            let row_raw = &raw[r * row_bytes..(r + 1) * row_bytes];
            dense.extend(crate::dequant::dequantize(GgmlType::Q6K, row_raw, cols).unwrap());
        }
        let w_dense = Matrix::from_vec(rows, cols, dense);

        let x = Matrix::from_vec(2, cols, (0..2 * cols).map(|i| (i as f32) * 0.013 - 0.5).collect());
        let bias = vec![0.6, -0.4, 1.2];

        let y_dense = linear(&x, &w_dense, Some(&bias));
        let y_quant = linear_quantized(&x, &qm, Some(&bias));

        assert_eq!((y_dense.rows, y_dense.cols), (y_quant.rows, y_quant.cols));
        // Same reasoning as the Q4_K/Q5_0 integration tests: genuinely
        // lossy (x quantized a second time), tolerance scales with the
        // data.
        for (a, b) in y_dense.data.iter().zip(&y_quant.data) {
            let scale = a.abs().max(1.0);
            assert!((a - b).abs() / scale < 0.05, "Q6_K native-dot matmul diverged beyond quantization tolerance: {a} vs {b}");
        }
    }

    #[test]
    fn rmsnorm_known_case() {
        // row = [3,4], mean_sq = (9+16)/2 = 12.5, scale = 1/sqrt(12.5+eps)
        let x = Matrix::from_vec(1, 2, vec![3.0, 4.0]);
        let out = rmsnorm(&x, &[1.0, 1.0], 0.0);
        let expected_scale = 1.0 / 12.5f32.sqrt();
        assert!((out.data[0] - 3.0 * expected_scale).abs() < 1e-6);
        assert!((out.data[1] - 4.0 * expected_scale).abs() < 1e-6);
    }

    #[test]
    fn rmsnorm_no_weight_matches_rmsnorm_with_all_ones() {
        // Same math either way; the no-weight variant exists as a distinct
        // op because Gemma4's V-projection genuinely has no weight tensor to
        // load (see this fn's doc comment) — not just as a convenience
        // wrapper. Numeric equivalence to weight=[1,1] is exactly what
        // should hold, and is what this pins down.
        let x = Matrix::from_vec(1, 2, vec![3.0, 4.0]);
        let weighted = rmsnorm(&x, &[1.0, 1.0], 1e-5);
        let unweighted = rmsnorm_no_weight(&x, 1e-5);
        assert!((weighted.data[0] - unweighted.data[0]).abs() < 1e-6);
        assert!((weighted.data[1] - unweighted.data[1]).abs() < 1e-6);
    }

    #[test]
    fn softmax_sums_to_one_and_is_monotonic() {
        let mut row = vec![1.0, 2.0, 3.0];
        softmax_inplace(&mut row);
        let sum: f32 = row.iter().sum();
        assert!((sum - 1.0).abs() < 1e-6);
        assert!(row[2] > row[1] && row[1] > row[0]);
    }

    #[test]
    fn moe_route_picks_highest_softmax_experts_and_renormalizes() {
        // 1 token, 4 experts, logits chosen with no ties so the top-2 by
        // softmax probability is unambiguous: expert 2 (logit 3.0) then
        // expert 1 (logit 2.0).
        let logits = Matrix::from_vec(1, 4, vec![1.0, 2.0, 3.0, 0.5]);
        let routed = moe_route(&logits, 2);
        assert_eq!(routed.len(), 1);
        let picks = &routed[0];
        assert_eq!(picks.len(), 2);
        assert_eq!(picks[0].0, 2, "highest logit (3.0) should rank first");
        assert_eq!(picks[1].0, 1, "second-highest logit (2.0) should rank second");

        // Independently re-derive the expected renormalized weights: full
        // softmax over all 4 experts, THEN take just the top-2 probs and
        // renormalize them to sum to 1 -- same order build_moe_ffn uses.
        let raw = [1.0f32, 2.0, 3.0, 0.5];
        let exp: Vec<f32> = raw.iter().map(|v| v.exp()).collect();
        let sum_all: f32 = exp.iter().sum();
        let full_softmax: Vec<f32> = exp.iter().map(|v| v / sum_all).collect();
        let top2_sum = full_softmax[2] + full_softmax[1];
        let expected_w2 = full_softmax[2] / top2_sum;
        let expected_w1 = full_softmax[1] / top2_sum;

        assert!((picks[0].1 - expected_w2).abs() < 1e-5, "got {}, expected {expected_w2}", picks[0].1);
        assert!((picks[1].1 - expected_w1).abs() < 1e-5, "got {}, expected {expected_w1}", picks[1].1);

        let total: f32 = picks.iter().map(|(_, w)| w).sum();
        assert!((total - 1.0).abs() < 1e-6, "selected weights must renormalize to sum 1, got {total}");
    }

    #[test]
    fn moe_route_is_independent_per_token() {
        // 2 tokens, 3 experts each, opposite rankings -- confirms routing
        // doesn't leak state or ordering across rows.
        let logits = Matrix::from_vec(2, 3, vec![5.0, 1.0, 1.0, 1.0, 1.0, 5.0]);
        let routed = moe_route(&logits, 1);
        assert_eq!(routed.len(), 2);
        assert_eq!(routed[0][0].0, 0, "token 0's highest logit is expert 0");
        assert_eq!(routed[1][0].0, 2, "token 1's highest logit is expert 2");
        // n_expert_used=1 -- the single selected expert's weight renormalizes to 1.
        assert!((routed[0][0].1 - 1.0).abs() < 1e-6);
        assert!((routed[1][0].1 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn silu_known_values() {
        assert_eq!(silu(0.0), 0.0);
        assert!(silu(10.0) > 9.99); // saturates towards x for large x
        assert!(silu(-10.0).abs() < 0.01); // saturates towards 0 for very negative x
    }

    #[test]
    fn gelu_known_values() {
        // x=0: the whole expression collapses to 0 exactly, tanh(0) or not.
        assert_eq!(gelu(0.0), 0.0);
        assert!(gelu(10.0) > 9.99); // saturates towards x for large positive x
        assert!(gelu(-10.0).abs() < 0.01); // saturates towards 0 for very negative x
    }

    #[test]
    fn gelu_matches_tanh_approximation_formula() {
        // Independently re-derives the expected value from the SAME public
        // formula (via Rust's trusted std f32::tanh), not a hand-typed
        // decimal — same style as rmsnorm_known_case above. Verifies the
        // implementation didn't drop/mistranspose a constant, not just that
        // it saturates in the right direction.
        for x in [-3.0f32, -0.5, 0.3, 1.0, 2.7] {
            let inner = 0.79788456080286535587989211986876f32 * x * (1.0 + 0.044715 * x * x);
            let expected = 0.5 * x * (1.0 + inner.tanh());
            assert!((gelu(x) - expected).abs() < 1e-6, "gelu({x}) = {}, expected {expected}", gelu(x));
        }
    }

    #[test]
    fn rope_preserves_pair_norm() {
        // RoPE is a rotation: for any position/head, ||[/x0,x1]|| must be
        // preserved (rotations don't change vector length).
        let mut x = Matrix::from_vec(1, 4, vec![1.0, 2.0, 3.0, 4.0]);
        let norm_before = (x.data[0].powi(2) + x.data[2].powi(2)).sqrt(); // pair (0, 0+half=2)
        rope_inplace(&mut x, 1, 4, 10000.0, &[5]);
        let norm_after = (x.data[0].powi(2) + x.data[2].powi(2)).sqrt();
        assert!((norm_before - norm_after).abs() < 1e-5);
    }

    #[test]
    fn rope_position_zero_is_identity() {
        let original = vec![1.0, 2.0, 3.0, 4.0];
        let mut x = Matrix::from_vec(1, 4, original.clone());
        rope_inplace(&mut x, 1, 4, 10000.0, &[0]);
        for (a, b) in x.data.iter().zip(&original) {
            assert!((a - b).abs() < 1e-6, "position 0 should leave the vector unrotated");
        }
    }

    #[test]
    fn rope_inplace_partial_with_n_rot_eq_head_dim_matches_rope_inplace() {
        // rope_inplace is now defined as rope_inplace_partial(..., head_dim,
        // ...) -- this pins that delegation so a future edit can't silently
        // desync the two.
        let mut a = Matrix::from_vec(2, 8, (0..16).map(|i| i as f32).collect());
        let mut b = a.clone();
        rope_inplace(&mut a, 2, 4, 10000.0, &[3, 7]);
        rope_inplace_partial(&mut b, 2, 4, 4, 10000.0, &[3, 7]);
        assert_eq!(a.data, b.data);
    }

    #[test]
    fn rope_inplace_partial_leaves_columns_beyond_n_rot_untouched() {
        // head_dim=4, n_rot=2: only columns [0,1] (the rotated pair) may
        // change: column 2 has nothing paired with it within n_rot (pairs
        // are (k, k+n_rot/2) for k in 0..n_rot/2=1, i.e. just (0,1)), and
        // column 3 is past n_rot entirely -- both must equal their input.
        let original = vec![1.0, 2.0, 3.0, 4.0];
        let mut x = Matrix::from_vec(1, 4, original.clone());
        rope_inplace_partial(&mut x, 1, 4, 2, 10000.0, &[5]);
        assert!((x.data[2] - original[2]).abs() < 1e-6, "column 2 (past the rotated pair) must be untouched");
        assert!((x.data[3] - original[3]).abs() < 1e-6, "column 3 (past n_rot) must be untouched");
        // Sanity: the rotated pair actually did rotate (position 5 != 0).
        assert!((x.data[0] - original[0]).abs() > 1e-6, "the rotated pair should have actually changed");
    }

    #[test]
    fn rope_inplace_partial_rotated_pair_matches_full_rotation_at_same_n_rot() {
        // The rotated sub-range [0, n_rot) should behave exactly like a
        // full rope_inplace call over a head_dim==n_rot-sized head -- same
        // theta_scale formula (driven by n_rot, verified against ggml's
        // `powf(freq_base, -2.0/n_dims)`), same pairing.
        let mut partial = Matrix::from_vec(1, 4, vec![1.0, 2.0, 3.0, 4.0]);
        rope_inplace_partial(&mut partial, 1, 4, 2, 10000.0, &[5]);

        let mut full = Matrix::from_vec(1, 2, vec![1.0, 2.0]);
        rope_inplace(&mut full, 1, 2, 10000.0, &[5]);

        assert!((partial.data[0] - full.data[0]).abs() < 1e-5);
        assert!((partial.data[1] - full.data[1]).abs() < 1e-5);
    }

    #[test]
    fn rope_inplace_with_freq_factors_none_matches_rope_inplace_partial() {
        let mut a = Matrix::from_vec(2, 8, (0..16).map(|i| i as f32).collect());
        let mut b = a.clone();
        rope_inplace_partial(&mut a, 2, 4, 4, 10000.0, &[3, 7]);
        rope_inplace_with_freq_factors(&mut b, 2, 4, 4, 10000.0, &[3, 7], None);
        assert_eq!(a.data, b.data);
    }

    #[test]
    fn rope_inplace_with_freq_factors_all_ones_matches_none() {
        // A checkpoint whose rope_freqs tensor happens to be all 1.0 (a
        // trivial/no-op proportional-rope table) must produce byte-identical
        // output to not declaring freq_factors at all.
        let mut a = Matrix::from_vec(1, 4, vec![1.0, 2.0, 3.0, 4.0]);
        let mut b = a.clone();
        rope_inplace_with_freq_factors(&mut a, 1, 4, 4, 10000.0, &[5], None);
        rope_inplace_with_freq_factors(&mut b, 1, 4, 4, 10000.0, &[5], Some(&[1.0, 1.0]));
        assert_eq!(a.data, b.data);
    }

    #[test]
    fn rope_inplace_with_freq_factors_divides_theta_before_rotation() {
        // VERIFIED formula (see function doc comment): rotated angle is
        // theta_base/ff, not theta_base*ff or some other combination --
        // hand-computed expected values, not just "output changed".
        let mut x = Matrix::from_vec(1, 2, vec![1.0, 0.0]);
        let pos = 3usize;
        let ff = 2.0f32;
        rope_inplace_with_freq_factors(&mut x, 1, 2, 2, 10000.0, &[pos], Some(&[ff]));

        let expected_theta = pos as f32 / ff;
        let (expected_x0, expected_x1) = (expected_theta.cos(), expected_theta.sin());
        assert!((x.data[0] - expected_x0).abs() < 1e-5);
        assert!((x.data[1] - expected_x1).abs() < 1e-5);
    }

    #[test]
    #[should_panic(expected = "one entry per rotated dimension-pair")]
    fn rope_inplace_with_freq_factors_rejects_wrong_length() {
        let mut x = Matrix::from_vec(1, 4, vec![1.0, 2.0, 3.0, 4.0]);
        rope_inplace_with_freq_factors(&mut x, 1, 4, 4, 10000.0, &[5], Some(&[1.0])); // needs 2 entries (n_rot/2), gave 1
    }

    #[test]
    fn rope_inplace_with_freq_factors_and_scale_attn_factor_one_matches_without_scale() {
        let mut a = Matrix::from_vec(1, 4, vec![1.0, 2.0, 3.0, 4.0]);
        let mut b = a.clone();
        rope_inplace_with_freq_factors(&mut a, 1, 4, 4, 10000.0, &[5], Some(&[2.0, 3.0]));
        rope_inplace_with_freq_factors_and_scale(&mut b, 1, 4, 4, 10000.0, &[5], Some(&[2.0, 3.0]), 1.0);
        assert_eq!(a.data, b.data);
    }

    #[test]
    fn rope_inplace_with_freq_factors_and_scale_scales_the_rotated_output() {
        // VERIFIED equivalence (see function doc comment): scaling cos/sin
        // before rotating == scaling the rotated pair after -- checked
        // directly against an unscaled baseline multiplied by hand.
        let mut baseline = Matrix::from_vec(1, 2, vec![1.0, 0.0]);
        rope_inplace_with_freq_factors_and_scale(&mut baseline, 1, 2, 2, 10000.0, &[3], None, 1.0);

        let mut scaled = Matrix::from_vec(1, 2, vec![1.0, 0.0]);
        let attn_factor = 1.25f32;
        rope_inplace_with_freq_factors_and_scale(&mut scaled, 1, 2, 2, 10000.0, &[3], None, attn_factor);

        assert!((scaled.data[0] - baseline.data[0] * attn_factor).abs() < 1e-5);
        assert!((scaled.data[1] - baseline.data[1] * attn_factor).abs() < 1e-5);
    }

    #[test]
    fn causal_attention_ignores_future_positions() {
        // 1 head, head_dim=2, 3 positions. Make v distinct per position so
        // we can tell which positions contributed to each output row.
        let q = Matrix::from_vec(3, 2, vec![1.0, 0.0, 1.0, 0.0, 1.0, 0.0]);
        let k = Matrix::from_vec(3, 2, vec![1.0, 0.0, 1.0, 0.0, 1.0, 0.0]); // uniform scores at every position
        let v = Matrix::from_vec(3, 2, vec![10.0, 0.0, 20.0, 0.0, 30.0, 0.0]);
        let out = causal_gqa_attention(&q, &k, &v, 1, 1, 2, 0);

        // Row 0 can only see position 0 -> exactly v[0].
        assert!((out.row(0)[0] - 10.0).abs() < 1e-4);
        // Row 1 sees positions 0,1 with equal weight (identical q.k scores) -> average of v[0],v[1].
        assert!((out.row(1)[0] - 15.0).abs() < 1e-4);
        // Row 2 sees positions 0,1,2 equally -> average of v[0],v[1],v[2].
        assert!((out.row(2)[0] - 20.0).abs() < 1e-4);
    }

    #[test]
    fn causal_gqa_attention_scaled_matches_default_at_standard_scale() {
        let q = Matrix::from_vec(2, 2, vec![1.0, 0.0, 0.5, 0.5]);
        let k = Matrix::from_vec(2, 2, vec![2.0, 0.0, 1.0, 1.0]);
        let v = Matrix::from_vec(2, 2, vec![10.0, 20.0, 30.0, 40.0]);
        let default = causal_gqa_attention(&q, &k, &v, 1, 1, 2, 0);
        let explicit_standard = causal_gqa_attention_scaled(&q, &k, &v, 1, 1, 2, 0, 1.0 / (2f32).sqrt());
        for i in 0..default.data.len() {
            assert!((default.data[i] - explicit_standard.data[i]).abs() < 1e-6);
        }
    }

    #[test]
    fn causal_gqa_attention_scaled_scale_actually_changes_output() {
        // Non-uniform q.k scores (unlike the other fixture above, which uses
        // identical scores everywhere and so can't detect a broken scale
        // parameter): a different scale must sharpen/flatten the softmax
        // differently and so produce a genuinely different weighted output.
        let q = Matrix::from_vec(1, 2, vec![1.0, 0.0]);
        let k = Matrix::from_vec(2, 2, vec![2.0, 0.0, 1.0, 0.0]); // q.k = [2, 1] -- distinct
        let v = Matrix::from_vec(2, 2, vec![10.0, 0.0, 0.0, 0.0]);
        let out_default_scale = causal_gqa_attention_scaled(&q, &k, &v, 1, 1, 2, 1, 1.0 / (2f32).sqrt());
        let out_gemma4_scale = causal_gqa_attention_scaled(&q, &k, &v, 1, 1, 2, 1, 1.0);
        assert!(
            (out_default_scale.data[0] - out_gemma4_scale.data[0]).abs() > 1e-3,
            "different scales must produce different softmax weighting: {} vs {}",
            out_default_scale.data[0],
            out_gemma4_scale.data[0]
        );
    }

    #[test]
    fn cached_attention_matches_full_recompute() {
        // The whole point of `kv_offset`: computing position 2's output by
        // (a) feeding it the full 3-row k/v history with kv_offset=2 and a
        // 1-row q, versus (b) recomputing all 3 positions from scratch with
        // kv_offset=0, must give bit-identical results — that equivalence
        // is what makes the KV-cache a pure performance optimization and
        // not a behavior change.
        let q_full = Matrix::from_vec(3, 2, vec![1.0, 0.5, 0.3, 0.9, 0.7, 0.2]);
        let k = Matrix::from_vec(3, 2, vec![0.4, 0.1, 0.2, 0.8, 0.6, 0.3]);
        let v = Matrix::from_vec(3, 2, vec![10.0, 1.0, 20.0, 2.0, 30.0, 3.0]);

        let full = causal_gqa_attention(&q_full, &k, &v, 1, 1, 2, 0);

        let q_last = Matrix::from_vec(1, 2, q_full.row(2).to_vec());
        let cached = causal_gqa_attention(&q_last, &k, &v, 1, 1, 2, 2);

        assert_eq!(full.row(2), cached.row(0));
    }
}
