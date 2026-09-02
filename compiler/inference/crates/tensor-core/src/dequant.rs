//! Dequantization: turning a tensor's raw on-disk bytes (as described by a
//! `gguf::GgmlType`) into f32 values.
//!
//! Every block layout and formula here is transcribed directly from ggml's
//! reference implementation (`ggml/src/ggml-quants.c`,
//! `ggml/src/ggml-common.h` in ggml-org/llama.cpp) rather than derived from
//! the general "quantization" concept — these formats have enough
//! non-obvious bit-packing (de-interleaved nibbles, packed 6-bit
//! scale/min pairs sharing bytes across sub-blocks) that guessing at the
//! layout produces plausible-looking but silently wrong numbers.
//!
//! Two entry points, both built on the same per-block functions:
//!   - `dequantize()` — a whole tensor at once, into a fresh `Vec<f32>`.
//!     Used by Fase 1's weight loader and by embedding lookups (a single
//!     row still needs materializing to index into).
//!   - `dequantize_block_into()` — exactly one block, into a caller-owned
//!     buffer, no allocation. This is what Fase 2's fused matmul kernel
//!     (`ops::linear_quantized`) calls in its inner loop, so a 24-layer,
//!     multi-thousand-row-per-layer forward pass doesn't allocate a Vec per
//!     block or materialize a whole weight matrix in f32 up front.

use gguf::GgmlType;

use crate::f16::f16_to_f32;

#[derive(Debug)]
pub enum DequantError {
    Unsupported(GgmlType),
    SizeMismatch { ty: GgmlType, expected: usize, got: usize },
}

impl std::fmt::Display for DequantError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DequantError::Unsupported(ty) => write!(f, "dequantization not implemented for {}", ty.name()),
            DequantError::SizeMismatch { ty, expected, got } => write!(
                f,
                "raw byte length mismatch for {}: expected {expected}, got {got}",
                ty.name()
            ),
        }
    }
}
impl std::error::Error for DequantError {}

/// Whether this crate can actually dequantize `ty`.
///
/// This is the single source of truth for that question, and it exists because
/// `block_info()` is NOT the same question. A format can have a perfectly
/// well-known block layout here and still have no implementation below:
/// `Q5_K`, `Q4_1`, `Q5_1`, `Q8_1`, `Q2_K`, `Q3_K` and `Q8_K` are exactly that
/// case. Checking only `block_info()` (as `dequantize` and
/// `QuantizedMatrix::from_raw` used to) let those types sail past validation
/// and blow up later in `dequantize_block_into`'s catch-all `panic!` — in the
/// middle of a forward pass, after minutes spent loading gigabytes of weights,
/// rather than at load time with a clear message.
///
/// `Q5_K_M` in particular is one of the most commonly published quantizations
/// on Hugging Face, so this is a realistic path, not a theoretical one.
///
/// MUST stay in sync with the `match` in `dequantize_block_into`. The
/// `implemented_types_match_dequantize_block_into` test below enforces that:
/// it drives every known `GgmlType` through both and fails if they disagree.
pub fn is_implemented(ty: GgmlType) -> bool {
    matches!(
        ty,
        GgmlType::F32
            | GgmlType::F16
            | GgmlType::Bf16
            | GgmlType::Q4_0
            | GgmlType::Q5_0
            | GgmlType::Q8_0
            | GgmlType::Q4K
            | GgmlType::Q6K
    )
}

/// Dequantize `n_elements` values of type `ty` from `raw`. `raw` must be
/// exactly the byte range this tensor occupies (i.e. `TensorInfo::size_bytes()`
/// bytes, no more, no less) — blocks are consumed whole, nothing left over.
pub fn dequantize(ty: GgmlType, raw: &[u8], n_elements: usize) -> Result<Vec<f32>, DequantError> {
    // Checked before `block_info()` so a known-layout-but-unimplemented type
    // (Q5_K and friends) returns this error instead of reaching the `panic!`
    // in `dequantize_block_into` — see `is_implemented`.
    if !is_implemented(ty) {
        return Err(DequantError::Unsupported(ty));
    }
    let (block_elems, block_bytes) = ty.block_info().ok_or(DequantError::Unsupported(ty))?;
    let block_elems = block_elems as usize;
    let block_bytes = block_bytes as usize;
    let n_blocks = n_elements.div_ceil(block_elems);
    let expected = n_blocks * block_bytes;
    if raw.len() != expected {
        return Err(DequantError::SizeMismatch { ty, expected, got: raw.len() });
    }

    let mut out = vec![0f32; n_blocks * block_elems];
    for b in 0..n_blocks {
        let block_raw = &raw[b * block_bytes..(b + 1) * block_bytes];
        let block_out = &mut out[b * block_elems..(b + 1) * block_elems];
        dequantize_block_into(ty, block_raw, block_out);
    }
    out.truncate(n_elements);
    Ok(out)
}

