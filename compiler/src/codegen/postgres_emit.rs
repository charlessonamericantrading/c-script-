// Adaptador y generador de esquemas SQL para PostgreSQL (Link 1.0 Enterprise).
// Provee mapeo de tipos nativo para PostgreSQL (BIGINT, JSONB, DOUBLE PRECISION, TEXT),
// generación de DDL completo, consultas preparadas y soporte de auto-migración no destructiva.

use crate::ast::{FieldCheck, Program};
use crate::checker::Checker;
use crate::runtime::db::{
    check_clause_sql, check_fields_by_collection, composite_unique_by_collection, create_composite_unique_statements,
    type_checks_by_collection,
};
use crate::types::{FieldType, Type};
use std::collections::HashSet;

/// Mapeo de tipos de Link a dialecto PostgreSQL nativo.
///
/// `T?` se mapea al tipo nativo de `T`: la nulabilidad de una columna la
/// expresa la ausencia de `NOT NULL`, no un tipo distinto. Sin desenvolver el
/// `Optional` aqui, `String?` no hacia match con `Type::String` y caia en el
/// `_ => "JSONB"` final, asi que una columna de texto nullable corriente se
/// declaraba `JSONB`. El runtime SQLite ya desenvuelve (`ColumnPlan::for_field`
/// en runtime/db.rs), de modo que los dos backends emitian esquemas distintos
/// para el mismo programa.
pub fn link_to_postgres_type(ty: &Type, simple_enums: &HashSet<String>) -> &'static str {
    match ty {
        Type::Optional(inner) => link_to_postgres_type(inner.as_ref(), simple_enums),
        Type::Int | Type::Int64 | Type::Timestamp => "BIGINT",
        Type::Float => "DOUBLE PRECISION",
        Type::String => "TEXT",
        // TEXT, no el tipo nativo UUID de Postgres -- mismo criterio de
        // "sin rama por backend" que el resto de este mapeo: SQLite no
        // tiene un tipo UUID nativo, así que los dos backends usan TEXT
        // (GRAMMAR.md §3.70). La validación de forma vive en el borde JSON,
        // no en una constraint de columna.
        Type::Uuid => "TEXT",
        Type::Bool => "BOOLEAN",
        Type::Enum(name) if simple_enums.contains(name) => "TEXT",
        // Todo tipo compuesto (structs, arrays, mapas, genéricos, uniones) se guarda en JSONB nativo
        _ => "JSONB",
    }
}

/// Tipo de columna de un campo concreto, con la misma regla de tres estados
/// que `ColumnPlan::for_field` (runtime/db.rs): `campo?: T?` distingue
/// ausente / null / valor, y eso no lo puede representar una columna nativa
/// nullable -- necesita JSON. Un solo nivel de opcionalidad, en cambio, es una
/// columna nativa que admite NULL.
fn postgres_column_type(field: &FieldType, simple_enums: &HashSet<String>) -> &'static str {
    let double_optional = field.optional && matches!(field.ty, Type::Optional(_));
    if double_optional {
        "JSONB"
    } else {
        link_to_postgres_type(&field.ty, simple_enums)
    }
}

