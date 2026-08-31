// Servidor HTTP mínimo que expone cada `rpc`/`stream` como POST
// /{Service}/{method}, leyendo argumentos como un objeto JSON (el mismo
// shape que produce client.ts) y devolviendo el resultado serializado. CORS
// abierto porque el frontend de la demo corre en otro puerto (ver
// examples/frontend/).
//
// Concurrencia (GRAMMAR.md §3.158, v1.114.0 -- Pilar 1 de un roadmap
// mayor): cada request atendida corre su propio `invoke_rpc`/cómputo en un
// `std::thread::spawn` DEDICADO, no en un único loop principal como antes
// de esta versión -- ver `spawn_handler!` más abajo. `Value::Closure` guarda
// un `Env` (`Rc<RefCell<Value>>`, no `Send`, desde la ronda de closures de
// GRAMMAR.md §3.10) sigue sin poder cruzar un borde de hilo -- pero eso ya
// no es un problema: cada `Value`/`Env` nace y muere ENTERO dentro del hilo
// de SU PROPIA request, nunca necesita viajar a otro. Lo que sí cruza hacia
// el hilo spawneado son `Arc<Db>`/`Arc<Program>`/`Arc<SessionStore>` (ya
// `Send + Sync` por diseño, ver runtime/db.rs) y el propio `Request`
// (`Send` por diseño de tiny_http).
//
// `stream` (GRAMMAR.md §2.1, streaming real v0): dentro del hilo de SU
// PROPIA request, el CÓMPUTO (`invoke_rpc`) sigue corriendo primero -- solo
// la ESCRITURA de los eventos SSE (potencialmente lenta si el cliente lee
// despacio) se manda a un hilo aparte todavía, así un stream largo no
// bloquea a ESA misma request de seguir procesando. El resultado que cruza
// hacia el hilo escritor sigue siendo `serde_json::Value` YA CONVERTIDO
// (sin ningún `Rc` adentro, `Send` de sobra), nunca `Db`/`Value` crudos.
//
// Push real v0 (GRAMMAR.md §3.16): un `stream` cuyo cuerpo matchea el
// shape reconocido (`ast::recognize_live_subscribe`) NUNCA pasa por
// `invoke_rpc_with_sessions` -- `live_subscribe_collection` lo detecta
// ANTES, y `Db::subscribe` (sincrónico, en el hilo de esa request) da la
// foto inicial más un `Receiver<serde_json::Value>` que el hilo escritor
// (`write_live_stream`) bloquea leyendo indefinidamente. Mismo respeto por
// el límite de `Send` de arriba: lo único que cruza al hilo escritor es
// JSON puro, nunca `Db`/`Value`.

use super::db::{encrypted_fields_by_collection, now_ms, Db};
use super::encryption;
use super::session::SessionStore;
use super::{
    invoke_rpc_with_sessions, is_cron_member, is_stream_member, live_subscribe_collection, required_auth, required_cache,
    required_cors, required_idempotent, required_rate_limit,
};
use crate::ast::{Annotation, Item, Member};
use crate::ast::Program;
use crate::cache::CacheStore;
use crate::idempotency::{hash_request_body, IdempotencyStore, Lookup};
use crate::metrics::MetricsStore;
use crate::rate_limit::{RateLimitSpec, RateLimiter};
use crate::route::RoutePattern;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::time::Duration;

/// Un id incremental por request -- lo único que hace falta para poder
/// correlacionar líneas de log una vez que hay más de un hilo escribiendo a
/// stdout al mismo tiempo (cada `stream` corre en el suyo, para la escritura).
/// Prerrequisito parcial de observabilidad real (PLAN.md §4, Fase 2) --
/// ver `log_done` para el resto.
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// `--log-level`/`LINK_LOG_LEVEL` (GRAMMAR.md §3.122): orden de declaración
/// = orden real (`derive(PartialOrd)`) -- `Debug < Info < Warn < Error`, así
/// que "esta línea se imprime" es literalmente `entry_level >= config.level`
/// en los dos call-sites de abajo. Sin entradas propias de nivel `Debug`
/// todavía (reservado para logging más fino a futuro); existe igual para
/// que la jerarquía completa esté declarada desde el principio.
#[derive(Clone, Copy, PartialEq, PartialOrd)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

/// `--log-format`/`LINK_LOG_FORMAT` (GRAMMAR.md §3.122).
#[derive(Clone, Copy, PartialEq)]
pub enum LogFormat {
    /// El formato de siempre, sin cambios -- `key=value` legible/greppable.
    Text,
    /// Una línea JSON por evento -- lo que un colector de logs real
    /// (CloudWatch, Datadog, `journald` con `journalctl -o json`) espera
    /// para poder indexar campos sin parsear texto libre.
    Json,
}

/// Armado UNA vez al arrancar (`main.rs`), `Copy` a propósito -- cruza a
/// los hilos de escritura de `stream` (`write_stream`/`write_live_stream`)
/// exactamente igual que `max_body_bytes: u64` ya cruza, sin necesitar
/// ninguna sincronización: es un valor fijo para toda la vida del proceso.
#[derive(Clone, Copy)]
pub struct LogConfig {
    pub format: LogFormat,
    pub level: LogLevel,
}

impl Default for LogConfig {
    /// Sin `--log-format`/`--log-level`: texto, nivel `Info` -- el
    /// comportamiento exacto de siempre (las dos líneas por request,
    /// recibida y completada, se siguen imprimiendo SIEMPRE). Solo pedir
    /// `--log-level warn`/`error` explícitamente reduce el volumen.
    fn default() -> Self {
        LogConfig { format: LogFormat::Text, level: LogLevel::Info }
    }
}

/// `status` -> el nivel que le corresponde a esa línea: `Error` (5xx) es un
/// fallo del SERVIDOR, `Warn` (4xx) es un rechazo esperado (auth, rate
/// limit, validación) pero igual señal de algo para mirar, cualquier otra
/// cosa (2xx/3xx, o `0` -- el sentinel de "cliente se desconectó a mitad de
/// un stream") es tráfico normal, `Info`.
fn status_level(status: u16) -> LogLevel {
    if status >= 500 {
        LogLevel::Error
    } else if status >= 400 {
        LogLevel::Warn
    } else {
        LogLevel::Info
    }
}

/// Tracing estructurado por RPC (GRAMMAR.md §2.1, auditoría post-push):
/// una sola línea por request COMPLETADA (greppable sin parsear JSON en
/// `LogFormat::Text`, mismo espíritu que el logging de texto de `tracing`/
/// Heroku -- no se suma la dependencia `tracing` para esto, `println!` ya
/// alcanza). `req_id` (existía desde antes, ver arriba) correlaciona esta
/// línea con la de "request recibida"; `method` es `None` para los casos
/// que nunca llegan a resolver `{service}.{rpc}` (ej. un 404 por URL mal
/// formada). `extra` es libre -- `error="..."` en una falla,
/// `sent=N total=M` en un stream, o simplemente vacío en un 200 normal --
/// en `LogFormat::Json` viaja tal cual, como un string sin parsear, dentro
/// del campo `"extra"` (`null` si está vacío): no hay una gramática fija
/// que separarlo en campos propios sin inventar un schema que esta ronda
/// no amerita, límite documentado en GRAMMAR.md §3.122, no escondido.
fn log_done(log: LogConfig, req_id: u64, method: Option<&str>, status: u16, start: std::time::Instant, extra: &str) {
    log_done_with_audit(log, req_id, method, status, start, extra, None)
}

/// Como `log_done`, más el rastro de auditoría de autorización (GRAMMAR.md
/// §3.148: "quién llamó a qué rpc, con qué rol, y si se permitió o
/// denegó") -- `audit` es `Some` SOLO para un rpc que de verdad declaró
/// `@authenticated`/`@requires` (`check_auth_gate::AuthGateResult`, más
/// arriba); un rpc público sigue usando `log_done` a secas, sin este campo.
/// En modo JSON, los tres campos van como claves de PRIMER NIVEL (no
/// enterrados en `extra`, a diferencia del resto de las anotaciones de esta
/// línea) -- son el dato que este ítem pide poder indexar/filtrar de
/// verdad ("mostrame todo lo que el rol X tuvo denegado"), no una nota
/// informativa más.
fn log_done_with_audit(
    log: LogConfig,
    req_id: u64,
    method: Option<&str>,
    status: u16,
    start: std::time::Instant,
    extra: &str,
    audit: Option<&AuthAudit>,
) {
    if status_level(status) < log.level {
        return;
    }
    let elapsed_ms = start.elapsed().as_millis();
    let method_field = method.unwrap_or("-");
    match log.format {
        LogFormat::Text => {
            let mut line = format!("[req {req_id}] method={method_field} status={status} duration_ms={elapsed_ms}");
            if let Some(a) = audit {
                line.push_str(&format!(
                    " auth_role={:?} auth_user_id={} auth_allowed={}",
                    a.role.as_deref().unwrap_or("-"),
                    a.user_id.map(|id| id.to_string()).unwrap_or_else(|| "-".to_string()),
                    a.allowed
                ));
            }
            if !extra.is_empty() {
                line.push(' ');
                line.push_str(extra);
            }
            println!("{line}");
        }
        LogFormat::Json => {
            let mut json = serde_json::json!({
                "req_id": req_id,
                "method": method,
                "status": status,
                "duration_ms": elapsed_ms,
                "extra": if extra.is_empty() { None } else { Some(extra) },
            });
            if let Some(a) = audit {
                json["auth_role"] = a.role.clone().map(serde_json::Value::String).unwrap_or(serde_json::Value::Null);
                json["auth_user_id"] = a.user_id.map(serde_json::Value::from).unwrap_or(serde_json::Value::Null);
                json["auth_allowed"] = serde_json::Value::Bool(a.allowed);
            }
            println!("{json}");
        }
    }
}

/// Una línea por corrida de una tarea `@cron` (GRAMMAR.md §3.159) -- mismo
/// espíritu que `log_done`, pero sin `req_id`/`status` HTTP (una tarea
/// programada no es una request). `ok=false` cuenta como `Error` para
/// `--log-level`, igual criterio que un 5xx en `status_level`.
fn log_cron_tick(log: LogConfig, method: &str, ok: bool, elapsed: std::time::Duration, extra: &str) {
    let level = if ok { LogLevel::Info } else { LogLevel::Error };
    if level < log.level {
        return;
    }
    let elapsed_ms = elapsed.as_millis();
    match log.format {
        LogFormat::Text => {
            let mut line = format!("[cron] method={method} ok={ok} duration_ms={elapsed_ms}");
            if !extra.is_empty() {
                line.push(' ');
                line.push_str(extra);
            }
            println!("{line}");
        }
        LogFormat::Json => {
            let json = serde_json::json!({
                "cron": method,
                "ok": ok,
                "duration_ms": elapsed_ms,
                "extra": if extra.is_empty() { None } else { Some(extra) },
            });
            println!("{json}");
        }
    }
}

/// De dónde salen los datos que sirve este servidor (GRAMMAR.md §3.36).
/// El resto del programa no cambia según cuál sea: el mismo `.link`, los
/// mismos rpc, el mismo contrato TypeScript generado.
#[derive(Clone)]
pub enum DbSource {
    /// Un archivo SQLite al lado del fuente -- el default de siempre.
    SqliteFile(PathBuf),
    /// Una URL de conexión de PostgreSQL (`postgres://usuario:clave@host/base`).
    Postgres(String),
}

/// Política de CORS del servidor (GRAMMAR.md §3.41), armada UNA vez al
/// arrancar (`main.rs::resolve_cors_origins`), nunca por request.
#[derive(Clone)]
pub enum CorsConfig {
    /// Sin `--cors-origin`/`LINK_CORS_ORIGINS`: cualquier origen, el
    /// comportamiento de siempre (`Access-Control-Allow-Origin: *`) -- no
    /// romper a nadie que no pida esto explícitamente.
    Any,
    /// Con al menos un origen configurado: solo esos, ecoados literal
    /// (nunca `*`) cuando el `Origin` de la request matchea EXACTO alguno
    /// de la lista.
    Allowlist(Vec<String>),
}

/// Los headers de CORS ya resueltos para UNA request particular -- se
/// computa una sola vez por request (`CorsConfig::headers_for`, con el
/// `Origin` que mandó el cliente) y se reusa en la respuesta que sea que
/// termine mandándose, incluida la de un `stream` SSE (`write_stream`/
/// `write_live_stream`, más abajo, que arman su header a mano).
#[derive(Clone)]
struct CorsHeaders {
    /// El valor para `Access-Control-Allow-Origin`, si corresponde
    /// mandarlo. `None` -- nunca un string vacío -- es la señal de "no
    /// mandes el header": con un allowlist configurado y un origen que no
    /// matchea, omitir el header es lo que hace que el navegador rechace
    /// la respuesta: exactamente el comportamiento que un allowlist tiene
    /// que dar.
    allow_origin: Option<String>,
    /// Si además hay que mandar `Vary: Origin` -- correcto solo cuando la
    /// respuesta depende de qué origen pidió (el caso `Allowlist`); con
    /// `*` la respuesta es la misma para cualquiera, así que `Vary` no
    /// aporta nada y no se manda.
    vary_origin: bool,
    /// El valor de `Strict-Transport-Security` a mandar en TODA respuesta,
    /// si `--hsts`/`LINK_HSTS` (GRAMMAR.md §3.143) lo configuró -- `None`
    /// por default, mismo criterio que `allow_origin`: nunca se INVENTA un
    /// valor, y HSTS nunca se manda solo. Constante para todo el proceso
    /// (no depende de la request, a diferencia de `allow_origin`) -- vive
    /// acá para que `cors_response_with_type`/`sse_preamble` (los dos
    /// lugares que arman una respuesta) lo lean de la MISMA bolsa que ya
    /// reciben, sin agregar un parámetro más a los 16 call-sites de
    /// `cors_response`/`cors_response_with_type`.
    hsts: Option<String>,
}

