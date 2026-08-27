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
use crate::ast::{BinaryOp, FieldCheck, Item, Program, TimeGranularity, TypeAnnotation, TypeExpr};
use crate::checker::Checker;
use crate::types::{FieldType, Type};
use super::store::{Backend, Cell, ColumnKind};
use rusqlite::Connection;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::Duration;

/// Cuántos eventos sin consumir tolera un suscriptor de push real
/// (GRAMMAR.md §3.16) antes de ser desconectado. Un canal ILIMITADO sería
/// un vector real de agotamiento de memoria si un cliente se queda lento
/// (o la conexión se cuelga) sin cerrarse -- `try_send` (nunca bloqueante,
/// ver `publish`) más este tope acotan el costo de un suscriptor colgado a
/// una cantidad fija, a costa de desconectarlo si se atrasa demasiado (el
/// mismo trade-off que la mayoría de sistemas de pub-sub/broadcast reales
/// hacen). No es un número investigado a fondo, es un default razonable.
const LIVE_STREAM_BUFFER: usize = 1024;

/// Default de `http_timeout` (GRAMMAR.md §3.86) hasta que `server.rs` lo
/// sobreescribe según `--http-timeout`/`LINK_HTTP_TIMEOUT` -- 30s, el mismo
/// número que `ureq` ya usa como timeout de CONEXIÓN por default (que sí
/// tiene uno; lo que faltaba era el de lectura/escritura). No es un número
/// investigado a fondo, es un default razonable -- mismo criterio que
/// `LIVE_STREAM_BUFFER` arriba.
const DEFAULT_HTTP_TIMEOUT: Duration = Duration::from_secs(30);

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
            Type::Int | Type::Int64 => ColumnKind::Int,
            // GRAMMAR.md §3.91: aparte de `Int`/`Int64` -- del lado Postgres
            // puede ser un `BIGINT` propio de c-script O un `date`/
            // `timestamp`/`timestamptz` nativo de una tabla adoptada, ver
            // `ColumnKind::Timestamp`.
            Type::Timestamp => ColumnKind::Timestamp,
            Type::Float => ColumnKind::Float,
            Type::Bool => ColumnKind::Bool,
            Type::String | Type::Uuid | Type::Enum(_) => ColumnKind::Text,
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
        // Misma columna TEXT que String -- la validación de forma
        // (GRAMMAR.md §3.70) ya pasó en el borde JSON antes de que un
        // Value::Uuid pueda siquiera llegar acá, así que la columna física
        // no necesita ningún constraint propio.
        Type::Uuid => Some("TEXT"),
        Type::Bool => Some("INTEGER"),
        Type::Enum(name) if simple_enums.contains(name) => Some("TEXT"),
        _ => None,
    }
}

/// Nombre de colección -> nombre del campo `@softDelete` de su tipo de
/// elemento, para cada colección que tenga uno (GRAMMAR.md §3.78, checker.rs
/// ya garantizó a lo sumo uno por struct). `checker.db_collections()` da el
/// `Type` YA RESUELTO (sin anotaciones); acá se cruza con `program.items`
/// (que sí tiene `ast::Field` con anotaciones) por el `name: Some(...)` que
/// el `Type::Struct` de un elemento de colección siempre conserva.
fn soft_delete_fields_by_collection(program: &Program, checker: &Checker) -> HashMap<String, String> {
    let mut result = HashMap::new();
    for (coll_name, element_ty) in checker.db_collections() {
        let Type::Struct { name: Some(type_name), .. } = element_ty else { continue };
        for item in &program.items {
            let Item::Type(t) = item else { continue };
            if &t.name != type_name {
                continue;
            }
            let TypeExpr::Struct(fields) = &t.ty else { continue };
            if let Some(f) = fields.iter().find(|f| f.soft_delete()) {
                result.insert(coll_name.clone(), f.name.clone());
            }
        }
    }
    result
}

/// Nombre de colección -> lista de `(campo, unique)` para cada campo con
/// `@index`/`@unique` de su tipo de elemento (GRAMMAR.md §3.80) -- mismo
/// cruce `checker.db_collections()` + `program.items` que
/// `soft_delete_fields_by_collection`, mismo motivo (las anotaciones viven
/// en `ast::Field`, no en el `Type` ya resuelto).
pub(crate) fn index_fields_by_collection(program: &Program, checker: &Checker) -> HashMap<String, Vec<(String, bool)>> {
    let mut result = HashMap::new();
    for (coll_name, element_ty) in checker.db_collections() {
        let Type::Struct { name: Some(type_name), .. } = element_ty else { continue };
        for item in &program.items {
            let Item::Type(t) = item else { continue };
            if &t.name != type_name {
                continue;
            }
            let TypeExpr::Struct(fields) = &t.ty else { continue };
            let indexed: Vec<(String, bool)> =
                fields.iter().filter_map(|f| f.index().map(|unique| (f.name.clone(), unique))).collect();
            if !indexed.is_empty() {
                result.insert(coll_name.clone(), indexed);
            }
        }
    }
    result
}

/// Nombre de colección -> lista de `(campo, FieldCheck)` para cada campo con
/// `@check` de su tipo de elemento (GRAMMAR.md §3.96) -- mismo cruce
/// `checker.db_collections()` + `program.items` que `index_fields_by_collection`
/// de abajo, mismo motivo: `ColumnPlan`/`Type::Struct` son estructurales,
/// sin anotaciones -- solo el `ast::Field` original las tiene.
pub(crate) fn check_fields_by_collection(program: &Program, checker: &Checker) -> HashMap<String, Vec<(String, FieldCheck)>> {
    let mut result = HashMap::new();
    for (coll_name, element_ty) in checker.db_collections() {
        let Type::Struct { name: Some(type_name), .. } = element_ty else { continue };
        for item in &program.items {
            let Item::Type(t) = item else { continue };
            if &t.name != type_name {
                continue;
            }
            let TypeExpr::Struct(fields) = &t.ty else { continue };
            let checks: Vec<(String, FieldCheck)> = fields.iter().filter_map(|f| f.check().map(|c| (f.name.clone(), c.clone()))).collect();
            if !checks.is_empty() {
                result.insert(coll_name.clone(), checks);
            }
        }
    }
    result
}

/// `CREATE [UNIQUE] INDEX IF NOT EXISTS ...` para cada campo `@index`/
/// `@unique` de `collection` (GRAMMAR.md §3.80) -- `IF NOT EXISTS` hace la
/// creación idempotente, así que corre en CADA arranque sin necesitar
/// detectar "¿ya existía?" (a diferencia de `ADD COLUMN`, que sí necesita
/// ese chequeo porque no es idempotente por sí solo). Nombre de índice
/// determinístico (`idx_<tabla>_<campo>`) para que dos arranques sucesivos
/// generen el MISMO nombre -- si generara uno al azar, cada arranque
/// crearía un índice nuevo en vez de reconocer el que ya existe.
pub(crate) fn create_index_statements(collection: &str, indexed: &[(String, bool)]) -> Vec<String> {
    indexed
        .iter()
        .map(|(field, unique)| {
            let unique_kw = if *unique { "UNIQUE " } else { "" };
            format!("CREATE {unique_kw}INDEX IF NOT EXISTS \"idx_{collection}_{field}\" ON \"{collection}\"(\"{field}\")")
        })
        .collect()
}

/// Nombre de colección -> lista de conjuntos de campos con `@unique(...)`
/// COMPUESTO de su tipo de elemento (GRAMMAR.md §3.155) -- mismo cruce
/// `checker.db_collections()` + `program.items` que `index_fields_by_collection`
/// arriba, mismo motivo (la anotación vive en `ast::TypeDecl`, no en el
/// `Type` ya resuelto).
pub(crate) fn composite_unique_by_collection(program: &Program, checker: &Checker) -> HashMap<String, Vec<Vec<String>>> {
    let mut result = HashMap::new();
    for (coll_name, element_ty) in checker.db_collections() {
        let Type::Struct { name: Some(type_name), .. } = element_ty else { continue };
        for item in &program.items {
            let Item::Type(t) = item else { continue };
            if &t.name != type_name {
                continue;
            }
            let sets: Vec<Vec<String>> =
                t.annotations.iter().map(|TypeAnnotation::Unique(fields)| fields.clone()).collect();
            if !sets.is_empty() {
                result.insert(coll_name.clone(), sets);
            }
        }
    }
    result
}

/// `CREATE UNIQUE INDEX IF NOT EXISTS ...` de VARIAS columnas a la vez, uno
/// por cada `@unique(...)` de nivel `type` (GRAMMAR.md §3.155) -- mismo
/// criterio de idempotencia y nombre determinístico que `create_index_statements`
/// (arriba), con el nombre de TODOS los campos codificados sin ambigüedad
/// (`composite_unique_index_name` abajo) para que dos constraints
/// compuestos sobre la misma tabla nunca colisionen de nombre.
pub(crate) fn create_composite_unique_statements(collection: &str, sets: &[Vec<String>]) -> Vec<String> {
    sets.iter()
        .map(|fields| {
            let idx_name = composite_unique_index_name(collection, fields);
            let cols = fields.iter().map(|f| format!("\"{f}\"")).collect::<Vec<_>>().join(", ");
            format!("CREATE UNIQUE INDEX IF NOT EXISTS \"{idx_name}\" ON \"{collection}\"({cols})")
        })
        .collect()
}

/// Nombre determinístico del índice para un `@unique(...)` compuesto --
/// bug real encontrado por una auditoría multi-agente adversarial
/// (26/08/2026): `format!("idx_{collection}_{}", fields.join("_"))` (la
/// forma original) es AMBIGUO cuando un nombre de campo ya contiene un
/// guion bajo -- `@unique(a_b, c)` y `@unique(a, b_c)` sobre el MISMO type
/// generan el mismo string `"idx_<t>_a_b_c"`. Con `CREATE UNIQUE INDEX IF
/// NOT EXISTS`, la segunda sentencia es un no-op silencioso: su constraint
/// nunca se crea de verdad, y el checker no lo atrapa (dedup por CONJUNTO
/// de campos, nunca por el nombre derivado) -- confirmado en vivo: una fila
/// que violaba el segundo `@unique` se aceptaba con 200, no el 400
/// documentado.
///
/// Codificación con prefijo de longitud (mismo principio que Bencode/
/// netstrings, con `$` como separador longitud/contenido -- nunca aparece
/// en un identificador c-script real): cada campo se codifica como
/// `"{len}${nombre}"`, concatenados SIN separador extra entre campos. Esto
/// es inyectivo -- dos secuencias DISTINTAS de nombres de campo nunca
/// producen la misma codificación, porque el prefijo de longitud de cada
/// entrada dice exactamente dónde termina, sin depender de que ningún
/// caracter esté ausente del nombre del campo (a diferencia de un `join`
/// con separador, que solo es seguro si el separador nunca puede aparecer
/// DENTRO de un campo -- exactamente la garantía que `_` no daba).
pub(crate) fn composite_unique_index_name(collection: &str, fields: &[String]) -> String {
    let encoded: String = fields.iter().map(|f| format!("{}${f}", f.len())).collect();
    format!("idx_{collection}_uniq_{encoded}")
}

/// ¿Es este el texto con el que SQLite o Postgres reportan una violación de
/// `UNIQUE`/`@unique` (GRAMMAR.md §3.80)? `Backend::execute`/
/// `insert_returning_id` devuelven `Result<_, String>` -- el error ya
/// perdió cualquier forma estructurada para cuando llega acá, así que la
/// única señal disponible es la frase fija que cada motor usa para este
/// caso (la forma PÚBLICA en la que reportan el error, estable en la
/// práctica, aunque siga siendo texto y no un tipo). Encontrado probando
/// contra un servidor real: sin este chequeo, una violación de `@unique`
/// -- error del CLIENTE, mandó un valor repetido -- salía como 500 (`insert
/// falló: UNIQUE constraint failed: ...`), no como el 400 que le
/// corresponde.
fn is_unique_violation(msg: &str) -> bool {
    msg.contains("UNIQUE constraint failed") || msg.contains("duplicate key value violates unique constraint")
}

/// ¿Es este el texto con el que SQLite o Postgres reportan una violación de
/// `CHECK`/`@check` (GRAMMAR.md §3.96)? Mismo criterio que
/// `is_unique_violation` -- texto fijo de cada motor, no un tipo
/// estructurado (`Backend::execute` ya perdió esa forma para cuando llega
/// acá). En la práctica, `apply_field_validators` (`runtime/mod.rs`) ya
/// atrapa esto ANTES de que la sentencia SQL siquiera se arme -- este
/// caso es la segunda barrera (defensa en profundidad), alcanzable solo si
/// algo escribe sin pasar por ese camino.
fn is_check_violation(msg: &str) -> bool {
    msg.contains("CHECK constraint failed") || msg.contains("violates check constraint")
}

/// Envuelve el error de una escritura fallida (`insert`/`applyPatch`) --
/// `RuntimeError::bad_request` (400) si es una violación de `@unique` (ver
/// `is_unique_violation`) o de `@check` (ver `is_check_violation`),
/// `RuntimeError::new` (500) para cualquier otra falla de SQL genuina
/// (columna inexistente, base caída, etc.).
fn write_error(action: &str, e: String) -> RuntimeError {
    if is_unique_violation(&e) {
        RuntimeError::bad_request(format!("ya existe una fila con ese valor único (@unique, GRAMMAR.md §3.80) -- {e}"))
    } else if is_check_violation(&e) {
        RuntimeError::bad_request(format!("un valor no cumple una restricción @check (GRAMMAR.md §3.96) -- {e}"))
    } else {
        RuntimeError::new(format!("{action} falló: {e}"))
    }
}

/// `CHECK (...)` inline para UNA columna (GRAMMAR.md §3.96) -- misma
/// sintaxis para SQLite y PostgreSQL, así que esta función la comparten
/// `create_table_sql` (acá abajo) y `codegen::postgres_emit::create_postgres_table_sql`.
/// Comparaciones numéricas simples, sin ningún riesgo de inyección: `field`
/// siempre es un nombre de columna ya validado por el checker (nunca texto
/// de usuario), y `min`/`max` son `f64` formateados por Rust, nunca
/// interpolación de un string externo.
pub(crate) fn check_clause_sql(field: &str, check: &FieldCheck) -> String {
    match check {
        FieldCheck::Min(min) => format!("CHECK (\"{field}\" >= {min})"),
        FieldCheck::Max(max) => format!("CHECK (\"{field}\" <= {max})"),
        FieldCheck::Range(min, max) => format!("CHECK (\"{field}\" >= {min} AND \"{field}\" <= {max})"),
        // GRAMMAR.md §3.146: `length(...)` cuenta CARACTERES (no bytes) en
        // los dos motores para una columna de texto -- mismo criterio que
        // `check_string_length` (runtime/mod.rs) del lado de la aplicación,
        // ninguno de los dos cuenta bytes UTF-8.
        FieldCheck::MinLength(min) => format!("CHECK (length(\"{field}\") >= {min})"),
        FieldCheck::MaxLength(max) => format!("CHECK (length(\"{field}\") <= {max})"),
    }
}

