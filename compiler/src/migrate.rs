// `linkc migrate --dry-run` (GRAMMAR.md §3.97): reporta el DDL EXACTO que
// `linkc serve --db postgres://...` ejecutaría al conectar, sin aplicar
// nada -- ninguna sentencia de este módulo se ejecuta, todas son texto.
//
// Deliberadamente reusa las MISMAS funciones puras de generación de SQL que
// ya usa el runtime real (`codegen::postgres_emit::create_postgres_table_sql`/
// `alter_table_add_column_postgres`, `runtime::db::create_index_statements`)
// -- si este módulo tuviera su propia copia del DDL, las dos podrían
// divergir con el tiempo (la clase de bug que este proyecto viene evitando
// desde GRAMMAR.md §3.9), y el reporte de "lo que se ejecutaría" dejaría de
// ser una promesa confiable.
//
// Solo PostgreSQL: SQLite ya reporta el diff exacto al conectar de verdad
// (`check_schema_matches`, GRAMMAR.md §3.17) -- antes de tocar nada, con un
// mensaje que nombra esperado vs. encontrado -- así que un modo aparte no
// agrega nada ahí.

use crate::ast::Program;
use crate::checker::Checker;
use crate::codegen::postgres_emit::{alter_table_add_column_postgres, create_postgres_table_sql};
use crate::runtime::db::{
    check_fields_by_collection, column_aliases_by_collection, composite_unique_by_collection, connect_postgres_client, create_composite_unique_statements,
    create_index_statements, index_fields_by_collection, type_checks_by_collection, validate_existing_id_column, IdKind,
};
use crate::runtime::store::{Backend, Cell, ColumnKind};
use crate::types::{FieldType, Type};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// El reporte completo (texto plano, ya formateado para imprimir tal cual)
/// de lo que `linkc serve --db <url>` ejecutaría en esta base AHORA MISMO,
/// sin ejecutar ninguna de esas sentencias. Conecta de verdad (necesita
/// leer `information_schema.columns` para saber qué ya existe), pero solo
/// hace `SELECT` -- nunca `CREATE`/`ALTER`.
pub fn dry_run_postgres(program: &Program, url: &str, schema: Option<&str>) -> Result<String, String> {
    let (body, _any_change) = compute_postgres_diff(program, url, schema)?;
    Ok(format!(
        "-- 'linkc migrate --dry-run': DDL que 'linkc serve'/'linkc serve-all' ejecutaría\n\
         -- al conectar a esta base AHORA MISMO -- nada de esto se aplicó.\n\n{body}"
    ))
}