/// `@cors("...")` (GRAMMAR.md §3.147) a un `CorsConfig` -- mismo formato
/// separado-por-comas que `LINK_CORS_ORIGINS` (`main.rs::resolve_cors_origins`),
/// más `"*"` como caso especial para `CorsConfig::Any`. El checker ya
/// garantizó que el valor no está vacío, así que el `Allowlist` resultante
/// siempre tiene al menos un origen.
fn parse_cors_override(raw: &str) -> CorsConfig {
    if raw.trim() == "*" {
        return CorsConfig::Any;
    }
    CorsConfig::Allowlist(raw.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
}

impl CorsConfig {
    fn headers_for(&self, request_origin: Option<&str>) -> CorsHeaders {
        // Defensa en profundidad: un `Origin` de verdad, parseado por
        // tiny_http, nunca puede traer CR/LF (el parser de líneas HTTP se
        // lo impide antes de que este código lo vea) -- pero `write_stream`/
        // `write_live_stream` interpolan este valor en un header armado a
        // mano, sin pasar por `tiny_http::Header::from_bytes` (que sí
        // valida esto), así que no vale la pena depender solo de esa
        // garantía ajena.
        let request_origin = request_origin.filter(|o| !o.contains(['\r', '\n']));
        match self {
            CorsConfig::Any => CorsHeaders { allow_origin: Some("*".to_string()), vary_origin: false, hsts: None },
            CorsConfig::Allowlist(list) => {
                let matched = request_origin.filter(|o| list.iter().any(|a| a == o)).map(str::to_string);
                CorsHeaders { allow_origin: matched, vary_origin: true, hsts: None }
            }
        }
    }
}

/// `host` (GRAMMAR.md §3.81): `"0.0.0.0"` por default -- mismo comportamiento
/// que antes de esta ronda -- o `"127.0.0.1"`/una IP puntual vía
/// `--host`/`LINK_HOST`, para no depender ÚNICAMENTE del firewall del
/// sistema operativo como capa de defensa cuando el proceso no necesita
/// aceptar conexiones desde fuera de la máquina (detrás de un proxy en el
/// mismo host, por ejemplo).
///
/// `max_body_bytes` (GRAMMAR.md §3.85): cuántos bytes de BODY acepta como
/// máximo cualquier request -- ver `handle_request` para el porqué (hasta
/// esta ronda se leía el body entero a memoria sin ningún límite, un vector
/// real de agotamiento de memoria).
///
/// `http_timeout` (GRAMMAR.md §3.86): cuánto puede tardar cualquier llamada
/// saliente (`http.get`/`post`/`getWithHeaders`/etc.) antes de abortar --
/// `ureq` no tiene timeout de lectura/escritura por default, así que sin
/// esto una request a un servidor lento o colgado bloqueaba para SIEMPRE
/// (desde GRAMMAR.md §3.158/v1.114.0, solo el hilo de ESA request -- salvo
/// dentro de un `transaction{}`, que sí sigue bloqueando a las demás).
///
/// `trust_proxy` (GRAMMAR.md §3.89): si `@rate_limit` (GRAMMAR.md §3.39)
/// puede usar `X-Forwarded-For` para identificar al cliente -- `false` por
/// default (usa `remote_addr()`, la conexión TCP real). `true` SOLO cuando
/// `linkc serve` corre detrás de un proxy de confianza (nginx, un load
/// balancer) que sobreescribe ese header con el valor real -- sin esto,
/// cualquier cliente directo podría mandar el header que quiera y evadir el
/// límite por completo.
/// `hsts` (GRAMMAR.md §3.143): el valor de `Strict-Transport-Security` a
/// mandar en toda respuesta, o `None` (default) para no mandarlo. `linkc
/// serve` nunca termina TLS por sí solo -- este flag es un opt-in explícito
/// para cuando el operador SABE que un proxy/balanceador de confianza
/// termina TLS delante (mismo espíritu que `trust_proxy`, arriba: una
/// garantía externa que c-script no puede verificar por su cuenta, así que
/// hace falta pedirla a propósito en vez de asumirla).
///
/// GRAMMAR.md §3.92: devuelve `Err` en vez de panic!/`process::exit` en los
/// dos fallos RECUPERABLES conocidos (puerto ya ocupado, Postgres caído al
/// arrancar) -- antes de esta ronda, cualquiera de los dos tumbaba el
/// PROCESO entero, que para `linkc serve` (un solo servicio) no importaba,
/// pero para `linkc serve-all` (§3.92, varios servicios en un mismo
/// proceso) se llevaría por delante a servicios sanos junto con el que
/// falló. El caller (`main.rs::run_serve_with_backoff`) decide qué hacer con
/// el `Err`: `linkc serve` sin `--restart-backoff` sigue terminando el
/// proceso con código 1 (comportamiento idéntico al de siempre, solo que
/// ahora vía un mensaje limpio en vez de un panic con backtrace), y
/// `serve-all` nunca termina el proceso por un solo servicio caído.
pub fn serve(
    program: &Program,
    host: &str,
    port: u16,
    source: DbSource,
    db_schema: Option<String>,
    cors: CorsConfig,
    session_ttl: Option<Duration>,
    argon2_params: argon2::Params,
    encryption_key: Option<String>,
    jwt_config: Option<(String, String, String)>,
    adopt_existing: bool,
    max_body_bytes: u64,
    http_timeout: Duration,
    trust_proxy: bool,
    service_api_key: Option<String>,
    log: LogConfig,
    hsts: Option<String>,
    mcp_secret: Option<String>,
) -> Result<(), String> {
    let server = tiny_http::Server::http((host, port)).map_err(|e| format!("no se pudo iniciar el servidor en {host}:{port}: {e}"))?;
    // Db::new(&program, &db_path), NO Db::seeded(): una colección real
    // (persistida en `db_path`, GRAMMAR.md §3.17) por cada una que el
    // programa DECLARA. `Db::seeded()` es un fixture de tests/demo que
    // inserta una colección "users" hardcodeada e ignora el programa por
    // completo -- que fuera lo que usaba el servidor real era un bug
    // encontrado en la auditoría, con dos síntomas confirmados: un programa
    // con `db { items: Item[] }` tipaba y después daba 500 "colección
    // desconocida: 'items'" en cada rpc; y uno que sí declaraba `users`
    // pero con otra forma recibía los campos del `User` del demo, que su
    // propio tipo no tiene.
    // `remote_changes`: `Some` solo con Postgres (GRAMMAR.md §3.44) -- el
    // otro lado de LISTEN/NOTIFY, drenado más abajo en el loop principal.
    // SQLite no tiene ningún mecanismo de notificación cross-proceso, así
    // que ahí es `None` y el loop se queda con el `incoming_requests()`
    // bloqueante de siempre, sin overhead de polling.
    let (db, remote_changes) = match source {
        DbSource::SqliteFile(db_path) => (Db::new_with_options(program, &db_path, adopt_existing), None),
        // A diferencia de abrir un archivo local, conectarse a una base remota
        // falla por motivos operativos normales (está caída, la clave cambió,
        // la base no existe todavía). Eso merece un mensaje que se entienda,
        // devuelto como `Err` -- no un exit del proceso entero, ver el
        // comentario de `serve` arriba.
        DbSource::Postgres(url) => match Db::connect_postgres_with_options(program, &url, adopt_existing, db_schema.as_deref()) {
            Ok((db, rx)) => (db, Some(rx)),
            Err(e) => {
                return Err(format!("error: {e}\n       revisá la URL de conexión (LINK_DATABASE_URL o --db) y que la base esté levantada"));
            }
        },
    };
    // GRAMMAR.md §3.191: si el programa declara algún campo `@encrypted`,
    // hace falta una clave real ANTES de aceptar la primera request --
    // fallar acá, con un mensaje claro, es mejor que fallar recién en el
    // primer insert/update sobre esa colección (o, peor, guardar el valor
    // sin cifrar en silencio). `Checker::build_symbols` es barato y ya es
    // el patrón establecido para esto (mismo que `invoke_rpc_with_sessions`
    // reconstruye por invocación) -- no hace falta el `Checker` completo
    // que `db` ya tiene adentro, y no está expuesto desde acá.
    let (checker_for_encryption, _) = crate::checker::Checker::build_symbols(program);
    let has_encrypted_fields = !encrypted_fields_by_collection(program, &checker_for_encryption).is_empty();
    let encryption_key = match encryption_key {
        Some(raw) => {
            Some(encryption::parse_encryption_key(&raw).map_err(|e| format!("--encryption-key/LINK_ENCRYPTION_KEY inválida: {e}"))?)
        }
        None if has_encrypted_fields => {
            return Err(
                "el programa declara al menos un campo '@encrypted', pero no se configuró --encryption-key/LINK_ENCRYPTION_KEY (GRAMMAR.md §3.191)"
                    .to_string(),
            );
        }
        None => None,
    };
    db.set_encryption_key(encryption_key);
    // GRAMMAR.md §3.55: costo de `crypto.hashPassword` para lo que quede de
    // vida del proceso -- default de la crate si no se pasó ningún flag/env
    // var (ver `resolve_argon2_params` en `main.rs`).
    db.set_argon2_params(argon2_params);
    // GRAMMAR.md §3.86: mismo criterio que `argon2_params`, una sola vez
    // antes de aceptar la primera request.
    db.set_http_timeout(http_timeout);
    // Auth v0 (GRAMMAR.md §3.14): vive mientras el proceso corre, igual que
    // `db` -- sin persistencia entre reinicios. Sin expiración por default;
    // `--session-ttl`/`LINK_SESSION_TTL` (GRAMMAR.md §3.50) la agrega.
    let sessions = match session_ttl {
        Some(ttl) => SessionStore::with_ttl(ttl),
        None => SessionStore::new(),
    };
    // Auth externo (GRAMMAR.md §3.64): verificar JWTs HS256 emitidos por un
    // backend YA existente, además de -- nunca en vez de -- las sesiones
    // propias de arriba.
    let sessions = match jwt_config {
        Some((secret, role_claim, user_id_claim)) => sessions.with_jwt(secret, role_claim, user_id_claim),
        None => sessions,
    };
    // MCP real (GRAMMAR.md §3.203): guardado en el store mismo (no un
    // parámetro suelto por llamada) para que `role_for`/`user_id_for`
    // reconozcan un `Mcp-Session-Id` de forma transparente -- ver el
    // comentario de `SessionStore::mcp_secret` (session.rs).
    let sessions = match &mcp_secret {
        Some(secret) => sessions.with_mcp_secret(secret.clone()),
        None => sessions,
    };
    let backend = if db.is_postgres() { "PostgreSQL" } else { "SQLite" };
    // `@route` (GRAMMAR.md §3.37): armada UNA vez al arrancar, nunca por
    // request -- el programa ya pasó el checker antes de llegar a `serve`,
    // así que build_route_table no puede encontrar nada inválido acá.
    let route_table = build_route_table(&program);
    // `@rate_limit` (GRAMMAR.md §3.39): un solo `RateLimiter` para todo el
    // proceso, igual criterio que `route_table` de arriba.
    let rate_limiter = RateLimiter::new();
    // `@idempotent` (GRAMMAR.md §3.140): mismo criterio que `rate_limiter`
    // de arriba -- un solo store para todo el proceso.
    let idempotency_store = IdempotencyStore::new();
    // `@cache` (GRAMMAR.md §3.144): mismo criterio que `idempotency_store`
    // de arriba.
    let cache_store = CacheStore::new();
    // `GET /metrics` (GRAMMAR.md §3.149): mismo criterio que los tres de
    // arriba.
    let metrics_store = MetricsStore::new();
    println!("c-script server escuchando en http://localhost:{port}  (datos en {backend}, Ctrl+C para detener)");

    // Pilar 1 del roadmap de concurrencia (26/08/2026, a partir del pedido
    // de skynet-d3): un hilo por request, no el loop de un solo hilo de
    // siempre -- así que TODO lo que una request puede tocar necesita
    // cruzar de forma segura al hilo que la va a atender. `Arc` para lo que
    // ya es internamente seguro entre hilos (`Db`/`SessionStore`, ver sus
    // propios candados) o inmutable después de este punto
    // (`program`/`route_table`); `Arc<Mutex<...>>` para lo que se sigue
    // mutando por request (los cuatro stores de arriba) -- cada uno con su
    // PROPIO candado, así que un rate-limit check de una request nunca
    // espera a que otra termine de escribir en el cache, por ejemplo.
    // `db`/`sessions` en particular sostienen su candado SOLO durante la
    // operación puntual que lo pide (o, para `transaction { }`, durante
    // toda su duración vía `Db::with_exclusive_connection`) -- nunca
    // durante una llamada `http.*` saliente, que no los toca en absoluto:
    // una pasarela de pago lenta en una request ya no bloquea a las demás.
    let program = std::sync::Arc::new(program.clone());
    let db = std::sync::Arc::new(db);
    let sessions = std::sync::Arc::new(sessions);
    let route_table = std::sync::Arc::new(route_table);
    let rate_limiter = std::sync::Arc::new(parking_lot::Mutex::new(rate_limiter));
    let idempotency_store = std::sync::Arc::new(parking_lot::Mutex::new(idempotency_store));
    let cache_store = std::sync::Arc::new(parking_lot::Mutex::new(cache_store));
    let metrics_store = std::sync::Arc::new(parking_lot::Mutex::new(metrics_store));

    // `@cron("Ns"/"Nm"/"Nh"/"Nd")` (GRAMMAR.md §3.159): un hilo dedicado
    // POR tarea, spawneado una sola vez acá, nunca por request (a
    // diferencia de `spawn_handler!` más abajo). Duerme el intervalo
    // COMPLETO antes de la primera corrida (mismo criterio que
    // `setInterval` de JS) -- así arrancar `serve`/`serve-all` con varias
    // tareas no las dispara todas a la vez contra la base en el instante
    // 0. Un error del cuerpo (`RuntimeError`, `@check`/`@unique`, un panic)
    // se loguea y el loop SIGUE -- una corrida fallida nunca apaga la tarea
    // entera ni el servidor.
    //
    // GRAMMAR.md §3.164: `invoke_rpc_with_sessions` va envuelto en
    // `catch_unwind` -- el comentario de arriba prometía esto desde
    // GRAMMAR.md §3.159, pero antes de esta ronda era falso para el caso
    // panic: un panic real (no un `RuntimeError`) atraviesa el `match Ok/Err`
    // sin tocarlo y sigue desenrollando -- `std::thread::spawn` no tiene
    // ningún `catch_unwind` propio, así que el unwind se lleva puesto TODO
    // el hilo: el `loop` nunca vuelve a `std::thread::sleep`, la tarea
    // simplemente deja de correr para siempre, sin ninguna línea de log ni
    // métrica que lo marque -- silencio total, indistinguible de "todavía no
    // le tocaba el turno". Encontrado auditando esta misma sección.
    // `AssertUnwindSafe` acá es seguro por la misma razón que en
    // `Expr::Transaction` (runtime/mod.rs, GRAMMAR.md §3.163): lo que
    // garantiza la limpieza no es que el compilador pueda probar
    // `UnwindSafe`, es que el brazo `Err` de abajo loguea+registra la
    // métrica de fallo exactamente igual que un `RuntimeError` normal, y el
    // `loop` sigue -- no queda ningún candado sostenido ni estado a medio
    // mutar que le importe a la corrida SIGUIENTE (cada corrida arranca su
    // propia transacción/conexión desde cero vía `invoke_rpc_with_sessions`).
    for item in program.items.iter() {
        let Item::Service(service) = item else { continue };
        for member in &service.members {
            let Member::Rpc(rpc) = member else { continue };
            let Some(raw_interval) = rpc.cron() else { continue };
            let interval = crate::cron::parse_interval(raw_interval)
                .expect("check_cron_annotation (checker.rs) ya validó este formato en compilación");
            let method = format!("{}.{}", service.name, rpc.name);
            let service_name = service.name.clone();
            let rpc_name = rpc.name.clone();
            let program = std::sync::Arc::clone(&program);
            let db = std::sync::Arc::clone(&db);
            let sessions = std::sync::Arc::clone(&sessions);
            let metrics_store = std::sync::Arc::clone(&metrics_store);
            std::thread::spawn(move || loop {
                std::thread::sleep(interval);
                let start = std::time::Instant::now();
                let no_args = serde_json::Value::Object(serde_json::Map::new());
                let unwind_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    invoke_rpc_with_sessions(&program, &service_name, &rpc_name, &no_args, &db, &sessions, None)
                }));
                match unwind_result {
                    Ok(Ok(_)) => {
                        metrics_store.lock().record_cron_run(&method, true);
                        log_cron_tick(log, &method, true, start.elapsed(), "");
                    }
                    Ok(Err(e)) => {
                        metrics_store.lock().record_cron_run(&method, false);
                        log_cron_tick(log, &method, false, start.elapsed(), &format!("error={:?}", e.message));
                    }
                    Err(payload) => {
                        let msg = super::panic_payload_message(&*payload);
                        metrics_store.lock().record_cron_run(&method, false);
                        log_cron_tick(log, &method, false, start.elapsed(), &format!("panic={msg:?}"));
                    }
                }
            });
        }
    }

    // Un hilo por request (Pilar 1, arriba) -- cada `Arc`/`Arc<Mutex<...>>`
    // se clona (barato: un incremento de refcount, nunca una copia real de
    // los datos) DENTRO del loop, una vez por request, y esa copia es lo
    // único que el hilo nuevo captura -- así el hilo no depende de que el
    // loop principal (o el `Server` mismo) siga vivo mientras lo atiende.
    // `cors`/`hsts`/`service_api_key` son pequeños (un enum con un
    // `Vec<String>` a lo sumo, dos `Option<String>`) -- clonarlos por
    // request es más simple que otro `Arc` y el costo es insignificante.
    macro_rules! spawn_handler {
        ($request:expr) => {{
            let program = std::sync::Arc::clone(&program);
            let db = std::sync::Arc::clone(&db);
            let sessions = std::sync::Arc::clone(&sessions);
            let route_table = std::sync::Arc::clone(&route_table);
            let rate_limiter = std::sync::Arc::clone(&rate_limiter);
            let idempotency_store = std::sync::Arc::clone(&idempotency_store);
            let cache_store = std::sync::Arc::clone(&cache_store);
            let metrics_store = std::sync::Arc::clone(&metrics_store);
            let cors = cors.clone();
            let hsts = hsts.clone();
            let service_api_key = service_api_key.clone();
            let mcp_secret = mcp_secret.clone();
            let request = $request;
            std::thread::spawn(move || {
                handle_request(
                    &program,
                    &db,
                    &sessions,
                    &route_table,
                    &rate_limiter,
                    &idempotency_store,
                    &cache_store,
                    &metrics_store,
                    &cors,
                    hsts.as_deref(),
                    max_body_bytes,
                    trust_proxy,
                    service_api_key.as_deref(),
                    log,
                    mcp_secret.as_deref(),
                    request,
                );
            });
        }};
    }

    match remote_changes {
        None => {
            for request in server.incoming_requests() {
                spawn_handler!(request);
            }
            // Inalcanzable en la práctica -- `incoming_requests()` solo
            // termina si el `Server` se apaga desde OTRO hilo (`.unblock()`),
            // algo que `serve` nunca hace hoy. Existe para que el tipo de
            // retorno sea honesto (`Result`, no un `loop {}` que tipa `!`).
            Ok(())
        }
        Some(remote_rx) => {
            // Además de aceptar requests, hay que drenar los cambios que
            // anunciaron OTRAS instancias (GRAMMAR.md §3.44) -- por eso
            // `recv_timeout` en vez del `incoming_requests()` bloqueante de
            // siempre: sin esto, un cambio remoto podría quedar esperando
            // indefinidamente si no llega ninguna request HTTP nueva que
            // "despierte" al loop. Este loop en sí sigue siendo UN solo
            // hilo -- lo único que cambia con el Pilar 1 es que YA NO
            // procesa la request en línea, la delega a un hilo nuevo y
            // sigue enseguida a la próxima vuelta.
            loop {
                while let Ok(change) = remote_rx.try_recv() {
                    // GRAMMAR.md §3.150: latencia real de propagación --
                    // `sent_at_ms` viajó en el propio payload del NOTIFY
                    // (armado por la instancia que escribió), nunca un
                    // valor local inventado. `max(0, ...)` por si los
                    // relojes de las dos instancias están levemente
                    // desalineados -- una latencia negativa no tiene
                    // sentido y solo ensuciaría el promedio.
                    let latency_ms = (now_ms() - change.sent_at_ms).max(0);
                    metrics_store.lock().record_notify_latency(std::time::Duration::from_millis(latency_ms as u64));
                    db.publish_remote(&change.collection, change.event);
                }
                // GRAMMAR.md §3.150: reintenta cualquier NOTIFY que haya
                // fallado por una conexión caída transitoria -- mismo tick
                // que ya drena `remote_rx` arriba, sin ningún hilo/timer
                // nuevo.
                db.flush_pending_notify_retries();
                match server.recv_timeout(REMOTE_CHANGE_POLL_INTERVAL) {
                    Ok(Some(request)) => spawn_handler!(request),
                    Ok(None) => {}
                    Err(e) => eprintln!("error aceptando una conexión: {e}"),
                }
            }
        }
    }
}

