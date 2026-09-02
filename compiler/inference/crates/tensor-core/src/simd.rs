//! AVX2 acceleration for the two hottest scalar loops in `ops.rs`: the f32
//! dot product (used by `linear_quantized`'s per-block accumulate step and
//! by `causal_gqa_attention`'s Q·K scores) and Q8_0 block dequantization.
//!
//! Every public function here is safe to call on any CPU/target — runtime
//! feature detection picks AVX2+FMA when available and falls back to the
//! plain scalar loop otherwise (older CPUs, non-x86_64 targets, or a build
//! without `unsafe` code enabled for this crate specifically). The `unsafe`
//! AVX2 bodies are gated behind `#[target_feature(enable = "avx2,fma")]` so
//! the compiler enforces they're only ever reached after the runtime check.
//!
//! Scope: Q4_K/Q6_K block *unpacking* stays scalar for now — profiling (see
//! the Fase 2.5 writeup: an A/B test showing threading alone only partially
//! closed the gap to Ollama) pointed at Q5_0 first, since it's the format
//! `ffn_gate`/`ffn_up` use — the two largest matmuls in every layer. Q4_K
//! and Q6_K remain scalar; hand-vectorizing each is its own separable chunk
//! of work, not done here.

/// `sum(a[i] * b[i])`. `a` and `b` must be the same length.
pub fn dot_product(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma") {
            return unsafe { x86::dot_avx2(a, b) };
        }
    }
    dot_product_scalar(a, b)
}

pub fn dot_product_scalar(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// `out[i] += w * v[i]` for all `i`. `out` and `v` must be the same length.
/// The SAXPY-shaped counterpart to `dot_product` — Fase 4 of the
/// optimization roadmap: `causal_gqa_attention_scaled`'s P·V accumulation
/// was the one remaining unvectorized step in the attention hot path (Q·K
/// already went through `dot_product` above), and it specifically worsens
/// with context length since it runs once per (query position, cached
/// position) pair.
pub fn axpy_inplace(out: &mut [f32], w: f32, v: &[f32]) {
    debug_assert_eq!(out.len(), v.len());
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma") {
            unsafe { x86::axpy_avx2(out, w, v) };
            return;
        }
    }
    axpy_inplace_scalar(out, w, v);
}

pub fn axpy_inplace_scalar(out: &mut [f32], w: f32, v: &[f32]) {
    for (o, &x) in out.iter_mut().zip(v) {
        *o += w * x;
    }
}

/// `sum(v[i]^2)`. Calculates the sum of squares of slice `v`.
pub fn sum_squared(v: &[f32]) -> f32 {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma") {
            return unsafe { x86::sum_squares_avx2(v) };
        }
    }
    sum_squared_scalar(v)
}

/// Scalar reference for `sum_squared`, exposed for testing (mirrors
/// `dot_product`/`dot_product_scalar`).
pub fn sum_squared_scalar(v: &[f32]) -> f32 {
    v.iter().map(|&x| x * x).sum()
}

/// `a[i] += b[i]` for all `i`.
pub fn vec_add_inplace(a: &mut [f32], b: &[f32]) {
    debug_assert_eq!(a.len(), b.len());
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            unsafe { x86::vec_add_avx2(a, b) };
            return;
        }
    }
    for (x, &y) in a.iter_mut().zip(b) {
        *x += y;
    }
}

/// `a[i] *= b[i]` for all `i`.
pub fn vec_mul_inplace(a: &mut [f32], b: &[f32]) {
    debug_assert_eq!(a.len(), b.len());
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            unsafe { x86::vec_mul_avx2(a, b) };
            return;
        }
    }
    for (x, &y) in a.iter_mut().zip(b) {
        *x *= y;
    }
}

/// `a[i] *= scale` for all `i`.
pub fn vec_scale_inplace(a: &mut [f32], scale: f32) {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            unsafe { x86::vec_scale_avx2(a, scale) };
            return;
        }
    }
    for x in a.iter_mut() {
        *x *= scale;
    }
}

/// Dequantize one Q8_0 block (`fp16 d; i8 qs[32]`, see `dequant.rs`) into
/// `out[0..32]`. Falls back to the scalar per-block formula when AVX2
/// isn't available.
pub fn dequant_q8_0_block(raw: &[u8], out: &mut [f32]) {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            unsafe { x86::dequant_q8_0_block_avx2(raw, out) };
            return;
        }
    }
    dequant_q8_0_block_scalar(raw, out);
}

pub fn dequant_q8_0_block_scalar(raw: &[u8], out: &mut [f32]) {
    let d = crate::f16::f16_to_f32(u16::from_le_bytes([raw[0], raw[1]]));
    for (i, &q) in raw[2..34].iter().enumerate() {
        out[i] = (q as i8) as f32 * d;
    }
}

/// Dequantize one Q5_0 block (`fp16 d; u8 qh[4]; u8 qs[16]`, see
/// `dequant.rs`) into `out[0..32]`. Falls back to the scalar per-block
/// formula when AVX2 isn't available.
pub fn dequant_q5_0_block(raw: &[u8], out: &mut [f32]) {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            unsafe { x86::dequant_q5_0_block_avx2(raw, out) };
            return;
        }
    }
    dequant_q5_0_block_scalar(raw, out);
}

pub fn dequant_q5_0_block_scalar(raw: &[u8], out: &mut [f32]) {
    let d = crate::f16::f16_to_f32(u16::from_le_bytes([raw[0], raw[1]]));
    let qh = u32::from_le_bytes([raw[2], raw[3], raw[4], raw[5]]);
    let qs = &raw[6..22];
    for j in 0..16 {
        let xh_0 = ((qh >> j) << 4) & 0x10;
        let xh_1 = (qh >> (j + 12)) & 0x10;
        let x0 = ((qs[j] & 0x0F) as u32 | xh_0) as i32 - 16;
        let x1 = ((qs[j] >> 4) as u32 | xh_1) as i32 - 16;
        out[j] = x0 as f32 * d;
        out[j + 16] = x1 as f32 * d;
    }
}

