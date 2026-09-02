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
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::mpsc::SyncSender;
use std::sync::Arc;
use std::time::Duration;

pub(crate) const SESSION_HEADER: &str = "Mcp-Session-Id";

/// Cuántos mensajes iniciados por el servidor puede acumular una conexión
/// `GET /mcp` antes de que `mcp.sample` empiece a fallar en vez de
/// bloquear -- mismo criterio y mismo orden de magnitud que
/// `Db::LIVE_STREAM_BUFFER` (`runtime/db.rs`, GRAMMAR.md §3.16): un cliente
/// MCP real procesa un `sampling/createMessage` bastante más rápido de lo
/// que este servidor podría generarlos en secuencia (cada uno bloquea la
/// invocación del `rpc` que lo pidió hasta tener respuesta), así que un
/// buffer chico alcanza de sobra.
pub(crate) const CONNECTION_BUFFER: usize = 32;

/// Cuánto espera `mcp.sample` una respuesta correlacionada antes de
/// rendirse -- deliberadamente generoso (un cliente MCP real puede
/// necesitar completar una llamada a un LLM), pero acotado: sin esto, un
/// cliente que nunca responde dejaría el hilo de ESE `rpc` bloqueado para
/// siempre (mismo espíritu que `MAX_WHILE_ITERATIONS`, GRAMMAR.md §3.15 --
/// un backstop contra el caso colgado, no una cuota fina de recursos).
const SAMPLE_TIMEOUT: Duration = Duration::from_secs(30);

/// Estado compartido de la Pieza C, construido UNA vez en `server::serve`
/// junto a los demás `Arc` (mismo criterio que `Db::subscribers`,
/// `runtime/db.rs`) y clonado (barato: dos incrementos de refcount) por
/// request en `spawn_handler!`.
#[derive(Clone)]
pub(crate) struct McpSharedState {
    /// A qué conexión `GET /mcp` abierta empujarle un mensaje iniciado por
    /// el servidor, por `jti` de sesión MCP.
    pub(crate) connections: Arc<parking_lot::Mutex<HashMap<String, SyncSender<serde_json::Value>>>>,
    /// La tabla de correlación en sí -- exactamente la forma validada por
    /// el spike aislado de PLAN.md §9.15 ítem 3 (`GET`/`POST` ->
    /// `recv_timeout` -> limpieza en timeout, candado tomado y soltado,
    /// nunca sostenido durante el bloqueo). El `String` extra es el `jti`
    /// de la sesión MCP DUEÑA de la request pendiente (GRAMMAR.md §3.212):
    /// solo esa sesión puede entregar la respuesta -- sin esto, cualquier
    /// POST anónimo que adivinara (o predijera, ver `fresh_id`) el id
    /// inyectaba la "respuesta del LLM" que consume el rpc.
    pending: Arc<parking_lot::Mutex<HashMap<String, (String, std::sync::mpsc::Sender<serde_json::Value>)>>>,
}

impl McpSharedState {
    pub(crate) fn new() -> Self {
        McpSharedState { connections: Arc::new(parking_lot::Mutex::new(HashMap::new())), pending: Arc::new(parking_lot::Mutex::new(HashMap::new())) }
    }
}

thread_local! {
    /// Contexto MCP de la sesión que invocó, vía `tools/call`, el `rpc` que
    /// está corriendo AHORA MISMO en ESTE hilo -- mismo mecanismo exacto
    /// que `CURRENT_REQUEST` (`runtime/db.rs`, GRAMMAR.md §3.38): un hilo
    /// por request (GRAMMAR.md §3.158) hace que un `thread_local!` sea
    /// exactamente "el contexto de la request actual", sin candado ni
    /// riesgo de que dos requests concurrentes se pisen. Fijado por
    /// `handle_post` justo antes de invocar el `rpc`, limpiado apenas
    /// vuelve.
    static CURRENT_MCP: RefCell<Option<(String, McpSharedState)>> = const { RefCell::new(None) };
}

