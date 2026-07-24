// Runtime mínimo interpretado (PLAN.md §2.4, Fase 0): un tree-walking
// interpreter que ejecuta cuerpos de rpc/fn contra un "db" en memoria.
// No es el runtime final del lenguaje — Fase 1+ compila a WASM/nativo
// (PLAN.md §4) — esto solo alcanza para que la demo E2E responda de verdad.

pub mod db;
pub mod server;

use crate::ast::*;
use db::Db;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// `PartialEq`/`Debug` NO se derivan (ver los `impl` a mano más abajo) --
/// `Value::Closure` guarda el `Env` que capturó, y un closure recursivo
/// armado reasignando un `mut` (`let mut f = |x|{x}; f = |x|{ ... f(x-1)
/// ... };`) captura un `Env` que termina conteniendo una referencia a sí
/// mismo (`Rc` cíclico, GRAMMAR.md §3.10) -- derivar estos dos impls
/// recursaría para siempre el día que algo compare o debug-imprima ese
/// valor. El checker ya rechaza `==`/`!=` sobre tipos función de entrada
/// (checker.rs, `type_contains_function`), pero esto es la defensa en
/// runtime para cualquier OTRO código (mensajes de error, tests) que
/// pueda comparar/imprimir un `Value` arbitrario sin saber que puede ser
/// autorreferencial.
#[derive(Clone)]
pub enum Value {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Null,
    Struct(Vec<(String, Value)>),
    /// `enum_name` (ej. "Role") además de `variant` (ej. "Member") -- hace
    /// falta para que `value_to_json` sepa si ESTE enum es "simple" (todo
    /// unit, serializa como string plano, ej. Role) o un ADT (serializa
    /// como objeto con tag `type`, ej. ValidationError) -- esa distinción
    /// es de la DECLARACIÓN completa (GRAMMAR.md §4, emit_enum_decl en
    /// ts_emit.rs), no algo que se pueda inferir de un solo Value::Variant
    /// suelto (un ADT puede tener variantes propias sin campos).
    Variant { enum_name: String, variant: String, fields: Vec<(String, Value)> },
    List(Vec<Value>),
    Tuple(Vec<Value>),
    /// Marcadores internos — nunca deberían llegar a `value_to_json` (ver la
    /// salvaguarda ahí). Representan `db`, `db.coleccion`, y un método ligado
    /// (`recv.metodo`) a la espera de ser invocado, ej. `db.users.find`.
    Db,
    DbCollection(String),
    BoundMethod(Box<Value>, String),
    /// Una `fn` de nivel superior referenciada POR NOMBRE, ej. `let g = add_one;`
    /// (GRAMMAR.md §3.10). Es una REFERENCIA a función (como un `fn` pointer
    /// de Rust), no un closure -- no captura ninguna variable, porque una
    /// `fn` de nivel superior no tiene scope léxico exterior que capturar.
    /// Nunca cruza el wire, igual que `Type::Function` (tabla de mapeo, §4).
    FnRef(String),
    /// `|params| { body }` evaluado -- a diferencia de `FnRef`, SÍ tiene
    /// captura léxica real: `Env` es el entorno en el momento en que el
    /// closure se construyó (GRAMMAR.md §3.10). Nunca cruza el wire, igual
    /// que `FnRef` (`value_to_json` lo trata como marcador interno).
    Closure(Vec<String>, Block, Env),
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        use Value::*;
        match (self, other) {
            (Int(a), Int(b)) => a == b,
            (Float(a), Float(b)) => a == b,
            (Str(a), Str(b)) => a == b,
            (Bool(a), Bool(b)) => a == b,
            (Null, Null) => true,
            (Struct(a), Struct(b)) => a == b,
            (
                Variant { enum_name: en1, variant: v1, fields: f1 },
                Variant { enum_name: en2, variant: v2, fields: f2 },
            ) => en1 == en2 && v1 == v2 && f1 == f2,
            (List(a), List(b)) => a == b,
            (Tuple(a), Tuple(b)) => a == b,
            (Db, Db) => true,
            (DbCollection(a), DbCollection(b)) => a == b,
            (BoundMethod(a, m1), BoundMethod(b, m2)) => a == b && m1 == m2,
            (FnRef(a), FnRef(b)) => a == b,
            // Nunca iguales, ni siquiera el mismo closure consigo mismo --
            // comparar closures no tiene un significado útil (el checker ya
            // lo rechaza de entrada), y esto es lo que evita recursar dentro
            // de `captured_env`.
            (Closure(..), Closure(..)) => false,
            _ => false,
        }
    }
}

impl std::fmt::Debug for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Int(n) => f.debug_tuple("Int").field(n).finish(),
            Value::Float(n) => f.debug_tuple("Float").field(n).finish(),
            Value::Str(s) => f.debug_tuple("Str").field(s).finish(),
            Value::Bool(b) => f.debug_tuple("Bool").field(b).finish(),
            Value::Null => write!(f, "Null"),
            Value::Struct(fields) => f.debug_tuple("Struct").field(fields).finish(),
            Value::Variant { enum_name, variant, fields } => f
                .debug_struct("Variant")
                .field("enum_name", enum_name)
                .field("variant", variant)
                .field("fields", fields)
                .finish(),
            Value::List(items) => f.debug_tuple("List").field(items).finish(),
            Value::Tuple(items) => f.debug_tuple("Tuple").field(items).finish(),
            Value::Db => write!(f, "Db"),
            Value::DbCollection(name) => f.debug_tuple("DbCollection").field(name).finish(),
            Value::BoundMethod(recv, method) => f.debug_tuple("BoundMethod").field(recv).field(method).finish(),
            Value::FnRef(name) => f.debug_tuple("FnRef").field(name).finish(),
            // A propósito NO imprime `captured_env` -- podría ser cíclico
            // (ver el comentario en el enum), y de todos modos un entorno
            // capturado entero no aporta nada legible a un mensaje de error.
            Value::Closure(params, ..) => write!(f, "Closure({params:?}, <cuerpo y entorno omitidos>)"),
        }
    }
}

#[derive(Debug)]
pub struct RuntimeError(pub String);

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "error en runtime: {}", self.0)
    }
}

fn err(msg: impl Into<String>) -> RuntimeError {
    RuntimeError(msg.into())
}

/// Cada variable es su propia celda compartida, no un `Value` directo. Es lo
/// que hace que la mutación (`x = ...`, GRAMMAR.md §2.3) atraviese bloques
/// anidados de verdad: `eval_block` clona el mapa `Env` al entrar a un bloque
/// (para que un `let` adentro no se filtre afuera), pero clonar un `HashMap`
/// de `Rc<RefCell<_>>` copia los punteros, no el contenido -- así que
/// `x = 2` dentro de un `if` muta la MISMA celda que ve el scope exterior.
/// Un `let` (mut o no) siempre crea una celda nueva -- así se sombrea
/// correctamente una variable exterior con el mismo nombre en vez de mutarla.
type Env = HashMap<String, Rc<RefCell<Value>>>;
type Fns<'a> = HashMap<String, &'a FnDecl>;
type Types<'a> = HashMap<String, &'a TypeDecl>;
type Enums<'a> = HashMap<String, &'a EnumDecl>;

/// Las dos tablas de símbolos que el narrowing de uniones necesita en
/// runtime (GRAMMAR.md §3.9) -- `value_matches_type` tiene que poder
/// resolver un `TypeExpr::Named("User", [])` de un patrón `nombre: Tipo` a
/// sus campos reales, y hasta esta ronda nada en este módulo conocía
/// ningún `type`/`enum` declarado (`db`/`fns` eran las únicas tablas que
/// ya existían). Empaquetadas juntas -- un solo parámetro nuevo enhebrado,
/// en vez de duplicar el enhebrado de dos tablas sueltas en cada función
/// que ya pasa `db`/`fns` explícitamente.
pub(crate) struct Symbols<'a> {
    types: Types<'a>,
    enums: Enums<'a>,
}

