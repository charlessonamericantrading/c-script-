// MCP real (Model Context Protocol) sobre `linkc serve` (GRAMMAR.md §3.203).
// Activo SOLO con `--mcp-jwt-secret`/`LINK_MCP_JWT_SECRET` configurado --
// sin eso, `/mcp` no existe (mismo criterio que cualquier otro flag
// opcional de este servidor). Deliberadamente SIN una anotación `.link`
// nueva (`@mcp`): `openapi_emit.rs::emit_openapi_json` ya expone TODAS las
// `service`s sin ningún opt-in, mismo criterio acá -- todo `rpc` no-stream
// y no-`@cron` queda expuesto como tool MCP, y la autorización de cada uno
// sigue viniendo de sus anotaciones YA existentes (`@authenticated`/
// `@requires`), reusando `check_auth_gate`/`handle_rpc` (`runtime/server.rs`)
// TAL CUAL -- MCP es un transporte más sobre el mismo `rpc` protegido,
// nunca una capa nueva sin auditar.
//
// Piezas (PLAN.md §9.15 ítem 3, las 3 en esta ronda):
//   A (v1.159.0): sesión -- `initialize`/`DELETE`.
//   B (este archivo, v1.160.0): `tools/list`/`tools/call`.
//   C (v1.161.0): `mcp.sample` + streaming bidireccional real.

use super::db::Db;
use super::server::{check_auth_gate, handle_rpc};
use super::session::SessionStore;
use crate::ast::{Item, Member, Program};
use crate::checker::Checker;
use crate::codegen::openapi_emit::type_to_json_schema;

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
fn handle_initialize(sessions: &SessionStore, bearer_token: Option<&str>, request_id: &serde_json::Value) -> (u16, serde_json::Value, Option<String>) {
    let Some(token) = bearer_token else {
        return (401, err_response(request_id, -32001, "initialize requiere Authorization: Bearer <token>"), None);
    };
    let Some((_, role)) = sessions.role_for(token) else {
        return (401, err_response(request_id, -32001, "token inválido o expirado"), None);
    };
    let user_id = sessions.user_id_for(token);
    let Some(mcp_session_id) = sessions.sign_mcp_session(&role, user_id) else {
        return (500, err_response(request_id, -32000, "MCP no está habilitado en este servidor"), None);
    };
    let result = serde_json::json!({
        "protocolVersion": PROTOCOL_VERSION,
        "serverInfo": { "name": "linkc", "version": crate::VERSION },
        "capabilities": { "tools": {} },
    });
    (200, ok_response(request_id, result), Some(mcp_session_id))
}

/// `method: "tools/list"` -- todo `rpc` no-stream y no-`@cron` de toda
/// `service`, mismo filtro que `openapi_emit.rs::emit_openapi_json`
/// (`Member::Rpc(r) if r.cron().is_none()`, excluye `Member::Stream`).
/// `inputSchema` reusa `type_to_json_schema` (`codegen/openapi_emit.rs`),
/// no una segunda copia del mapeo `Type` -> JSON Schema.
fn tools_list(program: &Program, checker: &Checker) -> Result<serde_json::Value, String> {
    let mut tools = Vec::new();
    for item in &program.items {
        let Item::Service(service) = item else { continue };
        for member in &service.members {
            let Member::Rpc(rpc) = member else { continue };
            if rpc.cron().is_some() {
                continue;
            }
            let mut properties = serde_json::Map::new();
            let mut required = Vec::new();
            for p in &rpc.params {
                let ty = checker.resolve_type(&p.ty).map_err(|e| e.to_string())?;
                properties.insert(p.name.clone(), type_to_json_schema(&ty));
                if p.default.is_none() {
                    required.push(serde_json::json!(p.name));
                }
            }
            tools.push(serde_json::json!({
                "name": format!("{}_{}", service.name, rpc.name),
                "description": rpc.doc.clone().unwrap_or_default(),
                "inputSchema": {
                    "type": "object",
                    "properties": properties,
                    "required": required,
                },
            }));
        }
    }
    Ok(serde_json::json!({ "tools": tools }))
}

/// Resuelve `"{service}_{rpc}"` -> `(service_name, rpc_name)` buscando,
/// entre los MISMOS tools que `tools_list` generaría, cuál nombre
/// construido coincide -- no un `split` estadístico del string (un nombre
/// de `service`/`rpc` puede tener guiones bajos propios, así que partir el
/// string a mano sería ambiguo); esto reusa la única fuente de verdad del
/// nombrado.
fn resolve_tool_name(program: &Program, tool_name: &str) -> Option<(String, String)> {
    for item in &program.items {
        let Item::Service(service) = item else { continue };
        for member in &service.members {
            let Member::Rpc(rpc) = member else { continue };
            if rpc.cron().is_some() {
                continue;
            }
            if format!("{}_{}", service.name, rpc.name) == tool_name {
                return Some((service.name.clone(), rpc.name.clone()));
            }
        }
    }
    None
}

