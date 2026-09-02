//! GGUF weight loading for Phi-3 (dense — PhiMoE is a distinct
//! `general.architecture` string, `"phimoe"`, out of scope for this crate;
//! see the crate-level doc comment).
//!
//! Every metadata key, tensor name, and structural claim below is VERIFIED
//! against llama.cpp source (not recalled), fetched 2026-07-12 from
//! ggml-org/llama.cpp commit e3546c7948e3af463d0b401e6421d5a4c2faf565:
//!   - hparams/tensor loading: `src/models/phi3.cpp`'s
//!     `load_arch_hparams`/`load_arch_tensors`.
//!   - fused-QKV creation + row layout: `llama_model_base::create_tensor_qkv`
//!     and `llm_graph_context::build_qkv`, both in `src/llama-model.cpp` /
//!     `src/llama-graph.cpp` — the fused tensor's OUTPUT rows are `[0,
//!     n_embd_q)` = Q, `[n_embd_q, n_embd_q+n_embd_kv)` = K, the remainder =
//!     V (`build_qkv`'s three `ggml_view_3d` byte offsets: `0`,
//!     `row_size(n_embd_q)`, `row_size(n_embd_q + n_embd_kv)`).
//!   - fused gate+up split: `ggml_vec_swiglu_f32`
//!     (`ggml/src/ggml-cpu/vec.h`) computes `SiLU(x) * g` where, for the
//!     single-fused-tensor (no separate `b` tensor) case with `swapped =
//!     false` (the `ggml_swiglu(ctx, a)` used by `phi3.cpp`'s `build_ffn`
//!     call), `x` = the FIRST half of the tensor's last dimension and `g` =
//!     the SECOND half (`ggml_compute_forward_swiglu_f32`,
//!     `ggml/src/ggml-cpu/ops.cpp`) — i.e. `ffn_up`'s output rows `[0,
//!     n_ff)` are the SiLU'd gate, `[n_ff, 2*n_ff)` are the plain multiplier.
//!   - metadata keys: generic `"%s.<key>"` templates from
//!     `src/llama-arch.cpp`'s `LLM_KV_*` table, `%s` = `"phi3"`.
//!
//! KNOWN, DOCUMENTED SIMPLIFICATIONS (not silently assumed correct):
//!   - Only the FUSED `attn_qkv` tensor layout is loaded (`MissingTensor`
//!     error if absent). The C++ reference also supports a separate
//!     wq/wk/wv fallback for generality across many architectures sharing
//!     `create_tensor_qkv`, but real Phi-3 GGUF conversions always produce
//!     the fused tensor (the HF checkpoint itself has one `qkv_proj`
//!     weight, not three) — the fallback path is dead code for this arch in
//!     practice, so it isn't implemented here.
//!   - LongRoPE (`rope_factors_long`/`rope_factors_short`, present only on
//!     long-context checkpoints) IS applied, resolved once at load time —
//!     see `resolve_rope_scaling`'s doc comment for the long-vs-short
//!     selection rule and why it can be a load-time (not per-request)
//!     decision here, and `tensor_core::ops::rope_inplace_with_freq_factors_and_scale`'s
//!     doc comment for the verified formula (freq_factors division +
//!     `attn_factor` amplitude scale, both applied — Phi-3, like Gemma4,
//!     never declares `rope.scaling.type`, so the fuller YaRN
//!     ramp/interpolation this crate does NOT implement never actually
//!     triggers for either architecture; a checkpoint that DID declare it
//!     would need that fuller formula, an open gap flagged, not hidden).
//!     Correct for the (more common on Ollama) base-context Phi-3/Phi-3.5
//!     tags too, which simply have no `rope_factors_long`/`_short` tensors
//!     at all — `active_rope_factors` resolves to `None` for them.
//!   - Sliding-window attention is NOT implemented, matching the reference
//!     itself: `phi3.cpp`'s `load_arch_hparams` unconditionally forces
//!     `hparams.swa_type = LLAMA_SWA_TYPE_NONE` even when the GGUF declares
//!     a sliding window, citing a known conversion-script bug (PR #13676).
//!     `n_swa` is read and stored for visibility but every layer uses full
//!     (global) attention here too — this matches upstream's actual current
//!     behavior, not a shortcut relative to it.
//!   - RMSNorm has no bias term by construction, so `attn_norm`/`ffn_norm`/
//!     `output_norm` biases (which the generic C++ tensor-creation code
//!     defensively supports for OTHER architectures using LayerNorm) are
//!     not modeled — there is nothing for them to load for an RMSNorm arch.
//!     `wo`/`output` biases are similarly not modeled: `TENSOR_NOT_REQUIRED`
//!     in the reference, but not present on any real Phi-3 checkpoint.
//!     Only the fused QKV bias (`attn_qkv.bias`) is modeled, since it rides
//!     along with the fused tensor this crate already loads.