/// Cada cuánto el loop principal vuelve a revisar el canal de cambios
/// remotos cuando no llegó ninguna request HTTP nueva mientras tanto
/// (GRAMMAR.md §3.44) -- lo bastante seguido para que la propagación
/// cross-instancia no se sienta atrasada en un servidor inactivo, sin
/// gastar CPU despertando el loop con más frecuencia de la que hace falta.
const REMOTE_CHANGE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);

/// El cuerpo de siempre del loop principal, extraído a función para poder
/// llamarse desde las dos formas de esperar la próxima request (bloqueante
/// de siempre, o con timeout cuando además hay que drenar cambios remotos)
/// sin duplicar esta lógica -- exactamente la clase de divergencia entre
/// dos copias del mismo código que este proyecto viene evitando desde
/// GRAMMAR.md §3.9.
#[allow(clippy::too_many_arguments)]
fn handle_request(
    program: &Program,
    db: &Db,
    sessions: &SessionStore,
    route_table: &[RouteEntry],
    rate_limiter: &parking_lot::Mutex<RateLimiter>,
    idempotency_store: &parking_lot::Mutex<IdempotencyStore>,
    cache_store: &parking_lot::Mutex<CacheStore>,
    metrics_store: &parking_lot::Mutex<MetricsStore>,
    cors: &CorsConfig,
    hsts: Option<&str>,
    max_body_bytes: u64,
    trust_proxy: bool,
    service_api_key: Option<&str>,
    log: LogConfig,
    mcp_secret: Option<&str>,
    mut request: tiny_http::Request,
) {
    // Resuelto UNA vez por request, antes de cualquier otra cosa --
    // hasta el preflight OPTIONS lo necesita (GRAMMAR.md §3.41).
    let request_origin = request
        .headers()
        .iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case("Origin"))
        .map(|h| h.value.as_str().to_string());
    let path = request.url().to_string();
    // `@cors("...")` (GRAMMAR.md §3.147): resuelve (service, rpc) del PATH
    // solo, SIN el body -- `resolve_route` con un body vacío es seguro para
    // esto: la rama `@route` nunca toca `body`, y la rama `/Service/rpc`
    // solo lo usa para armar los ARGUMENTOS, no para decidir cuál rpc es
    // (lo único que hace falta acá). Necesario ANTES del preflight OPTIONS
    // de abajo -- un override por ruta tiene que aplicar tanto al preflight
    // como a la respuesta real, o el browser nunca deja pasar la request
    // real para un origen que el override permite pero el CORS global no.
    let cors_override =
        resolve_route(&path, "", route_table).ok().and_then(|(service_name, rpc_name, _)| required_cors(&program, service_name, rpc_name));
    let effective_cors_config = cors_override.map(parse_cors_override);
    let cors: &CorsConfig = effective_cors_config.as_ref().unwrap_or(cors);
    let mut cors_headers = cors.headers_for(request_origin.as_deref());
    // `--hsts`/`LINK_HSTS` (GRAMMAR.md §3.143): constante para todo el
    // proceso -- se copia acá una vez por request, en la MISMA bolsa que
    // ya viaja a cada respuesta, en vez de agregar un parámetro más a los
    // 16 call-sites de `cors_response`/`cors_response_with_type`.
    cors_headers.hsts = hsts.map(str::to_string);

    if *request.method() == tiny_http::Method::Options {
        let resp = cors_response(204, String::new(), &cors_headers, &request);
        let _ = request.respond(resp);
        return;
    }

    let req_id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    let start = std::time::Instant::now();
    // Nivel `Info` fijo -- mismo criterio que un 2xx/3xx en `log_done`, así
    // que a `--log-level info` (el default) esta línea sigue imprimiéndose
    // SIEMPRE, byte a byte igual que antes de esta ronda; solo se suprime
    // pidiendo `warn`/`error` explícitamente.
    if LogLevel::Info >= log.level {
        match log.format {
            LogFormat::Text => println!("[req {req_id}] {} {path}", request.method()),
            LogFormat::Json => {
                println!("{}", serde_json::json!({"req_id": req_id, "http_method": request.method().to_string(), "path": path}))
            }
        }
    }

    // `--service-api-key`/`LINK_SERVICE_API_KEY` (GRAMMAR.md §3.93): un
    // secreto compartido que autentica al LLAMADOR (típicamente un gateway
    // servidor-a-servidor, GRAMMAR.md §3.93), una capa distinta y ANTERIOR a
    // `@requires`/JWT (que autentican a un USUARIO final) -- corre antes de
    // leer el body siquiera, para rechazar rápido sin gastar memoria en un
    // caller no autorizado. `/health`/`/`/`/status` quedan EXENTOS a
    // propósito: un orquestador/load balancer que hace liveness probing no
    // tiene por qué conocer el secreto del gateway.
    if let Some(expected) = service_api_key {
        if path != "/" && path != "/health" && path != "/status" {
            let provided = request
                .headers()
                .iter()
                .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case("X-Service-Api-Key"))
                .map(|h| h.value.as_str().to_string());
            let ok = provided.as_deref().is_some_and(|p| super::constant_time_eq(p.as_bytes(), expected.as_bytes()));
            if !ok {
                let resp = cors_response(
                    401,
                    error_json("falta o es inválido el header X-Service-Api-Key -- este servidor requiere autenticación servidor-a-servidor"),
                    &cors_headers,
                    &request,
                );
                let _ = request.respond(resp);
                log_done(log, req_id, None, 401, start, "error=\"service api key\"");
                return;
            }
        }
    }

    // `--max-body-bytes`/`LINK_MAX_BODY_BYTES` (GRAMMAR.md §3.85): hasta
    // esta ronda esto era `request.as_reader().read_to_string(&mut body)`
    // SIN NINGÚN límite -- un vector real de agotamiento de memoria, un
    // solo body enorme (a propósito o no) se leía entero antes de que
    // ningún otro chequeo (auth, rate limit, forma del JSON) tuviera
    // oportunidad de rechazarlo. `.take(max_body_bytes + 1)` acota la
    // lectura -- el `+ 1` es lo que permite distinguir "el body mide
    // EXACTO el límite" (permitido) de "el body sigue después del límite"
    // (rechazado), sin leer más allá de un solo byte de más en ningún
    // caso. No se intenta drenar el resto de un body rechazado -- ver
    // "Límites honestos" en GRAMMAR.md §3.85 sobre el único costo real de
    // esa simplicidad (una conexión keep-alive reusada después de un 413
    // puede confundir al servidor con el resto del body viejo, que
    // responde 400 y cierra -- nunca un colgado ni una fuga de memoria).
    let mut body = String::new();
    let _ = request.as_reader().take(max_body_bytes + 1).read_to_string(&mut body);
    if body.len() as u64 > max_body_bytes {
        let resp = cors_response(
            413,
            error_json(&format!("el body de la request supera el límite configurado ({max_body_bytes} bytes) -- ver --max-body-bytes/LINK_MAX_BODY_BYTES")),
            &cors_headers,
            &request,
        );
        let _ = request.respond(resp);
        // Todavía no se resolvió ningún `service.rpc` (eso pasa recién al
        // parsear el body) -- `None`, mismo criterio que cualquier rechazo
        // que ocurre antes de llegar tan lejos.
        log_done(log, req_id, None, 413, start, "");
        return;
    }

    // `request.rawBody()`/`request.header()` (GRAMMAR.md §3.38): fijado
    // ACÁ, antes de cualquier dispatch, así que para cuando un rpc
    // corre -- sea por `/Servicio/rpc` o por una `@route` -- el
    // contexto siempre es el de ESTA request. La próxima iteración lo
    // sobreescribe antes de que su propio dispatch corra, así que nunca
    // hace falta limpiarlo a mano entre medio.
    let headers: Vec<(String, String)> =
        request.headers().iter().map(|h| (h.field.as_str().as_str().to_string(), h.value.as_str().to_string())).collect();
    db.set_request_context(super::db::RequestContext { raw_body: body.clone(), headers });

    if path == "/" || path == "/health" || path == "/status" {
        let services: Vec<String> = program
            .items
            .iter()
            .filter_map(|it| match it {
                crate::ast::Item::Service(s) => Some(s.name.clone()),
                _ => None,
            })
            .collect();
        // GRAMMAR.md §3.87: `SELECT 1` real contra la base -- hasta esta
        // ronda `/health` devolvía 200 fijo sin importar si la base
        // respondía o no, inútil para cualquier orquestador que lo usa para
        // decidir si reiniciar el proceso o sacarlo de un load balancer.
        let (status, db_status) = match db.health_check() {
            Ok(()) => (200, serde_json::json!("ok")),
            Err(e) => (503, serde_json::json!(e)),
        };
        let health_json = serde_json::json!({
            "status": if status == 200 { "ok" } else { "error" },
            "engine": "c-script",
            // Del Cargo.toml, no escrita a mano: la versión que reporta el
            // servidor tiene que ser la del binario que está corriendo, y
            // una constante suelta se queda vieja en el primer release.
            "version": crate::VERSION,
            "services": services,
            "database": db_status,
        })
        .to_string();
        let resp = cors_response(status, health_json, &cors_headers, &request);
        let _ = request.respond(resp);
        log_done(log, req_id, Some("health"), status, start, "");
        return;
    }

    // `GET /metrics` (GRAMMAR.md §3.149): formato de exposición de
    // Prometheus. A diferencia de `/health`, NO está exento de
    // `--service-api-key` (arriba) -- los volúmenes/latencias por rpc son
    // más sensibles que un simple "¿está vivo?", así que si el operador
    // configuró esa capa, Prometheus también tiene que mandarla (soportado
    // nativamente por `scrape_configs.authorization` en `prometheus.yml`).
    if path == "/metrics" {
        // AUDIT-2026-08-27.md #9: por orden de evaluación de Rust, el
        // receptor de una llamada a método se evalúa ANTES que sus
        // argumentos, y el `MutexGuard` temporal que devuelve `.lock()`
        // sigue vivo hasta el final de la sentencia completa -- así que
        // llamar a `render_prometheus_text` directo sobre `.lock()` sostenía
        // `metrics_store` durante TODA la evaluación de sus argumentos,
        // incluyendo `db.size_bytes()` (un `PRAGMA` real en SQLite, o un
        // round-trip de red `SELECT pg_database_size(...)` en Postgres),
        // que a su vez pide el mismo candado de conexión que `transaction{}`/
        // `upsert` sostienen por toda su duración (§3.158). Un `GET
        // /metrics` que cae en medio de una transacción larga quedaba
        // bloqueado sosteniendo `metrics_store` -- y cualquier OTRO hilo que
        // necesitara ese candado (el registro de un rechazo de
        // `@rate_limit`, el registro final de cada request normal) quedaba
        // en cola detrás, sin relación ninguna con `/metrics` en sí. Fix:
        // calcular los tres valores ANTES de tomar el candado, sostenerlo
        // solo para el formateo (puro cómputo en memoria).
        let subscriber_counts = db.subscriber_counts();
        let size_bytes = db.size_bytes();
        let oversized_notify_drop_counts = db.oversized_notify_drop_counts();
        let metrics_text = metrics_store.lock().render_prometheus_text(&subscriber_counts, size_bytes, &oversized_notify_drop_counts);
        let resp = cors_response_with_type(200, metrics_text, "text/plain; version=0.0.4", &cors_headers, None, None, &request);
        let _ = request.respond(resp);
        log_done(log, req_id, Some("metrics"), 200, start, "");
        return;
    }

    // MCP real (GRAMMAR.md §3.203) -- solo existe con `--mcp-jwt-secret`/
    // `LINK_MCP_JWT_SECRET` configurado; sin eso, `path == "/mcp"` cae
    // directo al 404 normal de `resolve_route` de abajo, como cualquier
    // otro path que no matchea nada -- MCP deshabilitado es indistinguible
    // de MCP inexistente, mismo criterio que el resto de los flags
    // opcionales de este servidor.
    if mcp_secret.is_some() {
        if path == "/mcp" {
            let mcp_session_id = request
                .headers()
                .iter()
                .find(|h| h.field.equiv(super::mcp::SESSION_HEADER))
                .map(|h| h.value.as_str().to_string());
            match *request.method() {
                tiny_http::Method::Post => {
                    let result = super::mcp::handle_post(
                        &program,
                        &db,
                        &sessions,
                        extract_bearer_token(&request).as_deref(),
                        mcp_session_id.as_deref(),
                        &body,
                    );
                    let mut resp = cors_response(result.status, result.body.to_string(), &cors_headers, &request);
                    if let Some(session_id) = result.new_session_id {
                        if let Ok(header) = tiny_http::Header::from_bytes(&b"Mcp-Session-Id"[..], session_id.as_bytes()) {
                            resp = resp.with_header(header);
                        }
                    }
                    let _ = request.respond(resp);
                    log_done(log, req_id, Some("mcp"), result.status, start, "");
                    return;
                }
                tiny_http::Method::Delete => {
                    let status = super::mcp::handle_delete_session(&sessions, mcp_session_id.as_deref());
                    let resp = cors_response(status, String::new(), &cors_headers, &request);
                    let _ = request.respond(resp);
                    log_done(log, req_id, Some("mcp"), status, start, "");
                    return;
                }
                // `GET /mcp` (la conexión SSE de larga duración) es la
                // Pieza C -- todavía no conectada en esta versión.
                _ => {
                    let resp =
                        cors_response(501, error_json("GET /mcp (streaming) llega en una pieza futura de esta misma ronda -- GRAMMAR.md §3.203"), &cors_headers, &request);
                    let _ = request.respond(resp);
                    log_done(log, req_id, Some("mcp"), 501, start, "");
                    return;
                }
            }
        }
    }

    let (service_name, rpc_name, args_json) = match resolve_route(&path, &body, &route_table) {
        Ok(resolved) => resolved,
        Err(None) => {
            let resp = cors_response(404, error_json("URL debe tener la forma /Service/method"), &cors_headers, &request);
            let _ = request.respond(resp);
            log_done(log, req_id, None, 404, start, "");
            return;
        }
        Err(Some(msg)) => {
            let resp = cors_response(400, error_json(&msg), &cors_headers, &request);
            let _ = request.respond(resp);
            log_done(log, req_id, None, 400, start, &format!("error={msg:?}"));
            return;
        }
    };
    let method = format!("{service_name}.{rpc_name}");

    // `@cron` (GRAMMAR.md §3.159): nunca alcanzable vía HTTP -- el checker
    // ya garantiza que nunca coexiste con `@route`, pero el path por
    // defecto `POST /{Service}/{rpc}` de arriba encuentra cualquier rpc por
    // NOMBRE sin mirar sus anotaciones. 404, no 403 -- desde afuera, este
    // rpc "no existe" como endpoint, exactamente como uno mal escrito.
    if is_cron_member(&program, service_name, rpc_name) {
        let resp = cors_response(404, error_json("no existe ese rpc"), &cors_headers, &request);
        let _ = request.respond(resp);
        log_done(log, req_id, Some(&method), 404, start, "");
        return;
    }

    // `@rate_limit` (GRAMMAR.md §3.39): corre ANTES del gate de auth de
    // abajo, a propósito -- si corriera después, un rpc protegido dejaría
    // probar credenciales sin límite alguno (401 no cuesta nada). La IP
    // sale de la conexión TCP real (`remote_addr`) por default, o de
    // `X-Forwarded-For` SOLO si `--trust-proxy`/`LINK_TRUST_PROXY` lo pide
    // explícitamente (GRAMMAR.md §3.89) -- ver `client_ip_for_rate_limit`.
    if let Some((raw_spec, key_param)) = required_rate_limit(&program, service_name, rpc_name) {
        let spec = RateLimitSpec::parse(raw_spec)
            .expect("check_rate_limit_annotation (checker.rs) ya validó este formato en compilación");
        let client_ip = client_ip_for_rate_limit(&request, trust_proxy);
        // `key: <param>` (GRAMMAR.md §3.142): la clave del bucket pasa de
        // "solo IP" a "IP + valor del parámetro nombrado" -- el separador
        // `|` no aparece en una IP real ni en la mayoría de los valores
        // reales, así que dos (ip, valor) distintos no colisionan por
        // casualidad en el mismo bucket. `RateLimiter` no cambia nada de su
        // lado -- sigue recibiendo un solo string de identidad, como
        // siempre.
        let bucket_identity = match key_param {
            Some(param_name) => {
                let value = args_json.get(param_name).map(extra_rate_limit_key_as_string).unwrap_or_default();
                format!("{client_ip}|{param_name}={value}")
            }
            None => client_ip,
        };
        // GRAMMAR.md §3.178: si `db` tiene la tabla interna de rate
        // limiting distribuido lista (Postgres, sin --adopt-existing sin
        // esa tabla ya creada), el límite se aplica contra el bucket REAL
        // compartido por todas las instancias -- no uno por proceso.
        // `None` (SQLite, o degradado por algún motivo) cae al
        // `RateLimiter` en memoria de siempre, comportamiento IDÉNTICO al
        // de antes de esta ronda.
        let allowed = match db.check_rate_limit_distributed(&bucket_identity, service_name, rpc_name, spec) {
            Some(allowed) => allowed,
            None => rate_limiter.lock().check(&bucket_identity, service_name, rpc_name, spec),
        };
        if !allowed {
            metrics_store.lock().record_rate_limit_rejection(&method);
            let resp = cors_response(429, error_json("demasiadas requests, probá de nuevo en un momento"), &cors_headers, &request);
            let _ = request.respond(resp);
            log_done(log, req_id, Some(&method), 429, start, "");
            return;
        }
    }

    // El gate de autorización corre ACÁ, antes de `parse_args`/
    // `json_to_typed_value` en cualquiera de las dos ramas de abajo --
    // un rpc protegido rechaza la request sin filtrar el shape de sus
    // parámetros a través de un 400 detallado antes de que el caller
    // pruebe estar autorizado (GRAMMAR.md §3.14).
    let token = extract_bearer_token(&request);
    let auth_gate = check_auth_gate(&program, &sessions, token.as_deref(), service_name, rpc_name);
    if let Err((status, msg)) = auth_gate.outcome {
        let resp = cors_response(status, error_json(msg), &cors_headers, &request);
        let _ = request.respond(resp);
        log_done_with_audit(log, req_id, Some(&method), status, start, &format!("error={msg:?}"), auth_gate.audit.as_ref());
        return;
    }
    let auth_audit = auth_gate.audit;

    // `@requires(..., ownerOf: <colección>, id: <parámetro>, field: <campo>)`
    // (GRAMMAR.md §3.190) -- etapa NUEVA Y SEPARADA del chequeo de rol de
    // arriba, que no cambia en nada: mismo costo cero para el caso común sin
    // cláusula (`ownership: None`, el `if let` de abajo ni entra). El rol ya
    // se confirmó acá, así que un id mal formado es un 400 normal, sin
    // riesgo de fuga nueva. Streams quedan fuera de alcance v0 -- una
    // suscripción de larga vida re-chequeando dueño por evento es un
    // problema distinto (límite honesto documentado en GRAMMAR.md §3.190).
    if !is_stream_member(&program, service_name, rpc_name) {
        if let Some(Annotation::Requires { ownership: Some(clause), .. }) = required_auth(&program, service_name, rpc_name) {
            let (checker, _) = crate::checker::Checker::build_symbols(&program);
            if let Err((status, msg)) = check_resource_ownership(clause, &args_json, &checker, db, sessions, token.as_deref()) {
                let resp = cors_response(status, error_json(&msg), &cors_headers, &request);
                let _ = request.respond(resp);
                log_done_with_audit(log, req_id, Some(&method), status, start, &format!("error={msg:?}"), auth_audit.as_ref());
                return;
            }
        }
    }

    if is_stream_member(&program, service_name, rpc_name) {
        // Push real v0 (GRAMMAR.md §3.16): si el cuerpo matchea el
        // shape reconocido (`ast::recognize_live_subscribe`), esto NUNCA
        // llega a invocar `invoke_rpc_with_sessions` -- `Db::subscribe`
        // (sincrónico, en el hilo de esta request) da la foto inicial + un
        // `Receiver` que el hilo escritor bloquea leyendo para siempre.
        // Cualquier otro stream sigue el camino de List<T> de siempre,
        // sin cambios, más abajo.
        if let Some(collection) = live_subscribe_collection(&program, service_name, rpc_name) {
            match db.subscribe(collection) {
                Ok((snapshot, events)) => {
                    let cors_headers = cors_headers.clone();
                    std::thread::spawn(move || write_live_stream(request, snapshot, events, cors_headers, req_id, method, start, log));
                }
                Err(e) => {
                    let status = status_for(&e);
                    let msg = e.to_string();
                    let resp = cors_response(status, error_json(&msg), &cors_headers, &request);
                    let _ = request.respond(resp);
                    log_done_with_audit(log, req_id, Some(&method), status, start, &format!("error={msg:?}"), auth_audit.as_ref());
                }
            }
            return;
        }

        // `args_json` ya viene resuelto de `resolve_route` de arriba (el
        // mismo body-parseado-como-JSON de siempre: un `stream` nunca
        // puede tener `@route`, el checker lo rechaza, así que la tabla
        // de rutas nunca matchea acá -- no hace falta volver a parsear.
        //
        // invoke_rpc_with_sessions corre ACÁ, en el hilo de esta request --
        // ver el porqué en el comentario de arriba del módulo. Lo único
        // que cruza al hilo de escritura es `elements` (ya JSON puro) y
        // `request`.
        let elements = match invoke_rpc_with_sessions(&program, service_name, rpc_name, &args_json, &db, &sessions, token.as_deref()) {
            Ok(json) => json.as_array().cloned().expect(
                "check_rpc (checker.rs) exige que el cuerpo de un stream sea List<T> -- invoke_rpc no puede devolver otra cosa acá",
            ),
            Err(e) => {
                let status = status_for(&e);
                let msg = e.to_string();
                let resp = cors_response(status, error_json(&msg), &cors_headers, &request);
                let _ = request.respond(resp);
                log_done_with_audit(log, req_id, Some(&method), status, start, &format!("error={msg:?}"), auth_audit.as_ref());
                return;
            }
        };
        std::thread::spawn(move || write_stream(request, elements, cors_headers, req_id, method, start, log));
        return;
    }

    // `@idempotent` (GRAMMAR.md §3.140): opt-in por REQUEST, no por rpc --
    // si el caller no manda `Idempotency-Key`, este bloque entero es un
    // no-op y el rpc corre exactamente como si la anotación no existiera.
    // Corre DESPUÉS del gate de auth de arriba: repetir una respuesta
    // grabada sigue exigiendo estar autorizado para pedirla, mismo criterio
    // que el resto de la request.
    let idempotency_key = if required_idempotent(&program, service_name, rpc_name) { extract_idempotency_key(&request) } else { None };
    if let Some(key) = &idempotency_key {
        let request_hash = hash_request_body(&body);
        // AUDIT-2026-08-27.md #4/GRAMMAR.md §3.166: `reserve` es una única
        // operación atómica (revisar + marcar en vuelo bajo el MISMO
        // candado) -- antes, `lookup`+`store` eran dos adquisiciones
        // separadas con el cuerpo entero corriendo sin ningún candado entre
        // medio, así que dos requests concurrentes con la misma clave veían
        // las dos un `Miss` y las dos corrían el cuerpo (confirmado en vivo:
        // 30 requests concurrentes insertaron 2 filas para un solo cargo).
        match idempotency_store.lock().reserve(service_name, rpc_name, key, &request_hash) {
            Lookup::Hit { status, body: cached_body, content_type } => {
                let resp = cors_response_with_type(status, cached_body, &content_type, &cors_headers, None, None, &request);
                let _ = request.respond(resp);
                log_done_with_audit(log, req_id, Some(&method), status, start, "idempotent=\"replayed\"", auth_audit.as_ref());
                db.clear_request_context();
                return;
            }
            Lookup::Conflict => {
                let msg = format!(
                    "'Idempotency-Key: {key}' ya se usó en '{method}' con un body distinto -- generá una clave nueva para una operación distinta"
                );
                let resp = cors_response(409, error_json(&msg), &cors_headers, &request);
                let _ = request.respond(resp);
                log_done_with_audit(log, req_id, Some(&method), 409, start, &format!("error={msg:?}"), auth_audit.as_ref());
                db.clear_request_context();
                return;
            }
            Lookup::InFlight => {
                let msg = format!(
                    "'Idempotency-Key: {key}' ya tiene una request en vuelo para '{method}' -- esperá a que termine antes de reintentar"
                );
                let resp = cors_response(409, error_json(&msg), &cors_headers, &request);
                let _ = request.respond(resp);
                log_done_with_audit(log, req_id, Some(&method), 409, start, "idempotent=\"in-flight\"", auth_audit.as_ref());
                db.clear_request_context();
                return;
            }
            Lookup::Reserved => {}
        }
    }

    // `@cache("60s")` (GRAMMAR.md §3.144): cache del lado del SERVIDOR,
    // keyeado por (service, rpc, argumentos) -- dimensión ORTOGONAL a
    // `@idempotent` de arriba (esa es opt-in por request vía un header del
    // CLIENTE, esta es automática y transparente, sin ningún header). La
    // clave usa el JSON de `args_json` tal cual llegó (sin canonicalizar
    // orden de claves) -- mismo criterio ya aceptado del lado del cache de
    // Query en `hooks.ts` (`JSON.stringify(params)`, GRAMMAR.md §3.124).
    let cache_ttl = required_cache(&program, service_name, rpc_name);
    let cache_key = cache_ttl.map(|_| args_json.to_string());
    if let (Some(_), Some(key)) = (cache_ttl, &cache_key) {
        if let Some((status, body, content_type)) = cache_store.lock().get(service_name, rpc_name, key) {
            let resp = cors_response_with_type(status, body, &content_type, &cors_headers, None, None, &request);
            let _ = request.respond(resp);
            log_done_with_audit(log, req_id, Some(&method), status, start, "cache=\"hit\"", auth_audit.as_ref());
            db.clear_request_context();
            return;
        }
    }

    let (status, response_body, response_type, response_location, response_cache_control) =
        handle_rpc(&program, &db, &sessions, token.as_deref(), service_name, rpc_name, args_json);
    // `@idempotent`: solo se graba un ÉXITO (2xx) -- un error no se graba,
    // para que el caller pueda corregir y reintentar con la MISMA clave
    // (GRAMMAR.md §3.140, mismo criterio que Stripe: la clave protege
    // contra duplicar una operación que funcionó, no contra reintentar una
    // que falló). Location/Cache-Control de esta respuesta no se recuerdan
    // -- un hit repite status+body+content-type, alcance v0 deliberado.
    if let Some(key) = &idempotency_key {
        if (200..300).contains(&status) {
            let request_hash = hash_request_body(&body);
            idempotency_store.lock().complete(service_name, rpc_name, key, &request_hash, status, response_body.clone(), response_type.clone());
        } else {
            // `reserve()` (arriba) siempre deja la clave marcada EN VUELO --
            // si el cuerpo terminó en error, hay que liberarla acá, no solo
            // "no grabar nada": sin este `release`, la clave se queda
            // `InFlight` hasta `IN_FLIGHT_STALE_AFTER` (120s) aunque el
            // intento ya haya terminado, y un reintento inmediato con la
            // misma clave (el caso de uso central de `@idempotent`: corregí
            // y reintentá) recibiría 409 en vez de poder correr de nuevo.
            idempotency_store.lock().release(service_name, rpc_name, key);
        }
    }
    // `@cache`: mismo criterio de "solo se graba un éxito" que `@idempotent`
    // -- un error no queda cacheado, así que el próximo caller vuelve a
    // intentar el cuerpo real en vez de recibir una falla vieja repetida.
    if let (Some(raw_ttl), Some(key)) = (cache_ttl, &cache_key) {
        if (200..300).contains(&status) {
            let ttl = crate::cache::parse_ttl(raw_ttl).expect("check_cache_annotation (checker.rs) ya validó este formato en compilación");
            cache_store.lock().put(service_name, rpc_name, key, status, response_body.clone(), response_type.clone(), ttl);
        }
    }
    // `response_body` en una falla es `{"error": "<mensaje>"}`
    // (`error_json`, más abajo) -- se extrae el mensaje solo para el
    // log en vez de loguear el JSON completo escapado adentro de otro
    // string (`error="{\"error\":\"...\"}"`, técnicamente correcto
    // pero feo de leer); si el body no tiene esa forma exacta por
    // algún motivo, cae al body crudo en vez de esconder la falla.
    let extra = if status >= 400 {
        let message = serde_json::from_str::<serde_json::Value>(&response_body)
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.as_str().map(str::to_string)))
            .unwrap_or_else(|| response_body.clone());
        format!("error={message:?}")
    } else {
        String::new()
    };
    let resp = cors_response_with_type(
        status,
        response_body,
        &response_type,
        &cors_headers,
        response_location.as_deref(),
        response_cache_control.as_deref(),
        &request,
    );
    let _ = request.respond(resp);
    // GRAMMAR.md §3.149: alcance v0 -- solo el camino de dispatch NORMAL de
    // un rpc suma acá. Un hit de `@idempotent`/`@cache` (arriba, ambos
    // devuelven ANTES de llegar hasta acá) y un `stream` (spawneado en su
    // propio hilo, nunca pasa por esta línea) no se cuentan -- ver la nota
    // completa en GRAMMAR.md §3.149 para el porqué de cada uno.
    metrics_store.lock().record(&method, start.elapsed());
    log_done_with_audit(log, req_id, Some(&method), status, start, &extra, auth_audit.as_ref());
    // Defensa en profundidad, no carga estructural: el `set_request_context`
    // de arriba ya garantiza que la PRÓXIMA request nunca ve el contexto de
    // esta. Limpiarlo acá además evita que sobreviva en memoria más de lo
    // necesario entre el fin de esta request y el arranque de la próxima.
    db.clear_request_context();
}