/// Dequantize one Q4_K super-block (`fp16 d; fp16 dmin; u8 scales[12]; u8
/// qs[128]`, see `dequant.rs`) into `out[0..256]`. Falls back to the scalar
/// per-block formula when AVX2 isn't available.
pub fn dequant_q4_k_block(raw: &[u8], out: &mut [f32]) {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            unsafe { x86::dequant_q4_k_block_avx2(raw, out) };
            return;
        }
    }
    dequant_q4_k_block_scalar(raw, out);
}

pub fn dequant_q4_k_block_scalar(raw: &[u8], out: &mut [f32]) {
    let d = crate::f16::f16_to_f32(u16::from_le_bytes([raw[0], raw[1]]));
    let dmin = crate::f16::f16_to_f32(u16::from_le_bytes([raw[2], raw[3]]));
    let scales = &raw[4..16];
    let qs = &raw[16..144];

    let mut q_off = 0usize;
    let mut is = 0usize;
    let mut y_off = 0usize;
    for _ in 0..(256 / 64) {
        let (sc1, m1) = crate::dequant::get_scale_min_k4(is, scales);
        let (d1, mm1) = (d * sc1 as f32, dmin * m1 as f32);
        let (sc2, m2) = crate::dequant::get_scale_min_k4(is + 1, scales);
        let (d2, mm2) = (d * sc2 as f32, dmin * m2 as f32);

        for l in 0..32 {
            out[y_off + l] = d1 * (qs[q_off + l] & 0x0F) as f32 - mm1;
        }
        for l in 0..32 {
            out[y_off + 32 + l] = d2 * (qs[q_off + l] >> 4) as f32 - mm2;
        }

        q_off += 32;
        is += 2;
        y_off += 64;
    }
}

/// Dequantize one Q6_K super-block (`u8 ql[128]; u8 qh[64]; i8 scales[16];
/// fp16 d`, see `dequant.rs`) into `out[0..256]`. Falls back to the scalar
/// per-block formula when AVX2 isn't available.
pub fn dequant_q6_k_block(raw: &[u8], out: &mut [f32]) {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            unsafe { x86::dequant_q6_k_block_avx2(raw, out) };
            return;
        }
    }
    dequant_q6_k_block_scalar(raw, out);
}

pub fn dequant_q6_k_block_scalar(raw: &[u8], out: &mut [f32]) {
    let ql_all = &raw[0..128];
    let qh_all = &raw[128..192];
    let sc_all = &raw[192..208];
    let d = crate::f16::f16_to_f32(u16::from_le_bytes([raw[208], raw[209]]));

    for outer in 0..(256 / 128) {
        let ql = &ql_all[outer * 64..outer * 64 + 64];
        let qh = &qh_all[outer * 32..outer * 32 + 32];
        let sc = &sc_all[outer * 8..outer * 8 + 8];
        let y = &mut out[outer * 128..outer * 128 + 128];

        #[allow(clippy::identity_op)] // kept for visual symmetry with the >>2/>>4/>>6 lines below (mirrors ggml's own dequantize_row_q6_K)
        for l in 0..32 {
            let is = l / 16;
            let q1 = ((ql[l] & 0x0F) | (((qh[l] >> 0) & 3) << 4)) as i32 - 32;
            let q2 = ((ql[l + 32] & 0x0F) | (((qh[l] >> 2) & 3) << 4)) as i32 - 32;
            let q3 = ((ql[l] >> 4) | (((qh[l] >> 4) & 3) << 4)) as i32 - 32;
            let q4 = ((ql[l + 32] >> 4) | (((qh[l] >> 6) & 3) << 4)) as i32 - 32;
            // `scales` is ggml's `int8_t scales[16]` — signed. Casting
            // straight from the raw `u8` to `f32` reads e.g. byte 0xB7 as
            // +183 instead of -73: correct for small values, silently wrong
            // for any scale with the high bit set. `as i8` first
            // reinterprets the bit pattern (see `q6_k_scales_are_signed` in
            // `dequant.rs`'s test module for the regression case that caught
            // this).
            y[l] = d * (sc[is] as i8) as f32 * q1 as f32;
            y[l + 32] = d * (sc[is + 2] as i8) as f32 * q2 as f32;
            y[l + 64] = d * (sc[is + 4] as i8) as f32 * q3 as f32;
            y[l + 96] = d * (sc[is + 6] as i8) as f32 * q4 as f32;
        }
    }
}

#[cfg(target_arch = "x86_64")]
mod x86 {
    use std::arch::x86_64::*;

    /// # Safety
    /// Caller must have verified `is_x86_feature_detected!("avx2")` and
    /// `("fma")` — enforced structurally by `target_feature`, which makes
    /// calling this without that check undefined behavior the compiler
    /// won't catch, only the runtime dispatch in `dot_product` does.
    #[target_feature(enable = "avx2,fma")]
    pub unsafe fn dot_avx2(a: &[f32], b: &[f32]) -> f32 {
        let n = a.len();
        let lanes = n - (n % 8);

        let mut acc = _mm256_setzero_ps();
        let mut i = 0;
        while i < lanes {
            let va = _mm256_loadu_ps(a.as_ptr().add(i));
            let vb = _mm256_loadu_ps(b.as_ptr().add(i));
            acc = _mm256_fmadd_ps(va, vb, acc);
            i += 8;
        }

        // Horizontal sum of the 8 lanes in `acc`.
        let hi = _mm256_extractf128_ps(acc, 1);
        let lo = _mm256_castps256_ps128(acc);
        let sum128 = _mm_add_ps(hi, lo); // 4 lanes
        let shuf = _mm_movehdup_ps(sum128);
        let sums = _mm_add_ps(sum128, shuf);
        let shuf2 = _mm_movehl_ps(shuf, sums);
        let final_sum = _mm_add_ss(sums, shuf2);
        let mut total = _mm_cvtss_f32(final_sum);

        // Scalar tail for n not a multiple of 8.
        while i < n {
            total += a[i] * b[i];
            i += 1;
        }
        total
    }

