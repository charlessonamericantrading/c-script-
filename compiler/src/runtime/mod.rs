// Runtime mínimo interpretado (PLAN.md §2.4, Fase 0): un tree-walking
// interpreter que ejecuta cuerpos de rpc/fn contra un "db" en memoria.
// No es el runtime final del lenguaje — Fase 1+ compila a WASM/nativo
// (PLAN.md §4) — esto solo alcanza para que la demo E2E responda de verdad.

pub mod db;
pub mod server;
pub mod session;

use crate::ast::*;
use db::Db;
use session::SessionStore;
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
    /// Marcador interno del identificador `auth` (GRAMMAR.md §3.14, auth
    /// v0) -- mismo trato que `Db`: nunca llega a `value_to_json`.
    Auth,
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
            (Auth, Auth) => true,
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
            Value::Auth => write!(f, "Auth"),
            Value::BoundMethod(recv, method) => f.debug_tuple("BoundMethod").field(recv).field(method).finish(),
            Value::FnRef(name) => f.debug_tuple("FnRef").field(name).finish(),
            // A propósito NO imprime `captured_env` -- podría ser cíclico
            // (ver el comentario en el enum), y de todos modos un entorno
            // capturado entero no aporta nada legible a un mensaje de error.
            Value::Closure(params, ..) => write!(f, "Closure({params:?}, <cuerpo y entorno omitidos>)"),
        }
    }
}

/// De quién es la culpa -- lo único que `server.rs` necesita para elegir
/// entre 4xx y 5xx. Un request que no matchea el contrato declarado es un
/// error del CLIENTE (400): devolverlo como 500 haría parecer que el
/// servidor se rompió, cuando en realidad rechazó correctamente algo mal
/// formado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Runtime,
    BadRequest,
}

#[derive(Debug)]
pub struct RuntimeError {
    pub message: String,
    pub kind: ErrorKind,
}

impl RuntimeError {
    pub fn new(message: impl Into<String>) -> Self {
        RuntimeError { message: message.into(), kind: ErrorKind::Runtime }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        RuntimeError { message: message.into(), kind: ErrorKind::BadRequest }
    }
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kind {
            ErrorKind::Runtime => write!(f, "error en runtime: {}", self.message),
            ErrorKind::BadRequest => write!(f, "request inválido: {}", self.message),
        }
    }
}

fn err(msg: impl Into<String>) -> RuntimeError {
    RuntimeError::new(msg)
}