/// Cuerpo compartido de `dry_run_postgres` y `generate_migration` (GRAMMAR.md
/// §3.252, PLAN.md §9.21 Fase 4 ítem 14) -- el mismo DDL, calculado UNA sola
/// vez, con dos usos distintos: mostrarlo (`--dry-run`) o guardarlo en un
/// archivo de migración versionado (`migrate generate`). Devuelve el texto
/// (DDL real + comentarios SQL `--`, ejecutable tal cual con `batch_execute`)
/// y si hay algún cambio de verdad -- `generate_migration` usa ese booleano
/// para decidir si vale la pena crear un archivo.
fn compute_postgres_diff(program: &Program, url: &str, schema: Option<&str>) -> Result<(String, bool), String> {
    let (checker, errors) = Checker::build_symbols(program);
    if let Some(e) = errors.into_iter().next() {
        return Err(format!("programa inválido: {e}"));
    }
    let simple_enums: HashSet<String> = checker
        .enums
        .iter()
        .filter(|(_, decl)| decl.variants.iter().all(|v| v.fields.is_none()))
        .map(|(k, _)| k.clone())
        .collect();

    let client = connect_postgres_client(url, schema)?;
    let backend = Backend::postgres(client, url, schema, 1);
    let checks_by_collection = check_fields_by_collection(program, &checker);
    let type_checks_by_collection_map = type_checks_by_collection(program, &checker);
    let indexed_by_collection = index_fields_by_collection(program, &checker);
    let composite_unique_by_collection_map = composite_unique_by_collection(program, &checker);
    let aliases_by_collection = column_aliases_by_collection(program, &checker);
    let empty_aliases = HashMap::new();

    let mut out = String::new();

    let mut any_change = false;
    for (coll_name, elem_ty) in checker.db_collections() {
        let Type::Struct { fields, .. } = elem_ty else { continue };
        let non_id: Vec<FieldType> = fields.iter().filter(|f| f.name != "id").cloned().collect();
        let id_field_ty = &fields.iter().find(|f| f.name == "id").expect("validate_db_element_type ya garantizó 'id'").ty;
        let id_kind = IdKind::from_field_type(id_field_ty);
        let aliases = aliases_by_collection.get(coll_name).unwrap_or(&empty_aliases);
        let id_col = aliases.get("id").map(String::as_str).unwrap_or("id");

        if let Err(e) = validate_existing_id_column(&backend, coll_name, id_col, id_kind) {
            out.push_str(&format!("-- '{coll_name}': ¡ESTO FALLARÍA AL CONECTAR DE VERDAD! {e}\n\n"));
            any_change = true;
            continue;
        }

        let existing = existing_columns(&backend, coll_name)?;

        if existing.is_empty() {
            any_change = true;
            let checks = checks_by_collection.get(coll_name).cloned().unwrap_or_default();
            let type_checks = type_checks_by_collection_map.get(coll_name).cloned().unwrap_or_default();
            out.push_str(&format!("-- '{coll_name}': tabla nueva\n"));
            out.push_str(&create_postgres_table_sql(coll_name, id_field_ty, &non_id, &simple_enums, &checks, &type_checks, aliases));
            out.push_str("\n\n");
        } else {
            let declared_names: Vec<&str> = non_id.iter().map(|f| aliases.get(&f.name).map(String::as_str).unwrap_or(f.name.as_str())).collect();
            if !declared_names.is_empty() && !declared_names.iter().any(|n| existing.contains(*n)) {
                out.push_str(&format!(
                    "-- ADVERTENCIA '{coll_name}': la tabla ya existe pero NINGUNA columna declarada ([{}]) \
                     coincide con las que ya tiene ([{}]) -- podría pertenecer a otro programa (GRAMMAR.md §3.94).\n",
                    declared_names.join(", "),
                    { let mut v: Vec<&str> = existing.iter().map(String::as_str).collect(); v.sort(); v.join(", ") },
                ));
            }
            let missing: Vec<&FieldType> = non_id.iter().filter(|f| {
                let col_name = aliases.get(&f.name).map(String::as_str).unwrap_or(&f.name);
                !existing.contains(col_name)
            }).collect();
            if missing.is_empty() {
                out.push_str(&format!("-- '{coll_name}': sin cambios (todas las columnas declaradas ya existen)\n\n"));
            } else {
                any_change = true;
                out.push_str(&format!("-- '{coll_name}': {} columna(s) nueva(s), agregada(s) SIEMPRE nullable (GRAMMAR.md §3.17)\n", missing.len()));
                for f in missing {
                    let col_name = aliases.get(&f.name).map(String::as_str);
                    out.push_str(&alter_table_add_column_postgres(coll_name, f, &simple_enums, col_name));
                    out.push('\n');
                }
                out.push('\n');
            }
        }

        if let Some(indexed) = indexed_by_collection.get(coll_name) {
            for stmt in create_index_statements(coll_name, indexed) {
                out.push_str(&stmt);
                out.push_str(";\n");
            }
            out.push('\n');
        }

        if let Some(sets) = composite_unique_by_collection_map.get(coll_name) {
            for stmt in create_composite_unique_statements(coll_name, sets) {
                out.push_str(&stmt);
                out.push_str(";\n");
            }
            out.push('\n');
        }
    }

    // GRAMMAR.md §3.229: lo que NINGÚN DDL puede arreglar -- una columna
    // que existe pero con un tipo que el runtime no va a poder leer o
    // escribir. Como comentarios SQL, para que el archivo siga siendo
    // ejecutable tal cual y el aviso no se pierda en un pipe.
    let type_issues = crate::schema_check::check_program(program, &backend)?;
    if !type_issues.is_empty() {
        out.push_str("-- Problemas de TIPO entre lo declarado y las columnas reales (no los arregla ninguna migración\n");
        out.push_str("-- automática -- hay que cambiar el .link o la columna a mano, GRAMMAR.md §3.229):\n");
        for issue in &type_issues {
            out.push_str(&format!("--   {}\n", issue.render()));
        }
        out.push('\n');
    }
    if !any_change {
        out.push_str("-- Nada que migrar: el schema declarado ya coincide con lo que hay en la base.\n");
    }
    out.push_str(
        "\n-- Límite honesto (GRAMMAR.md §3.97): esta migración nunca es destructiva, no hace falta \
         --allow-destructive -- Postgres solo CREA tablas nuevas y AGREGA columnas nullable, nunca \
         borra ni cambia el tipo de nada existente (ver la matriz completa en GRAMMAR.md §3.17).\n",
    );
    Ok((out, any_change))
}

