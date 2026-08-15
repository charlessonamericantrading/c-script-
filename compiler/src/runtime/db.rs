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
use rusqlite::types::Value as SqlValue;
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

pub struct Db {
    connection: Connection,
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

        Db { connection, checker, simple_enums, columns, subscribers: RefCell::new(HashMap::new()) }
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

    pub fn call(&self, collection: &str, method: &str, args: Vec<Value>) -> Result<Value, RuntimeError> {
        let columns = self.columns.get(collection).ok_or_else(|| RuntimeError::new(format!("colección desconocida: '{collection}'")))?;
        match method {
            "all" => self.select_rows(collection, columns, None).map(Value::List),
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
                let mut params = Vec::with_capacity(columns.len());
                for col in columns {
                    let slot = fields.iter().find(|(n, _)| n == &col.field.name).map(|(_, v)| v);
                    col_names.push(format!("\"{}\"", col.field.name));
                    params.push(self.write_param(col, slot));
                }
                let sql = if col_names.is_empty() {
                    format!("INSERT INTO \"{collection}\" DEFAULT VALUES")
                } else {
                    format!("INSERT INTO \"{collection}\" ({}) VALUES ({})", col_names.join(", "), vec!["?"; col_names.len()].join(", "))
                };
                self.connection
                    .execute(&sql, rusqlite::params_from_iter(params.iter()))
                    .map_err(|e| RuntimeError::new(format!("insert falló: {e}")))?;
                let new_id = self.connection.last_insert_rowid();
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
                let mut params = Vec::new();
                for (name, value) in &patch_fields {
                    // "id" nunca es escribible -- mismo criterio que insert,
                    // que también lo excluye de lo que el caller puede fijar.
                    let Some(col) = columns.iter().find(|c| name == &c.field.name) else { continue };
                    set_clauses.push(format!("\"{name}\" = ?"));
                    params.push(self.write_param(col, Some(value)));
                }
                if !set_clauses.is_empty() {
                    params.push(SqlValue::Integer(id));
                    let sql = format!("UPDATE \"{collection}\" SET {} WHERE \"id\" = ?", set_clauses.join(", "));
                    self.connection
                        .execute(&sql, rusqlite::params_from_iter(params.iter()))
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
                let sql = format!("DELETE FROM \"{collection}\" WHERE \"id\" = ?");
                let rows_affected = self
                    .connection
                    .execute(&sql, rusqlite::params![id])
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
                let mut stmt = self.connection.prepare(&sql).map_err(|e| RuntimeError::new(format!("error en count de '{collection}': {e}")))?;
                let count: i64 = stmt.query_row([], |r| r.get(0)).map_err(|e| RuntimeError::new(format!("error en count de '{collection}': {e}")))?;
                Ok(Value::Int(count))
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
    fn publish(&self, collection: &str, row: &Value) {
        let json = value_to_json(row, &self.simple_enums);
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

    /// `all` (`id: None`, ordenado por "id" para output determinístico,
    /// mismo orden de inserción que ya daba el `Vec` de antes) o `find`/la
    /// re-consulta de `insert`/`applyPatch` (`id: Some(_)`, a lo sumo 1 fila).
    fn select_rows(&self, collection: &str, columns: &[ColumnPlan], id: Option<i64>) -> Result<Vec<Value>, RuntimeError> {
        let mut col_list = vec!["\"id\"".to_string()];
        col_list.extend(columns.iter().map(|c| format!("\"{}\"", c.field.name)));
        let sql = match id {
            Some(_) => format!("SELECT {} FROM \"{collection}\" WHERE \"id\" = ?", col_list.join(", ")),
            None => format!("SELECT {} FROM \"{collection}\" ORDER BY \"id\"", col_list.join(", ")),
        };
        let mut stmt = self.connection.prepare(&sql).map_err(|e| RuntimeError::new(format!("error de SQL en '{collection}': {e}")))?;
        let to_value = |row: &rusqlite::Row| -> rusqlite::Result<Value> { Ok(Value::Struct(self.row_to_fields(row, columns)?)) };
        let rows = match id {
            Some(id) => stmt.query_map([id], to_value).and_then(Iterator::collect),
            None => stmt.query_map([], to_value).and_then(Iterator::collect),
        };
        rows.map_err(|e| RuntimeError::new(format!("error de SQL en '{collection}': {e}")))
    }

    /// Reconstruye una fila entera (`"id"` + cada columna declarada) como
    /// los pares `(nombre, Value)` de un `Value::Struct` -- inversa de
    /// `write_param`. El orden de columnas del SELECT (`select_rows`) y de
    /// `columns` siempre coincide, así que se puede leer por NOMBRE sin
    /// arriesgar un desajuste posicional.
    fn row_to_fields(&self, row: &rusqlite::Row, columns: &[ColumnPlan]) -> rusqlite::Result<Vec<(String, Value)>> {
        let mut out = Vec::with_capacity(columns.len() + 1);
        out.push(("id".to_string(), Value::Int(row.get::<_, i64>("id")?)));
        for col in columns {
            if col.json {
                let raw: Option<String> = row.get(col.field.name.as_str())?;
                match raw {
                    // SQL NULL en una columna JSON SIEMPRE significa "clave
                    // ausente" -- solo alcanzable si `field.optional` (ver
                    // `write_param`, nunca escribimos NULL acá si la clave
                    // es requerida). El `Value::Null` defensivo de abajo no
                    // debería ser alcanzable en la práctica.
                    None => {
                        if !col.field.optional {
                            out.push((col.field.name.clone(), Value::Null));
                        }
                    }
                    Some(text) => {
                        let parsed: serde_json::Value = serde_json::from_str(&text)
                            .unwrap_or_else(|e| panic!("JSON guardado por nosotros mismos no puede ser inválido: {e}"));
                        let decoded = json_to_typed_value(&parsed, &col.field.ty, &self.checker, &col.field.name)
                            .unwrap_or_else(|e| panic!("un valor que nosotros escribimos tiene que decodificar contra su propio tipo: {e}"));
                        out.push((col.field.name.clone(), decoded));
                    }
                }
            } else {
                let effective_ty: &Type = match &col.field.ty {
                    Type::Optional(inner) => inner.as_ref(),
                    other => other,
                };
                let value = match effective_ty {
                    Type::Int => row.get::<_, Option<i64>>(col.field.name.as_str())?.map(Value::Int),
                    Type::Int64 => row.get::<_, Option<i64>>(col.field.name.as_str())?.map(Value::Int64),
                    Type::Timestamp => row.get::<_, Option<i64>>(col.field.name.as_str())?.map(Value::Timestamp),
                    Type::Float => row.get::<_, Option<f64>>(col.field.name.as_str())?.map(Value::Float),
                    Type::String => row.get::<_, Option<String>>(col.field.name.as_str())?.map(Value::Str),
                    Type::Bool => row.get::<_, Option<i64>>(col.field.name.as_str())?.map(|n| Value::Bool(n != 0)),
                    Type::Enum(name) => row
                        .get::<_, Option<String>>(col.field.name.as_str())?
                        .map(|variant| Value::Variant { enum_name: name.clone(), variant, fields: Vec::new() }),
                    other => unreachable!("tipo nativo inesperado en una columna no-JSON: {other:?}"),
                };
                match value {
                    Some(v) => out.push((col.field.name.clone(), v)),
                    // SQL NULL en una columna nativa: "ausente" si la clave
                    // es opcional, si no la columna es nullable-por-tipo
                    // (`x: T?`) y NULL significa `Value::Null` con la clave
                    // presente. `x?: T?` con T nativo nunca llega acá --
                    // ColumnPlan::for_field lo fuerza a `json` para tener el
                    // 3er estado.
                    None if col.field.optional => {}
                    None => out.push((col.field.name.clone(), Value::Null)),
                }
            }
        }
        Ok(out)
    }

    /// Valor a bindear para `col`, dado el valor del `Value::Struct` de
    /// entrada en esa clave (`None` si la clave está ausente -- solo
    /// alcanzable si `col.field.optional`, ver `ColumnPlan`). Inversa de
    /// `row_to_fields`.
    fn write_param(&self, col: &ColumnPlan, slot: Option<&Value>) -> SqlValue {
        let Some(v) = slot else { return SqlValue::Null };
        if col.json {
            // `value_to_json(Value::Null)` serializa al texto `"null"` --
            // exactamente el sentinel de "presente pero null" que el caso
            // `x?: T?` necesita, sin ningún código especial acá.
            return SqlValue::Text(serde_json::to_string(&value_to_json(v, &self.simple_enums)).expect("serializar a JSON no puede fallar"));
        }
        match v {
            Value::Null => SqlValue::Null,
            Value::Int(n) => SqlValue::Integer(*n),
            Value::Int64(n) => SqlValue::Integer(*n),
            Value::Timestamp(n) => SqlValue::Integer(*n),
            Value::Float(f) => SqlValue::Real(*f),
            Value::Str(s) => SqlValue::Text(s.clone()),
            Value::Bool(b) => SqlValue::Integer(i64::from(*b)),
            Value::Variant { variant, .. } => SqlValue::Text(variant.clone()),
            other => panic!("valor no representable en una columna nativa de SQL: {other:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