/// Para todo lo que rechaza un request por no matchear el contrato
/// (`json_to_typed_value` y su familia).
fn bad_req(msg: impl Into<String>) -> RuntimeError {
    RuntimeError::bad_request(msg)
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
/// El runtime lleva un `&Checker` -- no una tabla de símbolos propia.
///
/// Hace falta poder resolver un `TypeExpr` a un `Type` real en dos lugares:
/// los patrones de narrowing (`nombre: Tipo`, GRAMMAR.md §3.9) y la
/// validación tipada de los argumentos que llegan por el wire
/// (`json_to_typed_value`). La primera versión de esto traía un resolvedor
/// propio y simplificado acá (`resolve_pattern_type`), y esa duplicación
/// causó un bug real encontrado en la auditoría: devolvía `Type::Dynamic`
/// para `Generic`/`Tuple`/`Map`, así que un `match` sobre una unión con un
/// miembro `Box<Int>` compilaba y después NUNCA matcheaba en runtime
/// ("ningún arm coincidió"). Reusar el resolvedor del checker -- la única
/// fuente de verdad, que ya sabe de genéricos, alias, `Result`/`Patch`/
/// `Map` -- elimina la clase entera de bugs por divergencia entre los dos.
type Checker = crate::checker::Checker;

fn cell(v: Value) -> Rc<RefCell<Value>> {
    Rc::new(RefCell::new(v))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn eval_block(
    block: &Block,
    env: &Env,
    db: &Db,
    fns: &Fns,
    checker: &Checker,
    sessions: &SessionStore,
    current_token: Option<&str>,
) -> Result<Value, RuntimeError> {
    let mut local = env.clone();
    for stmt in &block.stmts {
        match stmt {
            Stmt::Let { name, value, .. } => {
                let v = eval_expr(value, &local, db, fns, checker, sessions, current_token)?;
                local.insert(name.clone(), cell(v));
            }
            Stmt::Assign { name, value } => {
                let v = eval_expr(value, &local, db, fns, checker, sessions, current_token)?;
                let target = local
                    .get(name)
                    .ok_or_else(|| err(format!("variable no declarada en runtime: '{name}'")))?;
                *target.borrow_mut() = v;
            }
            Stmt::Return(Some(e)) => return eval_expr(e, &local, db, fns, checker, sessions, current_token),
            Stmt::Return(None) => return Ok(Value::Null),
            Stmt::Expr(e) => {
                eval_expr(e, &local, db, fns, checker, sessions, current_token)?;
            }
        }
    }
    match &block.tail {
        Some(e) => eval_expr(e, &local, db, fns, checker, sessions, current_token),
        None => Ok(Value::Null),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn eval_expr(
    e: &Expr,
    env: &Env,
    db: &Db,
    fns: &Fns,
    checker: &Checker,
    sessions: &SessionStore,
    current_token: Option<&str>,
) -> Result<Value, RuntimeError> {
    match e {
        Expr::Int(n) => Ok(Value::Int(*n)),
        Expr::Float(n) => Ok(Value::Float(*n)),
        Expr::Str(s) => Ok(Value::Str(s.clone())),
        Expr::Bool(b) => Ok(Value::Bool(*b)),
        Expr::Null => Ok(Value::Null),
        Expr::Paren(inner) => eval_expr(inner, env, db, fns, checker, sessions, current_token),
        Expr::Ident(name) => {
            // El lookup de variables va PRIMERO -- antes, "db" se chequeaba
            // acá arriba de todo (bug preexistente, encontrado en el review
            // de esta ronda al agregar "auth" al lado: `synth_expr` del
            // checker YA ponía `env` primero, con este mismo comentario, pero
            // el fix nunca se aplicó acá. Consecuencia real: `fn f(db: Int)
            // -> Int { db + 1 }` tipaba perfecto y crasheaba en runtime,
            // porque esta rama devolvía `Value::Db` ignorando el parámetro
            // real. El único test relacionado solo verificaba que tipara,
            // nunca lo ejecutaba -- por eso no se había notado).
            if let Some(c) = env.get(name) {
                return Ok(c.borrow().clone());
            }
            if name == "db" {
                return Ok(Value::Db);
            }
            if name == "auth" {
                return Ok(Value::Auth);
            }
            // Un `const` de nivel superior: su valor es siempre un literal
            // (el checker lo exige), así que evaluarlo en un env vacío no
            // depende de nada del scope actual.
            if let Some(c) = checker.consts.get(name.as_str()) {
                return eval_expr(&c.value, &Env::new(), db, fns, checker, sessions, current_token);
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
            let base_v = eval_expr(base, env, db, fns, checker, sessions, current_token)?;
            match base_v {
                // Un campo ausente da `null`, no un error. El checker ya
                // rechaza leer un campo que el tipo no declara, así que
                // "no está" solo puede significar una clave declarada
                // OPCIONAL (`x?: T`, GRAMMAR.md §3.4) que efectivamente no
                // vino -- y para eso el tipo sintetizado ya es `T?`
                // (checker.rs::field_access_ty), así que `null` es
                // exactamente el valor que corresponde. Antes esto era un
                // error de runtime: un objeto perfectamente válido sin esa
                // clave reventaba al leerla.
                Value::Struct(fields) | Value::Variant { fields, .. } => Ok(fields
                    .into_iter()
                    .find(|(n, _)| n == field)
                    .map(|(_, v)| v)
                    .unwrap_or(Value::Null)),
                Value::Db => Ok(Value::DbCollection(field.clone())),
                // Métodos builtin sobre primitivos (GRAMMAR.md §3.8, ej.
                // `x.toFloat()`) usan el mismo BoundMethod que db/listas/auth --
                // el checker ya validó que el nombre existe para este tipo.
                Value::DbCollection(_) | Value::List(_) | Value::Int(_) | Value::Float(_) | Value::Str(_) | Value::Auth => {
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
                    let arg_vs = eval_args(args, env, db, fns, checker, sessions, current_token)?;
                    return call_fn_decl(decl, arg_vs, db, fns, checker, sessions, current_token);
                }
            }
            let callee_v = eval_expr(callee, env, db, fns, checker, sessions, current_token)?;
            let arg_vs = eval_args(args, env, db, fns, checker, sessions, current_token)?;
            match callee_v {
                Value::BoundMethod(receiver, method) => {
                    call_method(*receiver, &method, arg_vs, db, fns, checker, sessions, current_token)
                }
                // Llamada INDIRECTA: `callee` fue una variable/parámetro que
                // contenía una referencia a función o un closure (GRAMMAR.md
                // §3.10), no el nombre escrito ahí mismo -- ej. dentro de
                // `apply_twice(f, x) { f(f(x)) }`, `f` llega como FnRef;
                // `list.filter(f)` con `f` un closure ya evaluado, como
                // Closure. `call_callable` despacha ambos casos (y produce
                // el mismo error de "no se puede llamar" para cualquier otra
                // cosa, ver su propio fallback).
                other => call_callable(other, arg_vs, db, fns, checker, sessions, current_token),
            }
        }
        Expr::StructLit { name, variant, fields } => {
            let evaluated = fields
                .iter()
                .map(|(k, e)| Ok((k.clone(), eval_expr(e, env, db, fns, checker, sessions, current_token)?)))
                .collect::<Result<Vec<_>, RuntimeError>>()?;
            match variant {
                Some(v) => {
                    Ok(Value::Variant { enum_name: name.clone(), variant: v.clone(), fields: evaluated })
                }
                None => Ok(Value::Struct(evaluated)),
            }
        }
        Expr::Match { scrutinee, arms } => {
            let v = eval_expr(scrutinee, env, db, fns, checker, sessions, current_token)?;
            for arm in arms {
                if let Some(bindings) = try_match_pattern(&arm.pattern, &v, checker) {
                    let mut arm_env = env.clone();
                    arm_env.extend(bindings.into_iter().map(|(k, v)| (k, cell(v))));
                    if let Some(guard) = &arm.guard {
                        match eval_expr(guard, &arm_env, db, fns, checker, sessions, current_token)? {
                            Value::Bool(true) => {}
                            // El patrón matcheó pero el guard no se cumplió --
                            // se sigue probando el resto de los arms, no se
                            // considera "sin match" (GRAMMAR.md §3.3).
                            Value::Bool(false) => continue,
                            other => return Err(err(format!("el guard de 'match' no es Bool en runtime: {other:?}"))),
                        }
                    }
                    return match &arm.body {
                        MatchArmBody::Expr(e) => eval_expr(e, &arm_env, db, fns, checker, sessions, current_token),
                        MatchArmBody::Block(b) => eval_block(b, &arm_env, db, fns, checker, sessions, current_token),
                    };
                }
            }
            // No debería pasar: el checker ya garantizó exhaustividad
            // (GRAMMAR.md §3.3) antes de que este código llegara a ejecutarse.
            Err(err("ningún arm de match coincidió — el checker debería haber impedido esto"))
        }
        Expr::If { cond, then_block, else_block } => {
            let c = eval_expr(cond, env, db, fns, checker, sessions, current_token)?;
            match c {
                Value::Bool(true) => eval_block(then_block, env, db, fns, checker, sessions, current_token),
                Value::Bool(false) => eval_block(else_block, env, db, fns, checker, sessions, current_token),
                other => Err(err(format!("la condición de 'if' no es Bool en runtime: {other:?}"))),
            }
        }
        Expr::Binary { op, left, right } => eval_binary(*op, left, right, env, db, fns, checker, sessions, current_token),
        Expr::Unary { op, operand } => eval_unary(*op, operand, env, db, fns, checker, sessions, current_token),
        Expr::ArrayLit(items) => {
            let vs = items
                .iter()
                .map(|e| eval_expr(e, env, db, fns, checker, sessions, current_token))
                .collect::<Result<Vec<_>, RuntimeError>>()?;
            Ok(Value::List(vs))
        }
        Expr::Index { base, index } => {
            let base_v = eval_expr(base, env, db, fns, checker, sessions, current_token)?;
            let idx = as_int(&eval_expr(index, env, db, fns, checker, sessions, current_token)?)?;
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
                .map(|e| eval_expr(e, env, db, fns, checker, sessions, current_token))
                .collect::<Result<Vec<_>, RuntimeError>>()?;
            Ok(Value::Tuple(vs))
        }
        Expr::TupleIndex { base, index } => {
            let base_v = eval_expr(base, env, db, fns, checker, sessions, current_token)?;
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

// El intérprete enhebra su contexto (env/db/fns/checker) explícitamente en
// vez de guardarlo en un struct: mantiene visible en cada firma qué necesita
// de verdad cada función, que es justo lo que hizo evidente que `call_method`
// no tenía acceso a `fns` cuando hicieron falta `.map`/`.filter`.
#[allow(clippy::too_many_arguments)]
fn eval_binary(
    op: BinaryOp,
    left: &Expr,
    right: &Expr,
    env: &Env,
    db: &Db,
    fns: &Fns,
    checker: &Checker,
    sessions: &SessionStore,
    current_token: Option<&str>,
) -> Result<Value, RuntimeError> {
    use BinaryOp::*;
    // && / || cortocircuitan: el lado derecho no se evalúa si ya se sabe el
    // resultado, igual que en cualquier lenguaje con estos operadores.
    if matches!(op, And | Or) {
        let l = as_bool(&eval_expr(left, env, db, fns, checker, sessions, current_token)?)?;
        return match (op, l) {
            (And, false) => Ok(Value::Bool(false)),
            (Or, true) => Ok(Value::Bool(true)),
            _ => Ok(Value::Bool(as_bool(&eval_expr(right, env, db, fns, checker, sessions, current_token)?)?)),
        };
    }

    let l = eval_expr(left, env, db, fns, checker, sessions, current_token)?;
    let r = eval_expr(right, env, db, fns, checker, sessions, current_token)?;
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

#[allow(clippy::too_many_arguments)]
fn eval_unary(
    op: UnaryOp,
    operand: &Expr,
    env: &Env,
    db: &Db,
    fns: &Fns,
    checker: &Checker,
    sessions: &SessionStore,
    current_token: Option<&str>,
) -> Result<Value, RuntimeError> {
    let v = eval_expr(operand, env, db, fns, checker, sessions, current_token)?;
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

#[allow(clippy::too_many_arguments)]
fn eval_args(
    args: &[Expr],
    env: &Env,
    db: &Db,
    fns: &Fns,
    checker: &Checker,
    sessions: &SessionStore,
    current_token: Option<&str>,
) -> Result<Vec<Value>, RuntimeError> {
    args.iter().map(|a| eval_expr(a, env, db, fns, checker, sessions, current_token)).collect()
}

/// Invoca una `fn` de usuario ya resuelta con argumentos ya evaluados --
/// compartido por la llamada directa (`f(x)`) y la indirecta a través de un
/// `Value::FnRef` (`let g = f; g(x)`), para no duplicar el armado del scope.
#[allow(clippy::too_many_arguments)]
fn call_fn_decl(
    decl: &FnDecl,
    arg_vs: Vec<Value>,
    db: &Db,
    fns: &Fns,
    checker: &Checker,
    sessions: &SessionStore,
    current_token: Option<&str>,
) -> Result<Value, RuntimeError> {
    let mut fn_env = Env::new();
    for (p, v) in decl.params.iter().zip(arg_vs) {
        fn_env.insert(p.name.clone(), cell(v));
    }
    eval_block(&decl.body, &fn_env, db, fns, checker, sessions, current_token)
}

fn try_match_pattern(pattern: &Pattern, v: &Value, checker: &Checker) -> Option<Vec<(String, Value)>> {
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
                    bindings.extend(try_match_pattern(&fp.pattern, field_v, checker)?);
                }
            }
            Some(bindings)
        }
        // Ninguna alternativa liga nada (el checker ya lo garantizó, ver
        // bind_pattern), así que probar cada una hasta la primera que
        // matchee y devolver sus bindings (vacíos) alcanza.
        Pattern::Or(subs) => subs.iter().find_map(|p| try_match_pattern(p, v, checker)),
        // Narrowing de uniones (GRAMMAR.md §3.9): resuelve el texpr del
        // patrón con el resolvedor REAL del checker (que ya validó este
        // mismo texpr en `check_exhaustive_union` antes de que la ejecución
        // llegue acá) y chequea el SHAPE real del valor contra él.
        Pattern::Type(name, texpr) => {
            let resolved = checker.resolve_type(texpr).ok()?;
            value_matches_type(v, &resolved, checker).then(|| vec![(name.clone(), v.clone())])
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

/// ¿El SHAPE real de `v` corresponde a `ty`? Superficial pero recursivo en
/// los campos REQUERIDOS de un struct (no solo su presencia, también que el
/// valor guardado ahí tenga a su vez el shape correcto) -- eso es lo que
/// hace sound al análisis de ambigüedad del checker (`check_exhaustive_union`,
/// checker.rs): dos miembros de una unión con un campo compartido de tipos
/// mutuamente excluyentes (ej. `x: Int` vs `x: String`) se distinguen por el
/// tipo REAL del valor guardado en ese campo, no por su mera presencia (que
/// el subtipado estructural de ancho podría satisfacer para ambos a la vez
/// con un tercer tipo más ancho).
fn value_matches_type(v: &Value, ty: &crate::types::Type, checker: &Checker) -> bool {
    use crate::types::Type;
    match ty {
        Type::Int => matches!(v, Value::Int(_)),
        Type::Float => matches!(v, Value::Float(_)),
        Type::String => matches!(v, Value::Str(_)),
        Type::Bool => matches!(v, Value::Bool(_)),
        Type::Optional(inner) => matches!(v, Value::Null) || value_matches_type(v, inner, checker),
        Type::List(inner) => match v {
            Value::List(items) => items.iter().all(|i| value_matches_type(i, inner, checker)),
            _ => false,
        },
        Type::Tuple(tys) => match v {
            Value::Tuple(items) => {
                items.len() == tys.len()
                    && items.iter().zip(tys).all(|(i, t)| value_matches_type(i, t, checker))
            }
            _ => false,
        },
        Type::MapOf(_, val_ty) => match v {
            Value::Struct(entries) => entries.iter().all(|(_, val)| value_matches_type(val, val_ty, checker)),
            _ => false,
        },
        Type::Enum(name) => matches!(v, Value::Variant { enum_name, .. } if enum_name == name),
        Type::ResultOf(..) => matches!(v, Value::Variant { enum_name, .. } if enum_name == "Result"),
        Type::Struct { fields, .. } => struct_matches_fields(v, fields, checker),
        // Un genérico de usuario ya instanciado (`Box<Int>`): se expande a
        // su forma real -- struct o enum -- en vez de tratarse como opaco.
        // La versión anterior de este código lo resolvía a `Dynamic` y
        // devolvía `false` siempre, así que un `match` sobre una unión con
        // un miembro genérico compilaba y NUNCA matcheaba en runtime.
        Type::Generic(name, args) => {
            if let Ok(fields) = checker.expand_generic_struct(name, args) {
                return struct_matches_fields(v, &fields, checker);
            }
            // No es un struct genérico -> es un enum genérico instanciado;
            // esos siguen siendo nominales por su nombre base.
            matches!(v, Value::Variant { enum_name, .. } if enum_name == name)
        }
        Type::PatchOf(inner) => match (v, &**inner) {
            // Todo campo es opcional en un Patch<T>, pero los que estén
            // presentes tienen que tener el tipo declarado en T.
            (Value::Struct(entries), Type::Struct { fields, .. }) => entries.iter().all(|(k, val)| {
                fields
                    .iter()
                    .find(|f| &f.name == k)
                    .is_some_and(|f| value_matches_type(val, &f.ty, checker))
            }),
            _ => false,
        },
        Type::Null => matches!(v, Value::Null),
        Type::Void => matches!(v, Value::Null),
        Type::Union(members) => members.iter().any(|m| value_matches_type(v, m, checker)),
        Type::Dynamic => true,
        // Function/Db/DbCollection/TypeParam -- ninguno es un valor que
        // pueda existir con una forma verificable acá.
        _ => false,
    }
}

fn struct_matches_fields(v: &Value, fields: &[crate::types::FieldType], checker: &Checker) -> bool {
    match v {
        Value::Struct(vfields) => fields.iter().all(|f| {
            match vfields.iter().find(|(n, _)| n == &f.name) {
                Some((_, fv)) => value_matches_type(fv, &f.ty, checker),
                // Ausente: solo válido si la clave era opcional (`x?: T`).
                None => f.optional,
            }
        }),
        _ => false,
    }
}

/// Invoca un closure YA evaluado -- a diferencia de `call_fn_decl` (que
/// arranca de un `Env::new()` vacío, porque una `fn` de nivel superior no
/// tiene scope que capturar), acá el scope de la llamada arranca del
/// `captured_env` que el closure guardó al construirse (GRAMMAR.md §3.10)
/// -- ESA es la captura léxica real. Los parámetros se ligan encima,
/// sombreando cualquier variable capturada con el mismo nombre.
#[allow(clippy::too_many_arguments)]
fn call_closure(
    param_names: &[String],
    body: &Block,
    captured_env: &Env,
    arg_vs: Vec<Value>,
    db: &Db,
    fns: &Fns,
    checker: &Checker,
    sessions: &SessionStore,
    current_token: Option<&str>,
) -> Result<Value, RuntimeError> {
    let mut call_env = captured_env.clone();
    for (name, v) in param_names.iter().zip(arg_vs) {
        call_env.insert(name.clone(), cell(v));
    }
    eval_block(body, &call_env, db, fns, checker, sessions, current_token)
}

/// Cualquier `Value` invocable -- una referencia a `fn` por nombre o un
/// closure -- con argumentos ya evaluados. Compartido por la llamada
/// indirecta de `Expr::Call` y por `.map`/`.filter` (más abajo), que
/// necesitan invocar su callback sin que les importe cuál de las dos formas
/// sea.
#[allow(clippy::too_many_arguments)]
fn call_callable(
    v: Value,
    arg_vs: Vec<Value>,
    db: &Db,
    fns: &Fns,
    checker: &Checker,
    sessions: &SessionStore,
    current_token: Option<&str>,
) -> Result<Value, RuntimeError> {
    match v {
        Value::FnRef(name) => {
            let decl = fns
                .get(name.as_str())
                .ok_or_else(|| err(format!("fn desconocida: '{name}'")))?;
            call_fn_decl(decl, arg_vs, db, fns, checker, sessions, current_token)
        }
        Value::Closure(params, body, captured_env) => {
            call_closure(&params, &body, &captured_env, arg_vs, db, fns, checker, sessions, current_token)
        }
        other => Err(err(format!("no se puede llamar un valor {other:?}"))),
    }
}

#[allow(clippy::too_many_arguments)]
fn call_method(
    receiver: Value,
    method: &str,
    args: Vec<Value>,
    db: &Db,
    fns: &Fns,
    checker: &Checker,
    sessions: &SessionStore,
    current_token: Option<&str>,
) -> Result<Value, RuntimeError> {
    match receiver {
        Value::DbCollection(coll) => db.call(&coll, method, args),
        Value::List(items) => match method {
            "take" => {
                let n = as_int(args.first().ok_or_else(|| err("take requiere 1 argumento"))?)? as usize;
                Ok(Value::List(items.into_iter().take(n).collect()))
            }
            "length" => Ok(Value::Int(items.len() as i64)),
            "filter" => {
                let f = args.into_iter().next().ok_or_else(|| err("'filter' requiere 1 argumento"))?;
                let mut kept = Vec::new();
                for item in items {
                    if as_bool(&call_callable(f.clone(), vec![item.clone()], db, fns, checker, sessions, current_token)?)? {
                        kept.push(item);
                    }
                }
                Ok(Value::List(kept))
            }
            "map" => {
                let f = args.into_iter().next().ok_or_else(|| err("'map' requiere 1 argumento"))?;
                let mut mapped = Vec::with_capacity(items.len());
                for item in items {
                    mapped.push(call_callable(f.clone(), vec![item], db, fns, checker, sessions, current_token)?);
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
        // Auth v0 (GRAMMAR.md §3.14). `createSession` extrae (enum_name,
        // variant) del `Value::Variant` recibido -- el checker ya garantizó
        // que el argumento sintetiza a `Type::Enum(_)`, y la sesión solo
        // guarda el TAG, nunca los campos (ver SessionStore). `destroySession`
        // opera sobre `current_token` (la sesión que ya autenticó ESTA
        // request, resuelta en server.rs), NUNCA sobre un token que el
        // caller nombre como argumento -- si tomara un token como parámetro,
        // cualquiera podría destruir la sesión de cualquier otro con solo
        // conocer/adivinar ese string (hallado en el review adversarial).
        Value::Auth => match method {
            "createSession" => {
                let role = args.into_iter().next().ok_or_else(|| err("createSession requiere 1 argumento"))?;
                let Value::Variant { enum_name, variant, .. } = role else {
                    return Err(err("createSession requiere un valor de un enum declarado"));
                };
                Ok(Value::Str(sessions.create(enum_name, variant)))
            }
            "destroySession" => {
                if let Some(tok) = current_token {
                    sessions.destroy(tok);
                }
                Ok(Value::Null)
            }
            other => Err(err(format!("método desconocido sobre auth: '{other}'"))),
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
/// Wrapper que preserva la firma pública de siempre -- ~70 call sites
/// existentes (la enorme mayoría tests de este mismo archivo, más
/// `bin/wasm_demo.rs`) no necesitan saber nada de auth. Usa un
/// `SessionStore` descartable y ningún token: equivalente a una request
/// anónima sin ninguna sesión activa, que es exactamente lo que corresponde
/// para un rpc sin `@authenticated`/`@requires` (los únicos que estos call
/// sites ejercitan). El servidor real (`runtime/server.rs`) llama
/// `invoke_rpc_with_sessions` directamente, con el `SessionStore` que vive
/// mientras el proceso corre y el token que trajo la request.
pub fn invoke_rpc(
    program: &Program,
    service_name: &str,
    rpc_name: &str,
    args_json: &serde_json::Value,
    db: &Db,
) -> Result<serde_json::Value, RuntimeError> {
    invoke_rpc_with_sessions(program, service_name, rpc_name, args_json, db, &SessionStore::new(), None)
}

/// Punto de entrada real: ejecuta `{service_name}.{rpc_name}` con
/// argumentos JSON (el mismo shape que emite client.ts: `{ paramName: valor,
/// ... }`).
///
/// La decisión de autorización (¿puede ESTA request llamar a ESTE rpc?) NO
/// se toma acá -- vive en `server.rs::check_auth_gate`, que corre ANTES y
/// nunca llega a invocar esto si el gate no pasa. Lo que SÍ cruza hacia acá
/// es `sessions` (para que `auth.createSession`/`destroySession` funcionen
/// dentro del cuerpo) y `current_token` (para que `destroySession()` sepa
/// cuál es "la propia" sesión) -- ninguno de los dos es una decisión, son
/// datos ya resueltos por el caller.
#[allow(clippy::too_many_arguments)]
pub fn invoke_rpc_with_sessions(
    program: &Program,
    service_name: &str,
    rpc_name: &str,
    args_json: &serde_json::Value,
    db: &Db,
    sessions: &SessionStore,
    current_token: Option<&str>,
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

    // El resolvedor de tipos REAL (checker.rs), no una tabla propia --
    // hace falta para resolver los tipos declarados de los parámetros y
    // para los patrones de narrowing. El programa ya fue chequeado antes de
    // llegar acá (main.rs::load_and_check), así que no puede traer errores
    // de símbolos; aun así se propagan en vez de ignorarse.
    let (checker, symbol_errors) = crate::checker::Checker::build_symbols(program);
    if let Some(e) = symbol_errors.into_iter().next() {
        return Err(err(format!("programa inválido: {e}")));
    }

    let empty = serde_json::Map::new();
    let args_obj = args_json.as_object().unwrap_or(&empty);
    let mut env = Env::new();
    for p in &rpc.params {
        let declared = checker
            .resolve_type(&p.ty)
            .map_err(|e| err(format!("no se pudo resolver el tipo del parámetro '{}': {e}", p.name)))?;
        let v = match args_obj.get(&p.name) {
            // ACÁ es donde el borde se volvió tipado: el JSON que llega se
            // valida contra el tipo DECLARADO y se reconstruye con la forma
            // interna correcta (un enum pasa a ser Value::Variant, no un
            // Str/Struct suelto). Ver `json_to_typed_value`.
            Some(j) => json_to_typed_value(j, &declared, &checker, &p.name)?,
            None => match &p.default {
                Some(default_expr) => {
                    eval_expr(default_expr, &Env::new(), db, &fns, &checker, sessions, current_token)?
                }
                // Antes esto era `Value::Null` en silencio -- un parámetro
                // requerido que no venía en el body producía un fallo
                // confuso mucho más adentro (o ninguno).
                None if matches!(declared, crate::types::Type::Optional(_)) => Value::Null,
                None => {
                    return Err(bad_req(format!(
                        "falta el parámetro requerido '{}' (se esperaba {})",
                        p.name,
                        describe_type(&declared)
                    )))
                }
            },
        };
        env.insert(p.name.clone(), cell(v));
    }

    let result = eval_block(&rpc.body, &env, db, &fns, &checker, sessions, current_token)?;
    let simple_enums = simple_enum_names(program);
    Ok(value_to_json(&result, &simple_enums))
}

/// Convierte el JSON que llegó por el wire al `Value` interno que
/// corresponde al tipo DECLARADO, validándolo en el camino.
///
/// Esto es la contraparte, del lado servidor, de lo que `validators.ts` ya
/// hacía del lado cliente desde que existe (GRAMMAR.md §3.11): el cliente
/// verificaba cada RESPUESTA contra el contrato, pero el servidor nunca
/// verificó ninguna PETICIÓN -- `json_to_value` era una conversión
/// puramente sintáctica, sin ningún tipo a la vista. La auditoría mostró
/// las cuatro consecuencias, todas reproducidas de verdad contra un
/// servidor real:
///
/// 1. Un enum que llegaba por el wire (`"Admin"`, o `{type:"Circle",r:3}`)
///    se convertía en `Value::Str`/`Value::Struct`, nunca en
///    `Value::Variant`. Así que `match` sobre CUALQUIER parámetro de tipo
///    enum fallaba siempre con "ningún arm coincidió — el checker debería
///    haber impedido esto" (500), pese a que el cliente mandaba
///    exactamente lo que el contrato exige.
/// 2. Por lo mismo, `r == Role.Admin {}` daba `false` para un valor que
///    vino del wire y `true` para uno construido en el backend: dos
///    representaciones internas del mismo valor del contrato.
/// 3. JSON arbitrario (un String donde se declaró Int, campos que el tipo
///    no tiene, `null` en un campo no-nullable) entraba al intérprete y se
///    PERSISTÍA en la db, de donde salía después en respuestas que el
///    propio `validators.ts` del cliente rechazaba -- un cliente
///    malintencionado o simplemente roto podía dejar una fila inservible
///    para todos los demás.
/// 4. Un parámetro requerido ausente se volvía `Value::Null` en silencio.
///
/// Sobre campos de más: se ACEPTAN pero se DESCARTAN. Aceptarlos es
/// coherente con el subtipado estructural de ancho (GRAMMAR.md §3.2, un
/// valor con campos de más es un subtipo válido); descartarlos es lo que
/// garantiza que el `Value` resultante tenga EXACTAMENTE la forma
/// declarada, que es lo que corta la clase de bug (3).
fn json_to_typed_value(
    j: &serde_json::Value,
    ty: &crate::types::Type,
    checker: &Checker,
    path: &str,
) -> Result<Value, RuntimeError> {
    use crate::types::Type;
    let mismatch = || {
        bad_req(format!(
            "'{path}': se esperaba {}, se recibió {}",
            describe_type(ty),
            describe_json(j)
        ))
    };
    match ty {
        Type::Int => j.as_i64().map(Value::Int).ok_or_else(mismatch),
        Type::Float => j.as_f64().map(Value::Float).ok_or_else(mismatch),
        Type::String => j.as_str().map(|s| Value::Str(s.to_string())).ok_or_else(mismatch),
        Type::Bool => j.as_bool().map(Value::Bool).ok_or_else(mismatch),
        Type::Optional(inner) => {
            if j.is_null() {
                Ok(Value::Null)
            } else {
                json_to_typed_value(j, inner, checker, path)
            }
        }
        Type::List(inner) => {
            let items = j.as_array().ok_or_else(mismatch)?;
            items
                .iter()
                .enumerate()
                .map(|(i, item)| json_to_typed_value(item, inner, checker, &format!("{path}[{i}]")))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::List)
        }
        Type::Tuple(tys) => {
            let items = j.as_array().ok_or_else(mismatch)?;
            if items.len() != tys.len() {
                return Err(bad_req(format!(
                    "'{path}': se esperaba una tupla de {} elementos, se recibieron {}",
                    tys.len(),
                    items.len()
                )));
            }
            items
                .iter()
                .zip(tys)
                .enumerate()
                .map(|(i, (item, t))| json_to_typed_value(item, t, checker, &format!("{path}.{i}")))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Tuple)
        }
        Type::MapOf(key_ty, val_ty) => {
            let obj = j.as_object().ok_or_else(mismatch)?;
            let mut entries = Vec::with_capacity(obj.len());
            for (k, v) in obj {
                // Las claves de un objeto JSON son siempre string; para un
                // Map<Int,V> tienen que parsear como entero de verdad.
                if matches!(**key_ty, Type::Int) && k.parse::<i64>().is_err() {
                    return Err(bad_req(format!(
                        "'{path}': la clave '{k}' no es un entero válido para un Map<Int, _>"
                    )));
                }
                entries.push((k.clone(), json_to_typed_value(v, val_ty, checker, &format!("{path}.{k}"))?));
            }
            Ok(Value::Struct(entries))
        }
        Type::Struct { fields, .. } => struct_from_json(j, fields, checker, path, &mismatch),
        Type::Generic(name, args) => {
            if let Ok(fields) = checker.expand_generic_struct(name, args) {
                return struct_from_json(j, &fields, checker, path, &mismatch);
            }
            variant_from_json(j, ty, name, checker, path, &mismatch)
        }
        Type::Enum(name) => variant_from_json(j, ty, name, checker, path, &mismatch),
        Type::ResultOf(..) => variant_from_json(j, ty, "Result", checker, path, &mismatch),
        Type::PatchOf(inner) => {
            let Type::Struct { fields, .. } = &**inner else {
                return Err(bad_req(format!("'{path}': Patch<T> requiere que T sea un struct")));
            };
            let obj = j.as_object().ok_or_else(mismatch)?;
            let mut out = Vec::new();
            for (k, v) in obj {
                // Un campo que el tipo base no declara se descarta: sin
                // esto, `applyPatch` escribía claves inventadas directo en
                // la fila almacenada (bug real de la auditoría).
                if let Some(f) = fields.iter().find(|f| &f.name == k) {
                    out.push((k.clone(), json_to_typed_value(v, &f.ty, checker, &format!("{path}.{k}"))?));
                }
            }
            Ok(Value::Struct(out))
        }
        // Una unión acepta el primer miembro que encaje. El checker ya
        // rechaza las uniones cuyos miembros no se puedan distinguir
        // (GRAMMAR.md §3.9), así que "el primero que encaja" no es
        // ambiguo para las que sí se pueden matchear.
        Type::Union(members) => members
            .iter()
            .find_map(|m| json_to_typed_value(j, m, checker, path).ok())
            .ok_or_else(mismatch),
        Type::Void | Type::Null => Ok(Value::Null),
        // Sin forma declarada que verificar: se acepta tal cual.
        Type::Dynamic => Ok(json_to_value(j)),
        // Una función no puede cruzar el wire (tabla de mapeo, §4).
        Type::Function(..) => Err(bad_req(format!(
            "'{path}': un valor de tipo función no puede recibirse por la red"
        ))),
        other => Err(bad_req(format!("'{path}': tipo no soportado en el wire: {other:?}"))),
    }
}

fn struct_from_json(
    j: &serde_json::Value,
    fields: &[crate::types::FieldType],
    checker: &Checker,
    path: &str,
    mismatch: &dyn Fn() -> RuntimeError,
) -> Result<Value, RuntimeError> {
    let obj = j.as_object().ok_or_else(mismatch)?;
    let mut out = Vec::with_capacity(fields.len());
    for f in fields {
        match obj.get(&f.name) {
            Some(fv) => {
                out.push((f.name.clone(), json_to_typed_value(fv, &f.ty, checker, &format!("{path}.{}", f.name))?))
            }
            // `x?: T` -- clave que puede estar ausente (GRAMMAR.md §3.4):
            // simplemente no se incluye en el valor resultante.
            None if f.optional => {}
            None => {
                return Err(bad_req(format!(
                    "'{path}': falta el campo requerido '{}' (se esperaba {})",
                    f.name,
                    describe_type(&f.ty)
                )))
            }
        }
    }
    // Nótese que solo se copian los campos DECLARADOS: los de más se
    // aceptan (width subtyping) pero no sobreviven.
    Ok(Value::Struct(out))
}

/// Reconstruye un `Value::Variant` desde su forma de wire. Es el paso que
/// faltaba por completo: la serialización (`value_to_json`) distingue enum
/// "simple" (string plano) de ADT (objeto con tag `type`), pero no existía
/// la inversa, así que nada que llegara del cliente era nunca un Variant.
fn variant_from_json(
    j: &serde_json::Value,
    ty: &crate::types::Type,
    enum_name: &str,
    checker: &Checker,
    path: &str,
    mismatch: &dyn Fn() -> RuntimeError,
) -> Result<Value, RuntimeError> {
    let all_unit = checker
        .enums
        .get(enum_name)
        .is_some_and(|e| e.variants.iter().all(|v| v.fields.is_none()));

    if all_unit {
        let s = j.as_str().ok_or_else(mismatch)?;
        let names = checker
            .enum_variant_names(enum_name)
            .map_err(|e| bad_req(format!("'{path}': {e}")))?;
        if !names.iter().any(|n| n == s) {
            return Err(bad_req(format!(
                "'{path}': '{s}' no es una variante de '{enum_name}' (son: {})",
                names.join(", ")
            )));
        }
        return Ok(Value::Variant {
            enum_name: enum_name.to_string(),
            variant: s.to_string(),
            fields: Vec::new(),
        });
    }

    let obj = j.as_object().ok_or_else(mismatch)?;
    let variant = obj
        .get("type")
        .and_then(|t| t.as_str())
        .ok_or_else(|| bad_req(format!("'{path}': falta el tag 'type' que identifica la variante de '{enum_name}'")))?;
    let declared = checker
        .variant_field_types(ty, enum_name, variant)
        .map_err(|e| bad_req(format!("'{path}': {e}")))?;
    let mut fields = Vec::with_capacity(declared.len());
    for (fname, fty) in &declared {
        let fv = obj
            .get(fname)
            .ok_or_else(|| bad_req(format!("'{path}': la variante '{variant}' requiere el campo '{fname}'")))?;
        fields.push((fname.clone(), json_to_typed_value(fv, fty, checker, &format!("{path}.{fname}"))?));
    }
    Ok(Value::Variant {
        enum_name: enum_name.to_string(),
        variant: variant.to_string(),
        fields,
    })
}

/// Nombre legible de un tipo para los mensajes de error del borde -- el
/// `{:?}` de `Type` es útil para depurar el compilador, pero ilegible para
/// quien está mandando un request mal formado.
fn describe_type(ty: &crate::types::Type) -> String {
    use crate::types::Type;
    match ty {
        Type::Int => "Int".into(),
        Type::Float => "Float".into(),
        Type::String => "String".into(),
        Type::Bool => "Bool".into(),
        Type::Null | Type::Void => "null".into(),
        Type::Optional(inner) => format!("{}?", describe_type(inner)),
        Type::List(inner) => format!("{}[]", describe_type(inner)),
        Type::Tuple(items) => format!("({})", items.iter().map(describe_type).collect::<Vec<_>>().join(", ")),
        Type::MapOf(k, v) => format!("Map<{}, {}>", describe_type(k), describe_type(v)),
        Type::Enum(name) => name.clone(),
        Type::ResultOf(ok, e) => format!("Result<{}, {}>", describe_type(ok), describe_type(e)),
        Type::PatchOf(inner) => format!("Patch<{}>", describe_type(inner)),
        Type::Generic(name, args) => {
            format!("{name}<{}>", args.iter().map(describe_type).collect::<Vec<_>>().join(", "))
        }
        Type::Struct { name: Some(n), .. } => n.clone(),
        Type::Struct { fields, .. } => {
            format!("{{ {} }}", fields.iter().map(|f| f.name.clone()).collect::<Vec<_>>().join(", "))
        }
        Type::Union(members) => members.iter().map(describe_type).collect::<Vec<_>>().join(" | "),
        other => format!("{other:?}"),
    }
}

fn describe_json(j: &serde_json::Value) -> &'static str {
    match j {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "un booleano",
        serde_json::Value::Number(_) => "un número",
        serde_json::Value::String(_) => "un string",
        serde_json::Value::Array(_) => "un array",
        serde_json::Value::Object(_) => "un objeto",
    }
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

/// Anotación `@authenticated`/`@requires(...)` de `{service_name}.{rpc_name}`,
/// si tiene una -- hermana de `is_stream_member` (mismo archivo/patrón, ya
/// usada por `server.rs` antes de invocar nada). `None` cubre tanto "sin
/// anotación" como "el service/rpc no existe": ese segundo caso lo detecta
/// (con el error real) `invoke_rpc_with_sessions` cuando de verdad se llega
/// a invocar -- acá solo hace falta saber si HAY que exigir algo antes.
pub fn required_auth<'a>(program: &'a Program, service_name: &str, rpc_name: &str) -> Option<&'a Annotation> {
    program.items.iter().find_map(|i| match i {
        Item::Service(s) if s.name == service_name => s.members.iter().find_map(|m| match m {
            Member::Rpc(r) | Member::Stream(r) if r.name == rpc_name => r.annotation.as_ref(),
            _ => None,
        }),
        _ => None,
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
        Value::Db | Value::DbCollection(_) | Value::Auth | Value::BoundMethod(_, _) | Value::FnRef(_) | Value::Closure(..) => {
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
        parse(tokens).unwrap_or_else(|e| panic!("{e:?}"))
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

    // ---- validación tipada del borde (auditoría) ----
    //
    // Todos estos casos se reprodujeron primero contra un servidor real:
    // el cliente mandaba exactamente lo que el contrato exige y el
    // servidor respondía 500, o aceptaba basura con 200.

    fn wire_demo() -> Program {
        program_from(
            r#"
            enum Role { Admin, Member }
            enum Shape { Circle { r: Int }, Square { s: Int } }
            type Item = { id: Int, price: Int }
            type OptKey = { id: Int, note?: String }
            service S {
                rpc matchEnum(r: Role) -> String {
                    match r {
                        Role.Admin => "admin",
                        Role.Member => "member",
                    }
                }
                rpc matchAdt(sh: Shape) -> Int {
                    match sh {
                        Shape.Circle { r: r } => r,
                        Shape.Square { s: s } => s,
                    }
                }
                rpc eqEnum(r: Role) -> Bool { r == Role.Admin {} }
                rpc addOne(n: Int) -> Int { n + 1 }
                rpc readNote(o: OptKey) -> String? { o.note }
                rpc echoItem(i: Item) -> Item { i }
            }
        "#,
        )
    }

    #[test]
    fn an_enum_arriving_from_the_wire_becomes_a_real_variant_and_matches() {
        // Un enum simple viaja como string plano (GRAMMAR.md §4). Antes,
        // json_to_value lo dejaba como Value::Str y `match` no encontraba
        // ningún arm -> 500 "el checker debería haber impedido esto".
        let program = wire_demo();
        let db = Db::seeded();
        assert_eq!(
            invoke_rpc(&program, "S", "matchEnum", &json!({"r": "Admin"}), &db).unwrap(),
            json!("admin")
        );
        // Un ADT viaja como objeto con tag `type`.
        assert_eq!(
            invoke_rpc(&program, "S", "matchAdt", &json!({"sh": {"type": "Circle", "r": 3}}), &db).unwrap(),
            json!(3)
        );
    }

    #[test]
    fn equality_holds_between_a_wire_value_and_one_built_in_the_backend() {
        // El síntoma más sutil de la misma causa: dos representaciones
        // internas del mismo valor del contrato daban != entre sí.
        let program = wire_demo();
        assert_eq!(
            invoke_rpc(&program, "S", "eqEnum", &json!({"r": "Admin"}), &Db::seeded()).unwrap(),
            json!(true)
        );
        assert_eq!(
            invoke_rpc(&program, "S", "eqEnum", &json!({"r": "Member"}), &Db::seeded()).unwrap(),
            json!(false)
        );
    }

    #[test]
    fn a_value_that_does_not_match_the_declared_type_is_rejected_as_a_bad_request() {
        let program = wire_demo();
        let db = Db::seeded();
        for (rpc, args) in [
            ("addOne", json!({"n": "no soy un int"})),
            ("addOne", json!({"n": 1.5})),
            ("addOne", json!({})), // parámetro requerido ausente
            ("matchEnum", json!({"r": "NoExiste"})),
            ("matchEnum", json!({"r": {"type": "Admin"}})), // forma de ADT para un enum simple
            ("matchAdt", json!({"sh": {"type": "Circle"}})), // falta el campo de la variante
            ("matchAdt", json!({"sh": "Circle"})),
            ("echoItem", json!({"i": {"id": 1}})), // falta un campo requerido
            ("echoItem", json!({"i": {"id": "x", "price": 1}})),
        ] {
            let e = invoke_rpc(&program, "S", rpc, &args, &db)
                .expect_err(&format!("{rpc} con {args} debería rechazarse"));
            assert_eq!(e.kind, ErrorKind::BadRequest, "{rpc} con {args}: {e}");
        }
    }

    #[test]
    fn extra_fields_are_accepted_but_dropped() {
        // Aceptarlos es coherente con el subtipado de ancho (GRAMMAR.md
        // §3.2); descartarlos es lo que evita que se persistan y salgan
        // después en una respuesta que el validador del cliente rechaza.
        let program = wire_demo();
        let result = invoke_rpc(
            &program,
            "S",
            "echoItem",
            &json!({"i": {"id": 1, "price": 2, "colado": true}}),
            &Db::seeded(),
        )
        .unwrap();
        assert_eq!(result, json!({"id": 1, "price": 2}));
        assert!(result.get("colado").is_none(), "un campo no declarado no debe sobrevivir");
    }

    #[test]
    fn an_absent_optional_key_reads_as_null_instead_of_failing() {
        let program = wire_demo();
        let db = Db::seeded();
        assert_eq!(
            invoke_rpc(&program, "S", "readNote", &json!({"o": {"id": 1}}), &db).unwrap(),
            serde_json::Value::Null
        );
        assert_eq!(
            invoke_rpc(&program, "S", "readNote", &json!({"o": {"id": 1, "note": "hola"}}), &db).unwrap(),
            json!("hola")
        );
    }

    #[test]
    fn a_patch_cannot_smuggle_undeclared_fields_into_a_stored_row() {
        // Bug real: `applyPatch` escribía CUALQUIER clave del patch directo
        // en la fila almacenada, así que un request malformado dejaba una
        // fila que después ni el propio contrato admitía.
        let program = users_demo();
        let db = Db::seeded();
        let result = invoke_rpc(
            &program,
            "Users",
            "update",
            &json!({"id": 1, "patch": {"name": "Ada L.", "colado": {"x": 1}}}),
            &db,
        )
        .unwrap();
        assert_eq!(result["name"], json!("Ada L."));
        assert!(result.get("colado").is_none(), "un campo fuera del contrato no debe entrar a la fila");
    }

    #[test]
    fn a_patch_with_a_wrong_typed_field_is_rejected() {
        let program = users_demo();
        let e = invoke_rpc(
            &program,
            "Users",
            "update",
            &json!({"id": 1, "patch": {"name": 123}}),
            &Db::seeded(),
        )
        .expect_err("un patch con un campo del tipo equivocado debería rechazarse");
        assert_eq!(e.kind, ErrorKind::BadRequest, "{e}");
    }

    #[test]
    fn a_null_in_a_non_nullable_field_is_rejected() {
        let program = users_demo();
        let e = invoke_rpc(
            &program,
            "Users",
            "update",
            &json!({"id": 1, "patch": {"name": null}}),
            &Db::seeded(),
        )
        .expect_err("null en un campo no-nullable debería rechazarse");
        assert_eq!(e.kind, ErrorKind::BadRequest, "{e}");
    }

    #[test]
    fn a_const_evaluates_to_its_value_at_runtime() {
        let program = program_from(
            r#"
            const MAX: Int = 20;
            enum Role { Admin, Member }
            const DEF: Role = Role.Member {};
            service S {
                rpc limit() -> Int { MAX }
                rpc capped(n: Int) -> Int { if n > MAX { MAX } else { n } }
                rpc defaultRole() -> Role { DEF }
            }
        "#,
        );
        let db = Db::seeded();
        assert_eq!(invoke_rpc(&program, "S", "limit", &json!({}), &db).unwrap(), json!(20));
        assert_eq!(invoke_rpc(&program, "S", "capped", &json!({"n": 5}), &db).unwrap(), json!(5));
        assert_eq!(invoke_rpc(&program, "S", "capped", &json!({"n": 99}), &db).unwrap(), json!(20));
        // Y un const de enum simple serializa como string plano, igual que
        // cualquier otro valor de ese enum.
        assert_eq!(invoke_rpc(&program, "S", "defaultRole", &json!({}), &db).unwrap(), json!("Member"));
    }

    #[test]
    fn db_new_declares_exactly_the_collections_the_program_declares() {
        // `serve` usa Db::new, no Db::seeded: un programa que declara una
        // colección que no se llama "users" tiene que funcionar.
        let program = program_from(
            r#"
            type Item = { id: Int, price: Int }
            db { items: Item[] }
            service S {
                rpc all() -> Item[] { db.items.all() }
            }
        "#,
        );
        let db = Db::new(&program);
        assert_eq!(invoke_rpc(&program, "S", "all", &json!({}), &db).unwrap(), json!([]));
    }

    // ---- auth v0 (GRAMMAR.md §3.14) ----

    #[test]
    fn regression_a_parameter_named_db_is_not_shadowed_by_the_builtin() {
        // Bug preexistente encontrado en el review de esta ronda: esta
        // función tipaba perfecto (synth_expr del checker ya ponía `env`
        // antes que el chequeo de "db"), pero CRASHEABA en runtime porque
        // esta misma función chequeaba "db" ANTES de `env.get`. Ejecutado
        // de verdad (no solo `check_source(..).is_ok()`), que es como se
        // encontró que el fix anterior nunca se había aplicado acá.
        let program = program_from(
            r#"
            service S {
                rpc f(db: Int) -> Int { db + 1 }
            }
        "#,
        );
        let result = invoke_rpc(&program, "S", "f", &json!({"db": 41}), &Db::seeded()).unwrap();
        assert_eq!(result, json!(42));
    }

    #[test]
    fn regression_a_parameter_named_auth_is_not_shadowed_by_the_builtin() {
        // Mismo bug, para el identificador nuevo de esta ronda -- probado
        // aparte para que una regresión futura en cualquiera de los dos no
        // dependa de que el otro test lo cubra.
        let program = program_from(
            r#"
            service S {
                rpc f(auth: Int) -> Int { auth + 1 }
            }
        "#,
        );
        let result = invoke_rpc(&program, "S", "f", &json!({"auth": 41}), &Db::seeded()).unwrap();
        assert_eq!(result, json!(42));
    }

    #[test]
    fn create_session_then_role_for_round_trips_through_the_store() {
        let program = program_from(
            r#"
            enum Role { Admin, Member }
            service S {
                rpc login() -> String { auth.createSession(Role.Admin {}) }
            }
        "#,
        );
        let sessions = SessionStore::new();
        let result =
            invoke_rpc_with_sessions(&program, "S", "login", &json!({}), &Db::seeded(), &sessions, None).unwrap();
        let token = result.as_str().expect("login debería devolver un String").to_string();
        assert_eq!(sessions.role_for(&token), Some(("Role".to_string(), "Admin".to_string())));
    }

    #[test]
    fn destroy_session_removes_the_current_token_from_the_store() {
        let program = program_from(
            r#"
            service S {
                rpc logout() -> Void { auth.destroySession() }
            }
        "#,
        );
        let sessions = SessionStore::new();
        let token = sessions.create("Role".to_string(), "Admin".to_string());
        invoke_rpc_with_sessions(&program, "S", "logout", &json!({}), &Db::seeded(), &sessions, Some(&token))
            .unwrap();
        assert_eq!(sessions.role_for(&token), None);
    }

    #[test]
    fn destroy_session_without_a_current_token_does_not_panic() {
        // No debería pasar en la práctica (cualquier rpc que llame
        // destroySession de forma útil va a estar @authenticated), pero no
        // es un error real si pasa -- no-op silencioso.
        let program = program_from(
            r#"
            service S {
                rpc logout() -> Void { auth.destroySession() }
            }
        "#,
        );
        let sessions = SessionStore::new();
        let result =
            invoke_rpc_with_sessions(&program, "S", "logout", &json!({}), &Db::seeded(), &sessions, None);
        assert!(result.is_ok());
    }

    #[test]
    fn required_auth_reflects_the_annotation_on_each_rpc() {
        let program = program_from(
            r#"
            enum Role { Admin, Member }
            service S {
                @authenticated
                rpc me() -> Int { 1 }

                @requires(Role.Admin)
                rpc deleteThing(id: Int) -> Void { }

                rpc list() -> Int[] { [] }
            }
        "#,
        );
        assert_eq!(required_auth(&program, "S", "me"), Some(&Annotation::Authenticated));
        assert_eq!(
            required_auth(&program, "S", "deleteThing"),
            Some(&Annotation::Requires { enum_name: "Role".to_string(), variant_name: "Admin".to_string() })
        );
        assert_eq!(required_auth(&program, "S", "list"), None);
        assert_eq!(required_auth(&program, "S", "noExiste"), None);
        assert_eq!(required_auth(&program, "NoExiste", "list"), None);
    }

    #[test]
    fn list_length_counts_elements() {
        let program = program_from(
            r#"
            service S {
                rpc count(xs: Int[]) -> Int { xs.length() }
            }
        "#,
        );
        let result =
            invoke_rpc(&program, "S", "count", &json!({"xs": [1, 2, 3]}), &Db::seeded()).unwrap();
        assert_eq!(result, json!(3));
    }
}
