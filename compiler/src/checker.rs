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
            other => {
                return Err(err(format!(
                    "'match' requiere un valor de tipo enum, Int, String o Bool; se encontró {other:?}"
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
                        .map(|f| f.ty.clone())
                        .ok_or_else(|| err(format!("el struct no tiene campo '{field}'"))),
                    // struct genérico instanciado, ej. una variable Box<Int>
                    Type::Generic(name, args) => self
                        .expand_generic_struct(&name, &args)?
                        .into_iter()
                        .find(|f| &f.name == field)
                        .map(|f| f.ty)
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
            _ => None,
        };
        Ok(ty)
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
}
