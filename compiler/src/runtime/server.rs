// Servidor HTTP mínimo que expone cada `rpc`/`stream` como POST
// /{Service}/{method}, leyendo argumentos como un objeto JSON (el mismo
// shape que produce client.ts) y devolviendo el resultado serializado. CORS
// abierto porque el frontend de la demo corre en otro puerto (ver
// examples/frontend/).
//
// `stream` (GRAMMAR.md §2.1, streaming real v0): el CÓMPUTO (`invoke_rpc`)
// siempre corre en el loop principal, igual que cualquier `rpc` normal --
// solo la ESCRITURA de los eventos SSE (potencialmente lenta si el cliente
// lee despacio) se manda a un hilo aparte, así no bloquea al servidor de
// aceptar el resto de las conexiones mientras un stream largo todavía se
// está enviando. Diseño revisado durante la ronda de closures (GRAMMAR.md
// §3.10): `Value::Closure` guarda un `Env` (`Rc<RefCell<Value>>>`, no
// `Send`), así que `Value`/`Db`/`Program` ya no pueden cruzar un borde de
// hilo -- de ahí que `invoke_rpc` (que sí los toca) tenga que quedarse en
// el hilo principal, y el hilo de escritura reciba solamente el resultado
// YA CONVERTIDO a `serde_json::Value` (sin ningún `Rc` adentro, `Send` de
// sobra) más el propio `Request` (`Send` por diseño de tiny_http).
//
// Push real v0 (GRAMMAR.md §3.16): un `stream` cuyo cuerpo matchea el
// shape reconocido (`ast::recognize_live_subscribe`) NUNCA pasa por
// `invoke_rpc_with_sessions` -- `live_subscribe_collection` lo detecta
// ANTES, y `Db::subscribe` (sincrónico, hilo principal) da la foto inicial
// más un `Receiver<serde_json::Value>` que el hilo escritor
// (`write_live_stream`) bloquea leyendo indefinidamente. Mismo respeto por
// el límite de `Send` de arriba: lo único que cruza al hilo escritor es
// JSON puro, nunca `Db`/`Value`.

use super::db::Db;
use super::session::SessionStore;
use super::{invoke_rpc_with_sessions, is_stream_member, live_subscribe_collection, required_auth, required_rate_limit};
use crate::ast::{Annotation, Item, Member};
use crate::ast::Program;
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

