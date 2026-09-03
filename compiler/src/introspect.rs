//! `linkc introspect <db-url|archivo.db>` (GRAMMAR.md §3.66, PLAN.md §9.21 Fase 1 ítem 4):
//! genera un `.link` de partida a partir de una base PostgreSQL o SQLite YA EXISTENTE,
//! leyendo `information_schema` o `sqlite_master`/`PRAGMA` -- para no tener que escribir
//! cada `type`/`db{...}` a mano cuando se adopta un sistema que ya tiene datos.
//!
//! Extrae:
//! - Claves primarias (`id: Int` autoincrementales o `id: Uuid`)
//! - Tipos escalares, temporales, booleanos, decimales, arrays nativos y uuid
//! - Valores por defecto (`= now()`, `= true`, `= false`, numéricos y literales)
//! - `@autoUpdate` en columnas temporales de actualización
//! - Índices simples (`@index campo: Tipo`, `@unique campo: Tipo`)
//! - Índices compuestos (`@index(c1, c2)`, `@unique(c1, c2)` a nivel de struct)
//! - Claves foráneas (anotadas como comentario `// FK -> tabla(col)`)
//! - Restricciones CHECK (anotadas como comentario `// CHECK: (...)`)
//!
//! El resultado es un PUNTO DE PARTIDA para revisar a mano, no un `.link` listo
//! para producción sin mirarlo: cualquier columna que este módulo no pueda mapear
//! con confianza se emite como `String` con un comentario `/* TODO */` y advertencia
//! en stderr.

use crate::runtime::db::connect_postgres_client;
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Una columna real leída de la base de datos.
#[derive(Debug, Clone)]
struct Column {
    name: String,
    sql_type: String,
    udt_name: String,
    nullable: bool,
    default: Option<String>,
}

/// Resultado de introspeccionar una tabla.
pub struct TableIntrospection {
    pub link_type: String,
    pub warnings: Vec<String>,
}

/// Mapea un `data_type` de PostgreSQL al tipo c-script más cercano.
fn map_pg_type(pg_type: &str, udt_name: &str, column_name: &str) -> (&'static str, Option<String>) {
    match pg_type {
        "ARRAY" => match udt_name {
            "_int2" | "_int4" | "_int8" => ("Int[]", None),
            "_text" | "_varchar" | "_bpchar" | "_citext" => ("String[]", None),
            "_bool" => ("Bool[]", None),
            "_float4" | "_float8" => ("Float[]", None),
            other => (
                "String[]",
                Some(format!(
                    "'{column_name}' es un array de '{}' -- c-script solo lee/escribe arrays de enteros, texto, booleanos y \
                     flotantes (GRAMMAR.md §3.228); esta columna va a fallar al leer una fila real, declarala a mano o dejala fuera",
                    other.trim_start_matches('_')
                )),
            ),
        },
        "bigint" | "integer" | "smallint" => ("Int", None),
        "boolean" => ("Bool", None),
        "double precision" | "real" => ("Float", None),
        "numeric" => ("Decimal", None),
        "text" | "character varying" | "character" | "citext" => ("String", None),
        "uuid" => ("Uuid", None),
        "inet" | "cidr" => ("String", None),
        "jsonb" | "json" => (
            "String",
            Some(format!(
                "'{column_name}' es {pg_type} -- la FORMA real del JSON no se puede inferir de information_schema; \
                 declará un 'type' propio para ese shape y reemplazá 'String' acá si corresponde"
            )),
        ),
        "timestamp without time zone" | "timestamp with time zone" | "date" => ("Timestamp", None),
        "time without time zone" => (
            "String",
            Some(format!(
                "'{column_name}' es time without time zone -- 'Timestamp' de c-script es un INSTANTE completo \
                 (fecha + hora, GRAMMAR.md §3.31), no le cabe una hora suelta sin fecha; revisar a mano"
            )),
        ),
        other => (
            "String",
            Some(format!("'{column_name}' es '{other}', un tipo sin mapeo conocido -- revisado como String a mano")),
        ),
    }
}

