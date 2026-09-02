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
    check_fields_by_collection, composite_unique_by_collection, connect_postgres_client, create_composite_unique_statements,
    create_index_statements, index_fields_by_collection, type_checks_by_collection, validate_existing_id_column, IdKind,
};
use crate::runtime::store::{Backend, Cell, ColumnKind};
use crate::types::{FieldType, Type};
use std::cell::RefCell;
use std::collections::HashSet;

/// El reporte completo (texto plano, ya formateado para imprimir tal cual)
/// de lo que `linkc serve --db <url>` ejecutaría en esta base AHORA MISMO,
/// sin ejecutar ninguna de esas sentencias. Conecta de verdad (necesita
/// leer `information_schema.columns` para saber qué ya existe), pero solo
/// hace `SELECT` -- nunca `CREATE`/`ALTER`.
pub fn dry_run_postgres(program: &Program, url: &str, schema: Option<&str>) -> Result<String, String> {
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
    let backend = Backend::Postgres {
        client: parking_lot::ReentrantMutex::new(RefCell::new(client)),
        url: url.to_string(),
        schema: schema.map(str::to_string),
    };
    let checks_by_collection = check_fields_by_collection(program, &checker);
    let type_checks_by_collection_map = type_checks_by_collection(program, &checker);
    let indexed_by_collection = index_fields_by_collection(program, &checker);
    let composite_unique_by_collection_map = composite_unique_by_collection(program, &checker);

    let mut out = String::new();
    out.push_str("-- 'linkc migrate --dry-run': DDL que 'linkc serve'/'linkc serve-all' ejecutaría\n");
    out.push_str("-- al conectar a esta base AHORA MISMO -- nada de esto se aplicó.\n\n");

    let mut any_change = false;
    for (coll_name, elem_ty) in checker.db_collections() {
        let Type::Struct { fields, .. } = elem_ty else { continue };
        let non_id: Vec<FieldType> = fields.iter().filter(|f| f.name != "id").cloned().collect();
        let id_field_ty = &fields.iter().find(|f| f.name == "id").expect("validate_db_element_type ya garantizó 'id'").ty;
        let id_kind = IdKind::from_field_type(id_field_ty);

        if let Err(e) = validate_existing_id_column(&backend, coll_name, id_kind) {
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
            out.push_str(&create_postgres_table_sql(coll_name, id_field_ty, &non_id, &simple_enums, &checks, &type_checks));
            out.push_str("\n\n");
        } else {
            let declared_names: Vec<&str> = non_id.iter().map(|f| f.name.as_str()).collect();
            if !declared_names.is_empty() && !declared_names.iter().any(|n| existing.contains(*n)) {
                out.push_str(&format!(
                    "-- ADVERTENCIA '{coll_name}': la tabla ya existe pero NINGUNA columna declarada ([{}]) \
                     coincide con las que ya tiene ([{}]) -- podría pertenecer a otro programa (GRAMMAR.md §3.94).\n",
                    declared_names.join(", "),
                    { let mut v: Vec<&str> = existing.iter().map(String::as_str).collect(); v.sort(); v.join(", ") },
                ));
            }
            let missing: Vec<&FieldType> = non_id.iter().filter(|f| !existing.contains(&f.name)).collect();
            if missing.is_empty() {
                out.push_str(&format!("-- '{coll_name}': sin cambios (todas las columnas declaradas ya existen)\n\n"));
            } else {
                any_change = true;
                out.push_str(&format!("-- '{coll_name}': {} columna(s) nueva(s), agregada(s) SIEMPRE nullable (GRAMMAR.md §3.17)\n", missing.len()));
                for f in missing {
                    out.push_str(&alter_table_add_column_postgres(coll_name, f, &simple_enums));
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
    Ok(out)
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