use gguf::{GgufFile, MetadataValue};
use model_core::CacheShape;
use tensor_core::QuantizedMatrix;

use crate::error::LoadError;

#[derive(Debug, Clone)]
pub struct Config {
    pub n_layers: usize,
    pub embed_dim: usize,
    /// `n_ff` — the width of EACH half of the fused `ffn_up` tensor (whose
    /// total output width is `2 * ffn_dim`), and `ffn_down`'s input width.
    pub ffn_dim: usize,
    pub n_heads: usize,
    pub n_kv_heads: usize,
    pub head_dim: usize,
    /// Rotary dimension count — `<= head_dim` when `partial_rotary_factor <
    /// 1.0` (`rope.dimension_count` in the GGUF). Most Phi-3 checkpoints set
    /// this equal to `head_dim` (full rotation), but it's read, not assumed.
    pub n_rot: usize,
    pub vocab_size: usize,
    pub rope_freq_base: f32,
    pub rms_eps: f32,
    pub context_length: usize,
    /// Read for visibility only — NOT applied. See module doc comment:
    /// upstream itself disables SWA for this architecture.
    pub n_swa: usize,
    /// LongRoPE factors, already resolved to whichever of `rope_long`/
    /// `rope_short` applies (see `resolve_rope_scaling`) — `None` if the
    /// checkpoint has neither tensor (the common, base-context case).
    pub active_rope_factors: Option<Vec<f32>>,
    /// LongRoPE amplitude scale (`rope_scaling.attn_factor` in the GGUF,
    /// already baked in by the conversion script — not re-derived here).
    /// `1.0` (a no-op) when the checkpoint has no LongRoPE tensors.
    pub rope_attn_factor: f32,
}

impl Config {
    pub fn cache_shape(&self) -> CacheShape {
        CacheShape { n_layers: self.n_layers, n_kv_heads: self.n_kv_heads, head_dim: self.head_dim, context_length: self.context_length, per_layer_head_dim: None }
    }
}

/// Pre-split at load time (via `QuantizedMatrix::row_range`) from the
/// on-disk fused `attn_qkv`/`ffn_up` tensors, so `forward.rs` reads exactly
/// like `qwen2`/`llama`'s separate-tensor forward pass — the fused-layout
/// complexity is fully contained here, not leaked into the hot path.
pub struct LayerWeights {
    pub attn_norm: Vec<f32>,
    pub wq: QuantizedMatrix,
    pub wk: QuantizedMatrix,
    pub wv: QuantizedMatrix,
    /// `Some` only if the fused `attn_qkv.bias` tensor is present (rare in
    /// practice for Phi-3 — see module doc comment). Pre-split the same way
    /// as the weight, at the same three row ranges.
    pub bq: Option<Vec<f32>>,
    pub bk: Option<Vec<f32>>,
    pub bv: Option<Vec<f32>>,
    pub wo: QuantizedMatrix,
    pub ffn_norm: Vec<f32>,
    /// First half of the fused `ffn_up` tensor's output rows — the SiLU'd
    /// gate. See module doc comment for the verified split convention.
    pub ffn_gate: QuantizedMatrix,
    /// Second half — the plain multiplier.
    pub ffn_up: QuantizedMatrix,
    pub ffn_down: QuantizedMatrix,
}

