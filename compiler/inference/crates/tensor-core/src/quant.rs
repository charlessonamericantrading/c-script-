//! Fase 9 of the optimization roadmap: native quantized-domain dot
//! product. `ops::linear_quantized_range` normally dequantizes each
//! weight block to f32 and dots it against the (already f32) activation
//! row — that dequant-then-f32-dot round trip is the structural reason
//! this engine does more work per matmul than llama.cpp, which dots
//! directly against packed weight bytes.
//!
//! This module quantizes the ACTIVATION side too (to an int8-per-element,
//! f32-scale-per-32-block format, matching Q5_0/Q8_0's own block size) so
//! the hot inner loop becomes an integer dot product — no float rounding
//! in the accumulation, only in the two scale multiplies applied once per
//! block at the end. Fase 9 covered Q5_0 (symmetric: `w = d*q`, the
//! format `ffn_gate`/`ffn_up` actually use, 44.2% of qwen2.5:0.5b's
//! weight bytes). Fase 10 adds Q4_K — genuinely a third kernel family,
//! not a copy of Q5_0's: Q4_K is AFFINE (`w = d*q - m`, both a per-
//! sub-block scale AND min, q unsigned in [0,15] rather than centered/
//! signed), audited (see hidden-marinating-thimble.md) as the one format
//! present in all 3 real checkpoints (7.5% / 73.3% / 20.2% of weight
//! bytes) — the only one where a win actually reaches qwen2.5-coder:7b
//! and gemma4:e4b, unlike Q5_0 which is 0% there.

use crate::f16::f16_to_f32;

pub const QBLOCK: usize = 32;

/// An activation row quantized to int8-per-element + one f32 scale per
/// 32-element block, aligned with Q5_0/Q8_0's own block size. Quantized
/// ONCE per `linear_quantized_range` call (not once per output row) —
/// the whole point is amortizing this second, lossy quantization step
/// over every output feature that reads the same input row.
///
/// `bsums[b]` = `sum(values[b*QBLOCK..(b+1)*QBLOCK])` (as i32) — needed
/// only by Q4_K's affine dot product (`d*sum(q*x) - m*sum(x)`, the second
/// term needs this per-block activation sum), but computed for every row
/// regardless of which weight format ends up using it: it's a cheap O(n)
/// running sum added to the same loop that already computes `values`, and
/// keeping one `QuantizedRow` shape shared by both Q5_0 and Q4_K avoids a
/// second quantization pass over the same data.
pub struct QuantizedRow {
    pub scales: Vec<f32>,
    pub values: Vec<i8>,
    pub bsums: Vec<i32>,
}

/// Quantizes `x` (one activation row) to `QuantizedRow`. Per-block
/// scale = max_abs / 127 (the standard symmetric int8 scheme — ggml's own
/// `quantize_row_q8_0`, `ggml/src/ggml-quants.c`, uses the same
/// max_abs/127 formula), values rounded to nearest and clamped to
/// [-127, 127] (not -128, so the range is symmetric — again matching
/// ggml's own choice, avoids a -128 with no positive counterpart).
pub fn quantize_row_q8(x: &[f32]) -> QuantizedRow {
    let n_blocks = x.len().div_ceil(QBLOCK);
    let mut scales = Vec::with_capacity(n_blocks);
    let mut values = vec![0i8; x.len()];
    let mut bsums = Vec::with_capacity(n_blocks);
    for b in 0..n_blocks {
        let start = b * QBLOCK;
        let end = (start + QBLOCK).min(x.len());
        let block = &x[start..end];
        let max_abs = block.iter().fold(0f32, |acc, &v| acc.max(v.abs()));
        let scale = if max_abs > 0.0 { max_abs / 127.0 } else { 1.0 };
        scales.push(scale);
        let inv_scale = 1.0 / scale;
        let mut bsum = 0i32;
        for (i, &v) in block.iter().enumerate() {
            let q = (v * inv_scale).round().clamp(-127.0, 127.0) as i8;
            values[start + i] = q;
            bsum += q as i32;
        }
        bsums.push(bsum);
    }
    QuantizedRow { scales, values, bsums }
}

/// Native Q5_0 (weight) × Q8-quantized (activation) dot product for ONE
/// 32-element block — no dequantization of the weight block to f32.
/// `w_block` is a raw Q5_0 block (22 bytes: `fp16 d; u8 qh[4]; u8
/// qs[16]`, see `dequant.rs`'s `dequant_q5_0_block_scalar` for the
/// authoritative bit-layout this mirrors). `x_vals`/`x_scale` are the
/// matching 32-wide slice of an already-`quantize_row_q8`'d activation
/// row. Returns the block's contribution to the dot product (already
/// scaled by both `d_w` and `x_scale` — the caller just sums these
/// across blocks, no further scaling needed).
///
/// Dispatches to AVX2 when available (this machine has it, no VNNI/
/// AVX-512), falls back to the scalar loop otherwise — same runtime
/// `is_x86_feature_detected!` pattern as `simd::dot_product`.
pub fn dot_q5_0_q8_block(w_block: &[u8], x_vals: &[i8], x_scale: f32) -> f32 {
    debug_assert_eq!(w_block.len(), 22);
    debug_assert_eq!(x_vals.len(), QBLOCK);
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            return unsafe { x86::dot_q5_0_q8_avx2(w_block, x_vals, x_scale) };
        }
    }
    dot_q5_0_q8_block_scalar(w_block, x_vals, x_scale)
}

/// Native Q8_0 (weight) x Q8-quantized (activation) dot product for ONE
/// 32-element block (34 bytes: `fp16 d; i8 qs[32]` -- see
/// `dequant.rs`/`dequant_q8_0_block_scalar` for the authoritative layout).
/// Simpler than Q5_0/Q4_K/Q6_K: `qs` is already plain signed int8, no
/// nibble/high-bit unpacking needed at all, just a straight int8 dot
/// product scaled once at the end (`w = d*q`, no affine bias). Q8_0 is
/// 37.3% of qwen2.5:0.5b's weight bytes (0% in coder:7b/gemma4:e4b, same
/// profile as Q5_0 -- Fase 8's audit) and had no native kernel until now,
/// unlike every other format with real weight in any of the 3 checkpoints.
pub fn dot_q8_0_q8_block(w_block: &[u8], x_vals: &[i8], x_scale: f32) -> f32 {
    debug_assert_eq!(w_block.len(), 34);
    debug_assert_eq!(x_vals.len(), QBLOCK);
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            return unsafe { x86::dot_q8_0_q8_avx2(w_block, x_vals, x_scale) };
        }
    }
    dot_q8_0_q8_block_scalar(w_block, x_vals, x_scale)
}