fn set_current(jti: String, state: McpSharedState) {
    CURRENT_MCP.with(|c| *c.borrow_mut() = Some((jti, state)));
}

fn clear_current() {
    CURRENT_MCP.with(|c| *c.borrow_mut() = None);
}

fn current() -> Option<(String, McpSharedState)> {
    CURRENT_MCP.with(|c| c.borrow().clone())
}

/// El id de correlación es una credencial de facto (quien lo conoce puede
/// entregar la "respuesta del LLM" que consume el rpc), así que su entropía
/// no es negociable: si `getrandom` falla, cortar el proceso -- MISMO
/// criterio exacto que `fresh_128_bits` en `session.rs` (GRAMMAR.md §3.212).
/// El `let _ =` anterior dejaba el buffer EN CEROS ante un fallo -- un id
/// `"000...0"` perfectamente predecible, silenciosamente.
fn fresh_id() -> String {
    let mut buf = [0u8; 16];
    getrandom::getrandom(&mut buf).expect("getrandom falló: sin una fuente de aleatoriedad del sistema no se puede generar un id de correlación MCP seguro");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// `mcp.sample(prompt: String) -> String` (GRAMMAR.md §3.203, Pieza C) --
/// alcance v1 deliberadamente angosto: un solo turno de texto, sin roles
/// ni historial multi-turno. Arma una request `sampling/createMessage`
/// real, la empuja por la conexión `GET /mcp` abierta de la sesión actual
/// (`CURRENT_MCP`), y bloquea (con timeout) hasta que una respuesta
/// correlacionada llegue por `POST /mcp` (`handle_correlated_response`,
/// más abajo) -- exactamente el mecanismo que el spike aislado validó.
pub(crate) fn sample(prompt: &str) -> Result<String, String> {
    let Some((jti, state)) = current() else {
        return Err(
            "mcp.sample: no hay ninguna sesión MCP activa en este hilo -- solo se puede llamar dentro de un rpc invocado vía tools/call, con una conexión GET /mcp abierta para esa sesión (GRAMMAR.md §3.203)"
                .to_string(),
        );
    };
    let Some(sender) = state.connections.lock().get(&jti).cloned() else {
        return Err(
            "mcp.sample: no hay ninguna conexión GET /mcp abierta para esta sesión -- el cliente MCP tiene que mantener el stream abierto para poder recibir mensajes iniciados por el servidor"
                .to_string(),
        );
    };

    let (tx, rx) = std::sync::mpsc::channel::<serde_json::Value>();
    let id = fresh_id();
    state.pending.lock().insert(id.clone(), (jti.clone(), tx));

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "sampling/createMessage",
        "params": { "messages": [{ "role": "user", "content": { "type": "text", "text": prompt } }] },
    });
    if sender.try_send(request).is_err() {
        state.pending.lock().remove(&id);
        return Err("mcp.sample: no se pudo entregar el mensaje -- la conexión GET /mcp de esta sesión está saturada o cerrada".to_string());
    }

    match rx.recv_timeout(SAMPLE_TIMEOUT) {
        Ok(response) => extract_sample_text(&response),
        Err(_) => {
            state.pending.lock().remove(&id);
            Err("mcp.sample: el cliente MCP no respondió dentro del tiempo límite (30s)".to_string())
        }
    }
}

/// `result.content[0].text` (la forma real de una respuesta de
/// `sampling/createMessage`, mismo formato de bloques de contenido que
/// `tools/call` ya usa) -- o el mensaje de un `error` JSON-RPC, si el
/// cliente lo devolvió así.
fn extract_sample_text(response: &serde_json::Value) -> Result<String, String> {
    if let Some(error) = response.get("error") {
        let message = error.get("message").and_then(|m| m.as_str()).unwrap_or("error desconocido");
        return Err(format!("mcp.sample: el cliente MCP devolvió un error: {message}"));
    }
    response
        .get("result")
        .and_then(|r| r.get("content"))
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|first| first.get("text"))
        .and_then(|t| t.as_str())
        .map(str::to_string)
        .ok_or_else(|| "mcp.sample: la respuesta del cliente MCP no tiene la forma esperada (result.content[0].text)".to_string())
}