/// Mapea un tipo SQL de SQLite al tipo c-script más cercano.
fn map_sqlite_type(sql_type: &str, col_name: &str) -> (&'static str, Option<String>) {
    let base = sql_type.split('(').next().unwrap_or(sql_type).trim().to_ascii_uppercase();
    match base.as_str() {
        "INTEGER" | "INT" | "BIGINT" | "SMALLINT" | "TINYINT" | "INT2" | "INT4" | "INT8" | "MEDIUMINT" => ("Int", None),
        "BOOLEAN" | "BOOL" => ("Bool", None),
        "REAL" | "FLOAT" | "DOUBLE" | "DOUBLE PRECISION" => ("Float", None),
        "NUMERIC" | "DECIMAL" => ("Decimal", None),
        "TEXT" | "VARCHAR" | "NVARCHAR" | "CHAR" | "CHARACTER" | "CLOB" | "CITEXT" => ("String", None),
        "TIMESTAMP" | "DATETIME" | "DATE" => ("Timestamp", None),
        "UUID" => ("Uuid", None),
        "BLOB" => (
            "String",
            Some(format!("'{col_name}' es BLOB -- c-script no tiene tipo binario crudo nativo; mapeada como String a mano")),
        ),
        "JSON" | "JSONB" => (
            "String",
            Some(format!(
                "'{col_name}' es {sql_type} -- la FORMA real del JSON no se puede inferir; declará un 'type' propio si corresponde"
            )),
        ),
        "" => (
            "String",
            Some(format!("'{col_name}' no tiene tipo declarado en SQLite -- revisado como String a mano")),
        ),
        other => (
            "String",
            Some(format!("'{col_name}' tiene tipo '{other}' en SQLite, sin mapeo directo -- revisado como String a mano")),
        ),
    }
}

/// Parsea un valor por defecto de PostgreSQL.
fn parse_pg_default(dflt: &str, ty: &str) -> Option<String> {
    let trimmed = dflt.trim();
    if trimmed.starts_with("nextval(") {
        return None;
    }
    let upper = trimmed.to_ascii_uppercase();
    if upper.starts_with("NOW()")
        || upper.starts_with("CURRENT_TIMESTAMP")
        || upper.starts_with("CLOCK_TIMESTAMP()")
        || upper.starts_with("TRANSACTION_TIMESTAMP()")
    {
        return Some("= now()".to_string());
    }
    if ty == "Bool" {
        if trimmed.starts_with("true") || trimmed.starts_with("'true'") {
            return Some("= true".to_string());
        }
        if trimmed.starts_with("false") || trimmed.starts_with("'false'") {
            return Some("= false".to_string());
        }
    }
    if ty == "Int" || ty == "Float" {
        let clean = trimmed.split("::").next().unwrap_or(trimmed).trim().trim_matches('\'');
        if clean.parse::<f64>().is_ok() {
            return Some(format!("= {clean}"));
        }
    }
    if ty == "Decimal" {
        let clean = trimmed.split("::").next().unwrap_or(trimmed).trim().trim_matches('\'');
        if clean.parse::<f64>().is_ok() {
            return Some(format!("= {clean}.toDecimal()"));
        }
    }
    if ty == "String" {
        if let Some(pos) = trimmed.strip_prefix('\'') {
            if let Some(end) = pos.find('\'') {
                let inner = &pos[..end];
                return Some(format!("= \"{inner}\""));
            }
        }
    }
    None
}

/// Parsea un valor por defecto de SQLite.
fn parse_sqlite_default(dflt: &str, ty: &str) -> Option<String> {
    let trimmed = dflt.trim();
    let upper = trimmed.to_ascii_uppercase();
    if upper == "CURRENT_TIMESTAMP"
        || upper == "CURRENT_DATE"
        || upper == "CURRENT_TIME"
        || upper.starts_with("DATETIME('NOW'")
        || upper.starts_with("NOW(")
    {
        return Some("= now()".to_string());
    }
    if ty == "Bool" {
        if upper == "1" || upper == "'1'" || upper == "TRUE" || upper == "'TRUE'" {
            return Some("= true".to_string());
        }
        if upper == "0" || upper == "'0'" || upper == "FALSE" || upper == "'FALSE'" {
            return Some("= false".to_string());
        }
    }
    if ty == "Int" || ty == "Float" {
        let clean = trimmed.trim_matches('\'').trim_matches('"');
        if clean.parse::<f64>().is_ok() {
            return Some(format!("= {clean}"));
        }
    }
    if ty == "Decimal" {
        let clean = trimmed.trim_matches('\'').trim_matches('"');
        if clean.parse::<f64>().is_ok() {
            return Some(format!("= {clean}.toDecimal()"));
        }
    }
    if ty == "String" {
        let unquoted = if (trimmed.starts_with('\'') && trimmed.ends_with('\''))
            || (trimmed.starts_with('"') && trimmed.ends_with('"'))
        {
            &trimmed[1..trimmed.len() - 1]
        } else {
            trimmed
        };
        return Some(format!("= \"{unquoted}\""));
    }
    None
}

