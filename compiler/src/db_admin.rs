// `linkc db export`/`linkc db import` (GRAMMAR.md §3.185, PLAN.md §9.7 ítem
// 2 -- siguiente pieza de la suite de administración de datos, después de
// `linkc db inspect`, `src/inspect.rs`). `export` vuelca cada colección
// declarada a un solo archivo JSON, byte-idéntico al wire real (mismo
// `value_to_json` que ya usa `db.<c>.all()` por HTTP); `import` lo lee de
// vuelta contra un target SQLite o PostgreSQL, PRESERVANDO el id original de
// cada fila (una migración/restauración fiel, no una que reasigna ids
// nuevos). Importar contra un target vacío ES el caso "seed" -- mismo
// mecanismo, sin código aparte.
//
// `export` nunca corre DDL ni construye un `Db` completo (que siempre migra
// el esquema al abrir, salvo `adopt_existing=true` -- y ESE modo panickea
// apenas una colección declarada no tiene tabla física todavía, exactamente
// el caso normal que `inspect.rs` existe para tratar como estado normal, no
// error). Mismo espíritu de solo-lectura que `inspect.rs`: una tabla
// faltante son "0 filas", nunca un error.

use crate::ast::Program;
use crate::checker::Checker;
use crate::runtime::db::{
    connect_postgres_client, decode_row, encrypted_fields_by_collection, id_column_kind_for, now_ms, sqlite_table_exists, ColumnPlan, Db,
    IdKind,
};
use crate::runtime::store::Backend;
use crate::runtime::timestamp::format_iso8601_millis;
use crate::runtime::{json_to_typed_value, simple_enum_names, value_to_json, Value};
use crate::types::Type;
use rusqlite::{Connection, OpenFlags};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Foto completa de la base -- TODAS las colecciones que el `.link` ACTUAL
/// declara, cada una con su array de filas (vacío si la colección no tiene
/// tabla física todavía). `linkc_version` es puramente informativo, nunca
/// se compara contra el binario corriendo -- un desajuste de esquema se
/// manifiesta a través del `Db::new`/DDL normal del lado de `import`, no
/// de este campo.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct ExportFile {
    pub linkc_version: String,
    pub exported_at: String,
    pub collections: HashMap<String, Vec<serde_json::Value>>,
}

/// Plan de UNA colección declarada -- superset de
/// `inspect.rs::declared_collections` (que solo cuenta campos): acá hace
/// falta el `Vec<ColumnPlan>` completo (decodificar/escribir filas de
/// verdad) y el `IdKind` (autoincremento vs Uuid).
struct CollectionPlan {
    name: String,
    columns: Vec<ColumnPlan>,
    id_kind: IdKind,
    id_col: String,
}

