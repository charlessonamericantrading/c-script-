//! Resolves an Ollama-style model name (`"qwen2.5:0.5b"`) to the actual
//! GGUF blob path on disk, by reading Ollama's own manifest — the same
//! content-addressable blob+manifest layout researched in Fase 0 (a plain
//! OCI-image-shaped JSON manifest under
//! `~/.ollama/models/manifests/registry.ollama.ai/library/<name>/<tag>`,
//! pointing at a `sha256-<hex>`-named blob under `~/.ollama/models/blobs/`).
//!
//! This doesn't manage models (pull/create/delete) — it only *reads* what
//! Ollama already downloaded, so `--model qwen2.5:0.5b` doesn't require
//! hand-typing a blob hash path. Real model management (F6) is separate,
//! larger scope, not attempted here.

use std::path::PathBuf;

use crate::json::{self, Json};

#[derive(Debug)]
pub struct ResolveError(pub String);
impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for ResolveError {}

fn ollama_home() -> Result<PathBuf, ResolveError> {
    // Ollama itself resolves its data dir the same way: $OLLAMA_MODELS if
    // set, else <home>/.ollama/models. No XDG fallback needed here — this
    // project only runs on the one Windows machine it was built on.
    if let Ok(dir) = std::env::var("OLLAMA_MODELS") {
        return Ok(PathBuf::from(dir));
    }
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).map_err(|_| ResolveError("neither OLLAMA_MODELS nor USERPROFILE/HOME is set".to_string()))?;
    Ok(PathBuf::from(home).join(".ollama").join("models"))
}

/// Splits `"qwen2.5:0.5b"` into `("qwen2.5", "0.5b")`, defaulting the tag to
/// `"latest"` when absent (`"qwen2.5"` alone), matching Ollama's own
/// default-tag convention.
fn split_name_tag(spec: &str) -> (&str, &str) {
    match spec.split_once(':') {
        Some((name, tag)) => (name, tag),
        None => (spec, "latest"),
    }
}

/// Resolves `spec` (e.g. `"qwen2.5:0.5b"`) to the absolute path of its GGUF
/// weight blob, by reading Ollama's manifest for that name/tag.
pub fn resolve_blob_path(spec: &str) -> Result<PathBuf, ResolveError> {
    let models_dir = ollama_home()?;
    let (name, tag) = split_name_tag(spec);
    let manifest_path = models_dir.join("manifests").join("registry.ollama.ai").join("library").join(name).join(tag);

    let manifest_text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| ResolveError(format!("could not read manifest {}: {e} (is '{spec}' pulled in Ollama?)", manifest_path.display())))?;
    let manifest = json::parse(&manifest_text).map_err(|e| ResolveError(format!("manifest {} is not valid JSON: {e}", manifest_path.display())))?;

    let layers = manifest.get("layers").ok_or_else(|| ResolveError(format!("manifest {} has no \"layers\" field", manifest_path.display())))?;
    let Json::Array(layers) = layers else {
        return Err(ResolveError(format!("manifest {} \"layers\" is not an array", manifest_path.display())));
    };

    let model_layer = layers
        .iter()
        .find(|l| l.get("mediaType").and_then(Json::as_str) == Some("application/vnd.ollama.image.model"))
        .ok_or_else(|| ResolveError(format!("manifest {} has no model-weight layer", manifest_path.display())))?;

    let digest = model_layer
        .get("digest")
        .and_then(Json::as_str)
        .ok_or_else(|| ResolveError(format!("model layer in {} has no digest", manifest_path.display())))?;

    // Digests are "sha256:<hex>"; blob filenames use a dash instead of the
    // colon ("sha256-<hex>") — the same substitution seen throughout this
    // project's manual blob lookups since Fase 0.
    let blob_name = digest.replace(':', "-");
    let blob_path = models_dir.join("blobs").join(&blob_name);
    if !blob_path.is_file() {
        return Err(ResolveError(format!("blob {} referenced by manifest does not exist on disk", blob_path.display())));
    }
    Ok(blob_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_name_and_tag() {
        assert_eq!(split_name_tag("qwen2.5:0.5b"), ("qwen2.5", "0.5b"));
        assert_eq!(split_name_tag("qwen2.5"), ("qwen2.5", "latest"));
        assert_eq!(split_name_tag("qwen2.5-coder:7b"), ("qwen2.5-coder", "7b"));
    }
}
