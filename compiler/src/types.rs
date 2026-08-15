// Representación de tipos RESUELTOS — distinta del `TypeExpr` sintáctico de
// ast.rs. Un TypeExpr todavía tiene nombres sin resolver (`Named("User",[])`);
// un Type ya sabe si "User" es un struct, qué campos tiene, etc.

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    Int,
    /// Mismo rango que `Int` (`i64`) -- NO es un bignum de precisión
    /// arbitraria. Existe aparte de `Int` únicamente por el borde: `Int` se
    /// emite como `number` de TS (pierde precisión en silencio arriba de
    /// 2^53), `Int64` se emite como `string` en el wire y en TS para
    /// preservarla (GRAMMAR.md §3.30). Sin mezcla implícita con `Int` en
    /// aritmética -- `.toInt64()`/`.toInt()` son la única conversión.
    Int64,
    /// Milisegundos desde epoch UTC internamente; string ISO-8601
    /// (`YYYY-MM-DDTHH:mm:ss.sssZ`) en el wire y en TS (GRAMMAR.md §3.31).
    /// Solo comparable (`< <= > >= == !=`) -- sin aritmética (no hay tipo
    /// `Duration`), sin scrutinee de `match` (mismo criterio que `Float`),
    /// sin construcción desde código fuente en v0 (sin `now()`, que no
    /// tiene dónde engancharse: el lenguaje no tiene función builtin sin
    /// receptor todavía) -- solo llega como parámetro de un `rpc` o ya
    /// guardado en `db`.
    Timestamp,
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
    /// `A | B` (GRAMMAR.md §2.2). Alcance deliberado de v0: sirve para
    /// ACEPTAR/PASAR un valor de cualquiera de los miembros (subtipado más
    /// abajo), pero angostar (recuperar el miembro concreto después) NO
    /// está implementado -- el lenguaje no tiene ningún operador de test de
    /// tipo (`is`/`typeof`) ni forma de hacer `match` sobre una unión cruda,
    /// así que no hay ninguna construcción sintáctica que "angostar"
    /// pudiera usar todavía. Documentado como límite real, no escondido.
    Union(Vec<Type>),
    /// Pseudo-tipo para valores del runtime aún no modelado. Compatible con
    /// cualquier tipo en ambas direcciones, como `any` de TS -- reservado
    /// para lo que de verdad no tiene forma conocida todavía (ya no `db`,
    /// ver Type::Db/Type::DbCollection: "DB tipada" v0 resuelto).
    Dynamic,
    /// Tipo interno SOLO del identificador `db` en sí (GRAMMAR.md §2.1,
    /// DbDecl) -- nunca aparece en una firma de tipo escrita por el
    /// usuario, ni cruza a TypeExpr. `db.<coleccion>` lo resuelve a
    /// `DbCollection`.
    Db,
    /// `db.<coleccion>` ya resuelto contra lo declarado en `db { ... }` --
    /// el elemento es el tipo de struct de esa colección (siempre con un
    /// campo `id: Int`, exigido por el checker al procesar `Item::Db`).
    /// Los métodos builtin (`all/find/insert/applyPatch`) se resuelven
    /// contra esto en vez de caer a `Dynamic` -- por eso un nombre de
    /// colección o de método desconocido ya es un error de tipos, no algo
    /// que se descubre recién en runtime.
    DbCollection(Box<Type>),
    /// Tipo interno SOLO del identificador `auth` (GRAMMAR.md §3.14, auth v0)
    /// -- mismo trato que `Type::Db`: nunca aparece en una firma escrita por
    /// el usuario, ni cruza a `TypeExpr`, así que `check_wire_safe` no
    /// necesita ningún caso explícito para excluirlo (inalcanzable desde una
    /// firma de rpc por construcción, igual que `Db`).
    Auth,
    /// Tipo interno para un Service por su nombre (permite invocar `Service.rpc`)
    Service(String),
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
        // Regla estándar de subtipado de funciones (contravariante en los
        // parámetros, covariante en el retorno): S <: T si T acepta todo lo
        // que S acepta (T_param <: S_param, por eso p2/p1 al revés abajo) y
        // S devuelve algo tan específico como T promete (S_ret <: T_ret).
        // Sin esta regla, dos tipos función solo eran "iguales" por la
        // igualdad estructural de arriba (`sub == sup`) -- exacta, sin
        // aprovechar el subtipado de structs/optional/unión ya definido
        // para params y retorno.
        (Function(p1, r1), Function(p2, r2)) => params_accept(p2, p1) && is_subtype(r1, r2),
        (
            Struct {
                fields: sub_fields, ..
            },
            Struct {
                fields: sup_fields, ..
            },
        ) => sup_fields.iter().all(|sup_f| {
            match sub_fields.iter().find(|sub_f| sub_f.name == sup_f.name) {
                // Width/depth (GRAMMAR.md §3.2): si el supertipo exige el
                // campo (optional=false), el subtipo también debe exigirlo.
                Some(sub_f) => (sup_f.optional || !sub_f.optional) && is_subtype(&sub_f.ty, &sup_f.ty),
                // El subtipo NO declara el campo. Solo está bien si el
                // supertipo lo declaraba opcional (`y?: T`): esa clave puede
                // estar ausente, así que "no tenerla" es una forma válida de
                // cumplirlo. Antes de la auditoría esto era `is_some_and`,
                // que devolvía false acá y rechazaba `{x}` como subtipo de
                // `{x, y?}` -- un falso rechazo de código perfectamente
                // válido (y que TypeScript acepta).
                None => sup_f.optional,
            }
        }),
        // Unión a la IZQUIERDA primero (más específico): una unión es
        // subtipo de T si TODOS sus miembros lo son -- así, si `sup`
        // también es una unión, cada miembro recursa a la regla de abajo.
        (Union(members), sup) => members.iter().all(|m| is_subtype(m, sup)),
        // Unión a la derecha: S es subtipo de una unión si encaja en
        // AL MENOS UN miembro -- así fluye un valor concreto hacia un
        // parámetro/campo tipado como unión.
        (sub, Union(members)) => members.iter().any(|m| is_subtype(sub, m)),
        _ => false,
    }
}

