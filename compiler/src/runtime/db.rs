// "DB tipada" v0 (GRAMMAR.md §3.12) + persistencia real (GRAMMAR.md §3.17):
// el checker conoce la forma real de cada colección declarada en
// `db { ... }` (Type::DbCollection, nunca Dynamic), y el storage detrás es
// SQLite de verdad (`rusqlite`, feature "bundled" -- compila su propio
// SQLite, sin necesitar uno instalado en el sistema ni un proceso de
// servidor externo corriendo aparte, coherente con que `linkc serve` siga
// arrancando solo). El schema SQL de cada colección se DERIVA del
// `Type::Struct` que el checker ya resolvió -- nunca se escribe SQL a mano,
// mismo espíritu que contract.d.ts/client.ts/validators.ts (todos derivados
// de la misma fuente de verdad, cero duplicación manual).

use super::{as_int, json_to_typed_value, simple_enum_names, value_to_json, RuntimeError, Value};
use crate::ast::Program;
use crate::checker::Checker;
use crate::types::{FieldType, Type};
use super::store::{Backend, Cell, ColumnKind};
use rusqlite::Connection;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, SyncSender};

/// Cuántos eventos sin consumir tolera un suscriptor de push real
/// (GRAMMAR.md §3.16) antes de ser desconectado. Un canal ILIMITADO sería
/// un vector real de agotamiento de memoria si un cliente se queda lento
/// (o la conexión se cuelga) sin cerrarse -- `try_send` (nunca bloqueante,
/// ver `publish`) más este tope acotan el costo de un suscriptor colgado a
/// una cantidad fija, a costa de desconectarlo si se atrasa demasiado (el
/// mismo trade-off que la mayoría de sistemas de pub-sub/broadcast reales
/// hacen). No es un número investigado a fondo, es un default razonable.
const LIVE_STREAM_BUFFER: usize = 1024;

/// Cómo se representa UN campo (que no sea `id`) de una colección en SQL --
/// derivado una sola vez por colección, al abrir la conexión, y reusado por
/// cada `insert`/`applyPatch`/lectura (GRAMMAR.md §3.17).
struct ColumnPlan {
    field: FieldType,
    /// Tipo de columna DDL: `"INTEGER"`, `"REAL"` o `"TEXT"`. Cuando
    /// `json` es `true` siempre es `"TEXT"`.
    sql_type: &'static str,
    /// `true` => la columna guarda `serde_json::to_string(value_to_json(v))`
    /// (structs, enums con datos, listas, tuplas, Map, genéricos, uniones,
    /// Result/Patch, o el caso `x?: T?` -- ver `for_field`). `false` => la
    /// columna guarda el valor nativo tal cual (Int/Float/String/Bool/enum
    /// simple).
    json: bool,
}

impl ColumnPlan {
    /// `x?: T?` (opcional-por-clave Y nullable-por-tipo a la vez, GRAMMAR.md
    /// §3.4) es el único caso que SIEMPRE necesita el envoltorio JSON así T
    /// sea nativo: una sola columna SQL solo tiene un bit de NULL, y acá
    /// hacen falta 3 estados (ausente / presente-null / presente-valor). El
    /// texto JSON de `Value::Null` es simplemente `"null"`, así que ese
    /// tercer estado sale gratis de `value_to_json`/`json_to_typed_value`
    /// sin ningún caso especial en el resto de este archivo -- ver
    /// `write_param`/`row_to_fields`.
    fn for_field(field: FieldType, simple_enums: &HashSet<String>) -> Self {
        let double_optional = field.optional && matches!(field.ty, Type::Optional(_));
        let effective_ty: &Type = match &field.ty {
            Type::Optional(inner) => inner.as_ref(),
            other => other,
        };
        match if double_optional { None } else { native_sql_type(effective_ty, simple_enums) } {
            Some(sql_type) => ColumnPlan { field, sql_type, json: false },
            None => ColumnPlan { field, sql_type: "TEXT", json: true },
        }
    }

    fn not_null(&self) -> bool {
        !self.field.optional && !matches!(self.field.ty, Type::Optional(_))
    }

    /// Qué se lee de esta columna. Se deriva del MISMO plan que decide cómo se
    /// escribe, así que lectura y escritura no pueden divergir.
    fn kind(&self) -> ColumnKind {
        if self.json {
            return ColumnKind::Json;
        }
        let effective_ty: &Type = match &self.field.ty {
            Type::Optional(inner) => inner.as_ref(),
            other => other,
        };
        match effective_ty {
            Type::Int | Type::Int64 | Type::Timestamp => ColumnKind::Int,
            Type::Float => ColumnKind::Float,
            Type::Bool => ColumnKind::Bool,
            Type::String | Type::Enum(_) => ColumnKind::Text,
            other => unreachable!("tipo nativo inesperado en una columna no-JSON: {other:?}"),
        }
    }
}

/// `None` para cualquier tipo que no tenga una columna SQL nativa razonable
/// -- structs, enums CON datos, listas, tuplas, Map, genéricos, uniones,
/// Result/Patch. Un enum SIMPLE (todas sus variantes unitarias, `Role` por
/// ejemplo) sí es nativo: se guarda como el nombre de variante en texto
/// plano (`"Admin"`), no envuelto en JSON, para que quede legible desde
/// `sqlite3 archivo.db "select ..."` a mano.
fn native_sql_type(ty: &Type, simple_enums: &HashSet<String>) -> Option<&'static str> {
    match ty {
        Type::Int => Some("INTEGER"),
        // Mismo rango i64 que Int -- SQLite/rusqlite ya son 64-bit nativos
        // acá, sin columna especial (GRAMMAR.md §3.30).
        Type::Int64 => Some("INTEGER"),
        // Milisegundos crudos, INTEGER nativo -- range queries indexadas
        // correctas (`WHERE createdAt > ?`), más robusto que ordenar un
        // string ISO-8601 por lexicografía (GRAMMAR.md §3.31). La
        // conversión a/desde el string ISO-8601 solo pasa en el borde JSON
        // (json_to_typed_value/value_to_json, runtime/mod.rs), nunca acá.
        Type::Timestamp => Some("INTEGER"),
        Type::Float => Some("REAL"),
        Type::String => Some("TEXT"),
        Type::Bool => Some("INTEGER"),
        Type::Enum(name) if simple_enums.contains(name) => Some("TEXT"),
        _ => None,
    }
}

fn create_table_sql(collection: &str, columns: &[ColumnPlan]) -> String {
    let mut defs = vec!["\"id\" INTEGER PRIMARY KEY AUTOINCREMENT".to_string()];

    for col in columns {
        let not_null = if col.not_null() { " NOT NULL" } else { "" };
        defs.push(format!("\"{}\" {}{}", col.field.name, col.sql_type, not_null));
    }
    // STRICT (SQLite >= 3.37, muy por debajo de la versión bundleada de
    // rusqlite 0.40): que SQLite rechace un tipo incompatible en vez de
    // coaccionarlo por type affinity -- defensa en profundidad barata,
    // mismo nivel de rigor que el resto del proyecto ya tiene en otros
    // lados. Solo admite INTEGER/REAL/TEXT/BLOB/ANY, exactamente el
    // vocabulario que esta tabla ya necesita.
    format!("CREATE TABLE IF NOT EXISTS \"{collection}\" ({}) STRICT", defs.join(", "))
}

/// Compara el schema YA GUARDADO en el archivo contra el que el programa
/// actual declara -- falla fuerte ante cualquier diferencia, sin intentar
/// migrar (GRAMMAR.md §3.17). `PRAGMA table_info` sobre una tabla que
/// `CREATE TABLE IF NOT EXISTS` acaba de crear en esta misma llamada
/// siempre matchea por construcción, así que esto solo puede fallar contra
/// un archivo de una corrida ANTERIOR con un schema distinto.
fn check_schema_matches(connection: &Connection, collection: &str, columns: &[ColumnPlan], db_path: &str) -> Result<(), RuntimeError> {
    let mut stmt = connection
        .prepare(&format!("PRAGMA table_info(\"{collection}\")"))
        .map_err(|e| RuntimeError::new(format!("no se pudo leer el schema de '{collection}' en '{db_path}': {e}")))?;
    let existing: Vec<(String, String, bool)> = stmt
        .query_map([], |row| Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, i64>(3)? != 0)))
        .and_then(Iterator::collect)
        .map_err(|e| RuntimeError::new(format!("no se pudo leer el schema de '{collection}' en '{db_path}': {e}")))?;

    // Vacío significa que la tabla la acaba de crear el CREATE TABLE IF NOT
    // EXISTS de arriba en esta misma llamada -- coincide por construcción.
    if existing.is_empty() {
        return Ok(());
    }

    let mut expected: HashMap<String, (String, bool)> = HashMap::new();
    // `id INTEGER PRIMARY KEY` reporta notnull=0 en PRAGMA table_info aunque
    // nunca pueda terminar siendo NULL de verdad (SQLite autoasigna en vez
    // de aceptar NULL) -- si acá se esperara notnull=1, CUALQUIER reinicio
    // detectaría un mismatch falso desde el primer arranque.
    expected.insert("id".to_string(), ("INTEGER".to_string(), false));
    for col in columns {
        expected.insert(col.field.name.clone(), (col.sql_type.to_string(), col.not_null()));
    }
    let mut actual: HashMap<String, (String, bool)> =
        existing.into_iter().map(|(name, decl_type, notnull)| (name, (decl_type.to_uppercase(), notnull))).collect();

    if expected == actual {
        return Ok(());
    }

    // Auto-migración no destructiva (Link 1.0): si hay columnas esperadas que no existen en la tabla física
    // y son opcionales/nullable (no NOT NULL sin default), agregarlas con ALTER TABLE ADD COLUMN sin perder datos
    for col in columns {
        if !actual.contains_key(&col.field.name) && !col.not_null() {
            let alter_sql = format!("ALTER TABLE \"{collection}\" ADD COLUMN \"{}\" {}", col.field.name, col.sql_type);
            if connection.execute(&alter_sql, []).is_ok() {
                actual.insert(col.field.name.clone(), (col.sql_type.to_string(), false));
            }
        }
    }

    if expected == actual {
        return Ok(());
    }

    let describe = |m: &HashMap<String, (String, bool)>| {
        let mut out: Vec<String> = m.iter().map(|(n, (t, nn))| format!("{n} {t}{}", if *nn { " NOT NULL" } else { "" })).collect();
        out.sort();
        out.join(", ")
    };
    Err(RuntimeError::new(format!(
        "la colección '{collection}' en '{db_path}' ya existe pero con un schema incompatible que no se puede migrar automáticamente \
         (esperado: [{}], encontrado: [{}]).",
        describe(&expected),
        describe(&actual),
    )))
}