/// Mismo `Checker::build_symbols` (sin instanciar ningún `Db`/DDL) que
/// `inspect.rs::declared_collections` ya usa, extendido con el `ColumnPlan`
/// completo de cada colección en vez de solo un conteo.
///
/// GRAMMAR.md §3.191, "Límites honestos": un programa con algún campo
/// `@encrypted` se rechaza ACÁ, antes de tocar ninguna fila -- ni `export`
/// ni `import` soportan cifrado todavía. `export` (que sí llega a
/// `decode_row`, el chokepoint de descifrado) no tiene ninguna clave
/// disponible ahí, y mostrar el ciphertext crudo sin avisar sería
/// confuso, no solo incompleto; rechazar de entrada, con un mensaje
/// claro, es más honesto que un passthrough silencioso. `db shell` NO
/// necesita este chequeo -- nunca pasa por `decode_row` (SQL crudo,
/// sin `ColumnPlan`), así que ya muestra el ciphertext tal cual, de forma
/// consistente con cómo trata cualquier otro valor físico.
fn declared_collection_plans(program: &Program) -> Result<(Checker, Vec<CollectionPlan>), String> {
    let (checker, errors) = Checker::build_symbols(program);
    if let Some(e) = errors.into_iter().next() {
        return Err(format!("programa inválido: {e}"));
    }
    let encrypted_by_collection = encrypted_fields_by_collection(program, &checker);
    if let Some((coll, fields)) = encrypted_by_collection.iter().next() {
        let mut names: Vec<&str> = fields.iter().map(String::as_str).collect();
        names.sort_unstable();
        return Err(format!(
            "'{coll}' tiene campo(s) '@encrypted' ({}) -- 'db export'/'db import' todavía no soportan colecciones con campos cifrados (GRAMMAR.md §3.191)",
            names.join(", ")
        ));
    }
    let aliases_by_collection = crate::runtime::db::column_aliases_by_collection(program, &checker);
    let empty_aliases = HashMap::new();
    let simple_enums = simple_enum_names(program);
    let mut out: Vec<CollectionPlan> = checker
        .db_collections()
        .iter()
        .filter_map(|(name, ty)| match ty {
            Type::Struct { fields, .. } => {
                let id_field_ty = &fields.iter().find(|f| f.name == "id")?.ty;
                let aliases = aliases_by_collection.get(name).unwrap_or(&empty_aliases);
                let id_col = aliases.get("id").map(String::as_str).unwrap_or("id").to_string();
                let columns: Vec<ColumnPlan> = fields
                    .iter()
                    .filter(|f| f.name != "id")
                    .map(|f| ColumnPlan::for_field(f.clone(), &simple_enums, false, aliases.get(&f.name).cloned()))
                    .collect();
                Some(CollectionPlan { name: name.clone(), columns, id_kind: IdKind::from_field_type(id_field_ty), id_col })
            }
            _ => None,
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok((checker, out))
}

/// Tipo lógico de la columna `"id"` de una colección -- `Int`, `Uuid` o
/// `String` (GRAMMAR.md §3.177/§3.251), a partir de su `IdKind`. Necesario
/// para decodificar el valor `"id"` de una fila del archivo de export con
/// `json_to_typed_value`, igual que cualquier otro campo.
fn id_type(id_kind: IdKind) -> Type {
    match id_kind {
        IdKind::Int => Type::Int,
        IdKind::Uuid => Type::Uuid,
        IdKind::String => Type::String,
    }
}

/// TODAS las filas físicas de `plan`, SIN filtrar `@softDelete` -- mismo
/// criterio que `db.tableStats()`/`db inspect` (verdad física, no lo que un
/// rpc normal vería), a propósito distinto de `all()`. Decodificadas con el
/// MISMO `decode_row` que el runtime normal usa, y serializadas con el
/// MISMO `value_to_json` que `db.<c>.all()` ya manda por HTTP -- byte-
/// idéntico al wire real.
fn read_all_rows(
    backend: &Backend,
    checker: &Checker,
    simple_enums: &HashSet<String>,
    plan: &CollectionPlan,
) -> Result<Vec<serde_json::Value>, String> {
    let mut col_list = vec![format!("\"{}\"", plan.id_col)];
    let mut kinds = vec![id_column_kind_for(plan.id_kind)];
    for col in &plan.columns {
        col_list.push(format!("\"{}\"", col.sql_name));
        kinds.push(col.kind());
    }
    let sql = format!("SELECT {} FROM \"{}\" ORDER BY \"{}\"", col_list.join(", "), plan.name, plan.id_col);
    let rows = backend.query(&sql, &[], &kinds)?;
    rows.into_iter()
        .map(|cells| {
            // `None`: `declared_collection_plans` ya rechazó cualquier
            // programa con un campo `@encrypted` (GRAMMAR.md §3.191), así
            // que ninguna `col` de acá abajo tiene `encrypted: true` -- este
            // parámetro nunca se usa de verdad para `export`.
            let fields = decode_row(&plan.name, &cells, &plan.columns, plan.id_kind, checker, None).map_err(|e| e.to_string())?;
            Ok(value_to_json(&Value::Struct(fields), simple_enums))
        })
        .collect()
}

fn export_file(collections: HashMap<String, Vec<serde_json::Value>>) -> ExportFile {
    ExportFile { linkc_version: crate::VERSION.to_string(), exported_at: format_iso8601_millis(now_ms()), collections }
}

/// `db_path` inexistente (checkout fresco, antes del primer `linkc serve`)
/// exporta cada colección declarada como array VACÍO -- nunca un error,
/// mismo criterio que `inspect_sqlite`. Abre de solo lectura
/// (`SQLITE_OPEN_READ_ONLY`) cuando el archivo sí existe.
pub fn export_sqlite(program: &Program, db_path: &Path) -> Result<ExportFile, String> {
    let (checker, plans) = declared_collection_plans(program)?;
    let simple_enums = simple_enum_names(program);
    let mut collections = HashMap::with_capacity(plans.len());
    if !db_path.exists() {
        for plan in &plans {
            collections.insert(plan.name.clone(), Vec::new());
        }
        return Ok(export_file(collections));
    }
    let connection = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("no se pudo abrir '{}' de solo lectura: {e}", db_path.display()))?;
    // La existencia de cada tabla se resuelve con la conexión CRUDA, antes
    // de envolverla en un `Backend` (que la toma por valor) -- una sola
    // conexión sirve para las N colecciones de este export.
    let existing: HashSet<String> = plans.iter().filter(|p| sqlite_table_exists(&connection, &p.name)).map(|p| p.name.clone()).collect();
    let backend = Backend::sqlite(connection, None, 1);
    for plan in &plans {
        let rows = if existing.contains(&plan.name) { read_all_rows(&backend, &checker, &simple_enums, plan)? } else { Vec::new() };
        collections.insert(plan.name.clone(), rows);
    }
    Ok(export_file(collections))
}

/// Misma idea que `export_sqlite`, contra PostgreSQL real -- reusa
/// `existing_columns` (`migrate.rs`) para la detección de existencia, igual
/// que `inspect_postgres`. Backend armado a mano, sin ningún `Db` (sin hilo
/// LISTEN/NOTIFY, sin tabla de rate-limit -- nada de eso pertenece a un
/// export de solo lectura).
pub fn export_postgres(program: &Program, url: &str, schema: Option<&str>) -> Result<ExportFile, String> {
    let (checker, plans) = declared_collection_plans(program)?;
    let simple_enums = simple_enum_names(program);
    let client = connect_postgres_client(url, schema)?;
    let backend = Backend::postgres(client, url, schema, 1);
    let mut collections = HashMap::with_capacity(plans.len());
    for plan in &plans {
        let exists = !crate::migrate::existing_columns(&backend, &plan.name)?.is_empty();
        let rows = if exists { read_all_rows(&backend, &checker, &simple_enums, plan)? } else { Vec::new() };
        collections.insert(plan.name.clone(), rows);
    }
    Ok(export_file(collections))
}

/// Cuántas filas se importaron por colección -- solo las que de verdad
/// tenían una entrada en el archivo (una colección declarada pero ausente
/// del archivo no aparece acá, y no es un error).
pub type ImportReport = Vec<(String, usize)>;

/// Decodifica UNA fila del archivo (un objeto JSON) contra `plan`, CAMPO
/// POR CAMPO -- nunca envuelta en `Type::Struct{name: Some(...)}` (el
/// decodificador de BORDE que dispara `@validate`/`@check` de nivel tipo,
/// pensado para input de cliente no confiable). Import bypassea esos
/// validadores a propósito (GRAMMAR.md §3.185: una restauración cruda de
/// datos que ya eran válidos cuando se escribieron no debería bloquearse
/// por un validador de flujo de trabajo específico de la app) -- las
/// restricciones de BASE (`CHECK`/`UNIQUE` de la DDL) siguen aplicando
/// siempre, las impone el propio `INSERT` de `Db::import_row`. Una clave
/// `x?: T?` ausente del objeto JSON decodifica a "sin entrada" (nunca
/// `Value::Null`) -- mismo contrato que `write_param`/`decode_row` asumen.
fn decode_import_row(row: &serde_json::Value, plan: &CollectionPlan, checker: &Checker) -> Result<(Value, Vec<(String, Value)>), String> {
    let obj = row.as_object().ok_or_else(|| format!("'{}': cada fila del archivo tiene que ser un objeto JSON", plan.name))?;
    let id_json = obj.get("id").ok_or_else(|| format!("'{}': una fila no trae 'id'", plan.name))?;
    let id = json_to_typed_value(id_json, &id_type(plan.id_kind), checker, "id").map_err(|e| format!("'{}': {e}", plan.name))?;
    let mut fields = Vec::with_capacity(plan.columns.len());
    for col in &plan.columns {
        match obj.get(&col.field.name) {
            Some(field_json) => {
                let v = json_to_typed_value(field_json, &col.field.ty, checker, &col.field.name).map_err(|e| format!("'{}': {e}", plan.name))?;
                fields.push((col.field.name.clone(), v));
            }
            None if col.field.optional => {}
            None => return Err(format!("'{}': falta la clave requerida '{}' en una fila", plan.name, col.field.name)),
        }
    }
    Ok((id, fields))
}

/// Colección desconocida en el archivo -- error duro ANTES de conectar
/// siquiera con el target (llamado antes de `Db::new`/
/// `connect_postgres_for_testing`, que corren DDL idempotente -- si el
/// error saliera DESPUÉS de conectar, "nada se escribió" sería impreciso:
/// el archivo `.db`/las tablas ya existirían aunque ninguna FILA se haya
/// tocado). Descartar en silencio datos que el operador pidió restaurar
/// explícitamente es peor que el caso inverso (una colección declarada
/// pero ausente del archivo, normal y silencioso).
fn validate_known_collections(plans: &[CollectionPlan], file: &ExportFile) -> Result<(), String> {
    let declared: HashSet<&str> = plans.iter().map(|p| p.name.as_str()).collect();
    for name in file.collections.keys() {
        if !declared.contains(name.as_str()) {
            return Err(format!(
                "el archivo trae la colección '{name}', que el programa actual no declara en 'db {{ ... }}' -- import cancelado, nada se escribió"
            ));
        }
    }
    Ok(())
}

/// Corre el import ENTERO dentro de una sola transacción SQL real (mismas
/// piezas que `transaction {{ }}` usa: `with_exclusive_connection`/
/// `begin_transaction`/`commit_transaction`/`rollback_transaction`) --
/// todo o nada. Un choque de id (la fila ya existe en el target) o
/// cualquier otro error de escritura cancela y revierte TODO el import,
/// filas ya insertadas incluidas -- mismo criterio de "fallar limpio y
/// ruidoso" que `check_schema_for_adoption` ya establece, en vez de dejar
/// datos a medias o adivinar (overwrite/skip) sin que se pida.
fn run_import(db: &Db, checker: &Checker, plans: &[CollectionPlan], file: &ExportFile) -> Result<ImportReport, String> {
    db.with_exclusive_connection(|| {
        db.begin_transaction()?;
        let mut report = ImportReport::new();
        for plan in plans {
            let Some(rows) = file.collections.get(&plan.name) else { continue };
            for (i, row) in rows.iter().enumerate() {
                let (id, fields) = decode_import_row(row, plan, checker).map_err(|e| {
                    db.rollback_transaction();
                    format!("{e} (fila #{i}) -- import cancelado, ningún cambio quedó aplicado")
                })?;
                if let Err(e) = db.import_row(&plan.name, &id, &fields) {
                    db.rollback_transaction();
                    return Err(format!("'{}' fila #{i}: {e} -- import cancelado, ningún cambio quedó aplicado", plan.name));
                }
            }
            if !rows.is_empty() {
                if let Err(e) = db.resync_id_sequence(&plan.name) {
                    db.rollback_transaction();
                    return Err(format!("'{}': {e} -- import cancelado, ningún cambio quedó aplicado", plan.name));
                }
            }
            report.push((plan.name.clone(), rows.len()));
        }
        db.commit_transaction().map(|_| report).map_err(|e| {
            db.rollback_transaction();
            format!("COMMIT falló: {e} -- import cancelado, ningún cambio quedó aplicado")
        })
    })
}

/// Conecta con `Db::new` NORMAL (nunca `adopt_existing`) -- `CREATE TABLE
/// IF NOT EXISTS` cubre los dos casos de punta a punta con un solo camino:
/// target vacío (esto ES el caso "seed", sin código aparte) o target ya
/// servido antes con el mismo `.link` (DDL idempotente, sigue derecho a
/// los datos).
pub fn import_sqlite(program: &Program, db_path: &Path, file: &ExportFile) -> Result<ImportReport, String> {
    let (checker, plans) = declared_collection_plans(program)?;
    validate_known_collections(&plans, file)?;
    let db = Db::new(program, db_path);
    run_import(&db, &checker, &plans, file)
}

/// Misma idea que `import_sqlite`, contra PostgreSQL real --
/// `connect_postgres_for_testing` (envoltorio público existente que
/// descarta el receiver de LISTEN/NOTIFY: un `linkc db import` es una
/// corrida de una sola vez, sin nada más escuchando cambios en vivo).
pub fn import_postgres(program: &Program, url: &str, file: &ExportFile, schema: Option<&str>) -> Result<ImportReport, String> {
    let (checker, plans) = declared_collection_plans(program)?;
    validate_known_collections(&plans, file)?;
    let db = Db::connect_postgres_for_testing(program, url, false, schema)?;
    run_import(&db, &checker, &plans, file)
}

// ---- `linkc db shell` (GRAMMAR.md §3.189, PLAN.md §9.7 ítem 2 -- cierra la
// suite de administración de datos): SQL arbitrario, de SOLO LECTURA, con
// filas de tipo DINÁMICO -- a diferencia de `Backend::query` (`store.rs`,
// reusado por export/import arriba), que exige `&[ColumnKind]` de antemano
// porque siempre conoce la forma declarada. Acá no hay forma que conocer de
// antemano: el usuario escribe SQL suelto, la forma del resultado solo se
// sabe después de ejecutar. ----

/// Una sentencia SQL contra SQLite, devuelta ya como texto -- headers +
/// filas, cada celda formateada para mostrarse (nunca un `Value`/`Cell`
/// tipado: no hay ningún `ColumnPlan` declarado que darle forma, el punto
/// entero de un shell es aceptar CUALQUIER SQL). `rusqlite::Row::get_ref`
/// decodifica cada celda SIN saber su tipo de antemano (`ValueRef` ya es un
/// enum con Null/Integer/Real/Text/Blob) -- exactamente lo que hace falta
/// acá y que `Backend::query` no ofrece.
pub fn run_query_sqlite(connection: &Connection, sql: &str) -> Result<(Vec<String>, Vec<Vec<String>>), String> {
    let mut stmt = connection.prepare(sql).map_err(|e| e.to_string())?;
    let headers: Vec<String> = stmt.column_names().into_iter().map(str::to_string).collect();
    let n = headers.len();
    let mut rows_out = Vec::new();
    let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let mut cells = Vec::with_capacity(n);
        for i in 0..n {
            let text = match row.get_ref(i).map_err(|e| e.to_string())? {
                rusqlite::types::ValueRef::Null => "NULL".to_string(),
                rusqlite::types::ValueRef::Integer(v) => v.to_string(),
                rusqlite::types::ValueRef::Real(v) => v.to_string(),
                rusqlite::types::ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
                rusqlite::types::ValueRef::Blob(b) => format!("<blob, {} bytes>", b.len()),
            };
            cells.push(text);
        }
        rows_out.push(cells);
    }
    Ok((headers, rows_out))
}

