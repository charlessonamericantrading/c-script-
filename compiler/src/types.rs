// Representación de tipos RESUELTOS — distinta del `TypeExpr` sintáctico de
// ast.rs. Un TypeExpr todavía tiene nombres sin resolver (`Named("User",[])`);
// un Type ya sabe si "User" es un struct, qué campos tiene, etc.

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    Int,
    Float,
    String,
    Bool,
    Void,
    /// El tipo del literal `null`. Solo es subtipo de `Optional(_)` — ver
    /// `is_subtype` — no de un `T` concreto (GRAMMAR.md §3.4).
    Null,
    Optional(Box<Type>),
    List(Box<Type>),
    Tuple(Vec<Type>),
    Function(Vec<Type>, Box<Type>),
    /// `name` es solo para mensajes de error — la igualdad/subtipado de
    /// structs es ESTRUCTURAL (GRAMMAR.md §3.2), nunca compara `name`.
    Struct {
        name: Option<String>,
        fields: Vec<FieldType>,
    },
    /// Nominal (GRAMMAR.md §3.2): dos enums con las mismas variantes pero
    /// nombre distinto NO son intercambiables — por eso acá alcanza con el
    /// nombre, la igualdad de Type ya compara por variante de enum de Rust.
    Enum(String),
    /// `Result<T, E>` builtin (GRAMMAR.md §3.5) — no es un enum que el
    /// usuario declara; sus dos variantes fijas (Ok{value:T}, Err{error:E})
    /// están hardcodeadas en checker.rs. Ver la nota de alcance ahí mismo.
    ResultOf(Box<Type>, Box<Type>),
    /// `Patch<T>` builtin (GRAMMAR.md §3.4) — T debe resolver a un Struct.
    /// Se emite como el `Partial<T>` de TS (ver codegen/ts_emit.rs), que ya
    /// tiene exactamente la semántica que se documentó ahí.
    PatchOf(Box<Type>),
    /// `Map<K, V>` builtin (GRAMMAR.md §2.2) -- la forma de reemplazar el
    /// `{K: V}` literal que se dejó deferido por ambigüedad real con structs
    /// de un campo. `K` limitado a `String`/`Int` (claves JSON), igual que
    /// `{K: V}` en la tabla de mapeo (§4). Se emite como `Record<K, V>`.
    MapOf(Box<Type>, Box<Type>),
    /// Instanciación de un `type`/`enum` genérico DECLARADO POR EL USUARIO
    /// (GRAMMAR.md §3.6) -- ej. `Box<Int>`. A diferencia de Result/Patch/Map
    /// (siempre expandidos), este queda "opaco" (nombre base + args ya
    /// resueltos) hasta que hace falta la forma real (field access,
    /// construcción, match) -- ver `Checker::expand_generic` en checker.rs.
    /// Por monomorfización (PLAN.md §3.6): cada instanciación distinta
    /// (`Box<Int>` vs `Box<String>`) es un tipo concreto propio.
    Generic(String, Vec<Type>),
    /// Parámetro de tipo SIN instanciar -- solo aparece al emitir la
    /// declaración ABSTRACTA de un genérico al `.d.ts` (`interface Box<T>`),
    /// nunca durante el chequeo de un programa real (ahí siempre hay un
    /// `Generic` con args concretos). Ver `resolve_type_abstract`.
    TypeParam(String),
    /// Pseudo-tipo para valores del runtime aún no modelado (p. ej. `db`).
    /// Compatible con cualquier tipo en ambas direcciones, como `any` de TS —
    /// deliberado para v0: el checker todavía no conoce la forma de la base
    /// de datos (eso es Fase 2 "DB tipada" en PLAN.md §4), así que cualquier
    /// cadena `db.algo.mas(...)` queda sin verificar en vez de fallar.
    Dynamic,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldType {
    pub name: String,
    pub optional: bool,
    pub ty: Type,
}