fn create_table_sql(collection: &str, columns: &[ColumnPlan], checks: &[(String, FieldCheck)]) -> String {
    let mut defs = vec!["\"id\" INTEGER PRIMARY KEY AUTOINCREMENT".to_string()];

    for col in columns {
        let not_null = if col.not_null() { " NOT NULL" } else { "" };
        let check_clause = match checks.iter().find(|(name, _)| name == &col.field.name) {
            Some((_, c)) => format!(" {}", check_clause_sql(&col.field.name, c)),
            None => String::new(),
        };
        defs.push(format!("\"{}\" {}{}{}", col.field.name, col.sql_type, not_null, check_clause));
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

/// `true` si `collection` existe como tabla física en este archivo SQLite --
/// distinto de "`PRAGMA table_info` vino vacío" (`check_schema_matches` usa
/// eso para decir "recién la creó el `CREATE TABLE IF NOT EXISTS` de arriba
/// en esta misma llamada", que en modo adopción es justo el caso que NO
/// puede pasar: acá nunca se ejecuta ese `CREATE TABLE`).
fn sqlite_table_exists(connection: &Connection, collection: &str) -> bool {
    connection
        .query_row("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1", [collection], |_| Ok(()))
        .is_ok()
}

/// Adopción de tabla existente (`--adopt-existing`/`LINK_ADOPT_EXISTING`,
/// GRAMMAR.md §3.67): a diferencia de `check_schema_matches`, NUNCA ejecuta
/// `CREATE TABLE` ni `ALTER TABLE ADD COLUMN` -- el punto entero de este modo
/// es no tocar DDL, para bases donde el rol de la app puede no tener permiso
/// de crear/alterar tablas, o donde la tabla ya trae columnas que este
/// programa no modela y que hay que dejar intactas.
///
/// Solo valida que cada columna DECLARADA exista con un tipo compatible --
/// una columna física de más (no declarada en el `.link`) se ignora sin
/// queja, a propósito: es exactamente el caso de uso (adoptar una tabla
/// legacy con columnas que este programa todavía no necesita leer). NOT NULL
/// no se valida acá (límite honesto, GRAMMAR.md §3.67): una fila vieja con
/// NULL en un campo que el `.link` declara requerido recién falla en la
/// lectura que la toque, con el error normal de decode -- no al conectar.
fn check_schema_for_adoption(connection: &Connection, collection: &str, columns: &[ColumnPlan], db_path: &str) -> Result<(), RuntimeError> {
    if !sqlite_table_exists(connection, collection) {
        return Err(RuntimeError::new(format!(
            "la colección '{collection}' no existe como tabla en '{db_path}', pero --adopt-existing/LINK_ADOPT_EXISTING \
             asume que las tablas ya existen y no intenta crearlas. Sacá la flag si querés que c-script cree la tabla, \
             o creála a mano primero."
        )));
    }

    let mut stmt = connection
        .prepare(&format!("PRAGMA table_info(\"{collection}\")"))
        .map_err(|e| RuntimeError::new(format!("no se pudo leer el schema de '{collection}' en '{db_path}': {e}")))?;
    let raw: Vec<(String, String)> = stmt
        .query_map([], |row| Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?)))
        .and_then(Iterator::collect)
        .map_err(|e| RuntimeError::new(format!("no se pudo leer el schema de '{collection}' en '{db_path}': {e}")))?;
    let actual: HashMap<String, String> = raw.into_iter().map(|(name, decl_type)| (name, decl_type.to_uppercase())).collect();

    let mut missing = Vec::new();
    let mut incompatible = Vec::new();
    for col in columns {
        match actual.get(&col.field.name) {
            None => missing.push(col.field.name.clone()),
            Some(actual_type) if actual_type != col.sql_type => {
                incompatible.push(format!("'{}' declarado {} pero la tabla tiene {}", col.field.name, col.sql_type, actual_type))
            }
            Some(_) => {}
        }
    }

    if missing.is_empty() && incompatible.is_empty() {
        return Ok(());
    }
    let mut reasons = Vec::new();
    if !missing.is_empty() {
        reasons.push(format!("faltan columnas: [{}]", missing.join(", ")));
    }
    if !incompatible.is_empty() {
        reasons.push(format!("tipos incompatibles: [{}]", incompatible.join(", ")));
    }
    Err(RuntimeError::new(format!(
        "la colección '{collection}' en '{db_path}' no es compatible con lo que el programa declara ({}) -- \
         en modo --adopt-existing no se auto-migra nada, hay que ajustar la tabla física a mano.",
        reasons.join("; "),
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
pub(crate) fn validate_existing_id_column(backend: &Backend, collection: &str) -> Result<(), String> {
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

/// GRAMMAR.md §3.94: antes de agregarle columnas nuevas a una tabla que YA
/// EXISTÍA (la migración no destructiva de PostgreSQL, ver el loop de `ADD
/// COLUMN` en `connect_postgres_with_options`), avisa por stderr si esa
/// tabla no se PARECE en nada a lo que este programa declara -- podría ser
/// la tabla de OTRO programa que casualmente eligió el mismo nombre de
/// colección. Encontrado en una adopción real: `telemetry.link` estuvo a
/// punto de chocar así contra una tabla `events` real de otro servicio --
/// evitado a mano (renombrando la colección) porque alguien lo notó, no
/// porque el runtime lo hubiera avisado.
///
/// SOLO UNA ADVERTENCIA, nunca un error que corte el arranque -- a
/// diferencia del intento original de esta función (revertido durante esta
/// misma ronda al auditar `pg_integration.rs`): un test YA EXISTENTE,
/// deliberado y verificado
/// (`two_different_link_files_declaring_disjoint_columns_of_the_same_table_can_read_each_others_rows_but_not_always_write`)
/// prueba que DOS `.link` con columnas DISJUNTAS (cero nombres en común,
/// aparte de "id") sobre la MISMA tabla es un patrón SOPORTADO a propósito
/// -- schema por convención de nombre de colección, no por comparación de
/// columnas. Bloquear ese caso habría roto una feature ya shipeada; el
/// mismo shape de datos (tabla preexistente, cero overlap) es indistinguible
/// entre "colisión accidental" y "columnas compartidas a propósito" -- así
/// que la señal más honesta que se puede dar es un aviso legible por un
/// humano, no una negativa automática.
fn warn_if_table_looks_unrelated(backend: &Backend, collection: &str, columns: &[ColumnPlan]) {
    // Sin ninguna columna propia declarada (un struct que solo tiene "id"),
    // no hay ninguna señal -- positiva o negativa -- que comparar.
    if columns.is_empty() {
        return;
    }
    let sql = format!("SELECT column_name FROM information_schema.columns WHERE table_name = {}", backend.placeholder(1));
    let Ok(rows) = backend.query(&sql, &[Cell::Text(collection.to_string())], &[ColumnKind::Text]) else {
        // Best-effort: un fallo acá nunca debe impedir que el connect siga
        // su curso normal -- el resto del código ya maneja errores reales
        // de conexión en otro lado.
        return;
    };
    let existing: HashSet<String> =
        rows.into_iter().filter_map(|row| row.into_iter().next()).filter_map(|cell| if let Cell::Text(s) = cell { Some(s) } else { None }).collect();

    // Vacío: la tabla NO existía antes de esta corrida -- el CREATE TABLE IF
    // NOT EXISTS de arriba recién la creó, con exactamente estas columnas
    // (nada que avisar, por construcción coincide).
    if existing.is_empty() {
        return;
    }

    let declared: Vec<&str> = columns.iter().map(|c| c.field.name.as_str()).collect();

    // GRAMMAR.md §3.94: nombres de convención de auditoría (`createdAt`/
    // `updatedAt`/`deletedAt` -- la misma terna que `@autoUpdate`/
    // `@softDelete` promueven como estándar en todo el lenguaje, GRAMMAR.md
    // §3.68/§3.63) son tan comunes entre programas SIN ninguna relación
    // real entre sí que, solos, no son evidencia de nada: dos servicios de
    // dos equipos distintos casi seguro los nombran igual por seguir la
    // convención del lenguaje, no porque compartan la tabla a propósito --
    // el landmine real que esta lista cierra es una colisión accidental de
    // nombre de tabla que la advertencia de arriba NO detecta solo porque
    // las dos partes, sin relación, declararon `createdAt`. Si el struct
    // declara al menos un campo FUERA de esta lista, la comparación de
    // overlap lo ignora; si el struct no tiene NINGÚN campo fuera de ella
    // (caso raro -- un struct compuesto solo por campos de auditoría), cae
    // de vuelta a considerarlos a todos, mejor una señal débil que ninguna.
    const GENERIC_AUDIT_FIELDS: [&str; 3] = ["createdAt", "updatedAt", "deletedAt"];
    let meaningful: Vec<&str> = declared.iter().copied().filter(|name| !GENERIC_AUDIT_FIELDS.contains(name)).collect();
    let evidence_set: &[&str] = if meaningful.is_empty() { &declared } else { &meaningful };
    if evidence_set.iter().any(|name| existing.contains(*name)) {
        return;
    }

    let mut existing_sorted: Vec<String> = existing.into_iter().collect();
    existing_sorted.sort();
    eprintln!(
        "advertencia: la tabla '{collection}' ya existe en PostgreSQL, pero NINGUNA de las columnas que '{collection}' \
         declara ([{}]) coincide con las que la tabla ya tiene ([{}]). Si dos .link comparten esta tabla A PROPÓSITO \
         (columnas disjuntas, GRAMMAR.md §3.17), esta advertencia es esperada y no requiere ninguna acción. Si NO es \
         así, es probable que '{collection}' le pertenezca a OTRO programa que casualmente eligió el mismo nombre de \
         colección -- revisá antes de seguir, o renombrá la colección en este .link.",
        declared.join(", "),
        existing_sorted.join(", "),
    );
}

/// Equivalente de `check_schema_for_adoption` (SQLite) para PostgreSQL:
/// modo `--adopt-existing`/`LINK_ADOPT_EXISTING` (GRAMMAR.md §3.67), llamado
/// EN VEZ de `CREATE TABLE IF NOT EXISTS` + el loop de `ADD COLUMN` que
/// `connect_postgres` corre normalmente -- ninguno de los dos se ejecuta acá,
/// a propósito, porque el punto del modo adopción es no requerir permiso de
/// DDL sobre la base ajena. Solo lee `information_schema.columns` (un SELECT
/// común) para confirmar que cada columna DECLARADA existe -- no valida su
/// tipo columna por columna como sí hace la versión de SQLite: acá alcanza
/// con "existe", el mismo criterio que `validate_existing_id_column` ya usa
/// para "id" (un tipo incompatible en una columna que no sea "id" se
/// descubre en la primera lectura/escritura, con el error normal de
/// `store.rs`, no acá -- límite honesto, documentado en GRAMMAR.md §3.67).
fn validate_columns_exist_for_adoption(backend: &Backend, collection: &str, columns: &[ColumnPlan]) -> Result<(), String> {
    let sql = format!("SELECT column_name FROM information_schema.columns WHERE table_name = {}", backend.placeholder(1));
    let rows = backend
        .query(&sql, &[Cell::Text(collection.to_string())], &[ColumnKind::Text])
        .map_err(|e| format!("no se pudo verificar el esquema de '{collection}' en PostgreSQL: {e}"))?;

    if rows.is_empty() {
        return Err(format!(
            "la colección '{collection}' no existe como tabla en PostgreSQL, pero --adopt-existing/LINK_ADOPT_EXISTING \
             asume que las tablas ya existen y no intenta crearlas. Sacá la flag si querés que c-script cree la tabla, \
             o creála a mano primero."
        ));
    }
    let actual: HashSet<String> =
        rows.into_iter().filter_map(|row| row.into_iter().next()).filter_map(|cell| if let Cell::Text(s) = cell { Some(s) } else { None }).collect();

    let missing: Vec<&str> = columns.iter().map(|c| c.field.name.as_str()).filter(|name| !actual.contains(*name)).collect();
    if missing.is_empty() {
        return Ok(());
    }
    Err(format!(
        "la colección '{collection}' en PostgreSQL no tiene las columnas [{}] que el programa declara -- en modo \
         --adopt-existing no se auto-migra nada, hay que agregarlas a mano.",
        missing.join(", "),
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
    /// §3.16). `Mutex`, no `RefCell` -- Pilar 1 del roadmap de concurrencia
    /// (26/08/2026): con un hilo por request (`runtime/server.rs`), dos
    /// requests pueden tocar esto AL MISMO TIEMPO de verdad (una
    /// suscribiéndose, otra publicando). Nunca se toca desde ningún hilo
    /// escritor de stream (que solo recibe el `Receiver`, ya extraído,
    /// nunca vuelve a tocar `Db`) -- solo desde hilos de request.
    subscribers: parking_lot::Mutex<HashMap<String, Vec<SyncSender<serde_json::Value>>>>,
    /// Cola ACOTADA de `NOTIFY` que fallaron por un motivo TRANSITORIO
    /// (conexión caída -- nunca el caso "payload de más de 8000 bytes",
    /// que jamás se arregla solo reintentando, GRAMMAR.md §3.150). Un
    /// cambio local ya se publicó `deliver_local` de todos modos -- esto
    /// SOLO afecta si OTRAS instancias se enteran. `runtime/server.rs`
    /// reintenta drenar esta cola en cada vuelta del loop que ya escucha
    /// cambios remotos (Postgres únicamente, mismo tick de 200ms que
    /// `REMOTE_CHANGE_POLL_INTERVAL`). Acotada (`MAX_PENDING_NOTIFY_RETRIES`)
    /// -- descarta la más VIEJA al llenarse, nunca crece sin límite si la
    /// base queda caída por mucho tiempo.
    pending_notify_retries: parking_lot::Mutex<std::collections::VecDeque<(String, serde_json::Value)>>,
    /// Conteo de payloads NOTIFY descartados por superar
    /// `MAX_NOTIFY_PAYLOAD_BYTES`, por colección -- landmine encontrado en
    /// un barrido de "límites honestos" (GRAMMAR.md §3.44/§3.150): antes de
    /// esto, la única señal de que esto estaba pasando era un `eprintln!`
    /// que nadie lee corriendo desatendido bajo `pm2`/`systemd` -- una
    /// colección con filas grandes podía quedar desincronizada entre
    /// instancias durante MESES sin que nadie se enterara. Expuesto en
    /// `/metrics` (§3.149) como `linkc_notify_oversized_dropped_total`, el
    /// mismo lugar que un operador YA está mirando para latencia/conteo de
    /// requests -- no un canal de alertas nuevo.
    oversized_notify_drops: parking_lot::Mutex<HashMap<String, u64>>,
    /// `None` = no hay ninguna `transaction { ... }` abierta ahora mismo
    /// (comportamiento de siempre: `publish()` entrega de inmediato).
    /// `Some(vec)` = hay una transacción abierta -- `publish()` encola acá
    /// en vez de entregar, hasta que `commit_transaction`/
    /// `rollback_transaction` la vacíen o la descarten (GRAMMAR.md §3.154).
    /// `Option` en vez de un `bool` aparte + un `Vec` siempre vivo: un solo
    /// campo no puede quedar desincronizado del otro por accidente.
    transaction_pending_publishes: parking_lot::Mutex<Option<Vec<(String, serde_json::Value)>>>,
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
    /// motivo que `subscribers`: `db: &Db` ya está disponible en cualquier
    /// punto del árbol de evaluación. `RwLock`, no `Mutex` -- se ESCRIBE
    /// una sola vez al arrancar (antes de aceptar la primera request) y se
    /// LEE en cada `crypto.hashPassword`, desde cualquier hilo de request
    /// (Pilar 1 del roadmap de concurrencia, 26/08/2026) -- muchos
    /// lectores concurrentes no necesitan turnarse entre sí.
    /// `verifyPassword` NO lo necesita: el formato PHC embebe sus propios
    /// `m`/`t`/`p` en el hash guardado, así que verificar un hash viejo con
    /// otros parámetros sigue funcionando sin tocar esto.
    argon2_params: parking_lot::RwLock<argon2::Params>,
    /// Timeout de `http.get`/`post`/`getWithHeaders`/etc. (GRAMMAR.md
    /// §3.86): mismo criterio EXACTO que `argon2_params` de arriba (vive
    /// acá, no como parámetro enhebrado, porque `db: &Db` ya llega a
    /// `call_method`; `RwLock` por el mismo motivo -- escrito una vez,
    /// leído desde cualquier hilo de request). `ureq` (crate) NO tiene
    /// timeout de lectura/escritura por default -- solo 30s de timeout de
    /// CONEXIÓN -- así que sin esto una request saliente a un servidor
    /// lento o colgado bloqueaba TODO el intérprete cuando corría en un
    /// solo hilo (GRAMMAR.md §3.13) -- con un hilo por request (Pilar 1),
    /// ahora solo bloquea AL HILO de esa request puntual, pero el timeout
    /// sigue siendo la defensa real contra un servidor externo que nunca
    /// responde. `server.rs` lo sobreescribe UNA vez al arrancar según
    /// `--http-timeout`/`LINK_HTTP_TIMEOUT` (o su default, ver
    /// `main.rs::resolve_http_timeout`).
    http_timeout: parking_lot::RwLock<Duration>,
    /// Nombre de colección -> nombre del campo `@softDelete`, si esa
    /// colección tiene uno (GRAMMAR.md §3.78). Se calcula UNA vez al abrir
    /// la conexión (acá SÍ hay `Program`/`ast::Field` con anotaciones a
    /// mano, a diferencia de `Db::call` -- por eso se resuelve acá y se
    /// guarda, en vez de recalcularlo en cada `select`/`delete`). Vacío
    /// (sin entrada) es el caso normal -- la mayoría de colecciones no usa
    /// soft-delete.
    soft_delete_fields: HashMap<String, String>,
}

/// Un cambio anunciado por OTRA instancia de `linkc serve` contra la misma
/// base (GRAMMAR.md §3.44), recibido vía LISTEN/NOTIFY -- `runtime/server.rs`
/// lo drena del canal que devuelve `Db::connect_postgres` y lo vuelve a
/// publicar LOCAL (`Db::publish_remote`), para que un suscriptor conectado a
/// ESTA instancia también lo vea.
pub(crate) struct RemoteChange {
    pub collection: String,
    pub event: serde_json::Value,
    /// Epoch ms de cuando la instancia ORIGEN mandó el `NOTIFY` (GRAMMAR.md
    /// §3.150) -- `runtime/server.rs` resta esto de "ahora" al drenar el
    /// canal para medir la latencia de propagación real, sin necesitar
    /// relojes sincronizados entre instancias más allá de lo que ya asume
    /// cualquier métrica de este tipo (NTP de sistema operativo normal).
    pub sent_at_ms: i64,
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

/// Cuántos `NOTIFY` fallidos por conexión caída tolera la cola de reintento
/// (GRAMMAR.md §3.150) antes de descartar el más VIEJO -- un número chico
/// a propósito: esta cola existe para cubrir una caída CORTA (segundos,
/// hasta que `with_reconnect` repare la conexión sola), no como un
/// almacenamiento durable de cambios pendientes.
const MAX_PENDING_NOTIFY_RETRIES: usize = 50;

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

thread_local! {
    /// Contexto de LA request HTTP que está invocando un rpc AHORA MISMO en
    /// ESTE hilo -- body crudo + headers, para `request.rawBody()`/
    /// `request.header()` (GRAMMAR.md §3.38). `thread_local!`, no un campo
    /// de `Db` -- Pilar 1 del roadmap de concurrencia (26/08/2026): con un
    /// hilo por request (`runtime/server.rs`), cada request corre en SU
    /// PROPIO hilo de punta a punta, así que "el contexto de la request
    /// actual" es exactamente lo que un `thread_local!` modela -- cada
    /// hilo ve su propia copia, sin ningún candado ni riesgo de que la
    /// request A lea el contexto que la request B (en otro hilo) está
    /// escribiendo a la vez. `server.rs` lo fija justo antes de invocar el
    /// rpc y lo limpia apenas termina, igual que antes.
    static CURRENT_REQUEST: RefCell<Option<RequestContext>> = const { RefCell::new(None) };
    /// `response.setStatus(code)` (GRAMMAR.md §3.46) para la request de
    /// ESTE hilo -- mismo criterio que `CURRENT_REQUEST` de arriba. La
    /// escribe el CUERPO del rpc, la lee `server.rs` una sola vez después
    /// de que `invoke_rpc` vuelve con éxito, en el MISMO hilo.
    static RESPONSE_STATUS_OVERRIDE: std::cell::Cell<Option<u16>> = const { std::cell::Cell::new(None) };
    /// `response.redirect(url, permanent)` (GRAMMAR.md §3.111) -- mismo
    /// mecanismo y mismo ciclo de vida que `RESPONSE_STATUS_OVERRIDE`.
    static RESPONSE_LOCATION_OVERRIDE: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Ver la doc de `CURRENT_REQUEST` (arriba). Dos structs (no una tupla)
/// porque `runtime/server.rs` construye esto con nombres de campo, más
/// legible que posiciones.
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

/// `linkc doctor` (GRAMMAR.md §3.100): confirma que la base configurada
/// responde, sin ejecutar NINGÚN DDL -- a diferencia de `Db::new`/
/// `connect_postgres_with_options` (que corren el chequeo/migración de
/// schema completo al conectar), `doctor` corre ANTES de un despliegue y no
/// debe crear ni alterar nada por su cuenta. Mismo `connect_postgres_client`
/// que ya usa `linkc migrate --dry-run` (migrate.rs); pub (no pub(crate))
/// porque `main.rs` -- un crate binario separado de esta librería -- lo
/// llama directo, mismo motivo que `connect_postgres_for_testing` (§3.99,
/// más abajo en este archivo).
pub fn check_postgres_connectivity(url: &str) -> Result<(), String> {
    let mut client = connect_postgres_client(url)?;
    client.execute("SELECT 1", &[]).map_err(|e| format!("conectó pero la consulta de prueba falló: {e}"))?;
    Ok(())
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
    // `unwrap_or(now)` en vez de `?`: un payload de una instancia VIEJA (de
    // antes de GRAMMAR.md §3.150, sin este campo) sigue propagándose --
    // solo pierde la métrica de latencia para ESE evento puntual, nunca el
    // evento en sí.
    let sent_at_ms = v.get("sent_at_ms").and_then(|v| v.as_i64()).unwrap_or_else(now_ms);
    Some(RemoteChange { collection, event, sent_at_ms })
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
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
        Self::new_with_options(program, db_path, false)
    }

    /// Igual que `new`, más `adopt_existing` (`--adopt-existing`/
    /// `LINK_ADOPT_EXISTING`, GRAMMAR.md §3.67): en vez de `CREATE TABLE IF
    /// NOT EXISTS` + `check_schema_matches` (que exige que la tabla física
    /// calce EXACTO, y auto-agrega columnas nullable faltantes), corre
    /// `check_schema_for_adoption` -- nunca ejecuta DDL, solo valida que las
    /// columnas declaradas existan con tipo compatible, ignorando cualquier
    /// columna física de más. Parámetro nuevo en vez de sumar un tercer
    /// método (`new_adopting`): esto NO es la clase de función con ~11
    /// call-sites indirectos que justificó la interior-mutability de
    /// `current_request`/`argon2_params` más abajo -- es un constructor,
    /// se llama una sola vez por proceso real (`server.rs::serve`), así que
    /// un parámetro más acá es barato. `new` sigue siendo la firma pública
    /// de siempre, ahora un envoltorio con `false` (convención del proyecto:
    /// método nuevo agregado, ninguna firma existente cambia).
    pub fn new_with_options(program: &Program, db_path: &Path, adopt_existing: bool) -> Self {
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

        let checks_by_collection = check_fields_by_collection(program, &checker);
        let empty_checks: Vec<(String, FieldCheck)> = Vec::new();
        let mut columns = HashMap::new();
        for (name, element_ty) in checker.db_collections() {
            let Type::Struct { fields, .. } = element_ty else {
                unreachable!("Checker::validate_db_element_type ya garantizó que el elemento sea un struct");
            };
            let cols: Vec<ColumnPlan> =
                fields.iter().filter(|f| f.name != "id").map(|f| ColumnPlan::for_field(f.clone(), &simple_enums)).collect();
            if adopt_existing {
                // GRAMMAR.md §3.80/§3.96: `--adopt-existing` nunca ejecuta
                // DDL, punto -- ni `@index`/`@unique`/`@check` es la
                // excepción. Un constraint declarado sobre una colección
                // adoptada simplemente no se crea a nivel de base;
                // documentado, no un olvido -- la validación de `@check`
                // sigue aplicando del lado de la aplicación
                // (`apply_field_validators`, `runtime/mod.rs`) sin importar
                // este modo.
                check_schema_for_adoption(&connection, name, &cols, &db_path_display).unwrap_or_else(|e| panic!("{e}"));
            } else {
                let checks = checks_by_collection.get(name).unwrap_or(&empty_checks);
                connection
                    .execute(&create_table_sql(name, &cols, checks), [])
                    .unwrap_or_else(|e| panic!("no se pudo crear la tabla '{name}' en '{db_path_display}': {e}"));
                check_schema_matches(&connection, name, &cols, &db_path_display).unwrap_or_else(|e| panic!("{e}"));
            }
            columns.insert(name.clone(), cols);
        }
        if !adopt_existing {
            for (name, indexed) in index_fields_by_collection(program, &checker) {
                for stmt in create_index_statements(&name, &indexed) {
                    connection
                        .execute(&stmt, [])
                        .unwrap_or_else(|e| panic!("no se pudo crear un índice sobre '{name}' en '{db_path_display}': {e}"));
                }
            }
            for (name, sets) in composite_unique_by_collection(program, &checker) {
                for stmt in create_composite_unique_statements(&name, &sets) {
                    connection.execute(&stmt, []).unwrap_or_else(|e| {
                        panic!("no se pudo crear un constraint UNIQUE compuesto sobre '{name}' en '{db_path_display}': {e}")
                    });
                }
            }
        }
        let soft_delete_fields = soft_delete_fields_by_collection(program, &checker);

        Db {
            backend: Backend::Sqlite(parking_lot::ReentrantMutex::new(connection)),
            checker,
            simple_enums,
            columns,
            subscribers: parking_lot::Mutex::new(HashMap::new()),
            pending_notify_retries: parking_lot::Mutex::new(std::collections::VecDeque::new()),
            oversized_notify_drops: parking_lot::Mutex::new(HashMap::new()),
            transaction_pending_publishes: parking_lot::Mutex::new(None),
            instance_id: random_instance_id(),
            argon2_params: parking_lot::RwLock::new(argon2::Params::default()),
            http_timeout: parking_lot::RwLock::new(DEFAULT_HTTP_TIMEOUT),
            soft_delete_fields,
        }
    }

    /// Lo mismo que `new_with_options`, contra un PostgreSQL real (GRAMMAR.md
    /// §3.36). Todo lo de arriba de esta capa -- `call`, `subscribe`, el plan
    /// de columnas, la codificación JSON -- es exactamente el mismo código:
    /// lo único que cambia es quién ejecuta el SQL.
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
    ///
    /// `adopt_existing` (GRAMMAR.md §3.67): salta el `CREATE TABLE IF NOT
    /// EXISTS` y el loop de `ADD COLUMN` -- ningún DDL, solo el SELECT de
    /// siempre contra `information_schema` (`validate_existing_id_column`,
    /// que ya corría sin condicionar nada) más
    /// `validate_columns_exist_for_adoption` (nuevo, mismo criterio de
    /// solo-lectura). El punto es poder adoptar una base donde el rol de la
    /// app no tiene permiso de crear/alterar tablas -- una restricción real y
    /// común en producción, no solo un gusto de organización del esquema.
    pub(crate) fn connect_postgres_with_options(
        program: &Program,
        url: &str,
        adopt_existing: bool,
    ) -> Result<(Self, Receiver<RemoteChange>), String> {
        let (checker, symbol_errors) = Checker::build_symbols(program);
        if let Some(e) = symbol_errors.into_iter().next() {
            return Err(format!("programa inválido al abrir la base de datos: {e}"));
        }
        let simple_enums = simple_enum_names(program);

        let client = connect_postgres_client(url)?;
        let backend =
            Backend::Postgres { client: parking_lot::ReentrantMutex::new(std::cell::RefCell::new(client)), url: url.to_string() };

        let checks_by_collection = check_fields_by_collection(program, &checker);
        let empty_checks: Vec<(String, FieldCheck)> = Vec::new();
        let mut columns = HashMap::new();
        for (name, element_ty) in checker.db_collections() {
            let Type::Struct { fields, .. } = element_ty else {
                unreachable!("Checker::validate_db_element_type ya garantizó que el elemento sea un struct");
            };
            let cols: Vec<ColumnPlan> =
                fields.iter().filter(|f| f.name != "id").map(|f| ColumnPlan::for_field(f.clone(), &simple_enums)).collect();
            let non_id: Vec<FieldType> = cols.iter().map(|c| c.field.clone()).collect();

            if !adopt_existing {
                // El DDL sale del MISMO generador que usa `linkc build` para
                // emitir schema.pg.sql. Si el runtime creara las tablas por su
                // cuenta, el esquema que el proyecto documenta y el que la base
                // realmente tiene podrían divergir -- que es la clase de bug que
                // este repo ya encontró varias veces (GRAMMAR.md §3.9).
                let checks = checks_by_collection.get(name).unwrap_or(&empty_checks);
                backend
                    .execute_ddl(&crate::codegen::postgres_emit::create_postgres_table_sql(name, &non_id, &simple_enums, checks))
                    .map_err(|e| format!("no se pudo crear la tabla '{name}': {e}"))?;
            }

            // `CREATE TABLE IF NOT EXISTS` es un no-op sobre una tabla que ya
            // existía -- nunca mira si SU "id" es compatible. Encontrado en
            // producción: una tabla real con `id UUID` (migrando desde otro
            // backend) dejaba pasar el connect sin queja, y el primer insert
            // recién ahí fallaba -- antes de este chequeo, con un panic que
            // tiraba abajo el servidor entero (ver store.rs::insert_returning_id).
            // Falla ACÁ, al conectar, con un mensaje que dice qué hacer --
            // mismo momento y mismo criterio que `check_schema_matches` ya
            // aplica para SQLite, adaptado a que Postgres no recrea tablas.
            // Es un SELECT, no DDL, así que corre en los dos modos.
            validate_existing_id_column(&backend, name)?;

            if adopt_existing {
                validate_columns_exist_for_adoption(&backend, name, &cols)?;
            } else {
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
                //
                // GRAMMAR.md §3.94: antes de agregar columnas, avisa por
                // stderr (nunca bloquea) si esta tabla no se parece a lo que
                // el programa declara -- ver el comentario de la función.
                warn_if_table_looks_unrelated(&backend, name, &cols);
                for field in &non_id {
                    backend
                        .execute_ddl(&crate::codegen::postgres_emit::alter_table_add_column_postgres(name, field, &simple_enums))
                        .map_err(|e| format!("no se pudo migrar la tabla '{name}': {e}"))?;
                }
            }
            columns.insert(name.clone(), cols);
        }
        if !adopt_existing {
            // Mismo criterio que el lado SQLite: `--adopt-existing` nunca
            // ejecuta DDL, ni siquiera para un índice declarado.
            for (name, indexed) in index_fields_by_collection(program, &checker) {
                for stmt in create_index_statements(&name, &indexed) {
                    backend.execute_ddl(&stmt).map_err(|e| format!("no se pudo crear un índice sobre '{name}': {e}"))?;
                }
            }
            for (name, sets) in composite_unique_by_collection(program, &checker) {
                for stmt in create_composite_unique_statements(&name, &sets) {
                    backend
                        .execute_ddl(&stmt)
                        .map_err(|e| format!("no se pudo crear un constraint UNIQUE compuesto sobre '{name}': {e}"))?;
                }
            }
        }

        let instance_id = random_instance_id();
        let remote_rx = spawn_remote_listener(url.to_string(), instance_id.clone());
        let soft_delete_fields = soft_delete_fields_by_collection(program, &checker);

        Ok((
            Db {
                backend,
                checker,
                simple_enums,
                columns,
                subscribers: parking_lot::Mutex::new(HashMap::new()),
                pending_notify_retries: parking_lot::Mutex::new(std::collections::VecDeque::new()),
                oversized_notify_drops: parking_lot::Mutex::new(HashMap::new()),
                transaction_pending_publishes: parking_lot::Mutex::new(None),
                instance_id,
                argon2_params: parking_lot::RwLock::new(argon2::Params::default()),
                http_timeout: parking_lot::RwLock::new(DEFAULT_HTTP_TIMEOUT),
                soft_delete_fields,
            },
            remote_rx,
        ))
    }

    /// GRAMMAR.md §3.99: envoltorio público de `connect_postgres_with_options`
    /// para `linkc test --db <url-postgres>` (`main.rs`, un crate binario
    /// APARTE que solo ve la API `pub` de esta librería -- `RemoteChange`,
    /// el tipo del receiver de LISTEN/NOTIFY, es `pub(crate)` a propósito,
    /// así que la firma completa de `connect_postgres_with_options` no es
    /// nombrable desde afuera). Descarta el receiver -- un `linkc test` es
    /// una corrida de una sola vez, sin ningún otro proceso escuchando
    /// cambios en vivo, así que no hace falta esa plomería acá.
    pub fn connect_postgres_for_testing(program: &Program, url: &str, adopt_existing: bool) -> Result<Self, String> {
        Self::connect_postgres_with_options(program, url, adopt_existing).map(|(db, _remote_rx)| db)
    }

    /// Fija el costo de `crypto.hashPassword` para lo que quede de vida del
    /// proceso (GRAMMAR.md §3.55) -- `server.rs` lo llama UNA sola vez, antes
    /// de aceptar la primera request, con lo que haya resuelto de
    /// `--argon2-memory-kib`/`--argon2-iterations` (o sus env vars). Nunca se
    /// vuelve a llamar durante la vida del servidor.
    pub(crate) fn set_argon2_params(&self, params: argon2::Params) {
        *self.argon2_params.write() = params;
    }

    /// Los parámetros de costo configurados -- los lee `crypto.hashPassword`
    /// en `runtime/mod.rs` en cada llamada. `argon2::Params` no es `Copy`
    /// (guarda un `Option<Vec<u8>>` para el "secret" opcional que este
    /// proyecto no usa), así que clona en vez de prestar: el costo es
    /// insignificante comparado con el propio hasheo (~15ms, §3.34).
    pub(crate) fn argon2_params(&self) -> argon2::Params {
        self.argon2_params.read().clone()
    }

    /// Timeout de `http.*` para lo que quede de vida del proceso (GRAMMAR.md
    /// §3.86) -- mismo criterio que `set_argon2_params`: `server.rs` lo
    /// llama UNA sola vez, antes de aceptar la primera request.
    pub(crate) fn set_http_timeout(&self, timeout: Duration) {
        *self.http_timeout.write() = timeout;
    }

    /// El timeout configurado -- lo lee cada llamada a `http.get`/`post`/
    /// `getWithHeaders`/etc. en `runtime/mod.rs`. `Duration` es `Copy`, así
    /// que esto no necesita clonar nada.
    pub(crate) fn http_timeout(&self) -> Duration {
        *self.http_timeout.read()
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

    /// `/health` (GRAMMAR.md §3.87): un `SELECT 1` real contra la base --
    /// `Ok(())` si respondió, `Err(mensaje)` si no. `execute_ddl` (no
    /// `query`) porque no hace falta decodificar ninguna fila de vuelta,
    /// solo confirmar que la conexión responde -- y del lado Postgres ya
    /// pasa por `with_reconnect` (GRAMMAR.md §3.40), así que una caída
    /// transitoria se autorepara ACÁ MISMO antes de reportar error, igual
    /// que cualquier otra query real. Sin caché: cada request a `/health`
    /// hace su propio chequeo -- barato (un `SELECT 1`), y un health check
    /// que devuelve un resultado viejo no sirve para nada.
    pub fn health_check(&self) -> Result<(), String> {
        self.backend.execute_ddl("SELECT 1")
    }

    /// `GET /metrics` (GRAMMAR.md §3.149): tamaño de la base en bytes, o
    /// `None` si la query falla (nunca hace fallar `/metrics` entero por
    /// esto -- ver `runtime/server.rs`). SQLite no tiene una función SQL
    /// directa para "tamaño del archivo", pero `page_count * page_size` (dos
    /// `PRAGMA`, consultables como cualquier `SELECT`) es exacto -- es
    /// literalmente cómo SQLite calcula el tamaño del archivo por dentro.
    /// Postgres sí tiene una función dedicada, `pg_database_size`.
    pub fn size_bytes(&self) -> Option<i64> {
        match &self.backend {
            Backend::Sqlite(_) => {
                let page_count = self.backend.query("PRAGMA page_count", &[], &[ColumnKind::Int]).ok()?;
                let page_size = self.backend.query("PRAGMA page_size", &[], &[ColumnKind::Int]).ok()?;
                let Some(Cell::Int(pc)) = page_count.first().and_then(|r| r.first()) else { return None };
                let Some(Cell::Int(ps)) = page_size.first().and_then(|r| r.first()) else { return None };
                Some(pc * ps)
            }
            Backend::Postgres { .. } => {
                let rows = self.backend.query("SELECT pg_database_size(current_database())", &[], &[ColumnKind::Int]).ok()?;
                match rows.first().and_then(|r| r.first()) {
                    Some(Cell::Int(n)) => Some(*n),
                    _ => None,
                }
            }
        }
    }

    /// `db.vacuum() -> Void` (GRAMMAR.md §3.151) -- `VACUUM` real contra el
    /// motor, mismo comando en los dos backends. Sin ninguna gramática
    /// nueva del lado de c-script: un builtin sin argumentos sobre `db`
    /// (`Value::Db`), pensado para exponerse detrás de `@requires(Role.Admin)`
    /// en el propio `.link` de quien lo necesite -- la gramática de
    /// autorización YA existe, este ítem no inventa una nueva.
    pub fn run_vacuum(&self) -> Result<(), String> {
        self.backend.execute_ddl("VACUUM")
    }

    /// `db.tableStats() -> Map<String, Int>` (GRAMMAR.md §3.151) -- cuántas
    /// filas tiene CADA colección declarada, contando FILAS FÍSICAS (sin
    /// filtrar `@softDelete`) -- a propósito distinto de `count()`, que sí
    /// filtra: el caso de uso es diagnóstico de tamaño real de la tabla,
    /// donde una fila soft-deleteada sigue ocupando espacio real.
    pub fn table_stats(&self) -> Result<Vec<(String, i64)>, String> {
        let mut out = Vec::with_capacity(self.columns.len());
        for collection in self.columns.keys() {
            let rows = self.backend.query(&format!("SELECT COUNT(*) FROM \"{collection}\""), &[], &[ColumnKind::Int])?;
            let Some(Cell::Int(n)) = rows.first().and_then(|r| r.first()) else {
                return Err(format!("tableStats: no se pudo leer el conteo de '{collection}'"));
            };
            out.push((collection.clone(), *n));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }

    /// `GET /metrics` (GRAMMAR.md §3.149): cuántos clientes están
    /// suscriptos a cada colección AHORA MISMO, para el gauge
    /// `linkc_stream_subscribers`. Mismo límite ya documentado de
    /// `deliver_local` -- un suscriptor desconectado se poda RECIÉN en la
    /// próxima publicación a esa colección, así que este conteo puede
    /// sobre-reportar temporalmente hasta esa próxima escritura; no hay
    /// forma de saber que un cliente se fue sin intentar escribirle.
    pub fn subscriber_counts(&self) -> Vec<(String, usize)> {
        self.subscribers.lock().iter().map(|(collection, txs)| (collection.clone(), txs.len())).collect()
    }

    /// GRAMMAR.md §3.44/§3.150: cuántos payloads NOTIFY se descartaron por
    /// superar `MAX_NOTIFY_PAYLOAD_BYTES`, por colección, desde que arrancó
    /// el proceso -- expuesto en `/metrics` para que este landmine deje de
    /// depender de que alguien lea stderr.
    pub fn oversized_notify_drop_counts(&self) -> Vec<(String, u64)> {
        self.oversized_notify_drops.lock().iter().map(|(collection, count)| (collection.clone(), *count)).collect()
    }

    /// Ver la doc de `CURRENT_REQUEST` (`thread_local!`, arriba). Llamado
    /// por `server.rs` una vez por request, justo antes de invocar el rpc
    /// -- en el hilo QUE VA A MANEJAR esa request, así que queda en el
    /// `thread_local!` correcto sin que `set_request_context` necesite
    /// saber en qué hilo está (siempre es "el actual").
    pub(crate) fn set_request_context(&self, ctx: RequestContext) {
        CURRENT_REQUEST.with(|c| *c.borrow_mut() = Some(ctx));
    }

    /// Simétrico de `set_request_context` -- `server.rs` lo llama apenas
    /// termina de manejar la request, para que `request.rawBody()`/
    /// `request.header()` nunca puedan filtrar datos de una request anterior
    /// hacia otra EN EL MISMO HILO (el pool de hilos de request, GRAMMAR.md
    /// §3.13/Pilar 1, reusa hilos entre requests -- sin este clear, el
    /// contexto de una request vieja podría sobrevivir en el `thread_local!`
    /// de un hilo reciclado hasta que algo lo pisara).
    pub(crate) fn clear_request_context(&self) {
        CURRENT_REQUEST.with(|c| *c.borrow_mut() = None);
        // Defensa en profundidad simétrica a la de arriba -- si un rpc
        // llamó `response.setStatus` y DESPUÉS erró/panicó (así que
        // `handle_rpc` nunca llegó a consumirlo con `take_response_status`),
        // esto evita que el valor sobreviva para la PRÓXIMA request en ESE
        // hilo (mismo motivo del reciclado de hilos de arriba).
        RESPONSE_STATUS_OVERRIDE.with(|c| c.set(None));
        RESPONSE_LOCATION_OVERRIDE.with(|c| {
            c.borrow_mut().take();
        });
    }

    /// Llamado por `response.setStatus(code)` (GRAMMAR.md §3.46) -- guarda
    /// el override para que `handle_rpc` lo use en vez de 200 al armar la
    /// respuesta, SOLO en el camino de éxito (`handle_rpc` nunca llega a
    /// leerlo si el rpc termina en `Err`: un error siempre va con el status
    /// que le corresponde a la falla, nunca con uno que el cuerpo haya
    /// pedido antes de fallar).
    pub(crate) fn set_response_status(&self, code: u16) {
        RESPONSE_STATUS_OVERRIDE.with(|c| c.set(Some(code)));
    }

    /// Consume el override (lo deja en `None` para la próxima invocación) --
    /// `take`, no `get`, para que un valor de UNA request nunca sobreviva
    /// por accidente a la que sigue en el mismo hilo reciclado.
    pub(crate) fn take_response_status(&self) -> Option<u16> {
        RESPONSE_STATUS_OVERRIDE.with(|c| c.take())
    }

    /// Llamado por `response.redirect(url, permanent)` (GRAMMAR.md
    /// §3.111) -- guarda la URL destino para que `server.rs` la agregue
    /// como header `Location`, mismo ciclo de vida que
    /// `set_response_status`/`take_response_status` (`redirect` además
    /// llama a `set_response_status` con 301/302, así que las dos piezas
    /// siempre viajan juntas).
    pub(crate) fn set_response_location(&self, url: String) {
        RESPONSE_LOCATION_OVERRIDE.with(|c| *c.borrow_mut() = Some(url));
    }

    /// Simétrico de `take_response_status`.
    pub(crate) fn take_response_location(&self) -> Option<String> {
        RESPONSE_LOCATION_OVERRIDE.with(|c| c.borrow_mut().take())
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
        CURRENT_REQUEST.with(|c| c.borrow().as_ref().map(|c| c.raw_body.clone()).unwrap_or_default())
    }

    pub(crate) fn current_request_header(&self, name: &str) -> Option<String> {
        CURRENT_REQUEST.with(|c| {
            c.borrow()
                .as_ref()
                .and_then(|c| c.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone()))
        })
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
            "maxRow" | "minRow" => self.top_row(collection, columns, method, &args),
            "increment" => self.increment(collection, columns, args),
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
                let new_id = self.backend.insert_returning_id(&sql, &params).map_err(|e| write_error("insert", e))?;
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
                    self.backend.execute(&sql, &params).map_err(|e| write_error("applyPatch", e))?;
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
            // GRAMMAR.md §3.78: si `collection` tiene un campo `@softDelete`,
            // `delete` deja de ser un `DELETE` real -- pasa a ser un
            // `UPDATE` que fija ese campo a `now()`. `AND "<campo>" IS
            // NULL` en el WHERE hace la operación IDEMPOTENTE: una segunda
            // llamada sobre una fila ya borrada no re-toca el timestamp,
            // devuelve `false` (0 filas afectadas), igual que un `delete`
            // normal sobre un id que ya no existe.
            "delete" => {
                let id = as_int(args.first().ok_or_else(|| RuntimeError::new("delete requiere 1 argumento"))?)?;
                // `select_rows(id: Some(_))` NUNCA filtra por soft-delete
                // (ver su propio comentario) -- acá es exactamente lo que
                // hace falta: encontrar la fila sea cual sea su estado, para
                // saber si hay algo que borrar y qué publicar si se borra.
                let existing = self.select_rows(collection, columns, Some(id))?.into_iter().next();
                let rows_affected = match self.soft_delete_fields.get(collection) {
                    Some(field) => {
                        let now_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(0);
                        let sql = format!(
                            "UPDATE \"{collection}\" SET \"{field}\" = {} WHERE \"id\" = {} AND \"{field}\" IS NULL",
                            self.backend.placeholder(1),
                            self.backend.placeholder(2)
                        );
                        self.backend
                            .execute(&sql, &[Cell::Int(now_ms), Cell::Int(id)])
                            .map_err(|e| RuntimeError::new(format!("delete (soft) falló: {e}")))?
                    }
                    None => {
                        let sql = format!("DELETE FROM \"{collection}\" WHERE \"id\" = {}", self.backend.placeholder(1));
                        self.backend
                            .execute(&sql, &[Cell::Int(id)])
                            .map_err(|e| RuntimeError::new(format!("delete falló: {e}")))?
                    }
                };
                if rows_affected > 0 {
                    if let Some(deleted_row) = existing {
                        self.publish(collection, &deleted_row);
                    }
                }
                Ok(Value::Bool(rows_affected > 0))
            }
            "count" => {
                let where_clause = self.soft_delete_where(collection).map(|c| format!(" WHERE {c}")).unwrap_or_default();
                let sql = format!("SELECT COUNT(*) FROM \"{collection}\"{where_clause}");
                let rows = self
                    .backend
                    .query(&sql, &[], &[ColumnKind::Int])
                    .map_err(|e| RuntimeError::new(format!("error en count de '{collection}': {e}")))?;
                match rows.first().and_then(|r| r.first()) {
                    Some(Cell::Int(count)) => Ok(Value::Int(*count)),
                    other => Err(RuntimeError::new(format!("count de '{collection}' devolvió algo que no es un entero: {other:?}"))),
                }
            }

            // "deleteWhere"/"findWhere"/"countWhere" NUNCA se implementan
            // acá: evaluar un predicado por fila requiere invocar un closure
            // de usuario (`call_callable`), que necesita `fns`/`checker`/
            // `sessions`/`step_budget` -- ninguno de los cuales `Db::call`
            // recibe (ver su firma arriba). La implementación real vive en
            // `runtime::call_method`, que intercepta estos tres métodos
            // ANTES de llegar acá y sí tiene ese contexto (mismo patrón que
            // `List::filter`/`List::map`) -- incluido el atajo de SQL de
            // GRAMMAR.md §3.95/§3.108 (`count_where_conjunction`/`find_where_conjunction`,
            // arriba), que tampoco vive acá porque reconocer el predicado
            // (`recognize_pushable_conjunction`) necesita el `Env` capturado
            // del closure, otra cosa que `Db::call` no tiene forma de
            // recibir. `call_method` es el único camino que el intérprete
            // usa para despachar un método de `Value::DbCollection`, así que
            // en el uso normal este brazo nunca corre. Como `Db::call` es
            // `pub fn` y queda alcanzable directo (tests, LSP, código
            // futuro), antes devolvía un resultado SILENCIOSAMENTE
            // INCORRECTO ignorando el predicado (deleteWhere borraba TODAS
            // las filas; findWhere las devolvía TODAS) -- fallar con un
            // mensaje claro es siempre mejor que una respuesta que parece
            // válida y no lo es.
            "deleteWhere" | "findWhere" | "countWhere" => Err(RuntimeError::new(format!(
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
    /// **REGISTRARSE PRIMERO, soltar el candado, y RECIÉN DESPUÉS sacar la
    /// foto** -- el orden es lo único que hace correcta a esta función, y
    /// llegar acá costó dos bugs reales seguidos (GRAMMAR.md §3.16/§3.162):
    ///
    /// 1. El diseño ORIGINAL sacaba la foto primero y se registraba después,
    ///    sin ningún candado compartido con `publish`/`deliver_local` --
    ///    correcto mientras el servidor procesaba una request a la vez, roto
    ///    apenas hubo hilos reales (§3.158): un `insert` de OTRO hilo podía
    ///    publicar ADENTRO de esa ventana y no quedar ni en la foto (ya
    ///    tomada) ni en el canal (todavía sin registrar) -- **fila perdida
    ///    en silencio**.
    /// 2. El primer fix (26/08/2026) sostuvo el candado de `subscribers`
    ///    DURANTE `select_rows` para cerrar esa ventana. Cerró el bug 1 pero
    ///    creó uno peor: `select_rows` pide el candado de la CONEXIÓN, así
    ///    que este camino pasó a ser subscribers→conexión, mientras que
    ///    `upsert` (que desde la misma ronda sostiene la conexión durante
    ///    todo su cuerpo) llega a `publish`→`deliver_local` en el orden
    ///    inverso, conexión→subscribers. **Deadlock ABBA reproducido**: el
    ///    servidor queda vivo pero cualquier request que toque la base
    ///    cuelga para siempre.
    ///
    /// El orden de acá resuelve los dos a la vez **sin sostener nunca los
    /// dos candados**: si una escritura ocurre entre el registro y la foto,
    /// el suscriptor la recibe como EVENTO (ya está registrado) y además
    /// puede verla en la foto -- un duplicado ocasional, que es inofensivo
    /// (el consumidor de un `stream` ya trata cada evento como el estado
    /// ACTUAL de esa fila, nunca como un delta). Lo que NO puede pasar es
    /// que no aparezca en ninguna de las dos, que era el bug 1.
    ///
    /// **Invariante de candados, a respetar por cualquier código futuro:**
    /// nadie sostiene `subscribers` y la conexión a la vez EN ESTE ORDEN.
    /// `publish`/`deliver_local` sí toman `subscribers` con la conexión ya
    /// tomada (vía `upsert`/`transaction{}`), y eso está bien justamente
    /// porque este camino ya no hace lo contrario.
    pub fn subscribe(&self, collection: &str) -> Result<(Vec<serde_json::Value>, Receiver<serde_json::Value>), RuntimeError> {
        let columns = self.columns.get(collection).ok_or_else(|| RuntimeError::new(format!("colección desconocida: '{collection}'")))?;
        let (tx, rx) = mpsc::sync_channel(LIVE_STREAM_BUFFER);
        // El candado se toma y se SUELTA acá mismo (statement temporal) --
        // nunca sigue tomado durante el `select_rows` de abajo.
        self.subscribers.lock().entry(collection.to_string()).or_default().push(tx);
        // Si esto falla, el sender ya registrado queda huérfano en el mapa
        // -- inofensivo: `deliver_local` poda cualquier sender desconectado
        // de forma perezosa, en la próxima publicación a esta colección.
        let snapshot: Vec<serde_json::Value> =
            self.select_rows(collection, columns, None)?.iter().map(|v| value_to_json(v, &self.simple_enums)).collect();
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
    /// GRAMMAR.md §3.154: si esta escritura ocurre DENTRO de un
    /// `transaction { ... }` todavía sin cerrar, la publicación se DIFIERE
    /// (encolada en `transaction_pending_publishes`) en vez de entregarse
    /// ahora mismo -- publicar una fila que la transacción después
    /// rollbackea le mentiría a cualquier `stream` suscripto (le llegaría un
    /// cambio que, a nivel de la base, nunca pasó de verdad). El commit
    /// exitoso vacía la cola llamando exactamente este mismo camino
    /// (`deliver_local`/`notify_remote`) para cada evento pendiente, en
    /// orden; el rollback la descarta entera, sin publicar nada.
    fn publish(&self, collection: &str, row: &Value) {
        let json = value_to_json(row, &self.simple_enums);
        if let Some(pending) = self.transaction_pending_publishes.lock().as_mut() {
            pending.push((collection.to_string(), json));
            return;
        }
        self.deliver_local(collection, &json);
        if self.backend.is_postgres() {
            self.notify_remote(collection, &json);
        }
    }

    /// GRAMMAR.md §3.154, Pilar 1 del roadmap de concurrencia (26/08/2026):
    /// sostiene el candado REENTRANTE de la conexión física por toda la
    /// duración de `f` -- `Expr::Transaction` (`runtime/mod.rs`) envuelve
    /// acá `begin_transaction` + el `eval_block` del cuerpo + `commit_
    /// transaction`/`rollback_transaction`, como UNA sola sección
    /// exclusiva, para que ningún otro hilo (otra request) pueda intercalar
    /// una escritura suya en la MISMA conexión a mitad de esta transacción.
    /// Las llamadas de adentro (`db.<c>.insert(...)`, etc.) piden este
    /// mismo candado por su cuenta -- reentrante, así que no hay deadlock.
    pub(crate) fn with_exclusive_connection<T>(&self, f: impl FnOnce() -> T) -> T {
        self.backend.with_exclusive(f)
    }

    /// GRAMMAR.md §3.154: arranca la transacción SQL real detrás de un
    /// `transaction { ... }` -- llamado UNA vez, antes de evaluar el cuerpo
    /// del bloque.
    ///
    /// **El checker SOLO rechaza el anidamiento SINTÁCTICO** (un
    /// `transaction` escrito literalmente dentro de otro, en el mismo
    /// cuerpo de función) -- `in_transaction` (checker.rs) es un
    /// `Cell<bool>` con alcance de UN `check_block`, sin visibilidad sobre
    /// lo que hace una `fn` auxiliar llamada desde adentro. Antes de esta
    /// ronda, un `transaction` alcanzado por una llamada a otra función que
    /// a su vez abre su propio `transaction` (nesting real, pero a través
    /// de un límite de función, no de sintaxis) compilaba limpio y recién
    /// fallaba en RUNTIME con el error crudo del backend ("cannot start a
    /// transaction within a transaction" de SQLite/Postgres), sin ninguna
    /// pista de qué reglas de c-script se estaban violando -- encontrado
    /// por una auditoría multi-agente adversarial (26/08/2026). Chequear
    /// ACÁ, antes de intentar el `BEGIN` real, da el mismo mensaje claro
    /// que el checker ya usa para el caso sintáctico, para el caso que el
    /// checker estructuralmente no puede atrapar.
    pub(crate) fn begin_transaction(&self) -> Result<(), String> {
        if self.transaction_pending_publishes.lock().is_some() {
            return Err(
                "ya hay una transacción abierta en esta misma ejecución -- 'transaction { }' no admite anidamiento, ni siquiera a través de una función auxiliar que abre su propia transacción (GRAMMAR.md §3.154)".to_string(),
            );
        }
        self.backend.execute_ddl("BEGIN")?;
        *self.transaction_pending_publishes.lock() = Some(Vec::new());
        Ok(())
    }

    /// `COMMIT` real -- si sale bien, DEVUELVE cada publicación que quedó
    /// en cola durante la transacción (en el mismo orden en que se
    /// generaron) para que el caller las entregue. Si el `COMMIT` en sí
    /// falla (raro, pero posible -- ej. una constraint diferida), la cola
    /// se descarta SIN devolver nada -- un `COMMIT` fallido es, a todo
    /// efecto, un rollback.
    ///
    /// Deliberadamente NO entrega acá adentro (bug real, encontrado
    /// auditando esta misma sección tras shippear GRAMMAR.md §3.158,
    /// 26/08/2026): el ÚNICO caller (`Expr::Transaction`, runtime/mod.rs)
    /// corre este método entero DENTRO de `Db::with_exclusive_connection`,
    /// que sostiene el candado reentrante de la CONEXIÓN -- si `deliver_local`
    /// (que pide el candado de `subscribers`) corriera ahí adentro, quedaría
    /// order conexión→subscribers, exactamente al revés del order que
    /// `subscribe()` necesita (subscribers→conexión, ver su propio
    /// comentario) para no perder un evento contra un `insert` concurrente
    /// -- dos hilos pidiendo esos mismos dos candados en orden opuesto es
    /// un deadlock clásico. El caller entrega DESPUÉS de que
    /// `with_exclusive_connection` ya soltó el candado de la conexión,
    /// evitando el problema de raíz en vez de solo evitar el deadlock
    /// puntual.
    pub(crate) fn commit_transaction(&self) -> Result<Vec<(String, serde_json::Value)>, String> {
        self.backend.execute_ddl("COMMIT")?;
        Ok(self.transaction_pending_publishes.lock().take().unwrap_or_default())
    }

    /// `ROLLBACK` best-effort -- si el `ROLLBACK` en sí falla (ej. la
    /// conexión ya se cayó, en cuyo caso Postgres/SQLite ya abortaron la
    /// transacción solos por su cuenta), no hay nada más que hacer del lado
    /// de acá: se loguea y se sigue. La cola de publicaciones pendientes se
    /// descarta SIEMPRE, haya salido bien el `ROLLBACK` o no -- ninguna de
    /// esas filas quedó firme en la base, así que ningún `stream` debe
    /// enterarse de ellas.
    pub(crate) fn rollback_transaction(&self) {
        if let Err(e) = self.backend.execute_ddl("ROLLBACK") {
            eprintln!("aviso: 'ROLLBACK' de una transacción falló ({e}) -- probablemente la conexión ya se había cerrado, en cuyo caso la base ya descartó la transacción por su cuenta");
        }
        *self.transaction_pending_publishes.lock() = None;
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

    /// `pub(crate)`, no privado -- desde GRAMMAR.md §3.158 (v1.114.0),
    /// `Expr::Transaction` (runtime/mod.rs) también la llama, DESPUÉS de
    /// soltar el candado de la conexión (ver el comentario de
    /// `commit_transaction` arriba para el porqué exacto).
    pub(crate) fn deliver_local(&self, collection: &str, json: &serde_json::Value) {
        let mut subs = self.subscribers.lock();
        if let Some(list) = subs.get_mut(collection) {
            // `try_send` -- NUNCA bloqueante: publicar no puede colgar el
            // hilo que atiende ESTA request, ni siquiera si un suscriptor
            // está lento. `Full` (suscriptor demasiado atrasado, ver
            // LIVE_STREAM_BUFFER) o `Disconnected` (el cliente ya se fue)
            // se podan igual -- lazy, recién en la próxima publicación a
            // esta colección, no eager (un mecanismo eager necesitaría un
            // hilo aparte tocando `Db`, reabriendo la pregunta de Send/Sync
            // que todo este diseño evita).
            list.retain(|tx| tx.try_send(json.clone()).is_ok());
        }
    }

    /// `pub(crate)` por el mismo motivo que `deliver_local`, arriba.
    pub(crate) fn notify_remote(&self, collection: &str, json: &serde_json::Value) {
        if self.try_notify_remote(collection, json) {
            return;
        }
        // Falla TRANSITORIA (conexión caída) -- se encola para reintentar
        // en el próximo tick del loop de `server.rs` (GRAMMAR.md §3.150),
        // acotado para no crecer sin límite si la base queda caída mucho
        // tiempo. El payload de más de 8000 bytes NUNCA llega hasta acá --
        // `try_notify_remote` lo descarta con su propio aviso, sin encolar
        // nada (reintentarlo no lo arreglaría).
        let mut queue = self.pending_notify_retries.lock();
        if queue.len() >= MAX_PENDING_NOTIFY_RETRIES {
            queue.pop_front();
        }
        queue.push_back((collection.to_string(), json.clone()));
    }

    /// El envío real de un `NOTIFY` -- `true` si salió bien (o si el
    /// payload supera el límite y se descartó a propósito, `false` SOLO
    /// ante una falla transitoria que vale la pena reintentar). Separado de
    /// `notify_remote` para que tanto el envío original como
    /// `flush_pending_notify_retries` (más abajo) compartan la MISMA
    /// lógica de encolar en vez de tener dos copias que puedan divergir.
    fn try_notify_remote(&self, collection: &str, json: &serde_json::Value) -> bool {
        let payload = serde_json::json!({
            "instance": self.instance_id,
            "collection": collection,
            "event": json,
            "sent_at_ms": now_ms(),
        })
        .to_string();
        if payload.len() > MAX_NOTIFY_PAYLOAD_BYTES {
            eprintln!(
                "aviso: un cambio en '{collection}' de {} bytes supera el límite de NOTIFY de PostgreSQL \
                 ({MAX_NOTIFY_PAYLOAD_BYTES}) -- no se propaga a otras instancias (GRAMMAR.md §3.44)",
                payload.len()
            );
            *self.oversized_notify_drops.lock().entry(collection.to_string()).or_insert(0) += 1;
            return true;
        }
        match self.backend.notify(REMOTE_CHANGE_CHANNEL, &payload) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("aviso: no se pudo notificar el cambio en '{collection}' a otras instancias: {e}");
                false
            }
        }
    }

    /// Reintenta cada `NOTIFY` pendiente en la cola acotada (GRAMMAR.md
    /// §3.150) -- llamado por `runtime/server.rs` en cada vuelta del loop
    /// que ya escucha cambios remotos, así que no hace falta ningún hilo ni
    /// timer nuevo. Los que salen bien se sacan de la cola; los que vuelven
    /// a fallar quedan para el próximo tick, en el mismo orden (FIFO).
    pub(crate) fn flush_pending_notify_retries(&self) {
        let pending: Vec<(String, serde_json::Value)> = self.pending_notify_retries.lock().drain(..).collect();
        for (collection, json) in pending {
            if !self.try_notify_remote(&collection, &json) {
                self.pending_notify_retries.lock().push_back((collection, json));
            }
        }
    }

    /// `"<campo>" IS NULL`, si `collection` tiene un campo `@softDelete`
    /// (GRAMMAR.md §3.78) -- `None` para la enorme mayoría de colecciones,
    /// que no usan soft-delete.
    fn soft_delete_where(&self, collection: &str) -> Option<String> {
        self.soft_delete_fields.get(collection).map(|field| format!("\"{field}\" IS NULL"))
    }

    /// `all` (`id: None`, ordenado por "id" para output determinístico,
    /// mismo orden de inserción que ya daba el `Vec` de antes) o `find`/la
    /// re-consulta de `insert`/`applyPatch` (`id: Some(_)`, a lo sumo 1 fila).
    ///
    /// El filtro de `@softDelete` (GRAMMAR.md §3.78) SOLO se aplica cuando
    /// `id` es `None` (o sea, en `all()`) -- deliberado, no una omisión: la
    /// re-consulta que `insert`/`applyPatch` hacen contra el `id` que ELLOS
    /// mismos acaban de escribir usa este mismo camino con `id: Some(_)`, y
    /// si `applyPatch` estuviera fijando justo el campo de soft-delete (un
    /// patch puede tocarlo como cualquier otro campo), filtrar ahí haría que
    /// la re-consulta no encontrara la fila que acaba de escribir -- un
    /// panic, no un error limpio. `find(id)` comparte el mismo criterio por
    /// simplicidad (ver "Límites honestos", GRAMMAR.md §3.78): una fila
    /// soft-deleteada sigue siendo encontrable por id directo, solo
    /// desaparece de listados (`all`/`page`/`pageAfter`/agregaciones).
    fn select_rows(&self, collection: &str, columns: &[ColumnPlan], id: Option<i64>) -> Result<Vec<Value>, RuntimeError> {
        let mut col_list = vec!["\"id\"".to_string()];
        col_list.extend(columns.iter().map(|c| format!("\"{}\"", c.field.name)));
        let sql = match id {
            Some(_) => format!("SELECT {} FROM \"{collection}\" WHERE \"id\" = {}", col_list.join(", "), self.backend.placeholder(1)),
            None => match self.soft_delete_where(collection) {
                Some(cond) => format!("SELECT {} FROM \"{collection}\" WHERE {cond} ORDER BY \"id\"", col_list.join(", ")),
                None => format!("SELECT {} FROM \"{collection}\" ORDER BY \"id\"", col_list.join(", ")),
            },
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
        rows.iter().map(|cells| self.row_to_fields(collection, cells, columns).map(Value::Struct)).collect()
    }

    /// Cells bindeables + condición SQL `"{f1}" OP1 ? AND "{f2}" OP2 ? AND
    /// ...` (con soft-delete AND-eado al final si corresponde) para el
    /// shape de predicado que `countWhere`/`findWhere` empujan a SQL
    /// (GRAMMAR.md §3.95, `==` v1.59.0; §3.108, los otros cinco operadores
    /// relacionales; §3.109, una conjunción `&&` de varias condiciones así
    /// en vez de una sola): `|x| x.campo OP valor && ...`. Compartido entre
    /// `count_where_conjunction` y `find_where_conjunction` -- la única
    /// diferencia entre esos dos es qué `SELECT` arman con esta misma
    /// condición. Una lista de una sola condición es el caso de siempre (un
    /// solo operador).
    ///
    /// `None` (nunca un error) si CUALQUIER condición de la lista no es
    /// pusheable: su campo no existe declarado en esta colección (aparte de
    /// `"id"`), o es una columna serializada como JSON (`x?: T?`/struct/enum
    /// ADT/lista/... -- sin comparación simple de SQL contra un `Value` sin
    /// ambigüedad). El caller cae al camino interpretado de siempre ante
    /// `None` -- correcto en cualquier caso, más lento solo en ese caso
    /// puntual.
    fn conjunction_condition(&self, collection: &str, columns: &[ColumnPlan], conditions: &[(String, BinaryOp, Value)]) -> Option<(String, Vec<Cell>)> {
        if conditions.is_empty() {
            return None;
        }
        let mut clauses = Vec::with_capacity(conditions.len());
        let mut cells = Vec::with_capacity(conditions.len());
        for (field, op, value) in conditions.iter() {
            // Bug real, encontrado en una auditoría propia: `"campo" = ?`
            // ligado a un parámetro NULL nunca es cierto en SQL (NULL no es
            // igual a nada, ni siquiera a sí mismo) -- pero el camino
            // interpretado de siempre SÍ trata `Value::Null == Value::Null`
            // como `true` (`==`/`!=` de Rust sobre el enum `Value`). Sin este
            // caso especial, empujar a SQL una hoja `campo == variable` donde
            // `variable` resultó ser `null` en runtime (ej. un parámetro
            // opcional del propio rpc) hacía que la fila con ese campo en
            // NULL nunca matcheara -- silenciosamente distinto del camino
            // interpretado, y en `upsert` en particular, una fila "duplicada"
            // insertada en vez de actualizada. `IS [NOT] NULL` es la forma
            // SQL correcta, sin ningún parámetro ligado para esa hoja.
            if matches!(value, Value::Null) {
                let null_op = match op {
                    BinaryOp::Eq => "IS NULL",
                    BinaryOp::NotEq => "IS NOT NULL",
                    // Los cuatro operadores relacionales no tienen una forma
                    // NULL-segura razonable de todos modos (Rust ya niega
                    // cualquier orden con Value::Null en el camino
                    // interpretado) -- se cae al camino interpretado.
                    _ => return None,
                };
                if field != "id" && columns.iter().find(|c| &c.field.name == field).is_none_or(|c| c.json) {
                    return None;
                }
                clauses.push(format!("\"{field}\" {null_op}"));
                continue;
            }
            let sql_op = match op {
                BinaryOp::Eq => "=",
                BinaryOp::NotEq => "!=",
                BinaryOp::Lt => "<",
                BinaryOp::LtEq => "<=",
                BinaryOp::Gt => ">",
                BinaryOp::GtEq => ">=",
                // `ast::recognize_conjunction_predicate` ya filtra a estos
                // seis -- cualquier otro operador nunca llega hasta acá.
                _ => return None,
            };
            let cell = if field == "id" {
                let Value::Int(id) = value else { return None };
                Cell::Int(*id)
            } else {
                let col = columns.iter().find(|c| &c.field.name == field)?;
                if col.json {
                    return None;
                }
                self.write_param(col, Some(value))
            };
            clauses.push(format!("\"{field}\" {sql_op} {}", self.backend.placeholder(cells.len() + 1)));
            cells.push(cell);
        }
        let cond = clauses.join(" AND ");
        let where_clause = match self.soft_delete_where(collection) {
            Some(sd) => format!("{cond} AND {sd}"),
            None => cond,
        };
        Some((where_clause, cells))
    }

    /// `db.<c>.countWhere(|x| x.campo OP valor && ...)` (GRAMMAR.md
    /// §3.95/§3.108/§3.109): un `SELECT COUNT(*) ... WHERE` real -- CERO
    /// filas viajan del motor al proceso, a diferencia del `countWhere`
    /// interpretado (traer TODO con `all`, evaluar el predicado fila por
    /// fila en Rust, contar). `None` (nunca un error) si el predicado no
    /// tiene esta forma exacta, o algún campo no es pusheable -- el caller
    /// (`runtime/mod.rs`) cae al camino interpretado, que sigue siendo
    /// correcto siempre, solo más lento en ese caso.
    pub(crate) fn count_where_conjunction(&self, collection: &str, conditions: &[(String, BinaryOp, Value)]) -> Result<Option<i64>, RuntimeError> {
        let columns = self.columns.get(collection).ok_or_else(|| RuntimeError::new(format!("colección desconocida: '{collection}'")))?;
        let Some((where_clause, cells)) = self.conjunction_condition(collection, columns, conditions) else {
            return Ok(None);
        };
        let sql = format!("SELECT COUNT(*) FROM \"{collection}\" WHERE {where_clause}");
        let rows = self
            .backend
            .query(&sql, &cells, &[ColumnKind::Int])
            .map_err(|e| RuntimeError::new(format!("error en countWhere de '{collection}': {e}")))?;
        match rows.first().and_then(|r| r.first()) {
            Some(Cell::Int(n)) => Ok(Some(*n)),
            other => Err(RuntimeError::new(format!("countWhere de '{collection}' devolvió algo que no es un entero: {other:?}"))),
        }
    }

    /// Como `count_where_conjunction`, para `db.<c>.findWhere(|x| x.campo
    /// OP valor && ...)` -- un `SELECT ... WHERE` real, solo las filas que
    /// matchean viajan del motor al proceso (a diferencia del camino
    /// interpretado, que trae TODA la colección y filtra en Rust). Mismo
    /// criterio de `None` que `count_where_conjunction`.
    pub(crate) fn find_where_conjunction(&self, collection: &str, conditions: &[(String, BinaryOp, Value)]) -> Result<Option<Vec<Value>>, RuntimeError> {
        let columns = self.columns.get(collection).ok_or_else(|| RuntimeError::new(format!("colección desconocida: '{collection}'")))?;
        let Some((where_clause, cells)) = self.conjunction_condition(collection, columns, conditions) else {
            return Ok(None);
        };
        let mut col_list = vec!["\"id\"".to_string()];
        col_list.extend(columns.iter().map(|c| format!("\"{}\"", c.field.name)));
        let sql = format!("SELECT {} FROM \"{collection}\" WHERE {where_clause} ORDER BY \"id\"", col_list.join(", "));
        let mut kinds = vec![ColumnKind::Int];
        kinds.extend(columns.iter().map(ColumnPlan::kind));
        let rows = self
            .backend
            .query(&sql, &cells, &kinds)
            .map_err(|e| RuntimeError::new(format!("error en findWhere de '{collection}': {e}")))?;
        rows.iter().map(|cells| self.row_to_fields(collection, cells, columns).map(Value::Struct)).collect::<Result<Vec<_>, _>>().map(Some)
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
        let where_clause = self.soft_delete_where(collection).map(|c| format!("WHERE {c} ")).unwrap_or_default();
        let sql = format!(
            "SELECT {} FROM \"{collection}\" {where_clause}ORDER BY \"id\" LIMIT {} OFFSET {}",
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
        rows.iter().map(|cells| self.row_to_fields(collection, cells, columns).map(Value::Struct)).collect()
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
        let soft_delete_cond = self.soft_delete_where(collection);
        let sql = match (after, &soft_delete_cond) {
            (Some(_), Some(sd)) => format!(
                "SELECT {} FROM \"{collection}\" WHERE \"id\" > {} AND {sd} ORDER BY \"id\" LIMIT {}",
                col_list.join(", "),
                self.backend.placeholder(1),
                self.backend.placeholder(2)
            ),
            (Some(_), None) => format!(
                "SELECT {} FROM \"{collection}\" WHERE \"id\" > {} ORDER BY \"id\" LIMIT {}",
                col_list.join(", "),
                self.backend.placeholder(1),
                self.backend.placeholder(2)
            ),
            (None, Some(sd)) => format!(
                "SELECT {} FROM \"{collection}\" WHERE {sd} ORDER BY \"id\" LIMIT {}",
                col_list.join(", "),
                self.backend.placeholder(1)
            ),
            (None, None) => format!(
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
        rows.iter().map(|cells| self.row_to_fields(collection, cells, columns).map(Value::Struct)).collect()
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
        let (key_field, granularity) = closure_group_key(args.first())?;
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
        // GRAMMAR.md §3.157: con truncado, la expresión de agrupación deja
        // de ser la columna cruda -- se repite la MISMA expresión en SELECT
        // y GROUP BY (portable en los dos motores; referenciar el alias
        // "key" en GROUP BY no es seguro en todo dialecto/versión).
        let key_expr = match granularity {
            Some(g) => truncate_timestamp_sql(&key_field, g, self.backend.is_postgres()),
            None => format!("\"{key_field}\""),
        };
        let where_clause = self.soft_delete_where(collection).map(|c| format!("WHERE {c} ")).unwrap_or_default();
        let sql =
            format!("SELECT {key_expr} AS \"key\", {value_expr} AS \"value\" FROM \"{collection}\" {where_clause}GROUP BY {key_expr}");
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

    /// `db.<c>.maxRow(selector)` / `db.<c>.minRow(selector)` (GRAMMAR.md
    /// §3.102): la fila COMPLETA con el valor máximo/mínimo de un campo --
    /// `SELECT ... ORDER BY "<campo>" {DESC|ASC} LIMIT 1`, a diferencia de
    /// `maxBy`/`minBy` (arriba), que solo agregan un VALOR, nunca la fila
    /// que lo alcanza. Reusa `row_to_fields` (mismo decodificador que
    /// `select_rows`/`find_where_conjunction`) para la fila entera, no
    /// `scalar_cell_to_value` (que `select_grouped` usa para una sola
    /// celda) -- por eso no comparte código con `select_grouped` más allá
    /// del `closure_field_name` inicial. `Value::Null` sobre una colección
    /// vacía (o completamente soft-deleteada), nunca un error.
    fn top_row(&self, collection: &str, columns: &[ColumnPlan], method: &str, args: &[Value]) -> Result<Value, RuntimeError> {
        let field = closure_field_name(args.first(), "de orden")?;
        if !columns.iter().any(|c| c.field.name == field) {
            return Err(RuntimeError::new(format!("'{method}': '{field}' no es una columna real de '{collection}'")));
        }
        let mut col_list = vec!["\"id\"".to_string()];
        col_list.extend(columns.iter().map(|c| format!("\"{}\"", c.field.name)));
        let where_clause = self.soft_delete_where(collection).map(|c| format!("WHERE {c} ")).unwrap_or_default();
        let order = match method {
            "maxRow" => "DESC",
            "minRow" => "ASC",
            other => panic!("top_row llamado con un método que Db::call no debería enrutar acá: '{other}'"),
        };
        let sql = format!("SELECT {} FROM \"{collection}\" {where_clause}ORDER BY \"{field}\" {order} LIMIT 1", col_list.join(", "));
        let mut kinds = vec![ColumnKind::Int];
        kinds.extend(columns.iter().map(ColumnPlan::kind));
        let rows = self
            .backend
            .query(&sql, &[], &kinds)
            .map_err(|e| RuntimeError::new(format!("error de SQL en '{collection}': {e}")))?;
        match rows.into_iter().next() {
            Some(cells) => self.row_to_fields(collection, &cells, columns).map(Value::Struct),
            None => Ok(Value::Null),
        }
    }

    /// `db.<c>.increment(id, selector, delta) -> T` (GRAMMAR.md §3.105): un
    /// `UPDATE "t" SET "campo" = "campo" + ? WHERE "id" = ?` atómico -- SIN
    /// ida y vuelta de lectura previa, a diferencia de `upsert` con un
    /// `updateFn` que lee `existing.campo + delta` (dos procesos
    /// incrementando la MISMA fila a la vez pueden perder un incremento
    /// ahí -- lost-update real, encontrado en `bandit_rewards.link`/
    /// `bot_defense.link`/`banners.link` de IgnisLove, corriendo varios
    /// `linkc serve-all`/pm2 compartiendo un único Postgres). Mismo criterio
    /// de "no encontrado" que `applyPatch`: reconsulta por id después del
    /// `UPDATE` -- 0 filas afectadas y 0 filas en la reconsulta es la MISMA
    /// señal, "no existe ninguna fila con ese id", sin necesitar un chequeo
    /// aparte antes de escribir.
    fn increment(&self, collection: &str, columns: &[ColumnPlan], args: Vec<Value>) -> Result<Value, RuntimeError> {
        let mut it = args.into_iter();
        let id = as_int(&it.next().ok_or_else(|| RuntimeError::new("increment requiere 3 argumentos (id, selector, delta)"))?)?;
        let selector = it.next();
        let field = closure_field_name(selector.as_ref(), "a incrementar")?;
        let delta = as_int(&it.next().ok_or_else(|| RuntimeError::new("increment requiere 3 argumentos (id, selector, delta)"))?)?;
        if !columns.iter().any(|c| c.field.name == field) {
            return Err(RuntimeError::new(format!("'increment': '{field}' no es una columna real de '{collection}'")));
        }
        let sql = format!(
            "UPDATE \"{collection}\" SET \"{field}\" = \"{field}\" + {} WHERE \"id\" = {}",
            self.backend.placeholder(1),
            self.backend.placeholder(2)
        );
        self.backend.execute(&sql, &[Cell::Int(delta), Cell::Int(id)]).map_err(|e| write_error("increment", e))?;
        let updated = self
            .select_rows(collection, columns, Some(id))?
            .into_iter()
            .next()
            .ok_or_else(|| RuntimeError::new(format!("no hay ningún elemento con id {id} en '{collection}'")))?;
        self.publish(collection, &updated);
        Ok(updated)
    }

    /// Reconstruye una fila entera (`"id"` + cada columna declarada) como los
    /// pares `(nombre, Value)` de un `Value::Struct` -- inversa de
    /// `write_param`. Las celdas llegan en el mismo orden que emitió el SELECT
    /// (`select_rows`), que es el orden de `columns` con `"id"` adelante.
    ///
    /// Devuelve `Result`, no un `Vec` liso (PLAN.md §9.1.1): un campo
    /// REQUERIDO (no `T?`, no `x?: T`) que la base devuelve `NULL` es un
    /// desacuerdo real entre lo que el `.link` promete y lo que la fila
    /// física tiene -- típico tras migrar `x: T` -> `x?: T`/`T?` -> `x: T`
    /// en PostgreSQL, donde `connect_postgres` SIEMPRE agrega una columna
    /// nueva nullable (nunca puede saber qué poner en filas viejas) sin
    /// importar si el campo es requerido en el `.link` actual, así que una
    /// fila insertada ANTES de ese cambio queda con `NULL` en una columna
    /// que el contrato TypeScript declara no-nullable. Antes de esta ronda
    /// esto decodificaba silenciosamente a `Value::Null` -- el cliente
    /// tipado recibía `null` en un campo `string` sin ningún error en
    /// ningún lado, exactamente la clase de "los dos extremos no están de
    /// acuerdo" que este proyecto viene evitando desde §3.9. Ahora es un
    /// error de runtime limpio (5xx JSON normal, nunca un panic que
    /// tumbe el proceso entero -- `handle_rpc` corre sincrónico en el hilo
    /// principal del accept-loop, server.rs) que nombra la colección y el
    /// campo.
    fn row_to_fields(&self, collection: &str, cells: &[Cell], columns: &[ColumnPlan]) -> Result<Vec<(String, Value)>, RuntimeError> {
        let mut out = Vec::with_capacity(columns.len() + 1);
        let Some(Cell::Int(id)) = cells.first() else {
            panic!("la columna 'id' es la clave primaria: siempre es un entero no nulo, y llegó {:?}", cells.first());
        };
        out.push(("id".to_string(), Value::Int(*id)));

        let null_but_required = |field_name: &str| {
            RuntimeError::new(format!(
                "la colección '{collection}' tiene una fila (id={id}) con NULL en '{field_name}', pero el programa actual \
                 declara ese campo requerido (no `T?` ni `x?: T`) -- típico tras una migración de PostgreSQL, que siempre \
                 agrega una columna nueva como nullable sin importar si el campo es requerido en el `.link` (ver \
                 GRAMMAR.md §9.1.1): una fila insertada ANTES del cambio de tipo/opcionalidad queda con NULL ahí. \
                 Backfilleá esa columna a mano o volvé el campo a opcional."
            ))
        };

        for (col, cell) in columns.iter().zip(cells.iter().skip(1)) {
            if col.json {
                match cell {
                    // NULL en una columna JSON SIEMPRE significa "clave
                    // ausente" -- solo alcanzable si `field.optional` (ver
                    // `write_param`, nunca escribimos NULL acá si la clave es
                    // requerida). Si la clave NO es opcional y de todos modos
                    // llegó NULL, es el mismo desacuerdo real que el campo
                    // nativo de abajo -- error limpio, no `Value::Null` silencioso.
                    Cell::Null => {
                        if !col.field.optional {
                            return Err(null_but_required(&col.field.name));
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
                (Type::Uuid, Cell::Text(t)) => Some(Value::Uuid(t.clone())),
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
                // fuerza a `json` para tener el 3er estado. Si la clave NO es
                // opcional NI el tipo declarado es `T?`, un NULL acá es el
                // mismo desacuerdo real de arriba -- error limpio.
                None if col.field.optional => {}
                None if matches!(col.field.ty, Type::Optional(_)) => out.push((col.field.name.clone(), Value::Null)),
                None => return Err(null_but_required(&col.field.name)),
            }
        }
        Ok(out)
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
            Value::Uuid(s) => Cell::Text(s.clone()),
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

/// Igual que `closure_field_name`, pero para el selector de CLAVE de
/// `sumBy`/`countBy`/`avgBy`/`maxBy`/`minBy` -- admite además la forma con
/// truncado de fecha (GRAMMAR.md §3.157). El checker ya validó el shape en
/// compilación; esto vuelve a correr el mismo reconocedor sobre el
/// `Value::Closure` que de verdad llegó, mismo criterio que
/// `closure_field_name`.
fn closure_group_key(arg: Option<&Value>) -> Result<(String, Option<TimeGranularity>), RuntimeError> {
    let Some(Value::Closure(params, body, _)) = arg else {
        return Err(RuntimeError::new("selector de agrupación inválido: se esperaba un closure"));
    };
    crate::ast::recognize_group_key_selector(params, body)
        .map(|(field, g)| (field.to_string(), g))
        .ok_or_else(|| RuntimeError::new("selector de agrupación inválido: se esperaba `|item: T| item.campo`"))
}

/// Expresión SQL que trunca un campo `Timestamp` (milisegundos-desde-epoch
/// UTC, `Type::Timestamp` -- GRAMMAR.md §3.31) al inicio de su día/mes/año
/// EN UTC, devolviendo de nuevo milisegundos-desde-epoch como un entero
/// plano -- nunca un tipo de fecha nativo del motor, para que
/// `scalar_cell_to_value`/`ColumnKind::Timestamp` lo decodifiquen exacto
/// igual que cualquier otra columna `Timestamp` (que ya intenta `i64`
/// primero, `postgres_timestamp_cell`, store.rs). Los dos backends
/// DIVERGEN de verdad acá (GRAMMAR.md §3.65 lo documentaba como el motivo
/// para no apurar esto): SQLite trunca con los modificadores nativos de
/// `strftime` (`'start of day'`/`'start of month'`/`'start of year'`),
/// Postgres con `date_trunc(unit, ts, 'UTC')` (el overload de 3 argumentos,
/// PG 9.4+, que trunca EN una zona horaria explícita sin depender del
/// `TimeZone` de la sesión -- usar la variante de 2 argumentos habría dado
/// un resultado distinto según cómo esté configurado el servidor, un bug
/// real y silencioso, no solo una diferencia de estilo).
fn truncate_timestamp_sql(field: &str, granularity: TimeGranularity, is_postgres: bool) -> String {
    if is_postgres {
        let unit = match granularity {
            TimeGranularity::Day => "day",
            TimeGranularity::Month => "month",
            TimeGranularity::Year => "year",
        };
        format!("CAST(EXTRACT(EPOCH FROM date_trunc('{unit}', to_timestamp(\"{field}\" / 1000.0), 'UTC')) * 1000 AS BIGINT)")
    } else {
        let modifier = match granularity {
            TimeGranularity::Day => "start of day",
            TimeGranularity::Month => "start of month",
            TimeGranularity::Year => "start of year",
        };
        // Bug real, encontrado por una auditoría multi-agente adversarial
        // (26/08/2026): `"campo" / 1000` con AMBOS operandos enteros es
        // división entera de SQLite, que trunca HACIA CERO -- para un
        // epoch PRE-1970 (negativo) con resto de milisegundos no nulo,
        // redondea hacia arriba (más cerca de 1970) en vez de hacia abajo,
        // empujando la fila al día/mes/año siguiente en vez del correcto.
        // `/ 1000.0` (división real, como ya hacía el lado Postgres) deja
        // que `strftime(..., 'unixepoch', ...)` reciba segundos
        // fraccionarios de verdad y trunque el CALENDARIO correctamente --
        // confirmado con SQLite real: `-500 / 1000` da `0` (1970-01-01),
        // `-500 / 1000.0` da `-86400` segundos (1969-12-31), el día real.
        format!("(CAST(strftime('%s', \"{field}\" / 1000.0, 'unixepoch', '{modifier}') AS INTEGER) * 1000)")
    }
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
        // Mismo motivo que el brazo de Int64 de arriba, agregado en la
        // MISMA ronda que hace a `Timestamp` alcanzable como clave de
        // agrupación por primera vez (GRAMMAR.md §3.157, truncado de
        // fecha): sin este brazo, una clave `Timestamp` caía al genérico
        // de abajo y viajaba como NÚMERO plano en el wire, rompiendo la
        // promesa de §3.31 (siempre string ISO-8601) en silencio -- el
        // mismo bug de etiquetado que §3.65 ya había encontrado y cerrado
        // para Int64, ahora con Timestamp en vez de descubrirlo tarde.
        (Type::Timestamp, Cell::Int(n)) => Value::Timestamp(*n),
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

    /// Invariante de arquitectura para el Pilar 1 del roadmap de
    /// concurrencia (26/08/2026): `runtime/server.rs` comparte `Db` entre
    /// hilos de request vía `Arc<Db>`, lo que requiere `Db: Sync` (y
    /// `Send`, para poder construirlo en un hilo y usarlo desde otro). Si
    /// algún cambio futuro reintroduce un campo no-`Sync` (un `RefCell`
    /// suelto, por ejemplo) esto falla en COMPILACIÓN, no en runtime -- la
    /// señal más barata posible de que se rompió la premisa de la que
    /// depende todo el modelo de un-hilo-por-request.
    #[test]
    fn db_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Db>();
    }

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

    /// GRAMMAR.md §3.150: `sent_at_ms` viaja en el payload real (armado por
    /// `try_notify_remote`) y se decodifica tal cual -- la métrica de
    /// latencia de `server.rs` depende de que este valor sea EXACTO, no
    /// recalculado del lado receptor.
    #[test]
    fn parse_remote_notification_decodes_sent_at_ms() {
        let payload = serde_json::json!({
            "instance": "otra-instancia",
            "collection": "items",
            "event": {"id": 1},
            "sent_at_ms": 1_700_000_000_000i64,
        })
        .to_string();
        let change = parse_remote_notification(&payload, "mi-instancia").expect("debió parsear");
        assert_eq!(change.sent_at_ms, 1_700_000_000_000);
    }

    /// Compatibilidad hacia atrás: un payload de una instancia vieja (antes
    /// de GRAMMAR.md §3.150, sin este campo) sigue propagando el evento --
    /// solo pierde la métrica de latencia para ESE evento puntual.
    #[test]
    fn parse_remote_notification_tolerates_a_payload_without_sent_at_ms() {
        let payload = serde_json::json!({
            "instance": "otra-instancia",
            "collection": "items",
            "event": {"id": 1},
        })
        .to_string();
        let change = parse_remote_notification(&payload, "mi-instancia").expect("debió parsear igual, sin el campo nuevo");
        assert_eq!(change.event, serde_json::json!({"id": 1}));
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

    // ---- modo adopción (`--adopt-existing`/`LINK_ADOPT_EXISTING`, GRAMMAR.md §3.67) ----

    fn program_from(src: &str) -> Program {
        crate::parser::parse(crate::lexer::tokenize(src).unwrap()).unwrap()
    }

    #[test]
    fn adopting_an_existing_table_with_extra_unmodeled_columns_works() {
        let path = std::env::temp_dir().join("c_script_test_adopt_extra_column.db");
        let _ = std::fs::remove_file(&path);

        // Tabla "legacy": tiene una columna que el .link de abajo NUNCA declara.
        {
            let raw = Connection::open(&path).unwrap();
            raw.execute(
                "CREATE TABLE \"items\" (\"id\" INTEGER PRIMARY KEY AUTOINCREMENT, \"name\" TEXT NOT NULL, \"legacy_note\" TEXT)",
                [],
            )
            .unwrap();
            raw.execute("INSERT INTO \"items\" (\"name\", \"legacy_note\") VALUES ('Ada', 'columna que este programa no conoce')", [])
                .unwrap();
        }

        let program = program_from("type Item = { id: Int, name: String } db { items: Item[] }");
        let db = Db::new_with_options(&program, &path, true);
        let Value::List(rows) = db.call("items", "all", vec![]).unwrap() else { panic!("se esperaba lista") };
        assert_eq!(rows.len(), 1, "adoptar no crea la tabla ni la vacía -- la fila preexistente sigue ahí");
        let Value::Struct(fields) = &rows[0] else { panic!("se esperaba struct") };
        assert_eq!(fields.iter().find(|(n, _)| n == "name").map(|(_, v)| v.clone()), Some(Value::Str("Ada".to_string())));
        assert!(
            !fields.iter().any(|(n, _)| n == "legacy_note"),
            "una columna física no declarada en el .link se ignora, nunca se filtra al Value"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    #[should_panic(expected = "no existe como tabla")]
    fn adopting_a_table_that_does_not_exist_fails_instead_of_creating_it() {
        let path = std::env::temp_dir().join("c_script_test_adopt_missing_table.db");
        let _ = std::fs::remove_file(&path);
        let program = program_from("type Item = { id: Int, name: String } db { items: Item[] }");
        let _ = Db::new_with_options(&program, &path, true);
    }

    #[test]
    #[should_panic(expected = "faltan columnas")]
    fn adopting_a_table_missing_a_declared_column_fails_even_when_the_field_is_optional() {
        let path = std::env::temp_dir().join("c_script_test_adopt_missing_column.db");
        let _ = std::fs::remove_file(&path);

        // Sin `note`: en modo normal, un campo OPCIONAL faltante se
        // auto-agregaría con ALTER TABLE ADD COLUMN sin drama. En modo
        // adopción esto tiene que fallar igual -- el punto entero es no
        // ejecutar NINGÚN DDL, ni siquiera uno no destructivo.
        {
            let raw = Connection::open(&path).unwrap();
            raw.execute("CREATE TABLE \"items\" (\"id\" INTEGER PRIMARY KEY AUTOINCREMENT, \"name\" TEXT NOT NULL)", []).unwrap();
        }

        let program = program_from("type Item = { id: Int, name: String, note?: String } db { items: Item[] }");
        let _ = Db::new_with_options(&program, &path, true);
    }

    // ---- índices declarativos: `@index`/`@unique` (GRAMMAR.md §3.80) ----

    #[test]
    fn unique_field_creates_a_real_sqlite_unique_index_and_rejects_duplicate_inserts() {
        let path = std::env::temp_dir().join("c_script_test_unique_index.db");
        let _ = std::fs::remove_file(&path);
        let program = program_from("type User = { id: Int, @unique email: String } db { users: User[] }");
        let db = Db::new(&program, &path);

        db.call("users", "insert", vec![Value::Struct(vec![("email".into(), Value::Str("a@x.com".into()))])])
            .unwrap();
        let dup = db.call("users", "insert", vec![Value::Struct(vec![("email".into(), Value::Str("a@x.com".into()))])]);
        let err = dup.expect_err("un segundo insert con el mismo email ya usado debe rechazarse");
        assert_eq!(
            err.kind,
            crate::runtime::ErrorKind::BadRequest,
            "una violación de @unique es un error del CLIENTE (400), no del servidor: {err:?}"
        );

        drop(db);
        let raw = Connection::open(&path).unwrap();
        let sql: String = raw
            .query_row("SELECT sql FROM sqlite_master WHERE type = 'index' AND name = 'idx_users_email'", [], |r| r.get(0))
            .expect("el índice único debe existir de verdad en SQLite, no solo aplicarse desde el intérprete");
        assert!(sql.to_uppercase().contains("UNIQUE"), "{sql}");

        let _ = std::fs::remove_file(&path);
    }

    /// GRAMMAR.md §3.96: el caso real que motiva `@check` -- una "barrera a
    /// nivel de base", no solo del lado de la aplicación. Este test escribe
    /// SQL crudo, sin pasar por `Db::call`/`apply_field_validators`
    /// (`runtime/mod.rs`) en absoluto -- exactamente el escenario "otro rpc
    /// inserta sin pasar por esa función" que el reporte citaba. Si
    /// `@check` solo viviera del lado de la aplicación, este insert
    /// pasaría sin problema.
    #[test]
    fn check_field_creates_a_real_sqlite_check_constraint_that_rejects_raw_sql_too() {
        let path = std::env::temp_dir().join("c_script_test_check_constraint.db");
        let _ = std::fs::remove_file(&path);
        let program = program_from("type Review = { id: Int, @check(range, 1, 5) rating: Int } db { reviews: Review[] }");
        let db = Db::new(&program, &path);
        drop(db);

        let raw = Connection::open(&path).unwrap();
        let sql: String = raw
            .query_row("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'reviews'", [], |r| r.get(0))
            .unwrap();
        assert!(sql.contains("CHECK"), "el CHECK debe existir de verdad en la tabla física: {sql}");

        let err = raw.execute("INSERT INTO \"reviews\" (\"rating\") VALUES (99)", []).unwrap_err();
        assert!(
            format!("{err}").to_uppercase().contains("CHECK"),
            "un INSERT crudo que viola @check debe rechazarse a nivel de SQLite, sin pasar por Rust: {err}"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn unique_violation_on_apply_patch_is_rejected_as_bad_request() {
        let path = std::env::temp_dir().join("c_script_test_unique_index_patch.db");
        let _ = std::fs::remove_file(&path);
        let program = program_from("type User = { id: Int, @unique email: String } db { users: User[] }");
        let db = Db::new(&program, &path);

        db.call("users", "insert", vec![Value::Struct(vec![("email".into(), Value::Str("a@x.com".into()))])])
            .unwrap();
        let b = db
            .call("users", "insert", vec![Value::Struct(vec![("email".into(), Value::Str("b@x.com".into()))])])
            .unwrap();
        let Value::Struct(fields) = &b else { panic!("se esperaba struct") };
        let b_id = fields.iter().find(|(n, _)| n == "id").map(|(_, v)| as_int(v).unwrap()).unwrap();

        let patched = db.call(
            "users",
            "applyPatch",
            vec![Value::Int(b_id), Value::Struct(vec![("email".into(), Value::Str("a@x.com".into()))])],
        );
        let err = patched.expect_err("pisar el email de 'b' con el de 'a' (ya único) debe rechazarse");
        assert_eq!(err.kind, crate::runtime::ErrorKind::BadRequest, "{err:?}");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_plain_index_field_does_not_block_duplicate_values() {
        // `@index` (sin `unique: true`) solo acelera lecturas -- a
        // diferencia de `@unique`, dos filas con el mismo valor son
        // perfectamente válidas.
        let path = std::env::temp_dir().join("c_script_test_plain_index.db");
        let _ = std::fs::remove_file(&path);
        let program = program_from("type User = { id: Int, @index country: String } db { users: User[] }");
        let db = Db::new(&program, &path);

        db.call("users", "insert", vec![Value::Struct(vec![("country".into(), Value::Str("AR".into()))])]).unwrap();
        db.call("users", "insert", vec![Value::Struct(vec![("country".into(), Value::Str("AR".into()))])])
            .expect("un índice no-único no debe rechazar valores repetidos");

        drop(db);
        let raw = Connection::open(&path).unwrap();
        let sql: String = raw
            .query_row("SELECT sql FROM sqlite_master WHERE type = 'index' AND name = 'idx_users_country'", [], |r| r.get(0))
            .expect("el índice debe existir de verdad en SQLite");
        assert!(!sql.to_uppercase().contains("UNIQUE"), "{sql}");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn adopt_existing_never_creates_an_index_even_when_the_field_is_annotated() {
        let path = std::env::temp_dir().join("c_script_test_adopt_no_index.db");
        let _ = std::fs::remove_file(&path);
        {
            let raw = Connection::open(&path).unwrap();
            raw.execute(
                "CREATE TABLE \"users\" (\"id\" INTEGER PRIMARY KEY AUTOINCREMENT, \"email\" TEXT NOT NULL)",
                [],
            )
            .unwrap();
        }

        let program = program_from("type User = { id: Int, @unique email: String } db { users: User[] }");
        let db = Db::new_with_options(&program, &path, true);
        drop(db);

        let raw = Connection::open(&path).unwrap();
        let count: i64 = raw
            .query_row("SELECT count(*) FROM sqlite_master WHERE type = 'index' AND name = 'idx_users_email'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "--adopt-existing nunca ejecuta DDL, ni siquiera para un índice declarado");

        let _ = std::fs::remove_file(&path);
    }
}