/// Equivalente de `check_schema_matches` para PostgreSQL, pero acotado a lo
/// único que de verdad puede tirar abajo el servidor si se lo deja pasar: el
/// tipo de "id" en una tabla que YA EXISTÍA antes de este `connect_postgres`.
///
/// `CREATE TABLE IF NOT EXISTS` es un no-op sobre una tabla preexistente --
/// nunca mira sus columnas. Encontrado en producción real: una tabla creada
/// por otro sistema con `id UUID` (típico al migrar desde un backend que ya
/// usaba UUID como clave primaria) dejaba pasar el connect sin ninguna queja,
/// y recién en el primer `insert` -- `RETURNING "id"` leído con `i64` contra
/// una columna `uuid` -- `store.rs::insert_returning_id` panickeaba. Como
/// `handle_rpc` corre sincrónico en el hilo principal del accept-loop
/// (server.rs), ese panic no tiraba abajo solo esa request: tiraba abajo el
/// proceso entero. Acá se falla ANTES de aceptar la primera request, con un
/// mensaje que dice qué pasó y qué hacer -- el mismo momento y el mismo
/// criterio que `check_schema_matches` ya aplica para SQLite.
///
/// A propósito NO se generaliza a validar TODAS las columnas como hace
/// `check_schema_matches`: PostgreSQL es el backend con auto-migración no
/// destructiva (`ALTER TABLE ADD COLUMN IF NOT EXISTS`, ver `connect_postgres`
/// más abajo) -- un tipo distinto en una columna que no sea "id" hoy se
/// descubre en la primera lectura, vía el `try_get` normal de `store.rs`, que
/// devuelve un error limpio, NO un panic (el panic era específico de la
/// variante `Row::get` sin chequear que usaba justo el fetch del id nuevo).
/// "id" es el único caso donde ese error limpio no alcanzaba a tiempo.
fn validate_existing_id_column(backend: &Backend, collection: &str) -> Result<(), String> {
    let sql = format!(
        "SELECT data_type FROM information_schema.columns WHERE table_name = {} AND column_name = 'id'",
        backend.placeholder(1)
    );
    let rows = backend
        .query(&sql, &[Cell::Text(collection.to_string())], &[ColumnKind::Text])
        .map_err(|e| format!("no se pudo verificar el esquema de '{collection}' en PostgreSQL: {e}"))?;

    // Sin fila: o la tabla se acaba de crear (su "id" siempre es BIGSERIAL,
    // por construcción -- nada que validar) o por algún motivo no tiene
    // columna "id" en absoluto, en cuyo caso cualquier find/insert/delete
    // sobre esta colección va a fallar de todos modos con su propio mensaje.
    // Ninguno de los dos casos es este el lugar para inventar uno mejor.
    let Some(Cell::Text(data_type)) = rows.first().and_then(|row| row.first()) else {
        return Ok(());
    };
    if matches!(data_type.as_str(), "bigint" | "integer" | "smallint") {
        return Ok(());
    }
    Err(format!(
        "la tabla '{collection}' ya existe en PostgreSQL con \"id\" de tipo '{data_type}', pero c-script requiere una \
         clave primaria entera autoincremental (BIGSERIAL) -- típico al migrar desde un backend que usaba UUID como id. \
         No se puede usar esta tabla sin migrarla a mano: agregá una columna \"id\" BIGSERIAL nueva, o apuntá esta \
         colección a otro nombre de tabla."
    ))
}

pub struct Db {
    backend: Backend,
    /// Reconstruido UNA vez por vida del servidor (no por request, a
    /// diferencia de `invoke_rpc_with_sessions`) -- hace falta porque
    /// `json_to_typed_value` (usado para decodificar una columna JSON de
    /// vuelta a un `Value` tipado) lo pide para resolver enums/genéricos.
    checker: Checker,
    /// Para que un evento PUBLICADO (`publish`, más abajo) serialice
    /// EXACTAMENTE igual que cualquier respuesta normal del mismo programa
    /// (mismo `value_to_json` que usa `invoke_rpc_with_sessions`).
    simple_enums: HashSet<String>,
    /// Nombre de colección -> plan de columnas (todo menos `id`), derivado
    /// del `Type::Struct` de esa colección al abrir la conexión.
    columns: HashMap<String, Vec<ColumnPlan>>,
    /// Suscriptores activos por colección, para push real (GRAMMAR.md
    /// §3.16). `RefCell`, no `Mutex` -- por la MISMA razón que ya vale para
    /// `SessionStore`: esto solo se toca desde el hilo principal
    /// (`Db::call`/`Db::subscribe`), nunca desde ningún hilo escritor (que
    /// solo recibe el `Receiver`, ya extraído, nunca vuelve a tocar `Db`).
    subscribers: RefCell<HashMap<String, Vec<SyncSender<serde_json::Value>>>>,
    /// Contexto de LA request HTTP que está invocando un rpc ahora mismo --
    /// body crudo + headers, para `request.rawBody()`/`request.header()`
    /// (GRAMMAR.md §3.38). `server.rs` lo fija justo antes de invocar el rpc
    /// y lo limpia apenas termina; nunca sobrevive entre requests. Vive acá
    /// -- no como un parámetro nuevo enhebrado por las ~11 firmas que ya
    /// cargan `db`/`sessions`/`current_token` -- por el mismo motivo que
    /// `subscribers` vive acá: `db: &Db` ya está disponible en CUALQUIER
    /// punto del árbol de evaluación (`call_method`, `eval_expr`, ...), así
    /// que sumar un campo acá es aditivo puro, sin tocar ninguna firma
    /// existente. `RefCell`, mismo criterio de siempre: un solo hilo.
    current_request: RefCell<Option<RequestContext>>,
    /// `response.setStatus(code)` (GRAMMAR.md §3.46) para ESTA request --
    /// mismo criterio que `current_request` (vive acá para no enhebrar un
    /// parámetro nuevo por todas las firmas que ya cargan `db`), pero al
    /// revés: la escribe el CUERPO del rpc, la lee `server.rs` una sola vez
    /// después de que `invoke_rpc` vuelve con éxito. `Cell`, no `RefCell`
    /// -- `Option<u16>` es `Copy`, no hace falta pedir prestado nada.
    response_status_override: std::cell::Cell<Option<u16>>,
    /// Id aleatorio de ESTA instancia del proceso -- solo tiene sentido con
    /// Postgres (GRAMMAR.md §3.44): va adentro de cada `NOTIFY` para que el
    /// hilo de LISTEN de esta MISMA instancia pueda reconocer y descartar
    /// su propio eco (el cambio ya se publicó local, en `publish`, antes de
    /// mandar el NOTIFY). Con SQLite es un string que nunca se usa.
    instance_id: String,
    /// Costo de `crypto.hashPassword` (GRAMMAR.md §3.55, ronda "Argon2id
    /// configurable"): default de la crate hasta que `server.rs` lo
    /// sobreescribe UNA vez al arrancar, según `--argon2-memory-kib`/
    /// `--argon2-iterations` (o sus env vars). Vive acá -- no como un
    /// parámetro nuevo enhebrado por `call_method`/`eval_expr`/... -- mismo
    /// motivo que `current_request`/`response_status_override`: `db: &Db`
    /// ya está disponible en cualquier punto del árbol de evaluación.
    /// `verifyPassword` NO lo necesita: el formato PHC embebe sus propios
    /// `m`/`t`/`p` en el hash guardado, así que verificar un hash viejo con
    /// otros parámetros sigue funcionando sin tocar esto.
    argon2_params: RefCell<argon2::Params>,
}

/// Un cambio anunciado por OTRA instancia de `linkc serve` contra la misma
/// base (GRAMMAR.md §3.44), recibido vía LISTEN/NOTIFY -- `runtime/server.rs`
/// lo drena del canal que devuelve `Db::connect_postgres` y lo vuelve a
/// publicar LOCAL (`Db::publish_remote`), para que un suscriptor conectado a
/// ESTA instancia también lo vea.
pub(crate) struct RemoteChange {
    pub collection: String,
    pub event: serde_json::Value,
}

/// Un solo canal de Postgres para TODOS los cambios de TODAS las
/// colecciones -- el nombre de la colección va DENTRO del payload JSON, no
/// en el nombre del canal, así que hace falta un solo `LISTEN` sin importar
/// cuántas colecciones declare el programa (GRAMMAR.md §3.44).
const REMOTE_CHANGE_CHANNEL: &str = "link_stream_changes";

/// Cuántos cambios remotos sin consumir tolera el canal antes de que el
/// hilo de LISTEN se bloquee mandando el próximo -- mismo criterio y mismo
/// motivo que `LIVE_STREAM_BUFFER`: una cota fija en vez de crecer sin
/// límite si el hilo principal se atrasa procesándolos.
const REMOTE_CHANGE_BUFFER: usize = 1024;

/// Postgres rechaza un payload de `NOTIFY` de más de 8000 bytes -- con
/// margen para el resto del JSON (`instance`/`collection`), no solo el
/// evento. Un cambio más grande que esto simplemente no se propaga a otras
/// instancias (límite honesto, GRAMMAR.md §3.44): partirlo o comprimirlo
/// abriría su propia complejidad para un caso de borde.
const MAX_NOTIFY_PAYLOAD_BYTES: usize = 7900;

/// Un id de instancia nuevo, del CSPRNG del sistema -- mismo origen de
/// entropía que `crypto.uuid()`/`crypto.randomToken` (GRAMMAR.md §3.34), acá
/// sin formatear como UUID porque nunca sale del proceso hacia un humano:
/// es un tag interno para que el hilo de LISTEN de una instancia reconozca
/// (y descarte) su propio `NOTIFY`.
fn random_instance_id() -> String {
    let mut buf = [0u8; 16];
    // Si el CSPRNG del sistema falla acá, algo más grave ya está roto (esta
    // misma llamada nunca falló en `crypto.randomToken`/`crypto.uuid`,
    // donde SÍ se propaga el error porque hay un caller de verdad
    // esperando un `Result`) -- un id de instancia degradado a un patrón
    // fijo en ese caso extremo no cambia la corrección de nada más.
    match getrandom::getrandom(&mut buf) {
        Ok(()) => buf.iter().map(|b| format!("{b:02x}")).collect(),
        Err(_) => "sin-csprng".to_string(),
    }
}

/// Ver la doc de `Db::current_request`. Dos structs (no una tupla) porque
/// `runtime/server.rs` construye esto con nombres de campo, más legible que
/// posiciones.
pub(crate) struct RequestContext {
    pub raw_body: String,
    /// (nombre, valor) tal como llegaron -- la búsqueda por nombre
    /// (`current_request_header`) es case-insensitive, como manda HTTP; acá
    /// se guardan tal cual para no perder la capitalización original en caso
    /// de que algo alguna vez necesite mostrarlos.
    pub headers: Vec<(String, String)>,
}