pub fn dot_q8_0_q8_block_scalar(w_block: &[u8], x_vals: &[i8], x_scale: f32) -> f32 {
    let d_w = f16_to_f32(u16::from_le_bytes([w_block[0], w_block[1]]));
    let mut total = 0i32;
    for j in 0..32 {
        total += (w_block[2 + j] as i8) as i32 * x_vals[j] as i32;
    }
    total as f32 * d_w * x_scale
}

/// Dots an unquantized f32 row `q` against a `QuantizedRow` `k` (Q8_0 format).
pub fn dot_q8_row(q: &[f32], k: &QuantizedRow) -> f32 {
    debug_assert_eq!(q.len(), k.values.len());
    let n_blocks = k.scales.len();
    let mut total = 0f32;
    for b in 0..n_blocks {
        let start = b * QBLOCK;
        let end = (start + QBLOCK).min(q.len());
        let q_block = &q[start..end];
        let k_block = &k.values[start..end];
        let mut sum = 0f32;
        for i in 0..q_block.len() {
            sum += q_block[i] * (k_block[i] as f32);
        }
        total += sum * k.scales[b];
    }
    total
}

/// Dots two `QuantizedRow`s (both Q8_0 format) in integer domain.
pub fn dot_q8_q8_row(q: &QuantizedRow, k: &QuantizedRow) -> f32 {
    debug_assert_eq!(q.values.len(), k.values.len());
    let n_blocks = q.scales.len();
    let mut total = 0f32;
    for b in 0..n_blocks {
        let start = b * QBLOCK;
        let end = (start + QBLOCK).min(q.values.len());
        let q_vals = &q.values[start..end];
        let k_vals = &k.values[start..end];
        let mut isum = 0i32;
        for i in 0..q_vals.len() {
            isum += (q_vals[i] as i32) * (k_vals[i] as i32);
        }
        total += isum as f32 * q.scales[b] * k.scales[b];
    }
    total
}

pub fn dot_q5_0_q8_block_scalar(w_block: &[u8], x_vals: &[i8], x_scale: f32) -> f32 {
    let d_w = f16_to_f32(u16::from_le_bytes([w_block[0], w_block[1]]));
    let qh = u32::from_le_bytes([w_block[2], w_block[3], w_block[4], w_block[5]]);
    let qs = &w_block[6..22];
    let mut acc: i32 = 0;
    for j in 0..16 {
        let xh_0 = ((qh >> j) << 4) & 0x10;
        let xh_1 = (qh >> (j + 12)) & 0x10;
        let w0 = ((qs[j] & 0x0F) as u32 | xh_0) as i32 - 16;
        let w1 = ((qs[j] >> 4) as u32 | xh_1) as i32 - 16;
        acc += w0 * x_vals[j] as i32;
        acc += w1 * x_vals[j + 16] as i32;
    }
    acc as f32 * d_w * x_scale
}

/// Native Q4_K (weight) × Q8-quantized (activation) dot product for ONE
/// 256-element super-block (8 sub-blocks of 32, each with its own affine
/// `(scale, min)` pair) — no dequantization of the weight block to f32.
/// `w_block` is a raw Q4_K super-block (144 bytes: `fp16 d; fp16 dmin;
/// u8 scales[12]; u8 qs[128]`, see `dequant.rs`'s `get_scale_min_k4` and
/// `simd::dequant_q4_k_block_scalar` for the authoritative bit-layout
/// this mirrors). Unlike Q5_0 (symmetric, `w = d*q`), Q4_K is AFFINE
/// (`w = d*q - m`), so each sub-block's contribution needs two integer
/// reductions — `sum(q*x)` (the actual quantized dot) and `sum(x)`
/// (`x.bsums`, precomputed once per row in `quantize_row_q8`) — combined
/// as `d*sum(q*x) - m*sum(x)`. `x_block_start` is this super-block's
/// starting element offset within the (already-quantized) activation
/// row `x`; must be a multiple of `QBLOCK` since Q4_K's 8 sub-blocks
/// align exactly with 8 of `x`'s 32-wide activation blocks.
pub fn dot_q4_k_super_block(w_block: &[u8], x: &QuantizedRow, x_block_start: usize) -> f32 {
    debug_assert_eq!(w_block.len(), 144);
    debug_assert_eq!(x_block_start % QBLOCK, 0);
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            return unsafe { x86::dot_q4_k_super_block_avx2(w_block, x, x_block_start) };
        }
    }
    dot_q4_k_super_block_scalar(w_block, x, x_block_start)
}

