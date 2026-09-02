use crate::error::Result;
use crate::reader::Reader;
use crate::types::GgmlType;

/// One entry from the tensor-info table — everything needed to locate and
/// interpret a tensor's bytes, but not the bytes themselves.
#[derive(Debug, Clone)]
pub struct TensorInfo {
    pub name: String,
    /// GGUF stores dimensions fastest-varying-first (ggml convention), so
    /// `dimensions[0]` is the innermost/contiguous axis — the opposite of
    /// the row-major "outermost first" convention this code's eventual
    /// callers (the graph builder) will assume. Left as-is here; reordering
    /// is the graph builder's job, not the parser's.
    pub dimensions: Vec<u64>,
    /// `None` when the on-disk type tag isn't one of ggml's known values at
    /// all (as opposed to `GgmlType::Reserved`, which *is* known but unused).
    pub ggml_type: Option<GgmlType>,
    /// Byte offset relative to the start of the tensor-data section (i.e.
    /// *not* an absolute file offset — see `GgufFile::tensor_data_offset`).
    pub offset: u64,
}

impl TensorInfo {
    pub fn n_elements(&self) -> Option<u64> {
        self.dimensions.iter().try_fold(1u64, |acc, &d| acc.checked_mul(d))
    }

    /// Size in bytes this tensor occupies in the tensor-data section, if
    /// this parser knows the on-disk layout for its type.
    pub fn size_bytes(&self) -> Option<u64> {
        let ty = self.ggml_type?;
        let (block_elems, block_bytes) = ty.block_info()?;
        let n = self.n_elements()?;
        // Quantized tensors are stored in whole blocks; ggml requires each
        // dimension that carries a block-quantized axis to be a multiple of
        // the block size, so integer division here should always be exact
        // for a well-formed file. Round up defensively rather than assume.
        let n_blocks = n.div_ceil(block_elems);
        n_blocks.checked_mul(block_bytes)
    }
}

pub(crate) fn read_tensor_info(r: &mut Reader) -> Result<TensorInfo> {
    let name = r.gguf_string()?;
    let n_dims = r.u32()?;
    let mut dimensions = Vec::with_capacity(n_dims as usize);
    for _ in 0..n_dims {
        dimensions.push(r.u64()?);
    }
    // An unrecognized type tag is not fatal by itself: a newer ggml might
    // define types this parser doesn't know yet, and a caller may still
    // want the tensor's name/shape/offset. `ggml_type` is `None` in that
    // case; `size_bytes()` then simply returns `None` downstream instead of
    // guessing at a byte layout.
    let raw_type = r.u32()?;
    let ggml_type = GgmlType::from_u32(raw_type);
    let offset = r.u64()?;

    Ok(TensorInfo { name, dimensions, ggml_type, offset })
}