/// Un `POST /mcp` sin `method` pero CON `id` es una respuesta JSON-RPC
/// correlacionada (a un `sampling/createMessage` que este servidor inició)
/// -- distinto de una REQUEST sin `method`, que sigue siendo un error 400
/// (ver `handle_post`). Busca+saca el `Sender` pendiente (nunca lo deja
/// después de entregar -- mismo criterio "remove antes de send" que el
/// spike aislado confirmó necesario para que un timeout tardío no reciba
/// una entrega fantasma).
///
/// Exige un `Mcp-Session-Id` válido Y que sea el de la sesión DUEÑA de la
/// request pendiente (GRAMMAR.md §3.212) -- hasta v1.170.0 este camino no
/// verificaba NADA: cualquier POST anónimo con el id correcto inyectaba la
/// "respuesta del LLM". El `sampling/createMessage` salió por la conexión
/// GET de UNA sesión concreta; solo esa sesión tiene motivo legítimo para
/// responder. Sesión válida pero ajena da el MISMO 404 que un id
/// inexistente -- no confirmar a una sesión ajena que el id existe.
fn handle_correlated_response(
    state: &McpSharedState,
    sessions: &SessionStore,
    mcp_session_id: Option<&str>,
    parsed: &serde_json::Value,
) -> (u16, serde_json::Value) {
    let Some(token) = mcp_session_id else {
        return (401, serde_json::json!({"error": "falta el header Mcp-Session-Id -- una respuesta correlacionada solo puede entregarla la sesión que recibió el sampling/createMessage"}));
    };
    let Some((_, _, responder_jti)) = sessions.verify_mcp_session(token) else {
        return (401, serde_json::json!({"error": "Mcp-Session-Id inválido, expirado o revocado"}));
    };
    let id = match parsed.get("id") {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Number(n)) => n.to_string(),
        _ => return (400, serde_json::json!({"error": "falta 'id'"})),
    };
    let mut pending = state.pending.lock();
    match pending.get(&id) {
        Some((owner_jti, _)) if *owner_jti == responder_jti => {
            let (_, tx) = pending.remove(&id).expect("la entrada existe: se leyó bajo el mismo candado");
            drop(pending);
            let _ = tx.send(parsed.clone());
            (200, serde_json::json!({"delivered": true}))
        }
        _ => (404, serde_json::json!({"error": format!("no hay ninguna request pendiente con id '{id}'")})),
    }
}

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