/// Única forma de abrir una conexión NUEVA a PostgreSQL -- usada tanto por
/// `Db::connect_postgres` (arranque) como por `store::with_reconnect`
/// (después de una conexión perdida), a propósito: dos lugares que abrieran
/// la conexión por su cuenta con criterios distintos de TLS es exactamente
/// la clase de divergencia entre capas que este proyecto viene evitando
/// desde GRAMMAR.md §3.9.
///
/// TLS es rustls puro (crate `tokio-postgres-rustls` + `rustls` con el
/// backend `ring`), no OpenSSL/`native-tls` -- así los 4 targets de release
/// (Linux/Windows/macOS x86_64+ARM) compilan sin instalar ninguna librería
/// del sistema. `sslmode` sale de la URL de conexión (estándar libpq,
/// `postgres::Config` ya lo parsea): `disable` conecta en texto plano tal
/// cual el comportamiento de antes de esta ronda; cualquier otro valor
/// (incluido el default si no se especifica, `prefer`) intenta TLS primero
/// -- y si el servidor no lo ofrece, la propia `tokio-postgres` cae a texto
/// plano sola, sin código extra acá (GRAMMAR.md §3.40).
pub(crate) fn connect_postgres_client(url: &str) -> Result<postgres::Client, String> {
    // rustls exige un crypto provider de proceso instalado ANTES del primer
    // `ClientConfig::builder()` -- se instala UNA vez; `install_default()`
    // devuelve `Err` si ya había uno (llamado desde acá Y desde un
    // reconnect posterior), así que el resultado se ignora a propósito.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let config: postgres::Config = url.parse().map_err(|e| format!("URL de conexión inválida: {e}"))?;
    if config.get_ssl_mode() == postgres::config::SslMode::Disable {
        postgres::Client::connect(url, postgres::NoTls).map_err(|e| format!("no se pudo conectar a PostgreSQL: {e}"))
    } else {
        let tls = tokio_postgres_rustls::MakeRustlsConnect::with_webpki_roots();
        postgres::Client::connect(url, tls).map_err(|e| format!("no se pudo conectar a PostgreSQL: {e}"))
    }
}

/// Arranca el hilo de LISTEN dedicado (GRAMMAR.md §3.44) y devuelve el
/// extremo lector del canal por el que manda cada `RemoteChange` que
/// reconoce como AJENO (no su propio eco -- ver `parse_remote_notification`).
///
/// Conexión SEPARADA de la de queries normales: bloquear esperando
/// notificaciones y ejecutar SELECT/INSERT/UPDATE sincrónicos no pueden
/// compartir una sola conexión de `postgres` (el mismo motivo por el que
/// `store::with_reconnect` nunca toca esta conexión). Si la conexión de
/// LISTEN se cae, este hilo la reabre solo cada 5 segundos -- mismo
/// espíritu de auto-reparación que `with_reconnect` ya da a la conexión de
/// queries (§3.40), para que un problema de red no deje la propagación
/// cross-instancia rota para siempre sin un reinicio manual.
fn spawn_remote_listener(url: String, instance_id: String) -> Receiver<RemoteChange> {
    let (tx, rx) = mpsc::sync_channel::<RemoteChange>(REMOTE_CHANGE_BUFFER);
    std::thread::spawn(move || {
        use postgres::fallible_iterator::FallibleIterator;
        loop {
            let mut client = match connect_postgres_client(&url) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("LISTEN {REMOTE_CHANGE_CHANNEL}: no se pudo conectar ({e}), reintentando en 5s");
                    std::thread::sleep(std::time::Duration::from_secs(5));
                    continue;
                }
            };
            if let Err(e) = client.execute(&format!("LISTEN {REMOTE_CHANGE_CHANNEL}"), &[]) {
                eprintln!("LISTEN {REMOTE_CHANGE_CHANNEL}: no se pudo suscribir ({e}), reintentando en 5s");
                std::thread::sleep(std::time::Duration::from_secs(5));
                continue;
            }
            let mut notifications = client.notifications();
            let mut iter = notifications.blocking_iter();
            loop {
                match iter.next() {
                    Ok(Some(n)) => {
                        let Some(change) = parse_remote_notification(n.payload(), &instance_id) else { continue };
                        // `Err` acá significa que el `Receiver` ya no existe
                        // -- `serve()` terminó (proceso cerrando). Nada más
                        // que hacer: este hilo también termina.
                        if tx.send(change).is_err() {
                            return;
                        }
                    }
                    // Conexión cerrada, o error de verdad -- los dos casos
                    // se resuelven igual: reconectar desde el loop externo.
                    Ok(None) => break,
                    Err(e) => {
                        eprintln!("LISTEN {REMOTE_CHANGE_CHANNEL}: {e}, reconectando en 5s");
                        break;
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(5));
        }
    });
    rx
}

/// El payload de un `NOTIFY` es siempre `{"instance": "...", "collection":
/// "...", "event": ...}` (armado por `Db::notify_remote`) -- `None` si no
/// parsea con esa forma (nunca debería pasar salvo que algo externo mande
/// un NOTIFY al mismo canal por su cuenta, lo cual se ignora en vez de
/// reventar) o si `instance` coincide con la propia: es el eco del NOTIFY
/// que ESTA misma instancia mandó al escribir, y ese cambio ya se publicó
/// local en el momento de escribir (`Db::publish`) -- reinyectarlo de
/// nuevo acá lo entregaría DOS veces a los mismos suscriptores.
fn parse_remote_notification(payload: &str, my_instance_id: &str) -> Option<RemoteChange> {
    let v: serde_json::Value = serde_json::from_str(payload).ok()?;
    let instance = v.get("instance")?.as_str()?;
    if instance == my_instance_id {
        return None;
    }
    let collection = v.get("collection")?.as_str()?.to_string();
    let event = v.get("event")?.clone();
    Some(RemoteChange { collection, event })
}

impl Db {
    /// Única forma real de construcción -- `db_path` puede ser un archivo
    /// de verdad (persistencia real, lo que usa `linkc serve`) o el string
    /// mágico `":memory:"` de SQLite (mismo código, sin ninguna rama
    /// especial -- lo que usan `seeded()` y los tests). Infalible en la
    /// firma, como antes; internamente hace panic ante cualquier error de
    /// setup (archivo ilegible, schema incompatible con una corrida
    /// anterior, etc.) -- mismo estilo que `server.rs::serve` ya usa dos
    /// líneas más arriba para el bind de `tiny_http`.
    pub fn new(program: &Program, db_path: &Path) -> Self {
        let (checker, symbol_errors) = Checker::build_symbols(program);
        if let Some(e) = symbol_errors.into_iter().next() {
            panic!("programa inválido al abrir la base de datos: {e}");
        }
        let simple_enums = simple_enum_names(program);
        let db_path_display = db_path.display().to_string();

        let connection =
            Connection::open(db_path).unwrap_or_else(|e| panic!("no se pudo abrir la base de datos en '{db_path_display}': {e}"));
        connection.busy_timeout(std::time::Duration::from_millis(5000)).expect("configurar busy_timeout no debería fallar");
        // Best-effort: WAL no aplica a ":memory:" (SQLite lo ignora sin
        // error, sigue en modo "memory") y no cambia ningún argumento de
        // corrección -- solo permite inspeccionar el archivo con `sqlite3`
        // mientras `linkc serve` sigue corriendo.
        let _ = connection.pragma_update(None, "journal_mode", "WAL");

        let mut columns = HashMap::new();
        for (name, element_ty) in checker.db_collections() {
            let Type::Struct { fields, .. } = element_ty else {
                unreachable!("Checker::validate_db_element_type ya garantizó que el elemento sea un struct");
            };
            let cols: Vec<ColumnPlan> =
                fields.iter().filter(|f| f.name != "id").map(|f| ColumnPlan::for_field(f.clone(), &simple_enums)).collect();
            connection
                .execute(&create_table_sql(name, &cols), [])
                .unwrap_or_else(|e| panic!("no se pudo crear la tabla '{name}' en '{db_path_display}': {e}"));
            check_schema_matches(&connection, name, &cols, &db_path_display).unwrap_or_else(|e| panic!("{e}"));
            columns.insert(name.clone(), cols);
        }

        Db {
            backend: Backend::Sqlite(connection),
            checker,
            simple_enums,
            columns,
            subscribers: RefCell::new(HashMap::new()),
            current_request: RefCell::new(None),
            response_status_override: std::cell::Cell::new(None),
            instance_id: random_instance_id(),
            argon2_params: RefCell::new(argon2::Params::default()),
        }
    }

    /// Lo mismo que `new`, contra un PostgreSQL real (GRAMMAR.md §3.36). Todo
    /// lo de arriba de esta capa -- `call`, `subscribe`, el plan de columnas,
    /// la codificación JSON -- es exactamente el mismo código: lo único que
    /// cambia es quién ejecuta el SQL.
    ///
    /// Devuelve `Result` y no hace panic como `new`: una base remota puede
    /// estar caída, tener otra contraseña o no existir todavía, y eso es una
    /// condición operativa normal que `linkc serve` tiene que poder reportar
    /// con un mensaje entendible.
    ///
    /// El `Receiver<RemoteChange>` que devuelve junto con `Db` es el otro
    /// lado de LISTEN/NOTIFY (GRAMMAR.md §3.44): `runtime/server.rs` lo
    /// drena en su loop principal y reinyecta cada cambio con
    /// `Db::publish_remote`, para que un `stream` conectado a ESTA
    /// instancia vea también lo que escribió OTRA contra la misma base.
    pub(crate) fn connect_postgres(program: &Program, url: &str) -> Result<(Self, Receiver<RemoteChange>), String> {
        let (checker, symbol_errors) = Checker::build_symbols(program);
        if let Some(e) = symbol_errors.into_iter().next() {
            return Err(format!("programa inválido al abrir la base de datos: {e}"));
        }
        let simple_enums = simple_enum_names(program);

        let client = connect_postgres_client(url)?;
        let backend = Backend::Postgres { client: std::cell::RefCell::new(client), url: url.to_string() };

        let mut columns = HashMap::new();
        for (name, element_ty) in checker.db_collections() {
            let Type::Struct { fields, .. } = element_ty else {
                unreachable!("Checker::validate_db_element_type ya garantizó que el elemento sea un struct");
            };
            let cols: Vec<ColumnPlan> =
                fields.iter().filter(|f| f.name != "id").map(|f| ColumnPlan::for_field(f.clone(), &simple_enums)).collect();
            let non_id: Vec<FieldType> = cols.iter().map(|c| c.field.clone()).collect();

            // El DDL sale del MISMO generador que usa `linkc build` para
            // emitir schema.pg.sql. Si el runtime creara las tablas por su
            // cuenta, el esquema que el proyecto documenta y el que la base
            // realmente tiene podrían divergir -- que es la clase de bug que
            // este repo ya encontró varias veces (GRAMMAR.md §3.9).
            backend
                .execute_ddl(&crate::codegen::postgres_emit::create_postgres_table_sql(name, &non_id, &simple_enums))
                .map_err(|e| format!("no se pudo crear la tabla '{name}': {e}"))?;

            // `CREATE TABLE IF NOT EXISTS` es un no-op sobre una tabla que ya
            // existía -- nunca mira si SU "id" es compatible. Encontrado en
            // producción: una tabla real con `id UUID` (migrando desde otro
            // backend) dejaba pasar el connect sin queja, y el primer insert
            // recién ahí fallaba -- antes de este chequeo, con un panic que
            // tiraba abajo el servidor entero (ver store.rs::insert_returning_id).
            // Falla ACÁ, al conectar, con un mensaje que dice qué hacer --
            // mismo momento y mismo criterio que `check_schema_matches` ya
            // aplica para SQLite, adaptado a que Postgres no recrea tablas.
            validate_existing_id_column(&backend, name)?;

            // Migración no destructiva: una tabla que ya existe de una versión
            // anterior del programa gana las columnas nuevas. A diferencia de
            // SQLite (que acá falla fuerte ante cualquier deriva de esquema,
            // ver `check_schema_matches`), PostgreSQL es el backend donde ya
            // hay datos de producción y volver a crear la tabla no es opción.
            //
            // La columna se agrega SIEMPRE nullable, aunque el tipo del campo
            // sea requerido: `ADD COLUMN ... NOT NULL` sobre una tabla con
            // filas fallaría, porque no hay valor que poner en las que ya
            // están. Es un límite real y está documentado.
            for field in &non_id {
                backend
                    .execute_ddl(&crate::codegen::postgres_emit::alter_table_add_column_postgres(name, field, &simple_enums))
                    .map_err(|e| format!("no se pudo migrar la tabla '{name}': {e}"))?;
            }
            columns.insert(name.clone(), cols);
        }

        let instance_id = random_instance_id();
        let remote_rx = spawn_remote_listener(url.to_string(), instance_id.clone());

        Ok((
            Db {
                backend,
                checker,
                simple_enums,
                columns,
                subscribers: RefCell::new(HashMap::new()),
                current_request: RefCell::new(None),
                response_status_override: std::cell::Cell::new(None),
                instance_id,
                argon2_params: RefCell::new(argon2::Params::default()),
            },
            remote_rx,
        ))
    }

