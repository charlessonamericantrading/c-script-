//! GGUF weight loading for PhiMoE (Microsoft's "Phi-3.5-MoE") — a distinct
//! `general.architecture` string, `"phimoe"`, not a flag on dense Phi-3.
//!
//! VERIFIED against llama.cpp (commit e920c523e3b8a0163fe498af5bf90df35ff51d25:
//! `src/llama-arch.cpp`, `src/models/phimoe.cpp`, `src/models/phi3.cpp` —
//! phimoe's graph is a literal type alias to phi3's compiled template,
//! `src/models/models.h`, so attention/RoPE/residual mechanics are
//! PROVABLY identical between the two, only tensor layout and the FFN
//! block differ) and Microsoft's own `microsoft/Phi-3.5-MoE-instruct`
//! checkpoint (config.json/modeling_phimoe.py), via web research this
//! session — not recalled from memory. Several of these are genuine
//! surprises relative to "phimoe = dense phi3 + MoE":
//!
//!   - **QKV is SPLIT, not fused** — unlike dense phi3's single
//!     `attn_qkv.weight`, PhiMoE has separate `attn_q`/`attn_k`/`attn_v`
//!     tensors (Mixtral-style, matching its HF `PhiMoEAttention` class),
//!     each with a REQUIRED bias. `attn_output` also has a required bias.
//!   - **Every norm has a bias**: `attn_norm`/`ffn_norm`/`output_norm` are
//!     all `{weight, bias}` pairs, all required. llama.cpp's actual graph
//!     code (shared with phi3) computes these via `ggml_rms_norm` (no
//!     mean subtraction) plus a bias add — genuinely NOT the true
//!     mean-centering LayerNorm Microsoft's own reference math uses
//!     (`nn.LayerNorm(elementwise_affine=True)` in `modeling_phimoe.py`).
//!     This is a real, verified divergence baked into llama.cpp itself,
//!     not something introduced here — matched deliberately, since any
//!     real PhiMoE GGUF was produced by (and any Ollama-compatible
//!     consumer expects) llama.cpp's conversion/inference behavior, not
//!     Microsoft's bit-exact reference.
//!   - **The output (LM head) projection is REQUIRED, not optional** —
//!     `phimoe.cpp` marks `output.weight`/`output.bias` as required
//!     (unlike qwen2/qwen3/gemma4/phi3, which all use
//!     `TENSOR_NOT_REQUIRED` there to support tied embeddings). No
//!     tied-embedding fallback path exists for this architecture in
//!     llama.cpp, so none is modeled here either.
//!   - **`head_dim` is DERIVED** (`embed_dim/n_heads`), not explicit
//!     metadata — unlike Gemma4/Phi-3/Qwen3, which all need the opposite
//!     caution. Verified directly: `src/models/phimoe.cpp:15`.
//!   - **LongRoPE is the SAME mechanism as dense phi3** — same
//!     `rope_factors_long`/`rope_factors_short` tensors, same
//!     `attn_factor` amplitude scale, same load-time long-vs-short
//!     selection rule. `resolve_rope_scaling` below is copied from
//!     `phi3::model`'s function of the same name verbatim (see that
//!     crate for the fuller reasoning) rather than depended on, matching
//!     this workspace's established one-crate-per-architecture-is-
//!     self-contained convention. `rope.dimension_count` is read as
//!     OPTIONAL here (falling back to `head_dim`, i.e. full rotary) —
//!     a deliberate deviation from dense phi3's REQUIRED treatment of
//!     the same key: the research backing this crate could not confirm
//!     whether `phimoe.rope.dimension_count` is always written (the one
//!     real checkpoint checked has no partial-rotary factor at all, so
//!     the key may simply be absent on it), and rejecting a checkpoint
//!     llama.cpp itself loads fine would be a worse failure mode than a
//!     well-justified default.
//!
//! Qwen3MoE's routing style — plain softmax-over-all-experts + top-k +
//! renormalize, `tensor_core::ops::moe_route`'s existing contract — is
//! CONFIRMED to be exactly what llama.cpp uses for PhiMoE too (read
//! directly from `build_moe_ffn`'s body, `gating_op=SOFTMAX`, no bias, no
//! expert groups, `expert_weights_scale=0.0` unset/no-op). Microsoft's own
//! reference implementation uses a more elaborate "SparseMixer" scheme at
//! inference time (two masked softmax passes per pick, no final
//! renormalization) — llama.cpp implements no such code path for PhiMoE
//! at all, so this crate doesn't either, for the same reason as the
//! RMSNorm-vs-LayerNorm divergence above: matching what a real GGUF file
//! and its consumers actually run, not Microsoft's bit-exact reference.
//! Unlike Qwen3MoE (which can be purely dense, "qwen3"), EVERY PhiMoE
//! layer is unconditionally MoE — there is no dense FFN fallback tensor
//! or code path to model.
//!
//! STANDING CONSTRAINT: no real PhiMoE GGUF file exists locally (the
//! "never download a model" rule) and none is available via the local
//! Ollama install either, so this crate is unverified-in-anger like
//! `llama`'s Mistral/classic-Llama support and `qwen3` — synthetic unit
//! tests (shapes, no NaN/Inf, the genuinely new mechanisms actually
//! changing output when exercised) are the current ceiling.

