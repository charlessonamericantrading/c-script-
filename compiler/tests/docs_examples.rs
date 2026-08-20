// Los ejemplos de la documentación son tests, no prosa.
//
// Auditoría del 20/08/2026: de los 27 bloques de código Link publicados en
// README/llms.txt/reglas de agentes, la mayoría no compilaba. Los dos que
// más daño hacían eran justamente los que cualquiera copia primero: el
// ejemplo insignia del README usaba `role: Role.Member` (la variante de un
// enum en posición de EXPRESIÓN necesita llaves: `Role.Member {}`), y
// llms.txt -- el archivo cuyo único propósito es que un LLM aprenda el
// lenguaje -- enseñaba tres construcciones que el parser rechaza: tipo de
// retorno en un closure (`|t: Todo| -> Bool {}`), argumentos por defecto
// (`filter: Filter = Filter.All`) y lectura de un campo sobre `T?` tras un
// `if x != null` (no hay narrowing, GRAMMAR.md §3.4).
//
// Nada de eso lo detectaba ningún test porque la documentación nunca pasaba
// por el compilador. Este test cierra ese agujero: cada bloque marcado
// `<!-- linkc:check -->` se compila con el binario real, y si además declara
// un `test "..."`, se EJECUTA -- porque que tipe no prueba que corra (ver
// GRAMMAR.md §3.9 sobre divergencias checker-vs-runtime, que es donde este
// proyecto ya encontró bugs reales antes).
//
// Un bloque de código Link sin clasificar hace fallar este test a propósito:
// sin eso, el próximo ejemplo que alguien agregue vuelve a quedar fuera de
// la red sin que nadie se entere.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Archivos donde vive código Link publicado. Si agregás documentación nueva
/// con ejemplos, sumala acá.
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
    "CONTRIBUTING.md",
    "AGENTS.md",
];

const CHECK: &str = "<!-- linkc:check -->";
const PART: &str = "<!-- linkc:part -->";
const FRAGMENT: &str = "<!-- linkc:fragment -->";

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-docs-test-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let path = std::env::temp_dir().join(unique);
        fs::create_dir_all(&path).expect("no se pudo crear tempdir para test");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Cómo se verifica un bloque.
#[derive(PartialEq)]
enum Kind {
    /// Programa completo: compila solo.
    Standalone,
    /// Un capítulo de UN mismo programa explicado por partes a lo largo del
    /// archivo (el patrón de las reglas para agentes: primero los tipos,
    /// después `db`, después el `service`, al final el `test`). Ninguna parte
    /// compila aislada, pero todas juntas sí -- y así se verifican.
    Part,
    /// Recorte sin contexto suficiente ni siquiera sumado al resto.
    Fragment,
}

struct Block {
    doc: String,
    line: usize,
    code: String,
    kind: Kind,
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("compiler/ siempre tiene un directorio padre")
        .to_path_buf()
}

/// Un bloque cuenta como código Link si su cerca abre con `rust` o `link`.
/// Los demás lenguajes (bash, ts, json) se ignoran enteros.
fn is_link_fence(info: &str) -> bool {
    matches!(info.trim(), "rust" | "link")
}

fn fence_at(line: &str) -> Option<&str> {
    line.trim_start().strip_prefix("```")
}

fn collect_blocks(doc: &str, text: &str) -> Vec<Block> {
    let lines: Vec<&str> = text.lines().collect();
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let Some(info) = fence_at(lines[i]) else {
            i += 1;
            continue;
        };

        if !is_link_fence(info) {
            // Bloque de otro lenguaje: saltar hasta su cierre.
            i += 1;
            while i < lines.len() && fence_at(lines[i]).is_none() {
                i += 1;
            }
            i += 1;
            continue;
        }

        // El marcador es la última línea no vacía antes de la cerca.
        let marker = lines[..i]
            .iter()
            .rev()
            .find(|l| !l.trim().is_empty())
            .map(|l| l.trim())
            .unwrap_or("");
        let open_line = i + 1;
        i += 1;
        let start = i;
        while i < lines.len() && fence_at(lines[i]).is_none() {
            i += 1;
        }
        let code = lines[start..i].join("\n");
        i += 1;

        let kind = if marker == CHECK {
            Kind::Standalone
        } else if marker == PART {
            Kind::Part
        } else if marker == FRAGMENT {
            Kind::Fragment
        } else {
            panic!(
                "\n{doc}:{open_line}: bloque de código Link sin clasificar.\n\
                 Poné una de estas líneas JUSTO ANTES de la cerca de apertura:\n  \
                 {CHECK}     -- programa completo, compila solo\n  \
                 {PART}      -- un capítulo de un programa que el archivo arma por partes\n  \
                 {FRAGMENT}  -- recorte que no compila ni sumado al resto\n\
                 Sin esto, un ejemplo roto se publica sin que nadie se entere.\n"
            )
        };

        blocks.push(Block {
            doc: doc.to_string(),
            line: open_line,
            code,
            kind,
        });
    }
    blocks
}

