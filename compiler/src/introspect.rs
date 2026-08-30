//! `linkc introspect <db-url>` (GRAMMAR.md §3.66): genera un `.link` de
//! partida a partir de una base PostgreSQL YA EXISTENTE, leyendo
//! `information_schema` -- para no tener que escribir cada `type`/`db{...}`
//! a mano cuando se adopta un sistema que ya tiene datos.
//!
//! Alcance deliberadamente acotado a PostgreSQL (no SQLite: el caso real que
//! motiva esto -- "adoptar un sistema existente" -- casi siempre es sobre una
//! base de producción ya corriendo, y eso es Postgres, no un archivo SQLite
//! suelto). El resultado es un PUNTO DE PARTIDA para revisar a mano, no un
//! `.link` listo para producción sin mirarlo: cualquier columna que este
//! módulo no pueda mapear con confianza (JSONB de forma desconocida, un
//! `time` sin fecha) se emite igual, como `String`, con un comentario
//! `/* TODO */` al lado que dice exactamente qué hace falta revisar -- nunca
//! se omite una columna en silencio. `date`/`timestamp`/`timestamptz`
//! (GRAMMAR.md §3.91) y `uuid`/`inet`/`cidr` (§3.179) NATIVOS de Postgres SÍ
//! mapean con confianza (sin advertencia) -- los tres empezaron como "String
//! con advertencia" y resultaron ROTOS en los hechos (ni `String` ni el tipo
//! "obvio" decodificaban esa columna contra una fila real) hasta que una
//! ronda dedicada verificó el binario real y lo arregló.
//!
//! Los nombres de campo son los nombres REALES de columna SQL, `snake_case`
//! incluido -- c-script no tiene ningún mecanismo de alias campo->columna
//! (el nombre del campo ES el nombre de columna que usa `insert`/`find`/etc.,
//! `runtime/db.rs`), así que renombrar acá a `camelCase` rompería la
//! conexión real con la tabla existente. Queda como ejercicio manual para
//! quien quiera esa convención (y también renombrar la columna real).

use crate::runtime::db::connect_postgres_client;

/// Una columna real, ya leída de `information_schema.columns`.
struct Column {
    name: String,
    pg_type: String,
    nullable: bool,
}

/// Resultado de introspeccionar una tabla: el `type`/`db` que se puede
/// generar con confianza, más cualquier advertencia sobre columnas que
/// necesitan revisión manual.
struct TableIntrospection {
    link_type: String,
    warnings: Vec<String>,
}

