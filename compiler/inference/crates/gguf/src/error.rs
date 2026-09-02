use std::fmt;

/// Everything that can go wrong while parsing a GGUF file.
///
/// Kept as an explicit enum (no `Box<dyn Error>`) so a caller can match on
/// the exact failure — "ran off the end of the file" and "unknown value
/// type" need very different handling once this parser is driven by
/// untrusted model downloads instead of local test files.
#[derive(Debug)]
pub enum GgufError {
    UnexpectedEof { wanted: usize, at: usize, len: usize },
    BadMagic { found: u32 },
    UnsupportedVersion { found: u32 },
    InvalidUtf8 { at: usize },
    UnknownValueType { raw: u32, at: usize },
    UnknownGgmlType { raw: u32, tensor: String },
    NestedArray { at: usize },
    Overflow(&'static str),
}

impl fmt::Display for GgufError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GgufError::UnexpectedEof { wanted, at, len } => write!(
                f,
                "unexpected end of file: wanted {wanted} bytes at offset {at}, file is {len} bytes"
            ),
            GgufError::BadMagic { found } => write!(
                f,
                "not a GGUF file: magic is 0x{found:08x}, expected 0x{:08x} (\"GGUF\")",
                super::GGUF_MAGIC
            ),
            GgufError::UnsupportedVersion { found } => {
                write!(f, "unsupported GGUF version {found} (this parser supports v2 and v3)")
            }
            GgufError::InvalidUtf8 { at } => write!(f, "invalid UTF-8 string at offset {at}"),
            GgufError::UnknownValueType { raw, at } => {
                write!(f, "unknown metadata value type {raw} at offset {at}")
            }
            GgufError::UnknownGgmlType { raw, tensor } => {
                write!(f, "unknown ggml tensor type {raw} for tensor \"{tensor}\"")
            }
            GgufError::NestedArray { at } => {
                write!(f, "array-of-array metadata value at offset {at} (not allowed by the GGUF spec)")
            }
            GgufError::Overflow(what) => write!(f, "integer overflow computing {what}"),
        }
    }
}

impl std::error::Error for GgufError {}

pub type Result<T> = std::result::Result<T, GgufError>;