use gguf::{GgufFile, MetadataValue};
use model_core::CacheShape;
use tensor_core::QuantizedMatrix;

use crate::error::LoadError;

#[derive(Debug, Clone)]
pub struct Config {
    pub n_layers: usize,
    pub embed_dim: usize,
    pub n_heads: usize,
    pub n_kv_heads: usize,
    pub head_dim: usize,
    /// Rotary dimension count -- see module doc comment for why this is
    /// read as optional (falls back to `head_dim`) here, unlike phi3.
    pub n_rot: usize,
    pub vocab_size: usize,
    pub rope_freq_base: f32,
    pub rms_eps: f32,
    pub context_length: usize,
    pub active_rope_factors: Option<Vec<f32>>,
    pub rope_attn_factor: f32,
    pub n_expert_used: usize,
}

impl Config {
    pub fn cache_shape(&self) -> CacheShape {
        CacheShape { n_layers: self.n_layers, n_kv_heads: self.n_kv_heads, head_dim: self.head_dim, context_length: self.context_length, per_layer_head_dim: None }
    }
}

/// Every PhiMoE layer's FFN block, unconditionally (module doc comment) --
/// no dense fallback, unlike Qwen3MoE's per-layer `Option`.
pub struct MoeWeights {
    pub gate_inp: QuantizedMatrix,       // router: [n_expert, embed_dim], no bias
    pub gate_exps: Vec<QuantizedMatrix>, // n_expert x [n_ff_exp, embed_dim]
    pub up_exps: Vec<QuantizedMatrix>,   // n_expert x [n_ff_exp, embed_dim]
    pub down_exps: Vec<QuantizedMatrix>, // n_expert x [embed_dim, n_ff_exp]
}

pub struct LayerWeights {
    pub attn_norm: Vec<f32>,
    pub attn_norm_bias: Vec<f32>,
    pub wq: QuantizedMatrix,
    pub bq: Vec<f32>,
    pub wk: QuantizedMatrix,
    pub bk: Vec<f32>,
    pub wv: QuantizedMatrix,
    pub bv: Vec<f32>,
    pub wo: QuantizedMatrix,
    pub bo: Vec<f32>,
    pub ffn_norm: Vec<f32>,
    pub ffn_norm_bias: Vec<f32>,
    pub moe: MoeWeights,
}

