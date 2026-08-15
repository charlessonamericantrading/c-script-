// Adaptador y generador de esquemas SQL para PostgreSQL (Link 1.0 Enterprise).
// Provee mapeo de tipos nativo para PostgreSQL (BIGINT, JSONB, DOUBLE PRECISION, TEXT),
// generación de DDL completo, consultas preparadas y soporte de auto-migración no destructiva.

use crate::ast::Program;
use crate::checker::Checker;
use crate::types::{FieldType, Type};
use std::collections::HashSet;

/// Mapeo de tipos de Link a dialecto PostgreSQL nativo.
pub fn link_to_postgres_type(ty: &Type, simple_enums: &HashSet<String>) -> &'static str {
    match ty {
        Type::Int | Type::Int64 | Type::Timestamp => "BIGINT",
        Type::Float => "DOUBLE PRECISION",
        Type::String => "TEXT",
        Type::Bool => "BOOLEAN",
        Type::Enum(name) if simple_enums.contains(name) => "TEXT",
        // Todo tipo compuesto (structs, arrays, mapas, genéricos, uniones) se guarda en JSONB nativo
        _ => "JSONB",
    }
}

/// Genera la sentencia DDL `CREATE TABLE IF NOT EXISTS` para una colección en PostgreSQL.
pub fn create_postgres_table_sql(collection: &str, fields: &[FieldType], simple_enums: &HashSet<String>) -> String {
    let mut cols = vec!["\"id\" BIGSERIAL PRIMARY KEY".to_string()];

    for f in fields {
        let pg_type = link_to_postgres_type(&f.ty, simple_enums);
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

    let mut statements = vec![
        "-- Schema generado automáticamente por Link (PostgreSQL Enterprise Backend)".to_string(),
        "CREATE EXTENSION IF NOT EXISTS \"pgcrypto\";".to_string(),
    ];

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
    let pg_type = link_to_postgres_type(&field.ty, simple_enums);
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
