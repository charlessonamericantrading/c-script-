use std::fmt;

#[derive(Debug)]
pub enum LoadError {
    Gguf(gguf::GgufError),
    Dequant(tensor_core::DequantError),
    MissingTensor(String),
    /// Metadata keys are built at runtime (`"{arch}.block_count"` — `arch`
    /// is whichever of "qwen3"/"qwen3moe" this checkpoint declared, see
    /// `model.rs`'s module doc comment), so unlike every other crate here
    /// these can't be `&'static str`.
    MissingMetadata(String),
    WrongMetadataType(String),
    UnexpectedTensorShape { name: String, dims: Vec<u64> },
    UnexpectedArchitecture(String),
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
                write!(f, "expected general.architecture = \"qwen3\" or \"qwen3moe\", found \"{arch}\"")
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