fn parse_path(path: &str) -> Option<(&str, &str)> {
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match segments.as_slice() {
        [service, rpc] => Some((service, rpc)),
        _ => None,
    }
}

fn parse_args(body: &str) -> Result<serde_json::Value, String> {
    if body.trim().is_empty() {
        Ok(serde_json::json!({}))
    } else {
        serde_json::from_str(body).map_err(|e| format!("JSON inválido: {e}"))
    }
}

/// Una `@route` ya resuelta contra el programa (GRAMMAR.md §3.37, §3.42): a
/// qué (servicio, rpc) apunta, y si cada uno de sus parámetros -- en el
/// MISMO orden que `pattern.param_names()`/lo que devuelve `pattern.matches`
/// -- es `Int` (para convertir el segmento de URL capturado al JSON que
/// `invoke_rpc_with_sessions` espera; el checker ya garantizó que no puede
/// ser otra cosa que `String`/`Int`, así que "no es Int" siempre significa
/// "es String").
struct RouteEntry {
    pattern: RoutePattern,
    service_name: String,
    rpc_name: String,
    param_is_int: Vec<bool>,
    /// `(nombre, es_int, opcional)` para cada parámetro del rpc que NO está
    /// en el path (GRAMMAR.md §3.62) -- se lee de la query string por
    /// nombre. El checker ya garantizó que cada uno es `String`/`Int`/
    /// `String?`/`Int?`.
    query_params: Vec<(String, bool, bool)>,
}

