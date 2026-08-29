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
    connect_postgres_client, decode_row, id_column_kind_for, now_ms, sqlite_table_exists, ColumnPlan, Db, IdKind,
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
}

/// Mismo `Checker::build_symbols` (sin instanciar ningún `Db`/DDL) que
/// `inspect.rs::declared_collections` ya usa, extendido con el `ColumnPlan`
/// completo de cada colección en vez de solo un conteo.
fn declared_collection_plans(program: &Program) -> Result<(Checker, Vec<CollectionPlan>), String> {
    let (checker, errors) = Checker::build_symbols(program);
    if let Some(e) = errors.into_iter().next() {
        return Err(format!("programa inválido: {e}"));
    }
    let simple_enums = simple_enum_names(program);
    let mut out: Vec<CollectionPlan> = checker
        .db_collections()
        .iter()
        .filter_map(|(name, ty)| match ty {
            Type::Struct { fields, .. } => {
                let id_field_ty = &fields.iter().find(|f| f.name == "id")?.ty;
                let columns: Vec<ColumnPlan> =
                    fields.iter().filter(|f| f.name != "id").map(|f| ColumnPlan::for_field(f.clone(), &simple_enums)).collect();
                Some(CollectionPlan { name: name.clone(), columns, id_kind: IdKind::from_field_type(id_field_ty) })
            }
            _ => None,
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok((checker, out))
}

/// Tipo lógico de la columna `"id"` de una colección -- `Int` o `Uuid`
/// (GRAMMAR.md §3.177), a partir de su `IdKind`. Necesario para decodificar
/// el valor `"id"` de una fila del archivo de export con
/// `json_to_typed_value`, igual que cualquier otro campo.
fn id_type(id_kind: IdKind) -> Type {
    match id_kind {
        IdKind::Int => Type::Int,
        IdKind::Uuid => Type::Uuid,
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
    let mut col_list = vec!["\"id\"".to_string()];
    let mut kinds = vec![id_column_kind_for(plan.id_kind)];
    for col in &plan.columns {
        col_list.push(format!("\"{}\"", col.field.name));
        kinds.push(col.kind());
    }
    let sql = format!("SELECT {} FROM \"{}\" ORDER BY \"id\"", col_list.join(", "), plan.name);
    let rows = backend.query(&sql, &[], &kinds)?;
    rows.into_iter()
        .map(|cells| {
            let fields = decode_row(&plan.name, &cells, &plan.columns, plan.id_kind, checker).map_err(|e| e.to_string())?;
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
    let backend = Backend::Sqlite(parking_lot::ReentrantMutex::new(connection));
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
pub fn export_postgres(program: &Program, url: &str) -> Result<ExportFile, String> {
    let (checker, plans) = declared_collection_plans(program)?;
    let simple_enums = simple_enum_names(program);
    let client = connect_postgres_client(url)?;
    let backend = Backend::Postgres { client: parking_lot::ReentrantMutex::new(std::cell::RefCell::new(client)), url: url.to_string() };
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
pub fn import_postgres(program: &Program, url: &str, file: &ExportFile) -> Result<ImportReport, String> {
    let (checker, plans) = declared_collection_plans(program)?;
    validate_known_collections(&plans, file)?;
    let db = Db::connect_postgres_for_testing(program, url, false)?;
    run_import(&db, &checker, &plans, file)
}