pub struct Model {
    pub config: Config,
    pub token_embd: QuantizedMatrix,
    /// `None` means tied embeddings — same defensive per-file check
    /// `qwen2`/`llama`/`gemma4` all already use, not assumed either way.
    pub output_weight: Option<QuantizedMatrix>,
    pub output_norm: Vec<f32>,
    pub layers: Vec<LayerWeights>,
}

fn get_qmatrix2d(gguf: &GgufFile, bytes: &[u8], name: &str) -> Result<QuantizedMatrix, LoadError> {
    let t = gguf.tensors.iter().find(|t| t.name == name).ok_or_else(|| LoadError::MissingTensor(name.to_string()))?;
    if t.dimensions.len() != 2 {
        return Err(LoadError::UnexpectedTensorShape { name: name.to_string(), dims: t.dimensions.clone() });
    }
    let (in_features, out_features) = (t.dimensions[0] as usize, t.dimensions[1] as usize);
    let ty = t.ggml_type.ok_or_else(|| LoadError::UnexpectedTensorShape { name: name.to_string(), dims: t.dimensions.clone() })?;
    let raw = get_raw_bytes(gguf, bytes, t)?;
    Ok(QuantizedMatrix::from_raw(out_features, in_features, ty, raw.to_vec())?)
}

fn get_qmatrix2d_opt(gguf: &GgufFile, bytes: &[u8], name: &str) -> Result<Option<QuantizedMatrix>, LoadError> {
    match gguf.tensors.iter().find(|t| t.name == name) {
        Some(_) => Ok(Some(get_qmatrix2d(gguf, bytes, name)?)),
        None => Ok(None),
    }
}

fn get_vector(gguf: &GgufFile, bytes: &[u8], name: &str) -> Result<Vec<f32>, LoadError> {
    let t = gguf.tensors.iter().find(|t| t.name == name).ok_or_else(|| LoadError::MissingTensor(name.to_string()))?;
    if t.dimensions.len() != 1 {
        return Err(LoadError::UnexpectedTensorShape { name: name.to_string(), dims: t.dimensions.clone() });
    }
    let ty = t.ggml_type.ok_or_else(|| LoadError::UnexpectedTensorShape { name: name.to_string(), dims: t.dimensions.clone() })?;
    let n_elements = t.n_elements().ok_or_else(|| LoadError::UnexpectedTensorShape { name: name.to_string(), dims: t.dimensions.clone() })? as usize;
    let raw = get_raw_bytes(gguf, bytes, t)?;
    Ok(tensor_core::dequantize(ty, raw, n_elements)?)
}

fn get_vector_opt(gguf: &GgufFile, bytes: &[u8], name: &str) -> Result<Option<Vec<f32>>, LoadError> {
    match gguf.tensors.iter().find(|t| t.name == name) {
        Some(_) => Ok(Some(get_vector(gguf, bytes, name)?)),
        None => Ok(None),
    }
}

fn get_raw_bytes<'a>(gguf: &GgufFile, bytes: &'a [u8], t: &gguf::TensorInfo) -> Result<&'a [u8], LoadError> {
    let ty = t.ggml_type.ok_or_else(|| LoadError::UnexpectedTensorShape { name: t.name.clone(), dims: t.dimensions.clone() })?;
    let size = t.size_bytes().ok_or(tensor_core::DequantError::Unsupported(ty))?;
    let abs = gguf.tensor_absolute_offset(t)?;
    Ok(&bytes[abs as usize..abs as usize + size as usize])
}

fn meta_u32(gguf: &GgufFile, key: &'static str) -> Result<u32, LoadError> {
    match gguf.metadata.get(key) {
        Some(MetadataValue::Uint32(v)) => Ok(*v),
        Some(MetadataValue::Int32(v)) if *v >= 0 => Ok(*v as u32),
        Some(_) => Err(LoadError::WrongMetadataType(key)),
        None => Err(LoadError::MissingMetadata(key)),
    }
}

