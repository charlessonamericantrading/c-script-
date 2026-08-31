// MCP real (Model Context Protocol) sobre `linkc serve` (GRAMMAR.md §3.203).
// Activo SOLO con `--mcp-jwt-secret`/`LINK_MCP_JWT_SECRET` configurado --
// sin eso, `/mcp` no existe (mismo criterio que cualquier otro flag
// opcional de este servidor). Deliberadamente SIN una anotación `.link`
// nueva (`@mcp`): `openapi_emit.rs::emit_openapi_json` ya expone TODAS las
// `service`s sin ningún opt-in, mismo criterio acá -- todo `rpc` no-stream
// y no-`@cron` queda expuesto como tool MCP, y la autorización de cada uno
// sigue viniendo de sus anotaciones YA existentes (`@authenticated`/
// `@requires`), nunca de una capa nueva sin auditar.
//
// Piezas (PLAN.md §9.15 ítem 3, las 3 en esta ronda):
//   A (este archivo, v1.159.0): sesión -- `initialize`/`DELETE`.
//   B (v1.160.0): `tools/list`/`tools/call`.
//   C (v1.161.0): `mcp.sample` + streaming bidireccional real.

use super::session::SessionStore;

pub(crate) const SESSION_HEADER: &str = "Mcp-Session-Id";

/// Versión de la spec `Streamable HTTP transport` contra la que se diseñó
/// esta implementación.
const PROTOCOL_VERSION: &str = "2025-06-18";

fn ok_response(id: &serde_json::Value, result: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn err_response(id: &serde_json::Value, code: i64, message: &str) -> serde_json::Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// `method: "initialize"` -- exige un `Authorization: Bearer <token>`
/// normal (la sesión YA establecida por el login existente del programa,
/// GRAMMAR.md §3.51/§3.64), y si pasa, firma una sesión MCP nueva
/// (`SessionStore::sign_mcp_session`) embebiendo el mismo rol/`user_id`.
fn handle_initialize(
    sessions: &SessionStore,
    bearer_token: Option<&str>,
    mcp_secret: &str,
    request_id: &serde_json::Value,
) -> (u16, serde_json::Value, Option<String>) {
    let Some(token) = bearer_token else {
        return (401, err_response(request_id, -32001, "initialize requiere Authorization: Bearer <token>"), None);
    };
    let Some((_, role)) = sessions.role_for(token) else {
        return (401, err_response(request_id, -32001, "token inválido o expirado"), None);
    };
    let user_id = sessions.user_id_for(token);
    let mcp_session_id = sessions.sign_mcp_session(&role, user_id, mcp_secret);
    let result = serde_json::json!({
        "protocolVersion": PROTOCOL_VERSION,
        "serverInfo": { "name": "linkc", "version": crate::VERSION },
        "capabilities": { "tools": {} },
    });
    (200, ok_response(request_id, result), Some(mcp_session_id))
}

/// Resultado de manejar un `POST /mcp` -- separa el `status`/`body` JSON-RPC
/// del valor nuevo (si hay) para el header de respuesta `Mcp-Session-Id`,
/// que solo `initialize` produce hoy.
pub(crate) struct PostResult {
    pub status: u16,
    pub body: serde_json::Value,
    pub new_session_id: Option<String>,
}

/// Dispatch de `POST /mcp` por `method` del cuerpo JSON-RPC. `tools/list`/
/// `tools/call` (Pieza B) y la entrega de una respuesta correlacionada
/// (Pieza C) todavía no están conectados -- cualquier `method` que no sea
/// `initialize` da un 501 explícito, nunca un 404/500 genérico que
/// confunda "no implementado todavía" con "no existe".
pub(crate) fn handle_post(sessions: &SessionStore, bearer_token: Option<&str>, mcp_secret: &str, body: &str) -> PostResult {
    let parsed: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => {
            return PostResult { status: 400, body: err_response(&serde_json::Value::Null, -32700, "JSON inválido"), new_session_id: None };
        }
    };
    let request_id = parsed.get("id").cloned().unwrap_or(serde_json::Value::Null);
    let method = parsed.get("method").and_then(|m| m.as_str());

    match method {
        Some("initialize") => {
            let (status, resp_body, new_session_id) = handle_initialize(sessions, bearer_token, mcp_secret, &request_id);
            PostResult { status, body: resp_body, new_session_id }
        }
        Some(other) => PostResult {
            status: 501,
            body: err_response(
                &request_id,
                -32601,
                &format!("método MCP todavía no soportado: '{other}' (GRAMMAR.md §3.203 -- tools/list/tools/call llegan en la próxima pieza de esta ronda)"),
            ),
            new_session_id: None,
        },
        None => {
            PostResult { status: 400, body: err_response(&request_id, -32600, "falta 'method' en la request JSON-RPC"), new_session_id: None }
        }
    }
}

/// `DELETE /mcp` -- revoca la sesión nombrada por el header
/// `Mcp-Session-Id`. Sin body JSON-RPC de request/response, la spec solo
/// pide un status code: `204` (terminada), `400` (sin el header), `404`
/// (el header no nombra una sesión MCP válida -- ya vencida, revocada, o
/// nunca existió; las tres son indistinguibles desde afuera a propósito,
/// mismo criterio que el resto de este módulo).
pub(crate) fn handle_delete_session(sessions: &SessionStore, mcp_session_id: Option<&str>, mcp_secret: &str) -> u16 {
    let Some(token) = mcp_session_id else {
        return 400;
    };
    let Some((_, _, jti)) = sessions.verify_mcp_session(token, mcp_secret) else {
        return 404;
    };
    sessions.revoke_mcp_session(&jti);
    204
}
