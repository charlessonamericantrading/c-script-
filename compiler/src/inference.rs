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

/// GRAMMAR.md §3.235/§3.236: el modelo residente para `alias`, cargándolo si
/// es su primer uso -- con los dos errores que un `.link` puede ver: alias
/// no declarado (con la lista de los declarados) y GGUF que no carga.
/// Separado de `generate_with` para que un `stream` lo compruebe ANTES de
/// mandar los headers 200.
pub fn ensure_loaded(state: &ServerState, alias: &str) -> Result<std::sync::Arc<ModelEntry>, String> {
    state.get_or_load(alias).map_err(|e| match e {
        ModelLookupError::NotFound(_) => {
            let declared: Vec<String> = state
                .tags_json()
                .iter()
                .filter_map(|t| t.get("name").and_then(|n| n.as_str()).map(str::to_string))
                .collect();
            format!("ai: el alias '{alias}' no está declarado en 'ai {{ }}' -- declarados: [{}]", declared.join(", "))
        }
        ModelLookupError::LoadFailed(msg) => format!("ai: no se pudo cargar el modelo '{alias}': {msg}"),
    })
}

/// GRAMMAR.md §3.235: qué se le pide al modelo. `Raw` es el prompt tal
/// cual (sin chat template, como `/api/generate` con `raw: true`); `Chat`
/// pasa por el chat template propio de cada arquitectura
/// (`ModelTokenizer::render_prompt_ids`).
pub enum AiRequest {
    Raw(String),
    Chat(Vec<ChatMessage>),
}

/// GRAMMAR.md §3.235: el resultado de una generación, con las cuentas que
/// `/metrics` (Eje G ítem 8) va a exportar.
#[derive(Debug)]
pub struct AiOutput {
    pub text: String,
    pub prompt_tokens: usize,
    pub generated_tokens: usize,
    pub done_reason: &'static str,
    pub elapsed: std::time::Duration,
    /// GRAMMAR.md §3.237: tiempo del prefill (el prompt entero de una vez).
    pub prefill: std::time::Duration,
    /// GRAMMAR.md §3.237: tiempo del decode (un token por paso).
    pub decode: std::time::Duration,
    /// GRAMMAR.md §3.237: ¿el prompt reusó KV de un prefijo reciente?
    pub prefix_hit: bool,
}

/// GRAMMAR.md §3.235: el bucle de generación del motor (`routes.rs::
/// handle_generate` de origen), sin el HTTP de por medio: tokenizar,
/// reusar el prefix cache si el prompt comparte prefijo con uno reciente,
/// prefill, y decodificar greedy hasta EOS, `max_tokens` o `timeout`. Un
/// timeout es un ERROR (no un texto a medias devuelto en silencio): el
/// caller decide si reintenta con menos tokens o con otro modelo.
pub fn generate(
    state: &ServerState,
    alias: &str,
    request: AiRequest,
    max_tokens: i64,
    timeout: std::time::Duration,
) -> Result<AiOutput, String> {
    generate_with(state, alias, request, max_tokens, timeout, &mut |_| Ok(()))
}

/// GRAMMAR.md §3.236: como `generate`, pero `on_token` recibe cada token
/// decodificado en cuanto sale del motor -- es lo que `stream -> AiToken`
/// escribe por SSE. Si `on_token` devuelve un error (el cliente se fue), la
/// generación para ahí mismo: no se gasta CPU en tokens que nadie va a leer.
pub fn generate_with(
    state: &ServerState,
    alias: &str,
    request: AiRequest,
    max_tokens: i64,
    timeout: std::time::Duration,
    on_token: &mut dyn FnMut(&str) -> Result<(), String>,
) -> Result<AiOutput, String> {
    use model_core::{argmax, KvCache};
    if max_tokens <= 0 {
        return Err(format!("ai: maxTokens tiene que ser > 0, se recibió {max_tokens}"));
    }
    let entry = ensure_loaded(state, alias)?;
    let prompt_ids = match &request {
        AiRequest::Raw(p) => entry.tokenizer.encode(p),
        AiRequest::Chat(messages) => entry.tokenizer.render_prompt_ids(messages),
    };
    if prompt_ids.is_empty() {
        return Err("ai: el prompt está vacío".to_string());
    }
    let started = std::time::Instant::now();
    let (matched, cached) = state.prefix_cache.find_longest_prefix(alias, &prompt_ids);
    let safe = model_core::prefix_cache::safe_reuse_len(matched, prompt_ids.len());
    let prefix_hit = safe > 0 && cached.is_some();
    let (mut cache, mut logits) = match cached {
        Some(mut cache) if safe > 0 => {
            cache.truncate(safe);
            let logits = entry.model.forward_step(&mut cache, &prompt_ids[safe..]);
            (cache, logits)
        }
        _ => {
            let mut cache = KvCache::new(&entry.model.cache_shape());
            let logits = entry.model.forward_step(&mut cache, &prompt_ids);
            state.prefix_cache.insert_prefix(alias, prompt_ids.clone(), cache.clone());
            (cache, logits)
        }
    };
    let prefill = started.elapsed();
    let decode_started = std::time::Instant::now();
    let max = max_tokens as usize;
    let mut generated: Vec<u32> = Vec::new();
    let mut done_reason = "length";
    for step in 0..max {
        if started.elapsed() > timeout {
            return Err(format!(
                "ai: timeout tras {:.1}s con {} token(s) generados -- subí --ai-timeout/LINK_AI_TIMEOUT o bajá maxTokens (GRAMMAR.md §3.235)",
                started.elapsed().as_secs_f64(),
                generated.len()
            ));
        }
        let next = argmax(&logits);
        if Some(next) == entry.tokenizer.eos_token_id() {
            done_reason = "stop";
            break;
        }
        generated.push(next);
        on_token(&entry.tokenizer.decode(&[next]))?;
        if step + 1 == max {
            break;
        }
        logits = entry.model.forward_step(&mut cache, &[next]);
    }
    Ok(AiOutput {
        text: entry.tokenizer.decode(&generated),
        prompt_tokens: prompt_ids.len(),
        generated_tokens: generated.len(),
        done_reason,
        elapsed: started.elapsed(),
        prefill,
        decode: decode_started.elapsed(),
        prefix_hit,
    })
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
    /// GRAMMAR.md §3.235: los errores que no necesitan un modelo en disco.
    #[test]
    fn generate_rejects_a_bad_token_budget_and_an_undeclared_alias_with_the_declared_list() {
        let state = ServerState::new(vec![("router".to_string(), "/no/existe.gguf".to_string())], None);
        let err = generate(&state, "router", AiRequest::Raw("hola".into()), 0, std::time::Duration::from_secs(1)).unwrap_err();
        assert!(err.contains("maxTokens"), "{err}");
        let err = generate(&state, "nadie", AiRequest::Raw("hola".into()), 8, std::time::Duration::from_secs(1)).unwrap_err();
        assert!(err.contains("'nadie'") && err.contains("[router]"), "{err}");
        let err = generate(&state, "router", AiRequest::Raw("hola".into()), 8, std::time::Duration::from_secs(1)).unwrap_err();
        assert!(err.contains("no se pudo cargar"), "{err}");
    }

    #[test]
    fn the_embedded_engine_links_and_reports_itself() {
        let state = ServerState::new(Vec::new(), None);
        assert!(state.tags_json().is_empty());
        assert!(matches!(state.get_or_load("nadie"), Err(ModelLookupError::NotFound(_))));
        assert!(resolve_blob_path("modelo-que-no-existe:latest").is_err());
        assert!(describe().starts_with("inference: on ("), "{}", describe());
    }
}
