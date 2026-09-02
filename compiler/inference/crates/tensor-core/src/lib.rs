//! The project's own tensor math. Fase 1 laid down a dense f32 matrix type,
//! dequantization from GGUF's on-disk quantized formats, and the scalar
//! kernels needed for a decoder-only transformer forward pass (linear,
//! RMSNorm, softmax, SiLU, RoPE, causal GQA attention). Fase 2 adds
//! `QuantizedMatrix` + `ops::linear_quantized`, which keep weights in their
//! native quantized bytes instead of expanding everything to f32 up front.
//!
//! No GPU, no batching. See Fase 3+ in the project plan for those.

pub mod dequant;
pub mod f16;
pub mod fused_ops;
pub mod matrix;
pub mod ops;
pub mod qmatrix;
pub mod quant;
pub mod simd;
pub mod worker_pool;

pub use dequant::{dequantize, DequantError};
pub use f16::f16_to_f32;
pub use fused_ops::{fused_add_rmsnorm_into, fused_rmsnorm_silu_into};
pub use matrix::Matrix;
pub use qmatrix::QuantizedMatrix;
pub use worker_pool::{global_pool, WorkerPool};
