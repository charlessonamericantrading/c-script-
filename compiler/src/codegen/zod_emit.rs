//! Generador de esquemas Zod (schemas.ts) a partir de tipos Link.
//! Permite validación de formularios en React y runtime type safety con Zod.

use crate::ast::{EnumDecl, FieldValidator, Item, Program, TypeDecl, TypeExpr};
use crate::checker::Checker;
use crate::types::Type;

fn render_zod_type(ty: &Type) -> String {
    match ty {
        Type::Int => "z.number().int()".to_string(),
        Type::Int64 => "z.union([z.number().int(), z.string(), z.bigint()])".to_string(),
        // Misma forma fija que validators.ts (GRAMMAR.md §3.184): signo
        // opcional, uno o más dígitos, punto, EXACTAMENTE 4 decimales.
        Type::Decimal => "z.string().regex(/^-?\\d+\\.\\d{4}$/)".to_string(),
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
        // El wire real (y `Result<T, E>` en contract.d.ts, ts_emit.rs) usa
        // `{ type: "Ok" | "Err", ... }` -- NUNCA `{ ok: true | false, ... }`
        // (GRAMMAR.md §2.2, "Result<T,E> viaja siempre como {type:'Ok'|
        // 'Err', ...} en un 200"). Bug real encontrado auditando este
        // archivo: el discriminador y las claves acá usaban `ok`, un campo
        // que ningún payload real tiene -- `z.discriminatedUnion` con la
        // clave equivocada rechaza CUALQUIER `Result` real, sin excepción.
        // Ver GRAMMAR.md §3.131.
        Type::ResultOf(ok_ty, err_ty) => {
            format!(
                "z.discriminatedUnion(\"type\", [\n  z.object({{ type: z.literal(\"Ok\"), value: {} }}),\n  z.object({{ type: z.literal(\"Err\"), error: {} }})\n])",
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

// Bug real, misma familia que el de `Result<T,E>` (GRAMMAR.md §3.131/
// §3.132): esto SIEMPRE emitía `z.enum([...])` -- una unión de strings
// LITERALES -- sin importar si el enum era un ADT con datos por variante
// (`ValidationError { InvalidEmail { field: String }, ... }`, tal cual en
// `examples/users.link`). El wire real de un ADT (`emit_enum_decl`,
// ts_emit.rs) es un objeto con tag `type` más los campos de la variante,
// NUNCA un string pelado -- `z.enum(["InvalidEmail", "TooShort"])` rechaza
// CUALQUIER `ValidationError` real (`{ type: "InvalidEmail", field: "..." }`
// no es el string `"InvalidEmail"`). Mismo criterio `all_unit` que
// `emit_enum_decl` ya usa para decidir entre las dos formas.
fn emit_enum_zod(out: &mut String, e: &EnumDecl, checker: &Checker) -> Result<(), String> {
    let all_unit = e.variants.iter().all(|v| v.fields.is_none());
    if all_unit {
        let variants: Vec<String> = e.variants.iter().map(|v| format!("\"{}\"", v.name)).collect();
        out.push_str(&format!("export const {}Schema = z.enum([{}]);\n", e.name, variants.join(", ")));
    } else {
        out.push_str(&format!("export const {}Schema = z.discriminatedUnion(\"type\", [\n", e.name));
        let mut variant_schemas = Vec::new();
        for v in &e.variants {
            let mut parts = vec![format!("type: z.literal(\"{}\")", v.name)];
            if let Some(fields) = &v.fields {
                for f in fields {
                    // Un ADT genérico (`enum Result<T, E> { Ok { value: T },
                    // ... }`, GRAMMAR.md §2.2 -- distinto del `Result<T,E>`
                    // builtin del lenguaje) tiene campos de variante que
                    // referencian su propio parámetro de tipo (`T`) --
                    // `resolve_type` a secas lo rechaza ("tipo desconocido:
                    // 'T'"), regresión real encontrada por `docs_examples.rs`
                    // al agregar este camino. `resolve_type_abstract` (mismo
                    // criterio que `resolve_field_ty` en ts_emit.rs) deja
                    // `T` como `Type::TypeParam` en vez de fallar --
                    // `render_zod_type` ya tiene un catch-all (`z.unknown()`)
                    // para cualquier tipo sin forma Zod razonable, así que no
                    // hace falta un caso especial acá: Zod no tiene generics
                    // reales como TS, un parámetro de tipo sin instanciar no
                    // tiene schema propio posible.
                    let ty = if e.type_params.is_empty() {
                        checker.resolve_type(&f.ty).map_err(|e| e.to_string())?
                    } else {
                        checker.resolve_type_abstract(&f.ty, &e.type_params).map_err(|e| e.to_string())?
                    };
                    let zod_ty = render_zod_type_for_field(&ty, f.validator());
                    let optional_suffix = if f.optional || f.default.is_some() { ".optional()" } else { "" };
                    parts.push(format!("{}: {}{}", f.name, zod_ty, optional_suffix));
                }
            }
            variant_schemas.push(format!("  z.object({{ {} }})", parts.join(", ")));
        }
        out.push_str(&variant_schemas.join(",\n"));
        out.push_str("\n]);\n");
    }
    out.push_str(&format!("export type {} = z.infer<typeof {}Schema>;\n\n", e.name, e.name));
    Ok(())
}

/// No escribe nada si `t.ty` no es un `TypeExpr::Struct` (un alias, por
/// ejemplo) -- mismo `if let` que el llamador original tenía inline, ahora
/// factorizado para poder reusarse sobre `ExcelSheet`
/// (`checker::excel_sheet_type_decl()`, siempre un struct) sin duplicar el
/// cuerpo.
fn emit_struct_zod(out: &mut String, t: &TypeDecl, checker: &Checker) -> Result<(), String> {
    let TypeExpr::Struct(fields) = &t.ty else { return Ok(()) };
    out.push_str(&format!("export const {}Schema = z.object({{\n", t.name));
    // GRAMMAR.md §3.232: un campo `@hidden` nunca llega al cliente.
    for f in fields.iter().filter(|f| !f.hidden()) {
        // Mismo bug/fix que el de los ADT genéricos arriba (GRAMMAR.md
        // §3.132) -- confirmado a mano contra el binario real: un `type
        // Box<T> = { value: T }` rompía `linkc build` ENTERO ("tipo
        // desconocido: 'T'") con `resolve_type` a secas. `resolve_type_abstract`
        // deja `T` como `Type::TypeParam`, que cae al `z.unknown()`
        // catch-all de `render_zod_type` en vez de fallar.
        let ty = if t.type_params.is_empty() {
            checker.resolve_type(&f.ty).map_err(|e| e.to_string())?
        } else {
            checker.resolve_type_abstract(&f.ty, &t.type_params).map_err(|e| e.to_string())?
        };
        let zod_ty = render_zod_type_for_field(&ty, f.validator());
        // Un campo con `= default` (GRAMMAR.md §3.74) puede omitirse igual
        // que uno `?:` -- `.optional()` nada más, no `.default(...)`: el
        // default es una expresión c-script arbitraria (puede ser
        // `crypto.uuid()`), no algo traducible a JS sin evaluarla, así que
        // quien construye el objeto en TS simplemente no manda la clave y
        // el SERVIDOR es quien la completa (ver runtime/mod.rs::Expr::StructLit).
        let optional_suffix = if f.optional || f.default.is_some() { ".optional()" } else { "" };
        out.push_str(&format!("  {}: {}{},\n", f.name, zod_ty, optional_suffix));
    }
    out.push_str("});\n");
    out.push_str(&format!("export type {} = z.infer<typeof {}Schema>;\n\n", t.name, t.name));
    Ok(())
}

pub fn emit_zod_schemas(program: &Program) -> Result<String, String> {
    let (checker, errors) = Checker::build_symbols(program);
    if let Some(e) = errors.into_iter().next() {
        return Err(e.to_string());
    }

    let mut out = String::new();
    out.push_str(&format!("// Generado automáticamente por linkc v{} — no editar a mano.\n\n", crate::VERSION));
    out.push_str("import { z } from \"zod\";\n\n");

    // `PdfBlock`/`ExcelCell`/`ExcelSheet` (GRAMMAR.md §3.201/§3.202) son ADTs
    // reservados por el compilador -- pre-registrados en `checker.enums`/
    // `checker.types` por `Checker::build_symbols`, NUNCA en `program.items`
    // (no hay texto fuente que parsear para ellos). El loop de abajo, que
    // emite el schema de cualquier `Item::Enum`/`Item::Type` del programa,
    // nunca los ve -- así que `schemas.ts` salía SIN NADA para un programa
    // que solo usaba `pdf`/`excel` (confirmado: archivo vacío salvo el
    // import de `zod`). Se emiten acá, incondicionalmente, antes de iterar
    // `program.items` -- mismo criterio "ADT siempre disponible" que
    // `ts_emit.rs::emit_contract`/`openapi_emit.rs::emit_openapi_json` ya
    // aplican para estos tres tipos.
    emit_enum_zod(&mut out, &crate::checker::pdf_block_enum_decl(), &checker)?;
    emit_enum_zod(&mut out, &crate::checker::excel_cell_enum_decl(), &checker)?;
    emit_struct_zod(&mut out, &crate::checker::excel_sheet_type_decl(), &checker)?;
    emit_struct_zod(&mut out, &crate::checker::ai_message_type_decl(), &checker)?;
    emit_struct_zod(&mut out, &crate::checker::ai_token_type_decl(), &checker)?;

    for item in &program.items {
        match item {
            Item::Enum(e) => emit_enum_zod(&mut out, e, &checker)?,
            Item::Type(t) => emit_struct_zod(&mut out, t, &checker)?,
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

    /// PLAN.md §9.7, GRAMMAR.md §3.83: mismo estampado de versión que
    /// `contract.d.ts`/`client.ts`/`hooks.ts`/`validators.ts`.
    #[test]
    fn header_is_stamped_with_the_compiler_version() {
        let program = parser::parse(lexer::tokenize("type Item = { id: Int }").unwrap()).unwrap();
        let out = emit_zod_schemas(&program).unwrap();
        assert!(
            out.starts_with(&format!("// Generado automáticamente por linkc v{} — no editar a mano.", crate::VERSION)),
            "{out}"
        );
    }

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

    /// Bug real encontrado auditando este archivo (GRAMMAR.md §3.131): el
    /// schema de `Result<T,E>` discriminaba por `"ok"` con `z.literal(true/
    /// false)`, una forma que NINGÚN payload real tiene -- el wire (y
    /// `Result<T, E>` en contract.d.ts) usa `{ type: "Ok"|"Err", ... }`.
    /// `z.discriminatedUnion` con la clave equivocada rechaza CUALQUIER
    /// `Result` real -- verificado a mano con Zod real: la forma vieja
    /// (`{ ok: true, ... }`) queda rechazada por el schema arreglado, y un
    /// payload real (`{ type: "Ok"|"Err", ... }`) sí valida.
    #[test]
    fn result_schema_discriminates_by_the_real_type_field_not_a_fake_ok_field() {
        let code = r#"
            enum ValidationError { InvalidEmail }
            type Task = { id: Int }
            type LastAttempt = { outcome: Result<Task, ValidationError> }
        "#;
        let program = parser::parse(lexer::tokenize(code).unwrap()).unwrap();
        let zod_out = emit_zod_schemas(&program).unwrap();
        assert!(zod_out.contains("z.discriminatedUnion(\"type\", ["), "{zod_out}");
        assert!(zod_out.contains("z.object({ type: z.literal(\"Ok\"), value: TaskSchema })"), "{zod_out}");
        assert!(
            zod_out.contains("z.object({ type: z.literal(\"Err\"), error: ValidationErrorSchema })"),
            "{zod_out}"
        );
        assert!(!zod_out.contains("\"ok\""), "{zod_out}");
        assert!(!zod_out.contains("z.literal(true)"), "{zod_out}");
    }

    /// Bug real, misma familia que el de `Result<T,E>` (GRAMMAR.md §3.132):
    /// un enum ADT (variantes con datos, `ValidationError { InvalidEmail {
    /// field: String }, TooShort { field: String, min: Int } }` -- literal
    /// de `examples/users.link`) generaba `z.enum(["InvalidEmail",
    /// "TooShort"])`, una unión de strings LITERALES -- rechaza CUALQUIER
    /// payload real (`{ type: "InvalidEmail", field: "..." }` no es el
    /// string `"InvalidEmail"`). Ahora genera `z.discriminatedUnion("type",
    /// ...)`, mismo criterio `all_unit` que `emit_enum_decl` (ts_emit.rs) ya
    /// usa para decidir entre las dos formas -- un enum SIN datos (`Status`,
    /// arriba) sigue exactamente igual, `z.enum([...])`.
    #[test]
    fn adt_enum_schema_discriminates_by_type_not_a_bare_string_union() {
        let code = r#"
            enum ValidationError {
                InvalidEmail { field: String },
                TooShort { field: String, min: Int },
            }
        "#;
        let program = parser::parse(lexer::tokenize(code).unwrap()).unwrap();
        let zod_out = emit_zod_schemas(&program).unwrap();
        assert!(zod_out.contains("export const ValidationErrorSchema = z.discriminatedUnion(\"type\", ["), "{zod_out}");
        assert!(
            zod_out.contains("z.object({ type: z.literal(\"InvalidEmail\"), field: z.string() })"),
            "{zod_out}"
        );
        assert!(
            zod_out.contains("z.object({ type: z.literal(\"TooShort\"), field: z.string(), min: z.number().int() })"),
            "{zod_out}"
        );
        assert!(!zod_out.contains("z.enum([\"InvalidEmail\""), "{zod_out}");
    }

    /// Un enum ADT con una variante SIN datos mezclada con variantes CON
    /// datos -- la variante sin datos solo lleva el discriminador `type`,
    /// sin campos extra, igual que `emit_enum_decl` (ts_emit.rs) ya hace.
    #[test]
    fn adt_enum_with_a_unit_variant_mixed_in_only_carries_the_discriminant() {
        let code = r#"
            enum Shape {
                Circle { radius: Float },
                Point,
            }
        "#;
        let program = parser::parse(lexer::tokenize(code).unwrap()).unwrap();
        let zod_out = emit_zod_schemas(&program).unwrap();
        assert!(zod_out.contains("z.object({ type: z.literal(\"Circle\"), radius: z.number() })"), "{zod_out}");
        assert!(zod_out.contains("z.object({ type: z.literal(\"Point\") })"), "{zod_out}");
    }

    /// Regresión real, encontrada por `docs_examples.rs` al agregar el
    /// branch de ADT arriba: un ADT GENÉRICO (`enum Result<T, E> { Ok {
    /// value: T }, ... }`, el ejemplo educativo de GRAMMAR.md/docs -- no el
    /// `Result<T,E>` builtin del lenguaje) con un campo de variante que
    /// referencia su propio parámetro de tipo (`T`) hacía fallar `linkc
    /// build` ENTERO ("error de tipos: tipo desconocido: 'T'") -- antes de
    /// esta ronda, el código viejo nunca miraba los campos de una variante,
    /// así que nunca pisaba este error (aunque el schema que producía --
    /// `z.enum(["Ok","Err"])` -- ya era igual de incorrecto). Zod no tiene
    /// generics reales como TS -- un parámetro de tipo sin instanciar no
    /// tiene ningún schema Zod razonable posible, así que cae al
    /// `z.unknown()` catch-all de `render_zod_type`, sin que el build
    /// entero se rompa.
    #[test]
    fn a_generic_adt_enum_does_not_crash_the_whole_build() {
        let code = r#"
            enum MyResult<T, E> {
                Ok { value: T },
                Err { error: E },
            }
        "#;
        let program = parser::parse(lexer::tokenize(code).unwrap()).unwrap();
        let zod_out = emit_zod_schemas(&program).expect("no debería fallar sobre un ADT genérico");
        assert!(zod_out.contains("z.object({ type: z.literal(\"Ok\"), value: z.unknown() })"), "{zod_out}");
        assert!(zod_out.contains("z.object({ type: z.literal(\"Err\"), error: z.unknown() })"), "{zod_out}");
    }

    /// Mismo bug que la de arriba, pero para un `type` GENÉRICO (`Box<T> =
    /// { value: T }`) en vez de un `enum` -- confirmado a mano contra el
    /// binario real (`linkc build` sobre un programa con `Box<T>` rompía en
    /// `schemas.ts` con "tipo desconocido: 'T'" antes de este fix).
    #[test]
    fn a_generic_struct_does_not_crash_the_whole_build() {
        let code = r#"type Box<T> = { value: T }"#;
        let program = parser::parse(lexer::tokenize(code).unwrap()).unwrap();
        let zod_out = emit_zod_schemas(&program).expect("no debería fallar sobre un struct genérico");
        assert!(zod_out.contains("export const BoxSchema = z.object({\n  value: z.unknown(),\n});"), "{zod_out}");
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

    /// Auditoría del lenguaje (2026-09-01), GRAMMAR.md §3.204: `PdfBlock`/
    /// `ExcelCell`/`ExcelSheet` (§3.201/§3.202) son ADTs reservados por el
    /// compilador, pre-registrados en `checker.enums`/`checker.types` --
    /// NUNCA aparecen en `program.items`, así que el loop de
    /// `emit_zod_schemas` que emite el schema de cualquier `Item::Enum`/
    /// `Item::Type` nunca los veía. `schemas.ts` salía COMPLETAMENTE VACÍO
    /// (solo el import de `zod`) para un programa que usaba `pdf`/`excel`,
    /// confirmado antes del fix contra el binario real.
    #[test]
    fn pdf_and_excel_reserved_types_always_get_a_zod_schema() {
        let program = parser::parse(lexer::tokenize("type Item = { id: Int }").unwrap()).unwrap();
        let zod_out = emit_zod_schemas(&program).unwrap();
        assert!(zod_out.contains("export const PdfBlockSchema = z.discriminatedUnion(\"type\", ["), "{zod_out}");
        assert!(zod_out.contains("export const ExcelCellSchema = z.discriminatedUnion(\"type\", ["), "{zod_out}");
        assert!(zod_out.contains("export const ExcelSheetSchema = z.object({"), "{zod_out}");
        // `ExcelSheet.rows: ExcelCell[][]` referencia el schema de
        // `ExcelCell` por nombre, no `z.unknown()`.
        assert!(zod_out.contains("rows: z.array(z.array(ExcelCellSchema)),"), "{zod_out}");
    }
}
