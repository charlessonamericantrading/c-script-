use std::fmt;

#[derive(Debug)]
pub enum LoadError {
    Gguf(gguf::GgufError),
    Dequant(tensor_core::DequantError),
    MissingTensor(String),
    MissingMetadata(&'static str),
    WrongMetadataType(&'static str),
    UnexpectedTensorShape { name: String, dims: Vec<u64> },
    UnexpectedArchitecture(String),
    /// `tokenizer.ggml.pre` was present but isn't one this crate implements
    /// a pretokenizer regex for (`"llama3"`/`"llama-bpe"` or `"tekken"` —
    /// see `tokenizer.rs`'s module doc comment). Surfaced as a real error
    /// rather than silently applying the wrong regex.
    UnsupportedTokenizerPre(String),
    /// `tokenizer.ggml.model` was present but isn't `"gpt2"` (byte-level
    /// BPE) or `"llama"` (SentencePiece) — the only two vocab types this
    /// crate implements. See `tokenizer.rs`'s module doc comment.
    UnsupportedTokenizerModel(String),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Gguf(e) => write!(f, "{e}"),
            LoadError::Dequant(e) => write!(f, "{e}"),
            LoadError::MissingTensor(name) => write!(f, "model is missing expected tensor \"{name}\""),
            LoadError::MissingMetadata(key) => write!(f, "model is missing expected metadata key \"{key}\""),
            LoadError::WrongMetadataType(key) => write!(f, "metadata key \"{key}\" has an unexpected value type"),
            LoadError::UnexpectedTensorShape { name, dims } => {
                write!(f, "tensor \"{name}\" has unexpected shape {dims:?}")
            }
            LoadError::UnexpectedArchitecture(arch) => {
                write!(f, "expected general.architecture = \"llama\", found \"{arch}\"")
            }
            LoadError::UnsupportedTokenizerPre(pre) => {
                write!(f, "tokenizer.ggml.pre = \"{pre}\" is not supported (only \"llama3\"/\"llama-bpe\" and \"tekken\" are implemented -- see tokenizer.rs)")
            }
            LoadError::UnsupportedTokenizerModel(model) => {
                write!(f, "tokenizer.ggml.model = \"{model}\" is not supported (only \"gpt2\" and \"llama\" are implemented -- see tokenizer.rs)")
            }
        }
    }
}

impl std::error::Error for LoadError {}
impl From<gguf::GgufError> for LoadError {
    fn from(e: gguf::GgufError) -> Self {
        LoadError::Gguf(e)
    }
}
impl From<tensor_core::DequantError> for LoadError {
    fn from(e: tensor_core::DequantError) -> Self {
        LoadError::Dequant(e)
    }
}