/// Arma la tabla de `@route` UNA vez al arrancar, nunca por request,
/// ordenada por especificidad DESCENDENTE (más segmentos literales fijos
/// primero) -- `resolve_route` hace una sola pasada y devuelve el primer
/// match, así que el ORDEN de esta tabla ES la prioridad de despacho
/// (GRAMMAR.md §3.42). El checker (`check_route_conflicts`) ya garantizó
/// que nunca hay dos entradas con la MISMA especificidad que puedan
/// matchear el mismo path real, así que un empate en el orden de esta
/// tabla nunca puede importar cuál queda primero.
///
/// El checker ya corrió sobre `program` antes de que `serve` pudiera llegar
/// a llamarse (`load_and_check` en main.rs), así que acá no puede aparecer
/// un patrón inválido, un rpc con los parámetros mal, ni dos rutas en
/// conflicto -- si algo de eso pasara, sería un bug en el checker, no una
/// condición operativa a manejar con gracia, de ahí los
/// `expect`/`unwrap_or_else` con panic en vez de propagar un `Result`.
fn build_route_table(program: &Program) -> Vec<RouteEntry> {
    let (checker, _) = crate::checker::Checker::build_symbols(program);
    let mut table = Vec::new();
    for item in &program.items {
        let Item::Service(s) = item else { continue };
        for m in &s.members {
            let Member::Rpc(r) = m else { continue };
            let Some(raw) = r.route() else { continue };
            let pattern = crate::route::parse_route_pattern(raw)
                .unwrap_or_else(|e| panic!("@route inválido llegó a serve() sin que el checker lo rechazara: {e}"));
            let param_is_int: Vec<bool> = pattern
                .param_names()
                .iter()
                .map(|name| {
                    let param = r.params.iter().find(|p| &p.name == name).unwrap_or_else(|| {
                        panic!("@route(\"{raw}\") pide ':{name}' pero '{}' no tiene ese parámetro -- el checker debió haberlo rechazado", r.name)
                    });
                    let ty = checker
                        .resolve_type(&param.ty)
                        .unwrap_or_else(|e| panic!("tipo de parámetro de @route no resolvió en serve() habiendo pasado el checker: {e}"));
                    matches!(ty, crate::types::Type::Int)
                })
                .collect();
            let path_param_names = pattern.param_names();
            let query_params: Vec<(String, bool, bool)> = r
                .params
                .iter()
                .filter(|p| !path_param_names.contains(&p.name.as_str()))
                .map(|p| {
                    let ty = checker
                        .resolve_type(&p.ty)
                        .unwrap_or_else(|e| panic!("tipo de parámetro de @route no resolvió en serve() habiendo pasado el checker: {e}"));
                    let (inner, optional) = match &ty {
                        crate::types::Type::Optional(inner) => (inner.as_ref().clone(), true),
                        other => (other.clone(), false),
                    };
                    (p.name.clone(), matches!(inner, crate::types::Type::Int), optional)
                })
                .collect();
            table.push(RouteEntry { pattern, service_name: s.name.clone(), rpc_name: r.name.clone(), param_is_int, query_params });
        }
    }
    table.sort_by_key(|e| std::cmp::Reverse(e.pattern.specificity()));
    table
}