/// `linkc migrate generate <archivo.link> <nombre> --db <url>` (PLAN.md
/// §9.21 Fase 4 ítem 14, GRAMMAR.md §3.252): calcula el mismo diff que
/// `--dry-run` y, si hay algo que migrar, lo guarda en un archivo NUEVO y
/// numerado dentro de `migrations_dir` (`0001_<nombre>.sql`,
/// `0002_<nombre>.sql`, ...) -- nunca lo aplica, workflow explícito y
/// APARTE del auto-apply de siempre en `linkc serve` [decisión del
/// usuario: los dos conviven, este comando es opcional]. `Ok(None)` sin
/// crear ningún archivo si no hay ningún cambio -- generar una migración
/// vacía no tiene sentido.
pub fn generate_migration(program: &Program, url: &str, schema: Option<&str>, name: &str, migrations_dir: &Path) -> Result<Option<PathBuf>, String> {
    let (body, any_change) = compute_postgres_diff(program, url, schema)?;
    if !any_change {
        return Ok(None);
    }
    std::fs::create_dir_all(migrations_dir).map_err(|e| format!("no se pudo crear '{}': {e}", migrations_dir.display()))?;
    let next = next_migration_number(migrations_dir)?;
    let slug = sanitize_migration_name(name);
    let filename = format!("{next:04}_{slug}.sql");
    let path = migrations_dir.join(&filename);
    let header = format!(
        "-- Migración generada por 'linkc migrate generate' -- {filename}\n\
         -- Ejecutable tal cual: 'linkc migrate apply' la corre como un solo batch atómico.\n\n"
    );
    std::fs::write(&path, format!("{header}{body}")).map_err(|e| format!("no se pudo escribir '{}': {e}", path.display()))?;
    Ok(Some(path))
}

/// Mismo criterio de slug que cualquier generador de migraciones conocido
/// (Rails/Django/Prisma): minúsculas, solo alfanumérico, todo separador se
/// colapsa a UN guion bajo. Nunca vacío -- `"migration"` como default si el
/// nombre no dejó ningún caracter válido.
fn sanitize_migration_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() {
        "migration".to_string()
    } else {
        trimmed.to_string()
    }
}

/// El próximo número secuencial de 4 dígitos -- escanea `migrations_dir`
/// por archivos `NNNN_...` y toma el máximo + 1. `1` si el directorio no
/// existe todavía o está vacío (primera migración del proyecto).
fn next_migration_number(migrations_dir: &Path) -> Result<u32, String> {
    let mut max = 0u32;
    if migrations_dir.exists() {
        let entries = std::fs::read_dir(migrations_dir).map_err(|e| format!("no se pudo leer '{}': {e}", migrations_dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(prefix) = name.split('_').next() {
                if let Ok(n) = prefix.parse::<u32>() {
                    max = max.max(n);
                }
            }
        }
    }
    Ok(max + 1)
}

/// `linkc migrate apply --db <url>` (PLAN.md §9.21 Fase 4 ítem 14,
/// GRAMMAR.md §3.252): aplica, EN ORDEN, cada archivo de `migrations_dir`
/// que todavía no figure en la tabla de estado `"_link_migrations"` --
/// creándola si hace falta (idempotente, mismo criterio que el resto del
/// DDL de este proyecto). Cada migración corre como un solo
/// `batch_execute` -- el protocolo simple de Postgres ya envuelve varias
/// sentencias de un mismo mensaje en una transacción implícita, así que un
/// archivo con tres `ALTER TABLE` es todo-o-nada por sí solo, sin
/// `BEGIN`/`COMMIT` explícito -- seguido del `INSERT` que la marca
/// aplicada, EN LA MISMA llamada: si el DDL falla, el `INSERT` tampoco
/// corre, así que un archivo nunca queda "medio aplicado" en el estado.
/// Se corta en la primera que falle: las migraciones ya aplicadas en
/// llamadas ANTERIORES quedan aplicadas, las que faltan después de la que
/// falló ni se intentan -- mismo principio que cualquier migrador
/// (Rails/Django/Prisma), nunca "seguir de largo ante un error".
pub fn apply_migrations(url: &str, schema: Option<&str>, migrations_dir: &Path) -> Result<Vec<String>, String> {
    let client = connect_postgres_client(url, schema)?;
    let backend = Backend::postgres(client, url, schema, 1);
    backend.execute_ddl(
        "CREATE TABLE IF NOT EXISTS \"_link_migrations\" (\
            \"id\" BIGSERIAL PRIMARY KEY, \
            \"name\" TEXT NOT NULL UNIQUE, \
            \"applied_at\" TIMESTAMPTZ NOT NULL DEFAULT now()\
        )",
    )?;

    let applied: HashSet<String> = backend
        .query("SELECT \"name\" FROM \"_link_migrations\"", &[], &[ColumnKind::Text])?
        .into_iter()
        .filter_map(|row| row.into_iter().next())
        .filter_map(|cell| if let Cell::Text(s) = cell { Some(s) } else { None })
        .collect();

    let mut files: Vec<(String, PathBuf)> = if migrations_dir.exists() {
        std::fs::read_dir(migrations_dir)
            .map_err(|e| format!("no se pudo leer '{}': {e}", migrations_dir.display()))?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("sql"))
            .map(|e| (e.file_name().to_string_lossy().to_string(), e.path()))
            .collect()
    } else {
        Vec::new()
    };
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut newly_applied = Vec::new();
    for (name, path) in files {
        if applied.contains(&name) {
            continue;
        }
        let sql = std::fs::read_to_string(&path).map_err(|e| format!("no se pudo leer '{}': {e}", path.display()))?;
        // Sin bind param acá -- `execute_ddl`/`batch_execute` es el
        // protocolo SIMPLE de Postgres, no acepta parámetros -- así que el
        // nombre (un nombre de archivo, no un valor de red arbitrario) se
        // escapa a mano duplicando comillas simples, el escape estándar de
        // un literal SQL.
        let escaped_name = name.replace('\'', "''");
        let combined = format!("{sql}\nINSERT INTO \"_link_migrations\" (\"name\") VALUES ('{escaped_name}');");
        backend.execute_ddl(&combined).map_err(|e| format!("'{name}' falló, se cortó acá sin tocar las siguientes -- {e}"))?;
        newly_applied.push(name);
    }
    Ok(newly_applied)
}

