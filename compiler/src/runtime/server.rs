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

use super::db::Db;
use super::{invoke_rpc, is_stream_member};
use crate::ast::Program;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};

/// Un id incremental por request -- lo único que hace falta para poder
/// correlacionar líneas de log una vez que hay más de un hilo escribiendo a
/// stdout al mismo tiempo (cada `stream` corre en el suyo, para la escritura).
/// No es una iniciativa de observabilidad aparte: es lo mínimo que el hilo
/// de escritura hace necesario, ver PLAN.md §4 (Fase 2).
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

pub fn serve(program: Program, port: u16) {
    let server = tiny_http::Server::http(("0.0.0.0", port))
        .unwrap_or_else(|e| panic!("no se pudo iniciar el servidor en el puerto {port}: {e}"));
    // Db::new(&program), NO Db::seeded(): una colección vacía por cada una
    // que el programa DECLARA. `Db::seeded()` es un fixture de tests/demo
    // que inserta una colección "users" hardcodeada e ignora el programa
    // por completo -- que fuera lo que usaba el servidor real era un bug
    // encontrado en la auditoría, con dos síntomas confirmados: un programa
    // con `db { items: Item[] }` tipaba y después daba 500 "colección
    // desconocida: 'items'" en cada rpc; y uno que sí declaraba `users`
    // pero con otra forma recibía los campos del `User` del demo, que su
    // propio tipo no tiene.
    let db = Db::new(&program);
    println!("c-script server escuchando en http://localhost:{port}  (Ctrl+C para detener)");

    for mut request in server.incoming_requests() {
        if *request.method() == tiny_http::Method::Options {
            let _ = request.respond(cors_response(204, String::new()));
            continue;
        }

        let req_id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
        let path = request.url().to_string();
        println!("[req {req_id}] {} {path}", request.method());

        let mut body = String::new();
        let _ = request.as_reader().read_to_string(&mut body);

        let Some((service_name, rpc_name)) = parse_path(&path) else {
            let _ = request.respond(cors_response(404, error_json("URL debe tener la forma /Service/method")));
            println!("[req {req_id}] 404");
            continue;
        };

        if is_stream_member(&program, service_name, rpc_name) {
            let args_json = match parse_args(&body) {
                Ok(v) => v,
                Err(e) => {
                    let _ = request.respond(cors_response(400, error_json(&e)));
                    println!("[req {req_id}] 400");
                    continue;
                }
            };
            // invoke_rpc corre ACÁ, en el hilo principal -- ver el porqué en
            // el comentario de arriba del módulo. Lo único que cruza al
            // hilo de escritura es `elements` (ya JSON puro) y `request`.
            let elements = match invoke_rpc(&program, service_name, rpc_name, &args_json, &db) {
                Ok(json) => json.as_array().cloned().expect(
                    "check_rpc (checker.rs) exige que el cuerpo de un stream sea List<T> -- invoke_rpc no puede devolver otra cosa acá",
                ),
                Err(e) => {
                    let status = status_for(&e);
                    let _ = request.respond(cors_response(status, error_json(&e.to_string())));
                    println!("[req {req_id}] {status}");
                    continue;
                }
            };
            std::thread::spawn(move || write_stream(request, elements, req_id));
            continue;
        }

        let (status, response_body) = handle_rpc(&program, &db, service_name, rpc_name, &body);
        let _ = request.respond(cors_response(status, response_body));
        println!("[req {req_id}] {status}");
    }
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

fn handle_rpc(program: &Program, db: &Db, service_name: &str, rpc_name: &str, body: &str) -> (u16, String) {
    let args_json = match parse_args(body) {
        Ok(v) => v,
        Err(e) => return (400, error_json(&e)),
    };
    match invoke_rpc(program, service_name, rpc_name, &args_json, db) {
        Ok(result) => (200, result.to_string()),
        Err(e) => (status_for(&e), error_json(&e.to_string())),
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
/// Alcance v0 explícito (GRAMMAR.md §2.1, PLAN.md §4): `elements` es una
/// secuencia YA CALCULADA -- invoke_rpc evaluó el cuerpo COMPLETO en el hilo
/// principal antes de spawnear esto (el checker ya exige que ese cuerpo sea
/// List<T>). No hay generación perezosa ni suscripción a cambios futuros --
/// ninguna de las dos es posible todavía (el lenguaje no tiene ningún
/// constructo de loop, y "suscribirse" necesitaría una capa de pub-sub sobre
/// Db que no existe).
fn write_stream(request: tiny_http::Request, elements: Vec<serde_json::Value>, req_id: u64) {
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
    let header = b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nTransfer-Encoding: chunked\r\nAccess-Control-Allow-Origin: *\r\n\r\n";
    if writer.write_all(header).is_err() {
        println!("[req {req_id}] cliente ya desconectado antes del primer byte");
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
                println!(
                    "[req {req_id}] cliente desconectado a mitad de stream (kind={:?}), {sent}/{total} eventos enviados",
                    e.kind()
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
    println!("[req {req_id}] 200 (stream: {sent}/{total} eventos)");
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

fn cors_response(status: u16, body: String) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    let content_type = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
    let allow_origin = tiny_http::Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]).unwrap();
    let allow_methods =
        tiny_http::Header::from_bytes(&b"Access-Control-Allow-Methods"[..], &b"POST, OPTIONS"[..]).unwrap();
    let allow_headers =
        tiny_http::Header::from_bytes(&b"Access-Control-Allow-Headers"[..], &b"Content-Type"[..]).unwrap();
    tiny_http::Response::from_string(body)
        .with_status_code(status)
        .with_header(content_type)
        .with_header(allow_origin)
        .with_header(allow_methods)
        .with_header(allow_headers)
}