pub fn dot_q4_k_super_block_scalar(w_block: &[u8], x: &QuantizedRow, x_block_start: usize) -> f32 {
    let d = f16_to_f32(u16::from_le_bytes([w_block[0], w_block[1]]));
    let dmin = f16_to_f32(u16::from_le_bytes([w_block[2], w_block[3]]));
    let scales = &w_block[4..16];
    let qs = &w_block[16..144];
    let base_block = x_block_start / QBLOCK;

    let mut total = 0f32;
    let mut q_off = 0usize;
    let mut is = 0usize;
    for group in 0..4 {
        let (sc1, m1) = crate::dequant::get_scale_min_k4(is, scales);
        let (sc2, m2) = crate::dequant::get_scale_min_k4(is + 1, scales);

        let sub1 = base_block + group * 2;
        let sub2 = base_block + group * 2 + 1;
        let x1 = &x.values[sub1 * QBLOCK..(sub1 + 1) * QBLOCK];
        let x2 = &x.values[sub2 * QBLOCK..(sub2 + 1) * QBLOCK];

        let mut dot1: i32 = 0;
        let mut dot2: i32 = 0;
        for l in 0..32 {
            dot1 += (qs[q_off + l] & 0x0F) as i32 * x1[l] as i32;
            dot2 += (qs[q_off + l] >> 4) as i32 * x2[l] as i32;
        }

        // dot1/bsum1 are in the quantized (int) domain -- x.values, not
        // the real activation values -- so they still need scaling back
        // by x.scales[sub] to land in the real domain, same as Q5_0's
        // x_scale multiply. Missed on the first pass here (caught by
        // dot_q4_k_matches_dequant_then_f32_dot_within_quantization_tolerance
        // failing by ~68x, not by a small tolerance overshoot — the AVX2-
        // vs-scalar test didn't catch it because both paths shared the
        // identical bug).
        total += x.scales[sub1] * (d * sc1 as f32 * dot1 as f32 - dmin * m1 as f32 * x.bsums[sub1] as f32);
        total += x.scales[sub2] * (d * sc2 as f32 * dot2 as f32 - dmin * m2 as f32 * x.bsums[sub2] as f32);

        q_off += 32;
        is += 2;
    }
    total
}

/// Native Q6_K (weight) x Q8-quantized (activation) dot product for ONE
/// 256-element super-block (210 bytes: 128 `ql` + 64 `qh` + 16 signed
/// int8 `scales` + 2-byte f16 `d` -- see `simd::dequant_q6_k_block_scalar`
/// for the authoritative bit-layout this mirrors, same unpacking, dot
/// instead of dequant-then-write).
///
/// Q6_K's sub-block granularity is 16 elements, not 32 like Q4_K/Q5_0 --
/// but each 16-element sub-block always falls entirely within one half of
/// a `QBLOCK`-sized (32) activation block (16 divides 32), so it can still
/// share `QuantizedRow`'s per-32-block scale without needing a different
/// activation quantization pass. Unlike Q4_K, Q6_K's affine offset is a
/// fixed constant (-32 on the raw unsigned 6-bit value), not a second
/// per-sub-block quantity -- so it's baked directly into each unpacked
/// value before the integer multiply, with no separate bsums term needed
/// (Q4_K needs bsums specifically because its `min` varies per sub-block
/// independently of `scale`; Q6_K's bias doesn't).
pub fn dot_q6_k_super_block(w_block: &[u8], x: &QuantizedRow, x_block_start: usize) -> f32 {
    debug_assert_eq!(w_block.len(), 210);
    debug_assert_eq!(x_block_start % QBLOCK, 0);
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            return unsafe { x86::dot_q6_k_super_block_avx2(w_block, x, x_block_start) };
        }
    }
    dot_q6_k_super_block_scalar(w_block, x, x_block_start)
}

pub fn dot_q6_k_super_block_scalar(w_block: &[u8], x: &QuantizedRow, x_block_start: usize) -> f32 {
    let ql_all = &w_block[0..128];
    let qh_all = &w_block[128..192];
    let sc_all = &w_block[192..208];
    let d = f16_to_f32(u16::from_le_bytes([w_block[208], w_block[209]]));

    let base_block = x_block_start / QBLOCK;
    let mut total = 0f32;

    for outer in 0..2 {
        let ql = &ql_all[outer * 64..outer * 64 + 64];
        let qh = &qh_all[outer * 32..outer * 32 + 32];
        let sc = &sc_all[outer * 8..outer * 8 + 8];
        // This outer half covers 4 consecutive 32-element activation
        // blocks (128 elements / QBLOCK).
        let outer_base_block = base_block + outer * 4;

        for is in 0..2 {
            // is=0 -> the low 16 elements of each of the 4 blocks below,
            // is=1 -> the high 16.
            let l0 = is * 16;
            let block1 = outer_base_block;
            let block2 = outer_base_block + 1;
            let block3 = outer_base_block + 2;
            let block4 = outer_base_block + 3;
            let x1 = &x.values[block1 * QBLOCK + l0..block1 * QBLOCK + l0 + 16];
            let x2 = &x.values[block2 * QBLOCK + l0..block2 * QBLOCK + l0 + 16];
            let x3 = &x.values[block3 * QBLOCK + l0..block3 * QBLOCK + l0 + 16];
            let x4 = &x.values[block4 * QBLOCK + l0..block4 * QBLOCK + l0 + 16];

            let mut dot1 = 0i32;
            let mut dot2 = 0i32;
            let mut dot3 = 0i32;
            let mut dot4 = 0i32;
            #[allow(clippy::identity_op)] // mirrors dequant_q6_k_block_scalar's >>0/>>2/>>4/>>6 visual symmetry
            for k in 0..16 {
                let l = l0 + k;
                let q1 = ((ql[l] & 0x0F) | (((qh[l] >> 0) & 3) << 4)) as i32 - 32;
                let q2 = ((ql[l + 32] & 0x0F) | (((qh[l] >> 2) & 3) << 4)) as i32 - 32;
                let q3 = ((ql[l] >> 4) | (((qh[l] >> 4) & 3) << 4)) as i32 - 32;
                let q4 = ((ql[l + 32] >> 4) | (((qh[l] >> 6) & 3) << 4)) as i32 - 32;

                dot1 += q1 * x1[k] as i32;
                dot2 += q2 * x2[k] as i32;
                dot3 += q3 * x3[k] as i32;
                dot4 += q4 * x4[k] as i32;
            }

            total += x.scales[block1] * d * (sc[is] as i8) as f32 * dot1 as f32;
            total += x.scales[block2] * d * (sc[is + 2] as i8) as f32 * dot2 as f32;
            total += x.scales[block3] * d * (sc[is + 4] as i8) as f32 * dot3 as f32;
            total += x.scales[block4] * d * (sc[is + 6] as i8) as f32 * dot4 as f32;
        }
    }
    total
}

#[cfg(target_arch = "x86_64")]
mod x86 {
    use std::arch::x86_64::*;

    use super::f16_to_f32;