    /// Fija el costo de `crypto.hashPassword` para lo que quede de vida del
    /// proceso (GRAMMAR.md §3.55) -- `server.rs` lo llama UNA sola vez, antes
    /// de aceptar la primera request, con lo que haya resuelto de
    /// `--argon2-memory-kib`/`--argon2-iterations` (o sus env vars). Nunca se
    /// vuelve a llamar durante la vida del servidor.
    pub(crate) fn set_argon2_params(&self, params: argon2::Params) {
        *self.argon2_params.borrow_mut() = params;
    }

    /// Los parámetros de costo configurados -- los lee `crypto.hashPassword`
    /// en `runtime/mod.rs` en cada llamada. `argon2::Params` no es `Copy`
    /// (guarda un `Option<Vec<u8>>` para el "secret" opcional que este
    /// proyecto no usa), así que clona en vez de prestar: el costo es
    /// insignificante comparado con el propio hasheo (~15ms, §3.34).
    pub(crate) fn argon2_params(&self) -> argon2::Params {
        self.argon2_params.borrow().clone()
    }

    /// Fixture SOLO para tests y para el demo wasm (`bin/wasm_demo.rs`) --
    /// **no** es lo que usa `linkc serve` (ver `runtime/server.rs`, que usa
    /// `Db::new`). Mantiene su firma de cero argumentos: por dentro, arma
    /// un programa mínimo real (mismo shape que `User` en
    /// `examples/users.link`) a través del tokenizer/parser/checker DE
    /// VERDAD -- no `Value`s armados a mano como antes -- y siembra a Ada y
    /// Grace insertando por el mismo camino (`Db::call`) que usaría
    /// cualquier programa real. Esto además cierra gratis un hueco ya
    /// documentado antes de esta ronda: al pasar por un `Program` real,
    /// `simple_enums` sale poblado correctamente (antes quedaba vacío a
    /// propósito, con el riesgo de que un evento publicado contra datos
    /// sembrados acá serializara `role` con la forma equivocada).
    pub fn seeded() -> Self {
        const SEEDED_SCHEMA_SRC: &str = "\
type User = { id: Int, name: String, email: String, role: Role, bio?: String, deletedAt: String? }
enum Role { Admin, Member, Guest }
db { users: User[] }
";
        let tokens = crate::lexer::tokenize(SEEDED_SCHEMA_SRC).unwrap_or_else(|e| panic!("fixture de seeded() no tokeniza: {e}"));
        let program = crate::parser::parse(tokens).unwrap_or_else(|errors| panic!("fixture de seeded() no parsea: {errors:?}"));
        let db = Db::new(&program, Path::new(":memory:"));

        let role = |variant: &str| Value::Variant { enum_name: "Role".to_string(), variant: variant.to_string(), fields: Vec::new() };
        db.call(
            "users",
            "insert",
            vec![Value::Struct(vec![
                ("name".into(), Value::Str("Ada Lovelace".into())),
                ("email".into(), Value::Str("ada@example.com".into())),
                ("role".into(), role("Admin")),
                ("bio".into(), Value::Str("Pionera de la programación".into())),
                ("deletedAt".into(), Value::Null),
            ])],
        )
        .expect("sembrar a Ada Lovelace no debería fallar");
        db.call(
            "users",
            "insert",
            vec![Value::Struct(vec![
                ("name".into(), Value::Str("Grace Hopper".into())),
                ("email".into(), Value::Str("grace@example.com".into())),
                ("role".into(), role("Member")),
                // 'bio' se OMITE del todo -- `bio?: String` es opcional por
                // CLAVE (ausente = "no tiene"), no nullable (GRAMMAR.md §3.4).
                ("deletedAt".into(), Value::Null),
            ])],
        )
        .expect("sembrar a Grace Hopper no debería fallar");
        db
    }

    /// Qué motor está atrás. Solo para reportarlo al arrancar: ningún camino
    /// del intérprete se ramifica por esto.
    pub fn is_postgres(&self) -> bool {
        self.backend.is_postgres()
    }

    /// Ver la doc de `Db::current_request` (arriba). Llamado por
    /// `server.rs` una vez por request, justo antes de invocar el rpc.
    pub(crate) fn set_request_context(&self, ctx: RequestContext) {
        *self.current_request.borrow_mut() = Some(ctx);
    }

    /// Simétrico de `set_request_context` -- `server.rs` lo llama apenas
    /// termina de manejar la request, para que `request.rawBody()`/
    /// `request.header()` nunca puedan filtrar datos de una request anterior
    /// hacia otra (ej. si algún día se reusa un `Db` entre requests de
    /// formas que hoy no se dan, pero que esto deja imposibles por
    /// construcción en vez de "porque nadie se olvidó de limpiar").
    pub(crate) fn clear_request_context(&self) {
        *self.current_request.borrow_mut() = None;
        // Defensa en profundidad simétrica a la de arriba -- si un rpc
        // llamó `response.setStatus` y DESPUÉS erró/panicó (así que
        // `handle_rpc` nunca llegó a consumirlo con `take_response_status`),
        // esto evita que el valor sobreviva para la PRÓXIMA request.
        self.response_status_override.set(None);
    }

    /// Llamado por `response.setStatus(code)` (GRAMMAR.md §3.46) -- guarda
    /// el override para que `handle_rpc` lo use en vez de 200 al armar la
    /// respuesta, SOLO en el camino de éxito (`handle_rpc` nunca llega a
    /// leerlo si el rpc termina en `Err`: un error siempre va con el status
    /// que le corresponde a la falla, nunca con uno que el cuerpo haya
    /// pedido antes de fallar).
    pub(crate) fn set_response_status(&self, code: u16) {
        self.response_status_override.set(Some(code));
    }

    /// Consume el override (lo deja en `None` para la próxima invocación) --
    /// `take`, no `get`, para que un valor de UNA request nunca sobreviva
    /// por accidente a la que sigue si algún día `Db` se reusara de una
    /// forma que hoy no pasa.
    pub(crate) fn take_response_status(&self) -> Option<u16> {
        self.response_status_override.take()
    }

    /// `""` -- no `None` -- fuera de una request HTTP real (ej. invocado
    /// desde `linkc test`, que llama `invoke_rpc` directo sin pasar por
    /// `server.rs`): un body ausente y uno vacío son la misma cosa para
    /// cualquier verificación de firma que lo use, y devolver un `String`
    /// simple (no `String?`) evita que TODO código que llama `rawBody()`
    /// tenga que manejar un `null` que en la práctica nunca es información
    /// útil -- distinto de `current_request_header`, donde "el header no
    /// vino" sí es una distinción real que el caller necesita poder ver.
    pub(crate) fn current_request_body(&self) -> String {
        self.current_request.borrow().as_ref().map(|c| c.raw_body.clone()).unwrap_or_default()
    }

