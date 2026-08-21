// Punto de entrada wasm del playground web (playground/index.html) -- crate
// aparte de `linkc` a propósito: depende de él con `default-features = false`
// para excluir el módulo `runtime` (rusqlite/postgres/tiny_http/argon2/lettre,
// ninguno compila a wasm32-unknown-unknown) y quedarse solo con
// lexer/parser/checker/codegen, que sí. Ver el comentario sobre el feature
// `runtime` en `../Cargo.toml` y `../src/lib.rs`.
//
// Deliberadamente NO hay una pestaña "ejecutar tests" real: eso necesita el
// intérprete (`runtime`), que este crate excluye a propósito -- mentir con
// una salida enlatada para esa pestaña sería repetir exactamente el problema
// que este trabajo vino a resolver (ver el banner de "maqueta" que reemplaza).

use linkc::{checker, codegen, diagnostics, lexer, parser};
use serde_json::json;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub fn compile_link(source: &str) -> String {
    let tokens = match lexer::tokenize(source) {
        Ok(t) => t,
        Err(e) => {
            let snippet = diagnostics::render_diagnostic(source, "playground.link", e.span, &e.message);
            return json!({ "ok": false, "errors": [snippet] }).to_string();
        }
    };

    let program = match parser::parse(tokens) {
        Ok(p) => p,
        Err(errors) => {
            let snippets: Vec<String> =
                errors.iter().map(|e| diagnostics::render_diagnostic(source, "playground.link", e.span, &e.message)).collect();
            return json!({ "ok": false, "errors": snippets }).to_string();
        }
    };

    if let Err(errors) = checker::Checker::check_program(&program) {
        let snippets: Vec<String> = errors
            .iter()
            .map(|e| match e.span {
                Some(span) => diagnostics::render_diagnostic(source, "playground.link", span, &e.message),
                None => e.to_string(),
            })
            .collect();
        return json!({ "ok": false, "errors": snippets }).to_string();
    }

    let contract = codegen::ts_emit::emit_contract(&program).unwrap_or_else(|e| format!("// error al emitir contract.d.ts: {e}"));
    let client = codegen::ts_emit::emit_client(&program).unwrap_or_else(|e| format!("// error al emitir client.ts: {e}"));
    let validators =
        codegen::validators_emit::emit_validators(&program).unwrap_or_else(|e| format!("// error al emitir validators.ts: {e}"));
    let openapi = codegen::openapi_emit::emit_openapi_json(&program, "playground.link")
        .unwrap_or_else(|e| json!({ "error": e }).to_string());
    let postgres =
        codegen::postgres_emit::generate_postgres_ddl(&program).unwrap_or_else(|e| format!("-- error al emitir DDL: {e}"));

    json!({
        "ok": true,
        "contract": contract,
        "client": client,
        "validators": validators,
        "openapi": openapi,
        "postgres": postgres,
    })
    .to_string()
}