/// Extrae cláusulas CHECK del DDL de creación de una tabla en SQLite.
fn extract_sqlite_checks(table_sql: &str) -> Vec<String> {
    let mut checks = Vec::new();
    let mut rest = table_sql;
    while let Some(idx) = rest.to_ascii_uppercase().find("CHECK") {
        let after = rest[idx + 5..].trim_start();
        if after.starts_with('(') {
            let mut depth = 0;
            let mut end = None;
            for (i, c) in after.char_indices() {
                if c == '(' {
                    depth += 1;
                } else if c == ')' {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(i);
                        break;
                    }
                }
            }
            if let Some(e) = end {
                let clause = &after[1..e];
                checks.push(clause.trim().to_string());
                rest = &after[e + 1..];
                continue;
            }
        }
        rest = &rest[idx + 5..];
    }
    checks
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// `snake_case`/`kebab-case` -> `PascalCase`, para el nombre del `type`.
fn to_pascal_case(table_name: &str) -> String {
    table_name.split(['_', '-']).filter(|s| !s.is_empty()).map(capitalize).collect()
}

/// Introspecciona una tabla en PostgreSQL.
fn introspect_table(client: &mut postgres::Client, table: &str) -> Result<TableIntrospection, String> {
    let pk_rows = client
        .query(
            "SELECT kcu.column_name \
             FROM information_schema.table_constraints tc \
             JOIN information_schema.key_column_usage kcu \
               ON tc.constraint_name = kcu.constraint_name AND tc.table_schema = kcu.table_schema \
             WHERE tc.table_schema = ANY(current_schemas(false)) AND tc.table_name = $1 AND tc.constraint_type = 'PRIMARY KEY'",
            &[&table],
        )
        .map_err(|e| format!("no se pudo leer la clave primaria de '{table}': {e}"))?;
    let pk_columns: Vec<String> = pk_rows.iter().map(|r| r.get::<_, String>(0)).collect();

    let col_rows = client
        .query(
            "SELECT column_name, data_type, is_nullable, udt_name, column_default \
             FROM information_schema.columns \
             WHERE table_schema = ANY(current_schemas(false)) AND table_name = $1 \
             ORDER BY ordinal_position",
            &[&table],
        )
        .map_err(|e| format!("no se pudo leer las columnas de '{table}': {e}"))?;
    let columns: Vec<Column> = col_rows
        .iter()
        .map(|r| Column {
            name: r.get::<_, String>(0),
            sql_type: r.get::<_, String>(1),
            nullable: r.get::<_, String>(2) == "YES",
            udt_name: r.get::<_, String>(3),
            default: r.get::<_, Option<String>>(4),
        })
        .collect();

    // Foreign Keys
    let fk_rows = client
        .query(
            "SELECT kcu.column_name, ccu.table_name, ccu.column_name \
             FROM information_schema.table_constraints tc \
             JOIN information_schema.key_column_usage kcu \
               ON tc.constraint_name = kcu.constraint_name AND tc.table_schema = kcu.table_schema \
             JOIN information_schema.constraint_column_usage ccu \
               ON ccu.constraint_name = tc.constraint_name AND ccu.table_schema = tc.table_schema \
             WHERE tc.table_schema = ANY(current_schemas(false)) AND tc.table_name = $1 AND tc.constraint_type = 'FOREIGN KEY'",
            &[&table],
        )
        .unwrap_or_default();
    let mut foreign_keys: HashMap<String, String> = HashMap::new();
    for r in &fk_rows {
        let col: String = r.get(0);
        let f_tbl: String = r.get(1);
        let f_col: String = r.get(2);
        foreign_keys.insert(col, format!("// FK -> {f_tbl}({f_col})"));
    }

    // CHECK constraints
    let check_rows = client
        .query(
            "SELECT cc.check_clause \
             FROM information_schema.table_constraints tc \
             JOIN information_schema.check_constraints cc \
               ON tc.constraint_name = cc.constraint_name AND tc.table_schema = cc.table_schema \
             WHERE tc.table_schema = ANY(current_schemas(false)) AND tc.table_name = $1 AND tc.constraint_type = 'CHECK' \
               AND cc.check_clause NOT LIKE '%IS NOT NULL'",
            &[&table],
        )
        .unwrap_or_default();
    let checks: Vec<String> = check_rows.iter().map(|r| r.get::<_, String>(0)).collect();

    // Indices
    let idx_rows = client
        .query(
            "SELECT i.relname, ix.indisunique, array_to_string(array_agg(a.attname ORDER BY k.n), ',') \
             FROM pg_index ix \
             JOIN pg_class t ON t.oid = ix.indrelid \
             JOIN pg_class i ON i.oid = ix.indexrelid \
             JOIN pg_namespace n ON n.oid = t.relnamespace \
             CROSS JOIN LATERAL unnest(ix.indkey) WITH ORDINALITY AS k(attnum, n) \
             JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = k.attnum \
             WHERE n.nspname = ANY(current_schemas(false)) AND t.relname = $1 AND NOT ix.indisprimary \
             GROUP BY i.relname, ix.indisunique",
            &[&table],
        )
        .unwrap_or_default();

    let mut single_uniques = HashSet::new();
    let mut single_indices = HashSet::new();
    let mut multi_uniques = Vec::new();
    let mut multi_indices = Vec::new();

    for r in &idx_rows {
        let is_unique: bool = r.get(1);
        let cols_str: String = r.get(2);
        let cols: Vec<String> = cols_str.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        if cols.len() == 1 && cols[0] == "id" {
            continue;
        }
        if cols.len() == 1 {
            let col = cols[0].clone();
            if is_unique {
                single_uniques.insert(col);
            } else {
                single_indices.insert(col);
            }
        } else if cols.len() > 1 {
            if is_unique {
                multi_uniques.push(cols);
            } else {
                multi_indices.push(cols);
            }
        }
    }

    // Single indices redundant with single unique are excluded
    for u in &single_uniques {
        single_indices.remove(u);
    }

    let mut warnings = Vec::new();
    let mut fields = Vec::new();

    if pk_columns.len() == 1 && pk_columns[0] == "id" {
        let id_pg_type = columns.iter().find(|c| c.name == "id").map(|c| c.sql_type.as_str());
        match id_pg_type {
            Some("uuid") => fields.push("  id: Uuid,".to_string()),
            Some("bigint" | "integer" | "smallint") | None => fields.push("  id: Int,".to_string()),
            Some(other) => {
                warnings.push(format!(
                    "la clave primaria de '{table}' se llama \"id\" pero en PostgreSQL es '{other}' -- c-script solo \
                     soporta 'id: Int' (BIGSERIAL/INTEGER/SMALLSERIAL) o 'id: Uuid' (columna 'uuid' nativa) como PK; \
                     esta tabla no se puede adoptar tal cual con esta PK, 'linkc serve'/'linkc migrate --dry-run' \
                     la van a rechazar al conectar"
                ));
                fields.push("  id: Int,".to_string());
            }
        }
    } else if pk_columns.is_empty() {
        warnings.push(format!("la tabla '{table}' no tiene clave primaria -- c-script REQUIERE una columna \"id\" entera autoincremental; agregala antes de usar esta tabla"));
        fields.push("  id: Int, // TODO: la tabla no tenía PK, agregar una columna \"id\" real".to_string());
    } else if pk_columns != ["id"] {
        warnings.push(format!(
            "la clave primaria de '{table}' es {pk_columns:?}, no simplemente \"id\" -- c-script solo soporta una PK entera llamada \"id\"; revisar a mano"
        ));
        fields.push("  id: Int, // TODO: la PK real de esta tabla no es (solo) \"id\", revisar".to_string());
    }

    for col in &columns {
        if col.name == "id" {
            continue;
        }
        let (base_ty, warning) = map_pg_type(&col.sql_type, &col.udt_name, &col.name);
        if let Some(w) = warning {
            warnings.push(w);
        }
        let ty = if col.nullable { format!("{base_ty}?") } else { base_ty.to_string() };
        let default_val = col.default.as_deref().and_then(|d| parse_pg_default(d, base_ty));
        let is_auto_update = (col.name == "updated_at" || col.name == "updatedAt")
            && default_val.as_deref() == Some("= now()");

        let mut annotations = Vec::new();
        if is_auto_update {
            annotations.push("@autoUpdate");
        }
        if single_uniques.contains(&col.name) {
            annotations.push("@unique");
        } else if single_indices.contains(&col.name) {
            annotations.push("@index");
        }

        let prefix = if annotations.is_empty() { String::new() } else { format!("{} ", annotations.join(" ")) };
        let dflt_suffix = match &default_val {
            Some(d) => format!(" {d}"),
            None => String::new(),
        };
        let fk_comment = match foreign_keys.get(&col.name) {
            Some(fk) => format!(" {fk}"),
            None => String::new(),
        };

        fields.push(format!("  {}{}: {}{},{fk_comment}", prefix, col.name, ty, dflt_suffix));
    }

    for check in &checks {
        fields.push(format!("  // CHECK: {check}"));
    }

    let mut type_annotations = Vec::new();
    for cols in &multi_uniques {
        type_annotations.push(format!("@unique({})", cols.join(", ")));
    }
    for cols in &multi_indices {
        type_annotations.push(format!("@index({})", cols.join(", ")));
    }

    let type_name = to_pascal_case(table);
    let header = if type_annotations.is_empty() {
        String::new()
    } else {
        format!("{}\n", type_annotations.join("\n"))
    };
    let link_type = format!("{header}type {type_name} = {{\n{}\n}}", fields.join("\n"));
    Ok(TableIntrospection { link_type, warnings })
}

