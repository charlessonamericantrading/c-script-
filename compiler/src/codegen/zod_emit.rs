//! Generador de esquemas Zod (schemas.ts) a partir de tipos Link.
//! Permite validación de formularios en React y runtime type safety con Zod.

use crate::ast::{Item, Program, TypeExpr};
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
                        let zod_ty = render_zod_type(&ty);
                        let optional_suffix = if f.optional { ".optional()" } else { "" };
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
}
