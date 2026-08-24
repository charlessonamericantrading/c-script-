// Adaptador y generador de esquemas SQL para PostgreSQL (Link 1.0 Enterprise).
// Provee mapeo de tipos nativo para PostgreSQL (BIGINT, JSONB, DOUBLE PRECISION, TEXT),
// generación de DDL completo, consultas preparadas y soporte de auto-migración no destructiva.

use crate::ast::Program;
use crate::checker::Checker;
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
pub fn create_postgres_table_sql(collection: &str, fields: &[FieldType], simple_enums: &HashSet<String>) -> String {
    let mut cols = vec!["\"id\" BIGSERIAL PRIMARY KEY".to_string()];

    for f in fields {
        let pg_type = postgres_column_type(f, simple_enums);
        let not_null = if !f.optional && !matches!(f.ty, Type::Optional(_)) {
            " NOT NULL"
        } else {
            ""
        };
        cols.push(format!("\"{}\" {}{}", f.name, pg_type, not_null));
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

    for (coll_name, elem_ty) in checker.db_collections() {
        if let Type::Struct { fields, .. } = elem_ty {
            let non_id_fields: Vec<FieldType> = fields.iter().filter(|f| f.name != "id").cloned().collect();
            let sql = create_postgres_table_sql(coll_name, &non_id_fields, &simple_enums);
            statements.push(sql);
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