fn cell(v: Value) -> Rc<RefCell<Value>> {
    Rc::new(RefCell::new(v))
}

pub(crate) fn eval_block(block: &Block, env: &Env, db: &Db, fns: &Fns, symbols: &Symbols) -> Result<Value, RuntimeError> {
    let mut local = env.clone();
    for stmt in &block.stmts {
        match stmt {
            Stmt::Let { name, value, .. } => {
                let v = eval_expr(value, &local, db, fns, symbols)?;
                local.insert(name.clone(), cell(v));
            }
            Stmt::Assign { name, value } => {
                let v = eval_expr(value, &local, db, fns, symbols)?;
                let target = local
                    .get(name)
                    .ok_or_else(|| err(format!("variable no declarada en runtime: '{name}'")))?;
                *target.borrow_mut() = v;
            }
            Stmt::Return(Some(e)) => return eval_expr(e, &local, db, fns, symbols),
            Stmt::Return(None) => return Ok(Value::Null),
            Stmt::Expr(e) => {
                eval_expr(e, &local, db, fns, symbols)?;
            }
        }
    }
    match &block.tail {
        Some(e) => eval_expr(e, &local, db, fns, symbols),
        None => Ok(Value::Null),
    }
}

pub(crate) fn eval_expr(e: &Expr, env: &Env, db: &Db, fns: &Fns, symbols: &Symbols) -> Result<Value, RuntimeError> {
    match e {
        Expr::Int(n) => Ok(Value::Int(*n)),
        Expr::Float(n) => Ok(Value::Float(*n)),
        Expr::Str(s) => Ok(Value::Str(s.clone())),
        Expr::Bool(b) => Ok(Value::Bool(*b)),
        Expr::Null => Ok(Value::Null),
        Expr::Paren(inner) => eval_expr(inner, env, db, fns, symbols),
        Expr::Ident(name) => {
            if name == "db" {
                return Ok(Value::Db);
            }
            if let Some(c) = env.get(name) {
                return Ok(c.borrow().clone());
            }
            // No es una variable local -- si es una `fn` de nivel superior,
            // referenciarla por nombre produce un FnRef (ver su doc en el
            // enum Value), no un error: el checker ya la trata como un valor
            // de tipo Function en este mismo caso (checker.rs, synth_expr).
            if fns.contains_key(name.as_str()) {
                return Ok(Value::FnRef(name.clone()));
            }
            Err(err(format!("variable no declarada en runtime: '{name}'")))
        }
        Expr::FieldAccess { base, field } => {
            let base_v = eval_expr(base, env, db, fns, symbols)?;
            match base_v {
                Value::Struct(fields) | Value::Variant { fields, .. } => fields
                    .into_iter()
                    .find(|(n, _)| n == field)
                    .map(|(_, v)| v)
                    .ok_or_else(|| err(format!("no existe el campo '{field}'"))),
                Value::Db => Ok(Value::DbCollection(field.clone())),
                // Métodos builtin sobre primitivos (GRAMMAR.md §3.8, ej.
                // `x.toFloat()`) usan el mismo BoundMethod que db/listas --
                // el checker ya validó que el nombre existe para este tipo.
                Value::DbCollection(_) | Value::List(_) | Value::Int(_) | Value::Float(_) | Value::Str(_) => {
                    Ok(Value::BoundMethod(Box::new(base_v), field.clone()))
                }
                other => Err(err(format!("no se puede acceder al campo '{field}' sobre {other:?}"))),
            }
        }
        Expr::Call { callee, args } => {
            // Llamada directa a una `fn` de usuario por nombre -- atajo
            // frecuente que evita pasar por FnRef (ver Expr::Ident arriba)
            // solo para volver a buscar el mismo nombre en `fns`.
            if let Expr::Ident(name) = &**callee {
                if let Some(decl) = fns.get(name.as_str()) {
                    let arg_vs = eval_args(args, env, db, fns, symbols)?;
                    return call_fn_decl(decl, arg_vs, db, fns, symbols);
                }
            }
            let callee_v = eval_expr(callee, env, db, fns, symbols)?;
            let arg_vs = eval_args(args, env, db, fns, symbols)?;
            match callee_v {
                Value::BoundMethod(receiver, method) => call_method(*receiver, &method, arg_vs, db, fns, symbols),
                // Llamada INDIRECTA: `callee` fue una variable/parámetro que
                // contenía una referencia a función o un closure (GRAMMAR.md
                // §3.10), no el nombre escrito ahí mismo -- ej. dentro de
                // `apply_twice(f, x) { f(f(x)) }`, `f` llega como FnRef;
                // `list.filter(f)` con `f` un closure ya evaluado, como
                // Closure. `call_callable` despacha ambos casos (y produce
                // el mismo error de "no se puede llamar" para cualquier otra
                // cosa, ver su propio fallback).
                other => call_callable(other, arg_vs, db, fns, symbols),
            }
        }
        Expr::StructLit { name, variant, fields } => {
            let evaluated = fields
                .iter()
                .map(|(k, e)| Ok((k.clone(), eval_expr(e, env, db, fns, symbols)?)))
                .collect::<Result<Vec<_>, RuntimeError>>()?;
            match variant {
                Some(v) => {
                    Ok(Value::Variant { enum_name: name.clone(), variant: v.clone(), fields: evaluated })
                }
                None => Ok(Value::Struct(evaluated)),
            }
        }
        Expr::Match { scrutinee, arms } => {
            let v = eval_expr(scrutinee, env, db, fns, symbols)?;
            for arm in arms {
                if let Some(bindings) = try_match_pattern(&arm.pattern, &v, symbols) {
                    let mut arm_env = env.clone();
                    arm_env.extend(bindings.into_iter().map(|(k, v)| (k, cell(v))));
                    if let Some(guard) = &arm.guard {
                        match eval_expr(guard, &arm_env, db, fns, symbols)? {
                            Value::Bool(true) => {}
                            // El patrón matcheó pero el guard no se cumplió --
                            // se sigue probando el resto de los arms, no se
                            // considera "sin match" (GRAMMAR.md §3.3).
                            Value::Bool(false) => continue,
                            other => return Err(err(format!("el guard de 'match' no es Bool en runtime: {other:?}"))),
                        }
                    }
                    return match &arm.body {
                        MatchArmBody::Expr(e) => eval_expr(e, &arm_env, db, fns, symbols),
                        MatchArmBody::Block(b) => eval_block(b, &arm_env, db, fns, symbols),
                    };
                }
            }
            // No debería pasar: el checker ya garantizó exhaustividad
            // (GRAMMAR.md §3.3) antes de que este código llegara a ejecutarse.
            Err(err("ningún arm de match coincidió — el checker debería haber impedido esto"))
        }
        Expr::If { cond, then_block, else_block } => {
            let c = eval_expr(cond, env, db, fns, symbols)?;
            match c {
                Value::Bool(true) => eval_block(then_block, env, db, fns, symbols),
                Value::Bool(false) => eval_block(else_block, env, db, fns, symbols),
                other => Err(err(format!("la condición de 'if' no es Bool en runtime: {other:?}"))),
            }
        }
        Expr::Binary { op, left, right } => eval_binary(*op, left, right, env, db, fns, symbols),
        Expr::Unary { op, operand } => eval_unary(*op, operand, env, db, fns, symbols),
        Expr::ArrayLit(items) => {
            let vs = items
                .iter()
                .map(|e| eval_expr(e, env, db, fns, symbols))
                .collect::<Result<Vec<_>, RuntimeError>>()?;
            Ok(Value::List(vs))
        }
        Expr::Index { base, index } => {
            let base_v = eval_expr(base, env, db, fns, symbols)?;
            let idx = as_int(&eval_expr(index, env, db, fns, symbols)?)?;
            match base_v {
                Value::List(items) => {
                    let i: usize = idx
                        .try_into()
                        .map_err(|_| err(format!("índice negativo: {idx}")))?;
                    if i >= items.len() {
                        return Err(err(format!("índice {idx} fuera de rango (largo {})", items.len())));
                    }
                    Ok(items[i].clone())
                }
                other => Err(err(format!("no se puede indexar un valor {other:?}"))),
            }
        }
        Expr::TupleLit(items) => {
            let vs = items
                .iter()
                .map(|e| eval_expr(e, env, db, fns, symbols))
                .collect::<Result<Vec<_>, RuntimeError>>()?;
            Ok(Value::Tuple(vs))
        }
        Expr::TupleIndex { base, index } => {
            let base_v = eval_expr(base, env, db, fns, symbols)?;
            match base_v {
                Value::Tuple(items) => items
                    .get(*index)
                    .cloned()
                    .ok_or_else(|| err(format!("índice de tupla .{index} fuera de rango"))),
                other => Err(err(format!("'.{index}' requiere una tupla, se encontró {other:?}"))),
            }
        }
        // Captura el env ACTUAL -- esa es la captura léxica real (GRAMMAR.md
        // §3.10). `env.clone()` es barato: clona el HashMap pero solo hace
        // bump de refcount en cada `Rc<RefCell<Value>>`, no clona las celdas
        // (mismo mecanismo que ya usa `eval_block` para bloques anidados).
        Expr::Closure { params, body } => {
            let param_names = params.iter().map(|p| p.name.clone()).collect();
            Ok(Value::Closure(param_names, body.clone(), env.clone()))
        }
    }
}