    pub(crate) fn current_request_header(&self, name: &str) -> Option<String> {
        self.current_request
            .borrow()
            .as_ref()
            .and_then(|c| c.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone()))
    }

    pub fn call(&self, collection: &str, method: &str, args: Vec<Value>) -> Result<Value, RuntimeError> {
        let columns = self.columns.get(collection).ok_or_else(|| RuntimeError::new(format!("colección desconocida: '{collection}'")))?;
        match method {
            "all" => self.select_rows(collection, columns, None).map(Value::List),
            "page" => {
                let limit = as_int(args.first().ok_or_else(|| RuntimeError::new("page requiere 2 argumentos (limit, offset)"))?)?;
                let offset = as_int(args.get(1).ok_or_else(|| RuntimeError::new("page requiere 2 argumentos (limit, offset)"))?)?;
                if limit < 0 || offset < 0 {
                    return Err(RuntimeError::new(format!(
                        "db.<c>.page({limit}, {offset}): limit y offset tienen que ser >= 0"
                    )));
                }
                self.select_rows_page(collection, columns, limit, offset).map(Value::List)
            }
            "pageAfter" => {
                let after = match args.first() {
                    Some(Value::Int(n)) => Some(*n),
                    Some(Value::Null) | None => None,
                    _ => return Err(RuntimeError::new("pageAfter requiere un cursor Int? como primer argumento")),
                };
                let limit = as_int(args.get(1).ok_or_else(|| RuntimeError::new("pageAfter requiere 2 argumentos (cursor, limit)"))?)?;
                if limit < 0 {
                    return Err(RuntimeError::new(format!("db.<c>.pageAfter(_, {limit}): limit tiene que ser >= 0")));
                }
                self.select_rows_after(collection, columns, after, limit).map(Value::List)
            }
            "sumBy" | "countBy" | "avgBy" | "maxBy" | "minBy" => self.select_grouped(collection, columns, method, &args).map(Value::List),
            "find" => {
                let id = as_int(args.first().ok_or_else(|| RuntimeError::new("find requiere 1 argumento"))?)?;
                Ok(self.select_rows(collection, columns, Some(id))?.into_iter().next().unwrap_or(Value::Null))
            }
            "insert" => {
                let v = args.into_iter().next().ok_or_else(|| RuntimeError::new("insert requiere 1 argumento"))?;
                let Value::Struct(fields) = &v else {
                    return Err(RuntimeError::new("insert: el valor debe ser un struct"));
                };
                let mut col_names = Vec::with_capacity(columns.len());
                let mut params: Vec<Cell> = Vec::with_capacity(columns.len());
                for col in columns {
                    let slot = fields.iter().find(|(n, _)| n == &col.field.name).map(|(_, v)| v);
                    col_names.push(format!("\"{}\"", col.field.name));
                    params.push(self.write_param(col, slot));
                }
                let sql = if col_names.is_empty() {
                    format!("INSERT INTO \"{collection}\" DEFAULT VALUES")
                } else {
                    let placeholders: Vec<String> = (1..=col_names.len()).map(|n| self.backend.placeholder(n)).collect();
                    format!("INSERT INTO \"{collection}\" ({}) VALUES ({})", col_names.join(", "), placeholders.join(", "))
                };
                let new_id = self
                    .backend
                    .insert_returning_id(&sql, &params)
                    .map_err(|e| RuntimeError::new(format!("insert falló: {e}")))?;
                let inserted = self
                    .select_rows(collection, columns, Some(new_id))?
                    .into_iter()
                    .next()
                    .expect("la fila recién insertada tiene que existir");
                self.publish(collection, &inserted);
                Ok(inserted)
            }
            "applyPatch" => {
                let mut it = args.into_iter();
                let id = as_int(&it.next().ok_or_else(|| RuntimeError::new("applyPatch requiere 2 argumentos"))?)?;
                let patch = it.next().ok_or_else(|| RuntimeError::new("applyPatch requiere 2 argumentos"))?;
                let Value::Struct(patch_fields) = patch else {
                    return Err(RuntimeError::new("applyPatch: el patch debe ser un objeto"));
                };
                let mut set_clauses = Vec::new();
                let mut params: Vec<Cell> = Vec::new();
                for (name, value) in &patch_fields {
                    // "id" nunca es escribible -- mismo criterio que insert,
                    // que también lo excluye de lo que el caller puede fijar.
                    let Some(col) = columns.iter().find(|c| name == &c.field.name) else { continue };
                    set_clauses.push(format!("\"{name}\" = {}", self.backend.placeholder(params.len() + 1)));
                    params.push(self.write_param(col, Some(value)));
                }
                if !set_clauses.is_empty() {
                    let id_placeholder = self.backend.placeholder(params.len() + 1);
                    params.push(Cell::Int(id));
                    let sql = format!("UPDATE \"{collection}\" SET {} WHERE \"id\" = {id_placeholder}", set_clauses.join(", "));
                    self.backend
                        .execute(&sql, &params)
                        .map_err(|e| RuntimeError::new(format!("applyPatch falló: {e}")))?;
                }
                // Reconsultar por id, tanto si hubo UPDATE como si el patch
                // no traía ningún campo escribible -- "no encontrado" en
                // esta consulta es la única señal de "no existe", cubre los
                // dos casos con un solo camino.
                let updated = self
                    .select_rows(collection, columns, Some(id))?
                    .into_iter()
                    .next()
                    .ok_or_else(|| RuntimeError::new(format!("no hay ningún elemento con id {id} en '{collection}'")))?;
                self.publish(collection, &updated);
                Ok(updated)
            }
            "delete" => {
                let id = as_int(args.first().ok_or_else(|| RuntimeError::new("delete requiere 1 argumento"))?)?;
                let existing = self.select_rows(collection, columns, Some(id))?.into_iter().next();
                let sql = format!("DELETE FROM \"{collection}\" WHERE \"id\" = {}", self.backend.placeholder(1));
                let rows_affected = self
                    .backend
                    .execute(&sql, &[Cell::Int(id)])
                    .map_err(|e| RuntimeError::new(format!("delete falló: {e}")))?;
                if rows_affected > 0 {
                    if let Some(deleted_row) = existing {
                        self.publish(collection, &deleted_row);
                    }
                }
                Ok(Value::Bool(rows_affected > 0))
            }
            "count" => {
                let sql = format!("SELECT COUNT(*) FROM \"{collection}\"");
                let rows = self
                    .backend
                    .query(&sql, &[], &[ColumnKind::Int])
                    .map_err(|e| RuntimeError::new(format!("error en count de '{collection}': {e}")))?;
                match rows.first().and_then(|r| r.first()) {
                    Some(Cell::Int(count)) => Ok(Value::Int(*count)),
                    other => Err(RuntimeError::new(format!("count de '{collection}' devolvió algo que no es un entero: {other:?}"))),
                }
            }

            // "deleteWhere"/"findWhere" NUNCA se implementan acá: evaluar un
            // predicado por fila requiere invocar un closure de usuario
            // (`call_callable`), que necesita `fns`/`checker`/`sessions`/
            // `step_budget` -- ninguno de los cuales `Db::call` recibe (ver
            // su firma arriba). La implementación real vive en
            // `runtime::call_method`, que intercepta estos dos métodos
            // ANTES de llegar acá y sí tiene ese contexto (mismo patrón que
            // `List::filter`/`List::map`); `call_method` es el único camino
            // que el intérprete usa para despachar un método de
            // `Value::DbCollection`, así que en el uso normal este brazo
            // nunca corre. Como `Db::call` es `pub fn` y queda alcanzable
            // directo (tests, LSP, código futuro), antes devolvía un
            // resultado SILENCIOSAMENTE INCORRECTO ignorando el predicado
            // (deleteWhere borraba TODAS las filas; findWhere las
            // devolvía TODAS) -- fallar con un mensaje claro es siempre
            // mejor que una respuesta que parece válida y no lo es.
            "deleteWhere" | "findWhere" => Err(RuntimeError::new(format!(
                "'db.{collection}.{method}' solo se puede invocar a través del intérprete (evalúa un predicado por fila, y este método no tiene acceso a closures) -- llegó directo a Db::call, sin pasar por runtime::call_method"
            ))),
            other => Err(RuntimeError::new(format!("método desconocido: 'db.{collection}.{other}'"))),
        }
    }

    /// Nuevo suscriptor de TODA mutación futura (`insert`/`applyPatch`) de
    /// `collection`, más una foto de lo que ya hay ADENTRO en este mismo
    /// instante (GRAMMAR.md §3.16/§3.17, push real v0). Nunca lo llama el
    /// intérprete -- solo `runtime/server.rs`, directo sobre el `&Db` que
    /// ya tiene, ANTES de decidir si invoca `invoke_rpc_with_sessions` (ver
    /// `ast::recognize_live_subscribe`).
    ///
    /// Sacar la foto y registrarse son las dos líneas de ESTA MISMA llamada
    /// sincrónica, sin ningún punto de suspensión entre ellas -- un
    /// `SELECT` de `rusqlite` es una llamada de Rust sincrónica normal, sin
    /// `.await`, que corre entera en el hilo que la llama, ni distinto de
    /// clonar un `Vec` en ese sentido. La única otra cosa que podría
    /// "colarse" (una mutación) solo pasa dentro de `Db::call`, en el mismo
    /// único hilo que corre esto. El servidor entero procesa una request a
    /// la vez en ese hilo, así que no hay forma de que una mutación se
    /// intercale entre las dos líneas de acá: el single-threading del
    /// servidor ES el lock, no hace falta agregar uno.
    pub fn subscribe(&self, collection: &str) -> Result<(Vec<serde_json::Value>, Receiver<serde_json::Value>), RuntimeError> {
        let columns = self.columns.get(collection).ok_or_else(|| RuntimeError::new(format!("colección desconocida: '{collection}'")))?;
        let snapshot: Vec<serde_json::Value> =
            self.select_rows(collection, columns, None)?.iter().map(|v| value_to_json(v, &self.simple_enums)).collect();
        let (tx, rx) = mpsc::sync_channel(LIVE_STREAM_BUFFER);
        self.subscribers.borrow_mut().entry(collection.to_string()).or_default().push(tx);
        Ok((snapshot, rx))
    }

    /// Llamado SOLO desde el final de los arms `"insert"`/`"applyPatch"` de
    /// `call`, DESPUÉS de que la fila ya está firme en la tabla -- nunca
    /// antes, para no anunciar una mutación que en realidad falló más
    /// adelante (ambos arms ya tienen todos sus pasos falibles ANTES de
    /// esta llamada).
    ///
    /// Entrega LOCAL primero, siempre -- después, si el backend es
    /// Postgres, además `NOTIFY` para que otras instancias contra la misma
    /// base también se enteren (GRAMMAR.md §3.44). El NOTIFY es
    /// best-effort: si falla, esta instancia YA entregó local (arriba), así
    /// que no es pérdida de datos para nadie conectado acá -- solo una
    /// propagación cross-instancia que no llegó esta vez.
    fn publish(&self, collection: &str, row: &Value) {
        let json = value_to_json(row, &self.simple_enums);
        self.deliver_local(collection, &json);
        if self.backend.is_postgres() {
            self.notify_remote(collection, &json);
        }
    }

    /// Reinyecta LOCAL un cambio que anunció OTRA instancia (drenado del
    /// canal de `spawn_remote_listener`, GRAMMAR.md §3.44) -- mismo
    /// mecanismo de entrega que `publish`, pero el evento YA es JSON (llegó
    /// tal cual en el payload del NOTIFY) y esto NUNCA vuelve a notificar:
    /// si lo hiciera, cada instancia reenviaría el cambio de las demás sin
    /// parar nunca.
    pub(crate) fn publish_remote(&self, collection: &str, event: serde_json::Value) {
        self.deliver_local(collection, &event);
    }

    fn deliver_local(&self, collection: &str, json: &serde_json::Value) {
        let mut subs = self.subscribers.borrow_mut();
        if let Some(list) = subs.get_mut(collection) {
            // `try_send` -- NUNCA bloqueante: publicar no puede colgar el
            // único hilo que atiende todas las requests, ni siquiera si un
            // suscriptor está lento. `Full` (suscriptor demasiado atrasado,
            // ver LIVE_STREAM_BUFFER) o `Disconnected` (el cliente ya se
            // fue) se podan igual -- lazy, recién en la próxima publicación
            // a esta colección, no eager (un mecanismo eager necesitaría un
            // hilo aparte tocando `Db`, reabriendo la pregunta de Send/Sync
            // que todo este diseño evita).
            list.retain(|tx| tx.try_send(json.clone()).is_ok());
        }
    }

    fn notify_remote(&self, collection: &str, json: &serde_json::Value) {
        let payload = serde_json::json!({
            "instance": self.instance_id,
            "collection": collection,
            "event": json,
        })
        .to_string();
        if payload.len() > MAX_NOTIFY_PAYLOAD_BYTES {
            eprintln!(
                "aviso: un cambio en '{collection}' de {} bytes supera el límite de NOTIFY de PostgreSQL \
                 ({MAX_NOTIFY_PAYLOAD_BYTES}) -- no se propaga a otras instancias (GRAMMAR.md §3.44)",
                payload.len()
            );
            return;
        }
        if let Err(e) = self.backend.notify(REMOTE_CHANGE_CHANNEL, &payload) {
            eprintln!("aviso: no se pudo notificar el cambio en '{collection}' a otras instancias: {e}");
        }
    }

    /// `all` (`id: None`, ordenado por "id" para output determinístico,
    /// mismo orden de inserción que ya daba el `Vec` de antes) o `find`/la
    /// re-consulta de `insert`/`applyPatch` (`id: Some(_)`, a lo sumo 1 fila).
    fn select_rows(&self, collection: &str, columns: &[ColumnPlan], id: Option<i64>) -> Result<Vec<Value>, RuntimeError> {
        let mut col_list = vec!["\"id\"".to_string()];
        col_list.extend(columns.iter().map(|c| format!("\"{}\"", c.field.name)));
        let sql = match id {
            Some(_) => format!("SELECT {} FROM \"{collection}\" WHERE \"id\" = {}", col_list.join(", "), self.backend.placeholder(1)),
            None => format!("SELECT {} FROM \"{collection}\" ORDER BY \"id\"", col_list.join(", ")),
        };
        // El orden de `kinds` es el del SELECT: "id" primero, después las
        // columnas declaradas, en el mismo orden que `columns`.
        let mut kinds = vec![ColumnKind::Int];
        kinds.extend(columns.iter().map(ColumnPlan::kind));
        let params: Vec<Cell> = id.map(|i| vec![Cell::Int(i)]).unwrap_or_default();

        let rows = self
            .backend
            .query(&sql, &params, &kinds)
            .map_err(|e| RuntimeError::new(format!("error de SQL en '{collection}': {e}")))?;
        Ok(rows.iter().map(|cells| Value::Struct(self.row_to_fields(cells, columns))).collect())
    }

    /// `db.<c>.page(limit, offset)` (GRAMMAR.md §3.48) -- a diferencia de
    /// `.all()` (que trae TODA la tabla y recién ahí el intérprete podría
    /// cortarla con `.take()`, si el programa se acordara de hacerlo),
    /// `LIMIT`/`OFFSET` van adentro del propio SQL: para una tabla grande,
    /// pedir la página 400 sigue costando O(página), no O(tabla entera).
    /// Mismo orden que `.all()` (`ORDER BY "id"`) para que la paginación sea
    /// determinística entre llamadas -- páginas que se solapan o se saltean
    /// filas por un orden distinto en cada query serían peor que no tener
    /// paginación.
    fn select_rows_page(&self, collection: &str, columns: &[ColumnPlan], limit: i64, offset: i64) -> Result<Vec<Value>, RuntimeError> {
        let mut col_list = vec!["\"id\"".to_string()];
        col_list.extend(columns.iter().map(|c| format!("\"{}\"", c.field.name)));
        let sql = format!(
            "SELECT {} FROM \"{collection}\" ORDER BY \"id\" LIMIT {} OFFSET {}",
            col_list.join(", "),
            self.backend.placeholder(1),
            self.backend.placeholder(2)
        );
        let mut kinds = vec![ColumnKind::Int];
        kinds.extend(columns.iter().map(ColumnPlan::kind));
        let params = vec![Cell::Int(limit), Cell::Int(offset)];

        let rows = self
            .backend
            .query(&sql, &params, &kinds)
            .map_err(|e| RuntimeError::new(format!("error de SQL en '{collection}': {e}")))?;
        Ok(rows.iter().map(|cells| Value::Struct(self.row_to_fields(cells, columns))).collect())
    }

    /// `db.<c>.pageAfter(cursor, limit)` (GRAMMAR.md §3.61) -- cursor de
    /// continuación en vez del `offset` manual de `page`. El cursor ES el
    /// `id` del último elemento de la página anterior (`null` para la
    /// primera): no un token opaco codificado aparte, a propósito -- el
    /// `id` ya es un campo público del struct, inventar una capa de
    /// codificación encima no agrega ninguna garantía real, solo ceremonia.
    /// La diferencia real con `page(limit, offset)` no es la "opacidad" del
    /// cursor, es que `WHERE "id" > cursor` es ESTABLE bajo inserciones
    /// concurrentes -- un `OFFSET` cuenta filas desde el principio de la
    /// tabla en cada llamada, así que una fila insertada ENTRE dos páginas
    /// puede hacer que la página siguiente repita o se salte una fila; un
    /// cursor por `id` nunca tiene ese problema, porque no cuenta filas, filtra
    /// por una posición fija en el orden.
    fn select_rows_after(&self, collection: &str, columns: &[ColumnPlan], after: Option<i64>, limit: i64) -> Result<Vec<Value>, RuntimeError> {
        let mut col_list = vec!["\"id\"".to_string()];
        col_list.extend(columns.iter().map(|c| format!("\"{}\"", c.field.name)));
        let sql = match after {
            Some(_) => format!(
                "SELECT {} FROM \"{collection}\" WHERE \"id\" > {} ORDER BY \"id\" LIMIT {}",
                col_list.join(", "),
                self.backend.placeholder(1),
                self.backend.placeholder(2)
            ),
            None => format!(
                "SELECT {} FROM \"{collection}\" ORDER BY \"id\" LIMIT {}",
                col_list.join(", "),
                self.backend.placeholder(1)
            ),
        };
        let mut kinds = vec![ColumnKind::Int];
        kinds.extend(columns.iter().map(ColumnPlan::kind));
        let params: Vec<Cell> = match after {
            Some(id) => vec![Cell::Int(id), Cell::Int(limit)],
            None => vec![Cell::Int(limit)],
        };

        let rows = self
            .backend
            .query(&sql, &params, &kinds)
            .map_err(|e| RuntimeError::new(format!("error de SQL en '{collection}': {e}")))?;
        Ok(rows.iter().map(|cells| Value::Struct(self.row_to_fields(cells, columns))).collect())
    }

    /// `sumBy`/`countBy`/`avgBy`/`maxBy`/`minBy` (GRAMMAR.md §3.52) --
    /// `GROUP BY` real, corriendo adentro de la base. El checker ya validó
    /// el shape de cada closure (`check_aggregate_by`) y que los campos
    /// existen y son del tipo correcto; acá se vuelve a correr
    /// `recognize_field_selector` sobre el `Value::Closure` que de verdad
    /// llegó -- el checker corrió sobre el AST en compilación, esto corre
    /// sobre el mismo AST otra vez pero en runtime, mismo criterio que
    /// `ast::recognize_live_subscribe` (usado por checker.rs Y por
    /// runtime::live_subscribe_collection, cada uno en su propio momento).
    fn select_grouped(&self, collection: &str, columns: &[ColumnPlan], method: &str, args: &[Value]) -> Result<Vec<Value>, RuntimeError> {
        let key_field = closure_field_name(args.first(), "de agrupación")?;
        let key_col = columns
            .iter()
            .find(|c| c.field.name == key_field)
            .ok_or_else(|| RuntimeError::new(format!("'{method}': '{key_field}' no es una columna real de '{collection}'")))?;

        let (value_expr, value_kind, value_field_ty) = if method == "countBy" {
            ("COUNT(*)".to_string(), ColumnKind::Int, Type::Int)
        } else {
            let value_field = closure_field_name(args.get(1), "de valor")?;
            let value_col = columns
                .iter()
                .find(|c| c.field.name == value_field)
                .ok_or_else(|| RuntimeError::new(format!("'{method}': '{value_field}' no es una columna real de '{collection}'")))?;
            let sql_fn = match method {
                "sumBy" => "SUM",
                "avgBy" => "AVG",
                "maxBy" => "MAX",
                "minBy" => "MIN",
                other => panic!("select_grouped llamado con un método que Db::call no debería enrutar acá: '{other}'"),
            };
            // AVG en SQL siempre devuelve fraccionario, sin importar si la
            // columna de origen es entera -- MAX/MIN sí preservan el tipo
            // de la columna a nivel LÓGICO. Pero a nivel de TIPO DE CABLE,
            // Postgres promueve el resultado de SUM/AVG sobre una columna
            // entera a `numeric` (precisión arbitraria) -- ni `i64` ni
            // `f64` decodifican `numeric` directo (`postgres_cell`,
            // store.rs). SQLite no tiene ese problema (afinidad de tipos,
            // no tipos de cable fijos), así que esto pasó los tests
            // locales y explotó recién en CI contra Postgres real -- hallado
            // corriendo el job de Postgres, no por inspección de código.
            // `CAST(expr AS BIGINT/DOUBLE PRECISION)` fuerza el tipo de
            // cable de vuelta al que `kinds` promete, portable entre los
            // dos motores (a diferencia de `::bigint`, sintaxis exclusiva
            // de Postgres) -- incluso cuando ya sería un no-op (MAX sobre
            // una columna entera, que Postgres nunca promueve), el cast es
            // gratis y mantiene una sola regla sin memorizar la tabla de
            // promoción de tipos de cada función agregada.
            let kind = if method == "avgBy" { ColumnKind::Float } else { value_col.kind() };
            let cast_as = match kind {
                ColumnKind::Int => "BIGINT",
                ColumnKind::Float => "DOUBLE PRECISION",
                other => panic!("select_grouped: un selector de valor numérico no debería resolver a {other:?}"),
            };
            // AVG siempre da Float, sin importar el tipo de la columna de
            // origen (mismo motivo que el CAST de arriba); SUM/MAX/MIN
            // preservan el tipo LÓGICO real de la columna -- Int64, no solo
            // Int, desde la ronda de GRAMMAR.md §3.65 (antes esto asumía
            // Int siempre, así que un `sumBy` sobre una columna Int64
            // devolvía un `Value::Int` mal etiquetado en vez de
            // `Value::Int64`).
            let result_ty = if method == "avgBy" { Type::Float } else { value_col.field.ty.clone() };
            (format!("CAST({sql_fn}(\"{value_field}\") AS {cast_as})"), kind, result_ty)
        };

        let key_ty = key_col.field.ty.clone();
        let value_ty = value_field_ty;
        let sql =
            format!("SELECT \"{key_field}\" AS \"key\", {value_expr} AS \"value\" FROM \"{collection}\" GROUP BY \"{key_field}\"");
        let kinds = vec![key_col.kind(), value_kind];
        let rows = self
            .backend
            .query(&sql, &[], &kinds)
            .map_err(|e| RuntimeError::new(format!("error de SQL en '{collection}': {e}")))?;
        Ok(rows
            .iter()
            .map(|cells| {
                Value::Struct(vec![
                    ("key".to_string(), scalar_cell_to_value(&key_ty, &cells[0])),
                    ("value".to_string(), scalar_cell_to_value(&value_ty, &cells[1])),
                ])
            })
            .collect())
    }

    /// Reconstruye una fila entera (`"id"` + cada columna declarada) como los
    /// pares `(nombre, Value)` de un `Value::Struct` -- inversa de
    /// `write_param`. Las celdas llegan en el mismo orden que emitió el SELECT
    /// (`select_rows`), que es el orden de `columns` con `"id"` adelante.
    fn row_to_fields(&self, cells: &[Cell], columns: &[ColumnPlan]) -> Vec<(String, Value)> {
        let mut out = Vec::with_capacity(columns.len() + 1);
        let Some(Cell::Int(id)) = cells.first() else {
            panic!("la columna 'id' es la clave primaria: siempre es un entero no nulo, y llegó {:?}", cells.first());
        };
        out.push(("id".to_string(), Value::Int(*id)));

        for (col, cell) in columns.iter().zip(cells.iter().skip(1)) {
            if col.json {
                match cell {
                    // NULL en una columna JSON SIEMPRE significa "clave
                    // ausente" -- solo alcanzable si `field.optional` (ver
                    // `write_param`, nunca escribimos NULL acá si la clave es
                    // requerida). El `Value::Null` defensivo de abajo no
                    // debería ser alcanzable en la práctica.
                    Cell::Null => {
                        if !col.field.optional {
                            out.push((col.field.name.clone(), Value::Null));
                        }
                    }
                    Cell::Json(parsed) => {
                        let decoded = json_to_typed_value(parsed, &col.field.ty, &self.checker, &col.field.name)
                            .unwrap_or_else(|e| panic!("un valor que nosotros escribimos tiene que decodificar contra su propio tipo: {e}"));
                        out.push((col.field.name.clone(), decoded));
                    }
                    other => panic!("la columna JSON '{}' devolvió {other:?}", col.field.name),
                }
                continue;
            }

            let effective_ty: &Type = match &col.field.ty {
                Type::Optional(inner) => inner.as_ref(),
                other => other,
            };
            let value = match (effective_ty, cell) {
                (_, Cell::Null) => None,
                (Type::Int, Cell::Int(n)) => Some(Value::Int(*n)),
                (Type::Int64, Cell::Int(n)) => Some(Value::Int64(*n)),
                (Type::Timestamp, Cell::Int(n)) => Some(Value::Timestamp(*n)),
                (Type::Float, Cell::Float(f)) => Some(Value::Float(*f)),
                (Type::String, Cell::Text(t)) => Some(Value::Str(t.clone())),
                (Type::Bool, Cell::Bool(b)) => Some(Value::Bool(*b)),
                (Type::Enum(name), Cell::Text(variant)) => Some(Value::Variant {
                    enum_name: name.clone(),
                    variant: variant.clone(),
                    fields: Vec::new(),
                }),
                // Un desajuste acá significa que el plan de columnas y lo que
                // la base devolvió no coinciden: schema escrito por otra
                // versión del programa, o un backend nuevo mapeando mal un
                // tipo. Fallar fuerte con los dos lados a la vista es lo único
                // útil -- devolver un valor "parecido" escondería el problema
                // adentro de la respuesta de un rpc.
                (ty, cell) => panic!(
                    "la columna '{}' declara {ty} pero la base devolvió {cell:?}",
                    col.field.name
                ),
            };
            match value {
                Some(v) => out.push((col.field.name.clone(), v)),
                // NULL en una columna nativa: "ausente" si la clave es
                // opcional, si no la columna es nullable-por-tipo (`x: T?`) y
                // NULL significa `Value::Null` con la clave presente. `x?: T?`
                // con T nativo nunca llega acá -- ColumnPlan::for_field lo
                // fuerza a `json` para tener el 3er estado.
                None if col.field.optional => {}
                None => out.push((col.field.name.clone(), Value::Null)),
            }
        }
        out
    }

    /// Valor a bindear para `col`, dado el valor del `Value::Struct` de entrada
    /// en esa clave (`None` si la clave está ausente -- solo alcanzable si
    /// `col.field.optional`, ver `ColumnPlan`). Inversa de `row_to_fields`.
    fn write_param(&self, col: &ColumnPlan, slot: Option<&Value>) -> Cell {
        let Some(v) = slot else { return Cell::Null };
        if col.json {
            // `value_to_json(Value::Null)` da el JSON `null` -- exactamente el
            // sentinel de "presente pero null" que el caso `x?: T?` necesita,
            // sin ningún código especial acá. Y no es lo mismo que un NULL de
            // SQL, que significa "clave ausente": los dos backends conservan
            // esa diferencia (TEXT "null" en SQLite, JSONB null en PostgreSQL).
            return Cell::Json(value_to_json(v, &self.simple_enums));
        }
        match v {
            Value::Null => Cell::Null,
            Value::Int(n) => Cell::Int(*n),
            Value::Int64(n) => Cell::Int(*n),
            Value::Timestamp(n) => Cell::Int(*n),
            Value::Float(f) => Cell::Float(*f),
            Value::Str(s) => Cell::Text(s.clone()),
            Value::Bool(b) => Cell::Bool(*b),
            Value::Variant { variant, .. } => Cell::Text(variant.clone()),
            other => panic!("valor no representable en una columna nativa de SQL: {other:?}"),
        }
    }
}