    /// # Safety
    /// Caller must have verified `is_x86_feature_detected!("avx2")`.
    /// `w_block` must be exactly 22 bytes (Q5_0's block size), `x_vals`
    /// exactly 32 elements (`super::QBLOCK`).
    #[target_feature(enable = "avx2")]
    pub unsafe fn dot_q5_0_q8_avx2(w_block: &[u8], x_vals: &[i8], x_scale: f32) -> f32 {
        let d_w = f16_to_f32(u16::from_le_bytes([w_block[0], w_block[1]]));
        let qh = u32::from_le_bytes([w_block[2], w_block[3], w_block[4], w_block[5]]);
        let qs = &w_block[6..22];

        // Unpack the 5-bit signed weight values (range [-16,15]) to i8 —
        // this is NOT dequantizing to f32, just widening the packed
        // nibble/high-bit representation to one byte per element, the
        // minimum needed to feed the integer dot product below.
        let mut w_i8 = [0i8; 32];
        for j in 0..16 {
            let xh_0 = ((qh >> j) << 4) & 0x10;
            let xh_1 = (qh >> (j + 12)) & 0x10;
            w_i8[j] = (((qs[j] & 0x0F) as u32 | xh_0) as i32 - 16) as i8;
            w_i8[j + 16] = (((qs[j] >> 4) as u32 | xh_1) as i32 - 16) as i8;
        }

        let wv = _mm256_loadu_si256(w_i8.as_ptr() as *const __m256i);
        let xv = _mm256_loadu_si256(x_vals.as_ptr() as *const __m256i);

        // vpmaddubsw (_mm256_maddubs_epi16) requires its first operand
        // UNSIGNED — both w and x here are signed. Standard technique
        // (the same one ggml/llama.cpp uses for signed×signed int8 dot
        // products without VNNI): ax = |w| (unsigned), sy = sign(w)·x
        // (still signed) — then ax·sy = |w|·sign(w)·x = w·x, with the
        // correct sign preserved. No overflow risk: |w|<=16, |x|<=127,
        // so each pairwise-summed i16 lane maxes at 2*16*127=4064, far
        // under i16's saturation point.
        let ax = _mm256_abs_epi8(wv);
        let sy = _mm256_sign_epi8(xv, wv);
        let dot16 = _mm256_maddubs_epi16(ax, sy); // 16 x i16, adjacent pairs summed

        // Widen 16xi16 -> 8xi32 by summing adjacent pairs again (madd
        // against all-ones is the standard idiom for this widening step).
        let ones = _mm256_set1_epi16(1);
        let dot32 = _mm256_madd_epi16(dot16, ones);

        // Horizontal sum of the 8 i32 lanes.
        let sum128 = _mm_add_epi32(_mm256_castsi256_si128(dot32), _mm256_extracti128_si256(dot32, 1));
        let hi64 = _mm_unpackhi_epi64(sum128, sum128);
        let sum64 = _mm_add_epi32(sum128, hi64);
        let hi32 = _mm_shuffle_epi32(sum64, 0b01);
        let sum32 = _mm_add_epi32(sum64, hi32);
        let total = _mm_cvtsi128_si32(sum32);

        total as f32 * d_w * x_scale
    }

    /// # Safety
    /// Caller must have verified `is_x86_feature_detected!("avx2")`.
    /// `w_block` must be exactly 34 bytes (Q8_0's block size), `x_vals`
    /// exactly 32 elements (`super::QBLOCK`).
    ///
    /// Same maddubs+abs/sign technique as `dot_q5_0_q8_avx2` (both operands
    /// signed, `maddubs` needs an unsigned first operand), minus the
    /// nibble/high-bit unpacking step Q5_0 needs -- Q8_0's `qs` is already
    /// plain signed int8, loadable directly. Overflow check: weight in
    /// [-128,127], activation clamped to [-127,127] by `quantize_row_q8`
    /// (not -128), so max |product| = 128*127=16256, and `maddubs` sums
    /// adjacent PAIRS into i16 -- worst case 2*16256=32512, still under
    /// i16::MAX (32767) despite the tighter margin than Q5_0's ~16x
    /// smaller weight range.
    #[target_feature(enable = "avx2")]
    pub unsafe fn dot_q8_0_q8_avx2(w_block: &[u8], x_vals: &[i8], x_scale: f32) -> f32 {
        let d_w = f16_to_f32(u16::from_le_bytes([w_block[0], w_block[1]]));
        let wv = _mm256_loadu_si256(w_block[2..34].as_ptr() as *const __m256i);
        let xv = _mm256_loadu_si256(x_vals.as_ptr() as *const __m256i);

        let ax = _mm256_abs_epi8(wv);
        let sy = _mm256_sign_epi8(xv, wv);
        let dot16 = _mm256_maddubs_epi16(ax, sy);
        let ones = _mm256_set1_epi16(1);
        let dot32 = _mm256_madd_epi16(dot16, ones);

        let sum128 = _mm_add_epi32(_mm256_castsi256_si128(dot32), _mm256_extracti128_si256(dot32, 1));
        let hi64 = _mm_unpackhi_epi64(sum128, sum128);
        let sum64 = _mm_add_epi32(sum128, hi64);
        let hi32 = _mm_shuffle_epi32(sum64, 0b01);
        let sum32 = _mm_add_epi32(sum64, hi32);
        let total = _mm_cvtsi128_si32(sum32);

        total as f32 * d_w * x_scale
    }

    /// # Safety
    /// Caller must have verified `is_x86_feature_detected!("avx2")`.
    /// `w_u4` and `x_vals` must both be exactly 32 elements.
    #[target_feature(enable = "avx2")]
    unsafe fn dot_u4_i8_32(w_u4: &[u8], x_vals: &[i8]) -> i32 {
        let wv = _mm256_loadu_si256(w_u4.as_ptr() as *const __m256i);
        let xv = _mm256_loadu_si256(x_vals.as_ptr() as *const __m256i);
        // Q4_K's nibbles are already unsigned [0,15] (no offset-by-16 the
        // way Q5_0's are) — they can feed maddubs' unsigned operand
        // directly, no abs/sign trick needed here (that trick was only
        // for Q5_0's signed weight range).
        let dot16 = _mm256_maddubs_epi16(wv, xv);
        let ones = _mm256_set1_epi16(1);
        let dot32 = _mm256_madd_epi16(dot16, ones);
        let sum128 = _mm_add_epi32(_mm256_castsi256_si128(dot32), _mm256_extracti128_si256(dot32, 1));
        let hi64 = _mm_unpackhi_epi64(sum128, sum128);
        let sum64 = _mm_add_epi32(sum128, hi64);
        let hi32 = _mm_shuffle_epi32(sum64, 0b01);
        let sum32 = _mm_add_epi32(sum64, hi32);
        _mm_cvtsi128_si32(sum32)
    }

