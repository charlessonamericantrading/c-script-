use crate::error::{GgufError, Result};
use crate::reader::Reader;
use crate::GGUF_MAGIC;

#[derive(Debug, Clone, Copy)]
pub struct GgufHeader {
    pub version: u32,
    pub tensor_count: u64,
    pub metadata_kv_count: u64,
}

pub(crate) fn read_header(r: &mut Reader) -> Result<GgufHeader> {
    let magic = r.u32()?;
    if magic != GGUF_MAGIC {
        return Err(GgufError::BadMagic { found: magic });
    }

    let version = r.u32()?;
    if version != 2 && version != 3 {
        return Err(GgufError::UnsupportedVersion { found: version });
    }

    // v1 stored these as uint32; v2 widened both to uint64 and every file in
    // the wild today is v2/v3, so this parser only reads the wide form.
    let tensor_count = r.u64()?;
    let metadata_kv_count = r.u64()?;

    Ok(GgufHeader { version, tensor_count, metadata_kv_count })
}