pub struct Model {
    pub config: Config,
    pub token_embd: QuantizedMatrix,
    /// Required, not `Option` -- see module doc comment: PhiMoE has no
    /// tied-embedding fallback in llama.cpp.
    pub output_weight: QuantizedMatrix,
    pub output_bias: Vec<f32>,
    pub output_norm: Vec<f32>,
    pub output_norm_bias: Vec<f32>,
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

/// Slices a 3-D `[embed_dim, n_ff_exp, n_expert]` tensor (`n_expert` is the
/// SLOWEST-varying axis in GGUF's fastest-first convention) into `n_expert`
/// independent `QuantizedMatrix`es. Same technique as `qwen3`'s own
/// `get_expert_qmatrices` (and Gemma4's before it) -- duplicated rather
/// than shared, matching this workspace's established
/// one-crate-per-architecture-is-self-contained convention.
fn get_expert_qmatrices(gguf: &GgufFile, bytes: &[u8], name: &str, n_expert: usize) -> Result<Vec<QuantizedMatrix>, LoadError> {
    let t = gguf.tensors.iter().find(|t| t.name == name).ok_or_else(|| LoadError::MissingTensor(name.to_string()))?;
    if t.dimensions.len() != 3 {
        return Err(LoadError::UnexpectedTensorShape { name: name.to_string(), dims: t.dimensions.clone() });
    }
    let (in_features, out_features, n_expert_in_file) = (t.dimensions[0] as usize, t.dimensions[1] as usize, t.dimensions[2] as usize);
    if n_expert_in_file != n_expert {
        return Err(LoadError::UnexpectedTensorShape { name: name.to_string(), dims: t.dimensions.clone() });
    }
    let ty = t.ggml_type.ok_or_else(|| LoadError::UnexpectedTensorShape { name: name.to_string(), dims: t.dimensions.clone() })?;
    let total_bytes = t.size_bytes().ok_or(tensor_core::DequantError::Unsupported(ty))? as usize;
    if !total_bytes.is_multiple_of(n_expert) {
        return Err(LoadError::UnexpectedTensorShape { name: name.to_string(), dims: t.dimensions.clone() });
    }
    let per_expert_bytes = total_bytes / n_expert;
    let raw = get_raw_bytes(gguf, bytes, t)?;

    let mut out = Vec::with_capacity(n_expert);
    for e in 0..n_expert {
        let slice = &raw[e * per_expert_bytes..(e + 1) * per_expert_bytes];
        out.push(QuantizedMatrix::from_raw(out_features, in_features, ty, slice.to_vec())?);
    }
    Ok(out)
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

/// Copied from `phi3::model`'s function of the same name (see that
/// crate's doc comment for the fuller reasoning) rather than depended on,
/// matching this workspace's established one-crate-per-architecture-is-
/// self-contained convention. Picks between `rope_long`/`rope_short`, or
/// neither, by comparing this checkpoint's own `context_length` against
/// its declared `original_context_length` -- a load-time constant here
/// since this engine has no runtime-configurable context size.
fn resolve_rope_scaling(context_length: usize, orig_ctx_len: Option<usize>, rope_long: Option<Vec<f32>>, rope_short: Option<Vec<f32>>) -> Option<Vec<f32>> {
    let orig_ctx_len = orig_ctx_len?;
    if context_length > orig_ctx_len {
        rope_long
    } else {
        rope_short
    }
}

impl Model {
    pub fn load(bytes: &[u8]) -> Result<Model, LoadError> {
        let gguf = GgufFile::parse(bytes)?;

        let arch = gguf.architecture().unwrap_or("").to_string();
        if arch != "phimoe" {
            return Err(LoadError::UnexpectedArchitecture(arch));
        }

        let n_layers = meta_u32(&gguf, "phimoe.block_count")? as usize;
        let embed_dim = meta_u32(&gguf, "phimoe.embedding_length")? as usize;
        let n_heads = meta_u32(&gguf, "phimoe.attention.head_count")? as usize;
        let n_kv_heads = meta_u32(&gguf, "phimoe.attention.head_count_kv")? as usize;
        // Derived, NOT explicit metadata -- see module doc comment (the
        // opposite gotcha from Gemma4/Phi-3/Qwen3).
        let head_dim = embed_dim / n_heads;
        let rope_freq_base = meta_f32(&gguf, "phimoe.rope.freq_base")?;
        let rms_eps = meta_f32(&gguf, "phimoe.attention.layer_norm_rms_epsilon")?;
        let context_length = meta_u32(&gguf, "phimoe.context_length")? as usize;
        // Optional here, unlike phi3's required treatment -- see module doc
        // comment.
        let n_rot = meta_u32_opt(&gguf, "phimoe.rope.dimension_count")?.map(|v| v as usize).unwrap_or(head_dim);
        // Every layer is MoE (module doc comment), so both are required --
        // there's no dense fallback for these to silently default toward.
        let n_expert = meta_u32(&gguf, "phimoe.expert_count")? as usize;
        let n_expert_used = meta_u32(&gguf, "phimoe.expert_used_count")? as usize;

        let token_embd = get_qmatrix2d(&gguf, bytes, "token_embd.weight")?;
        let vocab_size = token_embd.rows;
        let output_norm = get_vector(&gguf, bytes, "output_norm.weight")?;
        let output_norm_bias = get_vector(&gguf, bytes, "output_norm.bias")?;
        let output_weight = get_qmatrix2d(&gguf, bytes, "output.weight")?;
        let output_bias = get_vector(&gguf, bytes, "output.bias")?;

        // Global (not per-layer) LongRoPE factors -- same TENSOR_DUPLICATED
        // convention as phi3 (only layer 0's copy is real on disk).
        let rope_long = get_vector_opt(&gguf, bytes, "rope_factors_long.weight")?;
        let rope_short = get_vector_opt(&gguf, bytes, "rope_factors_short.weight")?;
        let rope_attn_factor = meta_f32_opt(&gguf, "phimoe.rope.scaling.attn_factor")?.unwrap_or(1.0);
        let orig_ctx_len = meta_u32_opt(&gguf, "phimoe.rope.scaling.original_context_length")?.map(|v| v as usize);
        let active_rope_factors = resolve_rope_scaling(context_length, orig_ctx_len, rope_long, rope_short);

        let mut layers = Vec::with_capacity(n_layers);
        for i in 0..n_layers {
            let p = |suffix: &str| format!("blk.{i}.{suffix}");

            layers.push(LayerWeights {
                attn_norm: get_vector(&gguf, bytes, &p("attn_norm.weight"))?,
                attn_norm_bias: get_vector(&gguf, bytes, &p("attn_norm.bias"))?,
                wq: get_qmatrix2d(&gguf, bytes, &p("attn_q.weight"))?,
                bq: get_vector(&gguf, bytes, &p("attn_q.bias"))?,
                wk: get_qmatrix2d(&gguf, bytes, &p("attn_k.weight"))?,
                bk: get_vector(&gguf, bytes, &p("attn_k.bias"))?,
                wv: get_qmatrix2d(&gguf, bytes, &p("attn_v.weight"))?,
                bv: get_vector(&gguf, bytes, &p("attn_v.bias"))?,
                wo: get_qmatrix2d(&gguf, bytes, &p("attn_output.weight"))?,
                bo: get_vector(&gguf, bytes, &p("attn_output.bias"))?,
                ffn_norm: get_vector(&gguf, bytes, &p("ffn_norm.weight"))?,
                ffn_norm_bias: get_vector(&gguf, bytes, &p("ffn_norm.bias"))?,
                moe: MoeWeights {
                    gate_inp: get_qmatrix2d(&gguf, bytes, &p("ffn_gate_inp.weight"))?,
                    gate_exps: get_expert_qmatrices(&gguf, bytes, &p("ffn_gate_exps.weight"), n_expert)?,
                    up_exps: get_expert_qmatrices(&gguf, bytes, &p("ffn_up_exps.weight"), n_expert)?,
                    down_exps: get_expert_qmatrices(&gguf, bytes, &p("ffn_down_exps.weight"), n_expert)?,
                },
            });
        }

        Ok(Model {
            config: Config {
                n_layers,
                embed_dim,
                n_heads,
                n_kv_heads,
                head_dim,
                n_rot,
                vocab_size,
                rope_freq_base,
                rms_eps,
                context_length,
                active_rope_factors,
                rope_attn_factor,
                n_expert_used,
            },
            token_embd,
            output_weight,
            output_bias,
            output_norm,
            output_norm_bias,
            layers,
        })
    }
}