/// Tracing estructurado por RPC (GRAMMAR.md §2.1, auditoría post-push):
/// una sola línea por request COMPLETADA, formato `clave=valor` (greppable
/// sin parsear JSON, mismo espíritu que el logging de texto de `tracing`/
/// Heroku -- no se suma la dependencia `tracing` para esto, `println!` ya
/// alcanza). `req_id` (existía desde antes, ver arriba) correlaciona esta
/// línea con la de "request recibida"; `method` es `None` para los casos
/// que nunca llegan a resolver `{service}.{rpc}` (ej. un 404 por URL mal
/// formada). `extra` es libre -- `error="..."` en una falla,
/// `sent=N total=M` en un stream, o simplemente vacío en un 200 normal.
fn log_done(req_id: u64, method: Option<&str>, status: u16, start: std::time::Instant, extra: &str) {
    let elapsed_ms = start.elapsed().as_millis();
    let method_field = method.unwrap_or("-");
    if extra.is_empty() {
        println!("[req {req_id}] method={method_field} status={status} duration_ms={elapsed_ms}");
    } else {
        println!("[req {req_id}] method={method_field} status={status} duration_ms={elapsed_ms} {extra}");
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
            CorsConfig::Any => CorsHeaders { allow_origin: Some("*".to_string()), vary_origin: false },
            CorsConfig::Allowlist(list) => {
                let matched = request_origin.filter(|o| list.iter().any(|a| a == o)).map(str::to_string);
                CorsHeaders { allow_origin: matched, vary_origin: true }
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
/// esto una request a un servidor lento o colgado bloqueaba el intérprete
/// (de un solo hilo) para SIEMPRE.
///
/// `trust_proxy` (GRAMMAR.md §3.89): si `@rate_limit` (GRAMMAR.md §3.39)
/// puede usar `X-Forwarded-For` para identificar al cliente -- `false` por
/// default (usa `remote_addr()`, la conexión TCP real). `true` SOLO cuando
/// `linkc serve` corre detrás de un proxy de confianza (nginx, un load
/// balancer) que sobreescribe ese header con el valor real -- sin esto,
/// cualquier cliente directo podría mandar el header que quiera y evadir el
/// límite por completo.
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
    cors: CorsConfig,
    session_ttl: Option<Duration>,
    argon2_params: argon2::Params,
    jwt_config: Option<(String, String, String)>,
    adopt_existing: bool,
    max_body_bytes: u64,
    http_timeout: Duration,
    trust_proxy: bool,
    service_api_key: Option<String>,
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
        DbSource::Postgres(url) => match Db::connect_postgres_with_options(program, &url, adopt_existing) {
            Ok((db, rx)) => (db, Some(rx)),
            Err(e) => {
                return Err(format!("error: {e}\n       revisá la URL de conexión (LINK_DATABASE_URL o --db) y que la base esté levantada"));
            }
        },
    };
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
    let backend = if db.is_postgres() { "PostgreSQL" } else { "SQLite" };
    // `@route` (GRAMMAR.md §3.37): armada UNA vez al arrancar, nunca por
    // request -- el programa ya pasó el checker antes de llegar a `serve`,
    // así que build_route_table no puede encontrar nada inválido acá.
    let route_table = build_route_table(&program);
    // `@rate_limit` (GRAMMAR.md §3.39): un solo `RateLimiter` para todo el
    // proceso, igual criterio que `route_table` de arriba -- se arma/muta
    // en el hilo principal, nunca cruza a los hilos de escritura de stream.
    let mut rate_limiter = RateLimiter::new();
    println!("c-script server escuchando en http://localhost:{port}  (datos en {backend}, Ctrl+C para detener)");

    match remote_changes {
        None => {
            for request in server.incoming_requests() {
                handle_request(
                    &program,
                    &db,
                    &sessions,
                    &route_table,
                    &mut rate_limiter,
                    &cors,
                    max_body_bytes,
                    trust_proxy,
                    service_api_key.as_deref(),
                    request,
                );
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
            // "despierte" al loop.
            loop {
                while let Ok(change) = remote_rx.try_recv() {
                    db.publish_remote(&change.collection, change.event);
                }
                match server.recv_timeout(REMOTE_CHANGE_POLL_INTERVAL) {
                    Ok(Some(request)) => handle_request(
                        &program,
                        &db,
                        &sessions,
                        &route_table,
                        &mut rate_limiter,
                        &cors,
                        max_body_bytes,
                        trust_proxy,
                        service_api_key.as_deref(),
                        request,
                    ),
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
fn handle_request(
    program: &Program,
    db: &Db,
    sessions: &SessionStore,
    route_table: &[RouteEntry],
    rate_limiter: &mut RateLimiter,
    cors: &CorsConfig,
    max_body_bytes: u64,
    trust_proxy: bool,
    service_api_key: Option<&str>,
    mut request: tiny_http::Request,
) {
    // Resuelto UNA vez por request, antes de cualquier otra cosa --
    // hasta el preflight OPTIONS lo necesita (GRAMMAR.md §3.41).
    let request_origin = request
        .headers()
        .iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case("Origin"))
        .map(|h| h.value.as_str().to_string());
    let cors_headers = cors.headers_for(request_origin.as_deref());

    if *request.method() == tiny_http::Method::Options {
        let _ = request.respond(cors_response(204, String::new(), &cors_headers));
        return;
    }

    let req_id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    let start = std::time::Instant::now();
    let path = request.url().to_string();
    println!("[req {req_id}] {} {path}", request.method());

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
                let _ = request.respond(cors_response(
                    401,
                    error_json("falta o es inválido el header X-Service-Api-Key -- este servidor requiere autenticación servidor-a-servidor"),
                    &cors_headers,
                ));
                log_done(req_id, None, 401, start, "error=\"service api key\"");
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
        let _ = request.respond(cors_response(
            413,
            error_json(&format!("el body de la request supera el límite configurado ({max_body_bytes} bytes) -- ver --max-body-bytes/LINK_MAX_BODY_BYTES")),
            &cors_headers,
        ));
        // Todavía no se resolvió ningún `service.rpc` (eso pasa recién al
        // parsear el body) -- `None`, mismo criterio que cualquier rechazo
        // que ocurre antes de llegar tan lejos.
        log_done(req_id, None, 413, start, "");
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
        let _ = request.respond(cors_response(status, health_json, &cors_headers));
        log_done(req_id, Some("health"), status, start, "");
        return;
    }

    let (service_name, rpc_name, args_json) = match resolve_route(&path, &body, &route_table) {
        Ok(resolved) => resolved,
        Err(None) => {
            let _ = request.respond(cors_response(404, error_json("URL debe tener la forma /Service/method"), &cors_headers));
            log_done(req_id, None, 404, start, "");
            return;
        }
        Err(Some(msg)) => {
            let _ = request.respond(cors_response(400, error_json(&msg), &cors_headers));
            log_done(req_id, None, 400, start, &format!("error={msg:?}"));
            return;
        }
    };
    let method = format!("{service_name}.{rpc_name}");

    // `@rate_limit` (GRAMMAR.md §3.39): corre ANTES del gate de auth de
    // abajo, a propósito -- si corriera después, un rpc protegido dejaría
    // probar credenciales sin límite alguno (401 no cuesta nada). La IP
    // sale de la conexión TCP real (`remote_addr`) por default, o de
    // `X-Forwarded-For` SOLO si `--trust-proxy`/`LINK_TRUST_PROXY` lo pide
    // explícitamente (GRAMMAR.md §3.89) -- ver `client_ip_for_rate_limit`.
    if let Some(raw_spec) = required_rate_limit(&program, service_name, rpc_name) {
        let spec = RateLimitSpec::parse(raw_spec)
            .expect("check_rate_limit_annotation (checker.rs) ya validó este formato en compilación");
        let client_ip = client_ip_for_rate_limit(&request, trust_proxy);
        if !rate_limiter.check(&client_ip, service_name, rpc_name, spec) {
            let _ = request.respond(cors_response(429, error_json("demasiadas requests, probá de nuevo en un momento"), &cors_headers));
            log_done(req_id, Some(&method), 429, start, "");
            return;
        }
    }

    // El gate de autorización corre ACÁ, antes de `parse_args`/
    // `json_to_typed_value` en cualquiera de las dos ramas de abajo --
    // un rpc protegido rechaza la request sin filtrar el shape de sus
    // parámetros a través de un 400 detallado antes de que el caller
    // pruebe estar autorizado (GRAMMAR.md §3.14).
    let token = extract_bearer_token(&request);
    if let Err((status, msg)) = check_auth_gate(&program, &sessions, token.as_deref(), service_name, rpc_name) {
        let _ = request.respond(cors_response(status, error_json(msg), &cors_headers));
        log_done(req_id, Some(&method), status, start, &format!("error={msg:?}"));
        return;
    }

    if is_stream_member(&program, service_name, rpc_name) {
        // Push real v0 (GRAMMAR.md §3.16): si el cuerpo matchea el
        // shape reconocido (`ast::recognize_live_subscribe`), esto NUNCA
        // llega a invocar `invoke_rpc_with_sessions` -- `Db::subscribe`
        // (hilo principal, sincrónico) da la foto inicial + un
        // `Receiver` que el hilo escritor bloquea leyendo para siempre.
        // Cualquier otro stream sigue el camino de List<T> de siempre,
        // sin cambios, más abajo.
        if let Some(collection) = live_subscribe_collection(&program, service_name, rpc_name) {
            match db.subscribe(collection) {
                Ok((snapshot, events)) => {
                    let cors_headers = cors_headers.clone();
                    std::thread::spawn(move || write_live_stream(request, snapshot, events, cors_headers, req_id, method, start));
                }
                Err(e) => {
                    let status = status_for(&e);
                    let msg = e.to_string();
                    let _ = request.respond(cors_response(status, error_json(&msg), &cors_headers));
                    log_done(req_id, Some(&method), status, start, &format!("error={msg:?}"));
                }
            }
            return;
        }

        // `args_json` ya viene resuelto de `resolve_route` de arriba (el
        // mismo body-parseado-como-JSON de siempre: un `stream` nunca
        // puede tener `@route`, el checker lo rechaza, así que la tabla
        // de rutas nunca matchea acá -- no hace falta volver a parsear.
        //
        // invoke_rpc_with_sessions corre ACÁ, en el hilo principal --
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
                let _ = request.respond(cors_response(status, error_json(&msg), &cors_headers));
                log_done(req_id, Some(&method), status, start, &format!("error={msg:?}"));
                return;
            }
        };
        std::thread::spawn(move || write_stream(request, elements, cors_headers, req_id, method, start));
        return;
    }

    let (status, response_body, response_type, response_location) =
        handle_rpc(&program, &db, &sessions, token.as_deref(), service_name, rpc_name, args_json);
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
    let _ = request.respond(cors_response_with_type(status, response_body, &response_type, &cors_headers, response_location.as_deref()));
    log_done(req_id, Some(&method), status, start, &extra);
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

/// ¿Puede ESTA request llamar a `{service_name}.{rpc_name}`? La ÚNICA
/// decisión de autorización de todo el servidor -- vive acá, no en el
/// intérprete (`runtime/mod.rs`), que solo recibe `sessions`/`token` ya
/// resueltos para que `auth.createSession`/`destroySession` funcionen
/// dentro de un cuerpo. Nunca construye ningún `Value` del intérprete: solo
/// compara strings contra lo que `SessionStore` ya guarda.
fn check_auth_gate(
    program: &Program,
    sessions: &SessionStore,
    token: Option<&str>,
    service_name: &str,
    rpc_name: &str,
) -> Result<(), (u16, &'static str)> {
    // `None` cubre "sin anotación" Y "rpc desconocido" -- ese segundo caso
    // lo detecta con el error real `invoke_rpc_with_sessions` más abajo.
    let Some(annotation) = required_auth(program, service_name, rpc_name) else {
        return Ok(());
    };
    let Some(tok) = token else {
        return Err((401, "se requiere autenticación"));
    };
    let Some((role_enum, role_variant)) = sessions.role_for(tok) else {
        return Err((401, "se requiere autenticación"));
    };
    match annotation {
        Annotation::Authenticated => Ok(()),
        // `role_enum == ""` es el sentinel de `SessionStore::role_for`
        // (GRAMMAR.md §3.64) para "esta sesión viene de un JWT externo, sin
        // ningún enum de c-script asociado" -- matchea por NOMBRE de
        // variante nada más, sin la comparación de identidad de enum que sí
        // aplica a una sesión creada por `auth.createSession(WithId)` desde
        // este mismo programa.
        Annotation::Requires { enum_name, variant_names }
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
        Annotation::ContentType(_) | Annotation::Route(_) | Annotation::RateLimit(_) | Annotation::Deprecated(_) => Ok(()),
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

/// `args_json` ya viene resuelto por `resolve_route` -- del body si la
/// request usó la dirección `/Service/rpc` de siempre, o de un segmento de
/// URL si usó una `@route` (GRAMMAR.md §3.37). Esta función no sabe ni le
/// importa de cuál de los dos vino.
fn handle_rpc(
    program: &Program,
    db: &Db,
    sessions: &SessionStore,
    token: Option<&str>,
    service_name: &str,
    rpc_name: &str,
    args_json: serde_json::Value,
) -> (u16, String, String, Option<String>) {
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
            match declared_content_type(program, service_name, rpc_name) {
                // El checker ya garantizó que un rpc con `@content_type` devuelve
                // `String`, así que `as_str()` acá siempre acierta; el fallback
                // existe para no inventar un panic si esa invariante se rompiera.
                Some(ct) => {
                    let text = result.as_str().map(str::to_string).unwrap_or_else(|| result.to_string());
                    (status, text, ct, location)
                }
                None => (status, result.to_string(), JSON_CONTENT_TYPE.to_string(), location),
            }
        }
        // Un error SIEMPRE sale como JSON, aunque el rpc declare otro
        // Content-Type: el cliente generado espera `{"error": ...}` para
        // cualquier status >= 400, y una página de error en HTML rompería ese
        // contrato justo cuando algo ya salió mal. Un `Location` que un rpc
        // haya pedido ANTES de fallar tampoco se usa -- mismo motivo que el
        // status: una respuesta de error nunca lleva el resultado a medio
        // camino que el cuerpo haya intentado armar.
        Err(e) => (
            status_for(&e),
            error_json(&e.to_string()),
            JSON_CONTENT_TYPE.to_string(),
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
/// CALCULADA -- invoke_rpc evaluó el cuerpo COMPLETO en el hilo principal
/// antes de spawnear esto (el checker ya exige que ese cuerpo sea `List<T>`).
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
    header.push_str("X-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\nReferrer-Policy: no-referrer\r\n\r\n");
    header
}

fn write_stream(
    request: tiny_http::Request,
    elements: Vec<serde_json::Value>,
    cors: CorsHeaders,
    req_id: u64,
    method: String,
    start: std::time::Instant,
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
        log_done(req_id, Some(&method), 0, start, "client_disconnected=true stage=before_first_byte");
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
    log_done(req_id, Some(&method), 200, start, &format!("sent={sent} total={total}"));
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
) {
    let mut writer = request.into_writer();
    let header = sse_preamble(&cors);
    if writer.write_all(header.as_bytes()).is_err() {
        log_done(req_id, Some(&method), 0, start, "client_disconnected=true stage=before_first_byte");
        return;
    }
    let _ = writer.flush();

    let mut sent = 0usize;
    for element in &snapshot {
        if write_chunk(&mut writer, format!("data: {element}\n\n").as_bytes()).is_err() {
            log_done(req_id, Some(&method), 200, start, &format!("client_disconnected=true stage=snapshot sent={sent}"));
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
            log_done(req_id, Some(&method), 200, start, &format!("client_disconnected=true stage=live sent={sent}"));
            return;
        }
        sent += 1;
    }
    let _ = writer.write_all(b"0\r\n\r\n").and_then(|_| writer.flush());
    log_done(req_id, Some(&method), 200, start, &format!("sent={sent}"));
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

fn cors_response(status: u16, body: String, cors: &CorsHeaders) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    cors_response_with_type(status, body, JSON_CONTENT_TYPE, cors, None)
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
fn cors_response_with_type(
    status: u16,
    body: String,
    content_type_value: &str,
    cors: &CorsHeaders,
    location: Option<&str>,
) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    let content_type = tiny_http::Header::from_bytes(&b"Content-Type"[..], content_type_value.as_bytes())
        .unwrap_or_else(|_| {
            tiny_http::Header::from_bytes(&b"Content-Type"[..], JSON_CONTENT_TYPE.as_bytes()).unwrap()
        });
    let mut response = tiny_http::Response::from_string(body).with_status_code(status).with_header(content_type);
    if let Some(url) = location {
        if let Ok(location_header) = tiny_http::Header::from_bytes(&b"Location"[..], url.as_bytes()) {
            response = response.with_header(location_header);
        }
    }

    if let Some(origin) = &cors.allow_origin {
        // `.unwrap_or` en vez de `.unwrap()`: a diferencia de los headers de
        // abajo (valores constantes, siempre válidos), este es un `Origin`
        // que en el caso `Allowlist` viene de la request -- `headers_for`
        // ya lo filtró contra CR/LF, pero no vale la pena que una entrada
        // rara tire el proceso entero por un `unwrap()` en el hilo principal
        // del accept-loop.
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
    // CSP y HSTS quedan afuera a propósito -- ver GRAMMAR.md §3.41 para el
    // porqué (CSP depende del contenido de cada página; HSTS le corresponde
    // a quien termina TLS, que no es `linkc serve`).
    let nosniff = tiny_http::Header::from_bytes(&b"X-Content-Type-Options"[..], &b"nosniff"[..]).unwrap();
    let frame_options = tiny_http::Header::from_bytes(&b"X-Frame-Options"[..], &b"DENY"[..]).unwrap();
    let referrer_policy = tiny_http::Header::from_bytes(&b"Referrer-Policy"[..], &b"no-referrer"[..]).unwrap();
    response.with_header(nosniff).with_header(frame_options).with_header(referrer_policy)
}