    /// # Safety
    /// Caller must have verified `is_x86_feature_detected!("avx2")`.
    /// `w_block` must be exactly 144 bytes (Q4_K's super-block size),
    /// `x_block_start` a multiple of `super::QBLOCK`.
    #[target_feature(enable = "avx2")]
    pub unsafe fn dot_q4_k_super_block_avx2(w_block: &[u8], x: &super::QuantizedRow, x_block_start: usize) -> f32 {
        let d = f16_to_f32(u16::from_le_bytes([w_block[0], w_block[1]]));
        let dmin = f16_to_f32(u16::from_le_bytes([w_block[2], w_block[3]]));
        let scales = &w_block[4..16];
        let qs = &w_block[16..144];
        let base_block = x_block_start / super::QBLOCK;

        let mut total = 0f32;
        let mut q_off = 0usize;
        let mut is = 0usize;
        for group in 0..4 {
            let (sc1, m1) = crate::dequant::get_scale_min_k4(is, scales);
            let (sc2, m2) = crate::dequant::get_scale_min_k4(is + 1, scales);

            let sub1 = base_block + group * 2;
            let sub2 = base_block + group * 2 + 1;

            let mut w_lo = [0u8; 32];
            let mut w_hi = [0u8; 32];
            for l in 0..32 {
                w_lo[l] = qs[q_off + l] & 0x0F;
                w_hi[l] = qs[q_off + l] >> 4;
            }

            let dot1 = dot_u4_i8_32(&w_lo, &x.values[sub1 * super::QBLOCK..(sub1 + 1) * super::QBLOCK]);
            let dot2 = dot_u4_i8_32(&w_hi, &x.values[sub2 * super::QBLOCK..(sub2 + 1) * super::QBLOCK]);

            // See the scalar path's comment: dot1/bsum1 are in the
            // quantized (int) domain, need x.scales[sub] to land in the
            // real domain.
            total += x.scales[sub1] * (d * sc1 as f32 * dot1 as f32 - dmin * m1 as f32 * x.bsums[sub1] as f32);
            total += x.scales[sub2] * (d * sc2 as f32 * dot2 as f32 - dmin * m2 as f32 * x.bsums[sub2] as f32);

            q_off += 32;
            is += 2;
        }
        total
    }

    /// Dots 16 signed weight bytes against 16 signed activation bytes via
    /// `maddubs` (same abs/sign trick as `dot_q5_0_q8_avx2`/
    /// `dot_q8_0_q8_avx2`: both operands signed, `maddubs` needs an
    /// unsigned first operand). 128-bit width, not 256: Q6_K's natural
    /// sub-block granularity is 16 elements, and two different 16-element
    /// sub-blocks always have DIFFERENT scales (`sc[is]` vs `sc[is+2]` are
    /// never equal in general) -- packing two of them into one 256-bit
    /// maddubs call would need one scale multiply to cover elements
    /// governed by two different scales, which is simply the wrong
    /// answer, not a rounding nuance. 128-bit width respects the format's
    /// actual granularity instead of forcing a mismatched packing.
    ///
    /// # Safety
    /// Caller must have verified `is_x86_feature_detected!("avx2")` (this
    /// only needs SSSE3, but the call site's guard is strictly stronger,
    /// and every other kernel in this file gates on avx2 for consistency).
    /// `w`/`x` must both be exactly 16 elements.
    #[target_feature(enable = "avx2")]
    unsafe fn dot_i8_16_maddubs(w: &[i8], x: &[i8]) -> i32 {
        let wv = _mm_loadu_si128(w.as_ptr() as *const __m128i);
        let xv = _mm_loadu_si128(x.as_ptr() as *const __m128i);
        let ax = _mm_abs_epi8(wv);
        let sy = _mm_sign_epi8(xv, wv);
        let dot16 = _mm_maddubs_epi16(ax, sy); // 8 x i16, adjacent pairs summed
        let ones = _mm_set1_epi16(1);
        let dot32 = _mm_madd_epi16(dot16, ones); // 4 x i32
        let hi64 = _mm_unpackhi_epi64(dot32, dot32);
        let sum64 = _mm_add_epi32(dot32, hi64);
        let hi32 = _mm_shuffle_epi32(sum64, 0b01);
        let sum32 = _mm_add_epi32(sum64, hi32);
        _mm_cvtsi128_si32(sum32)
    }

