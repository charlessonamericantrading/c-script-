//! The KV-cache: post-RoPE K/V projections for every position generated so
//! far, kept around so `forward_step` never has to recompute a position it
//! already computed. One `LayerKv` per transformer layer, since each
//! layer's attention needs its own K/V history.
//!
//! Shape-only, not architecture-specific: every dense transformer needs
//! exactly `n_layers` per-layer K/V matrices sized by
//! `n_kv_heads * head_dim` columns and up to `context_length` rows, whatever
//! the attention/MLP details inside `forward_step` do with them. Moved out
//! of `qwen2` in Fase 0 (multi-architecture support) for that reason —
//! `llama`/`gemma`/... reuse this unchanged.

use tensor_core::Matrix;

/// The numbers `KvCache::new` needs — deliberately NOT the full
/// architecture `Config` (which varies per model family; e.g. Gemma's
/// embedding-scale factor, Llama's rope-scaling params), just the shape.
pub struct CacheShape {
    pub n_layers: usize,
    pub n_kv_heads: usize,
    /// Per-head K/V dimension, used uniformly for every layer UNLESS
    /// `per_layer_head_dim` overrides it.
    pub head_dim: usize,
    pub context_length: usize,
    /// `Some(dims)` (length `n_layers`) overrides `head_dim` per layer —
    /// needed by `gemma4`, whose SWA and full-attention layers genuinely
    /// use different per-head dimensions (VERIFIED against a real gemma4
    /// checkpoint's `attention.key_length`/`key_length_swa`, which differ:
    /// 512 vs 256 — not a checkpoint-specific quirk, the *architecture*
    /// decouples head_dim from `embed_dim/n_head`). `None` (every other
    /// architecture here) keeps the original single-`head_dim` behavior
    /// unchanged.
    pub per_layer_head_dim: Option<Vec<usize>>,
}

#[derive(Debug, Clone)]
pub struct LayerKv {
    pub k: Matrix,
    pub v: Matrix,
}

#[derive(Debug, Clone)]
pub struct KvCache {
    pub layers: Vec<LayerKv>,
}

impl KvCache {
    pub fn new(shape: &CacheShape) -> KvCache {
        let layers = (0..shape.n_layers)
            .map(|il| {
                let head_dim = shape.per_layer_head_dim.as_ref().map_or(shape.head_dim, |dims| dims[il]);
                let kv_dim = shape.n_kv_heads * head_dim;
                LayerKv {
                    k: Matrix::with_capacity_rows(kv_dim, shape.context_length),
                    v: Matrix::with_capacity_rows(kv_dim, shape.context_length),
                }
            })
            .collect();
        KvCache { layers }
    }

    /// Number of positions cached so far — every layer always has the same
    /// count (`forward_step` appends to all of them together), so any
    /// layer's row count answers this.
    pub fn len(&self) -> usize {
        self.layers.first().map_or(0, |l| l.k.rows)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns a new `KvCache` containing only the first `prefix_len` rows of history.
    pub fn prefix_clone(&self, prefix_len: usize) -> KvCache {
        let n_take = prefix_len.min(self.len());
        let layers = self
            .layers
            .iter()
            .map(|l| {
                let mut k = Matrix::with_capacity_rows(l.k.cols, l.k.rows);
                let mut v = Matrix::with_capacity_rows(l.v.cols, l.v.rows);
                for r in 0..n_take {
                    k.push_row(l.k.row(r));
                    v.push_row(l.v.row(r));
                }
                LayerKv { k, v }
            })
            .collect();
        KvCache { layers }
    }

    /// Truncates the cached history to `max_len` rows.
    pub fn truncate(&mut self, max_len: usize) {
        if max_len >= self.len() {
            return;
        }
        for l in &mut self.layers {
            l.k.data.truncate(max_len * l.k.cols);
            l.k.rows = max_len;
            l.v.data.truncate(max_len * l.v.cols);
            l.v.rows = max_len;
        }
    }
}