    /// # Safety
    /// Caller must have verified `is_x86_feature_detected!("avx2")` and
    /// `("fma")` — same contract as `dot_avx2` above.
    #[target_feature(enable = "avx2,fma")]
    pub unsafe fn axpy_avx2(out: &mut [f32], w: f32, v: &[f32]) {
        let n = out.len();
        let lanes = n - (n % 8);
        let w_vec = _mm256_set1_ps(w);

        let mut i = 0;
        while i < lanes {
            let vv = _mm256_loadu_ps(v.as_ptr().add(i));
            let vo = _mm256_loadu_ps(out.as_ptr().add(i));
            let result = _mm256_fmadd_ps(w_vec, vv, vo);
            _mm256_storeu_ps(out.as_mut_ptr().add(i), result);
            i += 8;
        }

        // Scalar tail for n not a multiple of 8 (needed for e.g. Phi-3's
        // partial-rotary head_dim, which need not be a multiple of 8).
        while i < n {
            out[i] += w * v[i];
            i += 1;
        }
    }

    #[target_feature(enable = "avx2,fma")]
    pub unsafe fn sum_squares_avx2(v: &[f32]) -> f32 {
        let n = v.len();
        let lanes = n - (n % 8);
        let mut acc = _mm256_setzero_ps();
        let mut i = 0;
        while i < lanes {
            let vv = _mm256_loadu_ps(v.as_ptr().add(i));
            acc = _mm256_fmadd_ps(vv, vv, acc);
            i += 8;
        }
        let hi = _mm256_extractf128_ps(acc, 1);
        let lo = _mm256_castps256_ps128(acc);
        let sum128 = _mm_add_ps(hi, lo);
        let shuf = _mm_movehdup_ps(sum128);
        let sums = _mm_add_ps(sum128, shuf);
        let shuf2 = _mm_movehl_ps(shuf, sums);
        let final_sum = _mm_add_ss(sums, shuf2);
        let mut total = _mm_cvtss_f32(final_sum);
        while i < n {
            total += v[i] * v[i];
            i += 1;
        }
        total
    }

    #[target_feature(enable = "avx2")]
    pub unsafe fn vec_add_avx2(a: &mut [f32], b: &[f32]) {
        let n = a.len();
        let lanes = n - (n % 8);
        let mut i = 0;
        while i < lanes {
            let va = _mm256_loadu_ps(a.as_ptr().add(i));
            let vb = _mm256_loadu_ps(b.as_ptr().add(i));
            let res = _mm256_add_ps(va, vb);
            _mm256_storeu_ps(a.as_mut_ptr().add(i), res);
            i += 8;
        }
        while i < n {
            a[i] += b[i];
            i += 1;
        }
    }

    #[target_feature(enable = "avx2")]
    pub unsafe fn vec_mul_avx2(a: &mut [f32], b: &[f32]) {
        let n = a.len();
        let lanes = n - (n % 8);
        let mut i = 0;
        while i < lanes {
            let va = _mm256_loadu_ps(a.as_ptr().add(i));
            let vb = _mm256_loadu_ps(b.as_ptr().add(i));
            let res = _mm256_mul_ps(va, vb);
            _mm256_storeu_ps(a.as_mut_ptr().add(i), res);
            i += 8;
        }
        while i < n {
            a[i] *= b[i];
            i += 1;
        }
    }

    #[target_feature(enable = "avx2")]
    pub unsafe fn vec_scale_avx2(a: &mut [f32], scale: f32) {
        let n = a.len();
        let lanes = n - (n % 8);
        let s_vec = _mm256_set1_ps(scale);
        let mut i = 0;
        while i < lanes {
            let va = _mm256_loadu_ps(a.as_ptr().add(i));
            let res = _mm256_mul_ps(va, s_vec);
            _mm256_storeu_ps(a.as_mut_ptr().add(i), res);
            i += 8;
        }
        while i < n {
            a[i] *= scale;
            i += 1;
        }
    }

    /// # Safety
    /// Caller must have verified `is_x86_feature_detected!("avx2")`.
    /// `raw` must be exactly 34 bytes (Q8_0's block size) and `out` at
    /// least 32 elements — both guaranteed by this crate's only call site
    /// (`dequant.rs`'s block dispatcher), not re-checked here.
    #[target_feature(enable = "avx2")]
    pub unsafe fn dequant_q8_0_block_avx2(raw: &[u8], out: &mut [f32]) {
        let d = crate::f16::f16_to_f32(u16::from_le_bytes([raw[0], raw[1]]));
        let d_vec = _mm256_set1_ps(d);

        // 32 signed bytes -> 4 groups of 8 x i32 (sign-extended) -> f32.
        let bytes = _mm256_loadu_si256(raw[2..34].as_ptr() as *const __m256i);
        let lo128 = _mm256_castsi256_si128(bytes);
        let hi128 = _mm256_extracti128_si256(bytes, 1);

        // _mm256_cvtepi8_epi32 sign-extends the low 8 bytes of a 128-bit
        // input; shifting right by 8 bytes exposes the next 8 for the
        // second call within each 16-byte half.
        let g0 = _mm256_cvtepi8_epi32(lo128);
        let g1 = _mm256_cvtepi8_epi32(_mm_srli_si128(lo128, 8));
        let g2 = _mm256_cvtepi8_epi32(hi128);
        let g3 = _mm256_cvtepi8_epi32(_mm_srli_si128(hi128, 8));

        let f0 = _mm256_mul_ps(_mm256_cvtepi32_ps(g0), d_vec);
        let f1 = _mm256_mul_ps(_mm256_cvtepi32_ps(g1), d_vec);
        let f2 = _mm256_mul_ps(_mm256_cvtepi32_ps(g2), d_vec);
        let f3 = _mm256_mul_ps(_mm256_cvtepi32_ps(g3), d_vec);

        _mm256_storeu_ps(out.as_mut_ptr(), f0);
        _mm256_storeu_ps(out.as_mut_ptr().add(8), f1);
        _mm256_storeu_ps(out.as_mut_ptr().add(16), f2);
        _mm256_storeu_ps(out.as_mut_ptr().add(24), f3);
    }

