//! Cuánto más rápido es el camino de CPU REAL del motor (`linear_quantized_into`,
//! AVX2 + hilos, sobre bytes cuantizados) que el escalar naive (`linear`).
//!
//! Existe por una razón concreta: `matmul-gpu-test` compara la GPU contra
//! `linear` — el escalar naive — y su propio doc-comment lo advierte. Sin este
//! factor, sus ratios ("1.85x más rápido en lm_head") se leen como si la GPU
//! ganase al motor, cuando lo que gana es a una implementación que el motor no
//! usa desde la Fase 9. Este binario da el factor de corrección.
//!
//! Mismas formas que `matmul-gpu-test` (qwen2.5:0.5b: hidden=896, ffn=4864,
//! vocab=151936) para que los números sean directamente comparables.
//!
//! Descarta la primera ronda (regla §14.5 del roadmap: la ronda inmediatamente
//! posterior a compilar está contaminada térmicamente) y alterna el orden del
//! par en cada ronda (lección Fase 18: un ganador que se invierte al voltear el
//! orden es sesgo, no señal).

use std::time::Instant;

use gguf::GgmlType;
use tensor_core::ops::{linear, linear_quantized_into};
use tensor_core::{Matrix, QuantizedMatrix};

/// Bloques Q8_0 sintéticos: `{ f16 escala; i8 qs[32] }` = 34 bytes por bloque
/// de 32 elementos. Para un benchmark solo importan la FORMA y el formato --
/// el coste del kernel no depende de los valores -- pero se usa una escala
/// sana (f16 1.0 = 0x3C00) para no caer en denormales, que sí distorsionarían
/// el tiempo. Q8_0 es el 37.3% del peso de qwen2.5:0.5b (auditoría Fase 8).
fn synthetic_q8_0(rows: usize, cols: usize) -> Vec<u8> {
    assert_eq!(cols % 32, 0, "Q8_0 necesita in_features multiplo de 32");
    let blocks_per_row = cols / 32;
    let mut raw = Vec::with_capacity(rows * blocks_per_row * 34);
    for i in 0..rows * blocks_per_row {
        raw.extend_from_slice(&0x3C00u16.to_le_bytes()); // f16 1.0
        for j in 0..32 {
            raw.push((((i + j) % 251) as i32 - 125) as i8 as u8);
        }
    }
    raw
}

const ROUNDS: usize = 4;
const ITERS: usize = 5;

fn main() {
    // (nombre, in_dim, out_dim) -- idénticas a matmul-gpu-test.
    let cases = [
        ("o_proj-ish ", 896usize, 896usize),
        ("ffn_up-ish ", 896, 4864),
        ("lm_head-ish", 896, 151936),
    ];

    for (name, in_dim, out_dim) in cases {
        let x = Matrix::from_vec(
            1,
            in_dim,
            (0..in_dim).map(|i| ((i % 17) as f32 - 8.0) * 0.05).collect(),
        );
        let w_f32 = Matrix::from_vec(
            out_dim,
            in_dim,
            (0..out_dim * in_dim)
                .map(|i| ((i % 23) as f32 - 11.0) * 0.01)
                .collect(),
        );
        // El camino real opera sobre pesos cuantizados, no sobre f32 expandido:
        // esa es justamente una de las razones por las que es más rápido.
        let w_q = QuantizedMatrix::from_raw(
            out_dim,
            in_dim,
            GgmlType::Q8_0,
            synthetic_q8_0(out_dim, in_dim),
        )
        .expect("Q8_0 esta implementado y las formas cuadran");

        let mut out = Matrix::zeros(1, out_dim);
        let mut naive_ms = Vec::new();
        let mut real_ms = Vec::new();

        for round in 0..ROUNDS {
            // Alternar el orden del par por ronda.
            let naive_first = round % 2 == 0;

            let run_naive = || {
                let t = Instant::now();
                for _ in 0..ITERS {
                    let _ = linear(&x, &w_f32, None);
                }
                t.elapsed().as_secs_f64() * 1000.0 / ITERS as f64
            };
            let run_real = |out: &mut Matrix| {
                let t = Instant::now();
                for _ in 0..ITERS {
                    linear_quantized_into(&x, &w_q, None, out);
                }
                t.elapsed().as_secs_f64() * 1000.0 / ITERS as f64
            };

            let (n, r) = if naive_first {
                let n = run_naive();
                let r = run_real(&mut out);
                (n, r)
            } else {
                let r = run_real(&mut out);
                let n = run_naive();
                (n, r)
            };

            // Ronda 0 descartada: contaminación térmica post-compilación.
            if round > 0 {
                naive_ms.push(n);
                real_ms.push(r);
            }
            println!(
                "  [{name}] ronda {round}{}: naive={n:8.3} ms  real={r:8.3} ms  ratio={:.1}x{}",
                if naive_first { " (naive->real)" } else { " (real->naive)" },
                n / r,
                if round == 0 { "   [DESCARTADA]" } else { "" },
            );
        }

        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        let (n, r) = (mean(&naive_ms), mean(&real_ms));
        println!(
            "  [{name}] MEDIA (rondas validas): naive={n:8.3} ms  real={r:8.3} ms  \
             -> el camino REAL es {:.1}x mas rapido que el naive\n",
            n / r
        );
    }
}
