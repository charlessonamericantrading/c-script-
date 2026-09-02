//! A from-scratch reader for the GGUF model format.
//!
//! This crate only *reads* GGUF — locating the header, metadata key/value
//! pairs, and the tensor-info table, and computing where each tensor's
//! bytes live in the file. It does not interpret tensor bytes (that's the
//! tensor-core crate's job once it exists) and does not write GGUF.
//!
//! Deliberately dependency-free: the binary layout is simple enough that a
//! hand-rolled cursor (see `reader`) is less code and less risk than a
//! general-purpose parsing crate would be.

pub mod error;
pub mod file;
pub mod header;
pub mod reader;
pub mod tensor_info;
pub mod types;
pub mod value;

pub use error::{GgufError, Result};
pub use file::GgufFile;
pub use header::GgufHeader;
pub use tensor_info::TensorInfo;
pub use types::GgmlType;
pub use value::MetadataValue;

/// The four magic bytes "GGUF" read as a little-endian u32.
pub const GGUF_MAGIC: u32 = 0x4655_4747;