/// Auditoría del lenguaje (2026-09-01), GRAMMAR.md §3.204: `tools/list`
/// aplana `(service, rpc)` a un único string `"{service}_{rpc}"` -- un
/// espacio de nombres plano exigido por el protocolo MCP, sin ningún
/// separador real. Como un nombre de `service`/`rpc` puede tener guiones
/// bajos propios, dos pares DISTINTOS pueden generar el MISMO nombre de tool
/// (`service A_B { rpc c() }` y `service A { rpc B_c() }` ambos dan
/// `"A_B_c"`) -- `resolve_tool_name` hacía un primer-match lineal sobre
/// `program.items`, así que una colisión así enrutaba SILENCIOSAMENTE
/// `tools/call` al primer `rpc` en orden de declaración, nunca al que el
/// nombre del tool realmente identificaba (riesgo real: el rpc "robado" por
/// la colisión puede tener un `@requires` distinto -- ver GRAMMAR.md §3.203
/// -- del que el cliente MCP pretendía invocar). En vez de una ambigüedad
/// silenciosa por request, esto falla FUERTE una única vez al arrancar
/// `--mcp`, nombrando los dos `(service, rpc)` en colisión.
pub fn validate_tool_names(program: &Program) -> Result<(), String> {
    let mut seen: HashMap<String, (String, String)> = HashMap::new();
    for item in &program.items {
        let Item::Service(service) = item else { continue };
        for member in &service.members {
            let Member::Rpc(rpc) = member else { continue };
            if rpc.cron().is_some() {
                continue;
            }
            let name = format!("{}_{}", service.name, rpc.name);
            if let Some((prev_service, prev_rpc)) = seen.get(&name) {
                return Err(format!(
                    "--mcp: '{prev_service}.{prev_rpc}' y '{}.{}' generan el mismo nombre de tool MCP ('{name}') -- \
                     los tools de MCP son un espacio de nombres plano ('{{service}}_{{rpc}}', sin separador real), \
                     así que dos service/rpc distintos con ese mismo texto combinado colisionan; renombrá uno de los dos",
                    service.name, rpc.name
                ));
            }
            seen.insert(name, (service.name.clone(), rpc.name.clone()));
        }
    }
    Ok(())
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
    mcp_state: &McpSharedState,
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
    // GRAMMAR.md §3.203, Pieza C: `mcp.sample` (dentro del cuerpo de ESTE
    // `rpc`, si lo llama) necesita saber a qué sesión/conexión pertenece
    // esta invocación -- fijado en el `thread_local!` de este hilo justo
    // antes de invocar, limpiado apenas vuelve (con o sin error). Un
    // `Mcp-Session-Id` que no verificó (no debería pasar: `check_auth_gate`
    // de arriba ya lo hubiera rechazado) simplemente deja el contexto sin
    // fijar -- `mcp.sample` da su propio error claro en ese caso, nunca un
    // panic.
    if let Some((_, _, jti)) = sessions.verify_mcp_session(session_token) {
        set_current(jti, mcp_state.clone());
    }
    let result = handle_rpc(program, db, sessions, Some(session_token), &service_name, &rpc_name, arguments);
    clear_current();
    let (status, body_str, _content_type, _location, _cache_control) = result;
    if (200..300).contains(&status) {
        let result_value: serde_json::Value = serde_json::from_str(&body_str).unwrap_or(serde_json::Value::Null);
        // Un `rpc -> String` no debe terminar con comillas JSON de más
        // adentro del bloque de texto -- `"hola"` (con comillas) en vez de
        // `hola` sería un bug real, no una simplificación: un cliente MCP
        // real le mostraría las comillas al usuario/LLM. Solo un resultado
        // NO-string (número/objeto/array/bool) se serializa a JSON de
        // verdad para el campo `text`.
        let text = match &result_value {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let content = serde_json::json!({ "content": [{ "type": "text", "text": text }] });
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

/// Dispatch de `POST /mcp` por `method` del cuerpo JSON-RPC. Un cuerpo SIN
/// `method` pero CON `id` es una respuesta correlacionada (Pieza C) a un
/// `sampling/createMessage` que este servidor inició -- distinto de una
/// request sin `method` de verdad, que sigue siendo un 400. Cualquier
/// `method` real que no sea `initialize`/`tools/list`/`tools/call` da un
/// 501 explícito, nunca un 404/500 genérico que confunda "no implementado
/// todavía" con "no existe".
pub(crate) fn handle_post(
    program: &Program,
    db: &Db,
    sessions: &SessionStore,
    mcp_state: &McpSharedState,
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

    if method.is_none() && parsed.get("id").is_some() {
        let (status, body) = handle_correlated_response(mcp_state, sessions, mcp_session_id, &parsed);
        return PostResult { status, body, new_session_id: None };
    }

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
            let (status, body) = handle_tools_call(program, db, sessions, mcp_state, mcp_session_id, &params, &request_id);
            PostResult { status, body, new_session_id: None }
        }
        Some(other) => PostResult {
            status: 501,
            body: err_response(&request_id, -32601, &format!("método MCP todavía no soportado: '{other}'")),
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