    /// # Safety
    /// Caller must have verified `is_x86_feature_detected!("avx2")`.
    /// `raw` must be exactly 22 bytes (Q5_0's block size) and `out` at
    /// least 32 elements.
    ///
    /// The scalar formula (see `dequant_q5_0_block_scalar`) reduces to: for
    /// element `i` in `0..32`, its 5th (high) bit is bit `i` of the 32-bit
    /// `qh` field. AVX2 has no per-lane-variable-shift-by-8-bit-amounts
    /// primitive to pull "bit `i`" out of a single broadcast `qh` register
    /// directly, so this uses a different, safer trick: AND a broadcast
    /// `qh` against a *constant* per-lane bitmask (`1 << i`, one distinct
    /// power of two per lane — an ordinary immediate load, not a runtime
    /// variable shift), then test "not equal to zero" rather than "greater
    /// than zero". That distinction matters: bit 31 of `qh`, ANDed into a
    /// lane by itself, is `i32::MIN` — a *negative* pattern — so a
    /// greater-than-zero test would silently misclassify exactly that one
    /// bit as unset. Equality-with-zero has no such sign pitfall.
    #[target_feature(enable = "avx2")]
    pub unsafe fn dequant_q5_0_block_avx2(raw: &[u8], out: &mut [f32]) {
        let d = crate::f16::f16_to_f32(u16::from_le_bytes([raw[0], raw[1]]));
        let qh = u32::from_le_bytes([raw[2], raw[3], raw[4], raw[5]]);
        let qs = &raw[6..22];

        let d_vec = _mm256_set1_ps(d);
        let sixteen = _mm256_set1_epi32(0x10);
        let bias16 = _mm256_set1_epi32(16);
        let low_nibble_mask = _mm256_set1_epi32(0x0F);
        let qh_vec = _mm256_set1_epi32(qh as i32);
        let zero = _mm256_setzero_si256();
        let all_ones = _mm256_set1_epi32(-1);

        // (byte slice to pull 8 lanes from, nibble shift, output offset, qh bit offset)
        // — the four 8-wide groups covering the 32 output elements. See the
        // scalar version: elements 0..16 come from qs's low nibbles paired
        // with qh bits 0..16; elements 16..32 come from qs's high nibbles
        // paired with qh bits 16..32 (`(qh >> (j+12)) & 0x10` is exactly
        // "bit `j+16` of qh", algebraically — see module docs above).
        let groups: [(&[u8], u32, usize, u32); 4] =
            [(&qs[0..8], 0, 0, 0), (&qs[8..16], 0, 8, 8), (&qs[0..8], 4, 16, 16), (&qs[8..16], 4, 24, 24)];

        for (qs_slice, nibble_shift, elem_offset, bit_offset) in groups {
            let bytes128 = _mm_loadl_epi64(qs_slice.as_ptr() as *const __m128i);
            let bytes_i32 = _mm256_cvtepu8_epi32(bytes128);
            let shifted = if nibble_shift == 0 { bytes_i32 } else { _mm256_srli_epi32(bytes_i32, 4) };
            let nibble = _mm256_and_si256(shifted, low_nibble_mask);

            // lane k (k=0..7) must test bit (bit_offset+k) of qh — a
            // distinct compile-time-computable constant per lane, loaded
            // directly rather than derived from a runtime shift.
            let bitmask = _mm256_set_epi32(
                (1u32 << (bit_offset + 7)) as i32,
                (1u32 << (bit_offset + 6)) as i32,
                (1u32 << (bit_offset + 5)) as i32,
                (1u32 << (bit_offset + 4)) as i32,
                (1u32 << (bit_offset + 3)) as i32,
                (1u32 << (bit_offset + 2)) as i32,
                (1u32 << (bit_offset + 1)) as i32,
                (1u32 << bit_offset) as i32,
            );
            let masked = _mm256_and_si256(qh_vec, bitmask);
            let is_zero = _mm256_cmpeq_epi32(masked, zero);
            let bit_set = _mm256_andnot_si256(is_zero, all_ones);
            let xh = _mm256_and_si256(bit_set, sixteen);

            let combined = _mm256_or_si256(nibble, xh);
            let signed = _mm256_sub_epi32(combined, bias16);
            let f = _mm256_mul_ps(_mm256_cvtepi32_ps(signed), d_vec);
            _mm256_storeu_ps(out.as_mut_ptr().add(elem_offset), f);
        }
    }

