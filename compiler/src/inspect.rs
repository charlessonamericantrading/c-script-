// `linkc db inspect` (GRAMMAR.md §3.175, PLAN.md §9.7 ítem 2 -- primera
// pieza de la suite de administración de datos): lista cada colección
// declarada en el `.link` con su estado FÍSICO real -- existe o no, cuántas
// filas tiene -- SIN ejecutar ningún DDL, a diferencia de `linkc serve`/
// `linkc serve-all` (que crean/migran tablas al conectar). Mismo espíritu
// de solo-lectura que `linkc doctor`/`linkc migrate --dry-run`, y misma
// filosofía de reusar funciones puras ya existentes en vez de duplicar
// lógica de conexión/introspección (`sqlite_table_exists`, `runtime::db`;
// `existing_columns`, `migrate.rs`).

use crate::ast::Program;
use crate::checker::Checker;
use crate::runtime::db::{connect_postgres_client, sqlite_table_exists};
use crate::runtime::store::{Backend, Cell, ColumnKind};
use crate::types::Type;
use rusqlite::{Connection, OpenFlags};
use std::path::Path;

/// Estado físico de UNA colección declarada -- `exists: false` implica
/// `row_count: None` (no hay nada que contar todavía), nunca `Some(0)`,
/// para que el reporte pueda distinguir "tabla vacía" de "tabla
/// inexistente" sin ambigüedad.
pub struct CollectionStatus {
    pub name: String,
    pub declared_columns: usize,
    pub exists: bool,
    pub row_count: Option<i64>,
}

/// Colecciones declaradas en `program`, ordenadas por nombre -- mismo
/// `Checker::build_symbols` (sin instanciar ningún `Db` real) que
/// `migrate.rs`/`codegen::postgres_emit` ya usan para DDL estático.
/// `declared_columns` cuenta todo menos `"id"` (implícito en toda
/// colección, nunca parte del `field_list` de `db { ... }` en sí).
fn declared_collections(program: &Program) -> Result<Vec<(String, usize)>, String> {
    let (checker, errors) = Checker::build_symbols(program);
    if let Some(e) = errors.into_iter().next() {
        return Err(format!("programa inválido: {e}"));
    }
    let mut out: Vec<(String, usize)> = checker
        .db_collections()
        .iter()
        .filter_map(|(name, ty)| match ty {
            Type::Struct { fields, .. } => Some((name.clone(), fields.iter().filter(|f| f.name != "id").count())),
            _ => None,
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// `db_path` inexistente (el caso común en un checkout fresco, antes del
/// primer `linkc serve`) NUNCA es un error acá -- es exactamente lo que
/// `exists: false` en cada colección ya representa. Abre de solo lectura
/// (`SQLITE_OPEN_READ_ONLY`) cuando el archivo sí existe -- ninguna
/// sentencia de esta función puede ser DDL/DML por construcción (`rusqlite`
/// rechazaría un intento de escritura contra esa conexión de todos modos,
/// pero la intención queda expresada en el flag, no solo en qué SQL se
/// manda).
pub fn inspect_sqlite(program: &Program, db_path: &Path) -> Result<Vec<CollectionStatus>, String> {
    let collections = declared_collections(program)?;
    if !db_path.exists() {
        return Ok(collections
            .into_iter()
            .map(|(name, declared_columns)| CollectionStatus { name, declared_columns, exists: false, row_count: None })
            .collect());
    }
    let connection = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("no se pudo abrir '{}' de solo lectura: {e}", db_path.display()))?;
    let mut out = Vec::with_capacity(collections.len());
    for (name, declared_columns) in collections {
        let exists = sqlite_table_exists(&connection, &name);
        let row_count = if exists {
            Some(
                connection
                    .query_row(&format!("SELECT COUNT(*) FROM \"{name}\""), [], |row| row.get::<_, i64>(0))
                    .map_err(|e| format!("no se pudo contar filas de '{name}': {e}"))?,
            )
        } else {
            None
        };
        out.push(CollectionStatus { name, declared_columns, exists, row_count });
    }
    Ok(out)
}

/// Misma idea que `inspect_sqlite`, contra un PostgreSQL real -- reusa
/// `existing_columns` (`migrate.rs`) para la detección de existencia (un
/// conjunto VACÍO de columnas es indistinguible de "la tabla no existe":
/// toda tabla real tiene al menos `"id"`) en vez de duplicar la consulta a
/// `information_schema`.
pub fn inspect_postgres(program: &Program, url: &str) -> Result<Vec<CollectionStatus>, String> {
    let collections = declared_collections(program)?;
    let client = connect_postgres_client(url)?;
    let backend = Backend::Postgres { client: parking_lot::ReentrantMutex::new(std::cell::RefCell::new(client)), url: url.to_string() };
    let mut out = Vec::with_capacity(collections.len());
    for (name, declared_columns) in collections {
        let exists = !crate::migrate::existing_columns(&backend, &name)?.is_empty();
        let row_count = if exists {
            let rows = backend.query(&format!("SELECT COUNT(*) FROM \"{name}\""), &[], &[ColumnKind::Int])?;
            match rows.first().and_then(|r| r.first()) {
                Some(Cell::Int(n)) => Some(*n),
                other => return Err(format!("'{name}': COUNT(*) devolvió algo inesperado: {other:?}")),
            }
        } else {
            None
        };
        out.push(CollectionStatus { name, declared_columns, exists, row_count });
    }
    Ok(out)
}