/// Mapea un `data_type` de `information_schema.columns` (siempre en
/// minúsculas, la forma que Postgres ya normaliza) al tipo c-script más
/// cercano -- `(tipo, advertencia)`. `advertencia` es `Some` cuando el mapeo
/// es un placeholder que hay que revisar a mano, nunca cuando es exacto.
fn map_pg_type(pg_type: &str, column_name: &str) -> (&'static str, Option<String>) {
    match pg_type {
        "bigint" | "integer" | "smallint" => ("Int", None),
        "boolean" => ("Bool", None),
        "double precision" | "real" => ("Float", None),
        // GRAMMAR.md §3.184: `Decimal`, no `Float` -- `numeric`/`decimal`
        // (sinónimos, `information_schema` siempre normaliza a "numeric")
        // es EXACTAMENTE el tipo que casi toda columna de dinero real usa
        // (nunca `float8`, por el error de redondeo binario que `numeric`
        // evita) -- mapeo EXACTO ahora que `Decimal` decodifica/codifica
        // el binario real sin pasar por `f64` en ningún punto, mismo
        // criterio que `uuid`/`inet`/`date`/`timestamp` arriba/abajo.
        "numeric" => ("Decimal", None),
        "text" | "character varying" | "character" | "citext" => ("String", None),
        // GRAMMAR.md §3.179: hasta antes de esa ronda, un `uuid` NATIVO de
        // Postgres contra un campo `Uuid`/`String` no estaba verificado
        // (mismo tipo de mapeo "parece obvio, nunca se probó contra una
        // fila real" que resultó roto para `date`/`timestamp`, §3.91).
        // Reporte de adopción real (iaacademy, vía skynet-43) confirmó el
        // gap Y motivó el fix -- `postgres_string_cell`/`Cell::to_sql`
        // (runtime/store.rs) ahora decodifican/codifican el binario real
        // de `uuid`. Mapeo EXACTO, sin advertencia -- mismo criterio que
        // `timestamp`/`date` de acá abajo.
        "uuid" => ("Uuid", None),
        // GRAMMAR.md §3.179: mismo fix, mismo criterio -- `inet`/`cidr`
        // ahora decodifican/codifican de verdad contra un campo `String`
        // (`PgInetText`/`inet_string_to_binary`, runtime/store.rs). Sin
        // sufijo "/N" para una IP sin máscara real (el caso normal, una
        // IP de cliente guardada tal cual), con él si la columna de
        // verdad tiene una máscara distinta del ancho completo.
        "inet" | "cidr" => ("String", None),
        "jsonb" | "json" => (
            "String",
            Some(format!(
                "'{column_name}' es {pg_type} -- la FORMA real del JSON no se puede inferir de information_schema; \
                 declará un 'type' propio para ese shape y reemplazá 'String' acá si corresponde"
            )),
        ),
        // GRAMMAR.md §3.91: hasta antes de esa ronda, un `date`/`timestamp`
        // NATIVO de Postgres (no el `BIGINT` propio de c-script) no
        // decodificaba -- `Timestamp` acá era un mapeo roto, disfrazado de
        // "revisar a mano". Ahora que decodifica de verdad (los dos
        // backends, `runtime/store.rs::postgres_timestamp_cell`), es un
        // mapeo EXACTO, sin advertencia -- mismo criterio que `bigint`/
        // `boolean` arriba.
        "timestamp without time zone" | "timestamp with time zone" | "date" => ("Timestamp", None),
        // `time` (sin fecha) SIGUE sin mapeo -- un Timestamp de c-script es
        // un INSTANTE completo (fecha + hora), no le cabe una hora-del-día
        // suelta sin perder información.
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

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// `snake_case`/`kebab-case` -> `PascalCase`, para el nombre del `type` --
/// una tabla `blog_posts` da `type BlogPosts`, no `type Blog_posts`.
fn to_pascal_case(table_name: &str) -> String {
    table_name.split(|c| c == '_' || c == '-').filter(|s| !s.is_empty()).map(capitalize).collect()
}

/// GRAMMAR.md §3.192: las tres consultas de este archivo (dos acá, una en
/// `generate_link_from_postgres` más abajo) hardcodeaban `table_schema =
/// 'public'` -- `linkc introspect` no podía ver NINGUNA tabla fuera de
/// `public`, sin ningún error que lo explicara (la tabla simplemente no
/// aparecía). `current_schemas(false)` (el `search_path` EFECTIVO de la
/// sesión, función nativa de Postgres) reemplaza el literal fijo -- ahora
/// introspecciona lo que la sesión REALMENTE ve, sea `public` (el default
/// de casi cualquier conexión) o cualquier otro schema que la URL/rol
/// haya configurado (`options=-c search_path=...`, GRAMMAR.md §3.193).
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
            "SELECT column_name, data_type, is_nullable \
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
            pg_type: r.get::<_, String>(1),
            nullable: r.get::<_, String>(2) == "YES",
        })
        .collect();

    let mut warnings = Vec::new();
    let mut fields = Vec::new();

    if pk_columns.len() == 1 && pk_columns[0] == "id" {
        // El caso normal: "id" es la única PK -- c-script la declara como
        // el primer campo, `Int` (GRAMMAR.md §3.59: BIGINT/integer/
        // smallint decodifican los tres igual) o `Uuid` (GRAMMAR.md
        // §3.177: solo si la columna real es 'uuid' NATIVO de Postgres --
        // c-script genera ese valor del lado de la aplicación en cada
        // insert, nunca depende de un DEFAULT de columna).
        //
        // "Se llama id" y "es uno de esos dos tipos" son cosas distintas
        // -- CUALQUIER otro tipo real (`text`, `character varying`, etc.)
        // sigue sin tener representación como PK en c-script hoy, así que
        // sigue emitiendo el placeholder `id: Int` de siempre con una
        // advertencia, en vez de fingir que compila.
        let id_pg_type = columns.iter().find(|c| c.name == "id").map(|c| c.pg_type.as_str());
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
            continue; // ya emitido arriba, siempre primero
        }
        let (base_ty, warning) = map_pg_type(&col.pg_type, &col.name);
        if let Some(w) = warning {
            warnings.push(w);
        }
        let ty = if col.nullable { format!("{base_ty}?") } else { base_ty.to_string() };
        fields.push(format!("  {}: {},", col.name, ty));
    }

    let type_name = to_pascal_case(table);
    let link_type = format!("type {type_name} = {{\n{}\n}}", fields.join("\n"));
    Ok(TableIntrospection { link_type, warnings })
}

