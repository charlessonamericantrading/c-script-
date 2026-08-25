//! Generador de `llms.txt` (convención [llmstxt.org](https://llmstxt.org/))
//! a partir de los servicios Link de un programa -- lista cada rpc/stream
//! con su firma y su docstring `///` ya capturada (GRAMMAR.md §3.72), para
//! que un agente de IA externo que llega al proyecto entienda la API sin
//! tener que leer el `.link` completo. PLAN.md §9.9 ítem 2.
//!
//! No confundir con el `llms.txt` de ESTE repo (documenta el COMPILADOR,
//! escrito a mano) -- este es el que `linkc build` emite para el proyecto
//! DE QUIEN adopta c-script, junto a `contract.d.ts`/`openapi.json`.

use crate::ast::{Item, Member, Program};
use crate::checker::Checker;

/// Arma el documento completo: título H1, resumen en blockquote, y una
/// sección H2 por `service` con un bullet `- [firma](/Servicio/rpc):
/// docstring` por rpc/stream -- mismo formato de lista de links que
/// llmstxt.org pide, usando la ruta real `/Servicio/rpc` (GRAMMAR.md §3.20)
/// como "link" aunque no sea un GET navegable: sigue siendo la referencia
/// exacta que un agente necesita para invocar ese rpc.
///
/// Un rpc/stream SIN docstring aparece igual, solo sin descripción --
/// omitirlo por completo escondería una capacidad real de la API, el mismo
/// criterio que `openapi_emit` ya sigue (un rpc sin `///` sigue apareciendo
/// en `paths`, solo sin `description`).
pub fn emit_llms_txt(program: &Program, title: &str) -> Result<String, String> {
    let (checker, errors) = Checker::build_symbols(program);
    if let Some(e) = errors.into_iter().next() {
        return Err(e.to_string());
    }

    let mut out = format!(
        "# {title}\n\n> API generada automáticamente por Link (c-script). Servicios y rpcs disponibles, cada uno con su firma y (si tiene) su docstring `///`.\n"
    );

    for item in &program.items {
        let Item::Service(service) = item else { continue };
        out.push_str(&format!("\n## {}\n\n", service.name));
        for member in &service.members {
            let (rpc, is_stream) = match member {
                Member::Rpc(r) => (r, false),
                Member::Stream(r) => (r, true),
            };

            let mut params_sig = Vec::with_capacity(rpc.params.len());
            for p in &rpc.params {
                let ty = checker.resolve_type(&p.ty).map_err(|e| e.to_string())?;
                params_sig.push(format!("{}: {ty}", p.name));
            }
            let ret_ty = checker.resolve_type(&rpc.return_type).map_err(|e| e.to_string())?;
            let kind = if is_stream { "stream" } else { "rpc" };
            let signature = format!("{kind} {}({}) -> {ret_ty}", rpc.name, params_sig.join(", "));
            let path = format!("/{}/{}", service.name, rpc.name);

            // Solo la PRIMERA línea de un docstring multi-línea -- llmstxt.org
            // espera una nota de una sola línea por entrada; el docstring
            // completo sigue disponible en `openapi.json`/`contract.d.ts`
            // para quien necesite el detalle entero.
            let first_doc_line = rpc.doc.as_deref().and_then(|d| d.lines().next()).map(str::trim).filter(|l| !l.is_empty());
            match first_doc_line {
                Some(line) => out.push_str(&format!("- [{signature}]({path}): {line}\n")),
                None => out.push_str(&format!("- [{signature}]({path})\n")),
            }
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer;
    use crate::parser;

    fn emit(code: &str) -> String {
        let tokens = lexer::tokenize(code).unwrap();
        let program = parser::parse(tokens).unwrap();
        emit_llms_txt(&program, "Task API").unwrap()
    }

    #[test]
    fn emits_a_title_and_a_section_per_service_with_one_bullet_per_rpc() {
        let out = emit(
            r#"
            service Tasks {
                rpc list() -> Int { 1 }
                rpc create(title: String) -> Int { 1 }
            }
            service Users {
                rpc me() -> Int { 1 }
            }
        "#,
        );
        assert!(out.starts_with("# Task API\n\n> "));
        assert!(out.contains("\n## Tasks\n\n"));
        assert!(out.contains("- [rpc list() -> Int](/Tasks/list)\n"));
        assert!(out.contains("- [rpc create(title: String) -> Int](/Tasks/create)\n"));
        assert!(out.contains("\n## Users\n\n"));
        assert!(out.contains("- [rpc me() -> Int](/Users/me)\n"));
    }

    /// Un docstring `///` (GRAMMAR.md §3.72) se propaga como la nota después
    /// de `:` -- mismo dato que `openapi_emit` ya usa como `description`.
    #[test]
    fn a_docstring_on_an_rpc_becomes_its_bullet_note() {
        let out = emit(
            r#"
            service Tasks {
                /// Lista todas las tareas pendientes, ordenadas por id.
                rpc list() -> Int { 1 }
            }
        "#,
        );
        assert!(out.contains("- [rpc list() -> Int](/Tasks/list): Lista todas las tareas pendientes, ordenadas por id.\n"));
    }

    /// Un docstring de más de una línea aporta solo la PRIMERA como nota --
    /// el resto sigue disponible en `openapi.json`/`contract.d.ts`.
    #[test]
    fn a_multiline_docstring_only_contributes_its_first_line() {
        let out = emit(
            "service Tasks {\n    /// Primera línea.\n    /// Segunda línea, más detalle.\n    rpc list() -> Int { 1 }\n}\n",
        );
        assert!(out.contains("- [rpc list() -> Int](/Tasks/list): Primera línea.\n"));
        assert!(!out.contains("Segunda línea"));
    }

    /// Un rpc SIN docstring sigue apareciendo -- solo sin nota después de
    /// `:` -- ocultarlo escondería una capacidad real de la API.
    #[test]
    fn an_rpc_without_a_docstring_still_appears_without_a_note() {
        let out = emit(
            r#"
            service Tasks {
                rpc list() -> Int { 1 }
            }
        "#,
        );
        assert!(out.contains("- [rpc list() -> Int](/Tasks/list)\n"));
        assert!(!out.contains("(/Tasks/list):"));
    }

    /// Un `stream` se distingue de un `rpc` en la firma -- misma
    /// información que `is_stream` ya usa en `openapi_emit` para el
    /// Content-Type de la respuesta.
    #[test]
    fn a_stream_is_labeled_differently_from_a_regular_rpc() {
        let out = emit(
            r#"
            service Feed {
                stream events() -> Int { [1] }
            }
        "#,
        );
        assert!(out.contains("- [stream events() -> Int](/Feed/events)\n"));
    }
}
