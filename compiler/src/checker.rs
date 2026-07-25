// Type checker bidireccional (GRAMMAR.md §3): síntesis (⇒) para lo que se
// puede inferir de abajo hacia arriba, chequeo (⇐) para lo que necesita un
// tipo esperado (match, y la construcción de Result<T,E> — ver más abajo).

use crate::ast::*;
use crate::types::{is_subtype, FieldType, Type};
use std::collections::{HashMap, HashSet};

#[derive(Debug)]
pub struct CheckError {
    pub message: String,
}

impl std::fmt::Display for CheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "error de tipos: {}", self.message)
    }
}

fn err(msg: impl Into<String>) -> CheckError {
    CheckError { message: msg.into() }
}

/// Cada binding rastrea su tipo Y si se declaró `mut` -- lo segundo es lo
/// que `check_block` consulta al validar un `assign_stmt` (GRAMMAR.md §2.3).
#[derive(Clone)]
struct Binding {
    ty: Type,
    mutable: bool,
}

fn immutable(ty: Type) -> Binding {
    Binding { ty, mutable: false }
}

type Env = HashMap<String, Binding>;

/// Todos los nombres que un patrón ligaría, recursivamente -- usado por
/// `bind_pattern` solo para RECHAZAR un `Pattern::Or` cuyas alternativas
/// intenten bindear algo (ver su doc en ast.rs), no para resolver tipos.
fn pattern_bindings(pattern: &Pattern) -> Vec<String> {
    match pattern {
        Pattern::Bind(name) => vec![name.clone()],
        Pattern::Literal(_) => Vec::new(),
        Pattern::Variant { fields, .. } => fields
            .iter()
            .flatten()
            .flat_map(|fp| pattern_bindings(&fp.pattern))
            .collect(),
        Pattern::Or(subs) => subs.iter().flat_map(pattern_bindings).collect(),
        // A diferencia de `Literal` (que no liga nada), `Type` SÍ liga un
        // nombre -- devolver `Vec::new()` acá dejaría pasar en silencio un
        // `i: Int | s: String` dentro de un Or (que debería rechazarse
        // igual que cualquier otro binding dentro de un Or, ver el doc de
        // `Pattern::Or` en ast.rs).
        Pattern::Type(name, _) => vec![name.clone()],
    }
}

/// ¿Hay algún `Stmt::Return` alcanzable desde este bloque? Usado por
/// `synth_block` (GRAMMAR.md §3.10) para rechazar `return` de entrada en
/// vez de heredar el bug preexistente de `check_block` (mismo `expected`
/// para la cola y para un `return` anidado dentro de un if/match en
/// posición de sentencia). `return` es siempre una SENTENCIA (ast.rs) --
/// nunca aparece anidado dentro de una expresión -- así que la única forma
/// de que esté "escondido" respecto de este bloque es a través de un
/// `if`/`match` cuyo cuerpo es, a su vez, otro `Block`; por eso alcanza con
/// recursar exactamente en esos dos casos.
fn block_has_return(block: &Block) -> bool {
    block.stmts.iter().any(|s| match s {
        Stmt::Return(_) => true,
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => expr_has_return(value),
        Stmt::Expr(e) => expr_has_return(e),
    }) || block.tail.as_deref().is_some_and(expr_has_return)
}

fn expr_has_return(e: &Expr) -> bool {
    match e {
        Expr::If { cond, then_block, else_block } => {
            expr_has_return(cond) || block_has_return(then_block) || block_has_return(else_block)
        }
        Expr::Match { scrutinee, arms } => {
            expr_has_return(scrutinee)
                || arms.iter().any(|arm| match &arm.body {
                    MatchArmBody::Expr(e) => expr_has_return(e),
                    MatchArmBody::Block(b) => block_has_return(b),
                })
        }
        // Un closure anidado tiene su PROPIO contexto de retorno,
        // chequeado aparte cuando a él le toque (synth_expr/check_expr
        // sobre ESE Expr::Closure) -- su `return` no es asunto del
        // synth_block que lo contiene.
        Expr::Closure { .. } => false,
        // Todo lo demás no puede contener un `return` sintácticamente --
        // `return` es una sentencia, nunca anidada dentro de una expresión.
        _ => false,
    }
}

/// ¿`ty` es, o contiene recursivamente (en un campo, elemento, miembro de
/// unión, etc.), un `Type::Function`? Usado para rechazar `==`/`!=` sobre
/// closures (`synth_binary`, GRAMMAR.md §3.10). Motivo real, no teórico: un
/// closure recursivo armado reasignando un `mut` (`let mut f = |x|{x}; f =
/// |x|{ ... f(x-1) ... };`) captura, en runtime, un `Env` que termina
/// conteniendo una referencia a sí mismo (`Rc` cíclico) -- comparar o
/// debug-imprimir un valor así recursaría para siempre. Rechazarlo acá, en
/// tiempo de chequeo, es más barato y más claro que confiar solo en que
/// `Value` nunca se compare/imprima por accidente en runtime.
fn type_contains_function(ty: &Type) -> bool {
    match ty {
        Type::Function(..) => true,
        Type::Optional(inner) | Type::List(inner) | Type::PatchOf(inner) | Type::DbCollection(inner) => {
            type_contains_function(inner)
        }
        Type::Tuple(items) | Type::Union(items) => items.iter().any(type_contains_function),
        Type::ResultOf(a, b) | Type::MapOf(a, b) => type_contains_function(a) || type_contains_function(b),
        Type::Struct { fields, .. } => fields.iter().any(|f| type_contains_function(&f.ty)),
        _ => false,
    }
}

/// El tipo que produce LEER un campo (`v.campo`). Para un campo declarado
/// `x?: T` -- clave que puede estar AUSENTE (GRAMMAR.md §3.4) -- eso es
/// `T?`, no `T`: leerlo puede no dar nada, y el lenguaje ya tiene una forma
/// de expresar eso.
///
/// Sin esto (el bug que había hasta la auditoría), `o.note` sobre un
/// `note?: String` sintetizaba `String` a secas -- así que un rpc podía
/// declarar `-> String`, devolver `o.note`, tipar perfecto, y después
/// fallar en runtime con "no existe el campo 'note'" al recibir un objeto
/// SIN esa clave (que es exactamente lo que `x?: T` permite). También hacía
/// pasar aritmética como `o.note + 1`. Nótese el contraste con `x: T?`
/// (clave siempre presente, valor nullable), que ya se rechazaba bien:
/// el bug era solo para la opcionalidad de CLAVE.
fn field_access_ty(f: &FieldType) -> Type {
    if f.optional {
        Type::Optional(Box::new(f.ty.clone()))
    } else {
        f.ty.clone()
    }
}

/// Recorre `ty` rechazando lo que no puede viajar como JSON. `top_level_ret`
/// solo habilita `Void` en la posición donde sí tiene sentido: el retorno
/// entero de un rpc que no devuelve nada.
fn check_wire_safe(ty: &Type, position: &str, top_level_ret: bool) -> Result<(), CheckError> {
    match ty {
        Type::Function(..) => Err(err(format!(
            "{position} incluye un tipo función, que no puede viajar por la red (GRAMMAR.md §4) -- \
             una función solo existe dentro del backend"
        ))),
        Type::Void if !top_level_ret => Err(err(format!(
            "{position} usa 'Void', que solo es válido como el retorno completo de un rpc (GRAMMAR.md §4)"
        ))),
        Type::Void => Ok(()),
        Type::Optional(inner) | Type::List(inner) | Type::PatchOf(inner) => {
            check_wire_safe(inner, position, false)
        }
        Type::Tuple(items) | Type::Union(items) => {
            items.iter().try_for_each(|t| check_wire_safe(t, position, false))
        }
        Type::ResultOf(a, b) | Type::MapOf(a, b) => {
            check_wire_safe(a, position, false)?;
            check_wire_safe(b, position, false)
        }
        Type::Struct { fields, .. } => fields.iter().try_for_each(|f| check_wire_safe(&f.ty, position, false)),
        Type::Generic(_, args) => args.iter().try_for_each(|t| check_wire_safe(t, position, false)),
        _ => Ok(()),
    }
}

/// ¿Se puede PROBAR que dos miembros de una unión son distinguibles en
/// runtime? "No" es la respuesta segura cuando el análisis no puede probar
/// que sí (GRAMMAR.md §3.9, `check_exhaustive_union`) -- falla cerrado, no
/// asume que está bien.
///
/// Un chequeo ingenuo de `is_subtype` mutuo entre los dos NO alcanza:
/// `{x:Int,y:Int}` y `{x:Int,z:Int}` no son subtipo mutuo entre sí, pero un
/// TERCER tipo más ancho (`{x:Int,y:Int,z:Int}`, construible por cualquier
/// usuario vía subtipado estructural de ancho) satisface los campos
/// requeridos de los DOS a la vez -- un valor de ese tercer tipo sería
/// ambiguo para cualquiera de las dos reglas que solo miran nombres de
/// campo. La condición real: existe al menos un campo REQUERIDO por ambos
/// cuyos tipos declarados tengan discriminantes de `Value` (runtime/mod.rs)
/// mutuamente excluyentes -- un valor real solo puede tener UNA forma
/// concreta en ese campo (nunca ambas a la vez), así que ESE campo sí
/// distingue de forma confiable, sin importar qué tan ancho sea el valor
/// real que llegue. `value_matches_type` (runtime/mod.rs) chequea
/// exactamente esto -- el tipo REAL del valor en cada campo requerido, no
/// solo su presencia -- para que este argumento de solidez se sostenga.
fn union_members_are_distinguishable(a: &Type, b: &Type) -> bool {
    match (a, b) {
        (Type::Struct { fields: fa, .. }, Type::Struct { fields: fb, .. }) => fa.iter().any(|field_a| {
            !field_a.optional
                && fb.iter().any(|field_b| {
                    !field_b.optional && field_a.name == field_b.name && shallow_tag_conflict(&field_a.ty, &field_b.ty)
                })
        }),
        _ => shallow_tag_conflict(a, b),
    }
}

/// ¿`a` y `b` tienen discriminantes de `Value` mutuamente excluyentes? "No"
/// (el lado seguro) para `Dynamic` emparejado con cualquier cosa (acepta
/// cualquier forma en ambas direcciones, `is_subtype`), dos `List` (una
/// lista vacía matchea cualquiera de los dos) y dos `Optional` (`null`
/// matchea ambos) -- ninguno de estos tres tiene un discriminante de
/// runtime que los distinga de forma confiable. Dos `Struct` siempre "no
/// conflicto" ACÁ (nivel de campo, chequeo superficial no recursivo): la
/// comparación real entre dos structs vive en
/// `union_members_are_distinguishable`, que mira sus campos compartidos,
/// no acá.
fn shallow_tag_conflict(a: &Type, b: &Type) -> bool {
    use Type::*;
    match (a, b) {
        (Dynamic, _) | (_, Dynamic) => false,
        (List(_), List(_)) => false,
        (Optional(_), _) | (_, Optional(_)) => false,
        (Struct { .. }, Struct { .. }) => false,
        (Enum(na), Enum(nb)) => na != nb,
        _ if a == b => false,
        _ => true,
    }
}

pub struct Checker {
    pub(crate) types: HashMap<String, TypeDecl>,
    pub(crate) enums: HashMap<String, EnumDecl>,
    fns: HashMap<String, (Vec<Type>, Type)>,
    /// Nombre de colección -> tipo de elemento ya resuelto, desde `db {
    /// ... }` (GRAMMAR.md §2.1, DbDecl). Vacío si el programa no declara
    /// ninguna `db` -- en ese caso `db` sigue existiendo como identificador
    /// (Type::Db), simplemente sin ninguna colección real.
    db_collections: HashMap<String, Type>,
}

impl Checker {
    /// Construye las tablas de símbolos (types/enums/fns) sin chequear los
    /// cuerpos de fn/rpc. Lo usa tanto `check_program` como el emisor de
    /// contrato (codegen/ts_emit.rs), que necesita `resolve_type` pero no
    /// quiere duplicar la lógica de resolución de nombres.
    pub(crate) fn build_symbols(program: &Program) -> (Self, Vec<CheckError>) {
        let mut checker = Checker {
            types: HashMap::new(),
            enums: HashMap::new(),
            fns: HashMap::new(),
            db_collections: HashMap::new(),
        };
        let mut errors = Vec::new();

        for item in &program.items {
            match item {
                // Duplicado detectado -- hallado al diseñar imports
                // multi-archivo (GRAMMAR.md §2.1): dos `type`/`enum` con el
                // mismo nombre ganaban por orden de inserción, en silencio.
                // Con un solo archivo ya era un gap real; con imports,
                // colisiones entre archivos se vuelven mucho más probables.
                Item::Type(t) if checker.types.contains_key(&t.name) => {
                    errors.push(err(format!("'{}' ya está declarado (type duplicado)", t.name)));
                }
                Item::Type(t) => {
                    checker.types.insert(t.name.clone(), t.clone());
                }
                Item::Enum(e) if checker.enums.contains_key(&e.name) => {
                    errors.push(err(format!("'{}' ya está declarado (enum duplicado)", e.name)));
                }
                Item::Enum(e) => {
                    checker.enums.insert(e.name.clone(), e.clone());
                }
                _ => {}
            }
        }

        for item in &program.items {
            if let Item::Fn(f) = item {
                if checker.fns.contains_key(&f.name) {
                    errors.push(err(format!("'{}' ya está declarado (fn duplicada)", f.name)));
                    continue;
                }
                match checker.resolve_fn_signature(f) {
                    Ok(sig) => {
                        checker.fns.insert(f.name.clone(), sig);
                    }
                    Err(e) => errors.push(e),
                }
            }
        }

        // Item::Db -- DESPUÉS de types/enums (resolver "User[]" necesita
        // poder encontrar "User" ya insertado). A lo sumo un `db { ... }`
        // en todo el Program ya fusionado (imports lo aplanan todo a un
        // solo Program, modules.rs, así que no hay "por archivo" acá).
        let mut db_decl_seen = false;
        for item in &program.items {
            if let Item::Db(db) = item {
                if db_decl_seen {
                    errors.push(err("ya hay un 'db { ... }' declarado en este programa (duplicado)"));
                    continue;
                }
                db_decl_seen = true;
                for coll in &db.collections {
                    match checker.resolve_type(&coll.ty) {
                        Ok(Type::List(element_ty)) => match checker.validate_db_element_type(&element_ty) {
                            Ok(()) => {
                                checker.db_collections.insert(coll.name.clone(), *element_ty);
                            }
                            Err(e) => errors.push(e),
                        },
                        Ok(other) => errors.push(err(format!(
                            "la colección '{}' de 'db' tiene que ser una lista de structs (T[]), se encontró {other:?}",
                            coll.name
                        ))),
                        Err(e) => errors.push(e),
                    }
                }
            }
        }

        (checker, errors)
    }

