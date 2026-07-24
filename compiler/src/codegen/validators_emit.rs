// Emisor de validadores runtime (`validators.ts`) -- planeado desde el
// documento original (PLAN.md líneas 197, 203: "esto es lo que hace la
// seguridad REAL en el borde, no solo compile-time") pero nunca construido.
// `ts_emit.rs` solo emitía `.d.ts` y `client.ts`; esto agrega el tercer
// output sin tocar ninguno de los dos (salvo el wireo en emit_client).
//
// Generación por TIPO CONCRETO alcanzado, no por declaración con nombre --
// evita el problema de Type::Generic opaco (GRAMMAR.md §3.6): nunca hace
// falta un validador para `Box<T>` abstracto, solo para instanciaciones
// concretas (`Box<Int>`) que ya aparecen resueltas en cada firma de rpc.
// Predicados TS a mano (`function isX(x): x is X`), no Zod/typia -- mismo
// criterio de cero dependencias nuevas que el resto del proyecto.

use super::ts_emit;
use crate::ast::*;
use crate::checker::Checker;
use crate::types::{FieldType, Type};
use std::collections::BTreeSet;

pub fn emit_validators(program: &Program) -> Result<String, String> {
    let (checker, errors) = Checker::build_symbols(program);
    if let Some(e) = errors.into_iter().next() {
        return Err(e.to_string());
    }

    let mut worklist: Vec<Type> = Vec::new();
    let mut seen: Vec<Type> = Vec::new();

    // Semillas: cada firma de rpc que efectivamente cruza el wire -- lo
    // mismo que ya recorre emit_client para collect_type_names (ts_emit.rs).
    for item in &program.items {
        if let Item::Service(s) = item {
            for m in &s.members {
                let rpc = match m {
                    Member::Rpc(r) | Member::Stream(r) => r,
                };
                for p in &rpc.params {
                    let ty = checker.resolve_type(&p.ty).map_err(|e| e.to_string())?;
                    enqueue(&ty, &mut worklist, &mut seen);
                }
                let ret_ty = checker.resolve_type(&rpc.return_type).map_err(|e| e.to_string())?;
                enqueue(&ret_ty, &mut worklist, &mut seen);
            }
        }
    }

    // Worklist, no una sola pasada: emitir el cuerpo de un validador puede
    // descubrir OTROS tipos con nombre anidados (ej. User referencia Role) --
    // se van encolando a medida que aparecen, hasta vaciar la cola. `seen`
    // sigue creciendo durante este loop, así que el import (más abajo) se
    // arma DESPUÉS de vaciarlo del todo, no antes.
    let mut body = String::new();
    while let Some(ty) = worklist.pop() {
        emit_named_validator(&mut body, &ty, &checker, &mut worklist, &mut seen)?;
    }

    let mut out = String::new();
    out.push_str("// Generado automáticamente por linkc — no editar a mano.\n\n");

    // Cada firma `x is X` que se emitió referencia el nombre de tipo X --
    // hay que importarlo de "./contract", igual que ya hace client.ts
    // (reusa collect_type_names, ts_emit.rs, para no duplicar esa lógica).
    // Sin esto, además, un archivo sin ningún import/export no es un módulo
    // TS real -- tsc lo trataría como script ambiental.
    let mut imported_names = BTreeSet::new();
    for ty in &seen {
        ts_emit::collect_type_names(ty, &mut imported_names);
    }
    if !imported_names.is_empty() {
        out.push_str(&format!(
            "import type {{ {} }} from \"./contract\";\n\n",
            imported_names.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }

    out.push_str(&body);
    Ok(out)
}

/// Encola `ty` -- y, si `ty` no tiene validador propio, todo lo que
/// CONTENGA que sí lo tenga.
///
/// Ese segundo caso faltaba y era un bug real: un service cuyo único rpc
/// devolvía `C[]` (o `C?`, o una tupla con un `C`) generaba un `client.ts`
/// con `import { isC } from "./validators.ts"` y un `validators.ts` que no
/// definía `isC` -- el frontend generado no compilaba. `examples/users.link`
/// se salvaba de casualidad porque `isUser` se sembraba igual vía
/// `create() -> Result<User, ...>` y `update() -> User`.
///
/// La causa de fondo era que esta función y `collect_validator_names` (que
/// decide qué IMPORTA client.ts) recorrían el tipo con criterios distintos.
/// Tienen que coincidir exactamente: una emite y la otra importa lo mismo.
fn enqueue(ty: &Type, worklist: &mut Vec<Type>, seen: &mut Vec<Type>) {
    if validator_fn_name(ty).is_some() {
        if !seen.contains(ty) {
            seen.push(ty.clone());
            worklist.push(ty.clone());
        }
        return;
    }
    // Sin nombre propio: se valida inline, pero puede envolver tipos que sí
    // necesitan su propia función.
    match ty {
        Type::Optional(inner) | Type::List(inner) => enqueue(inner, worklist, seen),
        Type::Tuple(items) => {
            for i in items {
                enqueue(i, worklist, seen);
            }
        }
        Type::Function(params, ret) => {
            for p in params {
                enqueue(p, worklist, seen);
            }
            enqueue(ret, worklist, seen);
        }
        Type::Struct { fields, .. } => {
            for f in fields {
                enqueue(&f.ty, worklist, seen);
            }
        }
        Type::MapOf(k, v) => {
            enqueue(k, worklist, seen);
            enqueue(v, worklist, seen);
        }
        Type::Union(members) => {
            for m in members {
                enqueue(m, worklist, seen);
            }
        }
        _ => {}
    }
}

/// Expresión de chequeo para `ty`, sin efecto de encolado -- para usar
/// desde `ts_emit.rs::emit_client`, que llama a los validadores ya
/// emitidos por `emit_validators` (mismas firmas de rpc como semilla en
/// ambos, así que nunca hace falta descubrir un tipo nuevo acá).
pub(crate) fn render_check_expr(ty: &Type, expr: &str) -> String {
    let mut worklist = Vec::new();
    let mut seen = Vec::new();
    render_check(ty, expr, &mut worklist, &mut seen)
}

/// Nombres de función validadora que un `client.ts` que valide `ty`
/// necesita importar de "./validators" -- se detiene apenas encuentra un
/// tipo con nombre propio (su expansión interna vive DENTRO de
/// validators.ts, `client.ts` nunca la necesita directamente), igual que
/// `render_check` corta ahí mismo.
pub(crate) fn collect_validator_names(ty: &Type, names: &mut std::collections::BTreeSet<String>) {
    if let Some(fn_name) = validator_fn_name(ty) {
        names.insert(fn_name);
        return;
    }
    match ty {
        Type::Optional(inner) | Type::List(inner) => collect_validator_names(inner, names),
        Type::Tuple(items) => {
            for i in items {
                collect_validator_names(i, names);
            }
        }
        Type::Function(params, ret) => {
            for p in params {
                collect_validator_names(p, names);
            }
            collect_validator_names(ret, names);
        }
        Type::Struct { fields, .. } => {
            for f in fields {
                collect_validator_names(&f.ty, names);
            }
        }
        Type::MapOf(k, v) => {
            collect_validator_names(k, names);
            collect_validator_names(v, names);
        }
        Type::Union(members) => {
            for m in members {
                collect_validator_names(m, names);
            }
        }
        _ => {}
    }
}

/// Nombre de la función validadora para un tipo con identidad propia, o
/// `None` para tipos puramente estructurales (siempre se validan inline,
/// nunca con una función aparte) -- misma división que hace `render_type`
/// entre "tiene nombre propio" y "se renderiza inline".
fn validator_fn_name(ty: &Type) -> Option<String> {
    match ty {
        Type::Struct { name: Some(n), .. } => Some(format!("is{n}")),
        Type::Enum(n) => Some(format!("is{n}")),
        Type::ResultOf(a, b) => Some(format!("isResult_{}_{}", type_key(a), type_key(b))),
        Type::PatchOf(inner) => Some(format!("isPatch_{}", type_key(inner))),
        Type::Generic(name, args) => {
            Some(format!("is{name}_{}", args.iter().map(type_key).collect::<Vec<_>>().join("_")))
        }
        _ => None,
    }
}

/// Fragmento de identificador único-al-alcance-que-hace-falta para un tipo
/// -- solo se usa para ARMAR nombres de función compuestos (`isResult_Int_
/// String`), nunca se muestra al usuario. No es lo mismo que `render_type`
/// (que produce sintaxis TS real).
fn type_key(ty: &Type) -> String {
    match ty {
        Type::Int => "Int".into(),
        Type::Float => "Float".into(),
        Type::String => "String".into(),
        Type::Bool => "Bool".into(),
        Type::Void => "Void".into(),
        Type::Null => "Null".into(),
        Type::Dynamic => "Dynamic".into(),
        Type::Optional(inner) => format!("Optional{}", type_key(inner)),
        Type::List(inner) => format!("ListOf{}", type_key(inner)),
        Type::Tuple(items) => format!("Tuple{}", items.iter().map(type_key).collect::<Vec<_>>().join("")),
        Type::Function(_, _) => "Fn".into(),
        Type::Struct { name: Some(n), .. } => n.clone(),
        Type::Struct { name: None, fields } => {
            format!("Anon{}", fields.iter().map(|f| f.name.clone()).collect::<Vec<_>>().join(""))
        }
        Type::Enum(n) => n.clone(),
        Type::ResultOf(a, b) => format!("Result{}{}", type_key(a), type_key(b)),
        Type::PatchOf(inner) => format!("Patch{}", type_key(inner)),
        Type::MapOf(k, v) => format!("MapOf{}{}", type_key(k), type_key(v)),
        Type::Generic(name, args) => format!("{name}{}", args.iter().map(type_key).collect::<Vec<_>>().join("")),
        Type::TypeParam(name) => name.clone(),
        Type::Union(members) => format!("Union{}", members.iter().map(type_key).collect::<Vec<_>>().join("")),
        Type::Db | Type::DbCollection(_) => {
            unreachable!("Type::Db/DbCollection nunca aparece en un TypeExpr real")
        }
    }
}

/// Expresión booleana TS que valida `expr` contra `ty`. Para un tipo con
/// nombre propio, es solo una llamada a su función (y lo encola si es la
/// primera vez que aparece); para uno estructural, se resuelve inline --
/// misma división de responsabilidades que `render_type`/`render_type_atom`
/// en ts_emit.rs.
fn render_check(ty: &Type, expr: &str, worklist: &mut Vec<Type>, seen: &mut Vec<Type>) -> String {
    if let Some(fn_name) = validator_fn_name(ty) {
        enqueue(ty, worklist, seen);
        return format!("{fn_name}({expr})");
    }
    match ty {
        Type::Int => format!("(typeof {expr} === \"number\" && Number.isInteger({expr}))"),
        Type::Float => format!("typeof {expr} === \"number\""),
        Type::String => format!("typeof {expr} === \"string\""),
        Type::Bool => format!("typeof {expr} === \"boolean\""),
        // Void nunca es un payload real (solo retorno de rpc, tabla de
        // mapeo GRAMMAR.md §4) -- no hay nada que validar.
        Type::Void => "true".to_string(),
        Type::Null => format!("{expr} === null"),
        // Dynamic es "unknown" a propósito (GRAMMAR.md, doc de Type::Dynamic)
        // -- no hay ninguna forma real que exigir todavía.
        Type::Dynamic => "true".to_string(),
        Type::Optional(inner) => format!("({expr} === null || {})", render_check(inner, expr, worklist, seen)),
        Type::List(inner) => {
            // `(item: unknown)` explícito, no inferido -- `expr` suele ser
            // un acceso de campo bajo `as any` (render_struct_fields_check),
            // y TS no siempre re-angosta ese `any` en la SEGUNDA aparición
            // de la misma expresión (acá, la del `.every`), dejando el
            // parámetro del callback como `any` implícito bajo `strict`.
            format!(
                "(Array.isArray({expr}) && {expr}.every((item: unknown) => {}))",
                render_check(inner, "item", worklist, seen)
            )
        }
        Type::Tuple(items) => {
            let checks: Vec<String> = items
                .iter()
                .enumerate()
                .map(|(i, t)| render_check(t, &format!("{expr}[{i}]"), worklist, seen))
                .collect();
            format!("(Array.isArray({expr}) && {expr}.length === {} && {})", items.len(), checks.join(" && "))
        }
        // No debería cruzar el wire nunca (tabla de mapeo §4: "solo como
        // campo de tipo función local") -- chequeo mínimo, no exhaustivo.
        Type::Function(_, _) => format!("typeof {expr} === \"function\""),
        Type::Struct { fields, .. } => render_struct_fields_check(fields, expr, worklist, seen),
        Type::MapOf(k, v) => {
            // Claves de un objeto JS siempre son string -- Int como clave
            // (GRAMMAR.md §4, K limitado a String/Int) se valida parseando
            // el string de vuelta, no con `typeof`.
            let key_check = match k.as_ref() {
                Type::Int => "Number.isInteger(Number(k))".to_string(),
                _ => render_check(k, "k", worklist, seen),
            };
            format!(
                "(typeof {expr} === \"object\" && {expr} !== null && Object.entries({expr}).every(([k, v]: [string, unknown]) => {key_check} && {}))",
                render_check(v, "v", worklist, seen)
            )
        }
        Type::Union(members) => {
            let checks: Vec<String> =
                members.iter().map(|m| render_check(m, expr, worklist, seen)).collect();
            format!("({})", checks.join(" || "))
        }
        // No debería aparecer en un tipo ya resuelto de una firma real (solo
        // en la declaración ABSTRACTA de un genérico, ver resolve_type_abstract).
        Type::TypeParam(_) => "true".to_string(),
        // Enum/ResultOf/PatchOf/Generic ya volvieron arriba vía
        // validator_fn_name -- nunca se llega acá para esos (Struct{name:
        // Some(_),..} tampoco, pero ese caso ya lo cubre el brazo de arriba
        // sin distinguir por nombre, así que no hace falta repetirlo).
        Type::Enum(_) | Type::ResultOf(_, _) | Type::PatchOf(_) | Type::Generic(_, _) => {
            unreachable!("cubierto por validator_fn_name")
        }
        Type::Db | Type::DbCollection(_) => {
            unreachable!("Type::Db/DbCollection nunca aparece en un TypeExpr real")
        }
    }
}

fn render_struct_fields_check(
    fields: &[FieldType],
    expr: &str,
    worklist: &mut Vec<Type>,
    seen: &mut Vec<Type>,
) -> String {
    let checks: Vec<String> = fields
        .iter()
        .map(|f| {
            let field_expr = format!("({expr} as any).{}", f.name);
            if f.optional {
                format!("({field_expr} === undefined || {})", render_check(&f.ty, &field_expr, worklist, seen))
            } else {
                render_check(&f.ty, &field_expr, worklist, seen)
            }
        })
        .collect();
    let body = if checks.is_empty() { "true".to_string() } else { checks.join(" && ") };
    format!("(typeof {expr} === \"object\" && {expr} !== null && {body})")
}

/// Igual que `render_struct_fields_check`, pero TODOS los campos vuelven
/// opcionales -- la contraparte de `Patch<T>` = `Partial<T>` (render_type,
/// ts_emit.rs) del lado de validación: omitido = no tocar, siempre válido.
fn render_patch_fields_check(
    fields: &[FieldType],
    expr: &str,
    worklist: &mut Vec<Type>,
    seen: &mut Vec<Type>,
) -> String {
    let checks: Vec<String> = fields
        .iter()
        .map(|f| {
            let field_expr = format!("({expr} as any).{}", f.name);
            format!("({field_expr} === undefined || {})", render_check(&f.ty, &field_expr, worklist, seen))
        })
        .collect();
    let body = if checks.is_empty() { "true".to_string() } else { checks.join(" && ") };
    format!("(typeof {expr} === \"object\" && {expr} !== null && {body})")
}

/// Cuerpo de un enum (simple o con datos) -- `variants` ya trae los nombres
/// (de `EnumDecl` crudo, para el chequeo all-unit igual que `emit_enum_decl`
/// en ts_emit.rs) y `scrutinee_ty` es lo que se le pasa a
/// `Checker::variant_field_types` para resolver los campos de cada variante
/// (maneja Result/enum genérico/enum plano de forma uniforme).
fn render_enum_check(
    checker: &Checker,
    scrutinee_ty: &Type,
    enum_name: &str,
    all_unit: bool,
    variant_names: &[String],
    expr: &str,
    worklist: &mut Vec<Type>,
    seen: &mut Vec<Type>,
) -> Result<String, String> {
    if all_unit {
        let checks: Vec<String> = variant_names.iter().map(|v| format!("{expr} === \"{v}\"")).collect();
        return Ok(format!("({})", checks.join(" || ")));
    }
    let mut arms = Vec::new();
    for v in variant_names {
        let fields = checker
            .variant_field_types(scrutinee_ty, enum_name, v)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|(name, ty)| FieldType { name, optional: false, ty })
            .collect::<Vec<_>>();
        let field_checks = render_struct_fields_check(&fields, expr, worklist, seen);
        arms.push(format!("(({expr} as any).type === \"{v}\" && {field_checks})"));
    }
    Ok(format!(
        "(typeof {expr} === \"object\" && {expr} !== null && ({}))",
        arms.join(" || ")
    ))
}

fn emit_named_validator(
    out: &mut String,
    ty: &Type,
    checker: &Checker,
    worklist: &mut Vec<Type>,
    seen: &mut Vec<Type>,
) -> Result<(), String> {
    let fn_name = validator_fn_name(ty).expect("solo se llama para tipos con validator_fn_name propio");
    match ty {
        Type::Struct { name: Some(n), fields } => {
            let body = render_struct_fields_check(fields, "x", worklist, seen);
            out.push_str(&format!("export function {fn_name}(x: unknown): x is {n} {{\n  return {body};\n}}\n\n"));
        }
        Type::Enum(name) => {
            let decl = checker.enums.get(name).ok_or_else(|| format!("enum desconocido: '{name}'"))?;
            let all_unit = decl.variants.iter().all(|v| v.fields.is_none());
            let variant_names: Vec<String> = decl.variants.iter().map(|v| v.name.clone()).collect();
            let body = render_enum_check(checker, ty, name, all_unit, &variant_names, "x", worklist, seen)?;
            out.push_str(&format!("export function {fn_name}(x: unknown): x is {name} {{\n  return {body};\n}}\n\n"));
        }
        Type::ResultOf(ok_ty, err_ty) => {
            // Forma fija de Result<T,E> (Ok{value:T}/Err{error:E}) -- no
            // viene de ningún EnumDecl declarado por el usuario, así que se
            // arma a mano en vez de pasar por variant_field_types.
            let ok_fields = vec![FieldType { name: "value".into(), optional: false, ty: (**ok_ty).clone() }];
            let err_fields = vec![FieldType { name: "error".into(), optional: false, ty: (**err_ty).clone() }];
            let ok_check = render_struct_fields_check(&ok_fields, "x", worklist, seen);
            let err_check = render_struct_fields_check(&err_fields, "x", worklist, seen);
            out.push_str(&format!(
                "export function {fn_name}(x: unknown): x is Result<{}, {}> {{\n  return (typeof x === \"object\" && x !== null && ((((x as any).type === \"Ok\") && {ok_check}) || (((x as any).type === \"Err\") && {err_check})));\n}}\n\n",
                ts_emit::render_type(ok_ty), ts_emit::render_type(err_ty)
            ));
        }
        Type::PatchOf(inner) => {
            let Type::Struct { fields, .. } = inner.as_ref() else {
                return Err(format!(
                    "Patch<T> requiere que T resuelva a un struct -- inesperado en el emisor: {inner:?}"
                ));
            };
            let body = render_patch_fields_check(fields, "x", worklist, seen);
            out.push_str(&format!(
                "export function {fn_name}(x: unknown): x is Patch<{}> {{\n  return {body};\n}}\n\n",
                ts_emit::render_type(inner)
            ));
        }
        Type::Generic(name, args) => {
            // A diferencia de Result/Patch (siempre en checking-mode
            // opaco), acá SÍ hace falta el tipo real (`Box<number>`, no el
            // nombre de la función) para que la firma `x is ...` compile --
            // reusa render_type (ts_emit.rs), la misma fuente de verdad que
            // ya usa el resto del contrato.
            let type_annotation = ts_emit::render_type(ty);
            if checker.types.contains_key(name) {
                let fields = checker.expand_generic_struct(name, args).map_err(|e| e.to_string())?;
                let body = render_struct_fields_check(&fields, "x", worklist, seen);
                out.push_str(&format!(
                    "export function {fn_name}(x: unknown): x is {type_annotation} {{\n  return {body};\n}}\n\n"
                ));
            } else if let Some(decl) = checker.enums.get(name) {
                let all_unit = decl.variants.iter().all(|v| v.fields.is_none());
                let variant_names: Vec<String> = decl.variants.iter().map(|v| v.name.clone()).collect();
                let body = render_enum_check(checker, ty, name, all_unit, &variant_names, "x", worklist, seen)?;
                out.push_str(&format!(
                    "export function {fn_name}(x: unknown): x is {type_annotation} {{\n  return {body};\n}}\n\n"
                ));
            } else {
                return Err(format!("tipo genérico desconocido: '{name}'"));
            }
        }
        other => return Err(format!("emit_named_validator llamado sobre un tipo sin nombre: {other:?}")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::parse;

    fn emit(src: &str) -> String {
        let tokens = tokenize(src).unwrap_or_else(|e| panic!("{e}"));
        let program = parse(tokens).unwrap_or_else(|e| panic!("{e}"));
        emit_validators(&program).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn struct_gets_a_type_predicate_checking_every_field() {
        let src = r#"
            type Point = { x: Int, y: Int }
            service S { rpc get() -> Point { db.thing.get() } }
        "#;
        let out = emit(src);
        assert!(out.contains("function isPoint(x: unknown): x is Point"));
        assert!(out.contains("Number.isInteger"));
    }

    #[test]
    fn optional_field_allows_undefined() {
        let src = r#"
            type Profile = { bio?: String }
            service S { rpc get() -> Profile { db.thing.get() } }
        "#;
        let out = emit(src);
        assert!(out.contains("=== undefined ||"));
    }

    #[test]
    fn simple_enum_checks_against_string_literals() {
        let src = r#"
            enum Role { Admin, Member, Guest }
            service S { rpc get() -> Role { db.thing.get() } }
        "#;
        let out = emit(src);
        assert!(out.contains("function isRole(x: unknown): x is Role"));
        assert!(out.contains("x === \"Admin\""));
    }

    #[test]
    fn adt_enum_checks_tag_then_fields() {
        let src = r#"
            enum E { A { n: Int }, B }
            service S { rpc get() -> E { db.thing.get() } }
        "#;
        let out = emit(src);
        assert!(out.contains("function isE(x: unknown): x is E"));
        assert!(out.contains("\"A\""));
        assert!(out.contains("\"B\""));
    }

    #[test]
    fn result_gets_its_own_validator_without_a_declared_enum() {
        let src = r#"
            enum Err1 { Bad }
            service S { rpc get() -> Result<Int, Err1> { db.thing.get() } }
        "#;
        let out = emit(src);
        assert!(out.contains("function isResult_Int_Err1"));
        assert!(out.contains("\"Ok\""));
        assert!(out.contains("\"Err\""));
    }

    #[test]
    fn patch_validator_makes_every_field_optional_even_required_ones() {
        let src = r#"
            type User = { id: Int, name: String }
            service S { rpc update(p: Patch<User>) -> User { db.thing.get() } }
        "#;
        let out = emit(src);
        assert!(out.contains("function isPatch_User"));
        // 'id' es requerido en User pero en el Patch tiene que poder faltar
        assert!(out.contains("id === undefined ||") || out.contains(").id === undefined ||"));
    }

    #[test]
    fn map_of_int_keys_validates_by_parsing_not_typeof() {
        let src = r#"
            type Config = { counts: Map<Int, String> }
            service S { rpc get() -> Config { db.thing.get() } }
        "#;
        let out = emit(src);
        assert!(out.contains("Number.isInteger(Number(k))"));
    }

    #[test]
    fn generic_struct_instantiation_gets_a_concrete_validator() {
        let src = r#"
            type Box<T> = { value: T }
            service S { rpc get() -> Box<Int> { db.thing.get() } }
        "#;
        let out = emit(src);
        assert!(out.contains("function isBox_Int"));
    }

    #[test]
    fn nested_named_type_is_discovered_and_gets_its_own_validator() {
        // User referencia Role -- isRole tiene que aparecer aunque Role no
        // sea, por sí mismo, el tipo de retorno de ningún rpc.
        let src = r#"
            enum Role { Admin, Member }
            type User = { role: Role }
            service S { rpc get() -> User { db.thing.get() } }
        "#;
        let out = emit(src);
        assert!(out.contains("function isUser"));
        assert!(out.contains("function isRole"));
    }

    #[test]
    fn no_service_means_no_validators_needed() {
        let src = "type Unused = { x: Int }";
        let out = emit(src);
        assert!(!out.contains("function"));
    }

    // Los siguientes cuatro tests son regresiones de bugs reales que las
    // pruebas de arriba NO atrapaban -- solo se encontraron corriendo el
    // output generado contra un `tsc` de verdad (los tests de este archivo,
    // por diseño, solo comparan substrings del texto Rust generado).

    #[test]
    fn every_generated_function_is_exported() {
        // Sin `export`, un archivo que además no tiene ningún `import`
        // tampoco (users_demo_src no lo dispara, pero cualquier programa
        // con service sí) deja de ser un módulo real para tsc.
        let src = r#"
            type Point = { x: Int }
            service S { rpc get() -> Point { db.thing.get() } }
        "#;
        let out = emit(src);
        assert!(out.contains("export function isPoint"));
    }

    #[test]
    fn validators_file_imports_every_type_name_it_references() {
        // Bug real: las firmas `x is X` usaban X sin importarlo nunca de
        // "./contract" -- tsc fallaba con "Cannot find name 'X'".
        let src = r#"
            enum Role { Admin, Member }
            type User = { role: Role }
            service S { rpc get() -> User { db.thing.get() } }
        "#;
        let out = emit(src);
        let import_line = out.lines().find(|l| l.starts_with("import type")).expect("falta el import");
        assert!(import_line.contains("User"));
        assert!(import_line.contains("Role"));
    }

    #[test]
    fn generic_instantiation_uses_the_real_ts_type_not_the_function_name() {
        // Bug real: `x is {fn_name}` usaba el nombre de la PROPIA función
        // (ej. "isBox_Int") como si fuera un tipo -- ni siquiera parsea
        // como anotación de tipo TS válida.
        let src = r#"
            type Box<T> = { value: T }
            service S { rpc get() -> Box<Int> { db.thing.get() } }
        "#;
        let out = emit(src);
        assert!(out.contains("x is Box<number>"), "se esperaba 'x is Box<number>': {out}");
        assert!(!out.contains("x is isBox_Int"));
    }

    #[test]
    fn list_and_map_callback_parameters_have_explicit_type_annotations() {
        // Bug real: `.every((item) => ...)` sobre una expresión bajo `as
        // any` no siempre re-angosta en tsc bajo `strict` -- "Parameter
        // 'item' implicitly has an 'any' type". Anotar explícito lo evita
        // sin depender de que tsc angoste bien una expresión repetida.
        let src = r#"
            type Config = { tags: Int[], counts: Map<Int, String> }
            service S { rpc get() -> Config { db.thing.get() } }
        "#;
        let out = emit(src);
        assert!(out.contains("(item: unknown)"));
        assert!(out.contains("[k, v]: [string, unknown]"));
    }
}
