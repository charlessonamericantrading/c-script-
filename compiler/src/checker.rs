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

type Env = HashMap<String, Type>;

pub struct Checker {
    pub(crate) types: HashMap<String, TypeDecl>,
    pub(crate) enums: HashMap<String, EnumDecl>,
    fns: HashMap<String, (Vec<Type>, Type)>,
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
        };
        let mut errors = Vec::new();

        for item in &program.items {
            match item {
                Item::Type(t) => {
                    checker.types.insert(t.name.clone(), t.clone());
                }
                Item::Enum(e) => {
                    checker.enums.insert(e.name.clone(), e.clone());
                }
                _ => {}
            }
        }

        for item in &program.items {
            if let Item::Fn(f) = item {
                match checker.resolve_fn_signature(f) {
                    Ok(sig) => {
                        checker.fns.insert(f.name.clone(), sig);
                    }
                    Err(e) => errors.push(e),
                }
            }
        }

        (checker, errors)
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
                        let rpc = match m {
                            Member::Rpc(r) | Member::Stream(r) => r,
                        };
                        if let Err(e) = checker.check_rpc(rpc) {
                            errors.push(e);
                        }
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

    pub(crate) fn resolve_type(&self, texpr: &TypeExpr) -> Result<Type, CheckError> {
        match texpr {
            TypeExpr::Named(name, args) => self.resolve_named_type(name, args),
            TypeExpr::Struct(fields) => {
                let mut ftys = Vec::new();
                for f in fields {
                    ftys.push(FieldType {
                        name: f.name.clone(),
                        optional: f.optional,
                        ty: self.resolve_type(&f.ty)?,
                    });
                }
                Ok(Type::Struct { name: None, fields: ftys })
            }
            TypeExpr::Optional(inner) => Ok(Type::Optional(Box::new(self.resolve_type(inner)?))),
            TypeExpr::List(inner) => Ok(Type::List(Box::new(self.resolve_type(inner)?))),
            TypeExpr::Tuple(items) => {
                let mut tys = Vec::new();
                for i in items {
                    tys.push(self.resolve_type(i)?);
                }
                Ok(Type::Tuple(tys))
            }
            TypeExpr::Function(params, ret) => {
                let mut ptys = Vec::new();
                for p in params {
                    ptys.push(self.resolve_type(p)?);
                }
                Ok(Type::Function(ptys, Box::new(self.resolve_type(ret)?)))
            }
            TypeExpr::Map(_, _) => Err(err(
                "tipo map { K: V } todavía no soportado por el checker (ambigüedad real con structs de un campo, GRAMMAR.md §2.2) — usa Map<K, V>",
            )),
            TypeExpr::Union(_) => Err(err(
                "uniones de tipo (A | B) todavía no soportadas por el checker — pendiente de una fase posterior",
            )),
        }
    }

    fn resolve_named_type(&self, name: &str, args: &[TypeExpr]) -> Result<Type, CheckError> {
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
                    Box::new(self.resolve_type(a)?),
                    Box::new(self.resolve_type(b)?),
                ))
            }
            "Patch" => {
                // Builtin (GRAMMAR.md §3.4). T debe resolver a un struct —
                // "parchear" un Int o un enum no tiene sentido en este diseño.
                let [inner] = args else {
                    return Err(err("Patch<T> requiere exactamente 1 argumento de tipo"));
                };
                match self.resolve_type(inner)? {
                    Type::Struct { .. } => Ok(Type::PatchOf(Box::new(self.resolve_type(inner)?))),
                    other => Err(err(format!(
                        "Patch<T> requiere que T sea un struct, se encontró {other:?}"
                    ))),
                }
            }
            _ => {
                if let Some(decl) = self.types.get(name) {
                    if !decl.type_params.is_empty() {
                        return Err(err(format!(
                            "'{name}' es genérico — instanciar type/enum declarados por el usuario aún no está soportado (PLAN.md §3.6)"
                        )));
                    }
                    let resolved = self.resolve_type(&decl.ty)?;
                    Ok(match resolved {
                        Type::Struct { fields, .. } => Type::Struct { name: Some(name.to_string()), fields },
                        other => other, // alias a un tipo no-struct, ej. `type Id = Int`
                    })
                } else if self.enums.contains_key(name) {
                    if !args.is_empty() {
                        return Err(err(format!(
                            "'{name}' no toma argumentos de tipo (genéricos en enums declarados por el usuario aún no soportados)"
                        )));
                    }
                    Ok(Type::Enum(name.to_string()))
                } else {
                    Err(err(format!("tipo desconocido: '{name}'")))
                }
            }
        }
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
            env.insert(p.name.clone(), self.resolve_type(&p.ty)?);
        }
        self.check_block(&f.body, &ret, &env)
    }

    fn check_rpc(&self, r: &RpcDecl) -> Result<(), CheckError> {
        let ret = self.resolve_type(&r.return_type)?;
        let mut env = Env::new();
        for p in &r.params {
            let pty = self.resolve_type(&p.ty)?;
            if let Some(default) = &p.default {
                self.check_expr(default, &pty, &Env::new())?;
            }
            env.insert(p.name.clone(), pty);
        }
        self.check_block(&r.body, &ret, &env)
    }

    // ---- bloques y sentencias ----

    fn check_block(&self, block: &Block, expected: &Type, env: &Env) -> Result<(), CheckError> {
        let mut local = env.clone();
        for stmt in &block.stmts {
            match stmt {
                Stmt::Let { name, ty, value, .. } => {
                    let value_ty = match ty {
                        Some(t) => {
                            let resolved = self.resolve_type(t)?;
                            self.check_expr(value, &resolved, &local)?;
                            resolved
                        }
                        None => self.synth_expr(value, &local)?,
                    };
                    local.insert(name.clone(), value_ty);
                }
                Stmt::Return(Some(e)) => self.check_expr(e, expected, &local)?,
                Stmt::Return(None) => {
                    if !is_subtype(&Type::Void, expected) {
                        return Err(err("'return' sin valor en una función que no devuelve Void"));
                    }
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

    fn check_match(
        &self,
        scrutinee: &Expr,
        arms: &[MatchArm],
        expected: &Type,
        env: &Env,
    ) -> Result<(), CheckError> {
        let scrutinee_ty = self.synth_expr(scrutinee, env)?;
        let enum_name = match &scrutinee_ty {
            Type::Enum(n) => n.clone(),
            Type::ResultOf(_, _) => "Result".to_string(),
            other => return Err(err(format!("'match' requiere un valor de tipo enum, se encontró {other:?}"))),
        };

        self.check_exhaustive(&scrutinee_ty, &enum_name, arms)?;

        for arm in arms {
            let mut arm_env = env.clone();
            self.bind_pattern(&arm.pattern, &scrutinee_ty, &mut arm_env)?;
            match &arm.body {
                MatchArmBody::Expr(e) => self.check_expr(e, expected, &arm_env)?,
                MatchArmBody::Block(b) => self.check_block(b, expected, &arm_env)?,
            }
        }
        Ok(())
    }

    /// Algoritmo de GRAMMAR.md §3.3: cualquier `Pattern::Bind` (incluye `_` y
    /// bindings con nombre, ej. `otro => ...`) es un catch-all irrefutable.
    fn check_exhaustive(&self, scrutinee_ty: &Type, enum_name: &str, arms: &[MatchArm]) -> Result<(), CheckError> {
        let variants: Vec<String> = if matches!(scrutinee_ty, Type::ResultOf(_, _)) {
            vec!["Ok".to_string(), "Err".to_string()]
        } else {
            self.enum_variant_names(enum_name)?
        };

        let mut covered = HashSet::new();
        let mut wildcard = false;
        for arm in arms {
            match &arm.pattern {
                Pattern::Bind(_) => wildcard = true,
                Pattern::Variant { enum_name: en, variant_name, .. } => {
                    if en != enum_name {
                        return Err(err(format!(
                            "patrón para el enum '{en}' no coincide con el tipo del escrutinio ('{enum_name}')"
                        )));
                    }
                    covered.insert(variant_name.clone());
                }
            }
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

    fn enum_variant_names(&self, name: &str) -> Result<Vec<String>, CheckError> {
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
                env.insert(name.clone(), ty.clone());
                Ok(())
            }
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
        }
    }

    fn variant_field_types(
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
                if name == "db" {
                    // Runtime builtin aún no modelado (ver Type::Dynamic).
                    return Ok(Type::Dynamic);
                }
                if let Some(t) = env.get(name) {
                    return Ok(t.clone());
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
                    other => Err(err(format!("no se puede acceder al campo '{field}' sobre {other:?}"))),
                }
            }
            Expr::Call { callee, args } => {
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
                self.resolve_named_type(name, &[])
            }
        }
    }

    fn check_fields_against(
        &self,
        decl_fields: &[Field],
        given: &[(String, Expr)],
        env: &Env,
    ) -> Result<(), CheckError> {
        for (fname, fexpr) in given {
            let decl_f = decl_fields
                .iter()
                .find(|f| &f.name == fname)
                .ok_or_else(|| err(format!("campo desconocido: '{fname}'")))?;
            let fty = self.resolve_type(&decl_f.ty)?;
            self.check_expr(fexpr, &fty, env)?;
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
    fn wrong_argument_count_is_rejected() {
        let src = r#"
            fn add(a: Int, b: Int) -> Int { a }
            fn use_it() -> Int { add(1) }
        "#;
        let result = check_source(src);
        assert!(result.is_err());
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
}