fn eval_binary(
    op: BinaryOp,
    left: &Expr,
    right: &Expr,
    env: &Env,
    db: &Db,
    fns: &Fns,
    symbols: &Symbols,
) -> Result<Value, RuntimeError> {
    use BinaryOp::*;
    // && / || cortocircuitan: el lado derecho no se evalúa si ya se sabe el
    // resultado, igual que en cualquier lenguaje con estos operadores.
    if matches!(op, And | Or) {
        let l = as_bool(&eval_expr(left, env, db, fns, symbols)?)?;
        return match (op, l) {
            (And, false) => Ok(Value::Bool(false)),
            (Or, true) => Ok(Value::Bool(true)),
            _ => Ok(Value::Bool(as_bool(&eval_expr(right, env, db, fns, symbols)?)?)),
        };
    }

    let l = eval_expr(left, env, db, fns, symbols)?;
    let r = eval_expr(right, env, db, fns, symbols)?;
    match op {
        // '+' concatena si ambos lados son String (checker.rs ya garantizó
        // que no llega acá un String mezclado con Int/Float).
        Add => match (l, r) {
            (Value::Str(a), Value::Str(b)) => Ok(Value::Str(a + &b)),
            (l, r) => numeric_op(l, r, |a, b| a + b, |a, b| a + b),
        },
        Sub => numeric_op(l, r, |a, b| a - b, |a, b| a - b),
        Mul => numeric_op(l, r, |a, b| a * b, |a, b| a * b),
        Div => numeric_op(l, r, |a, b| a / b, |a, b| a / b),
        Rem => numeric_op(l, r, |a, b| a % b, |a, b| a % b),
        Eq => Ok(Value::Bool(l == r)),
        NotEq => Ok(Value::Bool(l != r)),
        Lt => compare(l, r, |o| o == std::cmp::Ordering::Less),
        LtEq => compare(l, r, |o| o != std::cmp::Ordering::Greater),
        Gt => compare(l, r, |o| o == std::cmp::Ordering::Greater),
        GtEq => compare(l, r, |o| o != std::cmp::Ordering::Less),
        And | Or => unreachable!("manejado arriba con cortocircuito"),
    }
}

fn eval_unary(
    op: UnaryOp,
    operand: &Expr,
    env: &Env,
    db: &Db,
    fns: &Fns,
    symbols: &Symbols,
) -> Result<Value, RuntimeError> {
    let v = eval_expr(operand, env, db, fns, symbols)?;
    match op {
        UnaryOp::Neg => match v {
            Value::Int(n) => Ok(Value::Int(-n)),
            Value::Float(n) => Ok(Value::Float(-n)),
            other => Err(err(format!("'-' unario requiere Int o Float en runtime: {other:?}"))),
        },
        UnaryOp::Not => Ok(Value::Bool(!as_bool(&v)?)),
    }
}

fn as_bool(v: &Value) -> Result<bool, RuntimeError> {
    match v {
        Value::Bool(b) => Ok(*b),
        other => Err(err(format!("se esperaba Bool en runtime, se encontró {other:?}"))),
    }
}

fn numeric_op(
    l: Value,
    r: Value,
    int_op: impl Fn(i64, i64) -> i64,
    float_op: impl Fn(f64, f64) -> f64,
) -> Result<Value, RuntimeError> {
    match (l, r) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(int_op(a, b))),
        (Value::Float(a), Value::Float(b)) => Ok(Value::Float(float_op(a, b))),
        (l, r) => Err(err(format!(
            "operador aritmético requiere Int+Int o Float+Float en runtime: {l:?} y {r:?}"
        ))),
    }
}

fn compare(l: Value, r: Value, accept: impl Fn(std::cmp::Ordering) -> bool) -> Result<Value, RuntimeError> {
    let ordering = match (&l, &r) {
        (Value::Int(a), Value::Int(b)) => a.cmp(b),
        (Value::Float(a), Value::Float(b)) => {
            a.partial_cmp(b).ok_or_else(|| err("comparación con NaN"))?
        }
        _ => return Err(err(format!("operador relacional requiere Int+Int o Float+Float: {l:?} y {r:?}"))),
    };
    Ok(Value::Bool(accept(ordering)))
}

fn eval_args(args: &[Expr], env: &Env, db: &Db, fns: &Fns, symbols: &Symbols) -> Result<Vec<Value>, RuntimeError> {
    args.iter().map(|a| eval_expr(a, env, db, fns, symbols)).collect()
}

/// Invoca una `fn` de usuario ya resuelta con argumentos ya evaluados --
/// compartido por la llamada directa (`f(x)`) y la indirecta a través de un
/// `Value::FnRef` (`let g = f; g(x)`), para no duplicar el armado del scope.
fn call_fn_decl(decl: &FnDecl, arg_vs: Vec<Value>, db: &Db, fns: &Fns, symbols: &Symbols) -> Result<Value, RuntimeError> {
    let mut fn_env = Env::new();
    for (p, v) in decl.params.iter().zip(arg_vs) {
        fn_env.insert(p.name.clone(), cell(v));
    }
    eval_block(&decl.body, &fn_env, db, fns, symbols)
}

fn try_match_pattern(pattern: &Pattern, v: &Value, symbols: &Symbols) -> Option<Vec<(String, Value)>> {
    match pattern {
        Pattern::Bind(name) => Some(vec![(name.clone(), v.clone())]),
        Pattern::Literal(lit) => literal_matches(lit, v).then(Vec::new),
        Pattern::Variant { variant_name, fields, .. } => {
            let Value::Variant { variant, fields: value_fields, .. } = v else {
                return None;
            };
            if variant != variant_name {
                return None;
            }
            let mut bindings = Vec::new();
            if let Some(field_patterns) = fields {
                for fp in field_patterns {
                    let field_v = value_fields.iter().find(|(n, _)| n == &fp.name).map(|(_, v)| v)?;
                    bindings.extend(try_match_pattern(&fp.pattern, field_v, symbols)?);
                }
            }
            Some(bindings)
        }
        // Ninguna alternativa liga nada (el checker ya lo garantizó, ver
        // bind_pattern), así que probar cada una hasta la primera que
        // matchee y devolver sus bindings (vacíos) alcanza.
        Pattern::Or(subs) => subs.iter().find_map(|p| try_match_pattern(p, v, symbols)),
        // Narrowing de uniones (GRAMMAR.md §3.9): resuelve el texpr del
        // patrón a un `Type` (con los mismos `types`/`enums` que el checker
        // ya validó que existen -- `check_exhaustive_union` corrió antes
        // que esto) y chequea el SHAPE real del valor contra él.
        Pattern::Type(name, texpr) => {
            let resolved = resolve_pattern_type(texpr, &symbols.types, &symbols.enums);
            value_matches_type(v, &resolved).then(|| vec![(name.clone(), v.clone())])
        }
    }
}

