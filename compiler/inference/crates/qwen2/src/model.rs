use gguf::{GgufFile, MetadataValue};
use model_core::CacheShape;
use tensor_core::QuantizedMatrix;

use crate::error::LoadError;

#[derive(Debug, Clone)]
pub struct Config {
    pub n_layers: usize,
    pub embed_dim: usize,
    pub ffn_dim: usize,
    pub n_heads: usize,
    pub n_kv_heads: usize,
    pub head_dim: usize,
    pub vocab_size: usize,
    pub rope_freq_base: f32,
    pub rms_eps: f32,
    pub context_length: usize,
}

impl Config {
    /// The subset of fields `model_core::KvCache::new` needs — see
    /// `model_core::cache`'s doc comment for why the cache itself doesn't
    /// need the rest of this struct (rope/norm/etc. never touch the cache).
    pub fn cache_shape(&self) -> CacheShape {
        CacheShape {
            n_layers: self.n_layers,
            n_kv_heads: self.n_kv_heads,
            head_dim: self.head_dim,
            context_length: self.context_length,
            per_layer_head_dim: None,
        }
    }
}

/// The big matmul weights stay `QuantizedMatrix` (native on-disk bytes,
/// consumed block-at-a-time by `tensor_core::ops::linear_quantized`) —
/// Fase 2's change from Fase 1, which fully dequantized every weight to f32
/// at load time. Norm weights and biases are tiny (a few KB each, already
/// F32 on disk) and stay plain `Vec<f32>`; dequantizing those up front
/// costs nothing and keeps the forward pass code simple where it doesn't
/// matter.
pub struct LayerWeights {
    pub attn_norm: Vec<f32>,
    pub wq: QuantizedMatrix,
    pub bq: Vec<f32>,
    pub wk: QuantizedMatrix,
    pub bk: Vec<f32>,
    pub wv: QuantizedMatrix,
    pub bv: Vec<f32>,
    pub wo: QuantizedMatrix,
    pub ffn_norm: Vec<f32>,
    pub w_gate: QuantizedMatrix,
    pub w_up: QuantizedMatrix,
    pub w_down: QuantizedMatrix,
}

pub struct Model {
    pub config: Config,
    /// (vocab_size, embed_dim). Embedding lookup dequantizes one row at a
    /// time (`QuantizedMatrix::dequant_row`).
    pub token_embd: QuantizedMatrix,
    /// The output (LM head) projection, if this checkpoint has one as a
    /// separate tensor. `None` means tied embeddings — use `token_embd`
    /// itself for the final projection instead (see `forward.rs`).
    ///
    /// This is a real per-model architectural difference, not a detail to
    /// assume: Qwen2.5-0.5B-Instruct ties input/output embeddings (no
    /// `output.weight` tensor at all — verified in Fase 0's tensor dump),
    /// but Qwen2.5-Coder-7B-Instruct does **not** — it has a distinct
    /// `output.weight` tensor. An earlier version of this loader assumed
    /// tied embeddings unconditionally (true for the only model tested at
    /// the time) and silently ran the final projection through the *input*
    /// embedding table for every model — numerically well-behaved (no
    /// NaN/explosion, since it's still a valid matmul) but semantically
    /// meaningless, producing fluent-looking garbage only at the very last
    /// step. Caught by comparing against Ollama on qwen2.5-coder:7b, where
    /// it produced incoherent completions despite every transformer layer
    /// computing correctly.
    pub output_weight: Option<QuantizedMatrix>,
    pub layers: Vec<LayerWeights>,
    pub output_norm: Vec<f32>,
}

/// Every 2-D weight this model uses follows GGUF's own convention: on-disk
/// dims are `[in_features, out_features]` (fastest-varying axis first), so
/// as a row-major `(rows, cols)` matrix that's `rows = out_features`,
/// `cols = in_features` — i.e. exactly `nn.Linear.weight`'s layout. See
/// `gguf::tensor_info::TensorInfo` for the source of that convention.
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

