//! Generador de esquemas Zod (schemas.ts) a partir de tipos Link.
//! Permite validación de formularios en React y runtime type safety con Zod.

use crate::ast::{FieldValidator, Item, Program, TypeExpr};
use crate::checker::Checker;
use crate::types::Type;

fn render_zod_type(ty: &Type) -> String {
    match ty {
        Type::Int => "z.number().int()".to_string(),
        Type::Int64 => "z.union([z.number().int(), z.string(), z.bigint()])".to_string(),
        Type::Float => "z.number()".to_string(),
        Type::String => "z.string()".to_string(),
        // Misma regex canónica que validators.ts (GRAMMAR.md §3.70) --
        // Zod ya trae `.uuid()`, pero valida contra RFC 4122 estricto
        // (exige el nibble de versión) y ninguna otra capa de este
        // proyecto lo hace -- una regex propia mantiene el mismo criterio
        // en los tres lugares (runtime, validators.ts, schemas.ts).
        Type::Uuid => {
            "z.string().regex(/^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/)".to_string()
        }
        Type::Bool => "z.boolean()".to_string(),
        Type::Void => "z.void()".to_string(),
        Type::Timestamp => "z.string().datetime()".to_string(),
        Type::Optional(inner) => format!("{}.nullable()", render_zod_type(inner)),
        Type::List(elem) => format!("z.array({})", render_zod_type(elem)),
        Type::MapOf(k, v) => format!("z.record({}, {})", render_zod_type(k), render_zod_type(v)),
        Type::Tuple(elems) => {
            let rendered: Vec<_> = elems.iter().map(render_zod_type).collect();
            format!("z.tuple([{}])", rendered.join(", "))
        }
        Type::Struct { name: Some(n), .. } => format!("{}Schema", n),
        Type::Struct { name: None, fields } => {
            let mut parts = Vec::new();
            for f in fields {
                let f_ty = render_zod_type(&f.ty);
                let optional_suffix = if f.optional { ".optional()" } else { "" };
                parts.push(format!("  {}: {}{}", f.name, f_ty, optional_suffix));
            }
            format!("z.object({{\n{}\n}})", parts.join(",\n"))
        }
        Type::Enum(name) => format!("{}Schema", name),
        Type::ResultOf(ok_ty, err_ty) => {
            format!(
                "z.discriminatedUnion(\"ok\", [\n  z.object({{ ok: z.literal(true), value: {} }}),\n  z.object({{ ok: z.literal(false), error: {} }})\n])",
                render_zod_type(ok_ty),
                render_zod_type(err_ty)
            )
        }
        Type::PatchOf(inner) => {
            format!("{}.partial()", render_zod_type(inner))
        }
        Type::Union(members) => {
            let rendered: Vec<_> = members.iter().map(render_zod_type).collect();
            format!("z.union([{}])", rendered.join(", "))
        }
        Type::Generic(name, args) => {
            if args.is_empty() {
                format!("{}Schema", name)
            } else {
                let rendered_args: Vec<_> = args.iter().map(render_zod_type).collect();
                format!("{}Schema({})", name, rendered_args.join(", "))
            }
        }
        _ => "z.unknown()".to_string(),
    }
}

/// Igual que `render_zod_type`, pero aplica `@validate(...)` (GRAMMAR.md
/// §3.73) sobre el `String` de la HOJA, antes de cualquier `.nullable()` --
/// `.email()`/`.regex()` no existen sobre el `ZodNullable` que devuelve
/// `.nullable()`, así que el orden de encadenado importa: tiene que
/// aplicarse ANTES, no después, de ahí que esto no sea un simple postfijo
/// sobre `render_zod_type(ty)`. `validator` solo se usa en la hoja `String`
/// -- el checker (`check_field_validators`) ya garantiza que `@validate`
/// nunca aparece sobre otra cosa, así que no hace falta propagarlo a
/// ninguna otra rama.
fn render_zod_type_for_field(ty: &Type, validator: Option<&FieldValidator>) -> String {
    match ty {
        Type::Optional(inner) => format!("{}.nullable()", render_zod_type_for_field(inner, validator)),
        Type::String => {
            let base = render_zod_type(ty);
            match validator {
                // `new RegExp(json_string)` en vez de un literal `/.../` --
                // evita tener que escapar `/` dentro del patrón del usuario
                // para no cerrar el literal antes de tiempo (`serde_json`
                // ya produce un string JS válido, entre comillas dobles).
                Some(FieldValidator::Email) => format!("{base}.email()"),
                Some(FieldValidator::Regex(pattern)) => {
                    format!("{base}.regex(new RegExp({}))", serde_json::to_string(pattern).expect("string simple"))
                }
                None => base,
            }
        }
        other => render_zod_type(other),
    }
}