/// Compila el programa, y si declara tests, los ejecuta. Devuelve el error
/// tal como lo imprime el binario -- el mismo texto que vería quien copió el
/// ejemplo.
fn verify(temp: &TempDir, stem: &str, code: &str, executed: &mut usize) -> Result<(), String> {
    let src = temp.0.join(format!("{stem}.link"));
    fs::write(&src, code).unwrap();
    let out = temp.0.join(format!("{stem}_gen"));

    let build = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("build")
        .arg(&src)
        .arg(&out)
        .output()
        .expect("no se pudo ejecutar linkc build");

    if !build.status.success() {
        return Err(format!(
            "`linkc build` falló:\n{}{}",
            String::from_utf8_lossy(&build.stdout),
            String::from_utf8_lossy(&build.stderr),
        ));
    }

    // Que tipe no prueba que corra: si el ejemplo trae tests, se corren.
    if !code.lines().any(|l| l.trim_start().starts_with("test \"")) {
        return Ok(());
    }
    let run = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("test")
        .arg(&src)
        .output()
        .expect("no se pudo ejecutar linkc test");
    *executed += 1;
    if !run.status.success() {
        return Err(format!(
            "`linkc test` falló:\n{}{}",
            String::from_utf8_lossy(&run.stdout),
            String::from_utf8_lossy(&run.stderr),
        ));
    }
    Ok(())
}

#[test]
fn every_documented_example_compiles_and_runs() {
    let root = repo_root();
    let temp = TempDir::new("blocks");
    let mut compiled = 0usize;
    let mut executed = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for doc in DOCS {
        let path = root.join(doc);
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            // Un archivo listado que ya no existe es un error de
            // mantenimiento, no un test que se saltea en silencio.
            Err(e) => {
                failures.push(format!("{doc}: no se pudo leer ({e})"));
                continue;
            }
        };

        let blocks = collect_blocks(doc, &text);
        let slug = doc.replace(['/', '.', '\\'], "_");

        for (n, block) in blocks.iter().enumerate() {
            if block.kind != Kind::Standalone {
                continue;
            }
            compiled += 1;
            if let Err(e) = verify(&temp, &format!("{slug}_{n}"), &block.code, &mut executed) {
                failures.push(format!("{}:{} -- {e}", block.doc, block.line));
            }
        }

        // Las partes del archivo se compilan como el único programa que
        // describen entre todas.
        let parts: Vec<&Block> = blocks.iter().filter(|b| b.kind == Kind::Part).collect();
        if parts.is_empty() {
            continue;
        }
        let joined = parts
            .iter()
            .map(|b| b.code.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        compiled += 1;
        if let Err(e) = verify(&temp, &format!("{slug}_parts"), &joined, &mut executed) {
            let lines: Vec<String> = parts.iter().map(|b| b.line.to_string()).collect();
            failures.push(format!(
                "{doc} -- las partes (líneas {}) no forman un programa válido: {e}",
                lines.join(", ")
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "\n{} ejemplo(s) de la documentación no funcionan:\n\n{}\n",
        failures.len(),
        failures.join("\n---\n")
    );

    // Si alguien borra los marcadores, el test no debe pasar por vacío.
    assert!(
        compiled >= 5,
        "solo {compiled} bloques marcados con {CHECK}: la documentación perdió sus ejemplos verificados"
    );
    assert!(
        executed >= 1,
        "ningún ejemplo de la documentación ejecuta tests reales"
    );
}
