use std::collections::HashMap;

use crate::error::{GgufError, Result};
use crate::header::{read_header, GgufHeader};
use crate::reader::Reader;
use crate::tensor_info::{read_tensor_info, TensorInfo};
use crate::value::{read_value, MetadataValue};

const DEFAULT_ALIGNMENT: u64 = 32;

#[derive(Debug)]
pub struct GgufFile {
    pub header: GgufHeader,
    /// Insertion order preserved for display purposes even though lookups
    /// go through the map — see `metadata_in_order`.
    pub metadata: HashMap<String, MetadataValue>,
    metadata_order: Vec<String>,
    pub tensors: Vec<TensorInfo>,
    /// `general.alignment` if present in metadata, else the GGUF default of 32.
    pub alignment: u64,
    /// Absolute byte offset (from the start of the file) where the
    /// tensor-data section begins. Every `TensorInfo::offset` is relative
    /// to this.
    pub tensor_data_offset: u64,
}

impl GgufFile {
    pub fn parse(bytes: &[u8]) -> Result<GgufFile> {
        let mut r = Reader::new(bytes);

        let header = read_header(&mut r)?;

        let mut metadata = HashMap::with_capacity(header.metadata_kv_count as usize);
        let mut metadata_order = Vec::with_capacity(header.metadata_kv_count as usize);
        for _ in 0..header.metadata_kv_count {
            let key = r.gguf_string()?;
            let value = read_value(&mut r)?;
            metadata_order.push(key.clone());
            metadata.insert(key, value);
        }

        let alignment = match metadata.get("general.alignment") {
            Some(MetadataValue::Uint32(v)) => *v as u64,
            Some(MetadataValue::Int32(v)) if *v > 0 => *v as u64,
            _ => DEFAULT_ALIGNMENT,
        };
        if alignment == 0 || (alignment & (alignment - 1)) != 0 {
            return Err(GgufError::Overflow("general.alignment (must be a nonzero power of two)"));
        }

        let mut tensors = Vec::with_capacity(header.tensor_count as usize);
        for _ in 0..header.tensor_count {
            tensors.push(read_tensor_info(&mut r)?);
        }

        // Tensor data starts at the next multiple of `alignment` after the
        // tensor-info table, padded with filler bytes we don't need to read.
        let after_infos = r.position() as u64;
        let tensor_data_offset = after_infos.div_ceil(alignment) * alignment;

        Ok(GgufFile { header, metadata, metadata_order, tensors, alignment, tensor_data_offset })
    }

    /// Metadata keys in the order they appeared on disk — nicer for
    /// human-facing dumps than HashMap's arbitrary iteration order.
    pub fn metadata_in_order(&self) -> impl Iterator<Item = (&str, &MetadataValue)> {
        self.metadata_order.iter().map(move |k| (k.as_str(), &self.metadata[k]))
    }

    pub fn architecture(&self) -> Option<&str> {
        match self.metadata.get("general.architecture") {
            Some(MetadataValue::String(s)) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Absolute file offset of a tensor's data (as opposed to
    /// `TensorInfo::offset`, which is relative to `tensor_data_offset`).
    pub fn tensor_absolute_offset(&self, t: &TensorInfo) -> Result<u64> {
        self.tensor_data_offset.checked_add(t.offset).ok_or(GgufError::Overflow("tensor absolute offset"))
    }
}