/// S <: T — subtipado estructural para structs, nominal para enums
/// (GRAMMAR.md §3.2), con `Optional-Widen` (§3.4) y `Dynamic` como comodín.
pub fn is_subtype(sub: &Type, sup: &Type) -> bool {
    use Type::*;
    if sub == sup {
        return true;
    }
    match (sub, sup) {
        (Dynamic, _) | (_, Dynamic) => true,
        (Null, Optional(_)) => true,
        (a, Optional(b)) => is_subtype(a, b), // Optional-Widen: S <: T => S <: T?
        (List(a), List(b)) => is_subtype(a, b),
        (Tuple(a), Tuple(b)) if a.len() == b.len() => {
            a.iter().zip(b).all(|(x, y)| is_subtype(x, y))
        }
        (ResultOf(a1, b1), ResultOf(a2, b2)) => is_subtype(a1, a2) && is_subtype(b1, b2),
        (PatchOf(a), PatchOf(b)) => is_subtype(a, b),
        (MapOf(k1, v1), MapOf(k2, v2)) => is_subtype(k1, k2) && is_subtype(v1, v2),
        (
            Struct {
                fields: sub_fields, ..
            },
            Struct {
                fields: sup_fields, ..
            },
        ) => sup_fields.iter().all(|sup_f| {
            sub_fields
                .iter()
                .find(|sub_f| sub_f.name == sup_f.name)
                .is_some_and(|sub_f| {
                    // Width/depth (GRAMMAR.md §3.2): si el supertipo exige el
                    // campo (optional=false), el subtipo también debe exigirlo.
                    (sup_f.optional || !sub_f.optional) && is_subtype(&sub_f.ty, &sup_f.ty)
                })
        }),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(name: Option<&str>) -> Type {
        Type::Struct {
            name: name.map(String::from),
            fields: vec![
                FieldType { name: "x".into(), optional: false, ty: Type::Int },
                FieldType { name: "y".into(), optional: false, ty: Type::Int },
            ],
        }
    }

    #[test]
    fn structural_subtyping_ignores_the_name() {
        // Dos structs con nombres distintos pero la misma forma son
        // intercambiables — es la esencia de GRAMMAR.md §3.2.
        assert!(is_subtype(&point(Some("A")), &point(Some("B"))));
        assert!(is_subtype(&point(None), &point(Some("Point"))));
    }

    #[test]
    fn width_subtyping_extra_fields_ok_missing_required_fails() {
        let wide = Type::Struct {
            name: None,
            fields: vec![
                FieldType { name: "x".into(), optional: false, ty: Type::Int },
                FieldType { name: "y".into(), optional: false, ty: Type::Int },
                FieldType { name: "z".into(), optional: false, ty: Type::Int },
            ],
        };
        let narrow = point(None);
        assert!(is_subtype(&wide, &narrow)); // wide tiene de más, sirve donde se pide narrow
        assert!(!is_subtype(&narrow, &wide)); // narrow no alcanza donde se exige 'z'
    }

    #[test]
    fn optional_field_widening() {
        let required = Type::Struct {
            name: None,
            fields: vec![FieldType { name: "bio".into(), optional: false, ty: Type::String }],
        };
        let optional = Type::Struct {
            name: None,
            fields: vec![FieldType { name: "bio".into(), optional: true, ty: Type::String }],
        };
        // Un campo requerido sirve donde se pide opcional...
        assert!(is_subtype(&required, &optional));
        // ...pero no al revés: falta la garantía de que siempre esté.
        assert!(!is_subtype(&optional, &required));
    }

    #[test]
    fn map_of_subtyping_is_covariant_in_key_and_value() {
        let narrow = Type::MapOf(Box::new(Type::String), Box::new(Type::Int));
        let same = Type::MapOf(Box::new(Type::String), Box::new(Type::Int));
        assert!(is_subtype(&narrow, &same));
        let different_value = Type::MapOf(Box::new(Type::String), Box::new(Type::String));
        assert!(!is_subtype(&narrow, &different_value));
    }

    #[test]
    fn dynamic_is_compatible_with_anything() {
        assert!(is_subtype(&Type::Dynamic, &Type::Int));
        assert!(is_subtype(&Type::Int, &Type::Dynamic));
        assert!(is_subtype(&Type::Dynamic, &point(None)));
    }

    #[test]
    fn null_is_only_subtype_of_optional() {
        assert!(is_subtype(&Type::Null, &Type::Optional(Box::new(Type::Int))));
        assert!(!is_subtype(&Type::Null, &Type::Int));
    }

    #[test]
    fn enum_subtyping_is_nominal() {
        assert!(is_subtype(&Type::Enum("Role".into()), &Type::Enum("Role".into())));
        assert!(!is_subtype(&Type::Enum("Role".into()), &Type::Enum("Status".into())));
    }
}