fn meta_u32_opt(gguf: &GgufFile, key: &'static str) -> Result<Option<u32>, LoadError> {
    match gguf.metadata.get(key) {
        Some(MetadataValue::Uint32(v)) => Ok(Some(*v)),
        Some(MetadataValue::Int32(v)) if *v >= 0 => Ok(Some(*v as u32)),
        Some(_) => Err(LoadError::WrongMetadataType(key)),
        None => Ok(None),
    }
}

fn meta_f32(gguf: &GgufFile, key: &'static str) -> Result<f32, LoadError> {
    match gguf.metadata.get(key) {
        Some(MetadataValue::Float32(v)) => Ok(*v),
        Some(MetadataValue::Uint32(v)) => Ok(*v as f32),
        Some(_) => Err(LoadError::WrongMetadataType(key)),
        None => Err(LoadError::MissingMetadata(key)),
    }
}

fn meta_f32_opt(gguf: &GgufFile, key: &'static str) -> Result<Option<f32>, LoadError> {
    match gguf.metadata.get(key) {
        Some(MetadataValue::Float32(v)) => Ok(Some(*v)),
        Some(MetadataValue::Uint32(v)) => Ok(Some(*v as f32)),
        Some(_) => Err(LoadError::WrongMetadataType(key)),
        None => Ok(None),
    }
}

/// Picks between `rope_long`/`rope_short`, or neither. VERIFIED against
/// `llama_model::get_rope_factors` (`src/llama-model.cpp`): the reference
/// compares the RUNTIME-CONFIGURED context window (`cparams.n_ctx_seq` —
/// can be smaller OR larger than the checkpoint's own declared
/// `context_length`, e.g. `--ctx-size` on llama.cpp's CLI) against
/// `orig_ctx_len`, and re-checks on every call (a session could reconfigure
/// context size). This engine has no such runtime knob — every model
/// always runs at its own declared `context_length` — so `n_ctx_seq` and
/// `context_length` are the SAME value here, making the choice a genuine
/// load-time constant instead of matching the reference's more general
/// per-call check. If this engine ever grows a configurable context size,
/// this function's `context_length` parameter is exactly what would need
/// to become dynamic.
///
/// `orig_ctx_len` absent means the checkpoint has no LongRoPE metadata at
/// all -- `None` regardless of whether the tensors happen to be present
/// (mirroring `model.rs`'s existing "the reference's TENSOR_NOT_REQUIRED
/// doesn't mean the runtime ever reads it" caution). The selected table
/// being absent (metadata present, tensor missing -- a malformed file) also
/// resolves to `None` rather than guessing.
fn resolve_rope_scaling(context_length: usize, orig_ctx_len: Option<usize>, rope_long: Option<Vec<f32>>, rope_short: Option<Vec<f32>>) -> Option<Vec<f32>> {
    let orig_ctx_len = orig_ctx_len?;
    if context_length > orig_ctx_len {
        rope_long
    } else {
        rope_short
    }
}

/// Splits a fused `[rows, n_embd]` bias vector at the same three row
/// offsets `row_range` uses for the weight tensor — `bq`/`bk`/`bv` are
/// slices of one flat `Vec<f32>`, not separate on-disk tensors.
fn split_qkv_bias(fused: &[f32], n_embd_q: usize, n_embd_kv: usize) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
    let bq = fused[0..n_embd_q].to_vec();
    let bk = fused[n_embd_q..n_embd_q + n_embd_kv].to_vec();
    let bv = fused[n_embd_q + n_embd_kv..n_embd_q + 2 * n_embd_kv].to_vec();
    (bq, bk, bv)
}

