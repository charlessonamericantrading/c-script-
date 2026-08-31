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
// Predicados TS a mano (`function isX(x): x is X`), no Zod/typia -- este
// emisor en particular no necesitó ninguna dependencia nueva del lado TS
// (la persistencia real de `db`, GRAMMAR.md §3.17, sí trajo una del lado
// Rust -- `rusqlite` -- a propósito; "cero dependencias" nunca fue una
// regla de todo el proyecto, solo lo que cada feature terminó necesitando).

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
    // Segundo worklist, mismas semillas: no toda firma necesita un
    // "revividor" (GRAMMAR.md §3.156, Int64 -> bigint) -- solo las que
    // contienen un Int64 en algún lado, así que `enqueue_reviver` filtra
    // antes de encolar (`contains_int64`), a diferencia de `enqueue` que
    // encola todo tipo con nombre sin condición.
    let mut reviver_worklist: Vec<Type> = Vec::new();
    let mut reviver_seen: Vec<Type> = Vec::new();

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
                    enqueue_reviver(&ty, &checker, &mut reviver_worklist, &mut reviver_seen)?;
                }
                let ret_ty = checker.resolve_type(&rpc.return_type).map_err(|e| e.to_string())?;
                enqueue(&ret_ty, &mut worklist, &mut seen);
                enqueue_reviver(&ret_ty, &checker, &mut reviver_worklist, &mut reviver_seen)?;
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
    while let Some(ty) = reviver_worklist.pop() {
        emit_reviver(&mut body, &ty, &checker, &mut reviver_worklist, &mut reviver_seen)?;
    }

    let mut out = String::new();
    out.push_str(&format!("// Generado automáticamente por linkc v{} — no editar a mano.\n\n", crate::VERSION));

    // Cada firma `x is X` que se emitió referencia el nombre de tipo X --
    // hay que importarlo de "./contract", igual que ya hace client.ts
    // (reusa collect_type_names, ts_emit.rs, para no duplicar esa lógica).
    // Sin esto, además, un archivo sin ningún import/export no es un módulo
    // TS real -- tsc lo trataría como script ambiental.
    let mut imported_names = BTreeSet::new();
    for ty in seen.iter().chain(reviver_seen.iter()) {
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
        Type::Int64 => "Int64".into(),
        Type::Decimal => "Decimal".into(),
        Type::Timestamp => "Timestamp".into(),
        Type::Float => "Float".into(),
        Type::String => "String".into(),
        Type::Uuid => "Uuid".into(),
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
        Type::Db | Type::DbCollection(_) | Type::Auth | Type::Service(_) | Type::Math | Type::Crypto | Type::Http | Type::Json | Type::Base64 | Type::Pdf | Type::Excel | Type::Env | Type::Request | Type::Smtp | Type::Response => {
            unreachable!("Type::Db/DbCollection/Auth/Service/Math/Crypto/Http/Json/Base64/Pdf/Excel nunca aparece en un TypeExpr real")
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
        // El wire sigue mandando un string (GRAMMAR.md §3.30), pero para
        // cuando este chequeo corre en `client.ts` el valor YA pasó por el
        // revividor de tipo (GRAMMAR.md §3.156, `render_revive` arriba) --
        // acá se valida el `bigint` real, con el mismo rango de un i64 que
        // antes se validaba sobre el string.
        Type::Int64 => format!(
            "(typeof {expr} === \"bigint\" && {expr} >= -9223372036854775808n && {expr} <= 9223372036854775807n)"
        ),
        // Regex fija a la forma exacta que el servidor produce/acepta
        // (GRAMMAR.md §3.31: sin offset de timezone, milisegundos siempre
        // presentes) + Date.parse como red de seguridad barata contra una
        // fecha de calendario inválida (`"2026-13-40..."`) sin duplicar acá
        // la validación de calendario que ya hace parse_iso8601_millis en Rust.
        Type::Timestamp => format!(
            "(typeof {expr} === \"string\" && /^\\d{{4}}-\\d{{2}}-\\d{{2}}T\\d{{2}}:\\d{{2}}:\\d{{2}}\\.\\d{{3}}Z$/.test({expr}) && !isNaN(Date.parse({expr})))"
        ),
        Type::Float => format!("typeof {expr} === \"number\""),
        // Forma fija (GRAMMAR.md §3.184): signo opcional, uno o más
        // dígitos, punto, EXACTAMENTE 4 decimales -- nunca ceros finales
        // omitidos (el servidor siempre los manda, sin ambigüedad al
        // parsear de vuelta).
        Type::Decimal => format!("(typeof {expr} === \"string\" && /^-?\\d+\\.\\d{{4}}$/.test({expr}))"),
        Type::String => format!("typeof {expr} === \"string\""),
        // Forma canónica 8-4-4-4-12 en hex (GRAMMAR.md §3.70) -- sin
        // restringir el nibble de versión/variante: acepta UUIDs de
        // cualquier RFC 4122 (v1/v4/v7/etc.), rechaza basura con la forma
        // general equivocada. Case-insensitive: mayúsculas son válidas
        // aunque `crypto.uuid()` siempre las emite en minúscula.
        Type::Uuid => format!(
            "(typeof {expr} === \"string\" && /^[0-9a-fA-F]{{8}}-[0-9a-fA-F]{{4}}-[0-9a-fA-F]{{4}}-[0-9a-fA-F]{{4}}-[0-9a-fA-F]{{12}}$/.test({expr}))"
        ),
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
            // (GRAMMAR.md §4, K limitado a String/Int) se valida contra la
            // forma textual de un entero, NO con `Number.isInteger(Number(k))`:
            // `Number("")`, `Number(" ")`, `Number("1e3")` y `Number("0x10")`
            // dan enteros perfectamente válidos, así que ese chequeo aceptaba
            // claves que no son enteros escritos como tales.
            let key_check = match k.as_ref() {
                Type::Int => "/^-?\\d+$/.test(k)".to_string(),
                _ => render_check(k, "k", worklist, seen),
            };
            // `!Array.isArray` porque `typeof [] === "object"`: sin eso, un
            // array pasaba como si fuera un Record.
            format!(
                "(typeof {expr} === \"object\" && {expr} !== null && !Array.isArray({expr}) && !Array.isArray({expr}) && Object.entries({expr}).every(([k, v]: [string, unknown]) => {key_check} && {}))",
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
        Type::Db | Type::DbCollection(_) | Type::Auth | Type::Service(_) | Type::Math | Type::Crypto | Type::Http | Type::Json | Type::Base64 | Type::Pdf | Type::Excel | Type::Env | Type::Request | Type::Smtp | Type::Response => {
            unreachable!("Type::Db/DbCollection/Auth/Service/Math/Crypto/Http/Json/Base64/Pdf/Excel nunca aparece en un TypeExpr real")
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
    format!("(typeof {expr} === \"object\" && {expr} !== null && !Array.isArray({expr}) && {body})")
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
    format!("(typeof {expr} === \"object\" && {expr} !== null && !Array.isArray({expr}) && {body})")
}

/// Cuerpo de un enum (simple o con datos) -- `variants` ya trae los nombres
/// (de `EnumDecl` crudo, para el chequeo all-unit igual que `emit_enum_decl`
/// en ts_emit.rs) y `scrutinee_ty` es lo que se le pasa a
/// `Checker::variant_field_types` para resolver los campos de cada variante
/// (maneja Result/enum genérico/enum plano de forma uniforme).
#[allow(clippy::too_many_arguments)]
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
        "(typeof {expr} === \"object\" && {expr} !== null && !Array.isArray({expr}) && ({}))",
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
                "export function {fn_name}(x: unknown): x is Result<{}, {}> {{\n  return (typeof x === \"object\" && x !== null && !Array.isArray(x) && ((((x as any).type === \"Ok\") && {ok_check}) || (((x as any).type === \"Err\") && {err_check})));\n}}\n\n",
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

// ---- Int64 -> bigint: "revividores" tras JSON.parse (GRAMMAR.md §3.156) ----
//
// El wire sigue mandando un Int64 como string (GRAMMAR.md §3.30, sin
// cambios) -- lo nuevo es que `client.ts` ahora declara `bigint` de verdad
// (ts_emit.rs::render_type) y necesita convertir cada string de vuelta a
// bigint DESPUÉS de `res.json()`, dirigido por el tipo real de la
// respuesta -- no hay forma estructural de distinguir "este string es un
// Int64" de "este campo String que por casualidad es numérico" sin esa
// guía. Mismo patrón worklist/seen que los validadores de arriba, pero
// filtrado por `contains_int64`: un tipo sin ningún Int64 adentro no
// genera ningún revividor (cero costo para el caso común).

/// ¿`ty` es, o contiene recursivamente (en un campo, elemento, variante de
/// enum, instanciación de genérico, etc.), un `Type::Int64`? A diferencia
/// de `type_contains_function` (checker.rs), que se detiene en un genérico
/// opaco, ACÁ hay que expandir Generic/Enum de verdad (vía `checker`) --
/// dejar alguno sin expandir sería un Int64 escondido que `contract.d.ts`
/// promete como `bigint` pero que en runtime nunca se revive (mentira de
/// tipos silenciosa, peor que no tener la feature).
fn contains_int64(ty: &Type, checker: &Checker) -> Result<bool, String> {
    Ok(match ty {
        Type::Int64 => true,
        Type::Optional(inner) | Type::List(inner) | Type::PatchOf(inner) => contains_int64(inner, checker)?,
        Type::Tuple(items) | Type::Union(items) => {
            let mut any = false;
            for i in items {
                if contains_int64(i, checker)? {
                    any = true;
                    break;
                }
            }
            any
        }
        Type::MapOf(k, v) => contains_int64(k, checker)? || contains_int64(v, checker)?,
        Type::ResultOf(a, b) => contains_int64(a, checker)? || contains_int64(b, checker)?,
        Type::Struct { fields, .. } => {
            let mut any = false;
            for f in fields {
                if contains_int64(&f.ty, checker)? {
                    any = true;
                    break;
                }
            }
            any
        }
        Type::Enum(name) => enum_contains_int64(checker, ty, name)?,
        Type::Generic(name, args) => {
            if checker.types.contains_key(name) {
                let fields = checker.expand_generic_struct(name, args).map_err(|e| e.to_string())?;
                let mut any = false;
                for f in &fields {
                    if contains_int64(&f.ty, checker)? {
                        any = true;
                        break;
                    }
                }
                any
            } else if checker.enums.contains_key(name) {
                enum_contains_int64(checker, ty, name)?
            } else {
                false
            }
        }
        _ => false,
    })
}

fn enum_contains_int64(checker: &Checker, scrutinee_ty: &Type, enum_name: &str) -> Result<bool, String> {
    let decl = checker.enums.get(enum_name).ok_or_else(|| format!("enum desconocido: '{enum_name}'"))?;
    for v in &decl.variants {
        for (_, fty) in checker.variant_field_types(scrutinee_ty, enum_name, &v.name).map_err(|e| e.to_string())? {
            if contains_int64(&fty, checker)? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Nombre del revividor para un tipo con identidad propia -- mismo criterio
/// de "con nombre vs. estructural" que `validator_fn_name`, con el mismo
/// prefijo `revive` en vez de `is`.
fn reviver_fn_name(ty: &Type) -> Option<String> {
    match ty {
        Type::Struct { name: Some(n), .. } => Some(format!("revive{n}")),
        Type::Enum(n) => Some(format!("revive{n}")),
        Type::ResultOf(a, b) => Some(format!("reviveResult_{}_{}", type_key(a), type_key(b))),
        Type::PatchOf(inner) => Some(format!("revivePatch_{}", type_key(inner))),
        Type::Generic(name, args) => Some(format!("revive{name}_{}", args.iter().map(type_key).collect::<Vec<_>>().join("_"))),
        _ => None,
    }
}

/// Encola `ty` para tener su propio revividor SOLO si contiene algún
/// Int64 -- a diferencia de `enqueue` (validadores), que encola todo tipo
/// con nombre sin condición.
fn enqueue_reviver(ty: &Type, checker: &Checker, worklist: &mut Vec<Type>, seen: &mut Vec<Type>) -> Result<(), String> {
    if !contains_int64(ty, checker)? {
        return Ok(());
    }
    if reviver_fn_name(ty).is_some() {
        if !seen.contains(ty) {
            seen.push(ty.clone());
            worklist.push(ty.clone());
        }
        return Ok(());
    }
    match ty {
        Type::Optional(inner) | Type::List(inner) => enqueue_reviver(inner, checker, worklist, seen)?,
        Type::Tuple(items) | Type::Union(items) => {
            for i in items {
                enqueue_reviver(i, checker, worklist, seen)?;
            }
        }
        Type::Struct { fields, .. } => {
            for f in fields {
                enqueue_reviver(&f.ty, checker, worklist, seen)?;
            }
        }
        Type::MapOf(k, v) => {
            enqueue_reviver(k, checker, worklist, seen)?;
            enqueue_reviver(v, checker, worklist, seen)?;
        }
        _ => {}
    }
    Ok(())
}

/// Expresión TS que revive `expr` (ya parseado de JSON) contra `ty`,
/// devolviendo `None` si `ty` no contiene ningún Int64 -- nada que revivir,
/// el llamador reusa `expr` tal cual (identidad, sin costo en runtime).
fn render_revive(
    ty: &Type,
    expr: &str,
    checker: &Checker,
    worklist: &mut Vec<Type>,
    seen: &mut Vec<Type>,
) -> Result<Option<String>, String> {
    if !contains_int64(ty, checker)? {
        return Ok(None);
    }
    if let Some(fn_name) = reviver_fn_name(ty) {
        enqueue_reviver(ty, checker, worklist, seen)?;
        return Ok(Some(format!("{fn_name}({expr})")));
    }
    Ok(Some(match ty {
        // `as any`: `expr` puede ser el `json`/`revived` de tope (tipado
        // `unknown`, `BigInt(unknown)` no tipa bajo `strict`) o un acceso de
        // campo ya casteado (`(x as any).campo`, donde el cast es
        // redundante pero inofensivo).
        Type::Int64 => format!("BigInt({expr} as any)"),
        Type::Optional(inner) => {
            let inner_expr =
                render_revive(inner, expr, checker, worklist, seen)?.expect("contains_int64 ya confirmó Int64 adentro");
            format!("({expr} === null ? null : {inner_expr})")
        }
        Type::List(inner) => {
            let inner_expr =
                render_revive(inner, "item", checker, worklist, seen)?.expect("contains_int64 ya confirmó Int64 adentro");
            format!("(({expr} as any[]).map((item: any) => {inner_expr}))")
        }
        Type::Tuple(items) => {
            let mut parts = Vec::new();
            for (i, t) in items.iter().enumerate() {
                let e = format!("({expr} as any)[{i}]");
                parts.push(render_revive(t, &e, checker, worklist, seen)?.unwrap_or(e));
            }
            format!("[{}]", parts.join(", "))
        }
        Type::MapOf(_, v) => {
            let inner_expr = render_revive(v, "v", checker, worklist, seen)?.expect("contains_int64 ya confirmó Int64 adentro");
            format!(
                "Object.fromEntries(Object.entries({expr} as any).map(([k, v]: [string, any]) => [k, {inner_expr}]))"
            )
        }
        // Bug real, encontrado por una auditoría multi-agente adversarial
        // (26/08/2026): la forma original disambiguaba corriendo el `isX`
        // de cada miembro contra `expr` SIN REVIVIR -- pero `isX` para
        // `Int64` (`render_check` arriba) ya asume la forma POST-revivida
        // (`typeof === "bigint"`), nunca la del wire crudo (siempre
        // string). Resultado: un miembro `Int64` de una unión NUNCA
        // matcheaba contra el valor crudo, la disambiguación caía siempre
        // al siguiente miembro (típicamente `String`, que sí matchea
        // cualquier string), y el `Int64` real quedaba como string para
        // siempre -- silencioso, porque `isX` sobre la unión completa
        // también pasaba (el string sin revivir sigue siendo un `String`
        // válido para ese chequeo). Fix: revivir CADA candidato primero
        // (con su propio revividor, identidad si ese miembro no tiene
        // Int64 adentro), y recién ahí correr `isX` sobre el candidato YA
        // revivido -- el orden real tiene que ser revivir-después-validar,
        // igual que en cualquier otra posición de este archivo.
        Type::Union(members) => {
            let mut body = String::new();
            for (i, m) in members.iter().enumerate() {
                let var = format!("__u{i}");
                let candidate = render_revive(m, expr, checker, worklist, seen)?.unwrap_or_else(|| format!("({expr} as any)"));
                let check = render_check(m, &var, worklist, seen);
                // `try/catch`: revivir un candidato que NO es de verdad este
                // miembro puede lanzar de verdad (`BigInt("hello")` tira
                // `SyntaxError`, no devuelve algo que falle el chequeo
                // después) -- un candidato que revienta simplemente no es
                // este miembro, se sigue probando el próximo.
                body.push_str(&format!("try {{ const {var} = {candidate}; if ({check}) return {var}; }} catch {{}}\n"));
            }
            body.push_str(&format!("return ({expr} as any);\n"));
            format!("((): any => {{\n{body}}})()")
        }
        Type::Struct { fields, .. } => render_struct_revive(fields, expr, checker, worklist, seen)?,
        _ => unreachable!("cubierto por reviver_fn_name o contains_int64 = false"),
    }))
}

fn render_struct_revive(
    fields: &[FieldType],
    expr: &str,
    checker: &Checker,
    worklist: &mut Vec<Type>,
    seen: &mut Vec<Type>,
) -> Result<String, String> {
    let mut assignments = Vec::new();
    for f in fields {
        let field_expr = format!("({expr} as any).{}", f.name);
        if let Some(revived) = render_revive(&f.ty, &field_expr, checker, worklist, seen)? {
            if f.optional {
                assignments.push(format!("{}: {field_expr} === undefined ? undefined : ({revived})", f.name));
            } else {
                assignments.push(format!("{}: {revived}", f.name));
            }
        }
    }
    Ok(format!("{{ ...({expr} as any), {} }}", assignments.join(", ")))
}

/// Igual que `render_struct_revive`, pero tratando TODOS los campos como
/// opcionales -- misma contraparte que `render_patch_fields_check` para
/// `Patch<T>` (omitido = no tocar, no `undefined` a propósito).
fn render_patch_revive(
    fields: &[FieldType],
    expr: &str,
    checker: &Checker,
    worklist: &mut Vec<Type>,
    seen: &mut Vec<Type>,
) -> Result<String, String> {
    let mut assignments = Vec::new();
    for f in fields {
        let field_expr = format!("({expr} as any).{}", f.name);
        if let Some(revived) = render_revive(&f.ty, &field_expr, checker, worklist, seen)? {
            assignments.push(format!("{}: {field_expr} === undefined ? undefined : ({revived})", f.name));
        }
    }
    Ok(format!("{{ ...({expr} as any), {} }}", assignments.join(", ")))
}

fn render_enum_revive_body(
    checker: &Checker,
    scrutinee_ty: &Type,
    enum_name: &str,
    variant_names: &[String],
    expr: &str,
    worklist: &mut Vec<Type>,
    seen: &mut Vec<Type>,
) -> Result<String, String> {
    let mut lines = Vec::new();
    for v in variant_names {
        let fields = checker
            .variant_field_types(scrutinee_ty, enum_name, v)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|(name, ty)| FieldType { name, optional: false, ty })
            .collect::<Vec<_>>();
        let body = render_struct_revive(&fields, expr, checker, worklist, seen)?;
        lines.push(format!("  if (({expr} as any).type === \"{v}\") return {body} as any;"));
    }
    lines.push(format!("  return {expr} as any;"));
    Ok(lines.join("\n"))
}

fn emit_reviver(
    out: &mut String,
    ty: &Type,
    checker: &Checker,
    worklist: &mut Vec<Type>,
    seen: &mut Vec<Type>,
) -> Result<(), String> {
    let fn_name = reviver_fn_name(ty).expect("solo se llama para tipos con reviver_fn_name propio");
    let type_annotation = ts_emit::render_type(ty);
    match ty {
        Type::Struct { name: Some(_), fields } => {
            let body = render_struct_revive(fields, "x", checker, worklist, seen)?;
            out.push_str(&format!("export function {fn_name}(x: any): {type_annotation} {{\n  return {body};\n}}\n\n"));
        }
        Type::Enum(name) => {
            let decl = checker.enums.get(name).ok_or_else(|| format!("enum desconocido: '{name}'"))?;
            let variant_names: Vec<String> = decl.variants.iter().map(|v| v.name.clone()).collect();
            let body = render_enum_revive_body(checker, ty, name, &variant_names, "x", worklist, seen)?;
            out.push_str(&format!("export function {fn_name}(x: any): {type_annotation} {{\n{body}\n}}\n\n"));
        }
        Type::ResultOf(ok_ty, err_ty) => {
            let ok_revived = render_revive(ok_ty, "(x as any).value", checker, worklist, seen)?;
            let err_revived = render_revive(err_ty, "(x as any).error", checker, worklist, seen)?;
            let ok_expr = match ok_revived {
                Some(r) => format!("{{ type: \"Ok\", value: {r} }}"),
                None => "x as any".to_string(),
            };
            let err_expr = match err_revived {
                Some(r) => format!("{{ type: \"Err\", error: {r} }}"),
                None => "x as any".to_string(),
            };
            out.push_str(&format!(
                "export function {fn_name}(x: any): {type_annotation} {{\n  return (x as any).type === \"Ok\" ? {ok_expr} : {err_expr};\n}}\n\n"
            ));
        }
        Type::PatchOf(inner) => {
            let Type::Struct { fields, .. } = inner.as_ref() else {
                return Err(format!("Patch<T> requiere que T resuelva a un struct -- inesperado en el emisor: {inner:?}"));
            };
            let body = render_patch_revive(fields, "x", checker, worklist, seen)?;
            out.push_str(&format!("export function {fn_name}(x: any): {type_annotation} {{\n  return {body};\n}}\n\n"));
        }
        Type::Generic(name, args) => {
            if checker.types.contains_key(name) {
                let fields = checker.expand_generic_struct(name, args).map_err(|e| e.to_string())?;
                let body = render_struct_revive(&fields, "x", checker, worklist, seen)?;
                out.push_str(&format!("export function {fn_name}(x: any): {type_annotation} {{\n  return {body};\n}}\n\n"));
            } else if let Some(decl) = checker.enums.get(name) {
                let variant_names: Vec<String> = decl.variants.iter().map(|v| v.name.clone()).collect();
                let body = render_enum_revive_body(checker, ty, name, &variant_names, "x", worklist, seen)?;
                out.push_str(&format!("export function {fn_name}(x: any): {type_annotation} {{\n{body}\n}}\n\n"));
            } else {
                return Err(format!("tipo genérico desconocido: '{name}'"));
            }
        }
        other => return Err(format!("emit_reviver llamado sobre un tipo sin nombre: {other:?}")),
    }
    Ok(())
}

/// Igual que `render_check_expr`: se apoya en que `emit_validators` ya
/// recorrió las MISMAS semillas de rpc y emitió cualquier revividor con
/// nombre que este cálculo pudiera necesitar -- acá se descarta el
/// worklist/seen locales después de calcular la expresión.
pub(crate) fn render_revive_expr(ty: &Type, expr: &str, checker: &Checker) -> Result<Option<String>, String> {
    let mut worklist = Vec::new();
    let mut seen = Vec::new();
    render_revive(ty, expr, checker, &mut worklist, &mut seen)
}

/// Nombres de revividor que un `client.ts` que revive `ty` necesita
/// importar de "./validators" -- mismo criterio de corte que
/// `collect_validator_names`.
pub(crate) fn collect_reviver_names(ty: &Type, checker: &Checker, names: &mut BTreeSet<String>) -> Result<(), String> {
    if !contains_int64(ty, checker)? {
        return Ok(());
    }
    if let Some(fn_name) = reviver_fn_name(ty) {
        names.insert(fn_name);
        return Ok(());
    }
    match ty {
        Type::Optional(inner) | Type::List(inner) => collect_reviver_names(inner, checker, names)?,
        Type::Tuple(items) | Type::Union(items) => {
            for i in items {
                collect_reviver_names(i, checker, names)?;
            }
        }
        Type::Struct { fields, .. } => {
            for f in fields {
                collect_reviver_names(&f.ty, checker, names)?;
            }
        }
        Type::MapOf(k, v) => {
            collect_reviver_names(k, checker, names)?;
            collect_reviver_names(v, checker, names)?;
        }
        _ => {}
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
        let program = parse(tokens).unwrap_or_else(|e| panic!("{e:?}"));
        emit_validators(&program).unwrap_or_else(|e| panic!("{e}"))
    }

    /// PLAN.md §9.7, GRAMMAR.md §3.83: mismo estampado de versión que
    /// `contract.d.ts`/`client.ts`/`hooks.ts`.
    #[test]
    fn header_is_stamped_with_the_compiler_version() {
        let out = emit("type Point = { x: Int, y: Int }");
        assert!(
            out.starts_with(&format!("// Generado automáticamente por linkc v{} — no editar a mano.", crate::VERSION)),
            "{out}"
        );
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
    fn int64_check_requires_a_real_bigint_within_i64_range() {
        // GRAMMAR.md §3.156: para cuando este chequeo corre en client.ts el
        // valor ya pasó por el revividor de tipo (string del wire ->
        // bigint) -- acá se valida el bigint real, no el string.
        let src = r#"
            type Counter = { big: Int64 }
            service S { rpc get() -> Counter { db.thing.get() } }
        "#;
        let out = emit(src);
        assert!(out.contains("typeof") && out.contains("=== \"bigint\""), "debe exigir bigint: {out}");
        assert!(out.contains("9223372036854775807n"), "debe acotar al rango real de i64: {out}");
        assert!(out.contains("export function reviveCounter"), "debe emitir un revividor: {out}");
        assert!(out.contains("BigInt("), "el revividor debe convertir el string del wire a bigint: {out}");
    }

    #[test]
    fn a_type_with_no_int64_anywhere_gets_no_reviver_at_all() {
        // Cero costo para el caso común (GRAMMAR.md §3.156): un tipo sin
        // ningún Int64 adentro no genera NINGÚN "revive..." -- ni siquiera
        // uno identidad.
        let src = r#"
            type Point = { x: Int, y: Int, label: String }
            service S { rpc get() -> Point { db.thing.get() } }
        "#;
        let out = emit(src);
        assert!(!out.contains("revive"), "no debería emitir ningún revividor: {out}");
    }

    #[test]
    fn int64_inside_a_list_and_an_optional_gets_revived_recursively() {
        let src = r#"
            type Row = { tags: Int64[], maybe: Int64? }
            service S { rpc get() -> Row { db.thing.get() } }
        "#;
        let out = emit(src);
        assert!(out.contains("export function reviveRow"), "{out}");
        assert!(out.contains(".map((item: any) => BigInt(item as any))"), "List<Int64> debe revivir cada elemento: {out}");
        assert!(out.contains("=== null ? null : BigInt"), "Int64? debe preservar null sin llamar BigInt sobre null: {out}");
    }

    #[test]
    fn int64_inside_a_generic_instantiation_gets_revived() {
        // Un genérico opaco (`Box<Int64>`) NO puede tratarse como "no
        // contiene Int64" solo porque está detrás de un nombre -- eso sería
        // una mentira de tipos silenciosa (contract.d.ts diría `bigint`,
        // runtime seguiría entregando un string). `contains_int64` expande
        // el genérico vía el checker para evitar justo eso.
        let src = r#"
            type Box<T> = { value: T }
            service S { rpc get() -> Box<Int64> { db.thing.get() } }
        "#;
        let out = emit(src);
        assert!(out.contains("export function reviveBox_Int64"), "{out}");
        assert!(out.contains("value: BigInt"), "{out}");
    }

    #[test]
    fn int64_inside_an_enum_variant_gets_revived() {
        let src = r#"
            enum Event { Created { amount: Int64 }, Cancelled }
            service S { rpc get() -> Event { db.thing.get() } }
        "#;
        let out = emit(src);
        assert!(out.contains("export function reviveEvent"), "{out}");
        assert!(out.contains("(x as any).type === \"Created\""), "{out}");
        assert!(out.contains("amount: BigInt"), "{out}");
    }

    /// Bug real, encontrado por una auditoría multi-agente adversarial
    /// (26/08/2026): la disambiguación de un `Union` durante el revivido
    /// corría el `isX` de cada miembro contra el valor CRUDO (pre-revivir)
    /// -- pero `isInt64` (desde esta misma ronda) ya asume la forma
    /// POST-revivida (`typeof === "bigint"`), así que un miembro `Int64`
    /// nunca matcheaba y la unión caía siempre al miembro `String`, que sí
    /// matchea cualquier string. El fix revive CADA candidato primero,
    /// envuelto en `try/catch` (`BigInt("no numérico")` tira de verdad, no
    /// falla el chequeo después), y recién ahí valida el candidato ya
    /// revivido.
    #[test]
    fn int64_inside_a_union_member_gets_revived_not_silently_left_as_a_string() {
        let src = r#"
            type Event = { payload: Int64 | String }
            service S { rpc get() -> Event { db.thing.get() } }
        "#;
        let out = emit(src);
        assert!(out.contains("export function reviveEvent"), "{out}");
        // El candidato Int64 tiene que probarse ENVUELTO en try/catch (un
        // string no numérico como candidato Int64 tira de verdad) y
        // validarse YA revivido -- no `render_check` corriendo contra el
        // string crudo.
        assert!(out.contains("try {") && out.contains("catch {}"), "{out}");
        assert!(out.contains("BigInt("), "el candidato Int64 tiene que intentar revivir de verdad: {out}");
    }

    #[test]
    fn timestamp_check_requires_the_exact_iso8601_shape() {
        let src = r#"
            type Event = { at: Timestamp }
            service S { rpc get() -> Event { db.thing.get() } }
        "#;
        let out = emit(src);
        assert!(out.contains("=== \"string\""), "debe exigir string: {out}");
        assert!(out.contains("Date.parse("), "debe validar que sea una fecha real: {out}");
        assert!(out.contains(r"\d{4}-\d{2}-\d{2}"), "debe exigir la forma fija ISO-8601: {out}");
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
    fn map_of_int_keys_validates_the_textual_form_of_an_integer() {
        // Las claves de un objeto JS son siempre string, así que un
        // Map<Int,_> se valida contra la FORMA del string. No sirve
        // `Number.isInteger(Number(k))` (lo que había): `Number("")`,
        // `Number(" ")`, `Number("1e3")` y `Number("0x10")` son todos
        // enteros válidos, así que aceptaba claves que no son enteros
        // escritos como tales.
        let src = r#"
            type Config = { counts: Map<Int, String> }
            service S { rpc get() -> Config { db.thing.get() } }
        "#;
        let out = emit(src);
        assert!(out.contains(r"/^-?\d+$/.test(k)"), "{out}");
        assert!(!out.contains("Number.isInteger(Number(k))"));
    }

    #[test]
    fn object_guards_reject_arrays() {
        // `typeof [] === "object"`, así que sin un !Array.isArray explícito
        // un array pasaba como si fuera un struct o un Record.
        let src = r#"
            type Config = { counts: Map<Int, String> }
            service S { rpc get() -> Config { db.thing.get() } }
        "#;
        let out = emit(src);
        // Una vez para el struct Config, otra para el Map de adentro.
        assert!(out.matches("!Array.isArray(").count() >= 2, "{out}");
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