    /// # Safety
    /// Caller must have verified `is_x86_feature_detected!("avx2")`.
    /// `raw` must be exactly 144 bytes (Q4_K's block size) and `out` at
    /// least 256 elements.
    ///
    /// Simpler than Q5_0: no per-element bit testing at all. Each of the 8
    /// sub-blocks (32 elements) has one constant `(scale, min)` pair
    /// (computed scalar-side via `get_scale_min_k4` — cheap, only 8 calls
    /// per super-block) applied uniformly across all 32 of its elements, so
    /// the per-lane work is just "extract a nibble, multiply-subtract by a
    /// broadcast constant" — the same shape as the already-verified Q8_0
    /// kernel, just 4-bit source data instead of 8-bit and an extra
    /// subtract for the min term.
    #[target_feature(enable = "avx2")]
    pub unsafe fn dequant_q4_k_block_avx2(raw: &[u8], out: &mut [f32]) {
        let d = crate::f16::f16_to_f32(u16::from_le_bytes([raw[0], raw[1]]));
        let dmin = crate::f16::f16_to_f32(u16::from_le_bytes([raw[2], raw[3]]));
        let scales = &raw[4..16];
        let qs = &raw[16..144];

        let low_mask = _mm256_set1_epi32(0x0F);

        let mut q_off = 0usize;
        let mut is = 0usize;
        let mut y_off = 0usize;
        for _ in 0..(256 / 64) {
            let (sc1, m1) = crate::dequant::get_scale_min_k4(is, scales);
            let (sc2, m2) = crate::dequant::get_scale_min_k4(is + 1, scales);
            let d1_vec = _mm256_set1_ps(d * sc1 as f32);
            let mm1_vec = _mm256_set1_ps(dmin * m1 as f32);
            let d2_vec = _mm256_set1_ps(d * sc2 as f32);
            let mm2_vec = _mm256_set1_ps(dmin * m2 as f32);

            for g in 0..4 {
                let bytes128 = _mm_loadl_epi64(qs[q_off + g * 8..q_off + g * 8 + 8].as_ptr() as *const __m128i);
                let b32 = _mm256_cvtepu8_epi32(bytes128);

                let lo_nib = _mm256_and_si256(b32, low_mask);
                let lo_f = _mm256_sub_ps(_mm256_mul_ps(_mm256_cvtepi32_ps(lo_nib), d1_vec), mm1_vec);
                _mm256_storeu_ps(out.as_mut_ptr().add(y_off + g * 8), lo_f);

                let hi_nib = _mm256_and_si256(_mm256_srli_epi32(b32, 4), low_mask);
                let hi_f = _mm256_sub_ps(_mm256_mul_ps(_mm256_cvtepi32_ps(hi_nib), d2_vec), mm2_vec);
                _mm256_storeu_ps(out.as_mut_ptr().add(y_off + 32 + g * 8), hi_f);
            }

            q_off += 32;
            is += 2;
            y_off += 64;
        }
    }

