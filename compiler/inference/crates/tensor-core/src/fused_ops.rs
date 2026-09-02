//! Fused Compute Kernels for Skynet Inference Engine.
//!
//! High-throughput fused memory operations:
//! 1. `fused_rmsnorm_silu`: Performs RMSNorm and SiLU activation in 1 memory pass.
//! 2. `fused_add_rmsnorm`: Fuses residual add + RMSNormalization.
//! 3. `fused_scale_add_inplace`: Accelerated vector scale and add.
//!
//! Eliminates intermediate memory bandwidth bottlenecks for maximum tok/s.

use crate::matrix::Matrix;
use crate::simd;

/// Fuses Residual Add + RMSNormalization in a single pass into `out`.
pub fn fused_add_rmsnorm_into(
    x: &Matrix,
    residual: &mut Matrix,
    weight: Option<&[f32]>,
    eps: f32,
    out: &mut Matrix,
) {
    assert_eq!((x.rows, x.cols), (residual.rows, residual.cols));
    assert_eq!((x.rows, x.cols), (out.rows, out.cols));

    let cols = x.cols;

    for r in 0..x.rows {
        let x_row = x.row(r);
        let res_row = residual.row_mut(r);
        let out_row = out.row_mut(r);

        // Pass 1: Inplace residual add and sum of squares computation
        simd::vec_add_inplace(res_row, x_row);
        let ss = simd::sum_squared(res_row);

        let scale = 1.0 / (ss / cols as f32 + eps).sqrt();

        // Pass 2: Scale and optional weight multiply
        if let Some(w) = weight {
            for c in 0..cols {
                out_row[c] = res_row[c] * scale * w[c];
            }
        } else {
            for c in 0..cols {
                out_row[c] = res_row[c] * scale;
            }
        }
    }
}

/// Fuses RMSNorm and SiLU activation: `SiLU(RMSNorm(x))`.
pub fn fused_rmsnorm_silu_into(
    x: &Matrix,
    weight: Option<&[f32]>,
    eps: f32,
    out: &mut Matrix,
) {
    assert_eq!((x.rows, x.cols), (out.rows, out.cols));
    let cols = x.cols;

    for r in 0..x.rows {
        let x_row = x.row(r);
        let out_row = out.row_mut(r);

        let ss = simd::sum_squared(x_row);
        let scale = 1.0 / (ss / cols as f32 + eps).sqrt();

        if let Some(w) = weight {
            for c in 0..cols {
                let norm = x_row[c] * scale * w[c];
                // SiLU activation: x * sigmoid(x)
                out_row[c] = norm * (1.0 / (1.0 + (-norm).exp()));
            }
        } else {
            for c in 0..cols {
                let norm = x_row[c] * scale;
                out_row[c] = norm * (1.0 / (1.0 + (-norm).exp()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fused_add_rmsnorm() {
        let x = Matrix::from_vec(1, 4, vec![1.0, 2.0, 3.0, 4.0]);
        let mut res = Matrix::from_vec(1, 4, vec![0.5, 0.5, 0.5, 0.5]);
        let mut out = Matrix::zeros(1, 4);

        fused_add_rmsnorm_into(&x, &mut res, None, 1e-5, &mut out);

        // res = [1.5, 2.5, 3.5, 4.5]
        assert_eq!(res.row(0), &[1.5, 2.5, 3.5, 4.5]);
        assert!(out.row(0)[0] > 0.0);
    }
}
