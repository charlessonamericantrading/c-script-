/// A dense, row-major f32 matrix.
///
/// This is the only tensor shape this crate needs: every weight is either a
/// 1-D vector (norm weights, biases) or a 2-D linear-layer weight, and
/// every activation is a `(seq_len, embed_dim)` matrix (there is
/// deliberately no batch dimension). A general n-d tensor can replace this
/// once a second architecture actually needs one.
#[derive(Debug, Clone)]
pub struct Matrix {
    pub rows: usize,
    pub cols: usize,
    pub data: Vec<f32>,
}

impl Matrix {
    pub fn zeros(rows: usize, cols: usize) -> Matrix {
        Matrix { rows, cols, data: vec![0.0; rows * cols] }
    }

    pub fn from_vec(rows: usize, cols: usize, data: Vec<f32>) -> Matrix {
        assert_eq!(data.len(), rows * cols, "data length does not match rows*cols");
        Matrix { rows, cols, data }
    }

    #[inline]
    pub fn row(&self, r: usize) -> &[f32] {
        &self.data[r * self.cols..(r + 1) * self.cols]
    }

    #[inline]
    pub fn row_mut(&mut self, r: usize) -> &mut [f32] {
        let cols = self.cols;
        &mut self.data[r * cols..(r + 1) * cols]
    }

    #[inline]
    pub fn get(&self, r: usize, c: usize) -> f32 {
        self.data[r * self.cols + c]
    }

    #[inline]
    pub fn set(&mut self, r: usize, c: usize, v: f32) {
        self.data[r * self.cols + c] = v;
    }

    /// A matrix holding just the last row — the usual thing needed after a
    /// full-sequence forward pass, since generation only cares about the
    /// next-token distribution at the final position.
    pub fn last_row(&self) -> &[f32] {
        self.row(self.rows - 1)
    }

    /// An empty (0-row) matrix with `cols` fixed and room pre-reserved for
    /// `capacity_rows` — the starting point for a KV-cache entry that then
    /// grows one row per generated token via `push_row`.
    pub fn with_capacity_rows(cols: usize, capacity_rows: usize) -> Matrix {
        Matrix { rows: 0, cols, data: Vec::with_capacity(cols * capacity_rows) }
    }

    /// Appends one row, growing `rows` by 1. Used by the KV-cache to append
    /// a newly computed position's K/V projection without rebuilding the
    /// whole cache matrix.
    pub fn push_row(&mut self, row: &[f32]) {
        assert_eq!(row.len(), self.cols, "push_row: row length {} != matrix cols {}", row.len(), self.cols);
        self.data.extend_from_slice(row);
        self.rows += 1;
    }
}
