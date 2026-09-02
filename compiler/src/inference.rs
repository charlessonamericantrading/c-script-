//! GRAMMAR.md §3.233 (PLAN.md §9.20 Eje G ítem 1): el motor de inferencia de
//! Skynet -- Rust puro, sin dependencias, vendorizado en `inference/crates/`
//! (ver `inference/VENDORED.md`) -- embebido en el binario de `linkc` detrás
//! del feature `inference`. Esta ronda es SOLO la plomería: compila, se
//! enlaza, y `linkc --version`/`linkc doctor` lo reportan. Los builtins
//! `ai.*` (Eje G ítems 2-6) se construyen encima de estas re-exportaciones,
//! en rondas propias.
//!
//! Por qué embeber y no reescribir: un `.link` no puede (ni debe poder) leer
//! un GGUF de varios GB ni hacer un matmul cuantizado con SIMD; el motor ya
//! está escrito en el mismo lenguaje que el compilador, así que entra como
//! los crates de `pdf.build`/`excel.build`/bcrypt: un builtin curado, con el
//! runtime como único dueño de ficheros e hilos.

pub use inference_server::loader::load_model;
pub use inference_server::ollama_resolve::resolve_blob_path;
pub use inference_server::routes::{ModelEntry, ModelLookupError, ServerState};
pub use model_core::{ChatMessage, LanguageModel, ModelTokenizer};

/// ¿La CPU tiene AVX2 y FMA? El motor tiene un camino escalar de respaldo
/// para cualquier arquitectura, pero sin estas dos instrucciones la
/// inferencia real es inservible (decenas de veces más lenta) -- por eso
/// `linkc doctor` lo dice explícitamente, en vez de dejar que se descubra en
/// el primer request.
pub fn cpu_has_avx2_fma() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        std::is_x86_feature_detected!("avx2") && std::is_x86_feature_detected!("fma")
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

/// Una línea para `--version` y `doctor`: el motor está, y con qué kernels
/// va a correr en ESTA máquina.
pub fn describe() -> String {
    format!("inference: on ({})", if cpu_has_avx2_fma() { "avx2+fma" } else { "scalar" })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// La plomería entera enlaza: un `ServerState` vacío, la resolución de un
    /// alias de Ollama que no existe (error limpio, no panic), y la línea de
    /// descripción. Sin ningún modelo en disco -- eso es para las rondas
    /// siguientes, contra un GGUF real.
    #[test]
    fn the_embedded_engine_links_and_reports_itself() {
        let state = ServerState::new(Vec::new(), None);
        assert!(state.tags_json().is_empty());
        assert!(matches!(state.get_or_load("nadie"), Err(ModelLookupError::NotFound(_))));
        assert!(resolve_blob_path("modelo-que-no-existe:latest").is_err());
        assert!(describe().starts_with("inference: on ("), "{}", describe());
    }
}