fn literal_matches(lit: &LiteralPattern, v: &Value) -> bool {
    match (lit, v) {
        (LiteralPattern::Int(a), Value::Int(b)) => a == b,
        (LiteralPattern::Str(a), Value::Str(b)) => a == b,
        (LiteralPattern::Bool(a), Value::Bool(b)) => a == b,
        _ => false,
    }
}

/// Resuelve el `TypeExpr` de un patrón `nombre: Tipo` (GRAMMAR.md §3.9) al
/// `Type` (types.rs -- se reusa tal cual, no se inventa un enum paralelo)
/// que `value_matches_type` necesita. Deliberadamente más simple que
/// `Checker::resolve_type` (checker.rs): no maneja genéricos ni
/// type_params -- el checker YA validó (`check_exhaustive_union`) que este
/// texpr corresponde a un miembro concreto real de la unión antes de que
/// la ejecución llegue acá, así que alcanza con una resolución de struct/
/// enum/primitivo/Optional/List de un solo nivel, recursiva solo en eso.
fn resolve_pattern_type(texpr: &TypeExpr, types: &Types, enums: &Enums) -> crate::types::Type {
    use crate::types::{FieldType, Type};
    match texpr {
        TypeExpr::Named(name, _) => match name.as_str() {
            "Int" => Type::Int,
            "Float" => Type::Float,
            "String" => Type::String,
            "Bool" => Type::Bool,
            "Void" => Type::Void,
            _ => {
                if let Some(decl) = types.get(name.as_str()) {
                    match &decl.ty {
                        TypeExpr::Struct(fields) => Type::Struct {
                            name: Some(name.clone()),
                            fields: fields
                                .iter()
                                .map(|f| FieldType {
                                    name: f.name.clone(),
                                    optional: f.optional,
                                    ty: resolve_pattern_type(&f.ty, types, enums),
                                })
                                .collect(),
                        },
                        // Alias a un tipo no-struct, ej. `type Id = Int`.
                        other => resolve_pattern_type(other, types, enums),
                    }
                } else if enums.contains_key(name.as_str()) {
                    Type::Enum(name.clone())
                } else {
                    // No debería pasar -- el checker ya validó este texpr
                    // como miembro real de la unión antes de llegar acá.
                    Type::Dynamic
                }
            }
        },
        TypeExpr::Optional(inner) => Type::Optional(Box::new(resolve_pattern_type(inner, types, enums))),
        TypeExpr::List(inner) => Type::List(Box::new(resolve_pattern_type(inner, types, enums))),
        // Tuple/Function/struct-anónimo/Map/Union anidados como miembro
        // DIRECTO de un patrón de narrowing -- fuera de alcance v0 (no es
        // el caso de uso que esta ronda apunta a cubrir); `Dynamic` hace
        // que `value_matches_type` los trate de forma segura (nunca
        // matchea nada por accidente).
        _ => Type::Dynamic,
    }
}

/// ¿El SHAPE real de `v` corresponde a `ty`? Superficial pero recursivo en
/// los campos REQUERIDOS de un struct (no solo su presencia, también que el
/// valor guardado ahí tenga a su vez el shape correcto) -- eso es lo que
/// hace sound al análisis de ambigüedad del checker (`check_exhaustive_union`,
/// checker.rs): dos miembros de una unión con un campo compartido de tipos
/// mutuamente excluyentes (ej. `x: Int` vs `x: String`) se distinguen por el
/// tipo REAL del valor guardado en ese campo, no por su mera presencia (que
/// el subtipado estructural de ancho podría satisfacer para ambos a la vez
/// con un tercer tipo más ancho).
fn value_matches_type(v: &Value, ty: &crate::types::Type) -> bool {
    use crate::types::Type;
    match ty {
        Type::Int => matches!(v, Value::Int(_)),
        Type::Float => matches!(v, Value::Float(_)),
        Type::String => matches!(v, Value::Str(_)),
        Type::Bool => matches!(v, Value::Bool(_)),
        Type::Optional(inner) => matches!(v, Value::Null) || value_matches_type(v, inner),
        Type::List(_) => matches!(v, Value::List(_)),
        Type::Enum(name) => matches!(v, Value::Variant { enum_name, .. } if enum_name == name),
        Type::Struct { fields, .. } => match v {
            Value::Struct(vfields) => fields.iter().filter(|f| !f.optional).all(|f| {
                vfields
                    .iter()
                    .find(|(n, _)| n == &f.name)
                    .is_some_and(|(_, fv)| value_matches_type(fv, &f.ty))
            }),
            _ => false,
        },
        // Dynamic/Function/etc -- no debería llegar acá como resultado de
        // `resolve_pattern_type` para un texpr que el checker ya validó.
        _ => false,
    }
}

/// Invoca un closure YA evaluado -- a diferencia de `call_fn_decl` (que
/// arranca de un `Env::new()` vacío, porque una `fn` de nivel superior no
/// tiene scope que capturar), acá el scope de la llamada arranca del
/// `captured_env` que el closure guardó al construirse (GRAMMAR.md §3.10)
/// -- ESA es la captura léxica real. Los parámetros se ligan encima,
/// sombreando cualquier variable capturada con el mismo nombre.
fn call_closure(
    param_names: &[String],
    body: &Block,
    captured_env: &Env,
    arg_vs: Vec<Value>,
    db: &Db,
    fns: &Fns,
    symbols: &Symbols,
) -> Result<Value, RuntimeError> {
    let mut call_env = captured_env.clone();
    for (name, v) in param_names.iter().zip(arg_vs) {
        call_env.insert(name.clone(), cell(v));
    }
    eval_block(body, &call_env, db, fns, symbols)
}

/// Cualquier `Value` invocable -- una referencia a `fn` por nombre o un
/// closure -- con argumentos ya evaluados. Compartido por la llamada
/// indirecta de `Expr::Call` y por `.map`/`.filter` (más abajo), que
/// necesitan invocar su callback sin que les importe cuál de las dos formas
/// sea.
fn call_callable(v: Value, arg_vs: Vec<Value>, db: &Db, fns: &Fns, symbols: &Symbols) -> Result<Value, RuntimeError> {
    match v {
        Value::FnRef(name) => {
            let decl = fns
                .get(name.as_str())
                .ok_or_else(|| err(format!("fn desconocida: '{name}'")))?;
            call_fn_decl(decl, arg_vs, db, fns, symbols)
        }
        Value::Closure(params, body, captured_env) => {
            call_closure(&params, &body, &captured_env, arg_vs, db, fns, symbols)
        }
        other => Err(err(format!("no se puede llamar un valor {other:?}"))),
    }
}

