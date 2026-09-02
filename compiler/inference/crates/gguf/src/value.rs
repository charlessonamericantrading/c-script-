use crate::error::{GgufError, Result};
use crate::reader::Reader;

/// The `gguf_metadata_value_type` tag. Numeric values are on-disk, load-bearing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueType {
    Uint8,
    Int8,
    Uint16,
    Int16,
    Uint32,
    Int32,
    Float32,
    Bool,
    String,
    Array,
    Uint64,
    Int64,
    Float64,
}

impl ValueType {
    fn from_u32(v: u32) -> Option<ValueType> {
        use ValueType::*;
        Some(match v {
            0 => Uint8,
            1 => Int8,
            2 => Uint16,
            3 => Int16,
            4 => Uint32,
            5 => Int32,
            6 => Float32,
            7 => Bool,
            8 => String,
            9 => Array,
            10 => Uint64,
            11 => Int64,
            12 => Float64,
            _ => return None,
        })
    }
}

/// A decoded metadata value. Mirrors the `gguf_metadata_value_t` union, but
/// as a real Rust enum since we're not trying to preserve zero-copy access
/// to the original bytes here — metadata is tiny compared to tensor data.
#[derive(Debug, Clone, PartialEq)]
pub enum MetadataValue {
    Uint8(u8),
    Int8(i8),
    Uint16(u16),
    Int16(i16),
    Uint32(u32),
    Int32(i32),
    Float32(f32),
    Bool(bool),
    Uint64(u64),
    Int64(i64),
    Float64(f64),
    String(String),
    Array(Vec<MetadataValue>),
}

impl MetadataValue {
    /// Best-effort formatting for display/debug tooling. Arrays longer than
    /// a handful of entries are elided — a 151936-entry tokenizer vocab
    /// array is metadata too, and dumping it whole is never what you want
    /// when eyeballing a model's header.
    pub fn preview(&self) -> String {
        match self {
            MetadataValue::Uint8(v) => v.to_string(),
            MetadataValue::Int8(v) => v.to_string(),
            MetadataValue::Uint16(v) => v.to_string(),
            MetadataValue::Int16(v) => v.to_string(),
            MetadataValue::Uint32(v) => v.to_string(),
            MetadataValue::Int32(v) => v.to_string(),
            MetadataValue::Float32(v) => v.to_string(),
            MetadataValue::Bool(v) => v.to_string(),
            MetadataValue::Uint64(v) => v.to_string(),
            MetadataValue::Int64(v) => v.to_string(),
            MetadataValue::Float64(v) => v.to_string(),
            MetadataValue::String(v) => {
                // Truncate by character count, not byte index — GGUF string
                // values are arbitrary UTF-8 (SentencePiece vocab entries in
                // particular are full of multi-byte glyphs like '▁'), so a
                // raw byte slice can and does land mid-character.
                if v.chars().count() > 80 {
                    let truncated: String = v.chars().take(80).collect();
                    format!("{truncated:?}...")
                } else {
                    format!("{v:?}")
                }
            }
            MetadataValue::Array(items) => {
                let n = items.len();
                let head: Vec<String> = items.iter().take(3).map(|v| v.preview()).collect();
                if n > 3 {
                    format!("[{}, ... ({} items)]", head.join(", "), n)
                } else {
                    format!("[{}]", head.join(", "))
                }
            }
        }
    }
}

/// Read one metadata value given its already-decoded type tag.
fn read_scalar(r: &mut Reader, ty: ValueType, at: usize) -> Result<MetadataValue> {
    Ok(match ty {
        ValueType::Uint8 => MetadataValue::Uint8(r.u8()?),
        ValueType::Int8 => MetadataValue::Int8(r.i8()?),
        ValueType::Uint16 => MetadataValue::Uint16(r.u16()?),
        ValueType::Int16 => MetadataValue::Int16(r.i16()?),
        ValueType::Uint32 => MetadataValue::Uint32(r.u32()?),
        ValueType::Int32 => MetadataValue::Int32(r.i32()?),
        ValueType::Float32 => MetadataValue::Float32(r.f32()?),
        ValueType::Bool => MetadataValue::Bool(r.bool()?),
        ValueType::Uint64 => MetadataValue::Uint64(r.u64()?),
        ValueType::Int64 => MetadataValue::Int64(r.i64()?),
        ValueType::Float64 => MetadataValue::Float64(r.f64()?),
        ValueType::String => MetadataValue::String(r.gguf_string()?),
        ValueType::Array => return Err(GgufError::NestedArray { at }),
    })
}

pub fn read_value(r: &mut Reader) -> Result<MetadataValue> {
    let at = r.position();
    let raw_ty = r.u32()?;
    let ty = ValueType::from_u32(raw_ty).ok_or(GgufError::UnknownValueType { raw: raw_ty, at })?;

    if ty == ValueType::Array {
        let elem_at = r.position();
        let raw_elem_ty = r.u32()?;
        let elem_ty = ValueType::from_u32(raw_elem_ty)
            .ok_or(GgufError::UnknownValueType { raw: raw_elem_ty, at: elem_at })?;
        let len = r.u64()?;
        let len = usize::try_from(len).map_err(|_| GgufError::Overflow("array length"))?;
        let mut items = Vec::with_capacity(len.min(1 << 20));
        for _ in 0..len {
            let elem_at = r.position();
            items.push(read_scalar(r, elem_ty, elem_at)?);
        }
        Ok(MetadataValue::Array(items))
    } else {
        read_scalar(r, ty, at)
    }
}