/// `postgres::Error::to_string()` es deliberadamente parco para un error de
/// tipo `Kind::Db` (confirmado leyendo la fuente de `tokio-postgres`) -- imprime
/// LITERALMENTE el string fijo `"db error"`, sin el mensaje real del servidor
/// (severidad, texto, DETAIL, HINT), que vive aparte en el `DbError` accesible
/// vía `.as_db_error()`. Descubierto en CI, no localmente: el mensaje real de
/// Postgres para una escritura rechazada por `default_transaction_read_only`
/// (`ERROR: cannot execute INSERT in a read-only transaction`) se perdía
/// entero, dejando al usuario del shell sin ninguna pista de qué pasó. Esta
/// función es el único punto de conversión de un `postgres::Error` a texto en
/// todo `run_query_postgres` -- mismo criterio de "chokepoint único" que el
/// resto de este módulo ya usa para otras conversiones.
fn pg_error_text(e: postgres::Error) -> String {
    match e.as_db_error() {
        Some(db_err) => db_err.to_string(),
        None => e.to_string(),
    }
}

/// Misma idea que `run_query_sqlite`, contra PostgreSQL real. `client.prepare`
/// (no `client.query` directo) es lo que da los headers incluso cuando el
/// resultado tiene CERO filas -- `Statement::columns()` está disponible antes
/// de ejecutar nada, a diferencia de tratar de leerlos de la primera fila
/// (que no existiría en ese caso).
pub fn run_query_postgres(client: &mut postgres::Client, sql: &str) -> Result<(Vec<String>, Vec<Vec<String>>), String> {
    let stmt = client.prepare(sql).map_err(pg_error_text)?;
    let headers: Vec<String> = stmt.columns().iter().map(|c| c.name().to_string()).collect();
    let rows = client.query(&stmt, &[]).map_err(pg_error_text)?;
    let mut rows_out = Vec::with_capacity(rows.len());
    for row in &rows {
        let mut cells = Vec::with_capacity(headers.len());
        for (i, col) in row.columns().iter().enumerate() {
            cells.push(format_pg_cell(row, i, col.type_()));
        }
        rows_out.push(cells);
    }
    Ok((headers, rows_out))
}