/// ¿Una función con params `candidate_params` puede usarse donde se
/// requieren `expected_params`? Contravariante: cada tipo ESPERADO tiene
/// que ser subtipo del CANDIDATO correspondiente en esa posición -- "T
/// acepta todo lo que S acepta" (ver el comentario en `is_subtype`'s rama
/// `Function`, que llama a esto mismo en vez de repetir la lógica).
///
/// Nombrada y expuesta aparte (no solo inline dentro de `is_subtype`)
/// porque checker.rs necesita exactamente esta misma comparación al
/// chequear un closure anotado contra un `Type::Function` esperado
/// (GRAMMAR.md §3.10) -- escribirla una segunda vez ahí, a mano, es
/// exactamente cómo se invierte la dirección por accidente (`is_subtype(anotación,
/// esperado)` en vez de `is_subtype(esperado, anotación)`): un review de
/// diseño encontró ese error concreto antes de que se escribiera código,
/// con un contraejemplo real (un closure que acepta un struct más ANCHO
/// que el que el llamador realmente tiene, aceptado por error, y que
/// después crashea en runtime leyendo un campo que el dato real nunca
/// tuvo). Una sola función con la dirección ya resuelta es la manera de
/// que ese error no se pueda cometer dos veces.
pub(crate) fn params_accept(expected_params: &[Type], candidate_params: &[Type]) -> bool {
    expected_params.len() == candidate_params.len()
        && expected_params.iter().zip(candidate_params).all(|(e, c)| is_subtype(e, c))
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

        // Y no declararlo EN ABSOLUTO también sirve donde se pide opcional:
        // `y?: T` significa que la clave puede faltar, así que un struct que
        // ni la menciona lo cumple. (Falso rechazo hallado en la auditoría:
        // el `is_some_and` anterior devolvía false cuando el campo no
        // existía, sin mirar si el supertipo lo pedía opcional.)
        let absent = Type::Struct { name: None, fields: vec![] };
        assert!(is_subtype(&absent, &optional));
        assert!(!is_subtype(&absent, &required));
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
    fn function_subtyping_is_contravariant_in_params_covariant_in_return() {
        let narrow_point = Type::Struct {
            name: None,
            fields: vec![FieldType { name: "x".into(), optional: false, ty: Type::Int }],
        };
        let wide_point = Type::Struct {
            name: None,
            fields: vec![
                FieldType { name: "x".into(), optional: false, ty: Type::Int },
                FieldType { name: "y".into(), optional: false, ty: Type::Int },
            ],
        };
        // Retorno COVARIANTE: una fn que devuelve el struct más ancho (con
        // más campos) sirve donde se espera una que devuelve el angosto --
        // igual que width subtyping ya hacía para structs sueltos.
        let returns_wide = Type::Function(vec![], Box::new(wide_point.clone()));
        let returns_narrow = Type::Function(vec![], Box::new(narrow_point.clone()));
        assert!(is_subtype(&returns_wide, &returns_narrow));
        assert!(!is_subtype(&returns_narrow, &returns_wide));

        // Parámetro CONTRAVARIANTE: una fn que acepta el struct angosto (pide
        // menos) sirve donde se espera una que acepta el ancho -- puede
        // recibir cualquier cosa que tenga al menos 'x', que es lo único
        // que necesita, así que también funciona si le llega el ancho.
        let accepts_narrow = Type::Function(vec![narrow_point.clone()], Box::new(Type::Void));
        let accepts_wide = Type::Function(vec![wide_point.clone()], Box::new(Type::Void));
        assert!(is_subtype(&accepts_narrow, &accepts_wide));
        assert!(!is_subtype(&accepts_wide, &accepts_narrow));
    }

    #[test]
    fn concrete_member_flows_into_union_typed_slot() {
        let u = Type::Union(vec![Type::Int, Type::String]);
        assert!(is_subtype(&Type::Int, &u));
        assert!(is_subtype(&Type::String, &u));
        assert!(!is_subtype(&Type::Bool, &u));
    }

    #[test]
    fn union_is_subtype_of_t_only_if_every_member_is() {
        let u = Type::Union(vec![Type::Int, Type::String]);
        // Ningún tipo concreto simple contiene a Int Y a String a la vez.
        assert!(!is_subtype(&u, &Type::Int));
        // Pero una unión más ancha que cubre ambos miembros sí.
        let wider = Type::Union(vec![Type::Int, Type::String, Type::Bool]);
        assert!(is_subtype(&u, &wider));
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