/// Extrae el nombre de campo de un argumento `sumBy`/`countBy`/`avgBy`/
/// `maxBy`/`minBy` (GRAMMAR.md §3.52) -- defensivo, no un caso esperado en
/// la práctica: el checker ya garantizó en compilación que cada argumento
/// es exactamente `|item: T| item.campo` (mismo criterio que el
/// `unwrap_or_else` de `@content_type` en `server.rs`).
fn closure_field_name(arg: Option<&Value>, role: &str) -> Result<String, RuntimeError> {
    let Some(Value::Closure(params, body, _)) = arg else {
        return Err(RuntimeError::new(format!("selector {role} inválido: se esperaba un closure")));
    };
    crate::ast::recognize_field_selector(params, body)
        .map(str::to_string)
        .ok_or_else(|| RuntimeError::new(format!("selector {role} inválido: se esperaba `|item: T| item.campo`")))
}

/// Convierte la celda de `key`/`value` que devuelve `select_grouped` --
/// más simple que `row_to_fields` porque acá NUNCA hay JSON: el checker ya
/// exigió que tanto el campo de agrupación como el de valor sean
/// escalares NO opcionales (`check_aggregate_by`, checker.rs), y una
/// columna `NOT NULL` agregada sobre al menos una fila real (GROUP BY
/// nunca produce un grupo vacío) no puede devolver `Cell::Null` -- si
/// llega, es una violación de esa invariante, no una condición normal.
///
/// `ty` importa para el caso `Type::Enum`: agrupar por un campo enum
/// (ej. `countBy(|o: Order| { o.status })`) tiene que devolver una `key`
/// del tipo enum REAL, no un `String` -- el checker ya le prometió eso al
/// programa (`field_selector` devuelve el tipo declarado del campo tal
/// cual), así que acá hay que cumplirlo reconstruyendo `Value::Variant`,
/// mismo camino que ya usa `row_to_fields` para una columna enum normal
/// -- si no, el checker y el runtime terminarían en desacuerdo sobre qué
/// forma tiene el mismo valor (GRAMMAR.md §3.9).
fn scalar_cell_to_value(ty: &Type, cell: &Cell) -> Value {
    match (ty, cell) {
        (Type::Enum(name), Cell::Text(variant)) => {
            Value::Variant { enum_name: name.clone(), variant: variant.clone(), fields: Vec::new() }
        }
        // ANTES de la rama genérica de abajo: `Int` e `Int64` comparten
        // `ColumnKind::Int` (mismo `BIGINT`/`INTEGER PRIMARY KEY` de
        // storage, GRAMMAR.md §3.65) así que la única forma de saber cuál
        // de los dos `Value` armar es mirando el `Type` declarado, no la
        // `Cell` -- que es idéntica para los dos.
        (Type::Int64, Cell::Int(n)) => Value::Int64(*n),
        (_, Cell::Int(n)) => Value::Int(*n),
        (_, Cell::Float(f)) => Value::Float(*f),
        (_, Cell::Text(t)) => Value::Str(t.clone()),
        (_, Cell::Bool(b)) => Value::Bool(*b),
        (ty, cell) => panic!("una agregación devolvió {cell:?} para una columna declarada {ty}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_remote_notification_decodes_a_well_formed_payload_from_another_instance() {
        let payload = serde_json::json!({
            "instance": "otra-instancia",
            "collection": "items",
            "event": {"id": 1, "name": "hola"},
        })
        .to_string();
        let change = parse_remote_notification(&payload, "mi-instancia").expect("debió parsear");
        assert_eq!(change.collection, "items");
        assert_eq!(change.event, serde_json::json!({"id": 1, "name": "hola"}));
    }

    #[test]
    fn parse_remote_notification_discards_its_own_echo() {
        let payload = serde_json::json!({
            "instance": "mi-instancia",
            "collection": "items",
            "event": {"id": 1, "name": "hola"},
        })
        .to_string();
        assert!(
            parse_remote_notification(&payload, "mi-instancia").is_none(),
            "un NOTIFY con el mismo instance_id es el propio eco -- ya se publicó local al escribir"
        );
    }

    #[test]
    fn parse_remote_notification_ignores_malformed_payloads_instead_of_panicking() {
        assert!(parse_remote_notification("no es json", "cualquiera").is_none());
        assert!(parse_remote_notification("{}", "cualquiera").is_none());
        assert!(parse_remote_notification(r#"{"instance":"x"}"#, "cualquiera").is_none(), "falta collection/event");
    }

    #[test]
    fn test_db_delete_removes_row() {
        let program = crate::parser::parse(crate::lexer::tokenize("type User = { id: Int, name: String }\ndb { users: User[] }").unwrap()).unwrap();
        let db = Db::new(&program, Path::new(":memory:"));

        let user = db.call("users", "insert", vec![Value::Struct(vec![("name".into(), Value::Str("Alice".into()))])]).unwrap();
        let Value::Struct(fields) = &user else { panic!("se esperaba struct") };
        let id = fields.iter().find(|(n, _)| n == "id").map(|(_, v)| as_int(v).unwrap()).unwrap();

        let deleted = db.call("users", "delete", vec![Value::Int(id)]).unwrap();
        assert_eq!(deleted, Value::Bool(true));

        let find_res = db.call("users", "find", vec![Value::Int(id)]).unwrap();
        assert_eq!(find_res, Value::Null);

        let delete_again = db.call("users", "delete", vec![Value::Int(id)]).unwrap();
        assert_eq!(delete_again, Value::Bool(false));
    }

    #[test]
    fn test_db_page_pushes_limit_offset_to_sql_instead_of_fetching_everything() {
        let program = crate::parser::parse(crate::lexer::tokenize("type User = { id: Int, name: String }\ndb { users: User[] }").unwrap()).unwrap();
        let db = Db::new(&program, Path::new(":memory:"));

        let mut ids = Vec::new();
        for name in ["Ana", "Beto", "Cami", "Dani", "Ema"] {
            let row = db.call("users", "insert", vec![Value::Struct(vec![("name".into(), Value::Str(name.into()))])]).unwrap();
            let Value::Struct(fields) = row else { panic!("se esperaba struct") };
            ids.push(fields.iter().find(|(n, _)| n == "id").map(|(_, v)| as_int(v).unwrap()).unwrap());
        }

        let ids_of = |v: Value| -> Vec<i64> {
            let Value::List(items) = v else { panic!("se esperaba List") };
            items
                .into_iter()
                .map(|item| {
                    let Value::Struct(fields) = item else { panic!("se esperaba struct") };
                    fields.into_iter().find(|(n, _)| n == "id").map(|(_, v)| as_int(&v).unwrap()).unwrap()
                })
                .collect()
        };

        let page1 = db.call("users", "page", vec![Value::Int(2), Value::Int(0)]).unwrap();
        assert_eq!(ids_of(page1), ids[0..2], "primera página: los primeros 2 por id");

        let page2 = db.call("users", "page", vec![Value::Int(2), Value::Int(2)]).unwrap();
        assert_eq!(ids_of(page2), ids[2..4], "segunda página: sigue sin solaparse con la primera");

        let last = db.call("users", "page", vec![Value::Int(2), Value::Int(4)]).unwrap();
        assert_eq!(ids_of(last), ids[4..5], "última página parcial: lo que queda, no un error");

        let past_the_end = db.call("users", "page", vec![Value::Int(2), Value::Int(100)]).unwrap();
        assert_eq!(ids_of(past_the_end), Vec::<i64>::new(), "offset más allá del final: lista vacía");

        assert!(
            db.call("users", "page", vec![Value::Int(2), Value::Int(-1)]).is_err(),
            "offset negativo tiene que fallar en vez de mandarse tal cual al SQL"
        );
        assert!(
            db.call("users", "page", vec![Value::Int(-1), Value::Int(0)]).is_err(),
            "limit negativo tiene que fallar en vez de mandarse tal cual al SQL"
        );
    }

    #[test]
    fn test_db_page_after_pushes_a_cursor_predicate_to_sql() {
        let program = crate::parser::parse(crate::lexer::tokenize("type User = { id: Int, name: String }\ndb { users: User[] }").unwrap()).unwrap();
        let db = Db::new(&program, Path::new(":memory:"));

        let mut ids = Vec::new();
        for name in ["Ana", "Beto", "Cami", "Dani", "Ema"] {
            let row = db.call("users", "insert", vec![Value::Struct(vec![("name".into(), Value::Str(name.into()))])]).unwrap();
            let Value::Struct(fields) = row else { panic!("se esperaba struct") };
            ids.push(fields.iter().find(|(n, _)| n == "id").map(|(_, v)| as_int(v).unwrap()).unwrap());
        }

        let ids_of = |v: Value| -> Vec<i64> {
            let Value::List(items) = v else { panic!("se esperaba List") };
            items
                .into_iter()
                .map(|item| {
                    let Value::Struct(fields) = item else { panic!("se esperaba struct") };
                    fields.into_iter().find(|(n, _)| n == "id").map(|(_, v)| as_int(&v).unwrap()).unwrap()
                })
                .collect()
        };

        // Primera página: cursor null.
        let page1 = db.call("users", "pageAfter", vec![Value::Null, Value::Int(2)]).unwrap();
        assert_eq!(ids_of(page1), ids[0..2], "primera página: null trae desde el principio");

        // Segunda página: el cursor es el id del último elemento visto.
        let page2 = db.call("users", "pageAfter", vec![Value::Int(ids[1]), Value::Int(2)]).unwrap();
        assert_eq!(ids_of(page2), ids[2..4], "segunda página: sigue justo después del cursor");

        let last = db.call("users", "pageAfter", vec![Value::Int(ids[3]), Value::Int(2)]).unwrap();
        assert_eq!(ids_of(last), ids[4..5], "última página parcial: lo que queda, no un error");

        let past_the_end = db.call("users", "pageAfter", vec![Value::Int(ids[4]), Value::Int(2)]).unwrap();
        assert_eq!(ids_of(past_the_end), Vec::<i64>::new(), "cursor en el último id: lista vacía, no un error");

        // La propiedad que motiva el cursor sobre offset: una fila insertada
        // ENTRE dos llamadas nunca desplaza la página siguiente -- a
        // diferencia de `page(limit, offset)`, que cuenta desde el
        // principio de la tabla en cada llamada.
        let inserted_between =
            db.call("users", "insert", vec![Value::Struct(vec![("name".into(), Value::Str("Fabi".into()))])]).unwrap();
        let Value::Struct(fields) = inserted_between else { panic!("se esperaba struct") };
        let new_id = fields.iter().find(|(n, _)| n == "id").map(|(_, v)| as_int(v).unwrap()).unwrap();
        let page2_again = db.call("users", "pageAfter", vec![Value::Int(ids[1]), Value::Int(2)]).unwrap();
        assert_eq!(
            ids_of(page2_again),
            ids[2..4],
            "una fila nueva insertada DESPUÉS de la página 1 no corre la página 2 -- estable bajo escritura concurrente"
        );
        assert!(new_id > ids[4], "la fila nueva quedó al final por autoincremento, no en medio");

        assert!(
            db.call("users", "pageAfter", vec![Value::Null, Value::Int(-1)]).is_err(),
            "limit negativo tiene que fallar en vez de mandarse tal cual al SQL"
        );
    }

    #[test]
    fn test_db_autoincrement_does_not_reuse_ids() {
        let program = crate::parser::parse(crate::lexer::tokenize("type User = { id: Int, name: String }\ndb { users: User[] }").unwrap()).unwrap();
        let db = Db::new(&program, Path::new(":memory:"));

        let u1 = db.call("users", "insert", vec![Value::Struct(vec![("name".into(), Value::Str("Alice".into()))])]).unwrap();
        let u2 = db.call("users", "insert", vec![Value::Struct(vec![("name".into(), Value::Str("Bob".into()))])]).unwrap();

        let Value::Struct(f1) = u1 else { panic!() };
        let Value::Struct(f2) = u2 else { panic!() };

        let id1 = f1.iter().find(|(n, _)| n == "id").map(|(_, v)| as_int(v).unwrap()).unwrap();
        let id2 = f2.iter().find(|(n, _)| n == "id").map(|(_, v)| as_int(v).unwrap()).unwrap();
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);

        db.call("users", "delete", vec![Value::Int(id2)]).unwrap();

        let u3 = db.call("users", "insert", vec![Value::Struct(vec![("name".into(), Value::Str("Charlie".into()))])]).unwrap();
        let Value::Struct(f3) = u3 else { panic!() };
        let id3 = f3.iter().find(|(n, _)| n == "id").map(|(_, v)| as_int(v).unwrap()).unwrap();
        assert_eq!(id3, 3);
    }

    #[test]
    fn test_delete_where_and_find_where_error_instead_of_ignoring_the_predicate() {
        // `Db::call` no tiene acceso a `call_callable` (ver el comentario en
        // el brazo `"deleteWhere" | "findWhere"`), así que NO puede evaluar
        // un predicado de verdad -- antes, llamar estos dos métodos directo
        // acá (en vez de a través de `runtime::call_method`, que sí
        // intercepta y evalúa el predicado fila por fila) borraba/devolvía
        // TODAS las filas en silencio, ignorando el predicado por completo.
        // Ahora tiene que fallar con un mensaje claro en vez de dar un
        // resultado que parece válido y no lo es.
        let program = crate::parser::parse(crate::lexer::tokenize("type User = { id: Int, name: String }\ndb { users: User[] }").unwrap()).unwrap();
        let db = Db::new(&program, Path::new(":memory:"));

        db.call("users", "insert", vec![Value::Struct(vec![("name".into(), Value::Str("Alice".into()))])]).unwrap();
        db.call("users", "insert", vec![Value::Struct(vec![("name".into(), Value::Str("Bob".into()))])]).unwrap();

        let fake_predicate = Value::Bool(false);
        let find_err = db.call("users", "findWhere", vec![fake_predicate.clone()]).unwrap_err();
        assert!(find_err.to_string().contains("call_method"), "el error debe explicar que hay que pasar por el intérprete: {find_err}");

        let delete_err = db.call("users", "deleteWhere", vec![fake_predicate]).unwrap_err();
        assert!(delete_err.to_string().contains("call_method"));

        // Ninguna de las dos llamadas (que fallaron) debe haber tocado las filas.
        let remaining = db.call("users", "all", vec![]).unwrap();
        let Value::List(rows) = remaining else { panic!("se esperaba lista") };
        assert_eq!(rows.len(), 2, "un método que falla no debe borrar nada");
    }

    #[test]
    fn an_int64_column_survives_insert_and_read_back_exactly_at_i64_extremes() {
        // Este es el test que agarraría un brazo unreachable!()/panic!()
        // olvidado en native_sql_type/row_to_fields/write_param -- no solo
        // que "algún" valor sobreviva, sino que los extremos reales de i64
        // (donde una conversión a f64/otro tipo más angosto sí perdería
        // datos) lo hagan exactos.
        let program = crate::parser::parse(
            crate::lexer::tokenize("type Item = { id: Int, big: Int64 }\ndb { items: Item[] }").unwrap(),
        )
        .unwrap();
        let db = Db::new(&program, Path::new(":memory:"));

        for extreme in [i64::MIN, i64::MAX, 0] {
            let inserted = db
                .call("items", "insert", vec![Value::Struct(vec![("big".into(), Value::Int64(extreme))])])
                .unwrap();
            let Value::Struct(fields) = &inserted else { panic!("se esperaba struct") };
            let id = fields.iter().find(|(n, _)| n == "id").map(|(_, v)| as_int(v).unwrap()).unwrap();

            let found = db.call("items", "find", vec![Value::Int(id)]).unwrap();
            let Value::Struct(found_fields) = found else { panic!("se esperaba struct") };
            let big = found_fields.iter().find(|(n, _)| n == "big").map(|(_, v)| v.clone()).unwrap();
            assert_eq!(big, Value::Int64(extreme), "Int64 debe sobrevivir insert+find exacto en {extreme}");
        }
    }

    #[test]
    fn a_timestamp_column_survives_insert_and_read_back_exactly() {
        // A diferencia de Int64 (string en el wire), acá la columna SQL
        // guarda milisegundos crudos -- el round-trip que importa acá es
        // Value::Timestamp(i64) -> INTEGER -> Value::Timestamp(i64), sin
        // pasar por ningún formateo/parseo de ISO-8601 (eso es un borde
        // distinto, ya cubierto en runtime/mod.rs).
        let program = crate::parser::parse(
            crate::lexer::tokenize("type Event = { id: Int, at: Timestamp }\ndb { events: Event[] }").unwrap(),
        )
        .unwrap();
        let db = Db::new(&program, Path::new(":memory:"));

        for ms in [0i64, -1, 1_700_000_000_000] {
            let inserted = db
                .call("events", "insert", vec![Value::Struct(vec![("at".into(), Value::Timestamp(ms))])])
                .unwrap();
            let Value::Struct(fields) = &inserted else { panic!("se esperaba struct") };
            let id = fields.iter().find(|(n, _)| n == "id").map(|(_, v)| as_int(v).unwrap()).unwrap();

            let found = db.call("events", "find", vec![Value::Int(id)]).unwrap();
            let Value::Struct(found_fields) = found else { panic!("se esperaba struct") };
            let at = found_fields.iter().find(|(n, _)| n == "at").map(|(_, v)| v.clone()).unwrap();
            assert_eq!(at, Value::Timestamp(ms), "Timestamp debe sobrevivir insert+find exacto en {ms}");
        }
    }
}