fn call_method(
    receiver: Value,
    method: &str,
    args: Vec<Value>,
    db: &Db,
    fns: &Fns,
    symbols: &Symbols,
) -> Result<Value, RuntimeError> {
    match receiver {
        Value::DbCollection(coll) => db.call(&coll, method, args),
        Value::List(items) => match method {
            "take" => {
                let n = as_int(args.first().ok_or_else(|| err("take requiere 1 argumento"))?)? as usize;
                Ok(Value::List(items.into_iter().take(n).collect()))
            }
            "filter" => {
                let f = args.into_iter().next().ok_or_else(|| err("'filter' requiere 1 argumento"))?;
                let mut kept = Vec::new();
                for item in items {
                    if as_bool(&call_callable(f.clone(), vec![item.clone()], db, fns, symbols)?)? {
                        kept.push(item);
                    }
                }
                Ok(Value::List(kept))
            }
            "map" => {
                let f = args.into_iter().next().ok_or_else(|| err("'map' requiere 1 argumento"))?;
                let mut mapped = Vec::with_capacity(items.len());
                for item in items {
                    mapped.push(call_callable(f.clone(), vec![item], db, fns, symbols)?);
                }
                Ok(Value::List(mapped))
            }
            other => Err(err(format!("método de lista desconocido: '{other}'"))),
        },
        Value::Int(n) => match method {
            "toFloat" => Ok(Value::Float(n as f64)),
            other => Err(err(format!("método desconocido sobre Int: '{other}'"))),
        },
        Value::Float(n) => match method {
            "toInt" => Ok(Value::Int(n as i64)), // trunca hacia cero, no redondea (GRAMMAR.md §3.8)
            other => Err(err(format!("método desconocido sobre Float: '{other}'"))),
        },
        Value::Str(s) => match method {
            // chars().count(), no .len(): .len() cuenta bytes UTF-8, no
            // caracteres -- "é" son 2 bytes pero 1 carácter.
            "length" => Ok(Value::Int(s.chars().count() as i64)),
            "contains" => {
                let needle = match args.first() {
                    Some(Value::Str(n)) => n,
                    _ => return Err(err("'contains' requiere un argumento String")),
                };
                Ok(Value::Bool(s.contains(needle.as_str())))
            }
            other => Err(err(format!("método desconocido sobre String: '{other}'"))),
        },
        other => Err(err(format!("no se puede invocar '{method}' sobre {other:?}"))),
    }
}

pub(crate) fn as_int(v: &Value) -> Result<i64, RuntimeError> {
    match v {
        Value::Int(n) => Ok(*n),
        other => Err(err(format!("se esperaba un entero, se encontró {other:?}"))),
    }
}

/// Punto de entrada: ejecuta `{service_name}.{rpc_name}` con argumentos JSON
/// (el mismo shape que emite client.ts: `{ paramName: valor, ... }`).
pub fn invoke_rpc(
    program: &Program,
    service_name: &str,
    rpc_name: &str,
    args_json: &serde_json::Value,
    db: &Db,
) -> Result<serde_json::Value, RuntimeError> {
    let service = program
        .items
        .iter()
        .find_map(|i| match i {
            Item::Service(s) if s.name == service_name => Some(s),
            _ => None,
        })
        .ok_or_else(|| err(format!("service desconocido: '{service_name}'")))?;

    let rpc = service
        .members
        .iter()
        .find_map(|m| match m {
            Member::Rpc(r) | Member::Stream(r) if r.name == rpc_name => Some(r),
            _ => None,
        })
        .ok_or_else(|| err(format!("rpc desconocido: '{service_name}.{rpc_name}'")))?;

    let fns: Fns = program
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Fn(f) => Some((f.name.clone(), f)),
            _ => None,
        })
        .collect();

    // Narrowing de uniones (GRAMMAR.md §3.9): `value_matches_type` necesita
    // poder resolver un `TypeExpr::Named("User", [])` de un patrón `nombre:
    // Tipo` a sus campos reales -- mismo patrón que `fns` de arriba (y que
    // `simple_enum_names` más abajo), armado UNA vez acá, no en cada
    // llamada a `try_match_pattern`.
    let types: Types = program
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Type(t) => Some((t.name.clone(), t)),
            _ => None,
        })
        .collect();
    let enums: Enums = program
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Enum(e) => Some((e.name.clone(), e)),
            _ => None,
        })
        .collect();
    let symbols = Symbols { types, enums };

    let empty = serde_json::Map::new();
    let args_obj = args_json.as_object().unwrap_or(&empty);
    let mut env = Env::new();
    for p in &rpc.params {
        let v = match args_obj.get(&p.name) {
            Some(j) => json_to_value(j),
            None => match &p.default {
                Some(default_expr) => eval_expr(default_expr, &Env::new(), db, &fns, &symbols)?,
                None => Value::Null,
            },
        };
        env.insert(p.name.clone(), cell(v));
    }

    let result = eval_block(&rpc.body, &env, db, &fns, &symbols)?;
    let simple_enums = simple_enum_names(program);
    Ok(value_to_json(&result, &simple_enums))
}

/// Si `service_name.rpc_name` es un `stream` (no un `rpc` normal). Deliberadamente
/// una función APARTE en vez de cambiar la firma de retorno de `invoke_rpc` --
/// invoke_rpc ya hace la MISMA búsqueda y evalúa igual para ambos casos
/// (Member::Rpc(r) | Member::Stream(r), arriba), y su resultado (un
/// serde_json::Value) es idéntico en forma para quien lo llama; lo único que
/// server.rs necesita ANTES de invocar es "¿tengo que armar la respuesta como
/// un único JSON o como una secuencia de eventos SSE?" -- eso se resuelve acá,
/// sin forzar a los ~30 call sites de test existentes (todos `.unwrap()` un
/// solo Value) a desestructurar una tupla que no les interesa.
pub fn is_stream_member(program: &Program, service_name: &str, rpc_name: &str) -> bool {
    program.items.iter().any(|i| match i {
        Item::Service(s) if s.name == service_name => s
            .members
            .iter()
            .any(|m| matches!(m, Member::Stream(r) if r.name == rpc_name)),
        _ => false,
    })
}

/// Nombres de los enums "simples" (todas sus variantes son unitarias) de
/// todo el programa -- calculado UNA vez acá, no en cada `value_to_json`
/// recursivo. Mismo chequeo `all_unit` que ya usa `emit_enum_decl`
/// (ts_emit.rs) para decidir "string plano" vs "objeto con tag" en la
/// firma TS -- el runtime tiene que serializar EXACTAMENTE igual, o el
/// valor real no matchea lo que el contrato promete (ni lo que
/// `validators.ts` espera, GRAMMAR.md §3.11).
fn simple_enum_names(program: &Program) -> std::collections::HashSet<String> {
    program
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Enum(e) if e.variants.iter().all(|v| v.fields.is_none()) => Some(e.name.clone()),
            _ => None,
        })
        .collect()
}

pub fn value_to_json(v: &Value, simple_enums: &std::collections::HashSet<String>) -> serde_json::Value {
    use serde_json::json;
    match v {
        Value::Int(n) => json!(n),
        Value::Float(n) => json!(n),
        Value::Str(s) => json!(s),
        Value::Bool(b) => json!(b),
        Value::Null => serde_json::Value::Null,
        Value::Struct(fields) => {
            let mut m = serde_json::Map::new();
            for (k, v) in fields {
                m.insert(k.clone(), value_to_json(v, simple_enums));
            }
            serde_json::Value::Object(m)
        }
        // Enum simple (Role, etc.) -> string plano, igual que `emit_enum_decl`
        // lo mapea a un `type Role = "Admin" | "Member" | "Guest"` de TS, no
        // a un objeto -- antes de esto, CUALQUIER Value::Variant serializaba
        // como `{type: ...}` sin excepción, así que construir un enum simple
        // vía la sintaxis del lenguaje (`Role.Member {}`) daba un valor que
        // no matcheaba ni el contrato ni el validador generado.
        Value::Variant { enum_name, variant, fields } if simple_enums.contains(enum_name) => {
            debug_assert!(fields.is_empty(), "un enum simple no debería tener variantes con campos");
            json!(variant)
        }
        Value::Variant { variant, fields, .. } => {
            let mut m = serde_json::Map::new();
            m.insert("type".to_string(), json!(variant));
            for (k, v) in fields {
                m.insert(k.clone(), value_to_json(v, simple_enums));
            }
            serde_json::Value::Object(m)
        }
        Value::List(items) | Value::Tuple(items) => {
            serde_json::Value::Array(items.iter().map(|v| value_to_json(v, simple_enums)).collect())
        }
        // Salvaguarda: estos marcadores son internos del intérprete y nunca
        // deberían ser el resultado final de un rpc (ver eval_expr::Call).
        Value::Db | Value::DbCollection(_) | Value::BoundMethod(_, _) | Value::FnRef(_) | Value::Closure(..) => {
            serde_json::Value::Null
        }
    }
}