/// Genera la sentencia DDL `CREATE TABLE IF NOT EXISTS` para una colección en PostgreSQL.
/// `checks` (GRAMMAR.md §3.96, `@check` de un solo campo): pares `(campo,
/// FieldCheck)`, mismo formato que `runtime::db::check_fields_by_collection`
/// produce -- sin entrada para el `.link` mayoría que no usa `@check` en
/// absoluto. `type_checks` (GRAMMAR.md §3.173, `@check(<expr>)` de nivel
/// `type`): expresiones SQL YA TRADUCIDAS (`runtime::db::type_check_expr_sql`),
/// una por cada `@check(...)` a nivel `type` -- se agregan como constraint
/// de TABLA (no de columna, a diferencia de `checks`), mismo lugar en el
/// `CREATE TABLE` que ocuparía cualquier otro `CHECK` de más de una
/// columna.
pub fn create_postgres_table_sql(
    collection: &str,
    id_field_ty: &Type,
    fields: &[FieldType],
    simple_enums: &HashSet<String>,
    checks: &[(String, FieldCheck)],
    type_checks: &[String],
) -> String {
    // GRAMMAR.md §3.177: una PK `Uuid` usa el tipo NATIVO `UUID` de
    // Postgres (a diferencia de un campo `Uuid` normal, que sigue
    // mapeando a `TEXT` -- ver `link_to_postgres_type` -- porque acá SÍ
    // hace falta poder adoptar una tabla existente cuya columna real ya
    // es `uuid` nativo, el caso real que motiva esto). Sin
    // `DEFAULT gen_random_uuid()`: c-script genera el valor del lado de
    // la aplicación en CADA insert (`runtime/db.rs::Db::call`, "insert"),
    // nunca depende de un default de columna -- agregarlo solo sumaría
    // un requisito de versión (PostgreSQL 13+) sin ningún beneficio para
    // el camino real, así que se deja afuera a propósito.
    let id_def = match id_field_ty {
        Type::Uuid => "\"id\" UUID PRIMARY KEY".to_string(),
        _ => "\"id\" BIGSERIAL PRIMARY KEY".to_string(),
    };
    let mut cols = vec![id_def];

    for f in fields {
        let pg_type = postgres_column_type(f, simple_enums);
        let not_null = if !f.optional && !matches!(f.ty, Type::Optional(_)) {
            " NOT NULL"
        } else {
            ""
        };
        let check_clause = match checks.iter().find(|(name, _)| name == &f.name) {
            Some((_, c)) => format!(" {}", check_clause_sql(&f.name, c)),
            None => String::new(),
        };
        cols.push(format!("\"{}\" {}{}{}", f.name, pg_type, not_null, check_clause));
    }
    for sql in type_checks {
        cols.push(format!("CHECK {sql}"));
    }

    format!("CREATE TABLE IF NOT EXISTS \"{collection}\" (\n  {}\n);", cols.join(",\n  "))
}