/// Decodificación percent-encoding mínima (`%XX` -> byte), para un segmento
/// de PATH -- no de query string, así que a propósito no toca `+` (esa
/// convención es de `application/x-www-form-urlencoded`, no de un path).
/// Bytes que terminan formando UTF-8 inválido caen a `from_utf8_lossy`: un
/// slug de URL con basura no es motivo para que el servidor haga nada más
/// dramático que sustituir el caracter de reemplazo.
fn percent_decode(segment: &str) -> String {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
            match hex {
                Some(byte) => {
                    out.push(byte);
                    i += 3;
                    continue;
                }
                None => {}
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Igual que `percent_decode`, pero para un valor de QUERY STRING, no de
/// path: `+` significa espacio (`application/x-www-form-urlencoded`), a
/// diferencia de un segmento de path, donde no tiene ningún significado
/// especial. Antes de la ronda de query string (§3.62) esta distinción no
/// existía porque nada decodificaba query strings todavía.
fn percent_decode_query_value(segment: &str) -> String {
    percent_decode(&segment.replace('+', " "))
}

/// `a=1&b=hola%20mundo` -> `{"a": "1", "b": "hola mundo"}`. Un par sin `=`
/// (`?flag`) vale como clave con valor `""` -- no es un caso que este
/// lenguaje necesite distinguir de "vino vacío". Claves repetidas: gana la
/// ÚLTIMA (mismo criterio simple que `HashMap::insert`), no hay soporte para
/// arrays de query params -- fuera de alcance a propósito, un query param
/// de `@route` es siempre `String`/`Int` escalar (GRAMMAR.md §3.62).
fn parse_query_string(qs: &str) -> std::collections::HashMap<String, String> {
    qs.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (percent_decode_query_value(k), percent_decode_query_value(v))
        })
        .collect()
}

/// Identificador de cliente que usa `@rate_limit` (GRAMMAR.md §3.39/§3.89)
/// para agrupar requests -- `remote_addr()` (la conexión TCP real) por
/// default, o el PRIMER valor de `X-Forwarded-For` cuando `trust_proxy` es
/// `true` Y el header está presente. `X-Forwarded-For` es una lista
/// separada por comas que cada proxy en la cadena va extendiendo
/// (`cliente, proxy1, proxy2, ...`) -- el primer valor es el más cercano al
/// cliente original, así que es el que corresponde tomar. Alcance v0
/// deliberado: no valida CUÁNTOS proxies hay en el medio ni de qué IP
/// vienen -- confía en el valor completo del header en cuanto
/// `trust_proxy` está prendido, sin un mecanismo más fino de "proxy de
/// confianza por IP/CIDR" (ver GRAMMAR.md §3.89, "Límites honestos").
/// `X-Forwarded-For` ausente (incluso con `trust_proxy` prendido) cae de
/// vuelta a `remote_addr()` -- no hay motivo para tratar eso como "cliente
/// desconocido" cuando la conexión TCP real sigue siendo una IP perfectamente
/// válida para el propósito.
fn client_ip_for_rate_limit(request: &tiny_http::Request, trust_proxy: bool) -> String {
    if trust_proxy {
        let forwarded = request.headers().iter().find(|h| h.field.as_str().as_str().eq_ignore_ascii_case("X-Forwarded-For"));
        if let Some(header) = forwarded {
            if let Some(first) = header.value.as_str().split(',').next() {
                let first = first.trim();
                if !first.is_empty() {
                    return first.to_string();
                }
            }
        }
    }
    request.remote_addr().map(|a| a.ip().to_string()).unwrap_or_else(|| "desconocida".to_string())
}

/// Convierte el valor JSON de un parámetro nombrado por `@rate_limit(...,
/// key: <param>)` (GRAMMAR.md §3.142) a la forma texto que compone la clave
/// del bucket -- el checker ya garantizó que el parámetro es `String`/`Int`,
/// así que las dos ramas cubren todo lo que puede llegar acá; cualquier otra
/// forma (ausente, `null`, tipo inesperado) cae a un string vacío en vez de
/// entrar en pánico -- el peor caso es que ese request puntual comparta
/// bucket con otros del mismo valor "faltante", nunca un crash.
fn extra_rate_limit_key_as_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

/// Resuelve (servicio, rpc, args) para esta request -- por una `@route` si el
/// path matchea alguna, si no por el `/Service/rpc` de siempre leyendo
/// `body` como el JSON de argumentos (GRAMMAR.md §3.37). Las dos direcciones
/// conviven SIEMPRE, ninguna reemplaza a la otra: un rpc con `@route` sigue
/// siendo alcanzable por su dirección normal -- así lo llama el cliente
/// TypeScript generado -- además de por la ruta linda, pensada para un
/// crawler que nunca va a mandar un POST con JSON.
///
/// `Err(None)`: ninguna `@route` matcheó, y el path tampoco tiene la forma
/// `/Service/rpc` -- 404. `Err(Some(msg))`: una `@route` matcheó pero el
/// segmento capturado (o un query param) no convierte al tipo del parámetro,
/// o falta un query param obligatorio, o (mismo camino de siempre) el body
/// de un `/Service/rpc` no es JSON válido -- 400 en todos los casos.
fn resolve_route<'a>(
    path: &'a str,
    body: &str,
    route_table: &'a [RouteEntry],
) -> Result<(&'a str, &'a str, serde_json::Value), Option<String>> {
    // La query string se separa ACÁ, antes de partir en segmentos -- sin
    // esto, un pedido tan común como `/blog/hola-mundo?utm_source=twitter`
    // (cualquier URL real recibe parámetros de tracking tarde o temprano)
    // hacía que "hola-mundo?utm_source=twitter" ENTERO se capturara como el
    // valor de `:slug` -- un bug real, no solo la ausencia de la feature
    // que este bloque agrega (§3.62).
    let (path, query_string) = path.split_once('?').map_or((path, None), |(p, q)| (p, Some(q)));
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    // `route_table` ya viene ordenada por especificidad descendente
    // (`build_route_table`), así que el PRIMER match de esta única pasada
    // es, por construcción, el más específico -- `/blog/featured` le gana a
    // `/blog/:slug`, y eso se generaliza a cualquier combinación de
    // segmentos literales/parámetro (GRAMMAR.md §3.42). `check_route_conflicts`
    // (checker.rs) ya garantiza que nunca hay dos entradas EMPATADAS en
    // especificidad que puedan matchear el mismo path real, así que nunca
    // hay ambigüedad sobre cuál de las que matchean es "la primera".
    let matched = route_table.iter().find_map(|e| e.pattern.matches(&segments).map(|captured| (e, captured)));
    if let Some((entry, captured)) = matched {
        // `captured` está en el mismo orden que `entry.pattern.param_names()`
        // (invariante de `RoutePattern::matches`), que es el mismo orden en
        // que se armó `entry.param_is_int` en `build_route_table` -- así que
        // zippearlos con los nombres da la asociación correcta.
        let mut args = serde_json::Map::new();
        for ((name, raw_segment), is_int) in entry.pattern.param_names().into_iter().zip(captured).zip(&entry.param_is_int) {
            let decoded = percent_decode(&raw_segment);
            let value = if *is_int {
                match decoded.parse::<i64>() {
                    Ok(n) => serde_json::Value::from(n),
                    Err(_) => {
                        return Err(Some(format!(
                            "parámetro de ruta ':{name}' inválido: se esperaba un entero, se recibió '{decoded}'"
                        )));
                    }
                }
            } else {
                serde_json::Value::String(decoded)
            };
            args.insert(name.to_string(), value);
        }
        // Query string (§3.62): cualquier parámetro del rpc que no vino del
        // path. Ausente + opcional -> `null`, mismo criterio que cualquier
        // otro campo opcional del lenguaje; ausente + obligatorio -> 400.
        if !entry.query_params.is_empty() {
            let query_map = query_string.map(parse_query_string).unwrap_or_default();
            for (name, is_int, optional) in &entry.query_params {
                match query_map.get(name) {
                    Some(raw) => {
                        let decoded = percent_decode_query_value(raw);
                        let value = if *is_int {
                            match decoded.parse::<i64>() {
                                Ok(n) => serde_json::Value::from(n),
                                Err(_) => {
                                    return Err(Some(format!(
                                        "parámetro de query '{name}' inválido: se esperaba un entero, se recibió '{decoded}'"
                                    )));
                                }
                            }
                        } else {
                            serde_json::Value::String(decoded)
                        };
                        args.insert(name.clone(), value);
                    }
                    None if *optional => {
                        args.insert(name.clone(), serde_json::Value::Null);
                    }
                    None => {
                        return Err(Some(format!("falta el parámetro de query obligatorio '{name}'")));
                    }
                }
            }
        }
        return Ok((&entry.service_name, &entry.rpc_name, serde_json::Value::Object(args)));
    }
    let (service_name, rpc_name) = parse_path(path).ok_or(None)?;
    let args = parse_args(body).map_err(Some)?;
    Ok((service_name, rpc_name, args))
}

/// El header `Authorization: Bearer <token>`, si vino -- `None` para
/// "faltaba", "el prefijo 'Bearer ' no estaba", o "había más de un header
/// Authorization" (ambigüedad de smuggling entre proxy/origin, tratada como
/// anónima en vez de elegir una arbitrariamente). Nunca panica con un header
/// malformado: cualquier caso raro simplemente cae a `None` (anónimo), que
/// `check_auth_gate` ya sabe traducir a 401 si el rpc lo requiere.
fn extract_bearer_token(request: &tiny_http::Request) -> Option<String> {
    let matches: Vec<&str> =
        request.headers().iter().filter(|h| h.field.equiv("Authorization")).map(|h| h.value.as_str()).collect();
    let [raw] = matches[..] else { return None };
    raw.strip_prefix("Bearer ").map(str::trim).filter(|t| !t.is_empty()).map(str::to_string)
}

/// `Idempotency-Key` (GRAMMAR.md §3.140) -- mismo nombre de header que
/// Stripe usa para el mismo propósito, no un invento propio. `None` cubre
/// tanto "no se mandó" como "se mandó vacío" -- un caller que no opta por
/// esto no ve ningún cambio de comportamiento en un rpc `@idempotent`.
fn extract_idempotency_key(request: &tiny_http::Request) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Idempotency-Key"))
        .map(|h| h.value.as_str().trim().to_string())
        .filter(|k| !k.is_empty())
}