/// Formatea UNA celda de una fila Postgres a texto, sin conocer su tipo de
/// antemano más que por el `Type` que la propia fila ya trae. Cubre los
/// tipos nativos ya decodificados en algún lado de este proyecto (`uuid`
/// vía `PgUuidText`, GRAMMAR.md §3.179; `numeric` vía `PgDecimal`, GRAMMAR.md
/// §3.184 -- exacto, no el `PgNumeric` con pérdida que usa `Float`;
/// `timestamp`/`timestamptz`/`date` vía `PgTimestampMicros`/`PgDateDays`,
/// GRAMMAR.md §3.182, formateados ISO-8601 con `format_iso8601_millis`;
/// `json`/`jsonb` vía `PgJsonText`, GRAMMAR.md §3.187) más los escalares
/// nativos de `postgres-types`. Un tipo NO cubierto (`point`, `tsvector`,
/// un tipo de extensión, etc.) cae a un placeholder legible -- nunca falla
/// la consulta ENTERA por una sola columna de un tipo exótico.
fn format_pg_cell(row: &postgres::Row, i: usize, ty: &postgres::types::Type) -> String {
    use postgres::types::Type as PgType;
    macro_rules! cell {
        ($t:ty) => {
            match row.try_get::<_, Option<$t>>(i) {
                Ok(Some(v)) => v.to_string(),
                Ok(None) => "NULL".to_string(),
                Err(e) => format!("<error: {e}>"),
            }
        };
    }
    match *ty {
        PgType::BOOL => cell!(bool),
        PgType::INT2 => cell!(i16),
        PgType::INT4 => cell!(i32),
        PgType::INT8 => cell!(i64),
        PgType::FLOAT4 => cell!(f32),
        PgType::FLOAT8 => cell!(f64),
        PgType::TEXT | PgType::VARCHAR | PgType::BPCHAR | PgType::NAME => cell!(String),
        PgType::UUID => match row.try_get::<_, Option<crate::runtime::store::PgUuidText>>(i) {
            Ok(Some(crate::runtime::store::PgUuidText(s))) => s,
            Ok(None) => "NULL".to_string(),
            Err(e) => format!("<error: {e}>"),
        },
        PgType::NUMERIC => match row.try_get::<_, Option<crate::runtime::store::PgDecimal>>(i) {
            Ok(Some(crate::runtime::store::PgDecimal(raw))) => crate::runtime::format_decimal(raw),
            Ok(None) => "NULL".to_string(),
            Err(e) => format!("<error: {e}>"),
        },
        PgType::TIMESTAMP | PgType::TIMESTAMPTZ => match row.try_get::<_, Option<crate::runtime::store::PgTimestampMicros>>(i) {
            Ok(Some(crate::runtime::store::PgTimestampMicros(micros))) => {
                format_iso8601_millis(crate::runtime::timestamp::millis_from_pg_timestamp_micros(micros))
            }
            Ok(None) => "NULL".to_string(),
            Err(e) => format!("<error: {e}>"),
        },
        PgType::DATE => match row.try_get::<_, Option<crate::runtime::store::PgDateDays>>(i) {
            Ok(Some(crate::runtime::store::PgDateDays(days))) => {
                format_iso8601_millis(crate::runtime::timestamp::millis_from_pg_date_days(days))
            }
            Ok(None) => "NULL".to_string(),
            Err(e) => format!("<error: {e}>"),
        },
        PgType::JSON | PgType::JSONB => match row.try_get::<_, Option<crate::runtime::store::PgJsonText>>(i) {
            Ok(Some(crate::runtime::store::PgJsonText(s))) => s,
            Ok(None) => "NULL".to_string(),
            Err(e) => format!("<error: {e}>"),
        },
        ref other => format!("<tipo no soportado: {other}>"),
    }
}