impl Model {
    pub fn load(bytes: &[u8]) -> Result<Model, LoadError> {
        let gguf = GgufFile::parse(bytes)?;

        let arch = gguf.architecture().unwrap_or("").to_string();
        if arch != "phi3" {
            return Err(LoadError::UnexpectedArchitecture(arch));
        }

        let n_layers = meta_u32(&gguf, "phi3.block_count")? as usize;
        let embed_dim = meta_u32(&gguf, "phi3.embedding_length")? as usize;
        let ffn_dim = meta_u32(&gguf, "phi3.feed_forward_length")? as usize;
        let n_heads = meta_u32(&gguf, "phi3.attention.head_count")? as usize;
        let n_kv_heads = meta_u32(&gguf, "phi3.attention.head_count_kv")? as usize;
        let rope_freq_base = meta_f32(&gguf, "phi3.rope.freq_base")?;
        let rms_eps = meta_f32(&gguf, "phi3.attention.layer_norm_rms_epsilon")?;
        let context_length = meta_u32(&gguf, "phi3.context_length")? as usize;
        let n_rot = meta_u32(&gguf, "phi3.rope.dimension_count")? as usize;
        let n_swa = meta_u32_opt(&gguf, "phi3.attention.sliding_window")?.unwrap_or(0) as usize;
        let head_dim = embed_dim / n_heads;

        let n_embd_q = n_heads * head_dim;
        let n_embd_kv = n_kv_heads * head_dim;

        let token_embd = get_qmatrix2d(&gguf, bytes, "token_embd.weight")?;
        let vocab_size = token_embd.rows;
        let output_norm = get_vector(&gguf, bytes, "output_norm.weight")?;
        let output_weight = get_qmatrix2d_opt(&gguf, bytes, "output.weight")?;

        // Global (not per-layer) LongRoPE factors -- VERIFIED via
        // TENSOR_DUPLICATED in phi3.cpp's load_arch_tensors: only layer 0's
        // copy is real, every other layer's is a reference to the same
        // data, so there is exactly one array to load, not n_layers of them.
        let rope_long = get_vector_opt(&gguf, bytes, "rope_factors_long.weight")?;
        let rope_short = get_vector_opt(&gguf, bytes, "rope_factors_short.weight")?;
        let rope_attn_factor = meta_f32_opt(&gguf, "phi3.rope.scaling.attn_factor")?.unwrap_or(1.0);
        let orig_ctx_len = meta_u32_opt(&gguf, "phi3.rope.scaling.original_context_length")?.map(|v| v as usize);
        let active_rope_factors = resolve_rope_scaling(context_length, orig_ctx_len, rope_long, rope_short);

        let mut layers = Vec::with_capacity(n_layers);
        for i in 0..n_layers {
            let p = |suffix: &str| format!("blk.{i}.{suffix}");

            let wqkv = get_qmatrix2d(&gguf, bytes, &p("attn_qkv.weight"))?;
            let wq = wqkv.row_range(0, n_embd_q);
            let wk = wqkv.row_range(n_embd_q, n_embd_kv);
            let wv = wqkv.row_range(n_embd_q + n_embd_kv, n_embd_kv);

            let (bq, bk, bv) = match get_vector_opt(&gguf, bytes, &p("attn_qkv.bias"))? {
                Some(fused_bias) => {
                    let (bq, bk, bv) = split_qkv_bias(&fused_bias, n_embd_q, n_embd_kv);
                    (Some(bq), Some(bk), Some(bv))
                }
                None => (None, None, None),
            };

            let ffn_up_fused = get_qmatrix2d(&gguf, bytes, &p("ffn_up.weight"))?;
            let ffn_gate = ffn_up_fused.row_range(0, ffn_dim);
            let ffn_up = ffn_up_fused.row_range(ffn_dim, ffn_dim);

            layers.push(LayerWeights {
                attn_norm: get_vector(&gguf, bytes, &p("attn_norm.weight"))?,
                wq,
                wk,
                wv,
                bq,
                bk,
                bv,
                wo: get_qmatrix2d(&gguf, bytes, &p("attn_output.weight"))?,
                ffn_norm: get_vector(&gguf, bytes, &p("ffn_norm.weight"))?,
                ffn_gate,
                ffn_up,
                ffn_down: get_qmatrix2d(&gguf, bytes, &p("ffn_down.weight"))?,
            });
        }

        Ok(Model {
            config: Config {
                n_layers,
                embed_dim,
                ffn_dim,
                n_heads,
                n_kv_heads,
                head_dim,
                n_rot,
                vocab_size,
                rope_freq_base,
                rms_eps,
                context_length,
                n_swa,
                active_rope_factors,
                rope_attn_factor,
            },
            token_embd,
            output_weight,
            output_norm,
            layers,
        })
    }
}