/// Genera el script SQL de migración e inicialización completo para PostgreSQL a partir de un `Program` de Link.
pub fn generate_postgres_ddl(program: &Program) -> Result<String, String> {
    let (checker, errors) = Checker::build_symbols(program);
    if !errors.is_empty() {
        return Err(format!("error de análisis estático: {:?}", errors[0]));
    }

    let simple_enums: HashSet<String> = checker
        .enums
        .iter()
        .filter(|(_, decl)| decl.variants.iter().all(|v| v.fields.is_none()))
        .map(|(k, _)| k.clone())
        .collect();

    // Sin `CREATE EXTENSION "pgcrypto"` (PLAN.md §9.1): auditando si esa
    // línea necesita superusuario en Postgres gestionado (Neon/RDS/Supabase,
    // pedido explícito en un reporte de adopción real) apareció que NADA en
    // este codegen ni en el runtime usa ninguna función de pgcrypto --
    // `crypto.hashPassword`/`hmacSha256`/etc. (GRAMMAR.md §3.34/§3.38/§3.55)
    // son Argon2id/HMAC implementados en Rust (`argon2`/`hmac`), nunca SQL.
    // La línea era peso muerto heredado que podía bloquear a alguien sin
    // permiso de `CREATE EXTENSION` por una extensión que el proyecto nunca
    // necesitó -- la respuesta correcta no era documentar el requisito, era
    // borrarla.
    let mut statements = vec!["-- Schema generado automáticamente por Link (PostgreSQL Enterprise Backend)".to_string()];
    let checks_by_collection = check_fields_by_collection(program, &checker);
    let type_checks_by_collection = type_checks_by_collection(program, &checker);
    let empty_checks: Vec<(String, FieldCheck)> = Vec::new();
    let empty_type_checks: Vec<String> = Vec::new();

    for (coll_name, elem_ty) in checker.db_collections() {
        if let Type::Struct { fields, .. } = elem_ty {
            let non_id_fields: Vec<FieldType> = fields.iter().filter(|f| f.name != "id").cloned().collect();
            let id_field_ty = &fields.iter().find(|f| f.name == "id").expect("validate_db_element_type ya garantizó 'id'").ty;
            let checks = checks_by_collection.get(coll_name).unwrap_or(&empty_checks);
            let type_checks = type_checks_by_collection.get(coll_name).unwrap_or(&empty_type_checks);
            let sql = create_postgres_table_sql(coll_name, id_field_ty, &non_id_fields, &simple_enums, checks, type_checks);
            statements.push(sql);
        }
    }

    // `@index`/`@unique` (GRAMMAR.md §3.80) -- `Type::Struct` (resuelto,
    // arriba) no conserva anotaciones; se cruza con `program.items` (que sí
    // tiene `ast::Field` con anotaciones) por el nombre que
    // `Type::Struct{name: Some(...)}` de un elemento de colección siempre
    // conserva -- mismo criterio que `index_fields_by_collection` en
    // `runtime/db.rs` (duplicado acá porque este módulo genera el DDL
    // ESTÁTICO para `linkc build`, sin instanciar ningún `Db` real).
    for (coll_name, elem_ty) in checker.db_collections() {
        let Type::Struct { name: Some(type_name), .. } = elem_ty else { continue };
        for item in &program.items {
            let crate::ast::Item::Type(t) = item else { continue };
            if &t.name != type_name {
                continue;
            }
            let crate::ast::TypeExpr::Struct(ast_fields) = &t.ty else { continue };
            for f in ast_fields {
                if let Some(unique) = f.index() {
                    let unique_kw = if unique { "UNIQUE " } else { "" };
                    statements.push(format!(
                        "CREATE {unique_kw}INDEX IF NOT EXISTS \"idx_{coll_name}_{}\" ON \"{coll_name}\"(\"{}\");",
                        f.name, f.name
                    ));
                }
            }
        }
    }

    // `@unique(campo1, campo2, ...)` opcionalmente `where <expr>` a nivel de
    // `type` (GRAMMAR.md §3.155/§3.174) -- reusa DIRECTO las mismas dos
    // funciones puras que arma el `Db` real (`composite_unique_by_collection`/
    // `create_composite_unique_statements`, `runtime/db.rs`) en vez de
    // volver a derivar el `CREATE UNIQUE INDEX` a mano acá: ninguna de las
    // dos necesita un `Db` real (los dos toman `program`/`checker` puros),
    // así que la duplicación que este comentario justificaba antes de
    // §3.174 ya no hacía falta -- reusarlas de una es lo que evita que este
    // generador estático y el runtime real puedan divergir en la condición
    // `where` (el mismo riesgo que motivó reusar el generador para el resto
    // del DDL, GRAMMAR.md §3.9).
    for (coll_name, sets) in composite_unique_by_collection(program, &checker) {
        for stmt in create_composite_unique_statements(&coll_name, &sets) {
            statements.push(format!("{stmt};"));
        }
    }

    Ok(statements.join("\n\n"))
}