pub(crate) fn existing_columns(backend: &Backend, collection: &str) -> Result<HashSet<String>, String> {
    // GRAMMAR.md §3.192: mismo fix de `table_schema` que las funciones
    // equivalentes de `runtime/db.rs` -- sin esto, una tabla de OTRO schema
    // con el mismo nombre podía leerse por error (`linkc migrate --dry-run`,
    // `db export`, y el loop de `ADD COLUMN` de la auto-migración, los tres
    // reusan esta función).
    let sql = format!(
        "SELECT column_name FROM information_schema.columns WHERE table_name = {} AND table_schema = ANY(current_schemas(false))",
        backend.placeholder(1)
    );
    let rows = backend.query(&sql, &[Cell::Text(collection.to_string())], &[ColumnKind::Text])?;
    Ok(rows.into_iter().filter_map(|row| row.into_iter().next()).filter_map(|cell| if let Cell::Text(s) = cell { Some(s) } else { None }).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_migration_name_lowercases_and_collapses_separators() {
        assert_eq!(sanitize_migration_name("Adds Reviews"), "adds_reviews");
        assert_eq!(sanitize_migration_name("agrega--facturas!!"), "agrega_facturas");
        assert_eq!(sanitize_migration_name("  leading and trailing  "), "leading_and_trailing");
    }

    #[test]
    fn sanitize_migration_name_falls_back_to_migration_when_nothing_survives() {
        assert_eq!(sanitize_migration_name("---"), "migration");
        assert_eq!(sanitize_migration_name(""), "migration");
        assert_eq!(sanitize_migration_name("!@#$%"), "migration");
    }

    #[test]
    fn next_migration_number_is_one_for_an_empty_or_missing_directory() {
        let dir = std::env::temp_dir().join(format!("linkc-migrate-test-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(next_migration_number(&dir).unwrap(), 1, "directorio inexistente");
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(next_migration_number(&dir).unwrap(), 1, "directorio vacío");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn next_migration_number_takes_the_max_prefix_plus_one_regardless_of_creation_order() {
        let dir = std::env::temp_dir().join(format!("linkc-migrate-test-seq-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("0001_first.sql"), "").unwrap();
        std::fs::write(dir.join("0003_third.sql"), "").unwrap();
        std::fs::write(dir.join("0002_second.sql"), "").unwrap();
        assert_eq!(next_migration_number(&dir).unwrap(), 4, "toma el máximo existente (3) + 1, sin importar el orden en que se crearon");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