/// Introspecciona una tabla en SQLite.
fn introspect_sqlite_table(conn: &Connection, table: &str) -> Result<TableIntrospection, String> {
    // 1. Columnas y PK
    let pragma_sql = format!("PRAGMA table_info(\"{table}\")");
    let mut stmt = conn.prepare(&pragma_sql).map_err(|e| format!("no se pudo leer table_info de '{table}': {e}"))?;
    let col_rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,         // name
                row.get::<_, String>(2)?,         // type
                row.get::<_, i64>(3)? != 0,       // notnull
                row.get::<_, Option<String>>(4)?, // dflt_value
                row.get::<_, i64>(5)?,            // pk
            ))
        })
        .map_err(|e| format!("error al consultar columnas de '{table}': {e}"))?;

    let mut columns = Vec::new();
    let mut pk_columns = Vec::new();
    for r in col_rows {
        let (name, sql_type, notnull, dflt_value, pk) = r.map_err(|e| e.to_string())?;
        if pk > 0 {
            pk_columns.push((name.clone(), sql_type.clone()));
        }
        columns.push(Column {
            name,
            sql_type,
            udt_name: String::new(),
            nullable: !notnull,
            default: dflt_value,
        });
    }

    // 2. Foreign Keys
    let fk_pragma = format!("PRAGMA foreign_key_list(\"{table}\")");
    let mut fk_stmt = conn.prepare(&fk_pragma).unwrap();
    let fk_rows = fk_stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(2)?, // table
            row.get::<_, String>(3)?, // from
            row.get::<_, String>(4)?, // to
        ))
    });
    let mut foreign_keys: HashMap<String, String> = HashMap::new();
    if let Ok(rows) = fk_rows {
        for r in rows.flatten() {
            let (f_tbl, from, to) = r;
            foreign_keys.insert(from, format!("// FK -> {f_tbl}({to})"));
        }
    }

    // 3. Indices
    let idx_pragma = format!("PRAGMA index_list(\"{table}\")");
    let mut idx_stmt = conn.prepare(&idx_pragma).unwrap();
    let idx_rows = idx_stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(1)?,   // name
            row.get::<_, i64>(2)? != 0, // unique
            row.get::<_, String>(3)?,   // origin
        ))
    });

    let mut single_uniques = HashSet::new();
    let mut single_indices = HashSet::new();
    let mut multi_uniques = Vec::new();
    let mut multi_indices = Vec::new();

    if let Ok(rows) = idx_rows {
        for r in rows.flatten() {
            let (name, is_unique, origin) = r;
            if origin == "pk" {
                continue;
            }
            let info_pragma = format!("PRAGMA index_info(\"{name}\")");
            let mut info_stmt = conn.prepare(&info_pragma).unwrap();
            let cols_res = info_stmt.query_map([], |row| row.get::<_, String>(2));
            if let Ok(cols_iter) = cols_res {
                let cols: Vec<String> = cols_iter.flatten().collect();
                if cols.len() == 1 && cols[0] == "id" {
                    continue;
                }
                if cols.len() == 1 {
                    let col = cols[0].clone();
                    if is_unique {
                        single_uniques.insert(col);
                    } else {
                        single_indices.insert(col);
                    }
                } else if cols.len() > 1 {
                    if is_unique {
                        multi_uniques.push(cols);
                    } else {
                        multi_indices.push(cols);
                    }
                }
            }
        }
    }

    for u in &single_uniques {
        single_indices.remove(u);
    }

    // 4. CHECK constraints del DDL
    let mut checks = Vec::new();
    if let Ok(mut sql_stmt) = conn.prepare("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?") {
        if let Ok(Some(table_sql)) = sql_stmt.query_row([table], |r| r.get::<_, Option<String>>(0)) {
            checks = extract_sqlite_checks(&table_sql);
        }
    }

    let mut warnings = Vec::new();
    let mut fields = Vec::new();

    if pk_columns.len() == 1 && pk_columns[0].0 == "id" {
        let upper_ty = pk_columns[0].1.to_ascii_uppercase();
        if upper_ty.contains("INT") || upper_ty.is_empty() {
            fields.push("  id: Int,".to_string());
        } else if upper_ty == "UUID" {
            fields.push("  id: Uuid,".to_string());
        } else {
            warnings.push(format!(
                "la clave primaria de '{table}' se llama \"id\" pero en SQLite es '{upper_ty}' -- c-script solo soporta 'id: Int' o 'id: Uuid'; revisar a mano"
            ));
            fields.push("  id: Int,".to_string());
        }
    } else if pk_columns.is_empty() {
        warnings.push(format!("la tabla '{table}' no tiene clave primaria -- c-script REQUIERE una columna \"id\" entera autoincremental; agregala antes de usar esta tabla"));
        fields.push("  id: Int, // TODO: la tabla no tenía PK, agregar una columna \"id\" real".to_string());
    } else {
        let names: Vec<&str> = pk_columns.iter().map(|(n, _)| n.as_str()).collect();
        warnings.push(format!(
            "la clave primaria de '{table}' es {names:?}, no simplemente \"id\" -- c-script solo soporta una PK entera llamada \"id\"; revisar a mano"
        ));
        fields.push("  id: Int, // TODO: la PK real de esta tabla no es (solo) \"id\", revisar".to_string());
    }

    for col in &columns {
        if col.name == "id" {
            continue;
        }
        let (base_ty, warning) = map_sqlite_type(&col.sql_type, &col.name);
        if let Some(w) = warning {
            warnings.push(w);
        }
        let ty = if col.nullable { format!("{base_ty}?") } else { base_ty.to_string() };
        let default_val = col.default.as_deref().and_then(|d| parse_sqlite_default(d, base_ty));
        let is_auto_update = (col.name == "updated_at" || col.name == "updatedAt")
            && default_val.as_deref() == Some("= now()");

        let mut annotations = Vec::new();
        if is_auto_update {
            annotations.push("@autoUpdate");
        }
        if single_uniques.contains(&col.name) {
            annotations.push("@unique");
        } else if single_indices.contains(&col.name) {
            annotations.push("@index");
        }

        let prefix = if annotations.is_empty() { String::new() } else { format!("{} ", annotations.join(" ")) };
        let dflt_suffix = match &default_val {
            Some(d) => format!(" {d}"),
            None => String::new(),
        };
        let fk_comment = match foreign_keys.get(&col.name) {
            Some(fk) => format!(" {fk}"),
            None => String::new(),
        };

        fields.push(format!("  {}{}: {}{},{fk_comment}", prefix, col.name, ty, dflt_suffix));
    }

    for check in &checks {
        fields.push(format!("  // CHECK: {check}"));
    }

    let mut type_annotations = Vec::new();
    for cols in &multi_uniques {
        type_annotations.push(format!("@unique({})", cols.join(", ")));
    }
    for cols in &multi_indices {
        type_annotations.push(format!("@index({})", cols.join(", ")));
    }

    let type_name = to_pascal_case(table);
    let header = if type_annotations.is_empty() {
        String::new()
    } else {
        format!("{}\n", type_annotations.join("\n"))
    };
    let link_type = format!("{header}type {type_name} = {{\n{}\n}}", fields.join("\n"));
    Ok(TableIntrospection { link_type, warnings })
}