/// Tabla alineada -- mismo espíritu que las columnas alineadas de `db
/// inspect` (`main.rs::cmd_db_inspect`), extendido a un número arbitrario de
/// columnas (acá no se sabe cuántas ni cuáles hasta ejecutar la consulta).
fn format_table(headers: &[String], rows: &[Vec<String>]) -> String {
    if headers.is_empty() {
        return "(sin columnas)".to_string();
    }
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    let pad = |s: &str, w: usize| format!("{s}{}", " ".repeat(w.saturating_sub(s.chars().count())));
    let mut out = String::new();
    out.push_str(&headers.iter().zip(&widths).map(|(h, w)| pad(h, *w)).collect::<Vec<_>>().join("  "));
    out.push('\n');
    out.push_str(&widths.iter().map(|w| "-".repeat(*w)).collect::<Vec<_>>().join("  "));
    for row in rows {
        out.push('\n');
        out.push_str(&row.iter().zip(&widths).map(|(c, w)| pad(c, *w)).collect::<Vec<_>>().join("  "));
    }
    out.push('\n');
    out.push_str(&format!("({} fila(s))", rows.len()));
    out
}

/// El loop del REPL en sí -- genérico sobre CÓMO se corre una consulta
/// (`run_one`), así que SQLite y Postgres comparten exactamente el mismo
/// bucle de lectura/impresión, solo difieren en cómo abren la conexión.
/// Mismo patrón de "loop bloqueante, sin async, leyendo stdin línea por
/// línea" que `Lsp::run_stdio` (`lsp.rs`) ya usa -- acá con framing MUCHO
/// más simple: una línea de entrada es una consulta completa, sin el
/// `Content-Length` que LSP sí necesita (no hay protocolo que respetar).
/// Línea vacía, `.exit`/`.quit`, o EOF (cerrar stdin) terminan el loop
/// limpio -- mismo criterio de cierre que `Lsp::run_stdio` usa para EOF.
/// Después de cada consulta (éxito o error) se imprime un separador fijo
/// (`--fin--`) -- así un cliente automatizado (`cli_db_shell.rs`) sabe
/// exactamente dónde termina una respuesta sin tener que adivinar por la
/// forma del resultado.
fn run_shell_loop(mut run_one: impl FnMut(&str) -> Result<(Vec<String>, Vec<Vec<String>>), String>) -> Result<(), String> {
    use std::io::{BufRead, Write};
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    loop {
        print!("db> ");
        let _ = stdout.flush();
        let mut line = String::new();
        let n = stdin.lock().read_line(&mut line).map_err(|e| e.to_string())?;
        if n == 0 {
            println!();
            return Ok(());
        }
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed == ".exit" || trimmed == ".quit" {
            return Ok(());
        }
        match run_one(trimmed) {
            Ok((headers, rows)) => println!("{}", format_table(&headers, &rows)),
            Err(e) => println!("error: {e}"),
        }
        println!("--fin--");
        let _ = stdout.flush();
    }
}

