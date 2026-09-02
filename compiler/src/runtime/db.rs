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

use super::{as_int, encryption, generate_uuid_v4, json_to_typed_value, simple_enum_names, value_to_json, ConditionExpr, RuntimeError, Value};
use crate::ast::{BinaryOp, FieldCheck, Item, Program, TimeGranularity, TypeAnnotation, TypeExpr};
use crate::checker::Checker;
use crate::rate_limit::RateLimitSpec;
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
pub(crate) struct ColumnPlan {
    pub(crate) field: FieldType,
    /// Tipo de columna DDL: `"INTEGER"`, `"REAL"` o `"TEXT"`. Cuando
    /// `json` es `true` siempre es `"TEXT"`.
    sql_type: &'static str,
    /// `true` => la columna guarda `serde_json::to_string(value_to_json(v))`
    /// (structs, enums con datos, listas, tuplas, Map, genéricos, uniones,
    /// Result/Patch, o el caso `x?: T?` -- ver `for_field`). `false` => la
    /// columna guarda el valor nativo tal cual (Int/Float/String/Bool/enum
    /// simple).
    pub(crate) json: bool,
    /// `@encrypted` (GRAMMAR.md §3.191) -- `true` solo para un campo
    /// `String`/`String?` así marcado (el checker ya lo garantizó). La
    /// columna SQL sigue siendo `TEXT` normal, sin `ColumnKind` nuevo --
    /// `write_param`/`decode_row` son los únicos dos puntos que miran este
    /// campo, para cifrar al escribir/descifrar al leer.
    pub(crate) encrypted: bool,
}

impl ColumnPlan {
    /// `x?: T?` (opcional-por-clave Y nullable-por-tipo a la vez, GRAMMAR.md
    /// §3.4) es el único caso que SIEMPRE necesita el envoltorio JSON así T
    /// sea nativo: una sola columna SQL solo tiene un bit de NULL, y acá
    /// hacen falta 3 estados (ausente / presente-null / presente-valor). El
    /// texto JSON de `Value::Null` es simplemente `"null"`, así que ese
    /// tercer estado sale gratis de `value_to_json`/`json_to_typed_value`
    /// sin ningún caso especial en el resto de este archivo -- ver
    /// `write_param`/`row_to_fields`. `encrypted` viene de
    /// `encrypted_fields_by_collection` (el mismo cruce
    /// `program.items`/`checker.db_collections()` que `soft_delete_fields_by_collection`
    /// ya usa) -- `FieldType` es estructural, sin anotaciones, así que el
    /// caller es quien la resuelve, no `for_field`.
    pub(crate) fn for_field(field: FieldType, simple_enums: &HashSet<String>, encrypted: bool) -> Self {
        let double_optional = field.optional && matches!(field.ty, Type::Optional(_));
        let effective_ty: &Type = match &field.ty {
            Type::Optional(inner) => inner.as_ref(),
            other => other,
        };
        match if double_optional { None } else { native_sql_type(effective_ty, simple_enums) } {
            Some(sql_type) => ColumnPlan { field, sql_type, json: false, encrypted },
            None => ColumnPlan { field, sql_type: "TEXT", json: true, encrypted },
        }
    }

    fn not_null(&self) -> bool {
        !self.field.optional && !matches!(self.field.ty, Type::Optional(_))
    }

    /// Qué se lee de esta columna. Se deriva del MISMO plan que decide cómo se
    /// escribe, así que lectura y escritura no pueden divergir.
    pub(crate) fn kind(&self) -> ColumnKind {
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
            // GRAMMAR.md §3.184: a diferencia de Int64, NO reusa
            // `ColumnKind::Int` -- el rango de i128 no cabe en el i64 que
            // ese kind asume en todos lados (SQLite/Postgres INT2-INT8).
            Type::Decimal => ColumnKind::Decimal,
            Type::Float => ColumnKind::Float,
            Type::Bool => ColumnKind::Bool,
            Type::String | Type::Uuid | Type::Enum(_) => ColumnKind::Text,
            other => unreachable!("tipo nativo inesperado en una columna no-JSON: {other:?}"),
        }
    }
}

/// GRAMMAR.md §3.177: qué tipo es la PK de una colección -- deriva todo
/// lo demás que cambia entre las dos formas: columna DDL INTEGER
/// AUTOINCREMENT/BIGSERIAL vs TEXT/UUID, generación por autoincremento
/// del motor vs generada del lado de la app antes del INSERT, y el
/// `ColumnKind` con el que se lee/escribe la columna `"id"`
/// (`ColumnKind::Uuid`, distinto de `Text`, solo para esta PK -- ver
/// `store.rs::Cell::to_sql`/`postgres_cell` para el porqué).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IdKind {
    Int,
    Uuid,
}

impl IdKind {
    /// `Checker::validate_db_element_type` ya garantizó que el campo
    /// `id` de toda colección es `Int` o `Uuid` -- cualquier otro tipo
    /// nunca llega hasta acá.
    pub(crate) fn from_field_type(ty: &Type) -> Self {
        match ty {
            Type::Uuid => IdKind::Uuid,
            _ => IdKind::Int,
        }
    }
}