    /// Toda colección de `db` necesita un campo `id: Int` requerido --
    /// es lo que hace posible `insert(x: Omit<T,"id">)` sin romper la
    /// forma completa de T (GRAMMAR.md §2.1): sin esta regla, `insert`
    /// exigiendo el struct COMPLETO habría rechazado el propio demo
    /// insignia, donde `NewUser` es deliberadamente un subconjunto de `User`.
    fn validate_db_element_type(&self, element_ty: &Type) -> Result<(), CheckError> {
        let Type::Struct { fields, .. } = element_ty else {
            return Err(err(format!(
                "el tipo de elemento de una colección de 'db' tiene que ser un struct, se encontró {element_ty:?}"
            )));
        };
        if !fields.iter().any(|f| f.name == "id" && f.ty == Type::Int && !f.optional) {
            return Err(err(
                "toda colección de 'db' necesita un campo 'id: Int' requerido (no opcional, no nullable)",
            ));
        }
        Ok(())
    }

    pub fn check_program(program: &Program) -> Result<(), Vec<CheckError>> {
        let (checker, mut errors) = Self::build_symbols(program);

        for item in &program.items {
            match item {
                Item::Fn(f) => {
                    if let Err(e) = checker.check_fn(f) {
                        errors.push(e);
                    }
                }
                Item::Service(s) => {
                    for m in &s.members {
                        let (rpc, is_stream) = match m {
                            Member::Rpc(r) => (r, false),
                            Member::Stream(r) => (r, true),
                        };
                        if let Err(e) = checker.check_rpc(rpc, is_stream) {
                            errors.push(e);
                        }
                        if let Err(e) = checker.check_rpc_crosses_the_wire(rpc) {
                            errors.push(e);
                        }
                    }
                }
                Item::Const(c) => {
                    if let Err(e) = checker.check_const(c) {
                        errors.push(e);
                    }
                }
                _ => {}
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    // ---- resolución de TypeExpr (sintáctico) -> Type (resuelto) ----
    //
    // `resolve_type` es la fachada pública (subst vacío) que ya usa el
    // resto del checker sin cambios. `resolve_type_subst` es la que de
    // verdad hace el trabajo, y sabe qué hacer cuando un identificador de
    // tipo (ej. "T") está LIGADO a un tipo concreto por el subst actual --
    // así es como se resuelve el CUERPO de un genérico instanciado
    // (GRAMMAR.md §3.6, monomorfización): `Box<Int>` arma `{"T": Int}` y
    // resuelve `{value: T}` con ese subst, dando `{value: Int}`.

    pub(crate) fn resolve_type(&self, texpr: &TypeExpr) -> Result<Type, CheckError> {
        self.resolve_type_subst(texpr, &HashMap::new())
    }

    /// Resuelve la declaración ABSTRACTA (sin instanciar) de un genérico,
    /// para emitir `interface Box<T> { value: T }` tal cual en el .d.ts
    /// (ts_emit.rs) -- cada type_param se liga a `Type::TypeParam(nombre)`,
    /// que se renderiza literalmente como ese nombre en TypeScript.
    pub(crate) fn resolve_type_abstract(&self, texpr: &TypeExpr, type_params: &[String]) -> Result<Type, CheckError> {
        let subst: HashMap<String, Type> = type_params
            .iter()
            .map(|p| (p.clone(), Type::TypeParam(p.clone())))
            .collect();
        self.resolve_type_subst(texpr, &subst)
    }

    fn resolve_type_subst(&self, texpr: &TypeExpr, subst: &HashMap<String, Type>) -> Result<Type, CheckError> {
        match texpr {
            TypeExpr::Named(name, args) => self.resolve_named_type_subst(name, args, subst),
            TypeExpr::Struct(fields) => {
                let mut ftys = Vec::new();
                for f in fields {
                    ftys.push(FieldType {
                        name: f.name.clone(),
                        optional: f.optional,
                        ty: self.resolve_type_subst(&f.ty, subst)?,
                    });
                }
                Ok(Type::Struct { name: None, fields: ftys })
            }
            TypeExpr::Optional(inner) => Ok(Type::Optional(Box::new(self.resolve_type_subst(inner, subst)?))),
            TypeExpr::List(inner) => Ok(Type::List(Box::new(self.resolve_type_subst(inner, subst)?))),
            TypeExpr::Tuple(items) => {
                let mut tys = Vec::new();
                for i in items {
                    tys.push(self.resolve_type_subst(i, subst)?);
                }
                Ok(Type::Tuple(tys))
            }
            TypeExpr::Function(params, ret) => {
                let mut ptys = Vec::new();
                for p in params {
                    ptys.push(self.resolve_type_subst(p, subst)?);
                }
                Ok(Type::Function(ptys, Box::new(self.resolve_type_subst(ret, subst)?)))
            }
            TypeExpr::Map(_, _) => Err(err(
                "tipo map { K: V } todavía no soportado por el checker (ambigüedad real con structs de un campo, GRAMMAR.md §2.2) — usa Map<K, V>",
            )),
            TypeExpr::Union(members) => {
                let mut tys = Vec::new();
                for m in members {
                    tys.push(self.resolve_type_subst(m, subst)?);
                }
                Ok(Type::Union(tys))
            }
        }
    }

    fn resolve_named_type_subst(&self, name: &str, args: &[TypeExpr], subst: &HashMap<String, Type>) -> Result<Type, CheckError> {
        // "T" dentro del cuerpo de un genérico que YA está siendo resuelto
        // (instanciado o en modo abstracto) -- ver resolve_type_abstract.
        if let Some(bound) = subst.get(name) {
            if !args.is_empty() {
                return Err(err(format!("'{name}' es un parámetro de tipo, no toma argumentos")));
            }
            return Ok(bound.clone());
        }
        match name {
            "Int" => Ok(Type::Int),
            "Float" => Ok(Type::Float),
            "String" => Ok(Type::String),
            "Bool" => Ok(Type::Bool),
            "Void" => Ok(Type::Void),
            "Result" => {
                // Builtin (GRAMMAR.md §3.5), no un enum declarado por el
                // usuario. Sus variantes fijas se resuelven on-demand en
                // check_result_lit/variant_field_types, no acá.
                let [a, b] = args else {
                    return Err(err("Result<T, E> requiere exactamente 2 argumentos de tipo"));
                };
                Ok(Type::ResultOf(
                    Box::new(self.resolve_type_subst(a, subst)?),
                    Box::new(self.resolve_type_subst(b, subst)?),
                ))
            }
            "Patch" => {
                // Builtin (GRAMMAR.md §3.4). T debe resolver a un struct —
                // "parchear" un Int o un enum no tiene sentido en este diseño.
                let [inner] = args else {
                    return Err(err("Patch<T> requiere exactamente 1 argumento de tipo"));
                };
                match self.resolve_type_subst(inner, subst)? {
                    Type::Struct { .. } => Ok(Type::PatchOf(Box::new(self.resolve_type_subst(inner, subst)?))),
                    other => Err(err(format!(
                        "Patch<T> requiere que T sea un struct, se encontró {other:?}"
                    ))),
                }
            }
            "Map" => {
                // Builtin (GRAMMAR.md §2.2) -- documentado como el reemplazo
                // de `{K: V}` desde que se descubrió esa ambigüedad, pero
                // nunca conectado acá hasta ahora (bug real, no solo gap).
                let [k, v] = args else {
                    return Err(err("Map<K, V> requiere exactamente 2 argumentos de tipo"));
                };
                let k_ty = self.resolve_type_subst(k, subst)?;
                if !matches!(k_ty, Type::String | Type::Int) {
                    return Err(err(format!(
                        "Map<K, V>: K debe ser String o Int (son las únicas claves JSON válidas), se encontró {k_ty:?}"
                    )));
                }
                Ok(Type::MapOf(Box::new(k_ty), Box::new(self.resolve_type_subst(v, subst)?)))
            }
            _ => {
                if let Some(decl) = self.types.get(name) {
                    if decl.type_params.is_empty() {
                        if !args.is_empty() {
                            return Err(err(format!("'{name}' no es genérico, no toma argumentos de tipo")));
                        }
                        let resolved = self.resolve_type_subst(&decl.ty, subst)?;
                        Ok(match resolved {
                            Type::Struct { fields, .. } => Type::Struct { name: Some(name.to_string()), fields },
                            other => other, // alias a un tipo no-struct, ej. `type Id = Int`
                        })
                    } else {
                        // Genérico (GRAMMAR.md §3.6): NO se expande acá --
                        // queda "opaco" como Type::Generic hasta que hace
                        // falta la forma real (expand_generic_struct,
                        // variant_field_types), igual que Result/Patch/Map.
                        if args.len() != decl.type_params.len() {
                            return Err(err(format!(
                                "'{name}' espera {} argumento(s) de tipo, se dieron {}",
                                decl.type_params.len(),
                                args.len()
                            )));
                        }
                        let resolved_args = args
                            .iter()
                            .map(|a| self.resolve_type_subst(a, subst))
                            .collect::<Result<Vec<_>, _>>()?;
                        Ok(Type::Generic(name.to_string(), resolved_args))
                    }
                } else if let Some(decl) = self.enums.get(name) {
                    if decl.type_params.is_empty() {
                        if !args.is_empty() {
                            return Err(err(format!("'{name}' no es genérico, no toma argumentos de tipo")));
                        }
                        Ok(Type::Enum(name.to_string()))
                    } else {
                        if args.len() != decl.type_params.len() {
                            return Err(err(format!(
                                "'{name}' espera {} argumento(s) de tipo, se dieron {}",
                                decl.type_params.len(),
                                args.len()
                            )));
                        }
                        let resolved_args = args
                            .iter()
                            .map(|a| self.resolve_type_subst(a, subst))
                            .collect::<Result<Vec<_>, _>>()?;
                        Ok(Type::Generic(name.to_string(), resolved_args))
                    }
                } else {
                    Err(err(format!("tipo desconocido: '{name}'")))
                }
            }
        }
    }

    /// Expande un `type` genérico instanciado a sus campos reales, ej.
    /// `Box<Int>` -> `[FieldType{value, Int}]`. Usado por field access y
    /// construcción (ver check_expr/synth_expr) -- nunca por is_subtype,
    /// que compara genéricos nominalmente (mismo nombre + mismos args ya
    /// alcanza vía la igualdad derivada, ver types.rs).
    pub(crate) fn expand_generic_struct(&self, name: &str, args: &[Type]) -> Result<Vec<FieldType>, CheckError> {
        let decl = self.types.get(name).ok_or_else(|| err(format!("tipo desconocido: '{name}'")))?;
        let TypeExpr::Struct(fields) = &decl.ty else {
            return Err(err(format!("'{name}' no es un struct genérico, no se puede construir con {{...}}")));
        };
        let subst: HashMap<String, Type> = decl.type_params.iter().cloned().zip(args.iter().cloned()).collect();
        fields
            .iter()
            .map(|f| {
                Ok(FieldType {
                    name: f.name.clone(),
                    optional: f.optional,
                    ty: self.resolve_type_subst(&f.ty, &subst)?,
                })
            })
            .collect()
    }

    fn resolve_fn_signature(&self, f: &FnDecl) -> Result<(Vec<Type>, Type), CheckError> {
        let mut params = Vec::new();
        for p in &f.params {
            params.push(self.resolve_type(&p.ty)?);
        }
        Ok((params, self.resolve_type(&f.return_type)?))
    }

    // ---- ítems de nivel superior ----

    fn check_fn(&self, f: &FnDecl) -> Result<(), CheckError> {
        let ret = self.resolve_type(&f.return_type)?;
        let mut env = Env::new();
        for p in &f.params {
            // Los parámetros no tienen sintaxis `mut` propia -- son
            // siempre inmutables, igual que los bindings de patrones.
            env.insert(p.name.clone(), immutable(self.resolve_type(&p.ty)?));
        }
        self.check_block(&f.body, &ret, &env)
    }

    /// `const X: T = v` (GRAMMAR.md §2.1) -- hallado sin chequear ni emitir
    /// durante la auditoría final: se parseaba, pero `check_program` lo
    /// ignoraba del todo (`_ => {}`) y el emisor nunca lo tocaba. Ahora se
    /// valida `v ⇐ T` igual que cualquier otro valor con tipo esperado.
    fn check_const(&self, c: &ConstDecl) -> Result<(), CheckError> {
        let ty = self.resolve_type(&c.ty)?;
        self.check_expr(&c.value, &ty, &Env::new())
    }

    /// `is_stream` (`stream` en vez de `rpc`, GRAMMAR.md §2.1): la firma
    /// declara el tipo de ELEMENTO (ej. `-> User`, igual que un rpc normal
    /// -- así `AsyncIterable<User>` en el contrato TS sale del mismo
    /// `resolve_type(&r.return_type)` sin ningún caso especial en
    /// ts_emit.rs), pero el CUERPO tiene que producir la secuencia
    /// COMPLETA ya calculada: `List<User>`, no `User` suelto. Alcance v0
    /// explícito (PLAN.md §4, Fase 2): repetir una lista ya calculada por
    /// SSE real, no generadores ni suscripción a eventos futuros -- el
    /// lenguaje hoy no tiene ningún constructo de loop, así que "generador
    /// real" queda fuera de alcance con o sin este cambio.
    fn check_rpc(&self, r: &RpcDecl, is_stream: bool) -> Result<(), CheckError> {
        let ret = self.resolve_type(&r.return_type)?;
        let expected = if is_stream { Type::List(Box::new(ret)) } else { ret };
        let mut env = Env::new();
        for p in &r.params {
            let pty = self.resolve_type(&p.ty)?;
            if let Some(default) = &p.default {
                self.check_expr(default, &pty, &Env::new())?;
            }
            env.insert(p.name.clone(), immutable(pty));
        }
        self.check_block(&r.body, &expected, &env)
    }

    // ---- bloques y sentencias ----

    fn check_block(&self, block: &Block, expected: &Type, env: &Env) -> Result<(), CheckError> {
        let mut local = env.clone();
        for stmt in &block.stmts {
            match stmt {
                Stmt::Let { name, mutable, ty, value } => {
                    let value_ty = match ty {
                        Some(t) => {
                            let resolved = self.resolve_type(t)?;
                            self.check_expr(value, &resolved, &local)?;
                            resolved
                        }
                        None => self.synth_expr(value, &local)?,
                    };
                    local.insert(name.clone(), Binding { ty: value_ty, mutable: *mutable });
                }
                Stmt::Assign { name, value } => {
                    let binding = local
                        .get(name)
                        .ok_or_else(|| err(format!("variable no declarada: '{name}'")))?
                        .clone();
                    if !binding.mutable {
                        return Err(err(format!(
                            "no se puede asignar a '{name}': no fue declarada con 'mut' (GRAMMAR.md §2.3)"
                        )));
                    }
                    self.check_expr(value, &binding.ty, &local)?;
                }
                Stmt::Return(Some(e)) => self.check_expr(e, expected, &local)?,
                Stmt::Return(None) => {
                    if !is_subtype(&Type::Void, expected) {
                        return Err(err("'return' sin valor en una función que no devuelve Void"));
                    }
                }
                // if/match en posición de sentencia no tienen valor que
                // alguien use -- se chequean contra Void, lo que en la
                // práctica exige que cada rama sea puro efecto (sin tail),
                // igual que exigir `if cond { ... } else { ... }` sin usar
                // el resultado. synth_expr no sirve acá: if/match nunca
                // sintetizan (§3.1/§3.7, son de modo chequeo).
                Stmt::Expr(e @ (Expr::If { .. } | Expr::Match { .. })) => {
                    self.check_expr(e, &Type::Void, &local)?;
                }
                Stmt::Expr(e) => {
                    self.synth_expr(e, &local)?;
                }
            }
        }
        match &block.tail {
            Some(e) => self.check_expr(e, expected, &local),
            None => {
                if is_subtype(&Type::Void, expected) {
                    Ok(())
                } else {
                    Err(err(format!(
                        "el bloque no termina en una expresión y se esperaba un valor de tipo {expected:?}"
                    )))
                }
            }
        }
    }

    /// Todo lo que aparece en la firma de un `rpc`/`stream` viaja de verdad
    /// por la red, así que tiene que ser expresable como JSON.
    ///
    /// La tabla de mapeo (GRAMMAR.md §4) ya decía que un tipo función "no
    /// cruza el wire" y que `Void` es "solo válido como retorno de rpc",
    /// pero nada lo hacía cumplir: `type T = { h: (Int) -> String }` usado
    /// como retorno tipaba, emitía `h: (arg0: number) => string` al
    /// contrato, y generaba un validador con `typeof x.h === "function"` --
    /// una condición que ningún payload JSON puede satisfacer, así que el
    /// cliente rechazaba siempre. Mejor un error claro acá que un contrato
    /// imposible de cumplir.
    ///
    /// `Void` sí es válido como retorno de nivel superior (un rpc que no
    /// devuelve nada), pero no como parámetro ni anidado dentro de otro
    /// tipo, donde no significa nada.
    fn check_rpc_crosses_the_wire(&self, r: &RpcDecl) -> Result<(), CheckError> {
        for p in &r.params {
            let ty = self.resolve_type(&p.ty)?;
            check_wire_safe(&ty, &format!("el parámetro '{}' de '{}'", p.name, r.name), false)?;
        }
        let ret = self.resolve_type(&r.return_type)?;
        check_wire_safe(&ret, &format!("el retorno de '{}'", r.name), true)
    }

    /// Sintetiza el tipo de la cola de un bloque SIN ningún tipo esperado
    /// del contexto -- lo que necesita un closure cuyos parámetros están
    /// anotados pero cuyo tipo de retorno no viene de ningún lado
    /// (GRAMMAR.md §3.10, `synth_expr(Expr::Closure)`).
    ///
    /// A propósito NO es "espejar `check_block` y cambiar `check_expr` por
    /// `synth_expr` en la cola": `check_block` usa el mismo `expected` tanto
    /// para la cola como para cualquier `Stmt::Return` anidado, y un
    /// `if`/`match` en posición de sentencia (no cola) YA se chequea hoy
    /// contra `Type::Void` sin importar el `expected` real del bloque que
    /// lo contiene -- un bug preexistente y real (nunca ejercitado: `return`
    /// no se usa en ningún `.link` ni test existente), pero ortogonal a
    /// esta ronda -- se documenta acá, no se arregla. En vez de heredar ese
    /// bug de otra forma (ej. intentando enhebrar un "expected de return"
    /// separado a través de `check_expr`/`check_match`, que reimplementaría
    /// esa lógica), `synth_block` rechaza de entrada, con un error claro,
    /// cualquier `return` alcanzable desde el bloque que recorre -- incluso
    /// dentro de un `if`/`match` no-cola. `block_has_return`/`expr_has_return`
    /// (funciones libres, debajo de este `impl`) hacen ese barrido; nunca
    /// descienden a un `Expr::Closure` anidado -- ese closure tiene su
    /// PROPIO contexto de retorno, chequeado aparte cuando a él le toque.
    fn synth_block(&self, block: &Block, env: &Env) -> Result<Type, CheckError> {
        if block_has_return(block) {
            return Err(err(
                "un closure sin tipo de retorno conocido por contexto no puede usar 'return' -- anotá los tipos para que se chequee contra un tipo esperado, o reescribilo sin 'return' (GRAMMAR.md §3.10)",
            ));
        }
        let mut local = env.clone();
        for stmt in &block.stmts {
            match stmt {
                Stmt::Let { name, mutable, ty, value } => {
                    let value_ty = match ty {
                        Some(t) => {
                            let resolved = self.resolve_type(t)?;
                            self.check_expr(value, &resolved, &local)?;
                            resolved
                        }
                        None => self.synth_expr(value, &local)?,
                    };
                    local.insert(name.clone(), Binding { ty: value_ty, mutable: *mutable });
                }
                Stmt::Assign { name, value } => {
                    let binding = local
                        .get(name)
                        .ok_or_else(|| err(format!("variable no declarada: '{name}'")))?
                        .clone();
                    if !binding.mutable {
                        return Err(err(format!(
                            "no se puede asignar a '{name}': no fue declarada con 'mut' (GRAMMAR.md §2.3)"
                        )));
                    }
                    self.check_expr(value, &binding.ty, &local)?;
                }
                Stmt::Return(_) => unreachable!("descartado por block_has_return arriba"),
                Stmt::Expr(e @ (Expr::If { .. } | Expr::Match { .. })) => {
                    self.check_expr(e, &Type::Void, &local)?;
                }
                Stmt::Expr(e) => {
                    self.synth_expr(e, &local)?;
                }
            }
        }
        match &block.tail {
            Some(e) => self.synth_expr(e, &local),
            None => Ok(Type::Void),
        }
    }

    // ---- chequeo (modo ⇐): match y la construcción de Result<T,E> ----

    fn check_expr(&self, e: &Expr, expected: &Type, env: &Env) -> Result<(), CheckError> {
        match e {
            Expr::Match { scrutinee, arms } => self.check_match(scrutinee, arms, expected, env),
            // if/else es de modo chequeo, igual que match (GRAMMAR.md §3.7):
            // no tiene un tipo propio, necesita el esperado para verificar
            // que ambas ramas produzcan lo mismo que el contexto pide.
            Expr::If { cond, then_block, else_block } => {
                self.check_expr(cond, &Type::Bool, env)?;
                self.check_block(then_block, expected, env)?;
                self.check_block(else_block, expected, env)
            }
            Expr::StructLit { name, variant: Some(v), fields } if name == "Result" => {
                self.check_result_lit(v, fields, expected, env)
            }
            // Construcción de un type/enum genérico DECLARADO POR EL USUARIO
            // (GRAMMAR.md §3.6) -- igual que Result, no se puede sintetizar
            // sin contexto (¿de dónde saldrían los argumentos de tipo?),
            // así que necesita el `expected` ya instanciado como Generic.
            Expr::StructLit { name, variant, fields } if self.is_user_generic(name) => {
                self.check_generic_struct_lit(name, variant.as_deref(), fields, expected, env)
            }
            // '[]' vacío: sin esto, synth_expr fallaría (no hay elemento del
            // que inferir el tipo). Con un List(T) esperado, alcanza con
            // verificar que efectivamente se pidió una lista -- vacía
            // satisface "todos los elementos son T" sin elementos que revisar.
            Expr::ArrayLit(items) if items.is_empty() => match expected {
                Type::List(_) | Type::Dynamic => Ok(()),
                other => Err(err(format!(
                    "un array vacío '[]' requiere un tipo esperado de lista, se esperaba {other:?}"
                ))),
            },
            // Rama EXPLÍCITA, no delegar al fallback de abajo -- si esto
            // solo existiera en `synth_expr`, un closure sin anotar caería
            // acá, exigiría anotación en cada param (la regla de síntesis) y
            // perdería toda la inferencia contextual, que es todo el punto
            // de chequear (⇐) un closure contra un `Type::Function` ya
            // conocido (GRAMMAR.md §3.10, ej. el callback de `.filter`).
            Expr::Closure { params, body } => self.check_closure(params, body, expected, env),
            _ => {
                let t = self.synth_expr(e, env)?;
                if is_subtype(&t, expected) {
                    Ok(())
                } else {
                    Err(err(format!("se esperaba un valor de tipo {expected:?}, se encontró {t:?}")))
                }
            }
        }
    }

    /// `check_expr(Expr::Closure, expected, env)` -- solo válido si
    /// `expected` es un `Type::Function` ya conocido. Por cada parámetro,
    /// anotado o no, `expected_params[i]` es la referencia: si el usuario
    /// anotó un tipo, tiene que ACEPTAR lo que el contexto va a pasarle
    /// (contravariante -- `is_subtype(expected_pty, anotación)`, NUNCA al
    /// revés; ver el contraejemplo real en `types::params_accept`, que
    /// documenta esta misma dirección para que no se pueda invertir por
    /// accidente en un segundo lugar); si no anotó, se liga directo a
    /// `expected_params[i]` -- acá es donde `list.filter(|x| x.activo)`
    /// infiere el tipo de `x` sin que el usuario lo escriba.
    fn check_closure(
        &self,
        params: &[ClosureParam],
        body: &Block,
        expected: &Type,
        env: &Env,
    ) -> Result<(), CheckError> {
        let Type::Function(expected_params, expected_ret) = expected else {
            return Err(err(format!("se esperaba un valor de tipo {expected:?}, se encontró un closure")));
        };
        if params.len() != expected_params.len() {
            return Err(err(format!(
                "el closure tiene {} parámetro(s), el contexto espera {}",
                params.len(),
                expected_params.len()
            )));
        }
        let mut local = env.clone();
        for (p, expected_pty) in params.iter().zip(expected_params) {
            let pty = match &p.ty {
                Some(texpr) => {
                    let annotated = self.resolve_type(texpr)?;
                    if !is_subtype(expected_pty, &annotated) {
                        return Err(err(format!(
                            "el parámetro '{}' anotado como {annotated:?} no acepta lo que el contexto le pasa ({expected_pty:?})",
                            p.name
                        )));
                    }
                    annotated
                }
                None => expected_pty.clone(),
            };
            local.insert(p.name.clone(), immutable(pty));
        }
        self.check_block(body, expected_ret, &local)
    }

    fn check_result_lit(
        &self,
        variant: &str,
        fields: &[(String, Expr)],
        expected: &Type,
        env: &Env,
    ) -> Result<(), CheckError> {
        let Type::ResultOf(ok_ty, err_ty) = expected else {
            return Err(err(format!(
                "'Result.{variant} {{...}}' usado donde se esperaba {expected:?}, no un Result<T, E>"
            )));
        };
        match variant {
            "Ok" => self.check_single_field(fields, "value", ok_ty, env),
            "Err" => self.check_single_field(fields, "error", err_ty, env),
            other => Err(err(format!("Result no tiene variante '{other}' (solo Ok/Err)"))),
        }
    }

    fn check_single_field(
        &self,
        fields: &[(String, Expr)],
        expected_name: &str,
        ty: &Type,
        env: &Env,
    ) -> Result<(), CheckError> {
        if fields.len() != 1 || fields[0].0 != expected_name {
            return Err(err(format!("se esperaba exactamente el campo '{expected_name}'")));
        }
        self.check_expr(&fields[0].1, ty, env)
    }

    /// `true` si `name` es un `type`/`enum` DECLARADO POR EL USUARIO con
    /// type_params -- distinto de "Result"/"Patch"/"Map" (builtins, ya
    /// manejados aparte) y de un type/enum normal (sin type_params, sigue
    /// el camino existente de synth_struct_lit).
    fn is_user_generic(&self, name: &str) -> bool {
        self.types.get(name).is_some_and(|d| !d.type_params.is_empty())
            || self.enums.get(name).is_some_and(|d| !d.type_params.is_empty())
    }

    fn check_generic_struct_lit(
        &self,
        name: &str,
        variant: Option<&str>,
        fields: &[(String, Expr)],
        expected: &Type,
        env: &Env,
    ) -> Result<(), CheckError> {
        let Type::Generic(gname, gargs) = expected else {
            return Err(err(format!(
                "'{name}' es genérico -- se necesita un tipo esperado ya instanciado (ej. anotá el 'let', o usalo donde el tipo ya se conoce), se encontró {expected:?}"
            )));
        };
        if gname != name {
            return Err(err(format!("se esperaba '{gname}', se encontró una construcción de '{name}'")));
        }
        let field_decls: Vec<FieldType> = match variant {
            None => self.expand_generic_struct(name, gargs)?,
            Some(vname) => self
                .variant_field_types(expected, name, vname)?
                .into_iter()
                .map(|(n, ty)| FieldType { name: n, optional: false, ty })
                .collect(),
        };
        self.check_fields_against_resolved(&field_decls, fields, env)
    }

    fn check_match(
        &self,
        scrutinee: &Expr,
        arms: &[MatchArm],
        expected: &Type,
        env: &Env,
    ) -> Result<(), CheckError> {
        let scrutinee_ty = self.synth_expr(scrutinee, env)?;

        match &scrutinee_ty {
            Type::Enum(_) | Type::ResultOf(_, _) | Type::Generic(_, _) => {
                let enum_name = match &scrutinee_ty {
                    Type::Enum(n) => n.clone(),
                    Type::ResultOf(_, _) => "Result".to_string(),
                    Type::Generic(n, _) => n.clone(), // enum genérico instanciado, ej. Option<Int>
                    _ => unreachable!(),
                };
                self.check_exhaustive_enum(&scrutinee_ty, &enum_name, arms)?;
            }
            // Extensión más allá de enum (GRAMMAR.md §3.3): matchear un
            // primitivo con patrones de literal. Deliberadamente sin Float
            // (igualdad exacta de floats) ni Optional/Null (matchear un `T?`
            // directamente queda para más adelante, ver Pattern::Literal).
            Type::Int | Type::String | Type::Bool => {
                self.check_exhaustive_literal(&scrutinee_ty, arms)?;
            }
            // Narrowing de uniones (GRAMMAR.md §3.9): patrones `nombre: Tipo`.
            Type::Union(members) => {
                self.check_exhaustive_union(members, arms)?;
            }
            other => {
                return Err(err(format!(
                    "'match' requiere un valor de tipo enum, Int, String, Bool o unión; se encontró {other:?}"
                )))
            }
        }

        for arm in arms {
            let mut arm_env = env.clone();
            self.bind_pattern(&arm.pattern, &scrutinee_ty, &mut arm_env)?;
            // El guard ve las variables que el patrón acaba de ligar, ej.
            // `Status.Setting { level } if level > 10 => ...` -- por eso se
            // chequea acá, con arm_env, no con env.
            if let Some(guard) = &arm.guard {
                self.check_expr(guard, &Type::Bool, &arm_env)?;
            }
            match &arm.body {
                MatchArmBody::Expr(e) => self.check_expr(e, expected, &arm_env)?,
                MatchArmBody::Block(b) => self.check_block(b, expected, &arm_env)?,
            }
        }
        Ok(())
    }

    /// Algoritmo de GRAMMAR.md §3.3: cualquier `Pattern::Bind` SIN GUARD
    /// (incluye `_` y bindings con nombre, ej. `otro => ...`) es un catch-all
    /// irrefutable. Un arm CON guard nunca descarta exhaustividad -- la
    /// condición podría ser falsa en runtime, así que no cuenta como cubierto.
    fn check_exhaustive_enum(&self, scrutinee_ty: &Type, enum_name: &str, arms: &[MatchArm]) -> Result<(), CheckError> {
        let variants: Vec<String> = if matches!(scrutinee_ty, Type::ResultOf(_, _)) {
            vec!["Ok".to_string(), "Err".to_string()]
        } else {
            self.enum_variant_names(enum_name)?
        };

        let mut covered = HashSet::new();
        let mut wildcard = false;
        for arm in arms {
            if arm.guard.is_some() {
                continue;
            }
            self.collect_variant_coverage(&arm.pattern, enum_name, &mut wildcard, &mut covered)?;
        }

        if wildcard || variants.iter().all(|v| covered.contains(v)) {
            Ok(())
        } else {
            let missing: Vec<_> = variants.into_iter().filter(|v| !covered.contains(v)).collect();
            Err(err(format!(
                "match no exhaustivo sobre '{enum_name}': falta cubrir {missing:?} (GRAMMAR.md §3.3)"
            )))
        }
    }

    /// Recorre un patrón (posiblemente un `Or`) sumando a `covered`/`wildcard`
    /// -- separado de `check_exhaustive_enum` para que `Or` sea un solo punto
    /// de recursión, no un caso más a duplicar en cada algoritmo.
    fn collect_variant_coverage(
        &self,
        pattern: &Pattern,
        enum_name: &str,
        wildcard: &mut bool,
        covered: &mut HashSet<String>,
    ) -> Result<(), CheckError> {
        match pattern {
            Pattern::Bind(_) => {
                *wildcard = true;
                Ok(())
            }
            Pattern::Variant { enum_name: en, variant_name, .. } => {
                if en != enum_name {
                    return Err(err(format!(
                        "patrón para el enum '{en}' no coincide con el tipo del escrutinio ('{enum_name}')"
                    )));
                }
                covered.insert(variant_name.clone());
                Ok(())
            }
            Pattern::Or(subs) => {
                for s in subs {
                    self.collect_variant_coverage(s, enum_name, wildcard, covered)?;
                }
                Ok(())
            }
            Pattern::Literal(lit) => Err(err(format!(
                "patrón literal {lit:?} no válido contra un escrutinio de tipo enum ('{enum_name}')"
            ))),
            Pattern::Type(name, texpr) => Err(err(format!(
                "patrón de tipo '{name}: {texpr:?}' no válido contra un escrutinio de tipo enum ('{enum_name}') -- \
                 el narrowing de uniones (GRAMMAR.md §3.9) es solo para escrutinios Type::Union"
            ))),
        }
    }

    /// Exhaustividad para un escrutinio Int/String/Bool (GRAMMAR.md §3.3):
    /// los patrones de literal nunca alcanzan por sí solos -- Int/String
    /// tienen un espacio de valores no enumerable, así que siempre hace
    /// falta un catch-all sin guard. Única excepción: Bool, que sí se puede
    /// cubrir del todo con 'true' Y 'false' (es, en los hechos, un enum de
    /// dos variantes).
    fn check_exhaustive_literal(&self, scrutinee_ty: &Type, arms: &[MatchArm]) -> Result<(), CheckError> {
        let mut wildcard = false;
        let mut covered_bools: HashSet<bool> = HashSet::new();
        for arm in arms {
            if arm.guard.is_some() {
                continue;
            }
            self.collect_literal_coverage(&arm.pattern, scrutinee_ty, &mut wildcard, &mut covered_bools)?;
        }

        let bool_exhaustive = matches!(scrutinee_ty, Type::Bool) && covered_bools.len() == 2;

        if wildcard || bool_exhaustive {
            Ok(())
        } else {
            Err(err(format!(
                "match no exhaustivo: los patrones de literal nunca alcanzan por sí solos sobre {scrutinee_ty:?} -- \
                 hace falta un arm final sin guard que capture el resto (ej. '_ => ...'), salvo Bool con 'true' y \
                 'false' ambos cubiertos (GRAMMAR.md §3.3)"
            )))
        }
    }

    fn collect_literal_coverage(
        &self,
        pattern: &Pattern,
        scrutinee_ty: &Type,
        wildcard: &mut bool,
        covered_bools: &mut HashSet<bool>,
    ) -> Result<(), CheckError> {
        match pattern {
            Pattern::Bind(_) => {
                *wildcard = true;
                Ok(())
            }
            Pattern::Literal(lit) => {
                self.check_literal_matches_type(lit, scrutinee_ty)?;
                if let LiteralPattern::Bool(b) = lit {
                    covered_bools.insert(*b);
                }
                Ok(())
            }
            Pattern::Or(subs) => {
                for s in subs {
                    self.collect_literal_coverage(s, scrutinee_ty, wildcard, covered_bools)?;
                }
                Ok(())
            }
            Pattern::Variant { enum_name, .. } => Err(err(format!(
                "patrón de variante de enum ('{enum_name}') no válido contra un escrutinio de tipo {scrutinee_ty:?}"
            ))),
            Pattern::Type(name, texpr) => Err(err(format!(
                "patrón de tipo '{name}: {texpr:?}' no válido contra un escrutinio de tipo {scrutinee_ty:?} -- \
                 el narrowing de uniones (GRAMMAR.md §3.9) es solo para escrutinios Type::Union"
            ))),
        }
    }

    /// Narrowing de uniones (GRAMMAR.md §3.9): rechaza de entrada, ANTES de
    /// mirar los arms siquiera, una unión cuyos miembros no se puedan
    /// distinguir de forma demostrable (`union_members_are_distinguishable`)
    /// -- es una propiedad de la unión en sí, no de cómo se la matchea, así
    /// que corre una sola vez por par de miembros. Después, exhaustividad:
    /// mismo algoritmo que enum/literal (un `Pattern::Bind` sin guard cubre
    /// el resto; un arm con guard nunca descarta cobertura).
    fn check_exhaustive_union(&self, members: &[Type], arms: &[MatchArm]) -> Result<(), CheckError> {
        for i in 0..members.len() {
            for j in (i + 1)..members.len() {
                if !union_members_are_distinguishable(&members[i], &members[j]) {
                    return Err(err(format!(
                        "no se puede hacer 'match' sobre esta unión: los miembros {:?} y {:?} no se pueden \
                         distinguir de forma demostrable en runtime (GRAMMAR.md §3.9) -- si hace falta \
                         distinguirlos, modelá la alternancia como un 'enum' en vez de una unión estructural",
                        members[i], members[j]
                    )));
                }
            }
        }

        let mut wildcard = false;
        let mut covered = vec![false; members.len()];
        for arm in arms {
            if arm.guard.is_some() {
                continue;
            }
            self.collect_union_coverage(&arm.pattern, members, &mut wildcard, &mut covered)?;
        }

        if wildcard || covered.iter().all(|c| *c) {
            Ok(())
        } else {
            let missing: Vec<&Type> = members.iter().zip(&covered).filter(|(_, c)| !**c).map(|(m, _)| m).collect();
            Err(err(format!("match no exhaustivo sobre la unión: falta cubrir {missing:?} (GRAMMAR.md §3.9)")))
        }
    }

    /// Análogo a `collect_variant_coverage`/`collect_literal_coverage`, mismo
    /// patrón de "Or recursa, todo lo demás que no sea el patrón propio de
    /// este escrutinio es un error". `Type` no deriva `Hash`/`Eq` (solo
    /// `PartialEq`, y ese deriva es posicional sobre el `Vec<FieldType>` de
    /// un struct -- dos structs estructuralmente idénticos con campos en
    /// otro orden serían `==`-distintos), así que membership se decide por
    /// `is_subtype` MUTUO, no `==`; y la cobertura se trackea por posición
    /// (`Vec<bool>`), no `HashSet<Type>`.
    fn collect_union_coverage(
        &self,
        pattern: &Pattern,
        members: &[Type],
        wildcard: &mut bool,
        covered: &mut [bool],
    ) -> Result<(), CheckError> {
        match pattern {
            Pattern::Bind(_) => {
                *wildcard = true;
                Ok(())
            }
            Pattern::Type(_, texpr) => {
                let resolved = self.resolve_type(texpr)?;
                let mut matched_any = false;
                for (i, m) in members.iter().enumerate() {
                    if is_subtype(&resolved, m) && is_subtype(m, &resolved) {
                        covered[i] = true;
                        matched_any = true;
                    }
                }
                if matched_any {
                    Ok(())
                } else {
                    Err(err(format!(
                        "el patrón de tipo '{resolved:?}' no corresponde a ningún miembro de esta unión ({members:?})"
                    )))
                }
            }
            Pattern::Or(subs) => {
                for s in subs {
                    self.collect_union_coverage(s, members, wildcard, covered)?;
                }
                Ok(())
            }
            Pattern::Literal(lit) => Err(err(format!(
                "patrón literal {lit:?} no válido contra un escrutinio de tipo unión -- usá 'nombre: Tipo' (GRAMMAR.md §3.9)"
            ))),
            Pattern::Variant { enum_name, .. } => Err(err(format!(
                "patrón de variante de enum ('{enum_name}') no válido contra un escrutinio de tipo unión -- usá \
                 'nombre: Tipo' (GRAMMAR.md §3.9)"
            ))),
        }
    }

    fn check_literal_matches_type(&self, lit: &LiteralPattern, ty: &Type) -> Result<(), CheckError> {
        let ok = matches!(
            (lit, ty),
            (LiteralPattern::Int(_), Type::Int)
                | (LiteralPattern::Str(_), Type::String)
                | (LiteralPattern::Bool(_), Type::Bool)
        );
        if ok {
            Ok(())
        } else {
            Err(err(format!("el patrón literal {lit:?} no coincide con el tipo del escrutinio ({ty:?})")))
        }
    }

    pub(crate) fn enum_variant_names(&self, name: &str) -> Result<Vec<String>, CheckError> {
        self.enums
            .get(name)
            .map(|e| e.variants.iter().map(|v| v.name.clone()).collect())
            .ok_or_else(|| err(format!("enum desconocido: '{name}'")))
    }

    /// Da tipo a las variables que un patrón introduce, recursivamente —
    /// `Enum.Variante { campo: patrón_anidado }` puede anidar otro patrón.
    fn bind_pattern(&self, pattern: &Pattern, ty: &Type, env: &mut Env) -> Result<(), CheckError> {
        match pattern {
            Pattern::Bind(name) => {
                env.insert(name.clone(), immutable(ty.clone()));
                Ok(())
            }
            Pattern::Literal(lit) => self.check_literal_matches_type(lit, ty),
            Pattern::Variant { enum_name, variant_name, fields } => {
                let variant_fields = self.variant_field_types(ty, enum_name, variant_name)?;
                if let Some(fps) = fields {
                    for fp in fps {
                        let field_ty = variant_fields
                            .iter()
                            .find(|(n, _)| n == &fp.name)
                            .map(|(_, t)| t.clone())
                            .ok_or_else(|| err(format!("'{enum_name}.{variant_name}' no tiene campo '{}'", fp.name)))?;
                        self.bind_pattern(&fp.pattern, &field_ty, env)?;
                    }
                }
                Ok(())
            }
            // Alcance v0 (ver doc de Pattern::Or en ast.rs): ninguna
            // alternativa puede ligar nombres -- evita tener que reconciliar
            // "las N ramas bindean las mismas variables del mismo tipo"
            // (la parte cara de or-patterns en otros lenguajes).
            Pattern::Or(subs) => {
                for s in subs {
                    if !pattern_bindings(s).is_empty() {
                        return Err(err(
                            "las alternativas de un patrón 'A | B' no pueden introducir bindings (GRAMMAR.md §3.3) \
                             -- ninguna rama puede usar un nombre propio ni capturar un campo, solo literales o \
                             variantes sin capturar",
                        ));
                    }
                    self.bind_pattern(s, ty, env)?;
                }
                Ok(())
            }
            // Genérico a propósito -- no valida membership contra ninguna
            // lista de miembros de unión: para cuando el escrutinio es
            // Type::Union, `check_exhaustive_union` ya validó eso ANTES de
            // que el loop de arms llegue a bindear (checker.rs::check_match),
            // así que acá alcanza con resolver y ligar. Escribirlo así,
            // sin pedir una `Type::Union` en particular, es también lo que
            // permite que un `Pattern::Type` funcione anidado dentro de un
            // `FieldPattern` (`Enum.Variante { campo: p: Int }`), no solo
            // como patrón de tope de un match sobre unión.
            Pattern::Type(name, texpr) => {
                let resolved = self.resolve_type(texpr)?;
                env.insert(name.clone(), immutable(resolved));
                Ok(())
            }
        }
    }

    pub(crate) fn variant_field_types(
        &self,
        scrutinee_ty: &Type,
        enum_name: &str,
        variant_name: &str,
    ) -> Result<Vec<(String, Type)>, CheckError> {
        if let Type::ResultOf(ok_ty, err_ty) = scrutinee_ty {
            return match variant_name {
                "Ok" => Ok(vec![("value".to_string(), (**ok_ty).clone())]),
                "Err" => Ok(vec![("error".to_string(), (**err_ty).clone())]),
                other => Err(err(format!("Result no tiene variante '{other}'"))),
            };
        }
        // Enum genérico instanciado (GRAMMAR.md §3.6): arma el subst
        // type_param->arg concreto y resuelve los campos de la variante
        // con ESE subst, igual que expand_generic_struct para structs.
        if let Type::Generic(base_name, args) = scrutinee_ty {
            let decl = self
                .enums
                .get(base_name.as_str())
                .ok_or_else(|| err(format!("enum desconocido: '{base_name}'")))?;
            let variant = decl
                .variants
                .iter()
                .find(|v| v.name == variant_name)
                .ok_or_else(|| err(format!("'{base_name}' no tiene variante '{variant_name}'")))?;
            let subst: HashMap<String, Type> =
                decl.type_params.iter().cloned().zip(args.iter().cloned()).collect();
            let mut out = Vec::new();
            if let Some(fields) = &variant.fields {
                for f in fields {
                    out.push((f.name.clone(), self.resolve_type_subst(&f.ty, &subst)?));
                }
            }
            return Ok(out);
        }
        let decl = self
            .enums
            .get(enum_name)
            .ok_or_else(|| err(format!("enum desconocido: '{enum_name}'")))?;
        let variant = decl
            .variants
            .iter()
            .find(|v| v.name == variant_name)
            .ok_or_else(|| err(format!("'{enum_name}' no tiene variante '{variant_name}'")))?;
        let mut out = Vec::new();
        if let Some(fields) = &variant.fields {
            for f in fields {
                out.push((f.name.clone(), self.resolve_type(&f.ty)?));
            }
        }
        Ok(out)
    }

    // ---- síntesis (modo ⇒) ----

    fn synth_expr(&self, e: &Expr, env: &Env) -> Result<Type, CheckError> {
        match e {
            Expr::Int(_) => Ok(Type::Int),
            Expr::Float(_) => Ok(Type::Float),
            Expr::Str(_) => Ok(Type::String),
            Expr::Bool(_) => Ok(Type::Bool),
            Expr::Null => Ok(Type::Null),
            Expr::Ident(name) => {
                // El lookup de variables va PRIMERO -- antes, "db" se
                // chequeaba acá arriba de todo, así que un `let db = ...`
                // de un usuario quedaba sombreado en silencio por el
                // builtin (hallado al diseñar "DB tipada", GRAMMAR.md §2.1).
                if let Some(b) = env.get(name) {
                    return Ok(b.ty.clone());
                }
                if name == "db" {
                    return Ok(Type::Db);
                }
                if let Some((params, ret)) = self.fns.get(name) {
                    return Ok(Type::Function(params.clone(), Box::new(ret.clone())));
                }
                Err(err(format!("variable no declarada: '{name}'")))
            }
            Expr::FieldAccess { base, field } => {
                let base_ty = self.synth_expr(base, env)?;
                match base_ty {
                    Type::Dynamic => Ok(Type::Dynamic),
                    Type::Struct { fields, .. } => fields
                        .iter()
                        .find(|f| &f.name == field)
                        .map(field_access_ty)
                        .ok_or_else(|| err(format!("el struct no tiene campo '{field}'"))),
                    // struct genérico instanciado, ej. una variable Box<Int>
                    Type::Generic(name, args) => self
                        .expand_generic_struct(&name, &args)?
                        .iter()
                        .find(|f| &f.name == field)
                        .map(field_access_ty)
                        .ok_or_else(|| err(format!("el struct no tiene campo '{field}'"))),
                    // `db.<coleccion>` -- nombre desconocido ya es un error
                    // acá mismo, no `Dynamic` dejando pasar cualquier cosa.
                    Type::Db => self
                        .db_collections
                        .get(field.as_str())
                        .map(|element_ty| Type::DbCollection(Box::new(element_ty.clone())))
                        .ok_or_else(|| err(format!("'db' no tiene ninguna colección llamada '{field}'"))),
                    other => Err(err(format!("no se puede acceder al campo '{field}' sobre {other:?}"))),
                }
            }
            Expr::Call { callee, args } => {
                if let Some(ty) = self.try_builtin_method(callee, args, env)? {
                    return Ok(ty);
                }
                let callee_ty = self.synth_expr(callee, env)?;
                match callee_ty {
                    Type::Dynamic => {
                        for a in args {
                            self.synth_expr(a, env)?;
                        }
                        Ok(Type::Dynamic)
                    }
                    Type::Function(params, ret) => {
                        if params.len() != args.len() {
                            return Err(err(format!(
                                "se esperaban {} argumentos, se dieron {}",
                                params.len(),
                                args.len()
                            )));
                        }
                        for (a, p) in args.iter().zip(&params) {
                            self.check_expr(a, p, env)?;
                        }
                        Ok(*ret)
                    }
                    other => Err(err(format!("no se puede llamar un valor de tipo {other:?}"))),
                }
            }
            Expr::StructLit { name, variant, fields } => {
                self.synth_struct_lit(name, variant.as_deref(), fields, env)
            }
            Expr::Match { .. } => Err(err(
                "'match' en posición de síntesis no soportado — necesita un tipo esperado del contexto (GRAMMAR.md §3.1, regla Match es de modo chequeo)",
            )),
            Expr::If { .. } => Err(err(
                "'if' en posición de síntesis no soportado — necesita un tipo esperado del contexto (GRAMMAR.md §3.7, misma familia que match)",
            )),
            Expr::Binary { op, left, right } => self.synth_binary(*op, left, right, env),
            Expr::Unary { op, operand } => self.synth_unary(*op, operand, env),
            // Un array vacío no sintetiza -- no hay de dónde inferir el
            // tipo del elemento (GRAMMAR.md §2.3). Eso vive en check_expr.
            Expr::ArrayLit(items) => {
                let mut iter = items.iter();
                let Some(first) = iter.next() else {
                    return Err(err(
                        "un array vacío '[]' no se puede sintetizar sin un tipo esperado (ej. anotá el 'let': let xs: Int[] = [])",
                    ));
                };
                let elem_ty = self.synth_expr(first, env)?;
                for item in iter {
                    self.check_expr(item, &elem_ty, env)?;
                }
                Ok(Type::List(Box::new(elem_ty)))
            }
            Expr::Index { base, index } => {
                let base_ty = self.synth_expr(base, env)?;
                self.check_expr(index, &Type::Int, env)?;
                match base_ty {
                    Type::List(elem_ty) => Ok(*elem_ty),
                    Type::Dynamic => Ok(Type::Dynamic),
                    other => Err(err(format!("no se puede indexar un valor de tipo {other:?} (se esperaba una lista)"))),
                }
            }
            Expr::TupleLit(items) => {
                let mut tys = Vec::new();
                for item in items {
                    tys.push(self.synth_expr(item, env)?);
                }
                Ok(Type::Tuple(tys))
            }
            Expr::TupleIndex { base, index } => {
                let base_ty = self.synth_expr(base, env)?;
                match base_ty {
                    Type::Tuple(items) => items.get(*index).cloned().ok_or_else(|| {
                        err(format!(
                            "índice de tupla .{index} fuera de rango (tiene {} elementos)",
                            items.len()
                        ))
                    }),
                    Type::Dynamic => Ok(Type::Dynamic),
                    other => Err(err(format!("'.{index}' requiere una tupla, se encontró {other:?}"))),
                }
            }
            Expr::Paren(inner) => self.synth_expr(inner, env),
            // Sin ningún `Type::Function` esperado del contexto (a
            // diferencia de `check_closure`), la ÚNICA forma de saber el
            // tipo de cada parámetro es que el usuario lo haya anotado --
            // de ahí el error explícito si falta, en vez de un mensaje
            // confuso de "no se pudo resolver" más abajo.
            Expr::Closure { params, body } => {
                let mut param_tys = Vec::new();
                let mut local = env.clone();
                for p in params {
                    let Some(texpr) = &p.ty else {
                        return Err(err(format!(
                            "el parámetro '{}' de este closure necesita anotación de tipo -- sin un tipo de función esperado del contexto (ej. como argumento de '.filter'/'.map', o un 'let' con tipo declarado), cada parámetro tiene que anotarse (GRAMMAR.md §3.10)",
                            p.name
                        )));
                    };
                    let ty = self.resolve_type(texpr)?;
                    local.insert(p.name.clone(), immutable(ty.clone()));
                    param_tys.push(ty);
                }
                let ret_ty = self.synth_block(body, &local)?;
                Ok(Type::Function(param_tys, Box::new(ret_ty)))
            }
        }
    }

    /// GRAMMAR.md §3.7 — sin coerción implícita: Int+Int o Float+Float, no
    /// mezclados. `Dynamic` (el escape hatch de `db`, ver types.rs) sigue
    /// siendo compatible con cualquier operando, igual que en el resto del
    /// checker.
    fn synth_binary(&self, op: BinaryOp, left: &Expr, right: &Expr, env: &Env) -> Result<Type, CheckError> {
        use BinaryOp::*;
        match op {
            // '+' es el único aritmético que también sirve para concatenar
            // strings -- resta/multiplicación/división sobre texto no
            // tienen un significado razonable, así que quedan aparte.
            Add => {
                let l = self.synth_expr(left, env)?;
                let r = self.synth_expr(right, env)?;
                match (&l, &r) {
                    (Type::Int, Type::Int) => Ok(Type::Int),
                    (Type::Float, Type::Float) => Ok(Type::Float),
                    (Type::String, Type::String) => Ok(Type::String),
                    (Type::Dynamic, _) | (_, Type::Dynamic) => Ok(Type::Dynamic),
                    _ => Err(err(format!(
                        "'+' requiere Int+Int, Float+Float o String+String sin mezclar (GRAMMAR.md §3.7); se encontró {l:?} y {r:?}"
                    ))),
                }
            }
            Sub | Mul | Div | Rem => {
                let l = self.synth_expr(left, env)?;
                let r = self.synth_expr(right, env)?;
                match (&l, &r) {
                    (Type::Int, Type::Int) => Ok(Type::Int),
                    (Type::Float, Type::Float) => Ok(Type::Float),
                    (Type::Dynamic, _) | (_, Type::Dynamic) => Ok(Type::Dynamic),
                    _ => Err(err(format!(
                        "operador aritmético requiere Int+Int o Float+Float sin mezclar (GRAMMAR.md §3.7); se encontró {l:?} y {r:?}"
                    ))),
                }
            }
            Eq | NotEq => {
                let l = self.synth_expr(left, env)?;
                let r = self.synth_expr(right, env)?;
                if type_contains_function(&l) || type_contains_function(&r) {
                    return Err(err(
                        "'==' / '!=' no están definidos sobre valores de tipo función/closure (GRAMMAR.md §3.10)",
                    ));
                }
                // Comparables si son mutuamente compatibles (mismo tipo, o
                // uno de los dos Dynamic) -- no solo primitivos: dos enums
                // nominales del mismo tipo también se pueden comparar.
                if matches!(l, Type::Dynamic) || matches!(r, Type::Dynamic) || is_subtype(&l, &r) || is_subtype(&r, &l)
                {
                    Ok(Type::Bool)
                } else {
                    Err(err(format!(
                        "'==' / '!=' requieren operandos de tipos compatibles; se encontró {l:?} y {r:?}"
                    )))
                }
            }
            Lt | LtEq | Gt | GtEq => {
                let l = self.synth_expr(left, env)?;
                let r = self.synth_expr(right, env)?;
                match (&l, &r) {
                    (Type::Int, Type::Int) | (Type::Float, Type::Float) => Ok(Type::Bool),
                    (Type::Dynamic, _) | (_, Type::Dynamic) => Ok(Type::Bool),
                    _ => Err(err(format!(
                        "operador relacional requiere Int+Int o Float+Float; se encontró {l:?} y {r:?}"
                    ))),
                }
            }
            And | Or => {
                self.check_expr(left, &Type::Bool, env)?;
                self.check_expr(right, &Type::Bool, env)?;
                Ok(Type::Bool)
            }
        }
    }

    fn synth_unary(&self, op: UnaryOp, operand: &Expr, env: &Env) -> Result<Type, CheckError> {
        match op {
            UnaryOp::Neg => {
                let t = self.synth_expr(operand, env)?;
                match t {
                    Type::Int | Type::Float | Type::Dynamic => Ok(t),
                    other => Err(err(format!("'-' unario requiere Int o Float, se encontró {other:?}"))),
                }
            }
            UnaryOp::Not => {
                self.check_expr(operand, &Type::Bool, env)?;
                Ok(Type::Bool)
            }
        }
    }

    /// Reconoce `base.metodo(args)` como un builtin sobre un primitivo
    /// (GRAMMAR.md §3.8) antes de que el camino genérico intente resolver
    /// `callee` como FieldAccess normal (que fallaría: Int/Float/String no
    /// son Struct ni Dynamic). `Ok(None)` = no es un builtin conocido, seguí
    /// con el camino genérico de Call sin tocar nada.
    fn try_builtin_method(&self, callee: &Expr, args: &[Expr], env: &Env) -> Result<Option<Type>, CheckError> {
        let Expr::FieldAccess { base, field } = callee else {
            return Ok(None);
        };
        let base_ty = self.synth_expr(base, env)?;
        // `db.<coleccion>.<metodo>(...)` -- a diferencia de los builtins de
        // primitivos de abajo, un nombre de método desconocido acá es
        // siempre un error, nunca `Ok(None)` (que dejaría que el camino
        // genérico de Call lo reintente y produzca un error más confuso).
        if let Type::DbCollection(element_ty) = &base_ty {
            return self.check_db_method(element_ty, field, args, env).map(Some);
        }
        let ty = match (&base_ty, field.as_str()) {
            (Type::Int, "toFloat") => {
                self.expect_no_args(args, "toFloat")?;
                Some(Type::Float)
            }
            (Type::Float, "toInt") => {
                self.expect_no_args(args, "toInt")?;
                Some(Type::Int)
            }
            (Type::String, "length") => {
                self.expect_no_args(args, "length")?;
                Some(Type::Int)
            }
            (Type::String, "contains") => {
                let [needle] = args else {
                    return Err(err("'contains' toma exactamente 1 argumento"));
                };
                self.check_expr(needle, &Type::String, env)?;
                Some(Type::Bool)
            }
            // Ya tenía implementación real en runtime/mod.rs (call_method)
            // desde antes -- lo que faltaba era la regla del checker. Nunca
            // se notó porque `db.coleccion.all()` devolvía `Dynamic` (sin
            // chequear nada) hasta esta misma tarea; recién ahora que
            // devuelve `List<T>` de verdad hace falta esta regla para que
            // `.take(n)` siga tipando.
            (Type::List(inner), "take") => {
                let [n_arg] = args else {
                    return Err(err("'take' toma exactamente 1 argumento (n: Int)"));
                };
                self.check_expr(n_arg, &Type::Int, env)?;
                Some(Type::List(inner.clone()))
            }
            // El caso FÁCIL de los dos métodos de orden superior (GRAMMAR.md
            // §3.10): el tipo del callback (T) -> Bool ya se conoce ENTERO
            // de entrada, así que alcanza con el mismo check_expr(Closure,
            // expected, ...) que cualquier otro argumento de tipo función.
            (Type::List(inner), "filter") => {
                let [pred_arg] = args else {
                    return Err(err("'filter' toma exactamente 1 argumento (predicado (T) -> Bool)"));
                };
                let expected_fn = Type::Function(vec![(**inner).clone()], Box::new(Type::Bool));
                self.check_expr(pred_arg, &expected_fn, env)?;
                Some(Type::List(inner.clone()))
            }
            // El caso DIFÍCIL: el tipo de retorno del callback (U) es
            // exactamente lo que no se conoce de entrada -- `synth_callback_result`
            // lo sintetiza en vez de chequearlo contra un tipo ya fijo.
            (Type::List(inner), "map") => {
                let [f_arg] = args else {
                    return Err(err("'map' toma exactamente 1 argumento (f: (T) -> U)"));
                };
                let result_ty = self.synth_callback_result(f_arg, inner, env)?;
                Some(Type::List(Box::new(result_ty)))
            }
            _ => None,
        };
        Ok(ty)
    }

    /// Tipo de retorno de invocar `callback` con un único argumento de tipo
    /// `param_ty` -- lo que `.map` necesita para saber el tipo de elemento
    /// de la lista resultante, que no se conoce de entrada (a diferencia de
    /// `.filter`, cuyo callback siempre devuelve `Bool`).
    ///
    /// Dos formas de callback, dos caminos DISTINTOS a propósito:
    /// - Un closure literal: no hay ningún `Type::Function` ya resuelto del
    ///   que `synth_expr` pueda partir (el propio closure es lo que hay que
    ///   sintetizar) -- se liga el param a `param_ty` (single-source-of-truth
    ///   si no está anotado; si está anotado, tiene que ACEPTAR `param_ty`,
    ///   comprobado con `is_subtype(param_ty, anotación)` -- dirección NORMAL
    ///   de argumento, no la contravariante de `check_closure`, porque acá
    ///   `param_ty` es un tipo CONCRETO que de verdad se va a pasar, no el
    ///   tipo esperado de un `Type::Function` completo) y se sintetiza el
    ///   cuerpo con `synth_block`.
    /// - Cualquier otra cosa (ej. una `fn` referenciada por nombre): ya
    ///   sintetiza un `Type::Function` completo por su cuenta -- se verifica
    ///   que acepte `param_ty` como argumento (subtipado normal, igual que
    ///   cualquier otro argumento de una llamada) y se devuelve su retorno.
    fn synth_callback_result(&self, callback: &Expr, param_ty: &Type, env: &Env) -> Result<Type, CheckError> {
        if let Expr::Closure { params, body } = callback {
            let [p] = params.as_slice() else {
                return Err(err(format!(
                    "el callback de 'map' necesita exactamente 1 parámetro, se encontraron {}",
                    params.len()
                )));
            };
            let bound_ty = match &p.ty {
                Some(texpr) => {
                    let annotated = self.resolve_type(texpr)?;
                    if !is_subtype(param_ty, &annotated) {
                        return Err(err(format!(
                            "el parámetro '{}' anotado como {annotated:?} no acepta el elemento real de la lista ({param_ty:?})",
                            p.name
                        )));
                    }
                    annotated
                }
                None => param_ty.clone(),
            };
            let mut local = env.clone();
            local.insert(p.name.clone(), immutable(bound_ty));
            return self.synth_block(body, &local);
        }
        let callback_ty = self.synth_expr(callback, env)?;
        let Type::Function(actual_params, ret) = callback_ty else {
            return Err(err(format!(
                "el callback de 'map' tiene que ser una función de 1 parámetro, se encontró {callback_ty:?}"
            )));
        };
        let [actual_param] = actual_params.as_slice() else {
            return Err(err(format!(
                "el callback de 'map' necesita exactamente 1 parámetro, se encontraron {}",
                actual_params.len()
            )));
        };
        if !is_subtype(param_ty, actual_param) {
            return Err(err(format!(
                "el callback de 'map' no acepta el elemento real de la lista ({param_ty:?} no es subtipo de {actual_param:?})"
            )));
        }
        Ok(*ret)
    }

    /// `all/find/insert/applyPatch` sobre una colección de `db` (GRAMMAR.md
    /// §2.1) -- resueltos contra `element_ty` de verdad, así que un método
    /// desconocido ya es un error de tipos acá, no algo que se descubre en
    /// runtime (`Type::Dynamic` dejaba pasar cualquier nombre antes).
    fn check_db_method(&self, element_ty: &Type, method: &str, args: &[Expr], env: &Env) -> Result<Type, CheckError> {
        match method {
            "all" => {
                self.expect_no_args(args, "all")?;
                Ok(Type::List(Box::new(element_ty.clone())))
            }
            "find" => {
                let [id_arg] = args else {
                    return Err(err("'find' toma exactamente 1 argumento (id: Int)"));
                };
                self.check_expr(id_arg, &Type::Int, env)?;
                Ok(Type::Optional(Box::new(element_ty.clone())))
            }
            "insert" => {
                // Omit<T, "id"> (GRAMMAR.md §2.1): T completo rechazaría el
                // propio demo insignia, donde el shape de creación
                // (NewUser) es deliberadamente un subconjunto de T.
                let [value_arg] = args else {
                    return Err(err("'insert' toma exactamente 1 argumento"));
                };
                let insertable = self.omit_id_field(element_ty)?;
                self.check_expr(value_arg, &insertable, env)?;
                Ok(element_ty.clone())
            }
            "applyPatch" => {
                let [id_arg, patch_arg] = args else {
                    return Err(err("'applyPatch' toma exactamente 2 argumentos (id: Int, patch: Patch<T>)"));
                };
                self.check_expr(id_arg, &Type::Int, env)?;
                self.check_expr(patch_arg, &Type::PatchOf(Box::new(element_ty.clone())), env)?;
                Ok(element_ty.clone())
            }
            other => Err(err(format!(
                "'{other}' no es un método conocido de una colección de 'db' (all/find/insert/applyPatch)"
            ))),
        }
    }

    /// "Los campos de T menos 'id'" -- estructural, sin sintaxis de tipo
    /// nueva (se emite como `Omit<T, "id">`, un utility type nativo de TS,
    /// ver ts_emit.rs). `validate_db_element_type` ya garantizó que 'id'
    /// existe al procesar `Item::Db` -- el chequeo acá es una segunda
    /// defensa, no la única.
    fn omit_id_field(&self, element_ty: &Type) -> Result<Type, CheckError> {
        let Type::Struct { fields, .. } = element_ty else {
            return Err(err("una colección de 'db' debe resolver a un struct"));
        };
        if !fields.iter().any(|f| f.name == "id") {
            return Err(err("cada colección de 'db' necesita un campo 'id: Int'"));
        }
        let without_id: Vec<FieldType> = fields.iter().filter(|f| f.name != "id").cloned().collect();
        Ok(Type::Struct { name: None, fields: without_id })
    }

    fn expect_no_args(&self, args: &[Expr], method: &str) -> Result<(), CheckError> {
        if !args.is_empty() {
            return Err(err(format!("'{method}' no toma argumentos")));
        }
        Ok(())
    }

    fn synth_struct_lit(
        &self,
        name: &str,
        variant: Option<&str>,
        fields: &[(String, Expr)],
        env: &Env,
    ) -> Result<Type, CheckError> {
        if name == "Result" {
            return Err(err(
                "'Result.Ok'/'Result.Err' necesitan un tipo esperado del contexto (ej. el retorno declarado del rpc) — no se pueden usar en posición de síntesis (GRAMMAR.md §3.5)",
            ));
        }
        // Un type/enum genérico no puede sintetizarse: ¿de dónde saldrían
        // sus argumentos de tipo sin un `expected` que ya los traiga? Mismo
        // motivo que Result arriba -- ver check_generic_struct_lit.
        if self.is_user_generic(name) {
            return Err(err(format!(
                "'{name}' es genérico -- necesita un tipo esperado del contexto para inferir los argumentos de tipo (ej. anotá el 'let', o usalo donde el tipo ya se conoce)"
            )));
        }
        match variant {
            Some(vname) => {
                let decl = self
                    .enums
                    .get(name)
                    .ok_or_else(|| err(format!("enum desconocido: '{name}'")))?;
                let v = decl
                    .variants
                    .iter()
                    .find(|v| v.name == vname)
                    .ok_or_else(|| err(format!("'{name}' no tiene variante '{vname}'")))?;
                self.check_fields_against(v.fields.as_deref().unwrap_or(&[]), fields, env)?;
                Ok(Type::Enum(name.to_string()))
            }
            None => {
                let decl = self.types.get(name).ok_or_else(|| err(format!("tipo desconocido: '{name}'")))?;
                let TypeExpr::Struct(decl_fields) = &decl.ty else {
                    return Err(err(format!("'{name}' no es un tipo struct, no se puede construir con {{...}}")));
                };
                self.check_fields_against(decl_fields, fields, env)?;
                self.resolve_type(&TypeExpr::Named(name.to_string(), vec![]))
            }
        }
    }

    fn check_fields_against(
        &self,
        decl_fields: &[Field],
        given: &[(String, Expr)],
        env: &Env,
    ) -> Result<(), CheckError> {
        let resolved = decl_fields
            .iter()
            .map(|f| {
                Ok(FieldType {
                    name: f.name.clone(),
                    optional: f.optional,
                    ty: self.resolve_type(&f.ty)?,
                })
            })
            .collect::<Result<Vec<_>, CheckError>>()?;
        self.check_fields_against_resolved(&resolved, given, env)
    }

    /// Igual que `check_fields_against`, pero para cuando los campos ya
    /// están resueltos (con un subst de genérico ya aplicado) -- ver
    /// `check_generic_struct_lit`, que no puede usar `resolve_type` normal
    /// porque los campos de un genérico instanciado necesitan `resolve_type_subst`.
    fn check_fields_against_resolved(
        &self,
        decl_fields: &[FieldType],
        given: &[(String, Expr)],
        env: &Env,
    ) -> Result<(), CheckError> {
        for (fname, fexpr) in given {
            let decl_f = decl_fields
                .iter()
                .find(|f| &f.name == fname)
                .ok_or_else(|| err(format!("campo desconocido: '{fname}'")))?;
            self.check_expr(fexpr, &decl_f.ty, env)?;
        }
        for decl_f in decl_fields {
            if !decl_f.optional && !given.iter().any(|(n, _)| n == &decl_f.name) {
                return Err(err(format!("falta el campo requerido '{}'", decl_f.name)));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::parse;

    fn check_source(src: &str) -> Result<(), Vec<CheckError>> {
        let tokens = tokenize(src).unwrap_or_else(|e| panic!("{e}"));
        let program = parse(tokens).unwrap_or_else(|e| panic!("{e}"));
        Checker::check_program(&program)
    }

    #[test]
    fn full_users_demo_file_typechecks() {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/users.link"),
        )
        .expect("no se pudo leer examples/users.link");
        let result = check_source(&src);
        assert!(result.is_ok(), "errores de tipo inesperados: {:#?}", result.unwrap_err());
    }

    #[test]
    fn missing_required_field_is_rejected() {
        let src = r#"
            type Point = { x: Int, y: Int }
            fn origin() -> Point { Point { x: 0 } }
        "#;
        let result = check_source(src);
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains('y'), "el error debería mencionar el campo faltante 'y': {msg}");
    }

    #[test]
    fn non_exhaustive_match_is_rejected() {
        let src = r#"
            enum Status { Active, Paused, Cancelled }
            fn describe(s: Status) -> String {
                match s {
                    Status.Active => "activo",
                    Status.Paused => "pausado",
                }
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("Cancelled"), "debería señalar el variant faltante: {msg}");
    }

    #[test]
    fn wildcard_arm_satisfies_exhaustiveness() {
        let src = r#"
            enum Status { Active, Paused, Cancelled }
            fn describe(s: Status) -> String {
                match s {
                    Status.Active => "activo",
                    other => "otro",
                }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn or_pattern_combines_enum_variants_into_one_arm() {
        let src = r#"
            enum Status { Active, Paused, Cancelled }
            fn describe(s: Status) -> String {
                match s {
                    Status.Active | Status.Paused => "en curso",
                    Status.Cancelled => "cancelado",
                }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn or_pattern_branch_that_binds_a_field_is_rejected() {
        // Alcance v0 (ast.rs, doc de Pattern::Or): ninguna alternativa de un
        // '|' puede introducir bindings, así que combinar dos variantes que
        // capturan un campo -- aunque compartan nombre -- no está permitido.
        let src = r#"
            enum Shape { Circle { r: Int }, Square { r: Int } }
            fn area_hint(s: Shape) -> Int {
                match s {
                    Shape.Circle { r } | Shape.Square { r } => r,
                }
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "un patrón 'A | B' no debería poder bindear campos");
    }

    #[test]
    fn literal_match_over_int_requires_a_trailing_catch_all() {
        // Int tiene un espacio de valores no enumerable -- a diferencia de un
        // enum, ningún conjunto finito de literales agota el tipo (GRAMMAR.md §3.3).
        let src = r#"
            fn describe(n: Int) -> String {
                match n {
                    1 => "uno",
                    2 => "dos",
                }
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "literales sobre Int nunca deberían bastar sin un catch-all");
    }

    #[test]
    fn literal_match_over_int_with_wildcard_and_or_pattern_is_accepted() {
        let src = r#"
            fn describe(n: Int) -> String {
                match n {
                    1 | 2 => "bajo",
                    -1 => "negativo",
                    _ => "otro",
                }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn bool_match_covering_both_values_is_exhaustive_without_wildcard() {
        // Bool es, en los hechos, un enum de dos variantes -- único caso
        // donde literales solos (sin catch-all) sí alcanzan (GRAMMAR.md §3.3).
        let src = r#"
            fn describe(b: Bool) -> String {
                match b {
                    true => "sí",
                    false => "no",
                }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn string_literal_pattern_type_mismatch_is_rejected() {
        let src = r#"
            fn describe(n: Int) -> String {
                match n {
                    "uno" => "no debería tipar",
                    _ => "otro",
                }
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "un patrón String no debería aceptarse contra un escrutinio Int");
    }

    #[test]
    fn guarded_arm_alone_does_not_satisfy_exhaustiveness() {
        // Un guard puede fallar en runtime -- por más que su patrón sería un
        // catch-all sin el guard, no puede ser la única cobertura del match.
        let src = r#"
            fn describe(n: Int) -> String {
                match n {
                    x if x > 0 => "positivo",
                }
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "un solo arm con guard nunca es exhaustivo por sí solo");
    }

    #[test]
    fn guard_condition_must_synthesize_bool() {
        let src = r#"
            fn describe(n: Int) -> String {
                match n {
                    x if x => "no debería tipar",
                    _ => "otro",
                }
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "el guard 'if x' sobre un Int no es Bool");
    }

    #[test]
    fn guard_sees_the_bindings_introduced_by_its_own_pattern() {
        let src = r#"
            enum Setting { Level { value: Int } }
            fn describe(s: Setting) -> String {
                match s {
                    Setting.Level { value } if value > 10 => "alto",
                    Setting.Level { value } => "bajo",
                }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn wrong_argument_count_is_rejected() {
        let src = r#"
            fn add(a: Int, b: Int) -> Int { a }
            fn use_it() -> Int { add(1) }
        "#;
        let result = check_source(src);
        assert!(result.is_err());
    }

    #[test]
    fn assigning_to_mut_variable_is_accepted() {
        assert!(check_source(
            "fn f() -> Int { let mut x = 1; x = 2; x }"
        ).is_ok());
    }

    #[test]
    fn assigning_to_non_mut_variable_is_rejected() {
        let result = check_source("fn f() -> Int { let x = 1; x = 2; x }");
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("mut"), "el error debería mencionar 'mut': {msg}");
    }

    #[test]
    fn assigning_to_undeclared_variable_is_rejected() {
        assert!(check_source("fn f() -> Int { x = 2; 0 }").is_err());
    }

    #[test]
    fn assigning_wrong_type_is_rejected() {
        let result = check_source(r#"fn f() -> Int { let mut x = 1; x = "no"; x }"#);
        assert!(result.is_err());
    }

    #[test]
    fn array_literal_infers_from_first_element_and_checks_the_rest() {
        assert!(check_source("fn f() -> Int[] { [1, 2, 3] }").is_ok());
        assert!(check_source(r#"fn f() -> Int[] { [1, "no", 3] }"#).is_err());
    }

    #[test]
    fn empty_array_needs_an_expected_type() {
        assert!(check_source("fn f() -> Int[] { [] }").is_ok());
        // en posición de síntesis (sin contexto) debe fallar
        assert!(check_source("fn f() -> Int { let xs = []; 0 }").is_err());
    }

    #[test]
    fn indexing_returns_the_element_type_and_requires_int_index() {
        assert!(check_source("fn f() -> Int { let xs = [1, 2, 3]; xs[0] }").is_ok());
        assert!(check_source(r#"fn f() -> Int { let xs = [1, 2, 3]; xs["0"] }"#).is_err());
        assert!(check_source("fn f() -> Int { let x = 5; x[0] }").is_err()); // Int no es indexable
    }

    #[test]
    fn numeric_conversion_methods_work() {
        assert!(check_source("fn f(n: Int) -> Float { n.toFloat() }").is_ok());
        assert!(check_source("fn f(n: Float) -> Int { n.toInt() }").is_ok());
    }

    #[test]
    fn tuple_literal_synthesizes_and_index_returns_element_type() {
        assert!(check_source(r#"fn f() -> (Int, String) { (1, "a") }"#).is_ok());
        assert!(check_source(r#"fn f() -> Int { let t = (1, "a"); t.0 }"#).is_ok());
        assert!(check_source(r#"fn f() -> String { let t = (1, "a"); t.1 }"#).is_ok());
    }

    #[test]
    fn tuple_index_out_of_range_or_wrong_type_is_rejected() {
        assert!(check_source(r#"fn f() -> Int { let t = (1, "a"); t.2 }"#).is_err());
        assert!(check_source("fn f() -> Int { let x = 5; x.0 }").is_err());
    }

    #[test]
    fn string_length_and_contains_work() {
        assert!(check_source(r#"fn f(s: String) -> Int { s.length() }"#).is_ok());
        assert!(check_source(r#"fn f(s: String) -> Bool { s.contains("@") }"#).is_ok());
    }

    #[test]
    fn string_methods_reject_wrong_args() {
        assert!(check_source(r#"fn f(s: String) -> Int { s.length(1) }"#).is_err());
        assert!(check_source(r#"fn f(s: String) -> Bool { s.contains(1) }"#).is_err());
    }

    #[test]
    fn numeric_conversion_rejects_wrong_receiver_or_args() {
        assert!(check_source("fn f(n: Float) -> Float { n.toFloat() }").is_err()); // toFloat es de Int
        assert!(check_source("fn f(n: Int) -> Float { n.toFloat(1) }").is_err()); // no toma argumentos
    }

    #[test]
    fn map_of_string_int_is_accepted() {
        // Bug real: esto estaba documentado en GRAMMAR.md como el reemplazo
        // de {K:V} pero nunca se conectó al checker -- tiraba "tipo
        // desconocido: 'Map'" antes de este fix.
        assert!(check_source("fn f(m: Map<String, Int>) -> Int { 0 }").is_ok());
        assert!(check_source("fn f(m: Map<Int, String>) -> Int { 0 }").is_ok());
    }

    #[test]
    fn map_rejects_non_json_key_types() {
        let result = check_source("fn f(m: Map<Bool, Int>) -> Int { 0 }");
        assert!(result.is_err());
    }

    #[test]
    fn generic_struct_instantiates_constructs_and_accesses_fields() {
        let src = r#"
            type Box<T> = { value: T }
            fn wrap(n: Int) -> Box<Int> { Box { value: n } }
            fn unwrap(b: Box<Int>) -> Int { b.value }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn generic_enum_instantiates_constructs_matches_exhaustively() {
        let src = r#"
            enum Option<T> {
                Some { value: T },
                None,
            }
            fn find(has_it: Bool, n: Int) -> Option<Int> {
                // Option.None necesita "{}" explícito COMO EXPRESIÓN (el
                // lookahead del parser solo reconoce un literal de variante
                // si ve "{" después) -- distinto del patrón de match, que
                // no lo exige para una variante sin campos.
                if has_it { Option.Some { value: n } } else { Option.None {} }
            }
            fn unwrap_or(o: Option<Int>, default: Int) -> Int {
                match o {
                    Option.Some { value: v } => v,
                    Option.None => default,
                }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn generic_match_still_requires_exhaustiveness() {
        let src = r#"
            enum Option<T> { Some { value: T }, None }
            fn f(o: Option<Int>) -> Int {
                match o {
                    Option.Some { value: v } => v,
                }
            }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn generic_construction_without_context_is_rejected() {
        // Igual que Result: no hay de dónde inferir los argumentos de tipo
        // sin un `expected` -- síntesis pura no alcanza (GRAMMAR.md §3.6).
        let src = r#"
            type Box<T> = { value: T }
            fn f() -> Int {
                let b = Box { value: 1 };
                0
            }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn generic_wrong_arg_count_is_rejected() {
        let src = r#"
            type Pair<A, B> = { first: A, second: B }
            fn f(p: Pair<Int>) -> Int { 0 }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn different_generic_instantiations_are_not_interchangeable() {
        // Decisión deliberada (GRAMMAR.md §3.6): una vez genérico, la
        // comparación es NOMINAL (nombre + args), no estructural -- aunque
        // Box<Int> y un struct plano {value: Int} tengan la misma forma,
        // no son intercambiables.
        let src = r#"
            type Box<T> = { value: T }
            fn takes_box(b: Box<Int>) -> Int { b.value }
            fn f(plain: { value: Int }) -> Int { takes_box(plain) }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn patch_of_non_struct_is_rejected() {
        let src = r#"
            fn f(p: Patch<Int>) -> Int { 0 }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "Patch<Int> no debería aceptarse: T tiene que ser un struct");
    }

    #[test]
    fn patch_of_struct_is_accepted_and_widens_all_fields() {
        let src = r#"
            type User = { name: String, bio?: String }
            fn apply(id: Int, patch: Patch<User>) -> User {
                User { name: "x" }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn structurally_equivalent_inline_type_accepted() {
        // Un `type A` con la MISMA forma que el tipo inline del parámetro
        // debe aceptarse — subtipado estructural, no nominal (GRAMMAR.md §3.2).
        let src = r#"
            type A = { x: Int }
            fn f(v: { x: Int }) -> Int { v.x }
            fn use_it() -> Int { f(A { x: 1 }) }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn const_value_matching_its_declared_type_is_accepted() {
        // Hallado sin chequear durante la auditoría final: check_program
        // ignoraba Item::Const del todo (GRAMMAR.md §2.1/§4).
        assert!(check_source("const MAX_RETRIES: Int = 3;").is_ok());
    }

    #[test]
    fn const_value_not_matching_its_declared_type_is_rejected() {
        let result = check_source(r#"const MAX_RETRIES: Int = "tres";"#);
        assert!(result.is_err(), "un const declarado Int no debería aceptar un valor String");
    }

    #[test]
    fn arithmetic_ok_same_numeric_type() {
        assert!(check_source("fn add(a: Int, b: Int) -> Int { a + b * 2 - 1 }").is_ok());
        assert!(check_source("fn add(a: Float, b: Float) -> Float { a / b }").is_ok());
    }

    #[test]
    fn plus_concatenates_strings_but_other_arithmetic_ops_reject_them() {
        assert!(check_source(r#"fn greet(name: String) -> String { "hola, " + name }"#).is_ok());
        assert!(check_source(r#"fn f(a: String, b: String) -> String { a - b }"#).is_err());
    }

    #[test]
    fn arithmetic_rejects_mixed_int_and_float() {
        // GRAMMAR.md §3.7: sin coerción implícita -- Int y Float no se mezclan.
        let result = check_source("fn f(a: Int, b: Float) -> Float { a + b }");
        assert!(result.is_err());
    }

    #[test]
    fn comparison_and_logical_operators_produce_bool() {
        let src = r#"
            fn f(a: Int, b: Int) -> Bool {
                a < b && a != b || !(a == b)
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn logical_operators_reject_non_bool_operands() {
        let result = check_source("fn f(a: Int, b: Int) -> Bool { a && b }");
        assert!(result.is_err());
    }

    #[test]
    fn if_else_both_branches_must_match_expected_type() {
        assert!(check_source("fn f(x: Int) -> Int { if x > 0 { x } else { 0 } }").is_ok());

        // La rama else devuelve String donde se esperaba Int -- debe fallar.
        let result = check_source(r#"fn f(x: Int) -> Int { if x > 0 { x } else { "no" } }"#);
        assert!(result.is_err());
    }

    #[test]
    fn if_condition_must_be_bool() {
        let result = check_source("fn f(x: Int) -> Int { if x { 1 } else { 0 } }");
        assert!(result.is_err());
    }

    #[test]
    fn else_if_chain_typechecks() {
        let src = r#"
            fn classify(x: Int) -> String {
                if x > 0 { "positivo" } else if x < 0 { "negativo" } else { "cero" }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn concrete_member_of_a_union_param_is_accepted() {
        // Alcance v0 (types.rs, doc de Type::Union): flujo de valor hacia
        // un parámetro/campo tipado como unión, sin angosto posterior.
        let src = r#"
            fn f(x: Int | String) -> Int { 0 }
            fn use_it() -> Int { f(1) }
        "#;
        assert!(check_source(src).is_ok());
        let src2 = r#"
            fn f(x: Int | String) -> Int { 0 }
            fn use_it() -> Int { f("hola") }
        "#;
        assert!(check_source(src2).is_ok());
    }

    #[test]
    fn non_member_type_is_rejected_by_union_param() {
        let src = r#"
            fn f(x: Int | String) -> Int { 0 }
            fn use_it() -> Int { f(true) }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "Bool no es miembro de Int | String");
    }

    #[test]
    fn union_field_in_struct_is_accepted() {
        let src = r#"
            type Event = { payload: Int | String }
            fn make() -> Event { Event { payload: 1 } }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn named_fn_referenced_by_name_synthesizes_a_function_type() {
        // GRAMMAR.md §3.10: una `fn` de nivel superior referenciada por
        // nombre (sin llamarla ahí mismo) es un valor de tipo Function --
        // Expr::Ident cae a `self.fns` cuando no hay binding local con ese
        // nombre. Ver runtime/mod.rs para la contraparte en ejecución (FnRef).
        let src = r#"
            fn add_one(x: Int) -> Int { x + 1 }
            fn apply_twice(f: (Int) -> Int, x: Int) -> Int { f(f(x)) }
            fn use_it() -> Int { apply_twice(add_one, 5) }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn fn_reference_with_incompatible_signature_is_rejected() {
        let src = r#"
            fn add_one(x: Int) -> Int { x + 1 }
            fn apply_to_bool(f: (Bool) -> Bool, x: Bool) -> Bool { f(x) }
            fn use_it() -> Bool { apply_to_bool(add_one, true) }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "(Int)->Int no debería servir donde se pide (Bool)->Bool");
    }

    #[test]
    fn db_collection_without_id_field_is_rejected() {
        let src = r#"
            type Post = { title: String }
            db { posts: Post[] }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "toda colección de db necesita un campo 'id: Int'");
    }

    #[test]
    fn db_collection_that_is_not_a_list_of_structs_is_rejected() {
        let src = "db { posts: Int }";
        let result = check_source(src);
        assert!(result.is_err(), "una colección de db tiene que ser T[], no un tipo suelto");
    }

    #[test]
    fn duplicate_db_declaration_is_rejected() {
        let src = r#"
            type Post = { id: Int }
            db { posts: Post[] }
            db { posts: Post[] }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "no puede haber dos 'db {{ ... }}' en el mismo programa");
    }

    #[test]
    fn unknown_db_collection_name_is_rejected() {
        let src = r#"
            type Post = { id: Int }
            db { posts: Post[] }
            fn broken() -> Post? { db.comments.find(1) }
        "#;
        let result = check_source(src);
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("comments"), "debería señalar la colección desconocida: {msg}");
    }

    #[test]
    fn unknown_db_method_is_rejected() {
        let src = r#"
            type Post = { id: Int }
            db { posts: Post[] }
            fn broken() -> Post? { db.posts.fnid(1) }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "'fnid' no es un método real de una colección de db");
    }

    #[test]
    fn db_all_and_find_resolve_to_the_real_collection_type() {
        let src = r#"
            type Post = { id: Int, title: String }
            db { posts: Post[] }
            fn listAll() -> Post[] { db.posts.all() }
            fn one(id: Int) -> Post? { db.posts.find(id) }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn db_insert_accepts_the_element_type_without_id_but_rejects_the_id_field_missing_other_fields() {
        let src = r#"
            type Post = { id: Int, title: String }
            db { posts: Post[] }
            type NewPost = { title: String }
            fn create(input: NewPost) -> Post { db.posts.insert(input) }
        "#;
        assert!(check_source(src).is_ok(), "NewPost (sin id) debería servir para insert: Omit<Post,\"id\">");

        let src_missing_field = r#"
            type Post = { id: Int, title: String, body: String }
            db { posts: Post[] }
            type Incomplete = { title: String }
            fn create(input: Incomplete) -> Post { db.posts.insert(input) }
        "#;
        let result = check_source(src_missing_field);
        assert!(result.is_err(), "falta 'body' -- Incomplete no alcanza para Omit<Post,\"id\">");
    }

    #[test]
    fn db_apply_patch_requires_a_patch_of_the_element_type() {
        // Mismo patrón que examples/users.link: Patch<T> llega como
        // parámetro (del wire), no se construye con un literal acá.
        let src = r#"
            type Post = { id: Int, title: String }
            db { posts: Post[] }
            fn rename(id: Int, patch: Patch<Post>) -> Post { db.posts.applyPatch(id, patch) }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn user_variable_named_db_shadows_the_builtin() {
        // Hallado al diseñar "DB tipada": antes, "db" se chequeaba ANTES del
        // lookup de variables, así que un `let db = ...` de un usuario
        // quedaba sombreado en silencio por el builtin. Ahora el lookup de
        // variables va primero.
        let src = "fn f() -> Int { let db = 5; db }";
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn stream_rpc_body_is_checked_against_list_of_the_declared_element_type() {
        // La firma declara el ELEMENTO (`-> User`, igual que un rpc normal),
        // pero el cuerpo de un `stream` tiene que devolver la secuencia
        // COMPLETA ya calculada (GRAMMAR.md §2.1).
        let src = r#"
            type User = { id: Int, name: String }
            db { users: User[] }
            service Users {
                stream watchAll() -> User { db.users.all() }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn stream_rpc_body_returning_bare_element_instead_of_list_is_rejected() {
        // El error real que este chequeo existe para atrapar: devolver un
        // solo User (lo que un `rpc` normal pediría) donde `stream` pide
        // List<User> -- antes de esta ronda el checker chequeaba el cuerpo
        // de un `stream` contra `User` directamente (mismo camino que
        // `rpc`), así que esto SÍ tipaba antes y no debería tipar ahora.
        // Literal directo (no `db.users.find(id)`, que devuelve `User?` y
        // fallaría de todos modos por nullable-vs-no-nullable, sin probar
        // lo que este test quiere probar).
        let src = r#"
            type User = { id: Int, name: String }
            service Users {
                stream watchOne() -> User { User { id: 1, name: "Ada" } }
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "un stream que devuelve un User suelto (no List<User>) debería fallar");
    }

    #[test]
    fn stream_rpc_body_returning_list_of_wrong_element_type_is_rejected() {
        let src = r#"
            type User = { id: Int, name: String }
            type Post = { id: Int, title: String }
            db { users: User[] }
            fn allPosts() -> Post[] { [] }
            service Users {
                stream watchAll() -> User { allPosts() }
            }
        "#;
        let result = check_source(src);
        assert!(
            result.is_err(),
            "un stream declarado -> User no debería aceptar un cuerpo List<Post>"
        );
    }

    // ---- closures + List.map/.filter (GRAMMAR.md §3.10) ----

    #[test]
    fn closure_param_annotated_narrower_than_actual_element_is_accepted() {
        // El closure solo pide lo que usa ({x: Int}) -- la lista real trae
        // más campos (WidePoint), y eso tiene que alcanzar por contravarianza
        // (types::params_accept).
        let src = r#"
            type WidePoint = { x: Int, y: Int, z: Int }
            fn run(points: WidePoint[]) -> WidePoint[] {
                points.filter(|p: { x: Int }| { p.x > 0 })
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn closure_param_annotated_wider_than_actual_element_is_rejected() {
        // Al revés: el closure anota MÁS campos de los que el elemento real
        // tiene -- si esto se aceptara, el cuerpo podría leer un campo que
        // el dato real nunca tuvo y crashear en runtime. Una implementación
        // con la dirección de subtipado invertida aceptaría esto por error
        // (hallado por un review de diseño antes de escribir el resto de
        // la feature, ver el comentario de `types::params_accept`).
        let src = r#"
            type NarrowPoint = { x: Int }
            type WidePoint = { x: Int, y: Int, z: Int }
            fn run(points: NarrowPoint[]) -> NarrowPoint[] {
                points.filter(|p: WidePoint| { p.x > 0 })
            }
        "#;
        let result = check_source(src);
        assert!(
            result.is_err(),
            "un closure que anota MÁS campos de los que el elemento real tiene debería rechazarse"
        );
    }

    #[test]
    fn equality_between_function_typed_values_is_rejected() {
        let src = r#"
            fn same(a: (Int) -> Int, b: (Int) -> Int) -> Bool { a == b }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "comparar dos valores de tipo función debería rechazarse");
    }

    #[test]
    fn closure_without_context_needs_every_param_annotated() {
        let src = r#"
            fn make() -> Int {
                let f = |x| { x + 1 };
                f(1)
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "un closure sin contexto (let sin tipo declarado) necesita anotar sus params");
    }

    #[test]
    fn return_inside_a_closure_without_known_return_type_is_rejected() {
        let src = r#"
            fn make() -> Int {
                let f = |x: Int| { return x + 1; };
                f(1)
            }
        "#;
        let result = check_source(src);
        assert!(
            result.is_err(),
            "un closure sintetizado (sin retorno conocido por contexto) no puede usar 'return'"
        );
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("return"), "el error debería mencionar 'return': {msg}");
    }

    #[test]
    fn closure_with_unannotated_params_infers_from_let_type_annotation() {
        let src = r#"
            fn make() -> Int {
                let f: (Int) -> Int = |x| { x + 1 };
                f(5)
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn filter_over_a_list_keeps_the_same_element_type() {
        let src = r#"
            type User = { id: Int, active: Bool }
            fn activeOnly(users: User[]) -> User[] {
                users.filter(|u: User| { u.active })
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn filter_rejects_a_predicate_that_does_not_return_bool() {
        let src = r#"
            fn run(xs: Int[]) -> Int[] {
                xs.filter(|x: Int| { x + 1 })
            }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn map_over_a_list_can_change_the_element_type() {
        let src = r#"
            type User = { id: Int, name: String }
            fn names(users: User[]) -> String[] {
                users.map(|u: User| { u.name })
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn map_accepts_a_named_fn_reference_as_callback() {
        let src = r#"
            fn double(x: Int) -> Int { x * 2 }
            fn run(xs: Int[]) -> Int[] {
                xs.map(double)
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn map_infers_unannotated_closure_param_from_the_list_element_type() {
        let src = r#"
            type User = { id: Int, name: String }
            fn names(users: User[]) -> String[] {
                users.map(|u| { u.name })
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn nested_closure_inside_a_closure_body_typechecks() {
        let src = r#"
            fn run(xs: Int[]) -> Int[][] {
                xs.map(|x: Int| { xs.filter(|y: Int| { y > x }) })
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    // ---- narrowing de uniones (GRAMMAR.md §3.9) ----

    #[test]
    fn a_function_type_cannot_cross_the_wire() {
        // §4 ya lo decía ("no cruza el wire") pero nada lo hacía cumplir:
        // el contrato emitía `h: (arg0: number) => string` y el validador
        // generado exigía `typeof x.h === "function"`, imposible de
        // satisfacer con JSON -- el cliente rechazaba SIEMPRE.
        let nested = r#"
            type T = { id: Int, h: (Int) -> String }
            service S { rpc get() -> T { T { id: 1, h: f } } }
            fn f(x: Int) -> String { "x" }
        "#;
        assert!(check_source(nested).is_err(), "un campo función dentro de un tipo de retorno debe rechazarse");

        let param = r#"
            service S { rpc go(f: (Int) -> Int) -> Int { f(1) } }
        "#;
        assert!(check_source(param).is_err(), "un parámetro de tipo función debe rechazarse");

        // Pero DENTRO del backend sigue siendo perfectamente válido.
        let internal = r#"
            fn add_one(x: Int) -> Int { x + 1 }
            fn apply(f: (Int) -> Int, x: Int) -> Int { f(x) }
            service S { rpc go(x: Int) -> Int { apply(add_one, x) } }
        "#;
        assert!(check_source(internal).is_ok());
    }

    #[test]
    fn void_is_only_valid_as_a_whole_rpc_return() {
        let ok = "service S { rpc ping() -> Void { } }";
        assert!(check_source(ok).is_ok());

        let as_field = r#"
            type T = { id: Int, v: Void }
            service S { rpc get() -> T? { null } }
        "#;
        assert!(check_source(as_field).is_err(), "Void como campo de struct debe rechazarse");

        let as_param = "service S { rpc go(v: Void) -> Int { 1 } }";
        assert!(check_source(as_param).is_err(), "Void como parámetro debe rechazarse");
    }

    #[test]
    fn a_match_does_not_need_a_trailing_comma_on_its_last_arm() {
        // Exigirla rechazaba `match x { A => 1, B => 2 }` con un críptico
        // "se esperaba Comma, se encontró RBrace".
        let src = r#"
            fn describe(n: Int) -> String { match n { 1 => "uno", _ => "otro" } }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn reading_an_optional_key_field_yields_an_optional_type() {
        // `note?: String` es opcionalidad de CLAVE (puede estar ausente,
        // GRAMMAR.md §3.4) -- leerlo da `String?`, no `String`. Antes de la
        // auditoría esto tipaba y después fallaba en runtime con "no existe
        // el campo 'note'" al recibir un objeto válido sin esa clave.
        let src = r#"
            type A = { id: Int, note?: String }
            fn get(a: A) -> String { a.note }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "leer un campo de clave opcional no puede dar el tipo pelado");

        // Declarado como `String?` sí tipa -- que es justo la forma de
        // escribirlo correctamente.
        let ok = r#"
            type A = { id: Int, note?: String }
            fn get(a: A) -> String? { a.note }
        "#;
        assert!(check_source(ok).is_ok());
    }

    #[test]
    fn arithmetic_on_an_optional_key_field_is_rejected() {
        let src = r#"
            type A = { n?: Int }
            fn get(a: A) -> Int { a.n + 1 }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn a_struct_missing_an_optional_field_is_still_a_subtype() {
        // Width subtyping (GRAMMAR.md §3.2/§3.4): si el supertipo declara el
        // campo OPCIONAL, un valor que no lo trae sigue siendo válido -- la
        // clave puede estar ausente, ese es todo el punto de `y?: T`.
        let src = r#"
            type Narrow = { x: Int }
            type Wide = { x: Int, y?: String }
            fn takesWide(w: Wide) -> Int { w.x }
            fn pass(n: Narrow) -> Int { takesWide(n) }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn union_match_with_type_patterns_for_every_member_typechecks() {
        let src = r#"
            fn describe(v: Int | String) -> String {
                match v {
                    i: Int => "es un entero",
                    s: String => "es un string",
                }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn non_exhaustive_union_match_is_rejected() {
        let src = r#"
            fn describe(v: Int | String) -> String {
                match v {
                    i: Int => "es un entero",
                }
            }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn wildcard_covers_the_rest_of_a_union_match() {
        let src = r#"
            fn describe(v: Int | String) -> String {
                match v {
                    i: Int => "es un entero",
                    _ => "otra cosa",
                }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn matching_over_an_ambiguous_union_is_rejected() {
        // {x,y} y {x,z} no comparten ningún campo con tipos en conflicto
        // ('x' es Int en los dos) -- un tercer tipo más ancho {x,y,z},
        // construible por cualquier usuario vía subtipado estructural,
        // satisface los requisitos de los DOS a la vez. Un chequeo ingenuo
        // de is_subtype mutuo los aceptaría (ninguno es subtipo del otro)
        // -- exactamente el caso que el análisis real tiene que atrapar.
        let src = r#"
            type A = { x: Int, y: Int }
            type B = { x: Int, z: Int }
            fn describe(v: A | B) -> String {
                match v {
                    a: A => "es A",
                    b: B => "es B",
                }
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "una unión con miembros ambiguos no debería poder matchearse");
    }

    #[test]
    fn matching_over_a_distinguishable_union_is_accepted() {
        // x:Int vs x:String -- un valor real solo puede tener UNA forma
        // concreta en 'x' a la vez, así que sí son distinguibles de forma
        // confiable (a diferencia del caso ambiguo de arriba).
        let src = r#"
            type A = { x: Int }
            type B = { x: String }
            fn describe(v: A | B) -> String {
                match v {
                    a: A => "es A",
                    b: B => "es B",
                }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn union_of_two_optional_members_is_always_ambiguous() {
        let src = r#"
            type A = { x: Int }
            type B = { y: Int }
            fn describe(v: A? | B?) -> String {
                match v {
                    a: A? => "es A",
                    b: B? => "es B",
                }
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "dos miembros Optional siempre son ambiguos: null matchea a los dos");
    }

    #[test]
    fn union_of_two_list_members_is_always_ambiguous() {
        let src = r#"
            fn describe(v: Int[] | String[]) -> String {
                match v {
                    xs: Int[] => "ints",
                    ss: String[] => "strings",
                }
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "dos miembros List siempre son ambiguos: una lista vacía matchea a las dos");
    }

    #[test]
    fn guarded_union_arm_does_not_discharge_exhaustiveness() {
        let src = r#"
            fn describe(v: Int | String) -> String {
                match v {
                    i: Int if i > 0 => "positivo",
                    s: String => "string",
                }
            }
        "#;
        let result = check_source(src);
        assert!(
            result.is_err(),
            "un arm con guard no debería descartar exhaustividad -- falta cubrir Int sin guard"
        );
    }

    #[test]
    fn or_pattern_combining_two_type_patterns_is_rejected() {
        // Un patrón de tipo LIGA un nombre, igual que cualquier otro binding
        // -- prohibido dentro de un Or (mismo criterio que ya rechaza
        // bindings de enum/literal ahí).
        let src = r#"
            fn describe(v: Int | String) -> String {
                match v {
                    i: Int | s: String => "algo",
                }
            }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn type_pattern_against_an_enum_scrutinee_is_rejected() {
        let src = r#"
            enum Status { Active, Paused }
            fn describe(s: Status) -> String {
                match s {
                    x: Int => "no",
                    Status.Active => "activo",
                    Status.Paused => "pausado",
                }
            }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn union_typed_parameter_without_matching_still_works_even_if_members_would_be_ambiguous() {
        // {x,y}|{x,z} sería ambiguo si se intentara matchear -- pero
        // ACEPTAR-Y-PASAR (sin narrowing) no necesita distinguir nada, y no
        // debería verse afectado: el chequeo de ambigüedad corre solo
        // dentro de check_exhaustive_union (match), no en la resolución
        // general de un tipo unión.
        let src = r#"
            type A = { x: Int, y: Int }
            type B = { x: Int, z: Int }
            fn accept(v: A | B) -> A | B { v }
        "#;
        assert!(check_source(src).is_ok());
    }
}
