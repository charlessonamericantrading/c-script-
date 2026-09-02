use gguf::GgmlType;

use crate::dequant::{dequantize, DequantError};

/// A weight matrix kept in its on-disk quantized form — the Fase 2
/// alternative to fully dequantizing every weight into a `Matrix` of f32 at
/// load time (Fase 1's approach). Same `(rows=out_features,
/// cols=in_features)` convention as `Matrix`; see `gguf::tensor_info` for
/// why GGUF's on-disk dimension order works out to exactly that.
///
/// Each row is stored as a whole number of quantized blocks — `row_bytes`
/// bytes, computed once from `cols` and the type's block size — so a single
/// row's bytes are always contiguous and directly sliceable, which is what
/// makes `ops::linear_quantized`'s per-row fused dot product possible
/// without indexing into a shared block grid.
#[derive(Debug)]
pub struct QuantizedMatrix {
    pub rows: usize,
    pub cols: usize,
    pub ty: GgmlType,
    raw: Vec<u8>,
    row_bytes: usize,
}

impl QuantizedMatrix {
    pub fn from_raw(rows: usize, cols: usize, ty: GgmlType, raw: Vec<u8>) -> Result<QuantizedMatrix, DequantError> {
        // This is where an unsupported quantization has to be caught: the
        // matmul hot path (`ops::linear_quantized`) consumes `row_bytes()`
        // block-by-block via `dequantize_block_into`, which panics on a type
        // it doesn't implement. Without this check a `Q5_K` checkpoint would
        // load fine and then abort mid-forward. `block_info()` alone does NOT
        // answer this — see `dequant::is_implemented`.
        if !crate::dequant::is_implemented(ty) {
            return Err(DequantError::Unsupported(ty));
        }
        let (block_elems, block_bytes) = ty.block_info().ok_or(DequantError::Unsupported(ty))?;
        let blocks_per_row = (cols as u64).div_ceil(block_elems);
        let row_bytes = (blocks_per_row * block_bytes) as usize;
        let expected = row_bytes * rows;
        if raw.len() != expected {
            return Err(DequantError::SizeMismatch { ty, expected, got: raw.len() });
        }
        Ok(QuantizedMatrix { rows, cols, ty, raw, row_bytes })
    }

    #[inline]
    pub fn row_bytes(&self, r: usize) -> &[u8] {
        &self.raw[r * self.row_bytes..(r + 1) * self.row_bytes]
    }

    /// Fully dequantize one row. Used for embedding lookups (indexing into
    /// a vocab table needs a real `Vec<f32>` to hand back), *not* by the
    /// matmul hot path — that goes through `ops::linear_quantized`, which
    /// consumes `row_bytes()` block-by-block without this allocation.
    pub fn dequant_row(&self, r: usize) -> Vec<f32> {
        dequantize(self.ty, self.row_bytes(r), self.cols).expect("row_bytes()/cols were validated in from_raw")
    }

    /// Slices `[start, start+count)` output rows into a new, owned
    /// `QuantizedMatrix` — safe because `from_raw` guarantees every row is a
    /// whole number of quantized blocks, so row boundaries are always valid
    /// slice points (no mid-block splitting). Used to split a single fused
    /// on-disk tensor into its logical parts: Phi-3's `attn_qkv` (rows
    /// `0..n_embd_q` = Q, `n_embd_q..n_embd_q+n_embd_kv` = K, the rest = V —
    /// VERIFIED against llama.cpp's `build_qkv`, `src/llama-graph.cpp`,
    /// which views the same fused tensor at exactly these row offsets) and
    /// its fused `ffn_up` (first half = gate, second half = up).
    pub fn row_range(&self, start: usize, count: usize) -> QuantizedMatrix {
        assert!(start + count <= self.rows, "row_range({start}, {count}) out of bounds for {} rows", self.rows);
        let raw = self.raw[start * self.row_bytes..(start + count) * self.row_bytes].to_vec();
        QuantizedMatrix { rows: count, cols: self.cols, ty: self.ty, raw, row_bytes: self.row_bytes }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_slicing_and_dequant_row_agree_with_whole_tensor_dequant() {
        // 2 rows x 32 cols of Q8_0 (one block per row).
        let mut raw = vec![0u8; 34 * 2];
        raw[0..2].copy_from_slice(&0x3C00u16.to_le_bytes()); // d=1.0
        raw[2] = 5; // qs[0] = 5
        raw[34..36].copy_from_slice(&0x4000u16.to_le_bytes()); // d=2.0
        raw[36] = 3; // qs[0] = 3

        let qm = QuantizedMatrix::from_raw(2, 32, GgmlType::Q8_0, raw).unwrap();
        assert_eq!(qm.dequant_row(0)[0], 5.0);
        assert_eq!(qm.dequant_row(1)[0], 6.0); // 3 * d(2.0)
    }

    #[test]
    fn rejects_size_mismatch() {
        let raw = vec![0u8; 10];
        match QuantizedMatrix::from_raw(2, 32, GgmlType::Q8_0, raw) {
            Err(DequantError::SizeMismatch { .. }) => {}
            other => panic!("expected SizeMismatch, got {other:?}"),
        }
    }

    #[test]
    fn row_range_matches_dequant_row_of_the_source_matrix() {
        // 4 rows x 32 cols of Q8_0 (one block per row), distinct f16 scale
        // per row (1.0, 2.0, 3.0, 4.0 -- same known-bit-pattern style as the
        // test above) so each row's dequant_row(0) == its row index + 1.
        const D_BITS: [u16; 4] = [0x3C00, 0x4000, 0x4200, 0x4400];
        let mut raw = vec![0u8; 34 * 4];
        for r in 0..4 {
            raw[r * 34..r * 34 + 2].copy_from_slice(&D_BITS[r].to_le_bytes());
            raw[r * 34 + 2] = 1; // qs[0] = 1 -> dequant_row(r)[0] == r+1
        }
        let qm = QuantizedMatrix::from_raw(4, 32, GgmlType::Q8_0, raw).unwrap();

        let middle = qm.row_range(1, 2);
        assert_eq!(middle.rows, 2);
        assert_eq!(middle.cols, 32);
        assert_eq!(middle.dequant_row(0), qm.dequant_row(1));
        assert_eq!(middle.dequant_row(1), qm.dequant_row(2));
    }

    #[test]
    #[should_panic(expected = "out of bounds")]
    fn row_range_rejects_out_of_bounds_slice() {
        let qm = QuantizedMatrix::from_raw(2, 32, GgmlType::Q8_0, vec![0u8; 34 * 2]).unwrap();
        qm.row_range(1, 2);
    }

    #[test]
    fn unsupported_quant_is_rejected_at_construction_not_mid_forward() {
        // A Q5_K weight used to construct fine here (its block layout IS
        // known) and then panic deep inside `ops::linear_quantized`, after
        // the whole checkpoint had already been loaded. `raw` is sized
        // correctly for one row, so the only thing that can fail is the
        // unsupported type — not a size mismatch.
        let (elems, bytes) = GgmlType::Q5K.block_info().expect("Q5_K has a known block layout");
        let raw = vec![0u8; bytes as usize];
        match QuantizedMatrix::from_raw(1, elems as usize, GgmlType::Q5K, raw) {
            Err(DequantError::Unsupported(GgmlType::Q5K)) => {}
            other => panic!("expected Unsupported(Q5K), got {other:?}"),
        }
    }
}