/// ¿Puede ESTA request llamar a `{service_name}.{rpc_name}`? La ÚNICA
/// decisión de autorización de todo el servidor -- vive acá, no en el
/// intérprete (`runtime/mod.rs`), que solo recibe `sessions`/`token` ya
/// resueltos para que `auth.createSession`/`destroySession` funcionen
/// dentro de un cuerpo. Nunca construye ningún `Value` del intérprete: solo
/// compara strings contra lo que `SessionStore` ya guarda.
/// Resultado de `check_auth_gate` -- separa la decisión (`outcome`, lo que
/// de verdad cambia la respuesta) del rastro de auditoría (GRAMMAR.md
/// §3.148: "quién llamó a qué rpc, con qué rol, y si se permitió o
/// denegó"). `audit` es `Some` SOLO cuando el rpc de verdad declaró
/// `@authenticated`/`@requires` -- un rpc público no genera ruido de
/// auditoría, no hay ninguna decisión de autorización que registrar ahí.
pub(crate) struct AuthGateResult {
    pub(crate) outcome: Result<(), (u16, &'static str)>,
    audit: Option<AuthAudit>,
}

struct AuthAudit {
    /// El rol resuelto de la sesión, si había un token válido -- `None` si
    /// la request vino sin token o con uno que no resolvió a ninguna sesión
    /// real (los dos casos que `sessions.role_for` no puede distinguir del
    /// lado de quién llama, mismo criterio que el 401 que ya devuelve).
    role: Option<String>,
    user_id: Option<i64>,
    allowed: bool,
}

/// ¿Puede ESTA request llamar a `{service_name}.{rpc_name}`? La ÚNICA
/// decisión de autorización de todo el servidor -- vive acá, no en el
/// intérprete (`runtime/mod.rs`), que solo recibe `sessions`/`token` ya
/// resueltos para que `auth.createSession`/`destroySession` funcionen
/// dentro de un cuerpo. Nunca construye ningún `Value` del intérprete: solo
/// compara strings contra lo que `SessionStore` ya guarda.
pub(crate) fn check_auth_gate(
    program: &Program,
    sessions: &SessionStore,
    token: Option<&str>,
    service_name: &str,
    rpc_name: &str,
) -> AuthGateResult {
    // `None` cubre "sin anotación" Y "rpc desconocido" -- ese segundo caso
    // lo detecta con el error real `invoke_rpc_with_sessions` más abajo.
    let Some(annotation) = required_auth(program, service_name, rpc_name) else {
        return AuthGateResult { outcome: Ok(()), audit: None };
    };
    let role_info = token.and_then(|tok| sessions.role_for(tok).map(|(enum_, variant)| (tok, enum_, variant)));
    let user_id = token.and_then(|tok| sessions.user_id_for(tok));
    let audit_role = role_info.as_ref().map(|(_, _, variant)| variant.clone());
    let mk_audit = |allowed: bool| Some(AuthAudit { role: audit_role.clone(), user_id, allowed });

    let Some((_, role_enum, role_variant)) = role_info else {
        return AuthGateResult { outcome: Err((401, "se requiere autenticación")), audit: mk_audit(false) };
    };
    let outcome = match annotation {
        Annotation::Authenticated => Ok(()),
        // `role_enum == ""` es el sentinel de `SessionStore::role_for`
        // (GRAMMAR.md §3.64) para "esta sesión viene de un JWT externo, sin
        // ningún enum de c-script asociado" -- matchea por NOMBRE de
        // variante nada más, sin la comparación de identidad de enum que sí
        // aplica a una sesión creada por `auth.createSession(WithId)` desde
        // este mismo programa.
        Annotation::Requires { enum_name, variant_names, .. }
            if (role_enum.is_empty() || &role_enum == enum_name) && variant_names.iter().any(|v| v == &role_variant) =>
        {
            Ok(())
        }
        // A propósito NO nombra el rol exigido en el mensaje -- a
        // diferencia del nombre del rpc (ya público vía el client.ts/
        // contract.d.ts generado), qué rol hace falta para cada operación
        // es política interna del server; regalarla en el body le daría a
        // cualquiera con un token de bajo privilegio un mapeo completo
        // endpoint->rol gratis (hallado en el review adversarial).
        Annotation::Requires { .. } => Err((403, "no tenés permiso para esta operación")),
        // `required_auth` solo devuelve anotaciones de auth: si estas
        // llegaran a matchear, el bug está allá arriba, no acá.
        Annotation::ContentType(_)
        | Annotation::Route(_)
        | Annotation::RateLimit { .. }
        | Annotation::Deprecated(_)
        | Annotation::CacheControl(_)
        | Annotation::Example { .. }
        | Annotation::Invalidates(_)
        | Annotation::Infinite { .. }
        | Annotation::Idempotent
        | Annotation::Cache(_)
        | Annotation::Cors(_)
        | Annotation::Cron(_) => Ok(()),
    };
    AuthGateResult { audit: mk_audit(outcome.is_ok()), outcome }
}

/// `@requires(..., ownerOf: <colección>, id: <parámetro>, field: <campo>)`
/// (GRAMMAR.md §3.190) -- comparación DIRECTA, sin ninguna máquina de
/// expresiones (a diferencia de `@check`/`@unique where`): la forma es FIJA
/// (`campo == currentUserId()`), así que comparar el `Value::Int` leído del
/// struct encontrado contra `sessions.user_id_for(token)` (la MISMA función
/// que `auth.currentUserId()` ya llama) alcanza. `checker` ya la construyó
/// el caller (`Checker::build_symbols`, un solo build por request, mismo
/// criterio que `invoke_rpc_with_sessions` ya usa para el resto de los
/// parámetros) -- el tipo del id se deriva de la PK de la colección
/// (`Checker::db_id_type`), no del parámetro del rpc: el checker ya
/// garantizó en tiempo de compilación que los dos coinciden
/// (`check_requires_ownership_clause`), así que no hace falta volver a
/// buscar el `RpcDecl` acá.
fn check_resource_ownership(
    clause: &crate::ast::OwnershipClause,
    args_json: &serde_json::Value,
    checker: &crate::checker::Checker,
    db: &Db,
    sessions: &SessionStore,
    token: Option<&str>,
) -> Result<(), (u16, String)> {
    let element_ty = checker
        .db_collections()
        .get(&clause.collection)
        .unwrap_or_else(|| unreachable!("check_requires_ownership_clause ya garantizó que '{}' es una colección real", clause.collection));
    let id_ty = crate::checker::Checker::db_id_type(element_ty);
    let Some(id_json) = args_json.get(&clause.id_param) else {
        return Err((400, format!("falta el parámetro '{}'", clause.id_param)));
    };
    let id_value = super::json_to_typed_value(id_json, &id_ty, checker, &clause.id_param).map_err(|e| (status_for(&e), e.to_string()))?;
    let found = db.call(&clause.collection, "find", vec![id_value]).map_err(|e| (status_for(&e), e.to_string()))?;
    // Cualquier otra cosa que no sea `Value::Struct` (`find` devuelve
    // `Value::Null` cuando no hay fila) cuenta como "no encontrado" --
    // defensivo, nunca un panic sobre una forma inesperada.
    let super::Value::Struct(fields) = found else {
        return Err((404, "recurso no encontrado".to_string()));
    };
    let owner_id = fields.iter().find(|(n, _)| n == &clause.field).and_then(|(_, v)| match v {
        super::Value::Int(n) => Some(*n),
        _ => None,
    });
    let current_user_id = token.and_then(|tok| sessions.user_id_for(tok));
    // Mismo mensaje genérico que `check_auth_gate` ya usa para un rol que no
    // matchea -- sin nombrar el motivo (hallado en el review adversarial de
    // esa ronda: nombrarlo le regala a un atacante un mapeo gratis de qué
    // falló).
    if owner_id.is_some() && owner_id == current_user_id {
        Ok(())
    } else {
        Err((403, "no tenés permiso para esta operación".to_string()))
    }
}

/// El Content-Type que declaró el rpc con `@content_type("...")`, si lo hizo
/// (GRAMMAR.md §3.35). Cuando hay uno, el cuerpo de la respuesta es el
/// `String` que devolvió el rpc TAL CUAL -- sin comillas de JSON alrededor --
/// que es lo que permite servir HTML, XML (un sitemap), CSV o texto plano
/// desde un programa c-script.
fn declared_content_type(program: &Program, service_name: &str, rpc_name: &str) -> Option<String> {
    program.items.iter().find_map(|item| match item {
        crate::ast::Item::Service(s) if s.name == service_name => {
            s.members.iter().find_map(|m| match m {
                crate::ast::Member::Rpc(r) if r.name == rpc_name => {
                    r.content_type().map(str::to_string)
                }
                _ => None,
            })
        }
        _ => None,
    })
}

/// Como `declared_content_type`, para `@cache_control("...")` (GRAMMAR.md
/// §3.113) -- estático (viene del AST, no de un override por request como
/// `response.redirect`), así que se resuelve UNA vez por request igual que
/// el Content-Type declarado.
fn declared_cache_control(program: &Program, service_name: &str, rpc_name: &str) -> Option<String> {
    program.items.iter().find_map(|item| match item {
        crate::ast::Item::Service(s) if s.name == service_name => {
            s.members.iter().find_map(|m| match m {
                crate::ast::Member::Rpc(r) if r.name == rpc_name => {
                    r.cache_control().map(str::to_string)
                }
                _ => None,
            })
        }
        _ => None,
    })
}

/// `args_json` ya viene resuelto por `resolve_route` -- del body si la
/// request usó la dirección `/Service/rpc` de siempre, o de un segmento de
/// URL si usó una `@route` (GRAMMAR.md §3.37). Esta función no sabe ni le
/// importa de cuál de los dos vino.
pub(crate) fn handle_rpc(
    program: &Program,
    db: &Db,
    sessions: &SessionStore,
    token: Option<&str>,
    service_name: &str,
    rpc_name: &str,
    args_json: serde_json::Value,
) -> (u16, String, String, Option<String>, Option<String>) {
    match invoke_rpc_with_sessions(program, service_name, rpc_name, &args_json, db, sessions, token) {
        Ok(result) => {
            // `response.setStatus(code)` (GRAMMAR.md §3.46) / `response.
            // redirect(url, permanent)` (GRAMMAR.md §3.111): consumidos ACÁ,
            // una sola vez, solo en el camino de éxito -- un `Err` de abajo
            // nunca llega a este `match`, así que un override que el cuerpo
            // haya pedido antes de fallar simplemente no se usa (queda para
            // que `clear_request_context` lo limpie al final de la request).
            let status = db.take_response_status().unwrap_or(200);
            let location = db.take_response_location();
            let cache_control = declared_cache_control(program, service_name, rpc_name);
            match declared_content_type(program, service_name, rpc_name) {
                // El checker ya garantizó que un rpc con `@content_type` devuelve
                // `String`, así que `as_str()` acá siempre acierta; el fallback
                // existe para no inventar un panic si esa invariante se rompiera.
                Some(ct) => {
                    let text = result.as_str().map(str::to_string).unwrap_or_else(|| result.to_string());
                    (status, text, ct, location, cache_control)
                }
                None => (status, result.to_string(), JSON_CONTENT_TYPE.to_string(), location, cache_control),
            }
        }
        // Un error SIEMPRE sale como JSON, aunque el rpc declare otro
        // Content-Type: el cliente generado espera `{"error": ...}` para
        // cualquier status >= 400, y una página de error en HTML rompería ese
        // contrato justo cuando algo ya salió mal. Un `Location` que un rpc
        // haya pedido ANTES de fallar tampoco se usa -- mismo motivo que el
        // status: una respuesta de error nunca lleva el resultado a medio
        // camino que el cuerpo haya intentado armar. Un `Cache-Control`
        // declarado tampoco se agrega -- una respuesta de error nunca debe
        // quedar cacheada con la política pensada para el camino de éxito.
        Err(e) => (
            status_for(&e),
            error_json(&e.to_string()),
            JSON_CONTENT_TYPE.to_string(),
            None,
            None,
        ),
    }
}

/// Un request que no matchea el contrato declarado es culpa del CLIENTE
/// (400), no del servidor (500) -- devolver 500 haría parecer que el
/// backend se rompió cuando en realidad rechazó correctamente algo mal
/// formado. Es la contraparte servidor del `LinkValidationError` que el
/// cliente generado ya lanza para respuestas que no matchean.
fn status_for(e: &super::RuntimeError) -> u16 {
    match e.kind {
        super::ErrorKind::BadRequest => 400,
        super::ErrorKind::Runtime => 500,
    }
}

/// Corre enteramente en su propio hilo -- pero a diferencia de la primera
/// versión de este código, ya NO invoca `invoke_rpc` (ver el porqué al
/// inicio del módulo: `Value::Closure` rompió `Send` para `Value`/`Db`/
/// `Program`). `elements` ya es JSON puro (`serde_json::Value` no tiene
/// ningún `Rc` adentro), así que lo único que este hilo hace es escribir
/// bytes a un socket -- exactamente lo único que de verdad necesitaba correr
/// aparte (un cliente lento leyendo no debe bloquear al servidor de aceptar
/// otras conexiones).
///
/// Alcance v0 explícito (GRAMMAR.md §3.13): `elements` es una secuencia YA
/// CALCULADA -- invoke_rpc evaluó el cuerpo COMPLETO en el hilo de la
/// request antes de spawnear esto (el checker ya exige que ese cuerpo sea
/// `List<T>`).
/// Esto es lo que sigue corriendo un `stream` que NO matchea el shape de
/// push real reconocido por `live_subscribe_collection` -- ver
/// `write_live_stream`, más abajo, para el caso que sí anuncia eventos
/// futuros de verdad.
/// El preámbulo HTTP de una respuesta SSE, armado a mano (ver el comentario
/// de `write_stream` sobre por qué no `tiny_http::Response`). Comparte los
/// mismos headers de CORS y de seguridad que cualquier otra respuesta del
/// servidor (`cors_response_with_type`, más abajo) -- separado en su propia
/// función para que las dos NO diverjan en qué headers mandan (GRAMMAR.md
/// §3.9), ya que acá no hay forma de reusar el builder de `tiny_http` que sí
/// usa el resto del servidor.
fn sse_preamble(cors: &CorsHeaders) -> String {
    let mut header = String::from(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nTransfer-Encoding: chunked\r\n",
    );
    if let Some(origin) = &cors.allow_origin {
        header.push_str("Access-Control-Allow-Origin: ");
        header.push_str(origin);
        header.push_str("\r\n");
        if cors.vary_origin {
            header.push_str("Vary: Origin\r\n");
        }
    }
    header.push_str("X-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\nReferrer-Policy: no-referrer\r\n");
    if let Some(value) = &cors.hsts {
        header.push_str("Strict-Transport-Security: ");
        header.push_str(value);
        header.push_str("\r\n");
    }
    header.push_str("\r\n");
    header
}

fn write_stream(
    request: tiny_http::Request,
    elements: Vec<serde_json::Value>,
    cors: CorsHeaders,
    req_id: u64,
    method: String,
    start: std::time::Instant,
    log: LogConfig,
) {
    // Escrito a mano en vez de tiny_http::Response + request.respond(): ese
    // camino sólo llama flush() UNA vez, al final (request.rs::respond_impl),
    // sobre un BufWriter::with_capacity(1024, ...) (client.rs) que envuelve
    // el socket real. Confirmado con un spike aislado (no solo lectura de
    // fuente): un Read que produce datos de a poco con sleeps en el medio
    // NO llega incrementalmente por esa vía -- todo el body sale junto,
    // recién al cerrar la respuesta. into_writer() da acceso directo al
    // mismo BufWriter, y un flush() manual por evento SÍ fuerza cada uno al
    // socket en el momento, que es todo lo que un BufWriter::flush() hace
    // (ignora su capacity interno y escribe lo que tenga acumulado).
    //
    // Transfer-Encoding: chunked explícito (framing hecho a mano, sin
    // chunked_transfer::Encoder -- ese vive adentro de Response::raw_print,
    // que es justo el camino que este código bypassea) en vez de confiar
    // en "sin Content-Length + Connection: close = el body termina cuando
    // se cierra la conexión" (válido por RFC 7230 §3.3.3 regla 7, y el
    // primer diseño de este código usaba eso): un spike con un cliente TCP
    // crudo mostró que la conexión sí se cerraba bien, pero probando
    // después con el client.ts GENERADO de verdad (fetch() nativo de
    // Node, sobre undici) el `for await` nunca veía el done:true final y
    // se quedaba colgado esperando más datos -- undici no trata
    // "connection: close sin framing explícito" como señal confiable de
    // fin de body bajo HTTP/1.1. Chunked es la señal que todo cliente
    // HTTP/1.1 (fetch, curl, EventSource) sabe reconocer sin ambigüedad.
    let mut writer = request.into_writer();
    let header = sse_preamble(&cors);
    if writer.write_all(header.as_bytes()).is_err() {
        log_done(log, req_id, Some(&method), 0, start, "client_disconnected=true stage=before_first_byte");
        return;
    }
    let _ = writer.flush();

    let total = elements.len();
    let mut sent = 0usize;
    for element in &elements {
        let frame = format!("data: {element}\n\n");
        match write_chunk(&mut writer, frame.as_bytes()) {
            Ok(()) => sent += 1,
            Err(e) => {
                // BrokenPipe/ConnectionAborted/ConnectionReset, según la
                // plataforma -- el cliente cerró la conexión a mitad de
                // stream. Confirmado con un spike aislado: el PRÓXIMO
                // write() después de que el cliente cierra falla con este
                // error de inmediato, nunca se queda esperando -- así que
                // cortar acá alcanza para no dejar un hilo colgado por una
                // conexión abandonada. No hay nada más que limpiar: la
                // lista ya estaba completamente calculada en memoria de
                // entrada.
                log_done(
                    log,
                    req_id,
                    Some(&method),
                    200,
                    start,
                    &format!("client_disconnected=true kind={:?} sent={sent} total={total}", e.kind()),
                );
                return;
            }
        }
    }
    // Chunk final de longitud 0 -- lo que le dice al cliente "esto es
    // todo" bajo Transfer-Encoding: chunked (mismo terminador que emite
    // chunked_transfer::Encoder internamente, acá escrito a mano por la
    // misma razón que el resto de este framing).
    let _ = writer.write_all(b"0\r\n\r\n").and_then(|_| writer.flush());
    log_done(log, req_id, Some(&method), 200, start, &format!("sent={sent} total={total}"));
}

/// Push real v0 (GRAMMAR.md §3.16): a diferencia de `write_stream`,
/// `events` NO es una secuencia agotada -- es el extremo lector de un
/// canal que `Db::publish` alimenta cada vez que `insert`/`applyPatch`
/// mutan `collection`, desde el hilo PRINCIPAL (nunca desde acá). Este
/// hilo corre `for event in &events` (`Receiver` implementa `Iterator`,
/// bloqueando hasta el próximo mensaje) indefinidamente -- exactamente lo
/// correcto para un "watch" que en efecto nunca termina por su cuenta,
/// mientras el cliente siga conectado. `snapshot` sale primero, como
/// eventos SSE comunes, para que un cliente recién conectado vea el
/// estado actual antes de cualquier cambio futuro.
///
/// Ningún estado nuevo que limpiar al salir: `writer` y `events` se
/// dropean acá (cierran el socket y el extremo lector del canal). Recién
/// en la PRÓXIMA publicación a `collection` es cuando `Db::publish` nota
/// que el `SyncSender` pareja ya no tiene receptor y lo poda -- lazy, no
/// eager (ver `Db::publish`).
fn write_live_stream(
    request: tiny_http::Request,
    snapshot: Vec<serde_json::Value>,
    events: Receiver<serde_json::Value>,
    cors: CorsHeaders,
    req_id: u64,
    method: String,
    start: std::time::Instant,
    log: LogConfig,
) {
    let mut writer = request.into_writer();
    let header = sse_preamble(&cors);
    if writer.write_all(header.as_bytes()).is_err() {
        log_done(log, req_id, Some(&method), 0, start, "client_disconnected=true stage=before_first_byte");
        return;
    }
    let _ = writer.flush();

    let mut sent = 0usize;
    for element in &snapshot {
        if write_chunk(&mut writer, format!("data: {element}\n\n").as_bytes()).is_err() {
            log_done(log, req_id, Some(&method), 200, start, &format!("client_disconnected=true stage=snapshot sent={sent}"));
            return;
        }
        sent += 1;
    }
    // Bloquea ESTE hilo (nunca el principal) hasta el próximo evento --
    // mismo manejo de desconexión que `write_stream`: el próximo write()
    // sobre un socket cerrado falla de inmediato (BrokenPipe/etc.), nunca
    // se queda esperando.
    for event in &events {
        if write_chunk(&mut writer, format!("data: {event}\n\n").as_bytes()).is_err() {
            log_done(log, req_id, Some(&method), 200, start, &format!("client_disconnected=true stage=live sent={sent}"));
            return;
        }
        sent += 1;
    }
    let _ = writer.write_all(b"0\r\n\r\n").and_then(|_| writer.flush());
    log_done(log, req_id, Some(&method), 200, start, &format!("sent={sent}"));
}

/// Un chunk de HTTP chunked transfer encoding: tamaño en hex + CRLF + datos
/// + CRLF (RFC 7230 §4.1).
///
/// `write_all` + `flush` en la MISMA llamada (no solo al final del stream)
/// es lo que hace que esto llegue de verdad incremental al cliente -- ver
/// el comentario más arriba sobre por qué into_writer() en vez de
/// Response::respond().
fn write_chunk(writer: &mut dyn Write, data: &[u8]) -> std::io::Result<()> {
    write!(writer, "{:x}\r\n", data.len())?;
    writer.write_all(data)?;
    writer.write_all(b"\r\n")?;
    writer.flush()
}

fn error_json(message: &str) -> String {
    serde_json::json!({ "error": message }).to_string()
}

const JSON_CONTENT_TYPE: &str = "application/json; charset=utf-8";

/// `true` si la request declaró soportar `gzip` en `Accept-Encoding` (RFC
/// 9110 §12.5.3) -- mismo patrón de lectura de headers que ya usa este
/// archivo para `Origin`/`X-Service-Api-Key` más arriba. Un cliente que no
/// lo manda (o pide otra cosa, `br`/`deflate` sin `gzip`) recibe la
/// respuesta sin comprimir, byte a byte igual que antes de esta ronda.
fn accepts_gzip(request: &tiny_http::Request) -> bool {
    request
        .headers()
        .iter()
        .any(|h| h.field.as_str().as_str().eq_ignore_ascii_case("Accept-Encoding") && h.value.as_str().to_ascii_lowercase().contains("gzip"))
}

/// Bajo qué tamaño de body NO vale la pena comprimir -- el propio overhead
/// de GZIP (cabecera + checksum + tabla de Huffman) puede superar el ahorro
/// real en una respuesta chica. Mismo orden de magnitud que el
/// `gzip_min_length` que la mayoría de servidores reales (nginx, etc.) usan
/// por default.
const GZIP_MIN_BODY_BYTES: usize = 1024;

/// `Some(bytes comprimidos)` si el cliente declaró soportar gzip Y el body
/// supera el umbral mínimo -- `None` en cualquier otro caso (incluido un
/// error de compresión, aunque escribir sobre un `Vec<u8>` en memoria no
/// debería fallar nunca). El body sin comprimir es SIEMPRE una respuesta
/// válida, así que cualquier duda cae para ese lado.
fn maybe_gzip(body: &str, request_accepts_gzip: bool) -> Option<Vec<u8>> {
    if !request_accepts_gzip || body.len() < GZIP_MIN_BODY_BYTES {
        return None;
    }
    use std::io::Write;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(body.as_bytes()).ok()?;
    encoder.finish().ok()
}

fn cors_response(status: u16, body: String, cors: &CorsHeaders, request: &tiny_http::Request) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    cors_response_with_type(status, body, JSON_CONTENT_TYPE, cors, None, None, request)
}