/// Genera un `.link` de partida desde una base SQLite.
pub fn generate_link_from_sqlite(path_or_url: &str) -> Result<(String, Vec<String>), String> {
    let clean_path = path_or_url.strip_prefix("sqlite://").unwrap_or(path_or_url);
    let path = Path::new(clean_path);
    if !path.exists() {
        return Err(format!("el archivo SQLite '{clean_path}' no existe"));
    }
    let conn = Connection::open(path)
        .map_err(|e| format!("no se pudo abrir la base SQLite en '{clean_path}': {e}"))?;

    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name NOT LIKE '_link_%' ORDER BY name")
        .map_err(|e| format!("no se pudo listar las tablas: {e}"))?;
    let tables_res = stmt.query_map([], |row| row.get::<_, String>(0));
    let tables: Vec<String> = match tables_res {
        Ok(iter) => iter.flatten().collect(),
        Err(e) => return Err(format!("error al leer tablas: {e}")),
    };

    if tables.is_empty() {
        return Err(format!("la base SQLite '{clean_path}' no tiene ninguna tabla -- nada para introspeccionar"));
    }

    let mut type_blocks = Vec::new();
    let mut db_lines = Vec::new();
    let mut all_warnings = Vec::new();

    for table in &tables {
        let TableIntrospection { link_type, warnings } = introspect_sqlite_table(&conn, table)?;
        type_blocks.push(link_type);
        let type_name = to_pascal_case(table);
        db_lines.push(format!("  {table}: {type_name}[],"));
        all_warnings.extend(warnings.into_iter().map(|w| format!("{table}: {w}")));
    }

    let header = "// Generado por 'linkc introspect' -- PUNTO DE PARTIDA, no un .link listo\n\
                  // para producción sin revisar. Cualquier línea con '// TODO' o reportada\n\
                  // como advertencia en stderr necesita una decisión manual.\n\n";
    let content = format!("{header}{}\n\ndb {{\n{}\n}}\n", type_blocks.join("\n\n"), db_lines.join("\n"));
    Ok((content, all_warnings))
}