    /// # Safety
    /// Caller must have verified `is_x86_feature_detected!("avx2")`.
    /// `w_block` must be exactly 210 bytes (Q6_K's super-block size),
    /// `x_block_start` a multiple of `super::QBLOCK`.
    ///
    /// Bit-unpacking the 4 segments' 16 signed weight values each is done
    /// scalar into small stack buffers, not SIMD: SSE/AVX2 has no per-byte
    /// variable shift instruction (only per-dword via `_mm256_srli_epi32`,
    /// which is what the earlier widen-to-i32 version used), so unpacking
    /// this specific bit-packed 6-bit format at byte width isn't a clean
    /// single SIMD op the way widening to i32 was. Cheap next to the
    /// maddubs multiply-reduce it feeds (16 elements x ~5 ALU ops each,
    /// versus the widen-to-i32 version's prior approach of doing the
    /// unpacking itself in SIMD but the multiply less efficiently).
    #[target_feature(enable = "avx2")]
    pub unsafe fn dot_q6_k_super_block_avx2(w_block: &[u8], x: &super::QuantizedRow, x_block_start: usize) -> f32 {
        let ql_all = &w_block[0..128];
        let qh_all = &w_block[128..192];
        let sc_all = &w_block[192..208];
        let d = f16_to_f32(u16::from_le_bytes([w_block[208], w_block[209]]));

        let base_block = x_block_start / super::QBLOCK;
        let mut total = 0f32;

        for outer in 0..2 {
            let ql = &ql_all[outer * 64..outer * 64 + 64];
            let qh = &qh_all[outer * 32..outer * 32 + 32];
            let sc = &sc_all[outer * 8..outer * 8 + 8];
            let outer_base_block = base_block + outer * 4;

            for is in 0..2 {
                let l0 = is * 16;
                let block1 = outer_base_block;
                let block2 = outer_base_block + 1;
                let block3 = outer_base_block + 2;
                let block4 = outer_base_block + 3;

                let mut w1 = [0i8; 16];
                let mut w2 = [0i8; 16];
                let mut w3 = [0i8; 16];
                let mut w4 = [0i8; 16];
                #[allow(clippy::identity_op)] // mirrors dequant_q6_k_block_scalar's >>0/>>2/>>4/>>6 visual symmetry
                for k in 0..16 {
                    let l = l0 + k;
                    w1[k] = (((ql[l] & 0x0F) | (((qh[l] >> 0) & 3) << 4)) as i32 - 32) as i8;
                    w2[k] = (((ql[l + 32] & 0x0F) | (((qh[l] >> 2) & 3) << 4)) as i32 - 32) as i8;
                    w3[k] = (((ql[l] >> 4) | (((qh[l] >> 4) & 3) << 4)) as i32 - 32) as i8;
                    w4[k] = (((ql[l + 32] >> 4) | (((qh[l] >> 6) & 3) << 4)) as i32 - 32) as i8;
                }

                let dot1 = dot_i8_16_maddubs(&w1, &x.values[block1 * super::QBLOCK + l0..block1 * super::QBLOCK + l0 + 16]);
                let dot2 = dot_i8_16_maddubs(&w2, &x.values[block2 * super::QBLOCK + l0..block2 * super::QBLOCK + l0 + 16]);
                let dot3 = dot_i8_16_maddubs(&w3, &x.values[block3 * super::QBLOCK + l0..block3 * super::QBLOCK + l0 + 16]);
                let dot4 = dot_i8_16_maddubs(&w4, &x.values[block4 * super::QBLOCK + l0..block4 * super::QBLOCK + l0 + 16]);

                total += x.scales[block1] * d * (sc[is] as i8) as f32 * dot1 as f32;
                total += x.scales[block2] * d * (sc[is + 2] as i8) as f32 * dot2 as f32;
                total += x.scales[block3] * d * (sc[is + 4] as i8) as f32 * dot3 as f32;
                total += x.scales[block4] * d * (sc[is + 6] as i8) as f32 * dot4 as f32;
            }
        }
        total
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simd::{dequant_q4_k_block_scalar, dequant_q5_0_block_scalar, dot_product_scalar};

    fn make_q4_k_block(seed: f32) -> Vec<u8> {
        // 144 bytes: fp16 d, fp16 dmin, u8 scales[12], u8 qs[128] — varied
        // bit patterns across d/dmin/scales/qs, same synthetic-but-varied
        // style as make_q5_0_block, exercising the full 6-bit scale/min
        // and 4-bit nibble range rather than a trivial all-zero block.
        let d: u16 = 0x3400 + (seed as u16 % 64);
        let dmin: u16 = 0x3200 + (seed as u16 % 48);
        let mut raw = vec![0u8; 144];
        raw[0..2].copy_from_slice(&d.to_le_bytes());
        raw[2..4].copy_from_slice(&dmin.to_le_bytes());
        for i in 0..12 {
            // Keep top 2 bits variable too (get_scale_min_k4 reads all 6
            // low bits of scales[0..4] directly, and borrows high bits
            // from scales[4..8]/[0..4] for indices >=4 — vary the whole
            // byte so both paths get exercised, not just the low 6 bits).
            raw[4 + i] = ((seed as u32).wrapping_mul(2246822519).wrapping_add(i as u32 * 3) % 256) as u8;
        }
        for i in 0..128 {
            raw[16 + i] = ((seed as u32).wrapping_mul(40503).wrapping_add(i as u32 * 11) % 256) as u8;
        }
        raw
    }

    fn make_q6_k_block(seed: f32) -> Vec<u8> {
        // 210 bytes: u8 ql[128], u8 qh[64], i8 scales[16], fp16 d. Same
        // synthetic-but-varied hashing style as make_q4_k_block/
        // make_q5_0_block. `scales` is deliberately varied across the
        // full byte range (not just small positive values) so some
        // entries have the high bit set -- q6_k_scales_are_signed (in
        // dequant.rs) already caught a real bug where this was read
        // unsigned; this generator must keep exercising that case.
        let d: u16 = 0x3400 + (seed as u16 % 64);
        let mut raw = vec![0u8; 210];
        for i in 0..128 {
            raw[i] = ((seed as u32).wrapping_mul(2246822519).wrapping_add(i as u32 * 7) % 256) as u8;
        }
        for i in 0..64 {
            raw[128 + i] = ((seed as u32).wrapping_mul(2654435761).wrapping_add(i as u32 * 5) % 256) as u8;
        }
        for i in 0..16 {
            raw[192 + i] = ((seed as u32).wrapping_mul(40503).wrapping_add(i as u32 * 13) % 256) as u8;
        }
        raw[208..210].copy_from_slice(&d.to_le_bytes());
        raw
    }

    fn make_q5_0_block(seed: f32) -> Vec<u8> {
        // Same synthetic-but-varied pattern style used elsewhere in this
        // crate's tests (e.g. simd.rs's dequant tests) -- exercises the
        // full nibble/high-bit range, not just a trivial all-zero block.
        let d: u16 = 0x3400 + (seed as u16 % 64); // varied small fp16 values
        let mut raw = vec![0u8; 22];
        raw[0..2].copy_from_slice(&d.to_le_bytes());
        for i in 0..4 {
            raw[2 + i] = ((seed as u32).wrapping_mul(2654435761).wrapping_add(i as u32) % 256) as u8;
        }
        for i in 0..16 {
            raw[6 + i] = ((seed as u32).wrapping_mul(40503).wrapping_add(i as u32 * 7) % 256) as u8;
        }
        raw
    }

    #[test]
    fn quantize_row_q8_round_trips_within_expected_error() {
        let x: Vec<f32> = (0..64).map(|i| ((i as f32) * 0.37 - 10.0).sin() * 5.0).collect();
        let q = quantize_row_q8(&x);
        assert_eq!(q.scales.len(), 2);
        assert_eq!(q.values.len(), 64);
        for (i, &orig) in x.iter().enumerate() {
            let block = i / QBLOCK;
            let recon = q.values[i] as f32 * q.scales[block];
            let max_abs_in_block = x[block * QBLOCK..((block + 1) * QBLOCK).min(x.len())].iter().fold(0f32, |a, &v| a.max(v.abs()));
            let tol = (max_abs_in_block / 127.0) * 1.01; // one quantization step, small margin
            assert!((recon - orig).abs() <= tol, "i={i}: orig={orig} recon={recon} tol={tol}");
        }
    }

    #[test]
    fn dot_q5_0_q8_avx2_matches_scalar_bit_exactly() {
        // Unlike the dequant-then-f32-dot comparison below (genuinely
        // lossy, needs a tolerance), AVX2 vs scalar here must be
        // BIT-EXACT: both compute the exact same integer sum (no
        // floating-point reordering happens until the single final
        // scale multiply, which is identical in both paths since it
        // starts from the same integer total).
        for seed in [0.0, 1.0, 7.5, 13.25, 31.75, 63.0, 100.0] {
            let w_block = make_q5_0_block(seed);
            let x_vals: Vec<i8> = (0..32).map(|i| (((i as i32 + seed as i32) * 37) % 255 - 127) as i8).collect();
            let x_scale = 0.013 + seed * 0.001;

            let scalar = dot_q5_0_q8_block_scalar(&w_block, &x_vals, x_scale);
            let dispatched = dot_q5_0_q8_block(&w_block, &x_vals, x_scale);
            assert_eq!(scalar, dispatched, "seed={seed}: scalar={scalar} dispatched={dispatched}");
        }
    }

    #[test]
    fn dot_q5_0_q8_matches_dequant_then_f32_dot_within_quantization_tolerance() {
        // The native int-domain kernel must agree with the existing
        // dequant-then-f32-dot path (dequant_q5_0_block_scalar +
        // dot_product_scalar) up to the error quantize_row_q8 itself
        // introduces on the activation side -- NOT bit-exact (this is a
        // genuinely lossy second quantization), but bounded and derived
        // empirically here, not guessed.
        for seed in [0.0, 1.0, 7.5, 13.25, 31.75, 63.0] {
            let w_block = make_q5_0_block(seed);
            let mut w_f32 = [0f32; 32];
            dequant_q5_0_block_scalar(&w_block, &mut w_f32);

            let x: Vec<f32> = (0..32).map(|i| ((i as f32 + seed) * 0.29).cos() * 3.0).collect();
            let reference = dot_product_scalar(&w_f32, &x);

            let q = quantize_row_q8(&x);
            let native = dot_q5_0_q8_block_scalar(&w_block, &q.values, q.scales[0]);

            // Empirical tolerance: bounded by the activation quantization
            // step (x_scale) times the L1 norm of the weight block (worst
            // case every element's rounding error aligns) -- computed
            // per-case, not a single fixed constant, since it scales with
            // the actual data.
            let l1_w: f32 = w_f32.iter().map(|v| v.abs()).sum();
            let tol = q.scales[0] * l1_w + 1e-3;
            assert!((native - reference).abs() <= tol, "seed={seed}: native={native} reference={reference} tol={tol}");
        }
    }

    fn make_q8_0_block(seed: f32) -> Vec<u8> {
        // 34 bytes: fp16 d, i8 qs[32]. Same synthetic-but-varied hashing
        // style as make_q5_0_block/make_q4_k_block/make_q6_k_block.
        let d: u16 = 0x3400 + (seed as u16 % 64);
        let mut raw = vec![0u8; 34];
        raw[0..2].copy_from_slice(&d.to_le_bytes());
        for i in 0..32 {
            raw[2 + i] = ((seed as u32).wrapping_mul(2654435761).wrapping_add(i as u32 * 7) % 256) as u8;
        }
        raw
    }

    #[test]
    fn dot_q8_0_q8_avx2_matches_scalar_bit_exactly() {
        // Same reasoning as Q5_0's equivalent test: same integer sum on
        // both paths, only the single final scale multiply is float, and
        // it starts from the same integer total either way.
        for seed in [0.0, 1.0, 7.5, 13.25, 31.75, 63.0, 100.0] {
            let w_block = make_q8_0_block(seed);
            let x_vals: Vec<i8> = (0..32).map(|i| (((i as i32 + seed as i32) * 37) % 255 - 127) as i8).collect();
            let x_scale = 0.013 + seed * 0.001;

            let scalar = dot_q8_0_q8_block_scalar(&w_block, &x_vals, x_scale);
            let dispatched = dot_q8_0_q8_block(&w_block, &x_vals, x_scale);
            assert_eq!(scalar, dispatched, "seed={seed}: scalar={scalar} dispatched={dispatched}");
        }
    }

    #[test]
    fn dot_q8_0_q8_matches_dequant_then_f32_dot_within_quantization_tolerance() {
        // Same structure as Q5_0's equivalent test. Q8_0 is a plain
        // symmetric format (w = d*q, no affine bias) so this tolerance
        // should be the tightest of any native kernel in this file --
        // Q8_0 has no k-quant sub-block structure to add extra rounding.
        for seed in [0.0, 1.0, 7.5, 13.25, 31.75, 63.0] {
            let w_block = make_q8_0_block(seed);
            let mut w_f32 = [0f32; 32];
            crate::simd::dequant_q8_0_block_scalar(&w_block, &mut w_f32);

            let x: Vec<f32> = (0..32).map(|i| ((i as f32 + seed) * 0.29).cos() * 3.0).collect();
            let reference = dot_product_scalar(&w_f32, &x);

            let q = quantize_row_q8(&x);
            let native = dot_q8_0_q8_block_scalar(&w_block, &q.values, q.scales[0]);

            let l1_w: f32 = w_f32.iter().map(|v| v.abs()).sum();
            let tol = q.scales[0] * l1_w + 1e-3;
            assert!((native - reference).abs() <= tol, "seed={seed}: native={native} reference={reference} tol={tol}");
        }
    }

    #[test]
    fn dot_q4_k_avx2_matches_scalar_bit_exactly() {
        // Same reasoning as Q5_0's equivalent test: AVX2 vs scalar must
        // be bit-exact here (same integer sums, only the final combine
        // -- d*dot_int - m*bsum -- involves floats, identically on both
        // paths).
        for seed in [0.0, 1.0, 7.5, 13.25, 31.75, 63.0, 100.0] {
            let w_block = make_q4_k_block(seed);
            let x: Vec<f32> = (0..256).map(|i| (((i as f32) + seed) * 0.083).sin() * 4.0).collect();
            let q = quantize_row_q8(&x);

            let scalar = dot_q4_k_super_block_scalar(&w_block, &q, 0);
            let dispatched = dot_q4_k_super_block(&w_block, &q, 0);
            assert_eq!(scalar, dispatched, "seed={seed}: scalar={scalar} dispatched={dispatched}");
        }
    }

    #[test]
    fn dot_q4_k_matches_dequant_then_f32_dot_within_quantization_tolerance() {
        // Same structure as Q5_0's equivalent test, but for the affine
        // (d*q - m) format: native int-domain result vs
        // dequant_q4_k_block_scalar + dot_product_scalar on the same
        // synthetic-but-varied weight bytes, tolerance derived from the
        // actual data (activation quantization step x weight L1 norm),
        // not a guessed constant.
        for seed in [0.0, 1.0, 7.5, 13.25, 31.75, 63.0] {
            let w_block = make_q4_k_block(seed);
            let mut w_f32 = [0f32; 256];
            dequant_q4_k_block_scalar(&w_block, &mut w_f32);

            let x: Vec<f32> = (0..256).map(|i| (((i as f32) + seed) * 0.041).cos() * 2.5).collect();
            let reference = dot_product_scalar(&w_f32, &x);

            let q = quantize_row_q8(&x);
            let native = dot_q4_k_super_block_scalar(&w_block, &q, 0);

            let l1_w: f32 = w_f32.iter().map(|v| v.abs()).sum();
            let max_x_scale = q.scales.iter().cloned().fold(0f32, f32::max);
            let tol = max_x_scale * l1_w + 1e-2;
            assert!((native - reference).abs() <= tol, "seed={seed}: native={native} reference={reference} tol={tol}");
        }
    }

    #[test]
    fn dot_q6_k_avx2_matches_scalar_bit_exactly() {
        // Same reasoning as Q4_K/Q5_0's equivalent tests: both paths
        // compute the same integer sums, only the final combine (d * sc *
        // dot_int) involves floats, identically on both paths -- so this
        // must be bit-exact, not just within a tolerance.
        for seed in [0.0, 1.0, 7.5, 13.25, 31.75, 63.0, 100.0] {
            let w_block = make_q6_k_block(seed);
            let x: Vec<f32> = (0..256).map(|i| (((i as f32) + seed) * 0.083).sin() * 4.0).collect();
            let q = quantize_row_q8(&x);

            let scalar = dot_q6_k_super_block_scalar(&w_block, &q, 0);
            let dispatched = dot_q6_k_super_block(&w_block, &q, 0);
            assert_eq!(scalar, dispatched, "seed={seed}: scalar={scalar} dispatched={dispatched}");
        }
    }

    #[test]
    fn dot_q6_k_matches_dequant_then_f32_dot_within_quantization_tolerance() {
        // Same structure and rationale as the Q4_K equivalent above: run
        // the scalar reference (dequant_q6_k_block_scalar, already
        // validated by its own AVX2-vs-scalar test and the
        // q6_k_scales_are_signed regression test) against the native
        // int-domain path on the same synthetic-but-varied weight bytes.
        // This must catch a wrong-scale-domain bug the way Fase 10's own
        // equivalent test did (a ~68x discrepancy, not a small overshoot)
        // -- an AVX2-vs-scalar test alone wouldn't, since both share
        // whatever bug is in the shared math.
        for seed in [0.0, 1.0, 7.5, 13.25, 31.75, 63.0] {
            let w_block = make_q6_k_block(seed);
            let mut w_f32 = [0f32; 256];
            crate::simd::dequant_q6_k_block_scalar(&w_block, &mut w_f32);

            let x: Vec<f32> = (0..256).map(|i| (((i as f32) + seed) * 0.037).sin() * 2.5).collect();
            let reference = dot_product_scalar(&w_f32, &x);

            let q = quantize_row_q8(&x);
            let native = dot_q6_k_super_block_scalar(&w_block, &q, 0);

            let l1_w: f32 = w_f32.iter().map(|v| v.abs()).sum();
            let max_x_scale = q.scales.iter().cloned().fold(0f32, f32::max);
            let tol = max_x_scale * l1_w + 1e-2;
            assert!((native - reference).abs() <= tol, "seed={seed}: native={native} reference={reference} tol={tol}");
        }
    }

    #[test]
    fn test_dot_q8_row_matches_f32_dot() {
        let q_f32: Vec<f32> = (0..64).map(|i| (i as f32 * 0.1).sin()).collect();
        let k_f32: Vec<f32> = (0..64).map(|i| (i as f32 * 0.2).cos()).collect();

        let ref_dot: f32 = q_f32.iter().zip(&k_f32).map(|(a, b)| a * b).sum();

        let k_q8 = quantize_row_q8(&k_f32);
        let q_q8 = quantize_row_q8(&q_f32);

        let dot1 = dot_q8_row(&q_f32, &k_q8);
        let dot2 = dot_q8_q8_row(&q_q8, &k_q8);

        assert!((dot1 - ref_dot).abs() <= 0.1, "dot1={dot1} ref={ref_dot}");
        assert!((dot2 - ref_dot).abs() <= 0.15, "dot2={dot2} ref={ref_dot}");
    }
}
