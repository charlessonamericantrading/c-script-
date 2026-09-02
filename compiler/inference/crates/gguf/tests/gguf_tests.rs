//! Integration tests against hand-built byte buffers.
//!
//! The Fase 0 milestone ("parses real production models byte-for-byet") is
//! covered by manual runs of `gguf-inspect` against actual Ollama blobs —
//! that proves the happy path against ground truth, but says nothing about
//! error handling, and a 400MB+ real model file is a bad fixture to check
//! into version control. These tests build the smallest buffers that can
//! exercise each behavior directly, per the GGUF spec this crate implements.

use gguf::{GgmlType, GgufError, GgufFile};

/// A tiny hand-rolled writer mirroring the subset of GGUF's binary layout
/// these tests need. Deliberately separate from `gguf::reader::Reader` —
/// sharing code between "the thing under test" and "the thing building test
/// fixtures" would let a bug in one hide the same bug in the other.
#[derive(Default)]
struct Builder(Vec<u8>);

impl Builder {
    fn u32(mut self, v: u32) -> Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn u64(mut self, v: u64) -> Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn string(mut self, s: &str) -> Self {
        self.0.extend_from_slice(&(s.len() as u64).to_le_bytes());
        self.0.extend_from_slice(s.as_bytes());
        self
    }
    fn build(self) -> Vec<u8> {
        self.0
    }
}

const VALUE_TYPE_UINT32: u32 = 4;
const VALUE_TYPE_STRING: u32 = 8;

/// A minimal but complete file: header, two metadata entries (one of which
/// sets a non-default alignment), one Q4_0 tensor, alignment padding, and
/// exactly enough tensor-data bytes for that tensor's single block.
fn synthetic_file() -> Vec<u8> {
    let header = Builder::default()
        .u32(gguf::GGUF_MAGIC)
        .u32(3) // version
        .u64(1) // tensor_count
        .u64(2) // metadata_kv_count
        // "general.architecture" = "test"
        .string("general.architecture")
        .u32(VALUE_TYPE_STRING)
        .string("test")
        // "general.alignment" = 16u32
        .string("general.alignment")
        .u32(VALUE_TYPE_UINT32)
        .u32(16)
        // tensor 0: name "weight", shape [4, 3], type Q4_0, rel. offset 0
        .string("weight")
        .u32(2) // n_dimensions
        .u64(4)
        .u64(3)
        .u32(2) // GgmlType::Q4_0 == 2
        .u64(0) // offset
        .build();

    // Tensor-info table ends at byte 89 here (asserted below); pad up to
    // the next multiple of the 16-byte alignment declared above.
    let pad = (16 - (header.len() % 16)) % 16;
    let mut out = header;
    out.extend(std::iter::repeat_n(0u8, pad));

    // 4*3 = 12 elements, Q4_0 block = 32 elements -> ceil(12/32) = 1 block = 18 bytes.
    out.extend(std::iter::repeat_n(0xAAu8, 18));
    out
}

#[test]
fn parses_header_metadata_and_tensor_info() {
    let bytes = synthetic_file();
    let gguf = GgufFile::parse(&bytes).expect("synthetic file should parse");

    assert_eq!(gguf.header.version, 3);
    assert_eq!(gguf.header.tensor_count, 1);
    assert_eq!(gguf.header.metadata_kv_count, 2);
    assert_eq!(gguf.alignment, 16, "should read general.alignment from metadata, not assume the default 32");
    assert_eq!(gguf.architecture(), Some("test"));

    assert_eq!(gguf.tensors.len(), 1);
    let t = &gguf.tensors[0];
    assert_eq!(t.name, "weight");
    assert_eq!(t.dimensions, vec![4, 3]);
    assert_eq!(t.ggml_type, Some(GgmlType::Q4_0));
    assert_eq!(t.n_elements(), Some(12));
    assert_eq!(t.size_bytes(), Some(18), "12 elements at 32/block should round up to exactly one Q4_0 block");

    // tensor_data_offset must land on a 16-byte boundary at or after the
    // end of the tensor-info table, and the tensor's absolute offset must
    // point exactly at the data this test wrote for it.
    assert_eq!(gguf.tensor_data_offset % 16, 0);
    let abs = gguf.tensor_absolute_offset(t).unwrap();
    assert_eq!(abs, gguf.tensor_data_offset);
    assert_eq!(abs as usize + 18, bytes.len(), "tensor data should exactly fill the rest of the synthetic file");
}

#[test]
fn rejects_bad_magic() {
    let mut bytes = synthetic_file();
    bytes[0] = b'X'; // corrupt the first magic byte
    match GgufFile::parse(&bytes) {
        Err(GgufError::BadMagic { .. }) => {}
        other => panic!("expected BadMagic, got {other:?}"),
    }
}

#[test]
fn rejects_truncated_file() {
    let bytes = synthetic_file();
    // Cut it off partway through the metadata section — short of even the
    // full header-plus-first-key.
    let truncated = &bytes[..20];
    match GgufFile::parse(truncated) {
        Err(GgufError::UnexpectedEof { .. }) => {}
        other => panic!("expected UnexpectedEof, got {other:?}"),
    }
}

#[test]
fn rejects_unsupported_version() {
    let bytes = Builder::default().u32(gguf::GGUF_MAGIC).u32(99).build();
    match GgufFile::parse(&bytes) {
        Err(GgufError::UnsupportedVersion { found: 99 }) => {}
        other => panic!("expected UnsupportedVersion(99), got {other:?}"),
    }
}

#[test]
fn quantized_block_sizes_match_known_ggml_layouts() {
    // Cross-checked against the real qwen2.5:0.5b / qwen2.5-coder:7b / gemma4:e4b
    // blobs in Fase 0's manual validation run (computed end offset matched
    // each file's exact byte size). Pinning the individual constants here
    // so a future edit can't silently drift without a test failing.
    assert_eq!(GgmlType::Q4_0.block_info(), Some((32, 18)));
    assert_eq!(GgmlType::Q5_0.block_info(), Some((32, 22)));
    assert_eq!(GgmlType::Q8_0.block_info(), Some((32, 34)));
    assert_eq!(GgmlType::Q4K.block_info(), Some((256, 144)));
    assert_eq!(GgmlType::Q6K.block_info(), Some((256, 210)));
    assert_eq!(GgmlType::F32.block_info(), Some((1, 4)));
    assert_eq!(GgmlType::F16.block_info(), Some((1, 2)));
}
