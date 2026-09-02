/// A tensor's element type, as stored in a GGUF `tensor_info.type` field.
///
/// This mirrors ggml's `ggml_type` enum. The numeric values are load-bearing
/// — they're read directly off disk — so this must stay in exact sync with
/// upstream ggml's ordering, not just "a list of the types we support".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum GgmlType {
    F32,
    F16,
    Q4_0,
    Q4_1,
    Q5_0,
    Q5_1,
    Q8_0,
    Q8_1,
    Q2K,
    Q3K,
    Q4K,
    Q5K,
    Q6K,
    Q8K,
    Iq2Xxs,
    Iq2Xs,
    Iq3Xxs,
    Iq1S,
    Iq4Nl,
    Iq3S,
    Iq2S,
    Iq4Xs,
    I8,
    I16,
    I32,
    I64,
    F64,
    Iq1M,
    Bf16,
    Tq10,
    Tq20,
    Mxfp4,
    /// A value we recognize as *reserved by upstream* (removed quant
    /// formats like the old Q4_2/Q4_3/Q4_0_4_4 slots) but don't implement.
    /// Kept distinct from a truly unknown value so callers can tell "this
    /// tensor uses a real but unsupported format" from "this file is
    /// corrupt or from a newer/incompatible ggml version".
    Reserved(u32),
}

impl GgmlType {
    /// Decode the raw `uint32` on disk. Returns `None` for values outside
    /// ggml's known range entirely (as opposed to `Reserved`, which is a
    /// known-but-unimplemented slot).
    pub fn from_u32(v: u32) -> Option<GgmlType> {
        use GgmlType::*;
        Some(match v {
            0 => F32,
            1 => F16,
            2 => Q4_0,
            3 => Q4_1,
            4..=5 => Reserved(v), // Q4_2 / Q4_3, removed from ggml
            6 => Q5_0,
            7 => Q5_1,
            8 => Q8_0,
            9 => Q8_1,
            10 => Q2K,
            11 => Q3K,
            12 => Q4K,
            13 => Q5K,
            14 => Q6K,
            15 => Q8K,
            16 => Iq2Xxs,
            17 => Iq2Xs,
            18 => Iq3Xxs,
            19 => Iq1S,
            20 => Iq4Nl,
            21 => Iq3S,
            22 => Iq2S,
            23 => Iq4Xs,
            24 => I8,
            25 => I16,
            26 => I32,
            27 => I64,
            28 => F64,
            29 => Iq1M,
            30 => Bf16,
            31..=33 => Reserved(v), // Q4_0_4_4 / Q4_0_4_8 / Q4_0_8_8, removed
            34 => Tq10,
            35 => Tq20,
            39 => Mxfp4,
            _ => return None,
        })
    }

    pub fn name(&self) -> &'static str {
        use GgmlType::*;
        match self {
            F32 => "F32",
            F16 => "F16",
            Q4_0 => "Q4_0",
            Q4_1 => "Q4_1",
            Q5_0 => "Q5_0",
            Q5_1 => "Q5_1",
            Q8_0 => "Q8_0",
            Q8_1 => "Q8_1",
            Q2K => "Q2_K",
            Q3K => "Q3_K",
            Q4K => "Q4_K",
            Q5K => "Q5_K",
            Q6K => "Q6_K",
            Q8K => "Q8_K",
            Iq2Xxs => "IQ2_XXS",
            Iq2Xs => "IQ2_XS",
            Iq3Xxs => "IQ3_XXS",
            Iq1S => "IQ1_S",
            Iq4Nl => "IQ4_NL",
            Iq3S => "IQ3_S",
            Iq2S => "IQ2_S",
            Iq4Xs => "IQ4_XS",
            I8 => "I8",
            I16 => "I16",
            I32 => "I32",
            I64 => "I64",
            F64 => "F64",
            Iq1M => "IQ1_M",
            Bf16 => "BF16",
            Tq10 => "TQ1_0",
            Tq20 => "TQ2_0",
            Mxfp4 => "MXFP4",
            Reserved(_) => "RESERVED",
        }
    }

    /// `(elements_per_block, bytes_per_block)` for types whose on-disk size
    /// this parser knows how to compute. `None` means "recognized type, but
    /// this crate doesn't implement its block layout yet" — Fase 0 only
    /// needs this for the formats our three actual models use (F32/F16 and
    /// the Q4_0/Q4_K/Q5_K/Q6_K/Q8_0 family); the exotic IQ*/TQ*/MXFP4 codecs
    /// are deliberately left unimplemented rather than guessed at.
    pub fn block_info(&self) -> Option<(u64, u64)> {
        use GgmlType::*;
        Some(match self {
            F32 => (1, 4),
            F16 => (1, 2),
            Bf16 => (1, 2),
            F64 => (1, 8),
            I8 => (1, 1),
            I16 => (1, 2),
            I32 => (1, 4),
            I64 => (1, 8),
            // block_q4_0 { fp16 d; u8 qs[16]; }
            Q4_0 => (32, 18),
            // block_q4_1 { fp16 d; fp16 m; u8 qs[16]; }
            Q4_1 => (32, 20),
            // block_q5_0 { fp16 d; u8 qh[4]; u8 qs[16]; }
            Q5_0 => (32, 22),
            // block_q5_1 { fp16 d; fp16 m; u8 qh[4]; u8 qs[16]; }
            Q5_1 => (32, 24),
            // block_q8_0 { fp16 d; i8 qs[32]; }
            Q8_0 => (32, 34),
            // block_q8_1 { fp16 d; fp16 s; i8 qs[32]; }
            Q8_1 => (32, 36),
            // K-quants: super-block of 256 elements.
            // block_q2_K { u8 scales[16]; u8 qs[64]; fp16 d; fp16 dmin; }
            Q2K => (256, 84),
            // block_q3_K { u8 hmask[32]; u8 qs[64]; u8 scales[12]; fp16 d; }
            Q3K => (256, 110),
            // block_q4_K { fp16 d; fp16 dmin; u8 scales[12]; u8 qs[128]; }
            Q4K => (256, 144),
            // block_q5_K { fp16 d; fp16 dmin; u8 scales[12]; u8 qh[32]; u8 qs[128]; }
            Q5K => (256, 176),
            // block_q6_K { u8 ql[128]; u8 qh[64]; i8 scales[16]; fp16 d; }
            Q6K => (256, 210),
            // block_q8_K { f32 d; i8 qs[256]; i16 bsums[16]; }
            Q8K => (256, 292),
            Iq2Xxs | Iq2Xs | Iq3Xxs | Iq1S | Iq4Nl | Iq3S | Iq2S | Iq4Xs | Iq1M | Tq10 | Tq20
            | Mxfp4 | Reserved(_) => return None,
        })
    }
}