/// Igual que `cors_response` pero con el Content-Type que pidió el rpc
/// (GRAMMAR.md §3.35). Un valor que no sea un header HTTP válido cae de vuelta
/// a JSON en vez de tirar el servidor: el checker ya rechaza el string vacío,
/// pero esto no puede validar todo el universo de tipos MIME.
///
/// CORS y los headers de seguridad de acá abajo (GRAMMAR.md §3.41) van en
/// TODA respuesta, sin excepción -- incluidas las de error: un 401/404/429
/// también necesita `Access-Control-Allow-Origin` para que el browser deje
/// que el cliente generado LEA ese error en vez de reportar un fallo de red
/// genérico sin mensaje.
///
/// `location`: el header `Location` de `response.redirect` (GRAMMAR.md
/// §3.111), `None` en cualquier respuesta que no sea un redirect (incluido
/// TODO camino de error -- ver `handle_rpc`). Mismo criterio de "no tirar
/// el proceso" que `content_type_value`: una URL que no forme un header
/// válido simplemente no se agrega, en vez de un `unwrap()` en el hilo
/// principal del accept-loop -- el runtime ya rechazó CR/LF antes de
/// guardar el override, así que en la práctica esto solo protegería contra
/// un caso que `response.redirect` no debería dejar pasar en absoluto.
///
/// `cache_control`: el header `Cache-Control` de `@cache_control("...")`
/// (GRAMMAR.md §3.113), `None` en cualquier respuesta que no sea un éxito
/// de un rpc que la declare (incluido TODO camino de error, mismo criterio
/// que `location`). El checker ya rechaza un valor vacío en compilación,
/// así que acá solo queda el mismo resguardo defensivo de "no tirar el
/// proceso" que el resto de los headers armados a partir de un `String`.
fn cors_response_with_type(
    status: u16,
    body: String,
    content_type_value: &str,
    cors: &CorsHeaders,
    location: Option<&str>,
    cache_control: Option<&str>,
    request: &tiny_http::Request,
) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    let content_type = tiny_http::Header::from_bytes(&b"Content-Type"[..], content_type_value.as_bytes())
        .unwrap_or_else(|_| {
            tiny_http::Header::from_bytes(&b"Content-Type"[..], JSON_CONTENT_TYPE.as_bytes()).unwrap()
        });
    // GRAMMAR.md §3.180: `Response::from_data`/`from_string` devuelven el
    // MISMO tipo (`Response<Cursor<Vec<u8>>>`) -- por eso las dos ramas
    // (comprimida/sin comprimir) pueden convivir en una sola variable
    // `response` sin duplicar el resto de esta función más abajo.
    let mut response = match maybe_gzip(&body, accepts_gzip(request)) {
        Some(gzipped) => {
            let response = tiny_http::Response::from_data(gzipped).with_status_code(status).with_header(content_type);
            let encoding = tiny_http::Header::from_bytes(&b"Content-Encoding"[..], &b"gzip"[..]).unwrap();
            response.with_header(encoding)
        }
        None => tiny_http::Response::from_string(body).with_status_code(status).with_header(content_type),
    };
    if let Some(url) = location {
        if let Ok(location_header) = tiny_http::Header::from_bytes(&b"Location"[..], url.as_bytes()) {
            response = response.with_header(location_header);
        }
    }
    if let Some(value) = cache_control {
        if let Ok(cache_control_header) = tiny_http::Header::from_bytes(&b"Cache-Control"[..], value.as_bytes()) {
            response = response.with_header(cache_control_header);
        }
    }

    if let Some(origin) = &cors.allow_origin {
        // `.unwrap_or` en vez de `.unwrap()`: a diferencia de los headers de
        // abajo (valores constantes, siempre válidos), este es un `Origin`
        // que en el caso `Allowlist` viene de la request -- `headers_for`
        // ya lo filtró contra CR/LF, pero no vale la pena que una entrada
        // rara tire por un `unwrap()` el hilo que está atendiendo esta
        // request.
        if let Ok(allow_origin) = tiny_http::Header::from_bytes(&b"Access-Control-Allow-Origin"[..], origin.as_bytes()) {
            response = response.with_header(allow_origin);
            let allow_methods =
                tiny_http::Header::from_bytes(&b"Access-Control-Allow-Methods"[..], &b"POST, GET, OPTIONS"[..]).unwrap();
            // "Authorization" agregado para auth v0 (GRAMMAR.md §3.14): sin
            // esto, cualquier browser real rechaza el preflight OPTIONS
            // apenas el cliente generado manda ese header (ver
            // push_fetch_call en ts_emit.rs), y la request real nunca llega
            // a salir -- ni siquiera es que el servidor la rechace, el
            // propio browser la bloquea antes de intentarla.
            let allow_headers =
                tiny_http::Header::from_bytes(&b"Access-Control-Allow-Headers"[..], &b"Content-Type, Authorization"[..])
                    .unwrap();
            response = response.with_header(allow_methods).with_header(allow_headers);
            if cors.vary_origin {
                let vary = tiny_http::Header::from_bytes(&b"Vary"[..], &b"Origin"[..]).unwrap();
                response = response.with_header(vary);
            }
        }
    }

    // Headers de seguridad fijos (GRAMMAR.md §3.41) -- en TODA respuesta,
    // sin depender de `@content_type` ni de si el rpc devuelve HTML:
    //  - `nosniff`: un browser no debe "adivinar" el tipo de un body y
    //    ejecutarlo como algo distinto de lo que dice el Content-Type real.
    //  - `X-Frame-Options: DENY`: ninguna respuesta de este servidor se
    //    puede embeber en un <iframe> de otro sitio (protección clickjacking).
    //  - `Referrer-Policy: no-referrer`: la URL completa de una request a
    //    este servidor (que puede tener datos sensibles en el path o query)
    //    nunca sale en el header `Referer` de un link que salga desde acá.
    // CSP queda afuera a propósito (depende del contenido de cada página,
    // GRAMMAR.md §3.41). HSTS (GRAMMAR.md §3.143) SÍ se manda, pero solo si
    // `--hsts`/`LINK_HSTS` lo configuró explícitamente -- `linkc serve`
    // nunca termina TLS por sí solo, así que sin ese opt-in no hay forma de
    // saber que esta respuesta de verdad viajó (o va a viajar) sobre HTTPS.
    let nosniff = tiny_http::Header::from_bytes(&b"X-Content-Type-Options"[..], &b"nosniff"[..]).unwrap();
    let frame_options = tiny_http::Header::from_bytes(&b"X-Frame-Options"[..], &b"DENY"[..]).unwrap();
    let referrer_policy = tiny_http::Header::from_bytes(&b"Referrer-Policy"[..], &b"no-referrer"[..]).unwrap();
    response = response.with_header(nosniff).with_header(frame_options).with_header(referrer_policy);
    if let Some(value) = &cors.hsts {
        if let Ok(hsts_header) = tiny_http::Header::from_bytes(&b"Strict-Transport-Security"[..], value.as_bytes()) {
            response = response.with_header(hsts_header);
        }
    }
    response
}