pub fn emit_zod_schemas(program: &Program) -> Result<String, String> {
    let (checker, errors) = Checker::build_symbols(program);
    if let Some(e) = errors.into_iter().next() {
        return Err(e.to_string());
    }

    let mut out = String::new();
    out.push_str("// Generado automáticamente por linkc — no editar a mano.\n\n");
    out.push_str("import { z } from \"zod\";\n\n");

    for item in &program.items {
        match item {
            Item::Enum(e) => {
                let variants: Vec<String> = e.variants.iter().map(|v| format!("\"{}\"", v.name)).collect();
                out.push_str(&format!("export const {}Schema = z.enum([{}]);\n", e.name, variants.join(", ")));
                out.push_str(&format!("export type {} = z.infer<typeof {}Schema>;\n\n", e.name, e.name));
            }
            Item::Type(t) => {
                if let TypeExpr::Struct(fields) = &t.ty {
                    out.push_str(&format!("export const {}Schema = z.object({{\n", t.name));
                    for f in fields {
                        let ty = checker.resolve_type(&f.ty).map_err(|e| e.to_string())?;
                        let zod_ty = render_zod_type_for_field(&ty, f.validator());
                        // Un campo con `= default` (GRAMMAR.md §3.74) puede
                        // omitirse igual que uno `?:` -- `.optional()` nada
                        // más, no `.default(...)`: el default es una
                        // expresión c-script arbitraria (puede ser
                        // `crypto.uuid()`), no algo traducible a JS sin
                        // evaluarla, así que quien construye el objeto en TS
                        // simplemente no manda la clave y el SERVIDOR es
                        // quien la completa (ver runtime/mod.rs::Expr::StructLit).
                        let optional_suffix = if f.optional || f.default.is_some() { ".optional()" } else { "" };
                        out.push_str(&format!("  {}: {}{},\n", f.name, zod_ty, optional_suffix));
                    }
                    out.push_str("});\n");
                    out.push_str(&format!("export type {} = z.infer<typeof {}Schema>;\n\n", t.name, t.name));
                }
            }
            _ => {}
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer;
    use crate::parser;

    #[test]
    fn test_zod_emit_generates_valid_schemas() {
        let code = r#"
            enum Status { Active, Inactive }
            type User = {
                id: Int,
                name: String,
                email: String?,
                status: Status,
                createdAt: Timestamp,
            }
        "#;
        let tokens = lexer::tokenize(code).unwrap();
        let program = parser::parse(tokens).unwrap();
        let zod_out = emit_zod_schemas(&program).unwrap();

        assert!(zod_out.contains("export const StatusSchema = z.enum([\"Active\", \"Inactive\"]);"), "{zod_out}");
        assert!(zod_out.contains("export const UserSchema = z.object({"), "{zod_out}");
        assert!(zod_out.contains("id: z.number().int()"), "{zod_out}");
        assert!(zod_out.contains("email: z.string().nullable()"), "{zod_out}");
        assert!(zod_out.contains("createdAt: z.string().datetime()"), "{zod_out}");
    }

    /// `@validate(email)` se propaga como `.email()` encadenado (GRAMMAR.md
    /// §3.73).
    #[test]
    fn validate_email_becomes_a_chained_email_call() {
        let code = r#"type Signup = { @validate(email) email: String }"#;
        let tokens = lexer::tokenize(code).unwrap();
        let program = parser::parse(tokens).unwrap();
        let zod_out = emit_zod_schemas(&program).unwrap();
        assert!(zod_out.contains("email: z.string().email(),"), "{zod_out}");
    }

    /// `@validate(regex, "...")` se propaga como `.regex(new RegExp(...))`
    /// -- `new RegExp` en vez de un literal `/.../` para no tener que
    /// escapar `/` dentro del patrón del usuario.
    #[test]
    fn validate_regex_becomes_a_chained_regex_call_using_new_regexp() {
        let code = r#"type Order = { @validate(regex, "^[A-Z]{3}$") sku: String }"#;
        let tokens = lexer::tokenize(code).unwrap();
        let program = parser::parse(tokens).unwrap();
        let zod_out = emit_zod_schemas(&program).unwrap();
        assert!(zod_out.contains(r#"sku: z.string().regex(new RegExp("^[A-Z]{3}$")),"#), "{zod_out}");
    }

    /// Sobre un campo `String?`, `.email()`/`.regex()` tienen que ir ANTES
    /// de `.nullable()` -- `ZodNullable` no tiene esos métodos, así que el
    /// orden de encadenado no es cosmético, es lo único que compila.
    #[test]
    fn validate_on_an_optional_string_field_chains_email_before_nullable() {
        let code = r#"type Signup = { @validate(email) email: String? }"#;
        let tokens = lexer::tokenize(code).unwrap();
        let program = parser::parse(tokens).unwrap();
        let zod_out = emit_zod_schemas(&program).unwrap();
        assert!(zod_out.contains("email: z.string().email().nullable(),"), "{zod_out}");
    }

    /// Un campo con `= default` (GRAMMAR.md §3.74) se emite `.optional()`
    /// -- puede omitirse igual que uno `?:`.
    #[test]
    fn a_field_with_a_default_is_marked_optional() {
        let code = r#"type Task = { title: String, status: String = "pending" }"#;
        let tokens = lexer::tokenize(code).unwrap();
        let program = parser::parse(tokens).unwrap();
        let zod_out = emit_zod_schemas(&program).unwrap();
        assert!(zod_out.contains("title: z.string(),"), "{zod_out}");
        assert!(zod_out.contains("status: z.string().optional(),"), "{zod_out}");
    }
}