fn meta_f32(gguf: &GgufFile, key: &'static str) -> Result<f32, LoadError> {
    match gguf.metadata.get(key) {
        Some(MetadataValue::Float32(v)) => Ok(*v),
        Some(MetadataValue::Uint32(v)) => Ok(*v as f32),
        Some(_) => Err(LoadError::WrongMetadataType(key)),
        None => Err(LoadError::MissingMetadata(key)),
    }
}

impl Model {
    pub fn load(bytes: &[u8]) -> Result<Model, LoadError> {
        let gguf = GgufFile::parse(bytes)?;

        let arch = gguf.architecture().unwrap_or("").to_string();
        if arch != "qwen2" {
            return Err(LoadError::UnexpectedArchitecture(arch));
        }

        let n_layers = meta_u32(&gguf, "qwen2.block_count")? as usize;
        let embed_dim = meta_u32(&gguf, "qwen2.embedding_length")? as usize;
        let ffn_dim = meta_u32(&gguf, "qwen2.feed_forward_length")? as usize;
        let n_heads = meta_u32(&gguf, "qwen2.attention.head_count")? as usize;
        let n_kv_heads = meta_u32(&gguf, "qwen2.attention.head_count_kv")? as usize;
        let rope_freq_base = meta_f32(&gguf, "qwen2.rope.freq_base")?;
        let rms_eps = meta_f32(&gguf, "qwen2.attention.layer_norm_rms_epsilon")?;
        let context_length = meta_u32(&gguf, "qwen2.context_length")? as usize;
        let head_dim = embed_dim / n_heads;

        let token_embd = get_qmatrix2d(&gguf, bytes, "token_embd.weight")?;
        let vocab_size = token_embd.rows;
        let output_norm = get_vector(&gguf, bytes, "output_norm.weight")?;
        // Present only for checkpoints with untied input/output embeddings
        // (e.g. Qwen2.5-Coder-7B) — absent means tied (e.g. Qwen2.5-0.5B),
        // in which case `forward_step` reuses `token_embd` for the final
        // projection instead. Do not assume either way — check the file.
        let output_weight = match gguf.tensors.iter().find(|t| t.name == "output.weight") {
            Some(_) => Some(get_qmatrix2d(&gguf, bytes, "output.weight")?),
            None => None,
        };

        let mut layers = Vec::with_capacity(n_layers);
        for i in 0..n_layers {
            let p = |suffix: &str| format!("blk.{i}.{suffix}");
            layers.push(LayerWeights {
                attn_norm: get_vector(&gguf, bytes, &p("attn_norm.weight"))?,
                wq: get_qmatrix2d(&gguf, bytes, &p("attn_q.weight"))?,
                bq: get_vector(&gguf, bytes, &p("attn_q.bias"))?,
                wk: get_qmatrix2d(&gguf, bytes, &p("attn_k.weight"))?,
                bk: get_vector(&gguf, bytes, &p("attn_k.bias"))?,
                wv: get_qmatrix2d(&gguf, bytes, &p("attn_v.weight"))?,
                bv: get_vector(&gguf, bytes, &p("attn_v.bias"))?,
                wo: get_qmatrix2d(&gguf, bytes, &p("attn_output.weight"))?,
                ffn_norm: get_vector(&gguf, bytes, &p("ffn_norm.weight"))?,
                w_gate: get_qmatrix2d(&gguf, bytes, &p("ffn_gate.weight"))?,
                w_up: get_qmatrix2d(&gguf, bytes, &p("ffn_up.weight"))?,
                w_down: get_qmatrix2d(&gguf, bytes, &p("ffn_down.weight"))?,
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
                vocab_size,
                rope_freq_base,
                rms_eps,
                context_length,
            },
            token_embd,
            output_weight,
            layers,
            output_norm,
        })
    }
}