/// Genera una sentencia `ALTER TABLE ... ADD COLUMN IF NOT EXISTS` para auto-migración no destructiva en PostgreSQL.
pub fn alter_table_add_column_postgres(
    collection: &str,
    field: &FieldType,
    simple_enums: &HashSet<String>,
) -> String {
    let pg_type = postgres_column_type(field, simple_enums);
    format!(
        "ALTER TABLE \"{collection}\" ADD COLUMN IF NOT EXISTS \"{}\" {};",
        field.name, pg_type
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer;
    use crate::parser;

    #[test]
    fn test_postgres_ddl_generation_for_declared_database() {
        let code = r#"
        type User = { id: Int, name: String, email: String, is_active: Bool, metadata: Map<String, String>, created_at: Timestamp }
        enum Role { Admin, Member }
        db {
            users: User[],
        }
        "#;
        let tokens = lexer::tokenize(code).unwrap();
        let program = parser::parse(tokens).unwrap();
        let ddl = generate_postgres_ddl(&program).unwrap();

        assert!(ddl.contains("CREATE TABLE IF NOT EXISTS \"users\""));
        assert!(ddl.contains("\"id\" BIGSERIAL PRIMARY KEY"));
        assert!(ddl.contains("\"name\" TEXT NOT NULL"));
        assert!(ddl.contains("\"is_active\" BOOLEAN NOT NULL"));
        assert!(ddl.contains("\"created_at\" BIGINT NOT NULL"));
        assert!(ddl.contains("\"metadata\" JSONB NOT NULL"));
    }

    /// GRAMMAR.md §3.96: `@check` genera un `CHECK (...)` inline de verdad
    /// en el DDL estático que `linkc build` emite -- no solo se valida del
    /// lado de la aplicación (`apply_field_validators`, `runtime/mod.rs`).
    #[test]
    fn check_annotation_emits_a_real_sql_check_constraint() {
        let code = r#"
        type Review = { id: Int, @check(range, 1, 5) rating: Int, @check(min, 0) helpfulVotes: Int, @check(max, 100) discountPercent: Float }
        db { reviews: Review[] }
        "#;
        let program = parser::parse(lexer::tokenize(code).unwrap()).unwrap();
        let ddl = generate_postgres_ddl(&program).unwrap();
        assert!(ddl.contains("\"rating\" BIGINT NOT NULL CHECK (\"rating\" >= 1 AND \"rating\" <= 5)"), "{ddl}");
        assert!(ddl.contains("\"helpfulVotes\" BIGINT NOT NULL CHECK (\"helpfulVotes\" >= 0)"), "{ddl}");
        assert!(ddl.contains("\"discountPercent\" DOUBLE PRECISION NOT NULL CHECK (\"discountPercent\" <= 100)"), "{ddl}");
    }

    /// GRAMMAR.md §3.173: `@check(<expr>)` de nivel type genera un `CHECK`
    /// de TABLA de verdad en el DDL estático -- constraint de tabla, no de
    /// columna (a diferencia del `@check` de un solo campo del test de
    /// arriba), así que aparece como su propia entrada en la lista de
    /// columnas del `CREATE TABLE`, no pegado al final de una columna.
    #[test]
    fn type_level_check_annotation_emits_a_real_sql_table_check_constraint() {
        let code = r#"
        @check(endDay > startDay)
        type Booking = { id: Int, startDay: Int, endDay: Int }
        db { bookings: Booking[] }
        "#;
        let program = parser::parse(lexer::tokenize(code).unwrap()).unwrap();
        let ddl = generate_postgres_ddl(&program).unwrap();
        assert!(ddl.contains("CHECK (\"endDay\" > \"startDay\")"), "{ddl}");
    }

    /// PLAN.md §9.1: auditando si `CREATE EXTENSION "pgcrypto"` necesita
    /// superusuario en Postgres gestionado apareció que nada en este codegen
    /// ni en el runtime usa NINGUNA función de pgcrypto -- `crypto.*`
    /// (GRAMMAR.md §3.34/§3.38/§3.55) es Argon2id/HMAC en Rust, nunca SQL.
    /// La línea era peso muerto que podía bloquear a alguien sin permiso de
    /// `CREATE EXTENSION` por una extensión que el proyecto nunca necesitó.
    #[test]
    fn generated_ddl_never_requires_the_pgcrypto_extension() {
        let code = "type User = { id: Int, name: String } db { users: User[] }";
        let program = parser::parse(lexer::tokenize(code).unwrap()).unwrap();
        let ddl = generate_postgres_ddl(&program).unwrap();
        assert!(!ddl.to_lowercase().contains("pgcrypto"), "el DDL generado no debería mencionar pgcrypto en absoluto: {ddl}");
        assert!(!ddl.to_lowercase().contains("create extension"), "no debería pedir ninguna extensión: {ddl}");
    }

    /// Regresion: `String?` no hacia match con `Type::String` y caia en el
    /// `_ => "JSONB"`, asi que toda columna de texto nullable se declaraba
    /// JSONB en vez de TEXT. Lo mismo para Int?/Bool?/Float?/Timestamp? y
    /// para un enum simple opcional.
    #[test]
    fn optional_scalars_keep_their_native_column_type() {
        let code = r#"
        type Lead = {
          id: Int,
          email: String,
          company: String?,
          score: Int?,
          rating: Float?,
          contacted: Bool?,
          closedAt: Timestamp?,
          status: Status?,
        }
        enum Status { Nuevo, Cerrado }
        db { leads: Lead[], }
        "#;
        let tokens = lexer::tokenize(code).unwrap();
        let program = parser::parse(tokens).unwrap();
        let ddl = generate_postgres_ddl(&program).unwrap();

        assert!(ddl.contains("\"email\" TEXT NOT NULL"), "{ddl}");
        assert!(ddl.contains("\"company\" TEXT,") || ddl.contains("\"company\" TEXT
"), "company deberia ser TEXT nullable: {ddl}");
        assert!(!ddl.contains("\"company\" JSONB"), "company se declaro JSONB: {ddl}");
        assert!(!ddl.contains("\"score\" JSONB"), "score se declaro JSONB: {ddl}");
        assert!(ddl.contains("\"score\" BIGINT"), "{ddl}");
        assert!(ddl.contains("\"rating\" DOUBLE PRECISION"), "{ddl}");
        assert!(ddl.contains("\"contacted\" BOOLEAN"), "{ddl}");
        assert!(ddl.contains("\"closedAt\" BIGINT"), "{ddl}");
        assert!(ddl.contains("\"status\" TEXT"), "enum simple opcional deberia ser TEXT: {ddl}");
    }

    /// La nulabilidad la expresa `NOT NULL`, no el tipo: desenvolver el
    /// `Optional` no puede colar un NOT NULL en una columna que admite null.
    #[test]
    fn optional_columns_are_still_nullable() {
        let code = r#"
        type T = { id: Int, a: String, b: String? }
        db { ts: T[], }
        "#;
        let tokens = lexer::tokenize(code).unwrap();
        let program = parser::parse(tokens).unwrap();
        let ddl = generate_postgres_ddl(&program).unwrap();
        assert!(ddl.contains("\"a\" TEXT NOT NULL"), "{ddl}");
        assert!(!ddl.contains("\"b\" TEXT NOT NULL"), "b es nullable y quedo NOT NULL: {ddl}");
    }

    /// `campo?: T?` distingue ausente / null / valor. Ese tercer estado no
    /// cabe en una columna nativa nullable -- igual que en SQLite
    /// (`ColumnPlan::for_field`), se guarda como JSON.
    #[test]
    fn a_doubly_optional_field_stays_json_like_in_sqlite() {
        let simple_enums = HashSet::new();
        let field = FieldType {
            name: "nickname".to_string(),
            optional: true,
            ty: Type::Optional(Box::new(Type::String)),
        };
        assert_eq!(
            alter_table_add_column_postgres("users", &field, &simple_enums),
            "ALTER TABLE \"users\" ADD COLUMN IF NOT EXISTS \"nickname\" JSONB;"
        );
    }

    /// Un `T?` de un solo nivel via ALTER TABLE tambien es columna nativa.
    #[test]
    fn alter_table_uses_the_native_type_for_a_single_optional() {
        let simple_enums = HashSet::new();
        let field = FieldType {
            name: "avatar_url".to_string(),
            optional: false,
            ty: Type::Optional(Box::new(Type::String)),
        };
        assert_eq!(
            alter_table_add_column_postgres("users", &field, &simple_enums),
            "ALTER TABLE \"users\" ADD COLUMN IF NOT EXISTS \"avatar_url\" TEXT;"
        );
    }

    /// `@index`/`@unique` (GRAMMAR.md §3.80) -- el DDL estático que
    /// `linkc build` emite para Postgres, aparte del que crea el runtime al
    /// arrancar (`create_index_statements` en `runtime/db.rs`).
    #[test]
    fn index_and_unique_annotations_emit_create_index_statements() {
        let code = r#"
        type User = { id: Int, @unique email: String, @index country: String, name: String }
        db { users: User[] }
        "#;
        let tokens = lexer::tokenize(code).unwrap();
        let program = parser::parse(tokens).unwrap();
        let ddl = generate_postgres_ddl(&program).unwrap();

        assert!(
            ddl.contains("CREATE UNIQUE INDEX IF NOT EXISTS \"idx_users_email\" ON \"users\"(\"email\");"),
            "{ddl}"
        );
        assert!(
            ddl.contains("CREATE INDEX IF NOT EXISTS \"idx_users_country\" ON \"users\"(\"country\");"),
            "{ddl}"
        );
        assert!(!ddl.contains("idx_users_name"), "'name' no lleva anotación, no debería generar índice: {ddl}");
    }

    /// `@unique(campo1, campo2, ...)` a nivel de `type` (GRAMMAR.md §3.155)
    /// -- misma idea que el test de arriba, para el DDL estático del
    /// constraint COMPUESTO.
    #[test]
    fn composite_unique_annotation_emits_a_multi_column_create_unique_index() {
        let code = r#"
        @unique(profileId, slug)
        type Product = { id: Int, profileId: Int, slug: String, name: String }
        db { products: Product[] }
        "#;
        let tokens = lexer::tokenize(code).unwrap();
        let program = parser::parse(tokens).unwrap();
        let ddl = generate_postgres_ddl(&program).unwrap();

        assert!(
            ddl.contains("CREATE UNIQUE INDEX IF NOT EXISTS \"idx_products_uniq_9$profileId4$slug\" ON \"products\"(\"profileId\", \"slug\");"),
            "{ddl}"
        );
    }

    /// GRAMMAR.md §3.174: `@unique(...) where <expr>` genera un `CREATE
    /// UNIQUE INDEX` PARCIAL de verdad en el DDL estático -- con la
    /// cláusula `WHERE` traducida, no solo las columnas.
    #[test]
    fn conditional_composite_unique_annotation_emits_a_partial_create_unique_index() {
        let code = r#"
        @unique(userId, appointmentDate, startTime) where status != "cancelled"
        type Appointment = { id: Int, userId: Int, appointmentDate: String, startTime: String, status: String }
        db { appointments: Appointment[] }
        "#;
        let tokens = lexer::tokenize(code).unwrap();
        let program = parser::parse(tokens).unwrap();
        let ddl = generate_postgres_ddl(&program).unwrap();

        assert!(ddl.contains("(\"userId\", \"appointmentDate\", \"startTime\")"), "{ddl}");
        assert!(ddl.contains("WHERE (\"status\" != 'cancelled')"), "{ddl}");
    }

    /// Bug real, encontrado por una auditoría multi-agente adversarial
    /// (26/08/2026): con el nombre viejo (`fields.join("_")`),
    /// `@unique(a_b, c)` y `@unique(a, b_c)` producían el MISMO nombre de
    /// índice (`idx_t_a_b_c`) -- `CREATE UNIQUE INDEX IF NOT EXISTS` volvía
    /// el segundo un no-op silencioso. `composite_unique_index_name`
    /// (runtime/db.rs, reusada acá) codifica con prefijo de longitud, así
    /// que las dos sentencias tienen que tener nombres DISTINTOS.
    #[test]
    fn two_composite_unique_constraints_that_would_collide_under_naive_joining_get_distinct_index_names() {
        let code = r#"
        @unique(a_b, c)
        @unique(a, b_c)
        type T = { id: Int, a_b: Int, c: Int, a: Int, b_c: Int }
        db { ts: T[] }
        "#;
        let tokens = lexer::tokenize(code).unwrap();
        let program = parser::parse(tokens).unwrap();
        let ddl = generate_postgres_ddl(&program).unwrap();

        assert!(ddl.contains("(\"a_b\", \"c\")"), "{ddl}");
        assert!(ddl.contains("(\"a\", \"b_c\")"), "{ddl}");
        // El punto real del test: los dos nombres de índice, extraídos de
        // sus respectivas sentencias CREATE, tienen que ser DISTINTOS.
        let names: Vec<&str> = ddl
            .lines()
            .filter(|l| l.contains("CREATE UNIQUE INDEX IF NOT EXISTS"))
            .filter_map(|l| l.split('"').nth(1))
            .collect();
        assert_eq!(names.len(), 2, "esperaba 2 sentencias CREATE UNIQUE INDEX: {ddl}");
        assert_ne!(names[0], names[1], "los dos nombres de índice no pueden colisionar: {ddl}");
    }

    #[test]
    fn test_alter_table_add_column_postgres_sql() {
        let simple_enums = HashSet::new();
        let field = FieldType {
            name: "avatar_url".to_string(),
            optional: true,
            ty: Type::String,
        };
        let alter_sql = alter_table_add_column_postgres("users", &field, &simple_enums);
        assert_eq!(
            alter_sql,
            "ALTER TABLE \"users\" ADD COLUMN IF NOT EXISTS \"avatar_url\" TEXT;"
        );
    }
}