/// Genera un `.link` de partida desde TODAS las tablas base del schema
/// `public` de la base en `url` -- `(contenido, advertencias)`. Nunca
/// falla por una tabla individual rara: cada advertencia queda asociada a
/// la tabla que la generó, para que el caller decida cómo mostrarlas
/// (`main.rs` las manda a stderr, prefijadas con el nombre de tabla).
/// `linkc introspect <db-url>` -- a diferencia de `serve`/`db shell`/etc.,
/// no tiene ningún flag propio (`main.rs` la llama con la URL cruda tal
/// cual el usuario la pasó) -- así que no toma `--db-schema` (GRAMMAR.md
/// §3.193) por separado. Un `search_path` propio se compone gratis vía el
/// mecanismo NATIVO de Postgres, embebido en la URL misma
/// (`?options=-c%20search_path%3D...`), sin que este comando necesite saber
/// nada al respecto -- y `introspect_table`/§3.192 ya respeta cualquier
/// `search_path` real de la sesión, sea cual sea su origen.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_pg_type_covers_the_common_scalar_types_without_a_warning() {
        assert_eq!(map_pg_type("bigint", "x").0, "Int");
        assert_eq!(map_pg_type("integer", "x").0, "Int");
        assert_eq!(map_pg_type("smallint", "x").0, "Int");
        assert_eq!(map_pg_type("boolean", "x").0, "Bool");
        assert_eq!(map_pg_type("double precision", "x").0, "Float");
        assert_eq!(map_pg_type("numeric", "x").0, "Decimal");
        assert_eq!(map_pg_type("text", "x").0, "String");
        assert_eq!(map_pg_type("character varying", "x").0, "String");
        for ty in ["bigint", "integer", "smallint", "boolean", "double precision", "numeric", "text", "character varying"] {
            assert!(map_pg_type(ty, "x").1.is_none(), "'{ty}' no debería generar advertencia");
        }
    }

    #[test]
    fn map_pg_type_flags_jsonb_with_a_warning() {
        assert!(map_pg_type("jsonb", "meta").1.is_some());
        // Sigue dando un tipo VÁLIDO (String) -- nunca se omite la columna
        // del .link generado, aunque necesite revisión.
        assert_eq!(map_pg_type("jsonb", "meta").0, "String");
    }

    /// GRAMMAR.md §3.179: `uuid`/`inet`/`cidr` NATIVOS de Postgres
    /// decodifican/codifican de verdad (`postgres_string_cell`/
    /// `Cell::to_sql`, runtime/store.rs) desde esta ronda -- mapeo EXACTO,
    /// sin advertencia, mismo criterio que `timestamp`/`date` arriba.
    /// Antes de esta ronda `uuid` mapeaba a `String` con advertencia
    /// ("no está verificado todavía"), e `inet`/`cidr` caían en el
    /// catch-all genérico -- reporte de adopción real (iaacademy, vía
    /// skynet-43) confirmó el gap y motivó el fix.
    #[test]
    fn map_pg_type_maps_native_uuid_inet_and_cidr_without_a_warning() {
        let (mapped, warning) = map_pg_type("uuid", "external_id");
        assert_eq!(mapped, "Uuid");
        assert!(warning.is_none(), "{warning:?}");
        for ty in ["inet", "cidr"] {
            let (mapped, warning) = map_pg_type(ty, "source_ip");
            assert_eq!(mapped, "String", "'{ty}'");
            assert!(warning.is_none(), "'{ty}' no debería generar advertencia: {warning:?}");
        }
    }

    /// GRAMMAR.md §3.91: `date`/`timestamp`/`timestamptz` NATIVOS de
    /// Postgres decodifican de verdad contra un campo `Timestamp` desde
    /// esta ronda -- mapeo EXACTO, sin advertencia, mismo criterio que
    /// `bigint`/`boolean`. Antes de esta ronda mapeaban a `String` con
    /// advertencia (un mapeo que en los hechos estaba roto: ni `String` ni
    /// `Timestamp` decodificaban una columna así).
    #[test]
    fn map_pg_type_maps_native_date_and_timestamp_to_timestamp_without_a_warning() {
        for ty in ["date", "timestamp without time zone", "timestamp with time zone"] {
            let (mapped, warning) = map_pg_type(ty, "created_at");
            assert_eq!(mapped, "Timestamp", "'{ty}'");
            assert!(warning.is_none(), "'{ty}' no debería generar advertencia: {warning:?}");
        }
    }

    /// `time` (sin fecha) es la única forma temporal que SIGUE sin mapeo
    /// exacto -- un `Timestamp` de c-script es un instante completo, no le
    /// cabe una hora suelta.
    #[test]
    fn map_pg_type_still_warns_on_a_bare_time_without_a_date() {
        let (mapped, warning) = map_pg_type("time without time zone", "hora_apertura");
        assert_eq!(mapped, "String");
        assert!(warning.is_some());
    }

    #[test]
    fn map_pg_type_falls_back_to_string_with_a_warning_for_anything_unknown() {
        // GRAMMAR.md §3.179: "inet" ya no sirve de ejemplo acá -- pasó a
        // ser un mapeo exacto sin advertencia (ver
        // map_pg_type_maps_native_uuid_inet_and_cidr_without_a_warning).
        // "macaddr" sigue sin ningún mapeo dedicado.
        let (ty, warning) = map_pg_type("macaddr", "mac_address");
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
}
