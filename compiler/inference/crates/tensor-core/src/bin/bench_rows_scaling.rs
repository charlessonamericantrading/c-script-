//! ¿Es `linear_quantized_into` super-lineal en el número de FILAS?
//!
//! Contexto: el perfilado con VTune (roadmap §17) localizó que el ~84% del
//! tiempo de prefill vive en `ops::linear_quantized_range`, y que ese tiempo
//! escala 8.20x cuando los tokens escalan 7x, mientras la sincronización de
//! hilos escala SUB-linealmente (5.96x). O sea: la super-linealidad del
//! prefill está DENTRO del kernel de matmul, no en el threading.
//!
//! Esto lo comprueba directamente y aislado: misma matriz de pesos, mismo
//! trabajo por fila, solo cambia cuántas filas se pasan de golpe. Si el
//! ms/fila crece con el número de filas, la super-linealidad es del kernel y
//! está localizada. Si es plano, el kernel escala bien y la causa está en otra
//! parte del forward pass (y este resultado negativo también vale).
//!
//! Sin modelo, sin GGUF, sin servidor: el coste del kernel depende solo de las
//! formas. Descarta la ronda 0 (§14.5) y alterna la DIRECCIÓN del barrido de
//! filas entre rondas (Fase 18 aplicada a un barrido de escalado).

use std::time::Instant;

use gguf::GgmlType;
use tensor_core::ops::linear_quantized_into;
use tensor_core::{Matrix, QuantizedMatrix};

/// Bloques Q8_0 sintéticos: `{ f16 escala; i8 qs[32] }` = 34 bytes por bloque.
fn synthetic_q8_0(rows: usize, cols: usize) -> Vec<u8> {
    assert_eq!(cols % 32, 0);
    let blocks = rows * (cols / 32);
    let mut raw = Vec::with_capacity(blocks * 34);
    for i in 0..blocks {
        raw.extend_from_slice(&0x3C00u16.to_le_bytes()); // f16 1.0
        for j in 0..32 {
            raw.push((((i + j) % 251) as i32 - 125) as i8 as u8);
        }
    }
    raw
}

const ROUNDS: usize = 4;

fn main() {
    // Forma del FFN gate/up de qwen2.5:0.5b — la proyección que el profiling de
    // la Fase 17 midió como dominante (~63-77% del tiempo).
    let in_dim = 896usize;
    let out_dim = 4864usize;
    let row_counts = [1usize, 64, 128, 256, 512, 896];

    let w = QuantizedMatrix::from_raw(
        out_dim,
        in_dim,
        GgmlType::Q8_0,
        synthetic_q8_0(out_dim, in_dim),
    )
    .expect("Q8_0 soportado");

    println!("forma: in={in_dim} out={out_dim} (FFN gate/up de qwen2.5:0.5b), pesos Q8_0");
    println!("filas,ronda,ms_total,ms_por_fila");

    // acc[i] = tiempos por fila validos de row_counts[i]
    let mut acc: Vec<Vec<f64>> = vec![Vec::new(); row_counts.len()];

    for round in 0..ROUNDS {
        // Alternar la dirección del barrido: si el ms/fila solo crece medido en
        // un sentido, eso es deriva térmica, no escalado real.
        let ascending = round % 2 == 0;
        let idxs: Vec<usize> = if ascending {
            (0..row_counts.len()).collect()
        } else {
            (0..row_counts.len()).rev().collect()
        };

        for &i in &idxs {
            let rows = row_counts[i];
            let x = Matrix::from_vec(
                rows,
                in_dim,
                (0..rows * in_dim)
                    .map(|k| ((k % 17) as f32 - 8.0) * 0.05)
                    .collect(),
            );
            let mut out = Matrix::zeros(rows, out_dim);

            // Menos iteraciones cuanto más grande, para que cada punto tarde
            // parecido y ninguno domine el tiempo total del benchmark. Mínimo
            // 5 fijo (auditoría 2026-08-27, hallazgo 2): `(896/rows).max(1)`
            // colapsaba a 1 sola medida sin promediar justo en rows=512 y
            // rows=896 -- los dos puntos que sostienen la conclusión "el
            // kernel no es super-lineal". El promedio entre rondas lo cubría
            // parcialmente, pero una sola llamada a Instant::now() por punto
            // está mucho más expuesta a jitter del SO que un bucle promediado.
            let iters = (896 / rows).max(5);
            let t = Instant::now();
            for _ in 0..iters {
                linear_quantized_into(&x, &w, None, &mut out);
            }
            let ms = t.elapsed().as_secs_f64() * 1000.0 / iters as f64;
            let per_row = ms / rows as f64;

            if round > 0 {
                acc[i].push(per_row);
            }
            println!(
                "{rows},{round},{ms:.3},{per_row:.5}{}",
                if round == 0 { "   [DESCARTADA]" } else { "" }
            );
        }
    }

    println!("\n=== MEDIA de rondas validas ===");
    println!("filas,ms_por_fila,vs_1_fila");
    let base = acc[0].iter().sum::<f64>() / acc[0].len() as f64;
    for (i, &rows) in row_counts.iter().enumerate() {
        let m = acc[i].iter().sum::<f64>() / acc[i].len() as f64;
        println!("{rows},{m:.5},{:.2}x", m / base);
    }
    println!(
        "\nSi ms_por_fila CRECE con las filas -> el kernel es super-lineal (causa localizada).\n\
         Si BAJA y luego se aplana -> amortiza bien y la causa esta en otra parte."
    );
}