/// `linkc db shell <archivo.link> [--db <url|archivo>]` contra SQLite --
/// mismo `SQLITE_OPEN_READ_ONLY` que `inspect_sqlite` (`inspect.rs`) usa:
/// SQLite mismo rechaza cualquier escritura para esta conexión, sin
/// necesitar parsear el SQL a mano para adivinar si es un `SELECT`.
pub fn run_shell_sqlite(db_path: &Path) -> Result<(), String> {
    let connection = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("no se pudo abrir '{}' de solo lectura: {e}", db_path.display()))?;
    run_shell_loop(|sql| run_query_sqlite(&connection, sql))
}

/// Misma idea, contra PostgreSQL real. No existe un modo de conexión de
/// solo-lectura en este código todavía (a diferencia de SQLite) -- acá es
/// donde hace falta por primera vez: `SET default_transaction_read_only =
/// on`, UNA vez después de conectar, hace que el SERVIDOR MISMO rechace
/// cualquier escritura de esta sesión, para cualquier SQL que el usuario
/// escriba -- más robusto que parsear palabras clave del lado del cliente
/// (un `WITH x AS (INSERT ...) SELECT ...` engañaría un parser de
/// keywords, nunca a Postgres mismo).
pub fn run_shell_postgres(url: &str, schema: Option<&str>) -> Result<(), String> {
    let mut client = connect_postgres_client(url, schema)?;
    client.batch_execute("SET default_transaction_read_only = on").map_err(pg_error_text)?;
    run_shell_loop(|sql| run_query_postgres(&mut client, sql))
}