pub fn json_to_value(j: &serde_json::Value) -> Value {
    match j {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else {
                Value::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => Value::Str(s.clone()),
        serde_json::Value::Array(items) => Value::List(items.iter().map(json_to_value).collect()),
        serde_json::Value::Object(map) => {
            Value::Struct(map.iter().map(|(k, v)| (k.clone(), json_to_value(v))).collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::parse;
    use serde_json::json;

    fn program_from(src: &str) -> Program {
        let tokens = tokenize(src).unwrap_or_else(|e| panic!("{e}"));
        parse(tokens).unwrap_or_else(|e| panic!("{e}"))
    }

    fn users_demo() -> Program {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/users.link"),
        )
        .expect("no se pudo leer examples/users.link");
        program_from(&src)
    }

    #[test]
    fn get_by_id_returns_seeded_user() {
        let program = users_demo();
        let db = Db::seeded();
        let result = invoke_rpc(&program, "Users", "getById", &json!({"id": 1}), &db).unwrap();
        assert_eq!(result["name"], json!("Ada Lovelace"));
    }

    #[test]
    fn get_by_id_returns_null_when_missing() {
        let program = users_demo();
        let db = Db::seeded();
        let result = invoke_rpc(&program, "Users", "getById", &json!({"id": 999}), &db).unwrap();
        assert_eq!(result, serde_json::Value::Null);
    }

    #[test]
    fn list_respects_the_take_limit() {
        let program = users_demo();
        let db = Db::seeded();
        let result = invoke_rpc(&program, "Users", "list", &json!({"limit": 1}), &db).unwrap();
        assert_eq!(result.as_array().unwrap().len(), 1);
    }

    #[test]
    fn assignment_mutates_the_existing_binding() {
        let program = program_from(
            r#"
            service S {
                rpc f() -> Int {
                    let mut x = 1;
                    x = 2;
                    x
                }
            }
        "#,
        );
        let result = invoke_rpc(&program, "S", "f", &json!({}), &Db::seeded()).unwrap();
        assert_eq!(result, json!(2));
    }

    #[test]
    fn assignment_inside_if_branch_propagates_to_outer_scope() {
        // La razón de todo el rediseño con Rc<RefCell<Value>>: sin esto, la
        // mutación de "x" adentro del if quedaría atrapada en la copia local
        // de ese bloque y "x" seguiría valiendo 1 afuera.
        let program = program_from(
            r#"
            service S {
                rpc classify(n: Int) -> Int {
                    let mut result = 0;
                    if n > 0 {
                        result = 1;
                    } else {
                        result = -1;
                    }
                    result
                }
            }
        "#,
        );
        let db = Db::seeded();
        let positive = invoke_rpc(&program, "S", "classify", &json!({"n": 5}), &db).unwrap();
        assert_eq!(positive, json!(1));
        let negative = invoke_rpc(&program, "S", "classify", &json!({"n": -5}), &db).unwrap();
        assert_eq!(negative, json!(-1));
    }

    #[test]
    fn literal_and_or_patterns_dispatch_to_the_right_arm() {
        let program = program_from(
            r#"
            service S {
                rpc describe(n: Int) -> String {
                    match n {
                        1 | 2 => "bajo",
                        -1 => "negativo",
                        _ => "otro",
                    }
                }
            }
        "#,
        );
        let db = Db::seeded();
        assert_eq!(invoke_rpc(&program, "S", "describe", &json!({"n": 1}), &db).unwrap(), json!("bajo"));
        assert_eq!(invoke_rpc(&program, "S", "describe", &json!({"n": 2}), &db).unwrap(), json!("bajo"));
        assert_eq!(invoke_rpc(&program, "S", "describe", &json!({"n": -1}), &db).unwrap(), json!("negativo"));
        assert_eq!(invoke_rpc(&program, "S", "describe", &json!({"n": 99}), &db).unwrap(), json!("otro"));
    }

    #[test]
    fn failed_guard_falls_through_to_the_next_arm() {
        // La prueba de verdad del guard: no alcanza con que el checker lo
        // acepte -- el runtime tiene que efectivamente seguir probando arms
        // cuando el patrón matchea pero la condición del guard da false.
        let program = program_from(
            r#"
            service S {
                rpc classify(n: Int) -> String {
                    match n {
                        x if x > 100 => "grande",
                        x if x > 0 => "positivo chico",
                        _ => "cero o negativo",
                    }
                }
            }
        "#,
        );
        let db = Db::seeded();
        assert_eq!(invoke_rpc(&program, "S", "classify", &json!({"n": 200}), &db).unwrap(), json!("grande"));
        assert_eq!(invoke_rpc(&program, "S", "classify", &json!({"n": 5}), &db).unwrap(), json!("positivo chico"));
        assert_eq!(invoke_rpc(&program, "S", "classify", &json!({"n": -5}), &db).unwrap(), json!("cero o negativo"));
    }

    #[test]
    fn bool_match_without_wildcard_runs_correctly() {
        let program = program_from(
            r#"
            service S {
                rpc describe(b: Bool) -> String {
                    match b {
                        true => "sí",
                        false => "no",
                    }
                }
            }
        "#,
        );
        let db = Db::seeded();
        assert_eq!(invoke_rpc(&program, "S", "describe", &json!({"b": true}), &db).unwrap(), json!("sí"));
        assert_eq!(invoke_rpc(&program, "S", "describe", &json!({"b": false}), &db).unwrap(), json!("no"));
    }

    #[test]
    fn array_literal_and_indexing_work_in_runtime() {
        let program = program_from(
            r#"
            service S {
                rpc f() -> Int {
                    let xs = [10, 20, 30];
                    xs[1]
                }
            }
        "#,
        );
        let result = invoke_rpc(&program, "S", "f", &json!({}), &Db::seeded()).unwrap();
        assert_eq!(result, json!(20));
    }

    #[test]
    fn indexing_out_of_range_is_a_runtime_error_not_null() {
        let program = program_from(
            r#"
            service S {
                rpc f() -> Int {
                    let xs = [1, 2, 3];
                    xs[10]
                }
            }
        "#,
        );
        let result = invoke_rpc(&program, "S", "f", &json!({}), &Db::seeded());
        assert!(result.is_err(), "indexar fuera de rango debería fallar, no devolver null");
    }

    #[test]
    fn tuple_construction_access_and_json_shape() {
        let program = program_from(
            r#"
            service S {
                rpc pair() -> (Int, String) { (1, "a") }
                rpc first() -> Int { let t = (1, "a"); t.0 }
                rpc second() -> String { let t = (1, "a"); t.1 }
            }
        "#,
        );
        let db = Db::seeded();
        // (A, B) -> [A, B] en el cable (GRAMMAR.md §4), igual que un array.
        assert_eq!(invoke_rpc(&program, "S", "pair", &json!({}), &db).unwrap(), json!([1, "a"]));
        assert_eq!(invoke_rpc(&program, "S", "first", &json!({}), &db).unwrap(), json!(1));
        assert_eq!(invoke_rpc(&program, "S", "second", &json!({}), &db).unwrap(), json!("a"));
    }

    #[test]
    fn string_methods_work_in_runtime() {
        let program = program_from(
            r#"
            service S {
                rpc len(s: String) -> Int { s.length() }
                rpc has(s: String, needle: String) -> Bool { s.contains(needle) }
            }
        "#,
        );
        let db = Db::seeded();
        assert_eq!(
            invoke_rpc(&program, "S", "len", &json!({"s": "hola"}), &db).unwrap(),
            json!(4)
        );
        // "é" es 2 bytes en UTF-8 pero 1 carácter -- .length() cuenta caracteres.
        assert_eq!(
            invoke_rpc(&program, "S", "len", &json!({"s": "café"}), &db).unwrap(),
            json!(4)
        );
        assert_eq!(
            invoke_rpc(&program, "S", "has", &json!({"s": "ada@example.com", "needle": "@"}), &db).unwrap(),
            json!(true)
        );
        assert_eq!(
            invoke_rpc(&program, "S", "has", &json!({"s": "sin arroba", "needle": "@"}), &db).unwrap(),
            json!(false)
        );
    }

    #[test]
    fn numeric_conversion_works_in_runtime() {
        let program = program_from(
            r#"
            service S {
                rpc toFloat(n: Int) -> Float { n.toFloat() }
                rpc toInt(n: Float) -> Int { n.toInt() }
            }
        "#,
        );
        let db = Db::seeded();
        assert_eq!(
            invoke_rpc(&program, "S", "toFloat", &json!({"n": 3}), &db).unwrap(),
            json!(3.0)
        );
        // trunca hacia cero, no redondea
        assert_eq!(
            invoke_rpc(&program, "S", "toInt", &json!({"n": 3.9}), &db).unwrap(),
            json!(3)
        );
        assert_eq!(
            invoke_rpc(&program, "S", "toInt", &json!({"n": -3.9}), &db).unwrap(),
            json!(-3)
        );
    }

    #[test]
    fn string_concatenation_works_in_runtime() {
        let program = program_from(
            r#"
            service Greeter {
                rpc greet(name: String) -> String {
                    "hola, " + name
                }
            }
        "#,
        );
        let db = Db::seeded();
        let result = invoke_rpc(&program, "Greeter", "greet", &json!({"name": "Carlos"}), &db).unwrap();
        assert_eq!(result, json!("hola, Carlos"));
    }

    #[test]
    fn constructing_a_simple_enum_variant_serializes_as_a_bare_string() {
        // Bug real encontrado al implementar "DB tipada": Value::Variant
        // SIEMPRE serializaba como `{type: "..."}`, sin importar si el enum
        // era simple (Role) o un ADT (ValidationError) -- nadie lo notó
        // porque nada construía un enum simple vía la sintaxis del lenguaje
        // (`Role.Member {}`) antes; los datos sembrados a mano en db.rs
        // usaban Value::Str directo, sin pasar por acá. Justo lo que
        // emit_enum_decl (ts_emit.rs) promete como `type Role = "Admin" |
        // ...` -- y lo que isRole (validators.ts) exige -- es un string
        // plano, no un objeto.
        let program = program_from(
            r#"
            enum Role { Admin, Member, Guest }
            enum Wrapped { Has { value: Int }, Empty }
            service S {
                rpc getRole() -> Role { Role.Member {} }
                rpc getEmpty() -> Wrapped { Wrapped.Empty {} }
            }
        "#,
        );
        let db = Db::seeded();
        let role = invoke_rpc(&program, "S", "getRole", &json!({}), &db).unwrap();
        assert_eq!(role, json!("Member"), "un enum simple debe serializar como string plano");

        // Contraste importante: `Empty` no tiene campos propios, pero
        // `Wrapped` en su conjunto NO es un enum simple (`Has` sí tiene
        // datos) -- así que `Empty` tiene que seguir siendo un objeto con
        // tag, no un string plano. La distinción es de la DECLARACIÓN
        // completa, nunca de si esta variante puntual tiene campos.
        let empty = invoke_rpc(&program, "S", "getEmpty", &json!({}), &db).unwrap();
        assert_eq!(empty["type"], json!("Empty"));
    }

    #[test]
    fn create_wraps_the_new_user_in_result_ok() {
        let program = users_demo();
        let db = Db::seeded();
        let result = invoke_rpc(
            &program,
            "Users",
            "create",
            &json!({"input": {"name": "Grace Hopper", "email": "grace@example.com"}}),
            &db,
        )
        .unwrap();
        assert_eq!(result["type"], json!("Ok"));
        assert_eq!(result["value"]["name"], json!("Grace Hopper"));
        // "DB tipada" (GRAMMAR.md §2.1): `insert` ahora pide Omit<User,"id">
        // completo -- `validate` rellena role/deletedAt antes de insertar,
        // en vez del hack viejo en db.rs que solo ponía deletedAt (y ni
        // eso, si ya venía en el input) y dejaba 'role' directamente ausente.
        assert_eq!(result["value"]["role"], json!("Member"));
        assert_eq!(result["value"]["deletedAt"], serde_json::Value::Null);
        // 'id' lo asigna `db.users.insert` -- nunca lo manda el caller
        // (Omit<User,"id">), así que tiene que ser un entero nuevo real.
        assert!(result["value"]["id"].is_i64());
    }

    #[test]
    fn update_applies_the_patch_in_place() {
        let program = users_demo();
        let db = Db::seeded();
        let result = invoke_rpc(
            &program,
            "Users",
            "update",
            &json!({"id": 1, "patch": {"name": "Ada, Countess of Lovelace"}}),
            &db,
        )
        .unwrap();
        assert_eq!(result["name"], json!("Ada, Countess of Lovelace"));
        // el resto de los campos no se toca -- semántica de Patch<T>, GRAMMAR.md §3.4
        assert_eq!(result["email"], json!("ada@example.com"));
    }

    #[test]
    fn db_new_gives_each_declared_collection_its_own_independent_empty_store() {
        // "DB tipada" v0 (GRAMMAR.md §2.1): antes, runtime/db.rs solo
        // conocía una colección "users" hardcodeada -- Db::new arranca una
        // vacía por cada colección que el programa declare, ninguna
        // comparte estado con las demás.
        let program = program_from(
            r#"
            type Post = { id: Int, title: String }
            type Comment = { id: Int, body: String }
            db { posts: Post[], comments: Comment[] }
            fn newPost(title: String) -> Post { db.posts.insert(Post { title: title }) }
            fn newComment(body: String) -> Comment { db.comments.insert(Comment { body: body }) }
            service S {
                rpc addPost(title: String) -> Post { newPost(title) }
                rpc addComment(body: String) -> Comment { newComment(body) }
                rpc allPosts() -> Post[] { db.posts.all() }
                rpc allComments() -> Comment[] { db.comments.all() }
            }
        "#,
        );
        let db = Db::new(&program);

        let post = invoke_rpc(&program, "S", "addPost", &json!({"title": "Hola"}), &db).unwrap();
        assert_eq!(post["id"], json!(1)); // primer id de ESTA colección, no compartido con comments
        invoke_rpc(&program, "S", "addComment", &json!({"body": "Primer comentario"}), &db).unwrap();
        invoke_rpc(&program, "S", "addComment", &json!({"body": "Segundo comentario"}), &db).unwrap();

        let posts = invoke_rpc(&program, "S", "allPosts", &json!({}), &db).unwrap();
        assert_eq!(posts.as_array().unwrap().len(), 1);
        let comments = invoke_rpc(&program, "S", "allComments", &json!({}), &db).unwrap();
        assert_eq!(comments.as_array().unwrap().len(), 2);
    }

    #[test]
    fn create_with_empty_email_returns_invalid_email_error() {
        // `validate` ya no es un stub -- prueba que el if/== real del
        // backend efectivamente rechaza la entrada, de punta a punta.
        let program = users_demo();
        let db = Db::seeded();
        let result = invoke_rpc(
            &program,
            "Users",
            "create",
            &json!({"input": {"name": "Sin Email", "email": ""}}),
            &db,
        )
        .unwrap();
        assert_eq!(result["type"], json!("Err"));
        assert_eq!(result["error"]["type"], json!("InvalidEmail"));
        assert_eq!(result["error"]["field"], json!("email"));
    }

    #[test]
    fn create_with_empty_name_returns_too_short_error() {
        let program = users_demo();
        let db = Db::seeded();
        let result = invoke_rpc(
            &program,
            "Users",
            "create",
            &json!({"input": {"name": "", "email": "valido@example.com"}}),
            &db,
        )
        .unwrap();
        assert_eq!(result["type"], json!("Err"));
        assert_eq!(result["error"]["type"], json!("TooShort"));
        assert_eq!(result["error"]["field"], json!("name"));
        assert_eq!(result["error"]["min"], json!(2));
    }

    #[test]
    fn named_fn_passed_by_reference_is_callable_through_the_parameter() {
        // Alcance real de "funciones de primera clase" en v0 (GRAMMAR.md
        // §3.10): una `fn` de nivel superior referenciada POR NOMBRE (sin
        // llamarla ahí mismo) tiene que poder viajar como valor -- acá, como
        // argumento -- y ser invocable a través del parámetro que la recibe.
        // Antes del fix esto fallaba en runtime aunque el checker ya lo
        // aceptaba (Expr::Ident cae a `self.fns` en synth_expr).
        let program = program_from(
            r#"
            fn add_one(x: Int) -> Int { x + 1 }
            fn apply_twice(f: (Int) -> Int, x: Int) -> Int { f(f(x)) }
            service S {
                rpc run(x: Int) -> Int {
                    apply_twice(add_one, x)
                }
            }
        "#,
        );
        let result = invoke_rpc(&program, "S", "run", &json!({"x": 5}), &Db::seeded()).unwrap();
        assert_eq!(result, json!(7));
    }

    #[test]
    fn is_stream_member_distinguishes_stream_from_rpc_and_unknown() {
        let program = program_from(
            r#"
            type User = { id: Int, name: String }
            db { users: User[] }
            service Users {
                rpc getById(id: Int) -> User? { db.users.find(id) }
                stream watchAll() -> User { db.users.all() }
            }
        "#,
        );
        assert!(is_stream_member(&program, "Users", "watchAll"));
        assert!(!is_stream_member(&program, "Users", "getById"));
        assert!(!is_stream_member(&program, "Users", "noExiste"));
        assert!(!is_stream_member(&program, "NoService", "watchAll"));
    }

    #[test]
    fn invoke_rpc_on_a_stream_member_evaluates_the_body_to_a_json_array() {
        // El checker ya exige que el cuerpo de un `stream` sea List<T>
        // (check_rpc, checker.rs) -- acá se confirma que invoke_rpc (que no
        // distingue Rpc/Stream al evaluar, ver Member::Rpc(r) | Member::Stream(r)
        // más arriba) de verdad produce un array JSON, la forma que
        // server.rs necesita para poder emitir un evento SSE por elemento.
        let program = program_from(
            r#"
            type User = { id: Int, name: String }
            db { users: User[] }
            service Users {
                stream watchAll() -> User { db.users.all() }
            }
        "#,
        );
        let db = Db::seeded();
        let result = invoke_rpc(&program, "Users", "watchAll", &json!({}), &db).unwrap();
        let arr = result.as_array().expect("el resultado de un stream debería ser un array JSON");
        assert_eq!(arr.len(), 2, "Db::seeded() siembra 2 usuarios bajo 'users'");
    }

    // ---- closures + List.map/.filter (GRAMMAR.md §3.10) ----

    #[test]
    fn filter_and_map_actually_transform_the_list_at_runtime() {
        let program = program_from(
            r#"
            service S {
                rpc run() -> Int[] {
                    let xs = [1, 2, 3, 4, 5];
                    let evens = xs.filter(|x: Int| { x > 2 });
                    evens.map(|x: Int| { x * 10 })
                }
            }
        "#,
        );
        let result = invoke_rpc(&program, "S", "run", &json!({}), &Db::seeded()).unwrap();
        assert_eq!(result, json!([30, 40, 50]));
    }

    #[test]
    fn map_invokes_a_named_fn_reference_at_runtime() {
        let program = program_from(
            r#"
            fn double(x: Int) -> Int { x * 2 }
            service S {
                rpc run() -> Int[] {
                    [1, 2, 3].map(double)
                }
            }
        "#,
        );
        let result = invoke_rpc(&program, "S", "run", &json!({}), &Db::seeded()).unwrap();
        assert_eq!(result, json!([2, 4, 6]));
    }

    #[test]
    fn closure_captures_and_sees_later_mutation_of_the_captured_variable() {
        // La misma celda Rc<RefCell<Value>> que ve el scope exterior --
        // mutar `total` DESPUÉS de crear el closure sigue siendo visible
        // adentro de él en la llamada siguiente (mismo mecanismo que
        // assignment_inside_if_branch_propagates_to_outer_scope).
        let program = program_from(
            r#"
            service S {
                rpc run() -> Int {
                    let mut total = 0;
                    let addToTotal = |x: Int| { total = total + x; x };
                    addToTotal(5);
                    addToTotal(10);
                    total
                }
            }
        "#,
        );
        let result = invoke_rpc(&program, "S", "run", &json!({}), &Db::seeded()).unwrap();
        assert_eq!(result, json!(15));
    }

    #[test]
    fn recursive_closure_via_mut_reassignment_does_not_hang_and_computes_correctly() {
        // Construye un ciclo real de Rc (el segundo closure captura un Env
        // que contiene la misma celda que 'f' está a punto de sobreescribir,
        // ver el comentario sobre Value::PartialEq/Debug a mano) -- confirma
        // que invocarla funciona bien y el programa termina normalmente.
        let program = program_from(
            r#"
            service S {
                rpc run(n: Int) -> Int {
                    let mut f: (Int) -> Int = |x: Int| { x };
                    f = |x: Int| { if x <= 1 { 1 } else { x * f(x - 1) } };
                    f(n)
                }
            }
        "#,
        );
        let result = invoke_rpc(&program, "S", "run", &json!({"n": 5}), &Db::seeded()).unwrap();
        assert_eq!(result, json!(120));
    }

    // ---- narrowing de uniones (GRAMMAR.md §3.9) ----

    #[test]
    fn union_narrowing_dispatches_correctly_and_binds_the_narrowed_type() {
        let program = program_from(
            r#"
            service S {
                rpc describe(v: Int | String) -> Bool {
                    match v {
                        i: Int => i > 0,
                        s: String => s.length() > 0,
                    }
                }
            }
        "#,
        );
        let db = Db::seeded();
        assert_eq!(invoke_rpc(&program, "S", "describe", &json!({"v": 5}), &db).unwrap(), json!(true));
        assert_eq!(invoke_rpc(&program, "S", "describe", &json!({"v": -5}), &db).unwrap(), json!(false));
        assert_eq!(invoke_rpc(&program, "S", "describe", &json!({"v": "hola"}), &db).unwrap(), json!(true));
        assert_eq!(invoke_rpc(&program, "S", "describe", &json!({"v": ""}), &db).unwrap(), json!(false));
    }

    #[test]
    fn union_narrowing_over_structs_dispatches_by_the_actual_field_value_type() {
        // El test de solidez que de verdad importa: el checker aceptó esta
        // unión porque 'x' tiene tipos en conflicto (Int vs String) en A/B
        // -- eso solo es sound si el runtime de verdad chequea el tipo del
        // VALOR guardado en 'x', no solo su presencia (que ambos comparten).
        let program = program_from(
            r#"
            type A = { x: Int }
            type B = { x: String }
            service S {
                rpc describe(v: A | B) -> String {
                    match v {
                        a: A => "A",
                        b: B => "B",
                    }
                }
            }
        "#,
        );
        let db = Db::seeded();
        assert_eq!(
            invoke_rpc(&program, "S", "describe", &json!({"v": {"x": 5}}), &db).unwrap(),
            json!("A")
        );
        assert_eq!(
            invoke_rpc(&program, "S", "describe", &json!({"v": {"x": "hola"}}), &db).unwrap(),
            json!("B")
        );
    }
}