/// Genera un `.link` de partida desde TODAS las tablas base del schema
/// `public` de la base en `url` -- `(contenido, advertencias)`.
pub fn generate_link_from_postgres(url: &str) -> Result<(String, Vec<String>), String> {
    let mut client = connect_postgres_client(url, None)?;
    let table_rows = client
        .query(
            "SELECT table_name FROM information_schema.tables \
             WHERE table_schema = ANY(current_schemas(false)) AND table_type = 'BASE TABLE' \
             ORDER BY table_name",
            &[],
        )
        .map_err(|e| format!("no se pudo listar las tablas: {e}"))?;
    let tables: Vec<String> = table_rows.iter().map(|r| r.get::<_, String>(0)).collect();
    if tables.is_empty() {
        return Err("el schema 'public' no tiene ninguna tabla -- nada para introspeccionar".to_string());
    }

    let mut type_blocks = Vec::new();
    let mut db_lines = Vec::new();
    let mut all_warnings = Vec::new();
    for table in &tables {
        let TableIntrospection { link_type, warnings } = introspect_table(&mut client, table)?;
        type_blocks.push(link_type.clone());
        let type_name = to_pascal_case(table);
        db_lines.push(format!("  {table}: {type_name}[],"));
        all_warnings.extend(warnings.into_iter().map(|w| format!("{table}: {w}")));
    }

    let header = "// Generado por 'linkc introspect' -- PUNTO DE PARTIDA, no un .link listo\n\
                  // para producción sin revisar. Cualquier línea con '// TODO' o reportada\n\
                  // como advertencia en stderr necesita una decisión manual.\n\n";
    let content = format!("{header}{}\n\ndb {{\n{}\n}}\n", type_blocks.join("\n\n"), db_lines.join("\n"));
    Ok((content, all_warnings))
}