    /// # Safety
    /// Caller must have verified `is_x86_feature_detected!("avx2")`.
    /// `raw` must be exactly 210 bytes (Q6_K's block size) and `out` at
    /// least 256 elements.
    ///
    /// Like Q4_K (and unlike Q5_0): no per-lane-variable bit testing needed.
    /// The scalar formula's `is = l / 16` is constant across each 16-wide
    /// half of the `l in 0..32` loop, so each of q1/q2/q3/q4's four scale
    /// lookups (`sc[is]`, `sc[is+2]`, `sc[is+4]`, `sc[is+6]`) is a single
    /// value shared by all 16 lanes in that half — exactly the "extract
    /// fixed bits, multiply by a broadcast constant" shape the Q4_K kernel
    /// already uses, just with a 4-bit/2-bit split (`ql`/`qh`) instead of
    /// Q4_K's plain nibble, and four q-slots instead of two. Each 16-lane
    /// half is done as two 8-lane AVX2 groups. Within one group (8 lanes at
    /// offset `l`), all four q-slots read the *same* 8 bytes of `qh[l..l+8]`
    /// and reuse them via fixed shift amounts (0/2/4/6) — no data-dependent
    /// shift, so this is safe the same way Q4_K's fixed nibble shift is.
    #[target_feature(enable = "avx2")]
    pub unsafe fn dequant_q6_k_block_avx2(raw: &[u8], out: &mut [f32]) {
        let ql_all = &raw[0..128];
        let qh_all = &raw[128..192];
        let sc_all = &raw[192..208];
        let d = crate::f16::f16_to_f32(u16::from_le_bytes([raw[208], raw[209]]));

        let low_mask = _mm256_set1_epi32(0x0F);
        let two_bit_mask = _mm256_set1_epi32(3);
        let bias32 = _mm256_set1_epi32(32);

        for outer in 0..(256 / 128) {
            let ql = &ql_all[outer * 64..outer * 64 + 64];
            let qh = &qh_all[outer * 32..outer * 32 + 32];
            let sc = &sc_all[outer * 8..outer * 8 + 8];
            let y_base = outer * 128;

            for is in 0..2 {
                // Signed scale broadcasts for this 16-lane half — one
                // constant f32 per q-slot, shared by every lane below.
                let d1_vec = _mm256_set1_ps(d * (sc[is] as i8) as f32);
                let d2_vec = _mm256_set1_ps(d * (sc[is + 2] as i8) as f32);
                let d3_vec = _mm256_set1_ps(d * (sc[is + 4] as i8) as f32);
                let d4_vec = _mm256_set1_ps(d * (sc[is + 6] as i8) as f32);

                for g in 0..2 {
                    let l = is * 16 + g * 8;

                    let ql_lo128 = _mm_loadl_epi64(ql[l..l + 8].as_ptr() as *const __m128i);
                    let ql_lo = _mm256_cvtepu8_epi32(ql_lo128);
                    let ql_hi128 = _mm_loadl_epi64(ql[l + 32..l + 40].as_ptr() as *const __m128i);
                    let ql_hi = _mm256_cvtepu8_epi32(ql_hi128);
                    let qh128 = _mm_loadl_epi64(qh[l..l + 8].as_ptr() as *const __m128i);
                    let qh_v = _mm256_cvtepu8_epi32(qh128);

                    // q1 = (ql_lo & 0x0F) | ((qh >> 0 & 3) << 4) - 32
                    let qh0 = _mm256_slli_epi32(_mm256_and_si256(qh_v, two_bit_mask), 4);
                    let q1 = _mm256_sub_epi32(_mm256_or_si256(_mm256_and_si256(ql_lo, low_mask), qh0), bias32);
                    let f1 = _mm256_mul_ps(_mm256_cvtepi32_ps(q1), d1_vec);
                    _mm256_storeu_ps(out.as_mut_ptr().add(y_base + l), f1);

                    // q2 = (ql_hi & 0x0F) | ((qh >> 2 & 3) << 4) - 32
                    let qh2 = _mm256_slli_epi32(_mm256_and_si256(_mm256_srli_epi32(qh_v, 2), two_bit_mask), 4);
                    let q2 = _mm256_sub_epi32(_mm256_or_si256(_mm256_and_si256(ql_hi, low_mask), qh2), bias32);
                    let f2 = _mm256_mul_ps(_mm256_cvtepi32_ps(q2), d2_vec);
                    _mm256_storeu_ps(out.as_mut_ptr().add(y_base + 32 + l), f2);

                    // q3 = (ql_lo >> 4) | ((qh >> 4 & 3) << 4) - 32
                    let qh4 = _mm256_slli_epi32(_mm256_and_si256(_mm256_srli_epi32(qh_v, 4), two_bit_mask), 4);
                    let q3 = _mm256_sub_epi32(_mm256_or_si256(_mm256_srli_epi32(ql_lo, 4), qh4), bias32);
                    let f3 = _mm256_mul_ps(_mm256_cvtepi32_ps(q3), d3_vec);
                    _mm256_storeu_ps(out.as_mut_ptr().add(y_base + 64 + l), f3);

                    // q4 = (ql_hi >> 4) | ((qh >> 6 & 3) << 4) - 32
                    let qh6 = _mm256_slli_epi32(_mm256_and_si256(_mm256_srli_epi32(qh_v, 6), two_bit_mask), 4);
                    let q4 = _mm256_sub_epi32(_mm256_or_si256(_mm256_srli_epi32(ql_hi, 4), qh6), bias32);
                    let f4 = _mm256_mul_ps(_mm256_cvtepi32_ps(q4), d4_vec);
                    _mm256_storeu_ps(out.as_mut_ptr().add(y_base + 96 + l), f4);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dot_product_matches_scalar_on_various_lengths() {
        for n in [0, 1, 3, 7, 8, 9, 15, 16, 17, 32, 100, 257] {
            let a: Vec<f32> = (0..n).map(|i| (i as f32) * 0.37 - 5.0).collect();
            let b: Vec<f32> = (0..n).map(|i| ((i * 3 + 1) as f32) * -0.11 + 2.0).collect();
            let simd = dot_product(&a, &b);
            let scalar = dot_product_scalar(&a, &b);
            // Relative, not absolute, tolerance: AVX2 sums 8 lanes in
            // parallel then combines them, so it accumulates rounding
            // error in a different order than the sequential scalar sum —
            // expected floating-point non-associativity (same phenomenon
            // as Fase 1's engine-vs-Ollama comparison), not a bug. An
            // absolute epsilon breaks down once the sum itself is large
            // (n=257 sums to ~6e5, where 1e-2 is tighter than f32 can even
            // represent at that magnitude).
            let scale = scalar.abs().max(1.0);
            assert!((simd - scalar).abs() / scale < 1e-4, "n={n}: simd={simd} scalar={scalar}");
        }
    }

    #[test]
    fn axpy_matches_scalar_on_various_lengths() {
        // Covers every remainder mod 8 (the AVX2 tail-loop boundary) plus
        // real-world head_dim values across the 6 architectures (64/80/96
        // for dense models, 128/256 for larger ones, plus Phi-3-style
        // partial-rotary dims that need not be a multiple of 8) — this
        // property (SIMD == scalar) doesn't depend on which specific
        // head_dim a model happens to use, only on exercising every
        // possible tail remainder, so a broad sweep covers all of them
        // without needing to special-case each architecture.
        for n in [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 15, 16, 17, 32, 64, 80, 96, 100, 128, 256, 257] {
            let w = 1.375_f32;
            let v: Vec<f32> = (0..n).map(|i| (i as f32) * 0.29 - 3.0).collect();
            let mut simd_out: Vec<f32> = (0..n).map(|i| (i as f32) * -0.13 + 1.0).collect();
            let mut scalar_out = simd_out.clone();

            axpy_inplace(&mut simd_out, w, &v);
            axpy_inplace_scalar(&mut scalar_out, w, &v);

            for i in 0..n {
                let scale = scalar_out[i].abs().max(1.0);
                assert!(
                    (simd_out[i] - scalar_out[i]).abs() / scale < 1e-5,
                    "n={n} i={i}: simd={} scalar={}",
                    simd_out[i],
                    scalar_out[i]
                );
            }
        }
    }

    #[test]
    fn sum_squared_matches_scalar_on_various_lengths() {
        // Same rationale as dot_product_matches_scalar_on_various_lengths:
        // this is a horizontal reduction (8-wide FMA accumulator, then
        // tree-combined), so AVX2 sums in a different order than the
        // sequential scalar sum -- relative tolerance, not bit-exact.
        for n in [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 15, 16, 17, 32, 64, 100, 128, 257] {
            let v: Vec<f32> = (0..n).map(|i| (i as f32) * 0.29 - 3.0).collect();
            let simd = sum_squared(&v);
            let scalar = sum_squared_scalar(&v);
            let scale = scalar.abs().max(1.0);
            assert!((simd - scalar).abs() / scale < 1e-4, "n={n}: simd={simd} scalar={scalar}");
        }
    }

    #[test]
    fn vec_add_inplace_matches_scalar_on_various_lengths() {
        // Element-wise, not a reduction: each output only depends on the
        // same-index inputs, so AVX2 and scalar should be bit-exact, not
        // just within a tolerance -- unlike sum_squared/dot_product above.
        for n in [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 15, 16, 17, 32, 100, 257] {
            let a: Vec<f32> = (0..n).map(|i| (i as f32) * 0.37 - 5.0).collect();
            let b: Vec<f32> = (0..n).map(|i| ((i * 3 + 1) as f32) * -0.11 + 2.0).collect();
            let mut simd_out = a.clone();
            vec_add_inplace(&mut simd_out, &b);
            let mut scalar_out = a.clone();
            for (x, &y) in scalar_out.iter_mut().zip(&b) {
                *x += y;
            }
            assert_eq!(simd_out, scalar_out, "n={n}");
        }
    }

    #[test]
    fn vec_mul_inplace_matches_scalar_on_various_lengths() {
        for n in [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 15, 16, 17, 32, 100, 257] {
            let a: Vec<f32> = (0..n).map(|i| (i as f32) * 0.37 - 5.0).collect();
            let b: Vec<f32> = (0..n).map(|i| ((i * 3 + 1) as f32) * -0.11 + 2.0).collect();
            let mut simd_out = a.clone();
            vec_mul_inplace(&mut simd_out, &b);
            let mut scalar_out = a.clone();
            for (x, &y) in scalar_out.iter_mut().zip(&b) {
                *x *= y;
            }
            assert_eq!(simd_out, scalar_out, "n={n}");
        }
    }

    #[test]
    fn vec_scale_inplace_matches_scalar_on_various_lengths() {
        for n in [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 15, 16, 17, 32, 100, 257] {
            let a: Vec<f32> = (0..n).map(|i| (i as f32) * 0.37 - 5.0).collect();
            let scale = 1.375_f32;
            let mut simd_out = a.clone();
            vec_scale_inplace(&mut simd_out, scale);
            let mut scalar_out = a.clone();
            for x in scalar_out.iter_mut() {
                *x *= scale;
            }
            assert_eq!(simd_out, scalar_out, "n={n}");
        }
    }

    #[test]
    fn dequant_q8_0_block_matches_scalar() {
        let mut raw = vec![0u8; 34];
        raw[0..2].copy_from_slice(&0x3C00u16.to_le_bytes()); // d = 1.0
        for j in 0..32 {
            raw[2 + j] = ((j * 37 + 5) % 256) as u8;
        }
        let mut simd_out = [0f32; 32];
        let mut scalar_out = [0f32; 32];
        dequant_q8_0_block(&raw, &mut simd_out);
        dequant_q8_0_block_scalar(&raw, &mut scalar_out);
        assert_eq!(simd_out, scalar_out);
    }

    fn q5_0_block(d_bits: u16, qh: u32, qs: [u8; 16]) -> Vec<u8> {
        let mut raw = vec![0u8; 22];
        raw[0..2].copy_from_slice(&d_bits.to_le_bytes());
        raw[2..6].copy_from_slice(&qh.to_le_bytes());
        raw[6..22].copy_from_slice(&qs);
        raw
    }

    fn assert_q5_0_matches(raw: &[u8]) {
        let mut simd_out = [0f32; 32];
        let mut scalar_out = [0f32; 32];
        dequant_q5_0_block(raw, &mut simd_out);
        dequant_q5_0_block_scalar(raw, &mut scalar_out);
        assert_eq!(simd_out, scalar_out, "raw={raw:?}");
    }

    #[test]
    fn dequant_q5_0_block_matches_scalar_various_patterns() {
        // Plain pseudo-random-looking nibbles, qh = 0 (no high bits set at all).
        let mut qs = [0u8; 16];
        for (j, b) in qs.iter_mut().enumerate() {
            *b = ((j * 41 + 13) % 256) as u8;
        }
        assert_q5_0_matches(&q5_0_block(0x3C00, 0x0000_0000, qs));

        // Every high bit set — every element's low nibble gets OR'd with 0x10.
        assert_q5_0_matches(&q5_0_block(0x4200, 0xFFFF_FFFF, qs));

        // The specific trap this kernel's design note calls out: bit 31 of
        // qh alone. A signed greater-than-zero test on that lane in
        // isolation would misread it as "not set" (the masked value is
        // i32::MIN, negative) — this pins down that the equality-based
        // test used here gets it right regardless.
        assert_q5_0_matches(&q5_0_block(0x3800, 0x8000_0000, qs));

        // A scattered bit pattern touching every group's boundary bits
        // (0, 7, 8, 15, 16, 23, 24, 31 — first/last bit of each 8-bit group).
        let boundary_bits = (1u32 << 0) | (1u32 << 7) | (1u32 << 8) | (1u32 << 15) | (1u32 << 16) | (1u32 << 23) | (1u32 << 24) | (1u32 << 31);
        assert_q5_0_matches(&q5_0_block(0x4500, boundary_bits, qs));

        // qs = all zero, all 0xFF, to exercise nibble extraction at the extremes.
        assert_q5_0_matches(&q5_0_block(0x3C00, 0x1234_5678, [0u8; 16]));
        assert_q5_0_matches(&q5_0_block(0x3C00, 0x9ABC_DEF0, [0xFFu8; 16]));
    }

    #[test]
    fn dequant_q4_k_block_matches_scalar_various_patterns() {
        let mut raw = vec![0u8; 144];
        raw[0..2].copy_from_slice(&0x3C00u16.to_le_bytes()); // d = 1.0
        raw[2..4].copy_from_slice(&0x3400u16.to_le_bytes()); // dmin = 0.25
        for (i, b) in raw[4..16].iter_mut().enumerate() {
            // Every byte's top 2 bits set too, to exercise get_scale_min_k4's
            // bit-sharing path (sub-blocks 4..7 reuse the top bits of 0..3).
            *b = ((i * 53 + 7) % 256) as u8 | 0xC0;
        }
        for (i, b) in raw[16..144].iter_mut().enumerate() {
            *b = ((i * 29 + 11) % 256) as u8;
        }
        let mut simd_out = [0f32; 256];
        let mut scalar_out = [0f32; 256];
        dequant_q4_k_block(&raw, &mut simd_out);
        dequant_q4_k_block_scalar(&raw, &mut scalar_out);
        assert_eq!(simd_out, scalar_out);

        // All-zero scales/mins/qs — degenerate but must still match (every
        // output element should come out as exactly 0.0 - 0.0 = 0.0 on
        // both paths).
        let zero_raw = vec![0u8; 144];
        let mut simd_zero = [0f32; 256];
        let mut scalar_zero = [0f32; 256];
        dequant_q4_k_block(&zero_raw, &mut simd_zero);
        dequant_q4_k_block_scalar(&zero_raw, &mut scalar_zero);
        assert_eq!(simd_zero, scalar_zero);

        // Maximum 6-bit scale/min values and maximum nibbles — but *not*
        // 0xFF for d/dmin themselves: that fp16 bit pattern is NaN (sign=1,
        // exponent=all-ones, mantissa!=0), which no real GGUF file would
        // ever contain (d/dmin are trained scale values), and would make
        // this assertion compare NaN against NaN — which is never equal to
        // itself under IEEE 754, so it'd fail even though both sides
        // produced the "same" garbage. Use the largest finite fp16 instead.
        let mut max_raw = vec![0xFFu8; 144];
        max_raw[0..2].copy_from_slice(&0x7BFFu16.to_le_bytes()); // largest finite fp16 (~65504)
        max_raw[2..4].copy_from_slice(&0x7BFFu16.to_le_bytes());
        let mut simd_max = [0f32; 256];
        let mut scalar_max = [0f32; 256];
        dequant_q4_k_block(&max_raw, &mut simd_max);
        dequant_q4_k_block_scalar(&max_raw, &mut scalar_max);
        assert_eq!(simd_max, scalar_max);
    }

    fn assert_q6_k_matches(raw: &[u8]) {
        let mut simd_out = [0f32; 256];
        let mut scalar_out = [0f32; 256];
        dequant_q6_k_block(raw, &mut simd_out);
        dequant_q6_k_block_scalar(raw, &mut scalar_out);
        assert_eq!(simd_out, scalar_out, "raw={raw:?}");
    }

    #[test]
    fn dequant_q6_k_block_matches_scalar_various_patterns() {
        // (a) Varied non-trivial ql/qh/scales bit patterns, including
        // negative (high-bit-set) scale bytes so both sub-block halves
        // exercise the signed cast.
        let mut raw = vec![0u8; 210];
        raw[208..210].copy_from_slice(&0x3C00u16.to_le_bytes()); // d = 1.0
        for (i, b) in raw[0..128].iter_mut().enumerate() {
            *b = ((i * 37 + 5) % 256) as u8; // ql
        }
        for (i, b) in raw[128..192].iter_mut().enumerate() {
            *b = ((i * 19 + 3) % 256) as u8; // qh
        }
        for (i, b) in raw[192..208].iter_mut().enumerate() {
            // Alternate sign bit across the 16 scales so every sub-block
            // exercises both positive and negative values.
            let base = ((i * 41 + 7) % 128) as u8;
            *b = if i % 2 == 0 { base | 0x80 } else { base };
        }
        assert_q6_k_matches(&raw);

        // (b) All-zero — degenerate but must still match (everything comes
        // out as -32 * 0 * d = 0.0, or rather d * 0 * (-32) = 0.0, on both
        // paths).
        let zero_raw = vec![0u8; 210];
        assert_q6_k_matches(&zero_raw);

        // (c) ql/qh/scales all 0xFF, but d is a large *finite* fp16 (0x7BFF,
        // ~65504) rather than 0xFFFF — 0xFFFF as fp16 bits is NaN (sign=1,
        // exponent=all-ones, mantissa!=0), and NaN != NaN under IEEE 754 so
        // that comparison would fail even when both sides are "equally
        // wrong". All-0xFF scales bytes are also all negative (-1 as i8),
        // which additionally covers the signed-scale path.
        let mut max_raw = vec![0xFFu8; 210];
        max_raw[208..210].copy_from_slice(&0x7BFFu16.to_le_bytes());
        assert_q6_k_matches(&max_raw);

        // (d) Specifically the regression scenario from dequant.rs's
        // `q6_k_scales_are_signed` test: scales[0] = 0xB7 (-73 signed, +183
        // unsigned) — pins down that the AVX2 path also reads the sign bit
        // correctly, not just the scalar path.
        let mut signed_raw = vec![0u8; 210];
        signed_raw[208..210].copy_from_slice(&0x3C00u16.to_le_bytes()); // d = 1.0
        signed_raw[192] = 0xB7; // scales[0] = -73
        signed_raw[0] = 0x0F; // ql[0] low nibble = 0xF, qh[0] = 0 -> q1 = 15 - 32 = -17
        assert_q6_k_matches(&signed_raw);
        // Also exercise a negative scale that lands in the *second* 16-lane
        // half (is=1) and in the q4 slot (`sc[is+6]` = `sc[7]`), not just
        // is=0/q1, so the AVX2 kernel's second `is` iteration and its last
        // q-slot both get checked against a negative value too.
        signed_raw[192 + 7] = 0x91; // scales[7] = -111
        signed_raw[32 + 16] = 0xF0; // ql[48] high nibble = 0xF (feeds q4 at l=16, is=1)
        assert_q6_k_matches(&signed_raw);
    }
}
