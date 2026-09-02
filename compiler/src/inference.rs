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

/// GRAMMAR.md §3.234: la ruta real del GGUF de un modelo declarado en
/// `ai { }`. Una spec con separador de ruta o extensión `.gguf` es un
/// FICHERO (relativo a `models_dir` si se dio y la ruta es relativa);
/// cualquier otra es un alias de Ollama (`nombre:tag`), buscado primero
/// como `<models_dir>/<nombre-tag>.gguf` y después en el almacén de Ollama
/// en disco (`OLLAMA_MODELS` o `$HOME/.ollama/models`). Solo comprueba que el
/// archivo exista: cargarlo es cosa del primer uso (`ServerState::get_or_load`).
pub fn resolve_model_spec(spec: &str, models_dir: Option<&std::path::Path>) -> Result<std::path::PathBuf, String> {
    let looks_like_path = spec.contains('/') || spec.contains('\\') || spec.ends_with(".gguf");
    if looks_like_path {
        let p = std::path::PathBuf::from(spec);
        let p = if p.is_relative() { models_dir.map(|d| d.join(&p)).unwrap_or(p) } else { p };
        return if p.is_file() { Ok(p) } else { Err(format!("no existe el archivo '{}'", p.display())) };
    }
    if let Some(dir) = models_dir {
        let candidate = dir.join(format!("{}.gguf", spec.replace(':', "-")));
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    resolve_blob_path(spec).map_err(|e| e.to_string())
}

/// Resuelve TODOS los modelos declarados y devuelve `(alias, ruta)` listos
/// para `ServerState::new`. Un error lista TODOS los que faltan, no solo el
/// primero -- mismo criterio de reporte completo que `linkc doctor`.
pub fn resolve_declared_models(
    models: &[(String, String)],
    models_dir: Option<&std::path::Path>,
) -> Result<Vec<(String, String)>, String> {
    let mut known = Vec::with_capacity(models.len());
    let mut missing = Vec::new();
    for (alias, spec) in models {
        match resolve_model_spec(spec, models_dir) {
            Ok(path) => known.push((alias.clone(), path.to_string_lossy().to_string())),
            Err(e) => missing.push(format!("  - {alias} (\"{spec}\"): {e}")),
        }
    }
    if missing.is_empty() {
        Ok(known)
    } else {
        Err(format!(
            "modelos de 'ai {{ }}' no encontrados en esta máquina:\n{}\n(GRAMMAR.md §3.234: una spec es un nombre de Ollama ya descargado -- OLLAMA_MODELS o $HOME/.ollama/models -- o una ruta a un .gguf, relativa a --models-dir/LINK_MODELS_DIR)",
            missing.join("\n")
        ))
    }
}

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

    /// GRAMMAR.md §3.234: una ruta relativa se resuelve contra `models_dir`,
    /// un alias de Ollama contra `<models_dir>/<nombre-tag>.gguf` antes que
    /// contra el almacén de Ollama, y el error lista TODOS los que faltan.
    #[test]
    fn declared_models_resolve_to_files_and_missing_ones_are_all_listed() {
        let dir = std::env::temp_dir().join(format!("linkc-ai-resolve-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("tiny.gguf"), b"not a real model").unwrap();
        std::fs::write(dir.join("qwen2.5-0.5b.gguf"), b"not a real model").unwrap();

        assert!(resolve_model_spec("tiny.gguf", Some(&dir)).unwrap().ends_with("tiny.gguf"));
        assert!(resolve_model_spec("./tiny.gguf", Some(&dir)).is_ok());
        assert!(resolve_model_spec("qwen2.5:0.5b", Some(&dir)).unwrap().ends_with("qwen2.5-0.5b.gguf"));
        assert!(resolve_model_spec("tiny.gguf", None).is_err(), "sin models_dir una ruta relativa se busca tal cual");

        let err = resolve_declared_models(
            &[
                ("ok".to_string(), "tiny.gguf".to_string()),
                ("falta1".to_string(), "nada.gguf".to_string()),
                ("falta2".to_string(), "modelo-inexistente:latest".to_string()),
            ],
            Some(&dir),
        )
        .unwrap_err();
        assert!(err.contains("falta1") && err.contains("falta2") && !err.contains("- ok"), "{err}");
        assert!(err.contains("§3.234"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// La plomería entera enlaza: un `ServerState` vacío, la resolución de un
    /// alias de Ollama que no existe (error limpio, no panic), y la línea de
    /// descripción. Sin ningún modelo en disco.
    #[test]
    fn the_embedded_engine_links_and_reports_itself() {
        let state = ServerState::new(Vec::new(), None);
        assert!(state.tags_json().is_empty());
        assert!(matches!(state.get_or_load("nadie"), Err(ModelLookupError::NotFound(_))));
        assert!(resolve_blob_path("modelo-que-no-existe:latest").is_err());
        assert!(describe().starts_with("inference: on ("), "{}", describe());
    }
}