/// Punto de entrada unificado para `linkc introspect`: detecta PostgreSQL o SQLite.
pub fn generate_link(target: &str) -> Result<(String, Vec<String>), String> {
    let trimmed = target.trim();
    if trimmed.starts_with("postgres://") || trimmed.starts_with("postgresql://") {
        generate_link_from_postgres(trimmed)
    } else {
        generate_link_from_sqlite(trimmed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn array_columns_map_to_typed_lists_by_udt_name() {
        assert_eq!(map_pg_type("ARRAY", "_int4", "product_ids"), ("Int[]", None));
        assert_eq!(map_pg_type("ARRAY", "_int8", "ids"), ("Int[]", None));
        assert_eq!(map_pg_type("ARRAY", "_text", "tags"), ("String[]", None));
        assert_eq!(map_pg_type("ARRAY", "_varchar", "tags"), ("String[]", None));
        assert_eq!(map_pg_type("ARRAY", "_bool", "flags"), ("Bool[]", None));
        assert_eq!(map_pg_type("ARRAY", "_float8", "xs"), ("Float[]", None));
        let (ty, warning) = map_pg_type("ARRAY", "_uuid", "refs");
        assert_eq!(ty, "String[]");
        assert!(warning.as_deref().unwrap_or("").contains("array de 'uuid'"), "{warning:?}");
    }

    #[test]
    fn map_pg_type_covers_the_common_scalar_types_without_a_warning() {
        assert_eq!(map_pg_type("bigint", "", "x").0, "Int");
        assert_eq!(map_pg_type("integer", "", "x").0, "Int");
        assert_eq!(map_pg_type("smallint", "", "x").0, "Int");
        assert_eq!(map_pg_type("boolean", "", "x").0, "Bool");
        assert_eq!(map_pg_type("double precision", "", "x").0, "Float");
        assert_eq!(map_pg_type("numeric", "", "x").0, "Decimal");
        assert_eq!(map_pg_type("text", "", "x").0, "String");
        assert_eq!(map_pg_type("character varying", "", "x").0, "String");
        for ty in ["bigint", "integer", "smallint", "boolean", "double precision", "numeric", "text", "character varying"] {
            assert!(map_pg_type(ty, "", "x").1.is_none(), "'{ty}' no debería generar advertencia");
        }
    }

    #[test]
    fn map_pg_type_flags_jsonb_with_a_warning() {
        assert!(map_pg_type("jsonb", "", "meta").1.is_some());
        assert_eq!(map_pg_type("jsonb", "", "meta").0, "String");
    }

    #[test]
    fn map_pg_type_maps_native_uuid_inet_and_cidr_without_a_warning() {
        let (mapped, warning) = map_pg_type("uuid", "", "external_id");
        assert_eq!(mapped, "Uuid");
        assert!(warning.is_none(), "{warning:?}");
        for ty in ["inet", "cidr"] {
            let (mapped, warning) = map_pg_type(ty, "", "source_ip");
            assert_eq!(mapped, "String", "'{ty}'");
            assert!(warning.is_none(), "'{ty}' no debería generar advertencia: {warning:?}");
        }
    }

    #[test]
    fn map_pg_type_maps_native_date_and_timestamp_to_timestamp_without_a_warning() {
        for ty in ["date", "timestamp without time zone", "timestamp with time zone"] {
            let (mapped, warning) = map_pg_type(ty, "", "created_at");
            assert_eq!(mapped, "Timestamp", "'{ty}'");
            assert!(warning.is_none(), "'{ty}' no debería generar advertencia: {warning:?}");
        }
    }

    #[test]
    fn map_pg_type_still_warns_on_a_bare_time_without_a_date() {
        let (mapped, warning) = map_pg_type("time without time zone", "", "hora_apertura");
        assert_eq!(mapped, "String");
        assert!(warning.is_some());
    }

    #[test]
    fn map_pg_type_falls_back_to_string_with_a_warning_for_anything_unknown() {
        let (ty, warning) = map_pg_type("macaddr", "", "mac_address");
        assert_eq!(ty, "String");
        assert!(warning.is_some());
    }

    #[test]
    fn to_pascal_case_converts_snake_and_kebab_case_table_names() {
        assert_eq!(to_pascal_case("users"), "Users");
        assert_eq!(to_pascal_case("blog_posts"), "BlogPosts");
        assert_eq!(to_pascal_case("legacy-orders"), "LegacyOrders");
        assert_eq!(to_pascal_case("a_b_c"), "ABC");
    }

    #[test]
    fn map_sqlite_type_handles_affinities() {
        assert_eq!(map_sqlite_type("INTEGER", "id").0, "Int");
        assert_eq!(map_sqlite_type("INT", "age").0, "Int");
        assert_eq!(map_sqlite_type("TEXT", "name").0, "String");
        assert_eq!(map_sqlite_type("VARCHAR(255)", "code").0, "String");
        assert_eq!(map_sqlite_type("REAL", "price").0, "Float");
        assert_eq!(map_sqlite_type("DECIMAL", "amount").0, "Decimal");
        assert_eq!(map_sqlite_type("BOOLEAN", "active").0, "Bool");
        assert_eq!(map_sqlite_type("DATETIME", "created_at").0, "Timestamp");
        assert_eq!(map_sqlite_type("UUID", "token").0, "Uuid");
        assert!(map_sqlite_type("BLOB", "data").1.is_some());
    }

    #[test]
    fn parse_defaults_extracts_clean_cscript_values() {
        assert_eq!(parse_sqlite_default("CURRENT_TIMESTAMP", "Timestamp"), Some("= now()".to_string()));
        assert_eq!(parse_sqlite_default("1", "Bool"), Some("= true".to_string()));
        assert_eq!(parse_sqlite_default("0", "Bool"), Some("= false".to_string()));
        assert_eq!(parse_sqlite_default("'hello'", "String"), Some("= \"hello\"".to_string()));
        assert_eq!(parse_sqlite_default("42", "Int"), Some("= 42".to_string()));

        assert_eq!(parse_pg_default("now()", "Timestamp"), Some("= now()".to_string()));
        assert_eq!(parse_pg_default("true", "Bool"), Some("= true".to_string()));
        assert_eq!(parse_pg_default("false", "Bool"), Some("= false".to_string()));
        assert_eq!(parse_pg_default("'test'::text", "String"), Some("= \"test\"".to_string()));
        assert_eq!(parse_pg_default("100::numeric", "Decimal"), Some("= 100.toDecimal()".to_string()));
    }

    #[test]
    fn extract_sqlite_checks_finds_parenthesized_check_expressions() {
        let sql = "CREATE TABLE products (id INTEGER PRIMARY KEY, price REAL, CHECK(price >= 0), CHECK(price <= 1000))";
        let checks = extract_sqlite_checks(sql);
        assert_eq!(checks.len(), 2);
        assert_eq!(checks[0], "price >= 0");
        assert_eq!(checks[1], "price <= 1000");
    }
}