/// `method: "tools/call"` -- exige un `Mcp-Session-Id` válido (post-
/// `initialize`), delega la autorización y la invocación real a
/// `check_auth_gate`/`handle_rpc` (`runtime/server.rs`) SIN NINGÚN camino
/// paralelo: `role_for`/`user_id_for` (`session.rs`) ya saben resolver un
/// `Mcp-Session-Id` como una fuente de identidad más, así que estas dos
/// funciones funcionan tal cual, sin saber que el token es de MCP. El
/// resultado exitoso se envuelve en el formato de bloques de contenido de
/// MCP (`{content: [{type: "text", text: ...}]}`).
fn handle_tools_call(
    program: &Program,
    db: &Db,
    sessions: &SessionStore,
    mcp_session_id: Option<&str>,
    params: &serde_json::Value,
    request_id: &serde_json::Value,
) -> (u16, serde_json::Value) {
    let Some(tool_name) = params.get("name").and_then(|n| n.as_str()) else {
        return (400, err_response(request_id, -32602, "falta 'params.name'"));
    };
    let arguments = params.get("arguments").cloned().unwrap_or_else(|| serde_json::json!({}));
    let Some((service_name, rpc_name)) = resolve_tool_name(program, tool_name) else {
        return (404, err_response(request_id, -32601, &format!("tool desconocido: '{tool_name}'")));
    };
    let Some(session_token) = mcp_session_id else {
        return (401, err_response(request_id, -32001, "falta el header Mcp-Session-Id -- llamá 'initialize' primero"));
    };
    let auth_gate = check_auth_gate(program, sessions, Some(session_token), &service_name, &rpc_name);
    if let Err((status, msg)) = auth_gate.outcome {
        return (status, err_response(request_id, -32001, msg));
    }
    let (status, body_str, _content_type, _location, _cache_control) =
        handle_rpc(program, db, sessions, Some(session_token), &service_name, &rpc_name, arguments);
    if (200..300).contains(&status) {
        let result_value: serde_json::Value = serde_json::from_str(&body_str).unwrap_or(serde_json::Value::Null);
        let content = serde_json::json!({ "content": [{ "type": "text", "text": result_value.to_string() }] });
        (200, ok_response(request_id, content))
    } else {
        let message = serde_json::from_str::<serde_json::Value>(&body_str)
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
            .unwrap_or(body_str);
        (status, err_response(request_id, -32000, &message))
    }
}

/// Resultado de manejar un `POST /mcp` -- separa el `status`/`body` JSON-RPC
/// del valor nuevo (si hay) para el header de respuesta `Mcp-Session-Id`,
/// que solo `initialize` produce hoy.
pub(crate) struct PostResult {
    pub status: u16,
    pub body: serde_json::Value,
    pub new_session_id: Option<String>,
}

/// Dispatch de `POST /mcp` por `method` del cuerpo JSON-RPC. La entrega de
/// una respuesta correlacionada (Pieza C) todavía no está conectada --
/// cualquier `method` que no sea `initialize`/`tools/list`/`tools/call` da
/// un 501 explícito, nunca un 404/500 genérico que confunda "no
/// implementado todavía" con "no existe".
pub(crate) fn handle_post(
    program: &Program,
    db: &Db,
    sessions: &SessionStore,
    bearer_token: Option<&str>,
    mcp_session_id: Option<&str>,
    body: &str,
) -> PostResult {
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
            let (status, resp_body, new_session_id) = handle_initialize(sessions, bearer_token, &request_id);
            PostResult { status, body: resp_body, new_session_id }
        }
        Some("tools/list") => {
            let (checker, errors) = Checker::build_symbols(program);
            if let Some(e) = errors.into_iter().next() {
                return PostResult { status: 500, body: err_response(&request_id, -32000, &e.to_string()), new_session_id: None };
            }
            match tools_list(program, &checker) {
                Ok(result) => PostResult { status: 200, body: ok_response(&request_id, result), new_session_id: None },
                Err(e) => PostResult { status: 500, body: err_response(&request_id, -32000, &e), new_session_id: None },
            }
        }
        Some("tools/call") => {
            let params = parsed.get("params").cloned().unwrap_or(serde_json::Value::Null);
            let (status, body) = handle_tools_call(program, db, sessions, mcp_session_id, &params, &request_id);
            PostResult { status, body, new_session_id: None }
        }
        Some(other) => PostResult {
            status: 501,
            body: err_response(
                &request_id,
                -32601,
                &format!("método MCP todavía no soportado: '{other}' (GRAMMAR.md §3.203 -- streaming bidireccional llega en la próxima pieza de esta ronda)"),
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
pub(crate) fn handle_delete_session(sessions: &SessionStore, mcp_session_id: Option<&str>) -> u16 {
    let Some(token) = mcp_session_id else {
        return 400;
    };
    let Some((_, _, jti)) = sessions.verify_mcp_session(token) else {
        return 404;
    };
    sessions.revoke_mcp_session(&jti);
    204
}