/// The largest `block_info().0` across every type this crate implements —
/// callers doing block-at-a-time fused kernels (see `ops::linear_quantized`)
/// can size a single reusable stack buffer `[0f32; MAX_BLOCK_ELEMS]` and
/// never need to reallocate no matter which quant type they're consuming.
pub const MAX_BLOCK_ELEMS: usize = 256;

/// Dequantize exactly one block. `block_raw` must be exactly
/// `ty.block_info().1` bytes; `out` must be at least `ty.block_info().0`
/// elements (only that many are written — a caller sizing `out` at
/// `MAX_BLOCK_ELEMS` and passing a type with a smaller block, e.g. Q8_0's
/// 32, is fine and leaves the tail of `out` untouched).
///
/// Panics on a type this crate doesn't implement or a wrong-sized
/// `block_raw` — this is the hot-path primitive, so it trusts the caller
/// (both current call sites size things from `GgmlType::block_info()`
/// directly, so a mismatch here means a real bug in this crate, not bad
/// input from a model file).
///
/// Reaching the unsupported-type `panic!` below should now be impossible via
/// any normal path: both ways a model's bytes get here — `dequantize` and
/// `QuantizedMatrix::from_raw` — reject unimplemented types up front through
/// `is_implemented`. It stays a panic rather than a `Result` on purpose: this
/// runs in the innermost matmul loop, and by this point an unsupported type
/// genuinely is a bug in this crate, not bad input.
pub fn dequantize_block_into(ty: GgmlType, block_raw: &[u8], out: &mut [f32]) {
    match ty {
        GgmlType::F32 => {
            for (i, c) in block_raw.chunks_exact(4).enumerate() {
                out[i] = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
            }
        }
        GgmlType::F16 => {
            for (i, c) in block_raw.chunks_exact(2).enumerate() {
                out[i] = f16_to_f32(u16::from_le_bytes([c[0], c[1]]));
            }
        }
        GgmlType::Bf16 => {
            // bfloat16 is exactly the top 16 bits of an f32 (1 sign + 8
            // exponent + 7 mantissa bits, vs f16's 1+5+10) -- upcasting is
            // a left-shift into the high half of a u32, zero-filling the
            // low 16 mantissa bits, then reinterpreting as f32. No bias
            // adjustment needed (unlike f16), since the exponent width and
            // bias already match f32's exactly.
            for (i, c) in block_raw.chunks_exact(2).enumerate() {
                out[i] = f32::from_bits((u16::from_le_bytes([c[0], c[1]]) as u32) << 16);
            }
        }
        GgmlType::Q4_0 => dequant_block_q4_0(block_raw, out),
        GgmlType::Q5_0 => crate::simd::dequant_q5_0_block(block_raw, out),
        GgmlType::Q8_0 => crate::simd::dequant_q8_0_block(block_raw, out),
        GgmlType::Q4K => crate::simd::dequant_q4_k_block(block_raw, out),
        GgmlType::Q6K => crate::simd::dequant_q6_k_block(block_raw, out),
        _ => panic!("dequantize_block_into: unsupported type {}", ty.name()),
    }
}

