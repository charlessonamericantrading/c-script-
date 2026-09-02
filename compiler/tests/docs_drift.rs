//! Deriva mecánica de la documentación para agentes (PLAN.md §9.18 Eje C
//! ítem 2, GRAMMAR.md §3.218).
//!
//! `docs_examples.rs` garantiza que el CÓDIGO publicado compila. Este test
//! cubre la mitad que ese no puede: la PROSA que un agente lee para decidir
//! qué comando correr o qué sección de GRAMMAR.md abrir. Evidencia real que
//! motivó esto (auditoría del 02/09/2026, todo encontrado con el binario
//! real, no inferido): `README.md`/`README.es.md`/`llms.txt` seguían
//! diciendo que `--trust-proxy` toma el PRIMER `X-Forwarded-For` (v1.170.0
//! lo cambió al ÚLTIMO, por seguridad, §3.211); `README.md`/`llms.txt`
//! decían que `link-lang` "no está en npm" (publicado el 30/08/2026,
//! PLAN.md §8.1.1); y `linkc --help` omitía tres flags reales
//! (`--template`, `--port-registry`, `--service-api-key-exempt`) que la
//! documentación SÍ nombraba -- un agente que confía en `--help` como
//! fuente de verdad los daría por inexistentes.
//!
//! Qué verifica, mecánicamente (lo único que un test puede verificar sin
//! entender el texto):
//! 1. Toda sección `§3.N` citada en un archivo que lee un agente EXISTE en
//!    GRAMMAR.md -- una cita a una sección renumerada o inexistente manda
//!    al lector a la nada.
//! 2. Todo `--flag` que aparece en una línea que menciona `linkc` está en
//!    la salida real de `linkc --help` del binario que este test compila --
//!    en las dos direcciones: un flag inventado por la documentación falla
//!    acá, y un flag real que `--help` olvidó listar también (ese fue el
//!    caso de los tres de arriba).
//!
//! Lo que NO puede verificar, y por eso no lo intenta: deriva SEMÁNTICA
//! ("primer" vs. "último"). Esa se arregla leyendo, y la lista de arriba
//! queda como registro de que pasa de verdad.

use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Mismos archivos que `docs_examples.rs` -- los que un agente (o una
/// herramienta que lo alimenta) lee de verdad. Si agregás documentación
/// nueva, sumala en los dos.
const DOCS: &[&str] = &[
    "README.md",
    "README.es.md",
    "llms.txt",
    "llms-full.txt",
    ".cursorrules",
    ".cursor/rules/c-script.mdc",
    ".github/copilot-instructions.md",
    ".windsurfrules",
    "docs/language-reference.md",
    "docs/architecture.md",
    "docs/routing.md",
    "docs/sqlite-vs-postgres.md",
    "docs/multi-service-deployment.md",
    "docs/incremental-adoption.md",
    "docs/consuming-services.md",
    "docs/deploying-from-git.md",
    "CONTRIBUTING.md",
    "AGENTS.md",
    "CLAUDE.md",
];

/// Flags que aparecen en una línea con `linkc` pero NO son de `linkc`:
/// `cargo build --release  # produce target/release/linkc` y el propio
/// `linkc --help` (que la salida de uso no se nombra a sí misma). Cualquier
/// otro flag en una línea así tiene que ser real.
const NOT_LINKC_FLAGS: &[&str] = &["--help", "--release"];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("compiler/ siempre tiene un directorio padre")
        .to_path_buf()
}

fn read_doc(rel: &str) -> String {
    let path = repo_root().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("no se pudo leer {}: {e}", path.display()))
}

/// La salida de uso del binario REAL (sin argumentos imprime el listado de
/// subcomandos con todos sus flags). stdout+stderr juntos: el listado va a
/// stdout, pero cualquier reordenamiento futuro no debería romper este test.
fn linkc_help() -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).output().expect("no se pudo ejecutar linkc");
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(text.contains("linkc serve"), "la salida de uso de linkc no tiene la forma esperada:\n{text}");
    text
}

#[test]
fn every_grammar_section_cited_in_agent_docs_exists() {
    let grammar = read_doc("GRAMMAR.md");
    let headings: Vec<String> = grammar
        .lines()
        .filter_map(|l| l.strip_prefix("### 3."))
        .filter_map(|rest| rest.split_whitespace().next())
        .map(|n| n.to_string())
        .collect();
    assert!(headings.len() > 200, "GRAMMAR.md tiene menos secciones §3.N de las esperadas: {}", headings.len());

    let cite = Regex::new(r"§3\.(\d+)").unwrap();
    let mut missing = Vec::new();
    for doc in DOCS {
        let text = read_doc(doc);
        for (idx, line) in text.lines().enumerate() {
            for m in cite.captures_iter(line) {
                let whole = m.get(0).unwrap();
                // `PLAN.md §3.1` sería la sección 3.1 de PLAN.md, no de
                // GRAMMAR.md -- hoy ningún archivo de la lista la cita así,
                // pero el filtro deja la regla explícita.
                let before: String = line[..whole.start()].chars().rev().take(12).collect::<Vec<_>>().into_iter().rev().collect();
                if before.contains("PLAN.md") {
                    continue;
                }
                let n = &m[1];
                if !headings.iter().any(|h| h == n) {
                    missing.push(format!("{doc}:{}: §3.{n}", idx + 1));
                }
            }
        }
    }
    assert!(missing.is_empty(), "secciones citadas que no existen en GRAMMAR.md:\n{}", missing.join("\n"));
}

#[test]
fn every_linkc_flag_mentioned_in_agent_docs_is_in_linkc_help() {
    let help = linkc_help();
    let flag = Regex::new(r"--[a-z][a-z0-9-]*").unwrap();
    // Destinos de link Markdown (`](#3216-cero-warnings--d-warnings...)`,
    // `](https://...)`): un anchor de GitHub convierte `-D warnings` en
    // `--d-warnings`, que no es un flag de nada.
    let link_target = Regex::new(r"\]\([^)]*\)|\(#[^)]*\)").unwrap();
    let mut unknown = Vec::new();
    for doc in DOCS {
        let text = read_doc(doc);
        for (idx, raw) in text.lines().enumerate() {
            if !raw.contains("linkc") {
                continue;
            }
            let line = link_target.replace_all(raw, "");
            for m in flag.find_iter(&line) {
                let f = m.as_str();
                if NOT_LINKC_FLAGS.contains(&f) || help.contains(f) {
                    continue;
                }
                unknown.push(format!("{doc}:{}: {f}", idx + 1));
            }
        }
    }
    unknown.sort();
    unknown.dedup();
    assert!(
        unknown.is_empty(),
        "flags nombrados junto a `linkc` en la documentación que NO están en `linkc --help` (o el flag no existe, o --help lo omite):\n{}",
        unknown.join("\n")
    );
}