/// El `ColumnKind` con el que hay que decodificar/bindear la columna `"id"`
/// -- `Int` o `Uuid` (GRAMMAR.md §3.177), según `id_kind`. Función libre
/// (GRAMMAR.md §3.185) para que `db_admin.rs` (`linkc db export`) pueda
/// armar el `SELECT` de la columna `"id"` sin necesitar un `Db` completo --
/// `Db::id_column_kind` es ahora un wrapper de una línea sobre esto mismo.
pub(crate) fn id_column_kind_for(id_kind: IdKind) -> ColumnKind {
    match id_kind {
        // GRAMMAR.md §3.177: `ColumnKind::Uuid`, no `Text` -- distinto de
        // cualquier otro campo `Uuid` normal (que sí es `Text`, sin
        // cambios). SQLite decodifica los dos exactamente igual (no tiene
        // un tipo separado del texto), pero Postgres SÍ: la PK usa el tipo
        // NATIVO `uuid`, cuyo formato binario de verdad (16 bytes crudos)
        // no es el mismo que el de un `TEXT` común (los bytes UTF-8 de los
        // 36 caracteres) -- `postgres_cell` necesita saber la diferencia
        // para leer la columna sin reventar contra una tabla adoptada con
        // `id` nativamente `uuid`. Ver `store.rs::Cell::to_sql` para el
        // lado de ESCRITURA del mismo problema.
        IdKind::Int => ColumnKind::Int,
        IdKind::Uuid => ColumnKind::Uuid,
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
        // GRAMMAR.md §3.184: SQLite no tiene un tipo decimal nativo -- el
        // valor YA escalado ×10.000 se guarda como INTEGER (exacto, cabe
        // sobrado en 64 bits para cualquier magnitud financiera real; ver
        // `Cell::to_sql` para el chequeo de rango al escribir).
        Type::Decimal => Some("INTEGER"),
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

/// Nombre de colección -> nombres de sus campos `@encrypted` (GRAMMAR.md
/// §3.191) -- mismo cruce que `soft_delete_fields_by_collection`, pero SIN
/// el límite de "a lo sumo uno" (varios campos `@encrypted` en el mismo
/// struct son perfectamente válidos). `pub(crate)` -- `db_admin.rs`
/// (`export`/`import`) también lo necesita, para rechazar de entrada un
/// programa con algún campo `@encrypted` (ver GRAMMAR.md §3.191, "Límites
/// honestos": ninguno de los dos soporta cifrado todavía).
pub(crate) fn encrypted_fields_by_collection(program: &Program, checker: &Checker) -> HashMap<String, HashSet<String>> {
    let mut result = HashMap::new();
    for (coll_name, element_ty) in checker.db_collections() {
        let Type::Struct { name: Some(type_name), .. } = element_ty else { continue };
        for item in &program.items {
            let Item::Type(t) = item else { continue };
            if &t.name != type_name {
                continue;
            }
            let TypeExpr::Struct(fields) = &t.ty else { continue };
            let encrypted: HashSet<String> = fields.iter().filter(|f| f.encrypted()).map(|f| f.name.clone()).collect();
            if !encrypted.is_empty() {
                result.insert(coll_name.clone(), encrypted);
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

/// Nombre de colección -> lista de `(campos, condición SQL opcional)` por
/// cada `@unique(...)` COMPUESTO de su tipo de elemento (GRAMMAR.md §3.155/
/// §3.174).
pub(crate) type CompositeUniquesByCollection = HashMap<String, Vec<(Vec<String>, Option<String>)>>;

/// Ver el alias de arriba para la forma del resultado -- mismo cruce
/// `checker.db_collections()` + `program.items` que
/// `index_fields_by_collection` arriba, mismo motivo (la anotación vive en
/// `ast::TypeDecl`, no en el `Type` ya resuelto). La condición (`where
/// <expr>`, §3.174) ya viene TRADUCIDA a SQL (`type_check_expr_sql`, misma
/// función que usa `@check` de tipo) -- el checker
/// (`Checker::check_type_annotations`) ya validó su forma y su tipo.
pub(crate) fn composite_unique_by_collection(program: &Program, checker: &Checker) -> CompositeUniquesByCollection {
    let mut result = HashMap::new();
    for (coll_name, element_ty) in checker.db_collections() {
        let Type::Struct { name: Some(type_name), .. } = element_ty else { continue };
        for item in &program.items {
            let Item::Type(t) = item else { continue };
            if &t.name != type_name {
                continue;
            }
            let sets: Vec<(Vec<String>, Option<String>)> = t
                .annotations
                .iter()
                .filter_map(|a| match a {
                    TypeAnnotation::Unique(fields, condition) => {
                        Some((fields.clone(), condition.as_ref().map(|c| type_check_expr_sql(&c.node))))
                    }
                    TypeAnnotation::Check(_) => None,
                })
                .collect();
            if !sets.is_empty() {
                result.insert(coll_name.clone(), sets);
            }
        }
    }
    result
}

/// Nombre de colección -> lista de expresiones SQL ya traducidas, una por
/// cada `@check(<expr>)` de nivel `type` (GRAMMAR.md §3.173) -- mismo cruce
/// `checker.db_collections()` + `program.items` que `composite_unique_by_collection`
/// arriba. La traducción (`type_check_expr_sql`) asume que el checker
/// (`Checker::check_type_level_check_expr`) ya validó la forma Y el tipo de
/// la expresión -- acá no se repite ninguna de esas dos validaciones.
pub(crate) fn type_checks_by_collection(program: &Program, checker: &Checker) -> HashMap<String, Vec<String>> {
    let mut result = HashMap::new();
    for (coll_name, element_ty) in checker.db_collections() {
        let Type::Struct { name: Some(type_name), .. } = element_ty else { continue };
        for item in &program.items {
            let Item::Type(t) = item else { continue };
            if &t.name != type_name {
                continue;
            }
            let clauses: Vec<String> = t
                .annotations
                .iter()
                .filter_map(|a| match a {
                    TypeAnnotation::Check(expr) => Some(type_check_expr_sql(&expr.node)),
                    TypeAnnotation::Unique(..) => None,
                })
                .collect();
            if !clauses.is_empty() {
                result.insert(coll_name.clone(), clauses);
            }
        }
    }
    result
}

/// Traduce la expresión de un `@check(<expr>)` de nivel `type` (GRAMMAR.md
/// §3.173) a un booleano SQL real -- comparte sintaxis entre SQLite y
/// PostgreSQL (los mismos operadores, sin ninguna función específica de
/// motor), así que esta única función alimenta tanto `create_table_sql`
/// (acá abajo) como `codegen::postgres_emit::create_postgres_table_sql`,
/// igual que `check_clause_sql` para el `@check` de un solo campo.
///
/// Solo recibe formas que `Checker::check_type_level_check_expr` ya validó
/// como pusheables (identificadores que son campos declarados de ESTE
/// struct, literales, y los operadores de la lista de abajo) -- cualquier
/// otra forma (llamada, acceso a `db`, closure, campo de otro struct) ya
/// se rechazó en el checker, mucho antes de que el programa llegue a
/// generar SQL. `panic!` en el caso `_` de abajo documenta esa garantía en
/// vez de devolver `Option`/`Result` sin ningún caller real que pueda
/// fallar de verdad.
pub(crate) fn type_check_expr_sql(expr: &crate::ast::Expr) -> String {
    use crate::ast::Expr;
    match expr {
        Expr::Ident(name) => format!("\"{name}\""),
        Expr::Null => "NULL".to_string(),
        Expr::Int(n) => n.to_string(),
        Expr::Float(x) => x.to_string(),
        // `TRUE`/`FALSE`, no `1`/`0`: SQLite acepta las dos formas (desde
        // 3.23), pero Postgres NO convierte un entero a booleano en
        // silencio (`CHECK(activo AND 1)` falla ahí con un error de tipos)
        // -- las palabras clave son la única forma que funciona igual en
        // los dos motores, mismo criterio de "sin rama por backend" que el
        // resto de este archivo.
        Expr::Bool(b) => (if *b { "TRUE" } else { "FALSE" }).to_string(),
        Expr::Str(s) => format!("'{}'", s.replace('\'', "''")),
        Expr::Paren(inner) => format!("({})", type_check_expr_sql(&inner.node)),
        Expr::Unary { op: crate::ast::UnaryOp::Not, operand } => format!("NOT ({})", type_check_expr_sql(&operand.node)),
        Expr::Unary { op: crate::ast::UnaryOp::Neg, operand } => format!("-({})", type_check_expr_sql(&operand.node)),
        // `x = NULL`/`x != NULL` en SQL nunca es `true` (NULL no es igual a
        // nada, ni siquiera a sí mismo) -- mismo footgun que
        // `leaf_condition_sql` (§3.170) ya cerró para el pushdown de
        // predicados. `IS [NOT] NULL` es la forma SQL correcta para
        // expresar `campo == null`/`campo != null` acá.
        Expr::Binary { op: BinaryOp::Eq, left, right } if matches!(&left.node, Expr::Null) => {
            format!("({} IS NULL)", type_check_expr_sql(&right.node))
        }
        Expr::Binary { op: BinaryOp::Eq, left, right } if matches!(&right.node, Expr::Null) => {
            format!("({} IS NULL)", type_check_expr_sql(&left.node))
        }
        Expr::Binary { op: BinaryOp::NotEq, left, right } if matches!(&left.node, Expr::Null) => {
            format!("({} IS NOT NULL)", type_check_expr_sql(&right.node))
        }
        Expr::Binary { op: BinaryOp::NotEq, left, right } if matches!(&right.node, Expr::Null) => {
            format!("({} IS NOT NULL)", type_check_expr_sql(&left.node))
        }
        Expr::Binary { op, left, right } => {
            let l = type_check_expr_sql(&left.node);
            let r = type_check_expr_sql(&right.node);
            let sql_op = match op {
                BinaryOp::Eq => "=",
                BinaryOp::NotEq => "!=",
                BinaryOp::Lt => "<",
                BinaryOp::LtEq => "<=",
                BinaryOp::Gt => ">",
                BinaryOp::GtEq => ">=",
                BinaryOp::And => "AND",
                BinaryOp::Or => "OR",
                BinaryOp::Add => "+",
                BinaryOp::Sub => "-",
                BinaryOp::Mul => "*",
                BinaryOp::Div => "/",
                BinaryOp::Rem => "%",
                // `check_type_level_check_expr` ya filtra a estos doce --
                // cualquier otro operador (`&&`/`||` de cortocircuito con
                // efectos, `??`) nunca llega hasta acá.
                other => unreachable!("operador no pusheable en @check de nivel type: {other:?}"),
            };
            format!("({l} {sql_op} {r})")
        }
        other => unreachable!("forma no pusheable en @check de nivel type: {other:?}"),
    }
}

/// `CREATE UNIQUE INDEX IF NOT EXISTS ...` de VARIAS columnas a la vez, uno
/// por cada `@unique(...)` de nivel `type` (GRAMMAR.md §3.155), opcionalmente
/// PARCIAL (`WHERE <condición>`, GRAMMAR.md §3.174, `where <expr>`) -- mismo
/// criterio de idempotencia y nombre determinístico que `create_index_statements`
/// (arriba), con el nombre de TODOS los campos (Y la condición, si hay)
/// codificados sin ambigüedad (`composite_unique_index_name` abajo) para
/// que dos constraints compuestos sobre la misma tabla nunca colisionen de
/// nombre -- incluido el caso de dos `@unique` con EL MISMO conjunto de
/// campos pero condiciones DISTINTAS, que sin esto generarían el mismo
/// nombre de índice y la segunda sentencia sería un no-op silencioso.
pub(crate) fn create_composite_unique_statements(collection: &str, sets: &[(Vec<String>, Option<String>)]) -> Vec<String> {
    sets.iter()
        .map(|(fields, condition)| {
            let idx_name = composite_unique_index_name(collection, fields, condition.as_deref());
            let cols = fields.iter().map(|f| format!("\"{f}\"")).collect::<Vec<_>>().join(", ");
            let where_clause = condition.as_ref().map(|c| format!(" WHERE {c}")).unwrap_or_default();
            format!("CREATE UNIQUE INDEX IF NOT EXISTS \"{idx_name}\" ON \"{collection}\"({cols}){where_clause}")
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
/// `condition` (GRAMMAR.md §3.174, `where <expr>`) NO se concatena tal
/// cual -- a diferencia de un nombre de campo (identificador simple), el
/// SQL ya traducido de una condición trae comillas/paréntesis/espacios
/// (`("status" != 'cancelled')`), que romperían el identificador entre
/// comillas dobles que envuelve a este nombre completo (`"idx_..."`) si se
/// pegaran directo -- confirmado en vivo: un intento anterior sin hashear
/// producía `CREATE UNIQUE INDEX IF NOT EXISTS "idx_..._("status" != ...` y
/// SQLite lo rechazaba con un error de sintaxis a mitad del nombre. En vez
/// de escapar comillas a mano, se hashea con el MISMO SHA-256 que
/// `lockfile::hash_source` ya usa para otra cosa (detección de deriva) --
/// determinista, sin caracteres problemáticos, y sin sumar una segunda
/// implementación de hashing al proyecto. Dos `@unique` con el mismo
/// conjunto de campos pero condiciones DISTINTAS (o una con condición y
/// otra sin) igual generan nombres distintos -- lo único que importa acá
/// es que la MISMA condición siempre hashee igual (para que `CREATE UNIQUE
/// INDEX IF NOT EXISTS` reconozca el índice ya creado en el próximo
/// arranque) y una DISTINTA casi seguro hashee distinto.
pub(crate) fn composite_unique_index_name(collection: &str, fields: &[String], condition: Option<&str>) -> String {
    let encoded: String = fields.iter().map(|f| format!("{}${f}", f.len())).collect();
    let where_suffix = match condition {
        Some(c) => format!("_where_{}", &crate::lockfile::hash_source(c)[..16]),
        None => String::new(),
    };
    format!("idx_{collection}_uniq_{encoded}{where_suffix}")
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

fn create_table_sql(
    collection: &str,
    id_kind: IdKind,
    columns: &[ColumnPlan],
    checks: &[(String, FieldCheck)],
    type_checks: &[String],
) -> String {
    // GRAMMAR.md §3.177: una PK `Uuid` se genera del lado de la
    // aplicación (`crypto.uuid()`, `Db::call` "insert") ANTES de cada
    // INSERT, nunca por el motor -- así que la columna es TEXT sin
    // AUTOINCREMENT, no INTEGER. Mismo tipo SQL que cualquier otro campo
    // `Uuid` ya usa (`ColumnPlan::kind`), consistente con esa columna.
    // `NOT NULL` explícito en la rama Uuid: a diferencia de un `INTEGER
    // PRIMARY KEY` (el alias de `rowid`, que SQLite nunca deja en NULL
    // por construcción), un `PRIMARY KEY` sobre cualquier OTRO tipo --
    // incluido `TEXT` -- NO implica `NOT NULL` en SQLite (quirk
    // documentado del motor, distinto del estándar SQL); sin esto, un
    // `id` NULL pasaría el `CREATE TABLE ... STRICT` de arriba sin
    // ninguna queja.
    let id_def = match id_kind {
        IdKind::Int => "\"id\" INTEGER PRIMARY KEY AUTOINCREMENT".to_string(),
        IdKind::Uuid => "\"id\" TEXT PRIMARY KEY NOT NULL".to_string(),
    };
    let mut defs = vec![id_def];

    for col in columns {
        let not_null = if col.not_null() { " NOT NULL" } else { "" };
        let check_clause = match checks.iter().find(|(name, _)| name == &col.field.name) {
            Some((_, c)) => format!(" {}", check_clause_sql(&col.field.name, c)),
            None => String::new(),
        };
        defs.push(format!("\"{}\" {}{}{}", col.field.name, col.sql_type, not_null, check_clause));
    }
    // `@check(<expr>)` de nivel `type` (GRAMMAR.md §3.173) -- constraint de
    // TABLA, no de columna (a diferencia del loop de arriba), mismo lugar
    // que ocuparía cualquier `CHECK` de más de una columna.
    for sql in type_checks {
        defs.push(format!("CHECK {sql}"));
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
fn check_schema_matches(
    connection: &Connection,
    collection: &str,
    id_kind: IdKind,
    columns: &[ColumnPlan],
    db_path: &str,
) -> Result<(), RuntimeError> {
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
    // detectaría un mismatch falso desde el primer arranque. `id TEXT
    // PRIMARY KEY` (GRAMMAR.md §3.177) SÍ declara `NOT NULL` de verdad
    // (ver `create_table_sql`), así que ahí notnull=1 es lo esperado.
    let id_expected = match id_kind {
        IdKind::Int => ("INTEGER".to_string(), false),
        IdKind::Uuid => ("TEXT".to_string(), true),
    };
    expected.insert("id".to_string(), id_expected);
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
pub(crate) fn sqlite_table_exists(connection: &Connection, collection: &str) -> bool {
    connection
        .query_row("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1", [collection], |_| Ok(()))
        .is_ok()
}

/// GRAMMAR.md §3.178: rate limiting DISTRIBUIDO -- una tabla interna,
/// prefijo reservado (nunca colisiona con una colección declarada por el
/// usuario), compartida por TODAS las instancias de `linkc serve`/
/// `serve-all` que apunten a la MISMA base Postgres. Solo Postgres: SQLite
/// es de un solo archivo/proceso salvo un caso de borde raro, y el punto
/// entero de esto es coordinar ENTRE procesos -- el `RateLimiter` en
/// memoria (`rate_limit.rs`) ya es exacto para un solo proceso.
const RATE_LIMIT_TABLE: &str = "_linkc_internal_rate_limits";

fn create_rate_limit_table_sql() -> String {
    format!(
        "CREATE TABLE IF NOT EXISTS \"{RATE_LIMIT_TABLE}\" (\
            \"bucket_key\" TEXT PRIMARY KEY, \
            \"tokens\" DOUBLE PRECISION NOT NULL, \
            \"capacity\" DOUBLE PRECISION NOT NULL, \
            \"refill_per_sec\" DOUBLE PRECISION NOT NULL, \
            \"last_seen_ms\" BIGINT NOT NULL\
        )"
    )
}

/// GRAMMAR.md §3.192: `table_schema = ANY(current_schemas(false))` -- antes
/// hardcodeaba `table_schema = 'public'`, así que una tabla en cualquier
/// OTRO schema (visible por el `search_path` real de la sesión) se
/// reportaba como inexistente aunque estuviera ahí -- bug latente real,
/// independiente de `--db-schema`/§3.192, que ya podía morder a cualquiera
/// con un `search_path` propio configurado del lado de Postgres (`options=`
/// en la URL, o un rol con un `search_path` por default distinto de
/// `public`). `current_schemas(false)` es la función nativa de Postgres que
/// devuelve el `search_path` EFECTIVO de la sesión (`false` excluye
/// schemas implícitos como `pg_catalog`) -- la misma fuente de verdad que
/// Postgres mismo usa para resolver un identificador sin calificar, así
/// que esta consulta ahora ve exactamente lo mismo que vería un
/// `CREATE TABLE`/`SELECT` sin `"schema".` explícito contra la misma sesión.
fn postgres_table_exists(backend: &Backend, table: &str) -> Result<bool, String> {
    let rows = backend.query(
        "SELECT 1 FROM information_schema.tables WHERE table_schema = ANY(current_schemas(false)) AND table_name = $1",
        &[Cell::Text(table.to_string())],
        &[ColumnKind::Int],
    )?;
    Ok(!rows.is_empty())
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
pub(crate) fn validate_existing_id_column(backend: &Backend, collection: &str, expected: IdKind) -> Result<(), String> {
    // GRAMMAR.md §3.192: sin NINGÚN filtro de `table_schema` antes de esta
    // ronda -- con más de un schema visible en el `search_path` de la
    // sesión (o dos tablas del mismo nombre en schemas distintos), esto
    // podía leer la columna "id" de la tabla EQUIVOCADA en silencio. Mismo
    // fix que `postgres_table_exists`: filtrar por el `search_path`
    // EFECTIVO de la sesión, no una tabla-de-cualquier-schema-que-matchee.
    let sql = format!(
        "SELECT data_type FROM information_schema.columns WHERE table_name = {} AND column_name = 'id' AND table_schema = ANY(current_schemas(false))",
        backend.placeholder(1)
    );
    let rows = backend
        .query(&sql, &[Cell::Text(collection.to_string())], &[ColumnKind::Text])
        .map_err(|e| format!("no se pudo verificar el esquema de '{collection}' en PostgreSQL: {e}"))?;

    // Sin fila: o la tabla se acaba de crear (su "id" siempre es BIGSERIAL/
    // UUID según `expected`, por construcción -- nada que validar) o por
    // algún motivo no tiene columna "id" en absoluto, en cuyo caso
    // cualquier find/insert/delete sobre esta colección va a fallar de
    // todos modos con su propio mensaje. Ninguno de los dos casos es este
    // el lugar para inventar uno mejor.
    let Some(Cell::Text(data_type)) = rows.first().and_then(|row| row.first()) else {
        return Ok(());
    };
    // GRAMMAR.md §3.177: `id: Uuid` solo acepta una columna Postgres
    // NATIVA `uuid` -- no `text`/`varchar`, aunque guarden un UUID con
    // forma válida. Es un scope deliberado, no un descuido: el bind/la
    // lectura de esta columna (`Cell::to_sql`/`postgres_cell`, store.rs)
    // dan por sentado el formato BINARIO real de `uuid` (16 bytes), que
    // solo es correcto contra una columna genuinamente `uuid`. Aceptar
    // también `text` acá rompería esa lectura/escritura contra una
    // columna que en realidad guarda texto plano, sin ningún caso real
    // verificado que lo pida todavía (el motivador, iaacademy, tiene
    // columnas genuinamente `uuid`).
    let ok = match expected {
        IdKind::Int => matches!(data_type.as_str(), "bigint" | "integer" | "smallint"),
        IdKind::Uuid => data_type == "uuid",
    };
    if ok {
        return Ok(());
    }
    let (required, hint) = match expected {
        IdKind::Int => (
            "una clave primaria entera autoincremental (BIGSERIAL)".to_string(),
            "agregá una columna \"id\" BIGSERIAL nueva".to_string(),
        ),
        IdKind::Uuid => (
            "una clave primaria 'uuid' nativa de PostgreSQL".to_string(),
            "agregá una columna \"id\" UUID nueva, o cambiá el tipo del campo 'id' del .link a Int si esta tabla ya usa un entero/otro formato".to_string(),
        ),
    };
    Err(format!(
        "la tabla '{collection}' ya existe en PostgreSQL con \"id\" de tipo '{data_type}', pero c-script requiere {required} \
         -- típico al migrar desde otro backend. No se puede usar esta tabla sin migrarla a mano: {hint}, o apuntá esta \
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
    // GRAMMAR.md §3.192: mismo fix de `table_schema` que
    // `validate_existing_id_column` -- sin esto, una tabla de OTRO schema
    // con el mismo nombre podía compararse acá por error.
    let sql = format!(
        "SELECT column_name FROM information_schema.columns WHERE table_name = {} AND table_schema = ANY(current_schemas(false))",
        backend.placeholder(1)
    );
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
    // GRAMMAR.md §3.192: mismo fix de `table_schema` que
    // `validate_existing_id_column`/`warn_if_table_looks_unrelated`.
    let sql = format!(
        "SELECT column_name FROM information_schema.columns WHERE table_name = {} AND table_schema = ANY(current_schemas(false))",
        backend.placeholder(1)
    );
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
    /// GRAMMAR.md §3.222: los paths de cada `@route` ESTÁTICO y PÚBLICO del
    /// programa, calculados una vez acá porque el intérprete no tiene el
    /// `Program` a mano cuando `staticRoutes()` corre -- mismo criterio que
    /// `soft_delete_fields`. Orden de declaración, sin duplicados (el checker
    /// ya rechaza dos `@route` iguales).
    static_routes: Vec<String>,
    /// GRAMMAR.md §3.223: (host, clase de status) -> (conteo, suma de
    /// segundos) de cada llamada `http.*` SALIENTE que hizo este programa.
    /// Vive en `Db` y no en `MetricsStore` por la misma razón que
    /// `subscriber_counts`/`size_bytes`: el intérprete solo tiene `db` a
    /// mano cuando corre un `http.get`, y `MetricsStore` es del servidor.
    /// `/metrics` lo lee vía `outbound_http_stats`, mismo patrón.
    outbound_http: parking_lot::Mutex<HashMap<(String, String), (u64, f64)>>,
    /// Para que un evento PUBLICADO (`publish`, más abajo) serialice
    /// EXACTAMENTE igual que cualquier respuesta normal del mismo programa
    /// (mismo `value_to_json` que usa `invoke_rpc_with_sessions`).
    simple_enums: HashSet<String>,
    /// Nombre de colección -> plan de columnas (todo menos `id`), derivado
    /// del `Type::Struct` de esa colección al abrir la conexión.
    columns: HashMap<String, Vec<ColumnPlan>>,
    /// Nombre de colección -> tipo de su PK (GRAMMAR.md §3.177). Ausente
    /// de este mapa == `IdKind::Int` (`id_kind` de abajo defaultea así) --
    /// nunca puede pasar en la práctica (`Checker::validate_db_element_type`
    /// garantiza `id` en toda colección), pero el default barato evita un
    /// `unwrap`/panic en cualquier código que consulte esto.
    id_kinds: HashMap<String, IdKind>,
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
    /// `--encryption-key`/`LINK_ENCRYPTION_KEY` (GRAMMAR.md §3.191) -- mismo
    /// criterio EXACTO que `http_timeout` arriba: `None` hasta que
    /// `server.rs` lo sobreescribe UNA vez al arrancar (después de
    /// confirmar, si el programa declara algún campo `@encrypted`, que SÍ
    /// hay una clave real configurada -- `serve()` rechaza arrancar si no).
    /// `write_param`/`decode_row` lo leen para cifrar/descifrar cada campo
    /// `ColumnPlan::encrypted`.
    encryption_key: parking_lot::RwLock<Option<[u8; encryption::KEY_LEN]>>,
    /// Nombre de colección -> nombre del campo `@softDelete`, si esa
    /// colección tiene uno (GRAMMAR.md §3.78). Se calcula UNA vez al abrir
    /// la conexión (acá SÍ hay `Program`/`ast::Field` con anotaciones a
    /// mano, a diferencia de `Db::call` -- por eso se resuelve acá y se
    /// guarda, en vez de recalcularlo en cada `select`/`delete`). Vacío
    /// (sin entrada) es el caso normal -- la mayoría de colecciones no usa
    /// soft-delete.
    soft_delete_fields: HashMap<String, String>,
    /// GRAMMAR.md §3.178: `true` si la tabla interna de rate limiting
    /// distribuido (`RATE_LIMIT_TABLE`) está lista para usarse EN ESTE
    /// proceso -- siempre `false` en SQLite (nunca aplica), y en Postgres
    /// `true` salvo que `--adopt-existing` esté activo y la tabla no
    /// exista ya (adoptar nunca ejecuta DDL, ni siquiera para esta tabla
    /// propia) o la creación haya fallado por algún motivo (rol sin
    /// permiso de `CREATE TABLE`, por ejemplo -- degradado, nunca fatal:
    /// `check_rate_limit_distributed` devuelve `None` en ese caso, y el
    /// caller (`runtime/server.rs`) cae al `RateLimiter` en memoria de
    /// siempre, comportamiento IDÉNTICO al de antes de esta ronda).
    distributed_rate_limit: bool,
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
/// `schema` (GRAMMAR.md §3.193, `--db-schema`/`LINK_DATABASE_SCHEMA`) fija
/// `SET search_path` UNA vez, justo después de conectar -- mismo mecanismo
/// que `db_admin.rs::run_shell_postgres` ya usa para `default_transaction_read_only`
/// (`client.batch_execute`, sesión-level, nunca la URL: evita toda la
/// fragilidad de mezclar/escapar un query param `options=` a mano). `SET
/// search_path` a un schema que TODAVÍA no existe no es un error en Postgres
/// -- simplemente se salta al resolver nombres hasta que exista, así que
/// esto es seguro de llamar ANTES de que `Db::connect_postgres_with_options`
/// corra su propio `CREATE SCHEMA IF NOT EXISTS` (ver ahí). Esta función NO
/// crea nada -- eso es responsabilidad exclusiva del ÚNICO caller con
/// permiso de correr DDL.
pub(crate) fn connect_postgres_client(url: &str, schema: Option<&str>) -> Result<postgres::Client, String> {
    // rustls exige un crypto provider de proceso instalado ANTES del primer
    // `ClientConfig::builder()` -- se instala UNA vez; `install_default()`
    // devuelve `Err` si ya había uno (llamado desde acá Y desde un
    // reconnect posterior), así que el resultado se ignora a propósito.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let config: postgres::Config = url.parse().map_err(|e| format!("URL de conexión inválida: {e}"))?;
    let mut client = if config.get_ssl_mode() == postgres::config::SslMode::Disable {
        postgres::Client::connect(url, postgres::NoTls).map_err(|e| format!("no se pudo conectar a PostgreSQL: {e}"))?
    } else {
        let tls = tokio_postgres_rustls::MakeRustlsConnect::with_webpki_roots();
        postgres::Client::connect(url, tls).map_err(|e| format!("no se pudo conectar a PostgreSQL: {e}"))?
    };
    if let Some(schema) = schema {
        client
            .batch_execute(&format!("SET search_path TO \"{schema}\""))
            .map_err(|e| format!("no se pudo fijar search_path a '{schema}': {e}"))?;
    }
    Ok(client)
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
pub fn check_postgres_connectivity(url: &str, schema: Option<&str>) -> Result<(), String> {
    let mut client = connect_postgres_client(url, schema)?;
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
            // Sin `schema`: esta conexión solo hace LISTEN/lee NOTIFY sobre
            // un canal GLOBAL de nombre fijo (`REMOTE_CHANGE_CHANNEL`),
            // nunca referencia ninguna tabla -- `search_path` no aplica acá,
            // sin importar si `--db-schema` está configurado.
            let mut client = match connect_postgres_client(&url, None) {
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

    /// GRAMMAR.md §3.222: ver el campo `static_routes`.
    pub fn static_routes(&self) -> &[String] {
        &self.static_routes
    }

    /// GRAMMAR.md §3.223: registra UNA llamada `http.*` saliente. `status`
    /// es la clase (`2xx`/`3xx`/`4xx`/`5xx`) o `error` (sin respuesta HTTP:
    /// DNS, conexión rechazada, timeout) -- clase y no código exacto para
    /// que la cardinalidad de la serie sea acotada (un proveedor puede
    /// devolver decenas de códigos distintos; lo que un operador mira es
    /// "¿cuántos fallan y cuánto tardan?", no el histograma de códigos).
    pub fn record_outbound_http(&self, host: &str, status: &str, elapsed: std::time::Duration) {
        let mut map = self.outbound_http.lock();
        let entry = map.entry((host.to_string(), status.to_string())).or_insert((0, 0.0));
        entry.0 += 1;
        entry.1 += elapsed.as_secs_f64();
    }

    /// GRAMMAR.md §3.223: `(host, status, conteo, segundos)` ordenado, para
    /// que `/metrics` sea determinista entre scrapes.
    pub fn outbound_http_stats(&self) -> Vec<(String, String, u64, f64)> {
        let map = self.outbound_http.lock();
        let mut rows: Vec<(String, String, u64, f64)> =
            map.iter().map(|((host, status), (count, secs))| (host.clone(), status.clone(), *count, *secs)).collect();
        rows.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
        rows
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
        let type_checks_by_collection_map = type_checks_by_collection(program, &checker);
        let encrypted_by_collection = encrypted_fields_by_collection(program, &checker);
        let empty_checks: Vec<(String, FieldCheck)> = Vec::new();
        let empty_type_checks: Vec<String> = Vec::new();
        let empty_encrypted: HashSet<String> = HashSet::new();
        let mut columns = HashMap::new();
        let mut id_kinds = HashMap::new();
        for (name, element_ty) in checker.db_collections() {
            let Type::Struct { fields, .. } = element_ty else {
                unreachable!("Checker::validate_db_element_type ya garantizó que el elemento sea un struct");
            };
            let id_kind = IdKind::from_field_type(
                &fields.iter().find(|f| f.name == "id").expect("validate_db_element_type ya garantizó 'id'").ty,
            );
            let encrypted_fields = encrypted_by_collection.get(name).unwrap_or(&empty_encrypted);
            let cols: Vec<ColumnPlan> = fields
                .iter()
                .filter(|f| f.name != "id")
                .map(|f| ColumnPlan::for_field(f.clone(), &simple_enums, encrypted_fields.contains(&f.name)))
                .collect();
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
                let type_checks = type_checks_by_collection_map.get(name).unwrap_or(&empty_type_checks);
                connection
                    .execute(&create_table_sql(name, id_kind, &cols, checks, type_checks), [])
                    .unwrap_or_else(|e| panic!("no se pudo crear la tabla '{name}' en '{db_path_display}': {e}"));
                check_schema_matches(&connection, name, id_kind, &cols, &db_path_display).unwrap_or_else(|e| panic!("{e}"));
            }
            columns.insert(name.clone(), cols);
            id_kinds.insert(name.clone(), id_kind);
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
            id_kinds,
            subscribers: parking_lot::Mutex::new(HashMap::new()),
            pending_notify_retries: parking_lot::Mutex::new(std::collections::VecDeque::new()),
            oversized_notify_drops: parking_lot::Mutex::new(HashMap::new()),
            transaction_pending_publishes: parking_lot::Mutex::new(None),
            instance_id: random_instance_id(),
            argon2_params: parking_lot::RwLock::new(argon2::Params::default()),
            http_timeout: parking_lot::RwLock::new(DEFAULT_HTTP_TIMEOUT),
            encryption_key: parking_lot::RwLock::new(None),
            soft_delete_fields,
            static_routes: crate::route::static_public_routes(program),
            outbound_http: parking_lot::Mutex::new(HashMap::new()),
            // GRAMMAR.md §3.178: rate limiting distribuido es un concepto
            // exclusivamente Postgres -- SQLite nunca lo necesita, un solo
            // proceso ya tiene el estado exacto en memoria.
            distributed_rate_limit: false,
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
        schema: Option<&str>,
    ) -> Result<(Self, Receiver<RemoteChange>), String> {
        let (checker, symbol_errors) = Checker::build_symbols(program);
        if let Some(e) = symbol_errors.into_iter().next() {
            return Err(format!("programa inválido al abrir la base de datos: {e}"));
        }
        let simple_enums = simple_enum_names(program);

        let mut client = connect_postgres_client(url, schema)?;
        // GRAMMAR.md §3.193: el ÚNICO lugar de todo el proyecto que crea un
        // schema -- `connect_postgres_client` (arriba) solo fija
        // `search_path`, nunca DDL, así que sirve para todos los callers
        // de solo lectura (`db shell`/`inspect`/`export`/`introspect`) sin
        // ningún efecto secundario. `--adopt-existing` nunca ejecuta DDL,
        // ni siquiera esto -- si el schema no existe todavía, la
        // validación de adopción de más abajo falla con su mensaje normal
        // ("la colección no existe"), consistente con cómo trata una
        // TABLA faltante.
        if let (Some(schema), false) = (schema, adopt_existing) {
            client
                .batch_execute(&format!("CREATE SCHEMA IF NOT EXISTS \"{schema}\""))
                .map_err(|e| format!("no se pudo crear el schema '{schema}': {e}"))?;
        }
        let backend = Backend::Postgres {
            client: parking_lot::ReentrantMutex::new(std::cell::RefCell::new(client)),
            url: url.to_string(),
            schema: schema.map(str::to_string),
        };

        let checks_by_collection = check_fields_by_collection(program, &checker);
        let type_checks_by_collection_map = type_checks_by_collection(program, &checker);
        let encrypted_by_collection = encrypted_fields_by_collection(program, &checker);
        let empty_checks: Vec<(String, FieldCheck)> = Vec::new();
        let empty_type_checks: Vec<String> = Vec::new();
        let empty_encrypted: HashSet<String> = HashSet::new();
        let mut columns = HashMap::new();
        let mut id_kinds = HashMap::new();
        for (name, element_ty) in checker.db_collections() {
            let Type::Struct { fields, .. } = element_ty else {
                unreachable!("Checker::validate_db_element_type ya garantizó que el elemento sea un struct");
            };
            let id_field_ty = &fields.iter().find(|f| f.name == "id").expect("validate_db_element_type ya garantizó 'id'").ty;
            let id_kind = IdKind::from_field_type(id_field_ty);
            let encrypted_fields = encrypted_by_collection.get(name).unwrap_or(&empty_encrypted);
            let cols: Vec<ColumnPlan> = fields
                .iter()
                .filter(|f| f.name != "id")
                .map(|f| ColumnPlan::for_field(f.clone(), &simple_enums, encrypted_fields.contains(&f.name)))
                .collect();
            let non_id: Vec<FieldType> = cols.iter().map(|c| c.field.clone()).collect();

            if !adopt_existing {
                // El DDL sale del MISMO generador que usa `linkc build` para
                // emitir schema.pg.sql. Si el runtime creara las tablas por su
                // cuenta, el esquema que el proyecto documenta y el que la base
                // realmente tiene podrían divergir -- que es la clase de bug que
                // este repo ya encontró varias veces (GRAMMAR.md §3.9).
                let checks = checks_by_collection.get(name).unwrap_or(&empty_checks);
                let type_checks = type_checks_by_collection_map.get(name).unwrap_or(&empty_type_checks);
                backend
                    .execute_ddl(&crate::codegen::postgres_emit::create_postgres_table_sql(
                        name,
                        id_field_ty,
                        &non_id,
                        &simple_enums,
                        checks,
                        type_checks,
                    ))
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
            validate_existing_id_column(&backend, name, id_kind)?;

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
            id_kinds.insert(name.clone(), id_kind);
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

        // GRAMMAR.md §3.178: rate limiting distribuido -- tabla interna
        // compartida por TODAS las instancias contra la MISMA base.
        // `--adopt-existing` nunca ejecuta DDL, ni siquiera para esta
        // tabla propia (mismo criterio que cualquier colección
        // declarada) -- si ya existe (un operador la creó a mano, o una
        // instancia anterior sin --adopt-existing ya la creó), se usa
        // igual; si no, esta instancia cae al `RateLimiter` en memoria de
        // siempre. Fuera de ese modo, se intenta crear -- un fallo acá
        // (rol sin permiso de CREATE TABLE, poco común pero posible) NO
        // aborta el arranque del servidor: solo esta pieza se degrada,
        // con un aviso, nunca un servidor que no arranca por una tabla
        // que ni siquiera es del usuario.
        let distributed_rate_limit = if adopt_existing {
            postgres_table_exists(&backend, RATE_LIMIT_TABLE).unwrap_or(false)
        } else {
            match backend.execute_ddl(&create_rate_limit_table_sql()) {
                Ok(()) => true,
                Err(e) => {
                    eprintln!(
                        "advertencia: no se pudo crear la tabla interna de rate limiting distribuido ({e}) -- \
                         esta instancia usa el limitador en memoria de siempre (GRAMMAR.md §3.178)"
                    );
                    false
                }
            }
        };

        Ok((
            Db {
                backend,
                checker,
                simple_enums,
                columns,
                id_kinds,
                subscribers: parking_lot::Mutex::new(HashMap::new()),
                pending_notify_retries: parking_lot::Mutex::new(std::collections::VecDeque::new()),
                oversized_notify_drops: parking_lot::Mutex::new(HashMap::new()),
                transaction_pending_publishes: parking_lot::Mutex::new(None),
                instance_id,
                argon2_params: parking_lot::RwLock::new(argon2::Params::default()),
                http_timeout: parking_lot::RwLock::new(DEFAULT_HTTP_TIMEOUT),
                encryption_key: parking_lot::RwLock::new(None),
                soft_delete_fields,
                static_routes: crate::route::static_public_routes(program),
                outbound_http: parking_lot::Mutex::new(HashMap::new()),
                distributed_rate_limit,
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
    pub fn connect_postgres_for_testing(program: &Program, url: &str, adopt_existing: bool, schema: Option<&str>) -> Result<Self, String> {
        Self::connect_postgres_with_options(program, url, adopt_existing, schema).map(|(db, _remote_rx)| db)
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

    /// Fija la clave de `@encrypted` para lo que quede de vida del proceso
    /// (GRAMMAR.md §3.191) -- mismo criterio que `set_argon2_params`/
    /// `set_http_timeout`: `server.rs` lo llama UNA sola vez, antes de
    /// aceptar la primera request, después de confirmar (si hace falta) que
    /// hay una clave real configurada.
    pub(crate) fn set_encryption_key(&self, key: Option<[u8; encryption::KEY_LEN]>) {
        *self.encryption_key.write() = key;
    }

    /// La clave configurada, si hay -- la leen `write_param`/`decode_row`
    /// en cada campo `ColumnPlan::encrypted`. `[u8; 32]` es `Copy`, así que
    /// esto no necesita clonar nada más que el propio array.
    pub(crate) fn encryption_key(&self) -> Option<[u8; encryption::KEY_LEN]> {
        *self.encryption_key.read()
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

    /// GRAMMAR.md §3.178: rate limit DISTRIBUIDO vía la tabla interna
    /// `RATE_LIMIT_TABLE` -- `None` si no está disponible en este proceso
    /// (`self.distributed_rate_limit == false`: SQLite, o Postgres con
    /// `--adopt-existing` sin la tabla ya creada a mano, o la creación
    /// falló al conectar), momento en el que el caller
    /// (`runtime/server.rs`) cae al `RateLimiter` en memoria de siempre --
    /// comportamiento IDÉNTICO al de antes de esta ronda. Mismo algoritmo
    /// EXACTO que `rate_limit::RateLimiter` (token bucket, refill
    /// continuo, nunca ventanas fijas) -- la única diferencia es que el
    /// estado vive en una fila de Postgres compartida por todas las
    /// instancias, en vez de un `HashMap` propio de cada proceso.
    ///
    /// Un solo UPSERT atómico -- mismo criterio que `increment()`
    /// (`UPDATE ... SET col = col + ?`, nunca leer-y-después-escribir en
    /// dos pasos separados que puedan carrerear bajo concurrencia real
    /// entre procesos distintos): el refill/consumo se calcula DENTRO del
    /// propio `SET`, referenciando `"{RATE_LIMIT_TABLE}".tokens`/
    /// `.last_seen_ms` -- los valores REALES de la fila ya bloqueada por
    /// el propio UPSERT en el momento de escribir, nunca un valor leído
    /// por separado antes (que sí podría quedar desactualizado si otra
    /// instancia escribe en el medio). La cláusula `WHERE` sobre la
    /// acción `DO UPDATE` (sintaxis real de Postgres, no una comparación
    /// aparte) hace que la fila NO se toque en absoluto si no hay
    /// suficientes tokens -- ni siquiera `last_seen_ms` avanza, así que el
    /// próximo intento sigue viendo el reloj real transcurrido y el
    /// refill se sigue acumulando correctamente sin este paso.
    /// `capacity`/`refill_per_sec` se reescriben en cada check exitoso
    /// para que un redeploy con un `@rate_limit(...)` distinto converja
    /// solo, sin necesitar limpiar la tabla a mano.
    pub fn check_rate_limit_distributed(&self, client_identity: &str, service: &str, rpc: &str, spec: RateLimitSpec) -> Option<bool> {
        if !self.distributed_rate_limit {
            return None;
        }
        let bucket_key = format!("{client_identity}|{service}|{rpc}");
        let capacity = spec.count as f64;
        let refill_per_sec = capacity / spec.window.as_secs_f64();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        // GRAMMAR.md §3.178, bug real encontrado en CI: `$2 - 1` con el
        // literal entero `1` sin tipo hacía que Postgres infiriera `$2`
        // como `integer`, no `double precision` -- la PRIMERA aparición
        // de un parámetro es la que fija su tipo para TODA la sentencia,
        // y encontrarlo después en un contexto `DOUBLE PRECISION` (la
        // columna `capacity`) no lo corrige, solo inserta un cast
        // implícito ahí -- el propio driver seguía mandando 8 bytes de
        // `Cell::Float` (formato binario de `float8`) contra un
        // parámetro que el servidor esperaba como 4 bytes de `int4`:
        // "incorrect binary data format in bind parameter 2". El cast
        // explícito (`$N::double precision`/`$N::bigint`) en CADA
        // aparición fija el tipo sin ambigüedad, sin depender de en qué
        // orden Postgres visite las distintas apariciones.
        let sql = format!(
            "INSERT INTO \"{RATE_LIMIT_TABLE}\" (\"bucket_key\", \"tokens\", \"capacity\", \"refill_per_sec\", \"last_seen_ms\") \
             VALUES ($1, $2::double precision - 1, $2::double precision, $3::double precision, $4::bigint) \
             ON CONFLICT (\"bucket_key\") DO UPDATE SET \
                \"tokens\" = LEAST($2::double precision, \"{RATE_LIMIT_TABLE}\".\"tokens\" + GREATEST(0, $4::bigint - \"{RATE_LIMIT_TABLE}\".\"last_seen_ms\")::double precision / 1000.0 * \"{RATE_LIMIT_TABLE}\".\"refill_per_sec\") - 1, \
                \"capacity\" = $2::double precision, \
                \"refill_per_sec\" = $3::double precision, \
                \"last_seen_ms\" = $4::bigint \
             WHERE LEAST($2::double precision, \"{RATE_LIMIT_TABLE}\".\"tokens\" + GREATEST(0, $4::bigint - \"{RATE_LIMIT_TABLE}\".\"last_seen_ms\")::double precision / 1000.0 * \"{RATE_LIMIT_TABLE}\".\"refill_per_sec\") >= 1.0 \
             RETURNING \"tokens\""
        );
        let params = vec![Cell::Text(bucket_key), Cell::Float(capacity), Cell::Float(refill_per_sec), Cell::Int(now_ms)];
        // Cualquier error (transitorio o no) degrada a `None` -- nunca deja
        // una request colgada ni la rechaza por un problema de infra que no
        // es culpa suya. `Backend::query` ya reintenta una conexión caída
        // por su cuenta (`with_reconnect`, GRAMMAR.md §3.40) antes de
        // llegar hasta acá.
        match self.backend.query(&sql, &params, &[ColumnKind::Float]) {
            Ok(rows) => Some(!rows.is_empty()),
            Err(e) => {
                // Visible, no silenciosa -- degradarse al limitador en
                // memoria sin ningún rastro sería exactamente el tipo de
                // landmine que GRAMMAR.md ya viene documentando (§3.153):
                // un límite MÁS DÉBIL de lo prometido, sin ningún error
                // que lo señale hasta que alguien lo note en producción.
                eprintln!("advertencia: rate limit distribuido falló ({e}) -- esta request usó el limitador en memoria de este proceso");
                None
            }
        }
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
                let id_value = args.into_iter().next().ok_or_else(|| RuntimeError::new("find requiere 1 argumento"))?;
                let (id_cell, _) = self.id_cell_and_display(&id_value)?;
                Ok(self.select_rows(collection, columns, Some(id_cell))?.into_iter().next().unwrap_or(Value::Null))
            }
            "insert" => {
                let v = args.into_iter().next().ok_or_else(|| RuntimeError::new("insert requiere 1 argumento"))?;
                let Value::Struct(fields) = &v else {
                    return Err(RuntimeError::new("insert: el valor debe ser un struct"));
                };
                let mut col_names = Vec::with_capacity(columns.len() + 1);
                let mut params: Vec<Cell> = Vec::with_capacity(columns.len() + 1);
                // GRAMMAR.md §3.177: una PK `Uuid` se genera del lado de la
                // APLICACIÓN, ANTES del INSERT -- mismo generador que
                // `crypto.uuid()` (`generate_uuid_v4`, runtime/mod.rs) --
                // y se manda como valor EXPLÍCITO, nunca por DEFAULT ni
                // RETURNING. El caller nunca trae "id" en `fields`
                // (`Omit<T,"id">`, checker.rs::omit_id_field), así que no
                // hay riesgo de pisar un valor que el usuario haya
                // intentado fijar.
                let generated_uuid = match self.id_kind(collection) {
                    IdKind::Int => None,
                    IdKind::Uuid => Some(generate_uuid_v4()?),
                };
                if let Some(uuid) = &generated_uuid {
                    col_names.push("\"id\"".to_string());
                    params.push(Cell::Text(uuid.clone()));
                }
                for col in columns {
                    let slot = fields.iter().find(|(n, _)| n == &col.field.name).map(|(_, v)| v);
                    col_names.push(format!("\"{}\"", col.field.name));
                    params.push(self.write_param(col, slot)?);
                }
                let sql = if col_names.is_empty() {
                    format!("INSERT INTO \"{collection}\" DEFAULT VALUES")
                } else {
                    let placeholders: Vec<String> = (1..=col_names.len()).map(|n| self.backend.placeholder(n)).collect();
                    format!("INSERT INTO \"{collection}\" ({}) VALUES ({})", col_names.join(", "), placeholders.join(", "))
                };
                let (id_cell, id_display) = match generated_uuid {
                    Some(uuid) => {
                        self.backend.execute(&sql, &params).map_err(|e| write_error("insert", e))?;
                        (Cell::Text(uuid.clone()), uuid)
                    }
                    None => {
                        let new_id = self.backend.insert_returning_id(&sql, &params).map_err(|e| write_error("insert", e))?;
                        (Cell::Int(new_id), new_id.to_string())
                    }
                };
                // AUDIT-2026-08-27.md #5: el INSERT y este SELECT de
                // confirmación son dos llamadas independientes al backend
                // (cada una toma y suelta su propio candado, salvo que este
                // `insert` corra dentro de `with_exclusive_connection` --
                // `transaction{}`/`upsert`) -- hay una ventana real donde
                // otro hilo (un `deleteWhere` concurrente cuyo predicado
                // matchea la fila recién insertada, por ejemplo por un valor
                // de campo por defecto) puede borrarla antes de este SELECT.
                // `.expect(...)` panicaba acá en vez de dar el mismo
                // `RuntimeError` que `applyPatch` (unas líneas más abajo) ya
                // usa para la carrera IDÉNTICA -- asimetría sin motivo entre
                // dos funciones que reconsultan por id después de escribir.
                let inserted = self
                    .select_rows(collection, columns, Some(id_cell))?
                    .into_iter()
                    .next()
                    .ok_or_else(|| RuntimeError::new(format!("no hay ningún elemento con id {id_display} en '{collection}'")))?;
                self.publish(collection, &inserted);
                Ok(inserted)
            }
            "applyPatch" => {
                let mut it = args.into_iter();
                let id_value = it.next().ok_or_else(|| RuntimeError::new("applyPatch requiere 2 argumentos"))?;
                let (id_cell, id_display) = self.id_cell_and_display(&id_value)?;
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
                    params.push(self.write_param(col, Some(value))?);
                }
                if !set_clauses.is_empty() {
                    let id_placeholder = self.backend.placeholder(params.len() + 1);
                    params.push(id_cell.clone());
                    let sql = format!("UPDATE \"{collection}\" SET {} WHERE \"id\" = {id_placeholder}", set_clauses.join(", "));
                    self.backend.execute(&sql, &params).map_err(|e| write_error("applyPatch", e))?;
                }
                // Reconsultar por id, tanto si hubo UPDATE como si el patch
                // no traía ningún campo escribible -- "no encontrado" en
                // esta consulta es la única señal de "no existe", cubre los
                // dos casos con un solo camino.
                let updated = self
                    .select_rows(collection, columns, Some(id_cell))?
                    .into_iter()
                    .next()
                    .ok_or_else(|| RuntimeError::new(format!("no hay ningún elemento con id {id_display} en '{collection}'")))?;
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
                let id_value = args.into_iter().next().ok_or_else(|| RuntimeError::new("delete requiere 1 argumento"))?;
                let (id_cell, _) = self.id_cell_and_display(&id_value)?;
                // `select_rows(id: Some(_))` NUNCA filtra por soft-delete
                // (ver su propio comentario) -- acá es exactamente lo que
                // hace falta: encontrar la fila sea cual sea su estado, para
                // saber si hay algo que borrar y qué publicar si se borra.
                let existing = self.select_rows(collection, columns, Some(id_cell.clone()))?.into_iter().next();
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
                            .execute(&sql, &[Cell::Int(now_ms), id_cell])
                            .map_err(|e| RuntimeError::new(format!("delete (soft) falló: {e}")))?
                    }
                    None => {
                        let sql = format!("DELETE FROM \"{collection}\" WHERE \"id\" = {}", self.backend.placeholder(1));
                        self.backend
                            .execute(&sql, &[id_cell])
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
            // (`recognize_pushable_predicate`) necesita el `Env` capturado
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

    /// GRAMMAR.md §3.177: tipo de la PK de `collection` -- `IdKind::Int`
    /// si no está en el mapa (nunca pasa en la práctica, ver el comentario
    /// del campo `id_kinds`).
    fn id_kind(&self, collection: &str) -> IdKind {
        self.id_kinds.get(collection).copied().unwrap_or(IdKind::Int)
    }

    /// El `ColumnKind` con el que hay que DECODIFICAR la columna `"id"` al
    /// leerla -- `Int` o `Text`, según `id_kind`. Mismo `ColumnKind::Text`
    /// que ya usa cualquier otro campo `Uuid` (`ColumnPlan::kind`).
    fn id_column_kind(&self, collection: &str) -> ColumnKind {
        id_column_kind_for(self.id_kind(collection))
    }

    /// El `Cell` para bindear/comparar en SQL, más su representación en
    /// texto para un mensaje de error -- a partir del `Value` de id que el
    /// checker ya validó (`Int` o `Uuid`, según `check_db_method`). Evita
    /// repetir el mismo match en `find`/`applyPatch`/`delete`/`increment`/
    /// `insert`.
    fn id_cell_and_display(&self, v: &Value) -> Result<(Cell, String), RuntimeError> {
        match v {
            Value::Int(n) => Ok((Cell::Int(*n), n.to_string())),
            Value::Uuid(s) => Ok((Cell::Text(s.clone()), s.clone())),
            other => Err(RuntimeError::new(format!("id inválido: se esperaba Int o Uuid, se encontró {other:?}"))),
        }
    }

    /// Inserta una fila con un id EXPLÍCITO -- nunca alcanzable desde el
    /// lenguaje `.link` ni desde el intérprete (GRAMMAR.md §3.185): la
    /// rama `"insert"` de `Db::call`, arriba, SIEMPRE autogenera (Int por
    /// autoincremento del motor, Uuid del lado de la app) -- `omit_id_field`
    /// (checker.rs) garantiza que ningún caller de `.link` pueda siquiera
    /// intentar mandar un `id`. Solo `linkc db import` llama a esto,
    /// directo desde Rust. Mismo armado de `col_names`/`params` que la rama
    /// `"insert"` (reusa `write_param`/`Cell`/`placeholder` tal cual), con
    /// dos diferencias: el id SIEMPRE viene del caller (nunca generado), y
    /// no hay `RETURNING`/`last_insert_rowid()` que pedir -- ya se sabe.
    /// A propósito SIN re-SELECT de confirmación (un import mueve miles de
    /// filas, no una -- duplicar cada INSERT con un SELECT no tiene
    /// beneficio acá) ni `publish` a suscriptores (un proceso `linkc db
    /// import` de una sola corrida nunca tiene ningún `stream` conectado).
    pub(crate) fn import_row(&self, collection: &str, id: &Value, fields: &[(String, Value)]) -> Result<(), RuntimeError> {
        let columns = self
            .columns
            .get(collection)
            .ok_or_else(|| RuntimeError::new(format!("colección desconocida: '{collection}'")))?;
        let (id_cell, id_display) = self.id_cell_and_display(id)?;
        let mut col_names = vec!["\"id\"".to_string()];
        let mut params: Vec<Cell> = vec![id_cell];
        for col in columns {
            let slot = fields.iter().find(|(n, _)| n == &col.field.name).map(|(_, v)| v);
            col_names.push(format!("\"{}\"", col.field.name));
            params.push(self.write_param(col, slot)?);
        }
        let placeholders: Vec<String> = (1..=col_names.len()).map(|n| self.backend.placeholder(n)).collect();
        let sql = format!("INSERT INTO \"{collection}\" ({}) VALUES ({})", col_names.join(", "), placeholders.join(", "));
        self.backend
            .execute(&sql, &params)
            .map_err(|e| RuntimeError::new(format!("import: '{collection}' id={id_display}: {e}")))?;
        Ok(())
    }

    /// Resincroniza la secuencia de autoincremento de `collection` DESPUÉS
    /// de un `import_row` con ids explícitos -- sin esto, un `insert()`
    /// normal posterior podría chocar con un id importado (GRAMMAR.md
    /// §3.185: ni SQLite ni Postgres avanzan su generador de ids solos
    /// ante un INSERT que trae el id a mano). Autocorrectivo: lee el
    /// `MAX("id")` FÍSICO de la tabla, nunca confía en lo que el caller
    /// cree haber importado -- así que es seguro llamarlo aunque algunas
    /// filas del import hayan fallado antes de llegar acá (nunca pasa hoy,
    /// `run_import` aborta todo el proceso ante el primer error, pero esta
    /// función queda correcta igual si el caller cambiara ese criterio).
    pub(crate) fn resync_id_sequence(&self, collection: &str) -> Result<(), RuntimeError> {
        if self.id_kind(collection) != IdKind::Int {
            return Ok(()); // una PK Uuid no tiene ningún concepto de secuencia
        }
        let rows = self
            .backend
            .query(&format!("SELECT MAX(\"id\") FROM \"{collection}\""), &[], &[ColumnKind::Int])
            .map_err(|e| RuntimeError::new(format!("resync de secuencia de '{collection}' falló: {e}")))?;
        let Some(Cell::Int(max_id)) = rows.first().and_then(|r| r.first()) else {
            return Ok(()); // tabla vacía -- MAX(id) es NULL, nada que resincronizar
        };
        match &self.backend {
            Backend::Sqlite(_) => {
                // `sqlite_sequence` solo existe para columnas `INTEGER
                // PRIMARY KEY AUTOINCREMENT` (exactamente lo que
                // `create_table_sql` genera para `IdKind::Int`) -- guarda
                // el ÚLTIMO valor autoincremental usado, y SQLite lo
                // respeta incluso tras un DELETE (a diferencia de
                // `INTEGER PRIMARY KEY` sin `AUTOINCREMENT`, que reusaría
                // ids libremente). `UPDATE` si ya hay fila para esta tabla
                // (nunca la BAJA -- `seq < max_id` como guarda), `INSERT`
                // si no hay fila todavía (el caso común: una tabla recién
                // creada por `import` nunca hizo un insert autoincremental
                // antes, así que `sqlite_sequence` no la conoce todavía).
                let existing = self
                    .backend
                    .query("SELECT seq FROM sqlite_sequence WHERE name = ?", &[Cell::Text(collection.to_string())], &[
                        ColumnKind::Int,
                    ])
                    .map_err(|e| RuntimeError::new(format!("resync de secuencia de '{collection}' falló: {e}")))?;
                match existing.first().and_then(|r| r.first()) {
                    Some(Cell::Int(seq)) if *seq >= *max_id => {}
                    Some(_) => {
                        self.backend
                            .execute("UPDATE sqlite_sequence SET seq = ? WHERE name = ?", &[
                                Cell::Int(*max_id),
                                Cell::Text(collection.to_string()),
                            ])
                            .map_err(|e| RuntimeError::new(format!("resync de secuencia de '{collection}' falló: {e}")))?;
                    }
                    None => {
                        self.backend
                            .execute("INSERT INTO sqlite_sequence (name, seq) VALUES (?, ?)", &[
                                Cell::Text(collection.to_string()),
                                Cell::Int(*max_id),
                            ])
                            .map_err(|e| RuntimeError::new(format!("resync de secuencia de '{collection}' falló: {e}")))?;
                    }
                }
            }
            Backend::Postgres { .. } => {
                // `pg_get_serial_sequence` en vez de hardcodear
                // `"<tabla>_id_seq"` (el nombre por default de una columna
                // `BIGSERIAL`, `postgres_emit.rs`) -- la forma oficial y a
                // prueba de quoting de resolver la secuencia real de una
                // columna serial.
                self.backend
                    .execute("SELECT setval(pg_get_serial_sequence($1, 'id'), $2)", &[
                        Cell::Text(format!("\"{collection}\"")),
                        Cell::Int(*max_id),
                    ])
                    .map_err(|e| RuntimeError::new(format!("resync de secuencia de '{collection}' falló: {e}")))?;
            }
        }
        Ok(())
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
    fn select_rows(&self, collection: &str, columns: &[ColumnPlan], id: Option<Cell>) -> Result<Vec<Value>, RuntimeError> {
        let mut col_list = vec!["\"id\"".to_string()];
        col_list.extend(columns.iter().map(|c| format!("\"{}\"", c.field.name)));
        let sql = match id {
            Some(_) => {
                format!("SELECT {} FROM \"{collection}\" WHERE \"id\" = {}", col_list.join(", "), self.backend.placeholder(1))
            }
            None => match self.soft_delete_where(collection) {
                Some(cond) => format!("SELECT {} FROM \"{collection}\" WHERE {cond} ORDER BY \"id\"", col_list.join(", ")),
                None => format!("SELECT {} FROM \"{collection}\" ORDER BY \"id\"", col_list.join(", ")),
            },
        };
        // El orden de `kinds` es el del SELECT: "id" primero, después las
        // columnas declaradas, en el mismo orden que `columns`.
        let mut kinds = vec![self.id_column_kind(collection)];
        kinds.extend(columns.iter().map(ColumnPlan::kind));
        let params: Vec<Cell> = id.map(|c| vec![c]).unwrap_or_default();

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
    /// Una sola hoja `campo OP valor` -- misma lógica NULL-segura y de
    /// validación de columna que antes, extraída para que `condition_expr_sql`
    /// (el recorrido recursivo del árbol And/Or, GRAMMAR.md §3.170) la
    /// reuse en cualquier profundidad. Empuja directo a `cells` (en vez de
    /// devolver un Vec propio) para que la numeración de placeholders
    /// posicionales (`$1`, `$2`, ... en Postgres) quede correcta sin
    /// importar en qué rama del árbol cae cada hoja -- el orden de
    /// aparición en `cells` tiene que coincidir con el orden en que cada
    /// placeholder aparece en el SQL final, y como el recorrido es
    /// izquierda-a-derecha en el mismo orden en que se arma el string,
    /// `cells.len() + 1` en el momento de cada push ya da el número correcto.
    fn leaf_condition_sql(&self, columns: &[ColumnPlan], field: &str, op: BinaryOp, value: &Value, cells: &mut Vec<Cell>) -> Option<String> {
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
            if field != "id" && columns.iter().find(|c| c.field.name == field).is_none_or(|c| c.json) {
                return None;
            }
            return Some(format!("\"{field}\" {null_op}"));
        }
        let sql_op = match op {
            BinaryOp::Eq => "=",
            BinaryOp::NotEq => "!=",
            BinaryOp::Lt => "<",
            BinaryOp::LtEq => "<=",
            BinaryOp::Gt => ">",
            BinaryOp::GtEq => ">=",
            // `ast::recognize_predicate_expr` ya filtra a estos seis --
            // cualquier otro operador nunca llega hasta acá.
            _ => return None,
        };
        let cell = if field == "id" {
            let Value::Int(id) = value else { return None };
            Cell::Int(*id)
        } else {
            let col = columns.iter().find(|c| c.field.name == field)?;
            // `col.encrypted` (GRAMMAR.md §3.191): el ciphertext es distinto
            // en cada escritura (nonce aleatorio) -- comparar contra el
            // VALOR de un parámetro pusheado a SQL nunca podría matchear la
            // fila correcta, así que esto cae al camino interpretado de
            // siempre (`select_where_conjunction`/`select_rows` completo +
            // filtrado en memoria, donde `col.encrypted` SÍ descifra antes
            // de comparar) -- mismo criterio que `col.json` ya usa acá.
            if col.json || col.encrypted {
                return None;
            }
            self.write_param(col, Some(value)).ok()?
        };
        let clause = format!("\"{field}\" {sql_op} {}", self.backend.placeholder(cells.len() + 1));
        cells.push(cell);
        Some(clause)
    }

    /// `item.campoA OP item.campoB` (GRAMMAR.md §3.171) -- comparación entre
    /// DOS columnas de la misma fila, sin ningún valor que bindear (a
    /// diferencia de `leaf_condition_sql`, que siempre liga un placeholder).
    /// Solo los cuatro operadores relacionales llegan hasta acá
    /// (`ast::recognize_predicate_tree` ya filtra `==`/`!=` antes). Sin caso
    /// NULL-seguro que manejar: el checker (checker.rs::synth_binary, brazo
    /// `Lt | LtEq | Gt | GtEq`) solo tipa esta forma cuando ambos lados son
    /// Int/Int64/Float/Timestamp SIN envolver en `Optional`, y un campo no
    /// opcional siempre es `NOT NULL` en la columna real (postgres_emit.rs)
    /// -- para una tabla creada por c-script, ninguna de las dos columnas
    /// puede contener NULL nunca. La única excepción teórica es
    /// `--adopt-existing` sobre datos preexistentes que ya violaban esa
    /// invariante antes de que c-script tocara la tabla -- fuera de alcance
    /// acá, documentado como límite honesto en GRAMMAR.md §3.171 (el camino
    /// interpretado tampoco está libre de sorpresas en ese escenario:
    /// `row_to_fields` ya falla con un error limpio, no un panic, si
    /// encuentra un NULL inesperado en una columna no opcional).
    fn field_pair_condition_sql(&self, columns: &[ColumnPlan], left_field: &str, op: BinaryOp, right_field: &str) -> Option<String> {
        let sql_op = match op {
            BinaryOp::Lt => "<",
            BinaryOp::LtEq => "<=",
            BinaryOp::Gt => ">",
            BinaryOp::GtEq => ">=",
            // `ast::recognize_predicate_tree` ya filtra a estos cuatro --
            // cualquier otro operador nunca llega hasta acá.
            _ => return None,
        };
        let pushable = |f: &str| f == "id" || columns.iter().any(|c| c.field.name == f && !c.json);
        if !pushable(left_field) || !pushable(right_field) {
            return None;
        }
        Some(format!("\"{left_field}\" {sql_op} \"{right_field}\""))
    }

    /// Recorrido recursivo de un `ConditionExpr` (GRAMMAR.md §3.170) a una
    /// cláusula SQL -- `And`/`Or` se traducen a `AND`/`OR` reales, cada
    /// hijo compuesto que sea del tipo CONTRARIO al del padre (un `Or`
    /// adentro de un `And`, o viceversa) se parentiza para preservar la
    /// precedencia real (`a AND (b OR c)` nunca puede escribirse como `a
    /// AND b OR c`, que SQL parsearía como `(a AND b) OR c`). Un hijo del
    /// MISMO tipo que el padre no puede aparecer -- `ast::merge_and`/
    /// `merge_or` ya aplanan esos casos al construir el árbol, así que no
    /// hace falta ese chequeo acá.
    fn condition_expr_sql(&self, columns: &[ColumnPlan], expr: &ConditionExpr, cells: &mut Vec<Cell>) -> Option<String> {
        match expr {
            ConditionExpr::Leaf(field, op, value) => self.leaf_condition_sql(columns, field, *op, value, cells),
            ConditionExpr::FieldPair(left_field, op, right_field) => self.field_pair_condition_sql(columns, left_field, *op, right_field),
            ConditionExpr::And(items) => {
                let mut clauses = Vec::with_capacity(items.len());
                for item in items {
                    let clause = self.condition_expr_sql(columns, item, cells)?;
                    clauses.push(if matches!(item, ConditionExpr::Or(_)) { format!("({clause})") } else { clause });
                }
                Some(clauses.join(" AND "))
            }
            ConditionExpr::Or(items) => {
                let mut clauses = Vec::with_capacity(items.len());
                for item in items {
                    let clause = self.condition_expr_sql(columns, item, cells)?;
                    clauses.push(if matches!(item, ConditionExpr::And(_)) { format!("({clause})") } else { clause });
                }
                Some(clauses.join(" OR "))
            }
        }
    }

    /// Cells bindeables + condición SQL completa (con soft-delete AND-eado
    /// al final si corresponde) para el árbol que `countWhere`/`findWhere`/
    /// `upsert` empujan a SQL (GRAMMAR.md §3.95, `==` v1.59.0; §3.108, los
    /// otros cinco operadores relacionales; §3.109, una conjunción `&&` de
    /// varias condiciones; §3.170, `||` combinándolas). Compartido entre
    /// `count_where_conjunction` y `find_where_conjunction` -- la única
    /// diferencia entre esos dos es qué `SELECT` arman con esta misma
    /// condición.
    fn condition_sql(&self, collection: &str, columns: &[ColumnPlan], expr: &ConditionExpr) -> Option<(String, Vec<Cell>)> {
        let mut cells = Vec::new();
        let cond = self.condition_expr_sql(columns, expr, &mut cells)?;
        // El `cond` entero se parentiza acá si el árbol es un `Or` de nivel
        // superior -- sin esto, "a OR b AND soft_delete_is_null" parsearía
        // como "a OR (b AND soft_delete_is_null)", perdiendo el filtro de
        // soft-delete sobre la mitad "a" de la disyunción.
        let where_clause = match self.soft_delete_where(collection) {
            Some(sd) => {
                let wrapped = if matches!(expr, ConditionExpr::Or(_)) { format!("({cond})") } else { cond };
                format!("{wrapped} AND {sd}")
            }
            None => cond,
        };
        Some((where_clause, cells))
    }

    /// `db.<c>.countWhere(|x| ...)` (GRAMMAR.md §3.95/§3.108/§3.109/§3.170):
    /// un `SELECT COUNT(*) ... WHERE` real -- CERO filas viajan del motor
    /// al proceso, a diferencia del `countWhere` interpretado (traer TODO
    /// con `all`, evaluar el predicado fila por fila en Rust, contar).
    /// `None` (nunca un error) si el predicado no tiene esta forma exacta,
    /// o algún campo no es pusheable -- el caller (`runtime/mod.rs`) cae al
    /// camino interpretado, que sigue siendo correcto siempre, solo más
    /// lento en ese caso.
    pub(crate) fn count_where_conjunction(&self, collection: &str, conditions: &ConditionExpr) -> Result<Option<i64>, RuntimeError> {
        let columns = self.columns.get(collection).ok_or_else(|| RuntimeError::new(format!("colección desconocida: '{collection}'")))?;
        let Some((where_clause, cells)) = self.condition_sql(collection, columns, conditions) else {
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

    /// Como `count_where_conjunction`, para `db.<c>.findWhere(|x| ...)` --
    /// un `SELECT ... WHERE` real, solo las filas que matchean viajan del
    /// motor al proceso (a diferencia del camino interpretado, que trae
    /// TODA la colección y filtra en Rust). Mismo criterio de `None` que
    /// `count_where_conjunction`.
    pub(crate) fn find_where_conjunction(&self, collection: &str, conditions: &ConditionExpr) -> Result<Option<Vec<Value>>, RuntimeError> {
        let columns = self.columns.get(collection).ok_or_else(|| RuntimeError::new(format!("colección desconocida: '{collection}'")))?;
        let Some((where_clause, cells)) = self.condition_sql(collection, columns, conditions) else {
            return Ok(None);
        };
        let mut col_list = vec!["\"id\"".to_string()];
        col_list.extend(columns.iter().map(|c| format!("\"{}\"", c.field.name)));
        let sql = format!("SELECT {} FROM \"{collection}\" WHERE {where_clause} ORDER BY \"id\"", col_list.join(", "));
        let mut kinds = vec![self.id_column_kind(collection)];
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
        let mut kinds = vec![self.id_column_kind(collection)];
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
                // GRAMMAR.md §3.184: SUM/MAX/MIN sobre una columna Decimal
                // -- `avgBy` nunca llega acá (checker.rs ya lo rechaza para
                // Decimal, ver check_aggregate_by). NUMERIC es un no-op
                // real en los dos backends para este caso: Postgres ya
                // devuelve `numeric` de por sí (SUM/MAX/MIN sobre numeric
                // no promociona a otro tipo, a diferencia de Int→numeric);
                // SQLite conserva la afinidad INTEGER del valor YA escalado
                // que la columna guarda (mismo mecanismo que
                // `ColumnKind::Decimal` en `Cell::to_sql`/`sqlite_cell`).
                ColumnKind::Decimal => "NUMERIC",
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
        rows.iter()
            .map(|cells| {
                Ok(Value::Struct(vec![
                    ("key".to_string(), scalar_cell_to_value(collection, &key_ty, &cells[0])?),
                    ("value".to_string(), scalar_cell_to_value(collection, &value_ty, &cells[1])?),
                ]))
            })
            .collect()
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
        let mut kinds = vec![self.id_column_kind(collection)];
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
        let id_value = it.next().ok_or_else(|| RuntimeError::new("increment requiere 3 argumentos (id, selector, delta)"))?;
        let (id_cell, id_display) = self.id_cell_and_display(&id_value)?;
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
        self.backend.execute(&sql, &[Cell::Int(delta), id_cell.clone()]).map_err(|e| write_error("increment", e))?;
        let updated = self
            .select_rows(collection, columns, Some(id_cell))?
            .into_iter()
            .next()
            .ok_or_else(|| RuntimeError::new(format!("no hay ningún elemento con id {id_display} en '{collection}'")))?;
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
        let key = self.encryption_key();
        decode_row(collection, cells, columns, self.id_kind(collection), &self.checker, key.as_ref())
    }
}

/// Cuerpo real de `Db::row_to_fields`, extraído a función libre (GRAMMAR.md
/// §3.185) para que `db_admin.rs` (`linkc db export`) pueda decodificar filas
/// leídas con su PROPIO `Backend` de solo lectura, sin necesitar un `Db`
/// completo (que siempre corre DDL al construirse -- ver `db_admin.rs` para
/// el porqué de fondo). Único punto de verdad para "cómo se convierte una
/// fila cruda de vuelta a `Value`" -- `Db::row_to_fields` es ahora un
/// wrapper de una línea sobre esto mismo.
pub(crate) fn decode_row(
    collection: &str,
    cells: &[Cell],
    columns: &[ColumnPlan],
    id_kind: IdKind,
    checker: &Checker,
    encryption_key: Option<&[u8; encryption::KEY_LEN]>,
) -> Result<Vec<(String, Value)>, RuntimeError> {
    let mut out = Vec::with_capacity(columns.len() + 1);
    // GRAMMAR.md §3.177: `id` es `Cell::Int` para una PK autoincremento
    // o `Cell::Text` para una PK `Uuid` -- `id_column_kind` ya le pidió
    // al SELECT que decodifique esta columna acorde (`select_rows`), así
    // que la forma que llega acá siempre coincide con `id_kind`.
    let (id_field, id_display) = match (id_kind, cells.first()) {
        (IdKind::Int, Some(Cell::Int(n))) => (Value::Int(*n), n.to_string()),
        (IdKind::Uuid, Some(Cell::Text(s))) => (Value::Uuid(s.clone()), s.clone()),
        _ => panic!("la columna 'id' de '{collection}' no matchea su IdKind ({id_kind:?}): llegó {:?}", cells.first()),
    };
    out.push(("id".to_string(), id_field));
    let id = &id_display;

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
                        // AUDIT-2026-08-27.md #14: el comentario original
                        // ("un valor que nosotros escribimos") asume que
                        // TODA fila fue escrita por este mismo programa --
                        // falso bajo `--adopt-existing`/evolución de
                        // esquema: un blob JSON legado, escrito por una
                        // versión ANTERIOR del `.link` (un campo anidado que
                        // ahora es requerido y antes no existía, por
                        // ejemplo), puede no calzar con el tipo ACTUAL.
                        // `panic!` mataba el hilo de esa request en vez de
                        // dar el mismo `RuntimeError` limpio que el resto de
                        // esta función ya usa para "el schema declarado no
                        // coincide con lo que hay guardado".
                        let decoded = json_to_typed_value(parsed, &col.field.ty, checker, &col.field.name).map_err(|e| {
                            RuntimeError::new(format!(
                                "la colección '{collection}' tiene una fila (id={id}) con un JSON guardado en '{}' que no coincide con el tipo declarado actual: {e} -- típico tras evolucionar el tipo de un campo anidado (`--adopt-existing` o una migración de esquema); esa fila se guardó con una forma anterior",
                                col.field.name
                            ))
                        })?;
                        out.push((col.field.name.clone(), decoded));
                    }
                    // Mismo motivo: bajo `--adopt-existing`, una columna que
                    // el `.link` declara como JSON (struct/lista/map/
                    // genérico) podría, en la tabla física real, no ser
                    // JSON en absoluto (un `INTEGER`/`TEXT` plano de un
                    // programa completamente distinto que casualmente
                    // adoptó la misma tabla, por ejemplo).
                    other => {
                        return Err(RuntimeError::new(format!(
                            "la colección '{collection}' tiene una fila (id={id}) cuya columna '{}' debería contener JSON (tipo declarado {:?}) pero la base devolvió {other:?} -- la tabla física no coincide con lo que el programa espera ahí",
                            col.field.name, col.field.ty
                        )))
                    }
                }
                continue;
            }

            // `@encrypted` (GRAMMAR.md §3.191) -- el checker ya garantizó
            // que un campo así marcado es `String`/`String?` sin `x?: T?`
            // (así que nunca llega acá vía la rama JSON de arriba). Corre
            // ANTES del match genérico de abajo porque descifrar es
            // FALIBLE (clave incorrecta, dato corrompido) -- ese match
            // devuelve `Option<Value>` sin lugar para propagar un `Err`.
            if col.encrypted {
                let value = match cell {
                    Cell::Null => None,
                    Cell::Text(t) => {
                        // `encryption_key` es `None` solo si este `Db` nunca
                        // tuvo `set_encryption_key` con `Some(...)` -- no
                        // debería pasar nunca en un `linkc serve` real
                        // (rechaza arrancar sin clave si hay campos
                        // `@encrypted`, ver `server.rs::serve`), pero un
                        // `RuntimeError` limpio acá es mejor que un panic si
                        // de algún modo se llega igual (ej. un test que
                        // arma un `Db` a mano sin pasar por `serve`).
                        let Some(key) = encryption_key else {
                            return Err(RuntimeError::new(format!(
                                "la colección '{collection}' tiene un campo '@encrypted' ('{}') pero no hay ninguna clave de cifrado configurada en este proceso",
                                col.field.name
                            )));
                        };
                        let plaintext = encryption::decrypt_field(t, key).map_err(|e| {
                            RuntimeError::new(format!(
                                "la colección '{collection}' tiene una fila (id={id}) con un valor '@encrypted' en '{}' que no se pudo descifrar: {e}",
                                col.field.name
                            ))
                        })?;
                        Some(Value::Str(plaintext))
                    }
                    other => {
                        return Err(RuntimeError::new(format!(
                            "la colección '{collection}' tiene una fila (id={id}) cuya columna '@encrypted' '{}' debería contener texto cifrado pero la base devolvió {other:?}",
                            col.field.name
                        )))
                    }
                };
                match value {
                    Some(v) => out.push((col.field.name.clone(), v)),
                    None if col.field.optional => {}
                    None if matches!(col.field.ty, Type::Optional(_)) => out.push((col.field.name.clone(), Value::Null)),
                    None => return Err(null_but_required(&col.field.name)),
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
                (Type::Decimal, Cell::Decimal(n)) => Some(Value::Decimal(*n)),
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
                // tipo -- alcanzable de verdad bajo `--adopt-existing`
                // (AUDIT-2026-08-27.md #14): `check_schema_for_adoption`
                // valida existencia y tipo DECLARADO de cada columna, pero
                // SQLite tiene afinidad de tipo, no enforcement -- una
                // columna declarada `INTEGER` puede seguir teniendo filas
                // con `TEXT` físico adentro si algo la escribió así antes.
                // Fallar limpio con los dos lados a la vista (antes: panic,
                // mataba el hilo) es lo único útil -- devolver un valor
                // "parecido" escondería el problema adentro de la respuesta
                // de un rpc.
                (ty, cell) => {
                    return Err(RuntimeError::new(format!(
                        "la colección '{collection}' tiene una fila (id={id}) cuya columna '{}' declara {ty} pero la base devolvió {cell:?} -- la tabla física no coincide con lo que el programa espera ahí (típico bajo --adopt-existing con datos que no calzan)",
                        col.field.name
                    )))
                }
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

impl Db {
    /// Valor a bindear para `col`, dado el valor del `Value::Struct` de entrada
    /// en esa clave (`None` si la clave está ausente -- solo alcanzable si
    /// `col.field.optional`, ver `ColumnPlan`). Inversa de `row_to_fields`.
    /// Falible SOLO por `col.encrypted` (GRAMMAR.md §3.191) -- cifrar puede
    /// fallar si el CSPRNG del sistema falla (`os_random_bytes`, el mismo
    /// que usa la generación de PK `Uuid`) o si no hay ninguna clave
    /// configurada (no debería pasar nunca en un `linkc serve` real, que
    /// rechaza arrancar sin clave si hay campos `@encrypted` -- ver
    /// `server.rs::serve`); un `RuntimeError` limpio acá es mejor que un
    /// panic si de algún modo se llega igual.
    fn write_param(&self, col: &ColumnPlan, slot: Option<&Value>) -> Result<Cell, RuntimeError> {
        let Some(v) = slot else { return Ok(Cell::Null) };
        if col.json {
            // `value_to_json(Value::Null)` da el JSON `null` -- exactamente el
            // sentinel de "presente pero null" que el caso `x?: T?` necesita,
            // sin ningún código especial acá. Y no es lo mismo que un NULL de
            // SQL, que significa "clave ausente": los dos backends conservan
            // esa diferencia (TEXT "null" en SQLite, JSONB null en PostgreSQL).
            return Ok(Cell::Json(value_to_json(v, &self.simple_enums)));
        }
        if col.encrypted {
            return Ok(match v {
                Value::Null => Cell::Null,
                Value::Str(s) => {
                    let key = self.encryption_key().ok_or_else(|| {
                        RuntimeError::new(format!(
                            "la colección tiene un campo '@encrypted' ('{}') pero no hay ninguna clave de cifrado configurada en este proceso",
                            col.field.name
                        ))
                    })?;
                    Cell::Text(encryption::encrypt_field(s, &key)?)
                }
                other => panic!("un campo '@encrypted' solo puede recibir Value::Str/Value::Null -- el checker ya lo garantizó: {other:?}"),
            });
        }
        Ok(match v {
            Value::Null => Cell::Null,
            Value::Int(n) => Cell::Int(*n),
            Value::Int64(n) => Cell::Int(*n),
            // GRAMMAR.md §3.184: `Cell::Decimal`, no `Cell::Int` -- a
            // diferencia de Int64, el rango de i128 no cabe ahí.
            Value::Decimal(n) => Cell::Decimal(*n),
            Value::Timestamp(n) => Cell::Int(*n),
            Value::Float(f) => Cell::Float(*f),
            Value::Str(s) => Cell::Text(s.clone()),
            Value::Uuid(s) => Cell::Text(s.clone()),
            Value::Bool(b) => Cell::Bool(*b),
            Value::Variant { variant, .. } => Cell::Text(variant.clone()),
            other => panic!("valor no representable en una columna nativa de SQL: {other:?}"),
        })
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
/// AUDIT-2026-08-27.md #6: sin brazo para `Cell::Null`, una fila vieja con
/// `NULL` físico en la columna de agrupación/valor (típico tras agregar un
/// campo REQUERIDO a una colección con filas existentes -- Postgres agrega
/// la columna nueva sin `NOT NULL` sin importar la opcionalidad declarada en
/// el `.link`, `codegen/postgres_emit.rs::alter_table_add_column_postgres`,
/// confirmado leyendo esa función) caía al `panic!` genérico de abajo.
/// `row_to_fields` (lectura normal, `find`/`all`/etc.) ya tenía este mismo
/// caso cubierto con un `RuntimeError` limpio (`null_but_required`) -- ese
/// fix nunca se había aplicado acá, el camino de agregación (`sumBy`/
/// `countBy`/`avgBy`/`maxBy`/`minBy`).
fn scalar_cell_to_value(collection: &str, ty: &Type, cell: &Cell) -> Result<Value, RuntimeError> {
    Ok(match (ty, cell) {
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
        // GRAMMAR.md §3.184: `Cell::Decimal` es una variante PROPIA (no
        // comparte `Cell::Int` como Int64) -- sin ambigüedad de storage que
        // resolver mirando `Type`, a diferencia del brazo de Int64 arriba.
        (_, Cell::Decimal(n)) => Value::Decimal(*n),
        (_, Cell::Float(f)) => Value::Float(*f),
        (_, Cell::Text(t)) => Value::Str(t.clone()),
        (_, Cell::Bool(b)) => Value::Bool(*b),
        (ty, Cell::Null) => {
            return Err(RuntimeError::new(format!(
                "la colección '{collection}' tiene una fila con NULL en una columna declarada {ty} usada como clave/valor de agregación -- típico tras agregar un campo REQUERIDO a una colección con filas existentes (ver GRAMMAR.md §9.1.1): Postgres agrega la columna nueva sin NOT NULL sin importar la opcionalidad declarada. Backfilleá esa columna a mano o volvé el campo a opcional."
            )))
        }
        (ty, cell) => panic!("una agregación devolvió {cell:?} para una columna declarada {ty}"),
    })
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

    /// AUDIT-2026-08-27.md #6: `scalar_cell_to_value` (el decodificador de
    /// `sumBy`/`countBy`/`avgBy`/`maxBy`/`minBy`) panicaba sobre
    /// `Cell::Null` en vez de dar el mismo `RuntimeError` limpio que
    /// `row_to_fields` ya usa para "fila con NULL en un campo requerido" --
    /// alcanzable con un campo REQUERIDO agregado a una colección Postgres
    /// con filas viejas (la migración no destructiva agrega la columna
    /// nueva sin `NOT NULL` sin importar la opcionalidad declarada).
    #[test]
    fn scalar_cell_to_value_rejects_null_with_a_clean_error_instead_of_panicking() {
        let e = scalar_cell_to_value("sales", &Type::Int, &Cell::Null).unwrap_err();
        assert!(e.message.contains("sales"), "{}", e.message);
        assert!(e.message.contains("NULL"), "{}", e.message);
        // El camino feliz no cambia.
        assert!(matches!(scalar_cell_to_value("sales", &Type::Int, &Cell::Int(5)), Ok(Value::Int(5))));
        assert!(matches!(scalar_cell_to_value("sales", &Type::Int64, &Cell::Int(5)), Ok(Value::Int64(5))));
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

    /// AUDIT-2026-08-27.md #14: `check_schema_for_adoption` valida el tipo
    /// DECLARADO de cada columna, pero SQLite tiene afinidad de tipo, no
    /// enforcement real -- una columna declarada `INTEGER` puede seguir
    /// aceptando (y guardando tal cual) un valor `TEXT` si algo la escribió
    /// así por fuera de c-script. Antes de este fix, leer esa fila panicaba
    /// (`row_to_fields`) en vez de dar un `RuntimeError` limpio.
    #[test]
    fn adopting_a_table_whose_physical_type_does_not_match_the_declared_one_gives_a_clean_error_not_a_panic() {
        let path = std::env::temp_dir().join("c_script_test_adopt_type_mismatch.db");
        let _ = std::fs::remove_file(&path);

        {
            let raw = Connection::open(&path).unwrap();
            raw.execute("CREATE TABLE \"items\" (\"id\" INTEGER PRIMARY KEY AUTOINCREMENT, \"qty\" INTEGER NOT NULL)", []).unwrap();
            // SQLite tiene afinidad de tipo, no enforcement -- esto SÍ
            // guarda un TEXT en una columna declarada INTEGER.
            raw.execute("INSERT INTO \"items\" (\"qty\") VALUES ('no-es-un-numero')", []).unwrap();
        }

        let program = program_from("type Item = { id: Int, qty: Int } db { items: Item[] }");
        let db = Db::new_with_options(&program, &path, true);
        let e = db.call("items", "all", vec![]).unwrap_err();
        assert!(e.message.contains("items"), "{}", e.message);
        assert!(e.message.contains("qty"), "{}", e.message);

        let _ = std::fs::remove_file(&path);
    }

    /// Mismo hallazgo (#14), el otro sitio: una columna JSON-serializada
    /// (struct anidado) cuya forma GUARDADA (de una versión anterior del
    /// `.link`) ya no calza con el tipo DECLARADO actual -- ej. un campo
    /// nuevo requerido que el JSON legado nunca tuvo. `json_to_typed_value`
    /// da `Err` legítimamente ahí (falta un campo requerido), pero el
    /// comentario original asumía "esto lo escribimos nosotros mismos,
    /// nunca puede fallar" y panicaba en vez de propagar ese error.
    #[test]
    fn adopting_a_table_whose_stored_json_no_longer_matches_the_current_nested_type_gives_a_clean_error() {
        let path = std::env::temp_dir().join("c_script_test_adopt_json_shape_mismatch.db");
        let _ = std::fs::remove_file(&path);

        {
            let raw = Connection::open(&path).unwrap();
            raw.execute("CREATE TABLE \"orders\" (\"id\" INTEGER PRIMARY KEY AUTOINCREMENT, \"meta\" TEXT NOT NULL)", []).unwrap();
            // Forma legada: nunca tuvo 'trackingCode', que la versión
            // actual del .link declara como campo requerido.
            raw.execute("INSERT INTO \"orders\" (\"meta\") VALUES ('{\"carrier\":\"DHL\"}')", []).unwrap();
        }

        let program = program_from(
            "type Meta = { carrier: String, trackingCode: String } type Order = { id: Int, meta: Meta } db { orders: Order[] }",
        );
        let db = Db::new_with_options(&program, &path, true);
        let e = db.call("orders", "all", vec![]).unwrap_err();
        assert!(e.message.contains("orders"), "{}", e.message);
        assert!(e.message.contains("meta"), "{}", e.message);

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