fn read_u16_le(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

/// block_q4_0 { fp16 d; u8 qs[16]; } — 32 elements/block.
/// Layout: qs[j] packs elements j (low nibble) and j+16 (high nibble), not
/// elements 2j/2j+1 — the two halves of the block are de-interleaved.
fn dequant_block_q4_0(raw: &[u8], out: &mut [f32]) {
    let d = f16_to_f32(read_u16_le(raw, 0));
    let qs = &raw[2..18];
    for j in 0..16 {
        let x0 = (qs[j] & 0x0F) as i32 - 8;
        let x1 = (qs[j] >> 4) as i32 - 8;
        out[j] = x0 as f32 * d;
        out[j + 16] = x1 as f32 * d;
    }
}

/// Unpacks the 6-bit (scale, min) pair for K-quant sub-block `j` (0..7) from
/// the 12-byte packed `scales` array. The packing spends 6 bits each on the
/// first 4 sub-blocks' scale and min (bytes 0..7 hold exactly that), then
/// reuses the *top* 2 bits of those same first-4 bytes as the high bits of
/// sub-blocks 4..7's scale/min (bytes 8..11 hold their low 4 bits) — i.e.
/// the encoding trades 16 bytes of naive 6-bit-per-value storage for 12 by
/// overlapping the last 4 sub-blocks' high bits into the first 8 bytes'
/// unused top bits. Transcribed from ggml's `get_scale_min_k4`.
pub(crate) fn get_scale_min_k4(j: usize, q: &[u8]) -> (u8, u8) {
    if j < 4 {
        (q[j] & 63, q[j + 4] & 63)
    } else {
        let d = (q[j + 4] & 0x0F) | ((q[j - 4] >> 6) << 4);
        let m = (q[j + 4] >> 4) | ((q[j] >> 6) << 4);
        (d, m)
    }
}

// block_q4_K { fp16 d; fp16 dmin; u8 scales[12]; u8 qs[128]; } — 256
// elements/block (8 sub-blocks of 32). Value = d*sc*q4 - dmin*m, i.e. an
// affine (not symmetric) quantization: each sub-block has its own 6-bit
// scale *and* 6-bit min, both further scaled by the block-wide fp16 d/dmin.
// Scalar reference lives in `simd::dequant_q4_k_block_scalar` alongside its
// AVX2 counterpart — see that module.

// block_q6_K { u8 ql[128]; u8 qh[64]; i8 scales[16]; fp16 d; } — 256
// elements/block (16 sub-blocks of 16). Each value packs a 4-bit low part
// (`ql`) and a 2-bit high part (`qh`, two bits per element, four elements
// packed per `qh` byte), combined into a 6-bit signed value centered at 32.
// `scales` is signed (`int8_t[16]` in ggml) — see the `q6_k_scales_are_signed`
// regression test below. Scalar reference lives in
// `simd::dequant_q6_k_block_scalar` alongside its AVX2 counterpart — see
// that module.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f32_passthrough() {
        let raw = [1.5f32, -2.25, 0.0].iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
        let out = dequantize(GgmlType::F32, &raw, 3).unwrap();
        assert_eq!(out, vec![1.5, -2.25, 0.0]);
    }

    #[test]
    fn bf16_upcasts_via_left_shift_no_bias_adjustment() {
        // bf16 is the top 16 bits of the SAME f32 bit pattern (unlike f16,
        // which has its own narrower exponent range and needs bias
        // conversion) -- 1.0f32 = 0x3F800000, top half = 0x3F80; -2.0f32 =
        // 0xC0000000, top half = 0xC000. Values chosen so both the sign bit
        // and a nonzero exponent shift are exercised.
        let raw: Vec<u8> = [0x3F80u16, 0xC000u16].iter().flat_map(|b| b.to_le_bytes()).collect();
        let out = dequantize(GgmlType::Bf16, &raw, 2).unwrap();
        assert_eq!(out, vec![1.0, -2.0]);
    }

    #[test]
    fn q8_0_single_block_all_scale_one() {
        // d = 1.0 (fp16 0x3C00), qs = [0, 1, -1, 2, ...] with the rest 0.
        let mut raw = vec![0u8; 34];
        raw[0..2].copy_from_slice(&0x3C00u16.to_le_bytes());
        raw[2] = 0; // 0
        raw[3] = 1; // 1
        raw[4] = (-1i8) as u8; // -1
        raw[5] = 2; // 2
        let out = dequantize(GgmlType::Q8_0, &raw, 32).unwrap();
        assert_eq!(out.len(), 32);
        assert_eq!(&out[0..4], &[0.0, 1.0, -1.0, 2.0]);
    }

    #[test]
    fn q4_0_deinterleaving() {
        // d = 2.0 (fp16 0x4000). qs[0] = 0x9F -> low nibble 0xF(15)-8=7,
        // high nibble 0x9(9)-8=1. Expect block[0]=7*2=14, block[16]=1*2=2.
        let mut raw = vec![0u8; 18];
        raw[0..2].copy_from_slice(&0x4000u16.to_le_bytes());
        raw[2] = 0x9F;
        let out = dequantize(GgmlType::Q4_0, &raw, 32).unwrap();
        assert_eq!(out[0], 14.0);
        assert_eq!(out[16], 2.0);
    }

    #[test]
    fn q6_k_scales_are_signed() {
        // Regression test for a real bug caught in Fase 1: ggml's Q6_K
        // `scales` field is `int8_t[16]` (signed), but an early version of
        // this dequantizer read it as `u8` and converted straight to f32 —
        // correct for scale bytes < 128, silently wrong (off by nearly a
        // factor of -256/x) for any negative scale. Cross-checked against
        // gguf-py's `dequantize()` on a real model tensor
        // (blk.0.ffn_down.weight in qwen2.5:0.5b) where this first surfaced
        // as garbage generation output despite no NaN/Inf anywhere.
        let mut raw = vec![0u8; 210];
        // d = 1.0 (fp16 0x3C00).
        raw[208..210].copy_from_slice(&0x3C00u16.to_le_bytes());
        // scales[0] = -73 (0xB7) — the sub-block used by output elements 0..16.
        raw[192] = 0xB7;
        // ql[0] = 0x0F -> q1's low-4-bits nibble = 0xF; qh[0] = 0 -> high 2 bits = 0.
        // combined 6-bit value = 0xF (15), minus 32 bias = -17.
        raw[0] = 0x0F;
        let out = dequantize(GgmlType::Q6K, &raw, 256).unwrap();
        // expected = d * scale * q = 1.0 * -73 * -17 = 1241.0
        assert_eq!(out[0], 1241.0, "Q6_K scale byte 0xB7 must be read as signed (-73), not unsigned (183)");
    }

    #[test]
    fn rejects_wrong_length() {
        let raw = vec![0u8; 10]; // not a multiple of any real block size
        match dequantize(GgmlType::Q8_0, &raw, 32) {
            Err(DequantError::SizeMismatch { .. }) => {}
            other => panic!("expected SizeMismatch, got {other:?}"),
        }
    }

    #[test]
    fn multi_block_matches_per_block_calls() {
        // dequantize() (whole-tensor) and repeated dequantize_block_into()
        // calls (Fase 2's fused-kernel primitive) must agree exactly — one
        // is implemented in terms of the other, but this pins that down as
        // a regression check rather than an implementation detail.
        let mut raw = vec![0u8; 34 * 3];
        for b in 0..3 {
            let base = b * 34;
            raw[base..base + 2].copy_from_slice(&(0x3C00u16 + b as u16 * 4).to_le_bytes());
            for j in 0..32 {
                raw[base + 2 + j] = ((b * 7 + j) % 256) as u8;
            }
        }
        let whole = dequantize(GgmlType::Q8_0, &raw, 96).unwrap();
        for b in 0..3 {
            let mut block_out = [0f32; 32];
            dequantize_block_into(GgmlType::Q8_0, &raw[b * 34..(b + 1) * 34], &mut block_out);
            assert_eq!(&whole[b * 32..(b + 1) * 32], &block_out[..]);
        }
    }

    #[test]
    fn known_layout_but_unimplemented_quant_errors_instead_of_panicking() {
        // Q5_K is the realistic case: `Q5_K_M` is one of the most commonly
        // published quantizations on Hugging Face, and it has a perfectly
        // well-known block layout here — so it used to sail past the
        // `block_info()` check in `dequantize` and abort the process in
        // `dequantize_block_into`'s catch-all panic instead of returning.
        let (elems, bytes) = GgmlType::Q5K.block_info().expect("Q5_K has a known block layout");
        let raw = vec![0u8; bytes as usize];
        match dequantize(GgmlType::Q5K, &raw, elems as usize) {
            Err(DequantError::Unsupported(GgmlType::Q5K)) => {}
            other => panic!("expected Unsupported(Q5K), got {other:?}"),
        }
    }

    #[test]
    fn implemented_types_match_dequantize_block_into() {
        // `is_implemented` is a hand-written match that has to mirror the one
        // in `dequantize_block_into`. Two hand-written matches drift, so this
        // drives every known type through BOTH and fails if they disagree —
        // the drift would otherwise show up as a panic in production, which
        // is exactly what `is_implemented` exists to prevent.
        use GgmlType::*;
        let candidates = [
            F32, F16, Bf16, F64, I8, I16, I32, I64, Q4_0, Q4_1, Q5_0, Q5_1, Q8_0, Q8_1, Q2K, Q3K,
            Q4K, Q5K, Q6K, Q8K,
        ];

        // Silence the panic output — the catch_unwind hits below are expected,
        // and their backtraces would drown the real test output.
        let prev_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));

        let mut mismatches = Vec::new();
        for ty in candidates {
            let Some((block_elems, block_bytes)) = ty.block_info() else {
                continue; // no layout to size a buffer from — nothing to drive
            };
            let raw = vec![0u8; block_bytes as usize];
            let mut out = vec![0f32; block_elems as usize];
            let did_not_panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                dequantize_block_into(ty, &raw, &mut out);
            }))
            .is_ok();

            if did_not_panic != is_implemented(ty) {
                mismatches.push(format!(
                    "{}: is_implemented()={}, but dequantize_block_into() {}",
                    ty.name(),
                    is_implemented(ty),
                    if did_not_panic { "succeeded" } else { "panicked" }
                ));
            }
        }

        std::panic::set_hook(prev_hook);
        assert!(mismatches.is_empty(), "is_implemented() out of sync:\n{}", mismatches.join("\n"));
    }
}
