// Runtime mínimo interpretado (PLAN.md §2.4, Fase 0): un tree-walking
// interpreter que ejecuta cuerpos de rpc/fn contra un "db" en memoria.
// No es el runtime final del lenguaje — Fase 1+ compila a WASM/nativo
// (PLAN.md §4) — esto solo alcanza para que la demo E2E responda de verdad.

pub mod db;
pub mod server;
pub mod session;
pub(crate) mod store;
mod timestamp;

use crate::ast::*;
use db::Db;
use session::SessionStore;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

/// Cota dura de iteraciones de `while` por invocación de rpc/fn (GRAMMAR.md
/// §3.15). Sin esto, un `while true { }` (o cualquier condición que el
/// programa nunca vuelve falsa) cuelga PARA SIEMPRE el único hilo que
/// atiende TODAS las requests (`runtime/server.rs::serve` no tiene timeout
/// ni scheduling cooperativo) -- no solo la request que lo disparó.
/// Deliberadamente generoso y NO configurable en v0: es un backstop contra
/// el bug/loop-infinito más común, no un sistema fino de cuotas de
/// recursos. Se cuenta UNA vez por invocación de `invoke_rpc_with_sessions`
/// (el `Cell` se crea ahí y se enhebra por TODO el árbol de evaluación,
/// incluyendo loops anidados y loops dentro de una fn/closure llamada desde
/// el cuerpo) -- así un programa no puede esquivar la cota partiendo un
/// loop grande en muchos chicos.
const MAX_WHILE_ITERATIONS: u64 = 1_000_000;

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
    /// Mismo rango que `Int` -- ver la doc de `Type::Int64` (types.rs) para
    /// por qué existe como variante propia en vez de reusar `Int` (el borde
    /// serializa cada uno distinto, y `value_to_json` no tiene contexto de
    /// `Type` para decidirlo de otra forma).
    Int64(i64),
    /// Milisegundos desde epoch UTC -- ver la doc de `Type::Timestamp`
    /// (types.rs) para el resto del diseño (GRAMMAR.md §3.31).
    Timestamp(i64),
    Float(f64),
    Str(String),
    /// Forma canónica ya validada -- ver la doc de `Type::Uuid` (types.rs,
    /// GRAMMAR.md §3.70) para por qué es una variante propia en vez de
    /// reusar `Str`: sin esto, `call_method` no podría distinguir "esto es
    /// un Uuid, `.toString()` tiene sentido" de "esto ya es un String
    /// plano" una vez que la información de tipo ESTÁTICO ya no está
    /// disponible en runtime.
    Uuid(String),
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
    /// Marcador interno para el nombre de un Service (ej. `Users` en `Users.create`)
    Service(String),
    /// Marcador interno para el módulo `math`
    Math,
    /// Marcador interno para el módulo `crypto`
    Crypto,
    /// Marcador interno para el módulo `http`
    Http,
    /// Marcador interno para el módulo `json`
    Json,
    /// Marcador interno para el módulo `base64`
    Base64,
    /// Marcador interno para el módulo `env` (GRAMMAR.md §3.38)
    Env,
    /// Marcador interno para el módulo `request` (GRAMMAR.md §3.38) -- body
    /// crudo y headers de la request HTTP que invocó este rpc, si la hay
    /// (ver `Db::request_context`; ausente fuera de un servidor real, ej.
    /// desde `linkc test`).
    Request,
    /// Marcador interno para el módulo `smtp` (GRAMMAR.md §3.43) -- mandar
    /// un email por SMTP.
    Smtp,
    /// Marcador interno para el módulo `response` (GRAMMAR.md §3.46) --
    /// controlar la respuesta HTTP de este rpc.
    Response,
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
            (Int64(a), Int64(b)) => a == b,
            (Timestamp(a), Timestamp(b)) => a == b,
            (Float(a), Float(b)) => a == b,
            (Str(a), Str(b)) => a == b,
            (Uuid(a), Uuid(b)) => a == b,
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
            (Service(a), Service(b)) => a == b,
            (Math, Math) => true,
            (Crypto, Crypto) => true,
            (Http, Http) => true,
            (Json, Json) => true,
            (Base64, Base64) => true,
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
            Value::Int64(n) => f.debug_tuple("Int64").field(n).finish(),
            Value::Timestamp(n) => f.debug_tuple("Timestamp").field(n).finish(),
            Value::Float(n) => f.debug_tuple("Float").field(n).finish(),
            Value::Str(s) => f.debug_tuple("Str").field(s).finish(),
            Value::Uuid(s) => f.debug_tuple("Uuid").field(s).finish(),
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
            Value::Service(name) => f.debug_tuple("Service").field(name).finish(),
            Value::Math => write!(f, "Math"),
            Value::Crypto => write!(f, "Crypto"),
            Value::Http => write!(f, "Http"),
            Value::Json => write!(f, "Json"),
            Value::Base64 => write!(f, "Base64"),
            Value::Env => write!(f, "Env"),
            Value::Request => write!(f, "Request"),
            Value::Smtp => write!(f, "Smtp"),
            Value::Response => write!(f, "Response"),
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
    step_budget: &Cell<u64>,
) -> Result<Value, RuntimeError> {
    let mut local = env.clone();
    for stmt in &block.stmts {
        match &stmt.node {
            Stmt::Let { name, value, .. } => {
                let v = eval_expr(value, &local, db, fns, checker, sessions, current_token, step_budget)?;
                local.insert(name.clone(), cell(v));
            }
            Stmt::Assign { name, value } => {
                let v = eval_expr(value, &local, db, fns, checker, sessions, current_token, step_budget)?;
                let target = local
                    .get(name)
                    .ok_or_else(|| err(format!("variable no declarada en runtime: '{name}'")))?;
                *target.borrow_mut() = v;
            }
            Stmt::Return(Some(e)) => return eval_expr(e, &local, db, fns, checker, sessions, current_token, step_budget),
            Stmt::Return(None) => return Ok(Value::Null),
            Stmt::Expr(e) => {
                eval_expr(e, &local, db, fns, checker, sessions, current_token, step_budget)?;
            }
            // Loop real de Rust re-evaluando `cond` contra el MISMO `local`
            // que ya acumuló los `let`/`let mut` previos de este bloque --
            // es lo que hace que `i = i + 1;` adentro del cuerpo mute la
            // celda que ve `let mut i = 0;` de afuera (misma mecánica
            // Rc<RefCell<Value>> que ya usa `if`, sin ningún scoping nuevo).
            // El checker ya garantizó que `body` no contiene ningún
            // `return` alcanzable (checker.rs::check_stmt), así que no hace
            // falta ninguna señal de control nueva acá -- el valor de
            // `eval_block(body, ...)` se descarta a propósito, igual que el
            // de un if/match en posición de sentencia.
            Stmt::While { cond, body } => loop {
                match eval_expr(cond, &local, db, fns, checker, sessions, current_token, step_budget)? {
                    Value::Bool(true) => {
                        step_budget.set(step_budget.get() + 1);
                        if step_budget.get() > MAX_WHILE_ITERATIONS {
                            return Err(err(format!(
                                "límite de {MAX_WHILE_ITERATIONS} iteraciones de 'while' excedido -- \
                                 posible loop infinito (GRAMMAR.md §3.15)"
                            )));
                        }
                        eval_block(body, &local, db, fns, checker, sessions, current_token, step_budget)?;
                    }
                    Value::Bool(false) => break,
                    other => {
                        return Err(err(format!("la condición de 'while' no es Bool en runtime: {other:?}")))
                    }
                }
            },
        }
    }
    match &block.tail {
        Some(e) => eval_expr(e, &local, db, fns, checker, sessions, current_token, step_budget),
        None => Ok(Value::Null),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn eval_expr(
    e: &Spanned<Expr>,
    env: &Env,
    db: &Db,
    fns: &Fns,
    checker: &Checker,
    sessions: &SessionStore,
    current_token: Option<&str>,
    step_budget: &Cell<u64>,
) -> Result<Value, RuntimeError> {
    match &e.node {
        Expr::Int(n) => Ok(Value::Int(*n)),
        Expr::Float(n) => Ok(Value::Float(*n)),
        Expr::Str(s) => Ok(Value::Str(s.clone())),
        Expr::Bool(b) => Ok(Value::Bool(*b)),
        Expr::Null => Ok(Value::Null),
        Expr::Paren(inner) => eval_expr(inner, env, db, fns, checker, sessions, current_token, step_budget),
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
            if name == "math" {
                return Ok(Value::Math);
            }
            if name == "crypto" {
                return Ok(Value::Crypto);
            }
            if name == "http" {
                return Ok(Value::Http);
            }
            if name == "json" {
                return Ok(Value::Json);
            }
            if name == "base64" {
                return Ok(Value::Base64);
            }
            if name == "env" {
                return Ok(Value::Env);
            }
            if name == "request" {
                return Ok(Value::Request);
            }
            if name == "smtp" {
                return Ok(Value::Smtp);
            }
            if name == "response" {
                return Ok(Value::Response);
            }
            // Un `const` de nivel superior: su valor es siempre un literal
            // (el checker lo exige), así que evaluarlo en un env vacío no
            // depende de nada del scope actual.
            if let Some(c) = checker.consts.get(name.as_str()) {
                return eval_expr(&c.value, &Env::new(), db, fns, checker, sessions, current_token, step_budget);
            }
            // No es una variable local -- si es una `fn` de nivel superior,
            // referenciarla por nombre produce un FnRef (ver su doc en el
            // enum Value), no un error: el checker ya la trata como un valor
            // de tipo Function en este mismo caso (checker.rs, synth_expr).
            if fns.contains_key(name.as_str()) {
                return Ok(Value::FnRef(name.clone()));
            }
            if name == "now" {
                return Ok(Value::FnRef("now".to_string()));
            }
            if name == "assert" {
                return Ok(Value::FnRef("assert".to_string()));
            }
            if name == "panic" {
                return Ok(Value::FnRef("panic".to_string()));
            }
            if checker.services.contains_key(name.as_str()) {
                return Ok(Value::Service(name.clone()));
            }
            Err(err(format!("variable no declarada en runtime: '{name}'")))
        }
        Expr::FieldAccess { base, field } => {
            let base_v = eval_expr(base, env, db, fns, checker, sessions, current_token, step_budget)?;
            match base_v {
                Value::Struct(fields) | Value::Variant { fields, .. } => Ok(fields
                    .into_iter()
                    .find(|(n, _)| n == field)
                    .map(|(_, v)| v)
                    .unwrap_or(Value::Null)),
                Value::Db => Ok(Value::DbCollection(field.clone())),
                Value::Service(_) | Value::DbCollection(_) | Value::List(_) | Value::Int(_) | Value::Int64(_) | Value::Float(_) | Value::Bool(_) | Value::Str(_) | Value::Uuid(_) | Value::Timestamp(_) | Value::Auth | Value::Math | Value::Crypto | Value::Http | Value::Json | Value::Base64 | Value::Env | Value::Request | Value::Smtp | Value::Response => {
                    Ok(Value::BoundMethod(Box::new(base_v), field.clone()))
                }
                other => Err(err(format!("no se puede acceder al campo '{field}' sobre {other:?}"))),
            }
        }
        Expr::Call { callee, args } => {
            // Llamada directa a una `fn` de usuario por nombre -- atajo
            // frecuente que evita pasar por FnRef (ver Expr::Ident arriba)
            // solo para volver a buscar el mismo nombre en `fns`.
            if let Expr::Ident(name) = &callee.node {
                if !env.contains_key(name.as_str()) {
                    if let Some(decl) = fns.get(name.as_str()) {
                        let arg_vs = eval_args(args, env, db, fns, checker, sessions, current_token, step_budget)?;
                        return call_fn_decl(decl, arg_vs, db, fns, checker, sessions, current_token, step_budget);
                    }
                    if name == "now" {
                        if !args.is_empty() {
                            return Err(err("'now' no toma argumentos"));
                        }
                        let now_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(0);
                        return Ok(Value::Timestamp(now_ms));
                    }
                    if name == "assert" {
                        let arg_vs = eval_args(args, env, db, fns, checker, sessions, current_token, step_budget)?;
                        let cond = match arg_vs.first() {
                            Some(Value::Bool(b)) => *b,
                            _ => return Err(err("'assert' requiere un primer argumento Bool")),
                        };
                        if !cond {
                            let msg = match arg_vs.get(1) {
                                Some(Value::Str(s)) => format!("asercion fallida: {s}"),
                                _ => "asercion fallida".to_string(),
                            };
                            return Err(err(msg));
                        }
                        return Ok(Value::Null);
                    }
                    if name == "panic" {
                        let arg_vs = eval_args(args, env, db, fns, checker, sessions, current_token, step_budget)?;
                        let msg = match arg_vs.first() {
                            Some(Value::Str(s)) => format!("panic: {s}"),
                            _ => "panic".to_string(),
                        };
                        return Err(err(msg));
                    }
                }
            }
            // `.isSome()`/`.isNone()` sobre un `T?` (GRAMMAR.md §3.9):
            // interceptado ACÁ, antes de evaluar `callee` como field access
            // genérico -- el checker solo aprueba estos dos nombres cuando
            // `base` es de tipo `Optional(_)` (`try_builtin_method`), pero un
            // opcional PRESENTE no tiene ningún envoltorio en runtime: su
            // valor es el de `T` tal cual, que puede ser `Value::Struct`.
            // `Expr::FieldAccess` de más abajo, evaluado genéricamente,
            // buscaría "isSome" como un CAMPO real de ese struct (y fallaría
            // a `Value::Null` en vez de producir un método) -- exactamente
            // el mismo desacuerdo que motivó este atajo.
            //
            // Ojo con el caso adversarial: un struct PLANO (no opcional)
            // puede legítimamente declarar un campo `isSome`/`isNone` de
            // tipo función y llamarlo con esta MISMA sintaxis
            // (`x.isSome()`, closures como campos, GRAMMAR.md §3.10) -- ESE
            // caso tiene que seguir el camino normal, no el atajo de abajo.
            // Se distinguen mirando si el valor real tiene un campo con ese
            // nombre: si lo tiene, es el campo (posible closure), no el
            // opcional; `base_v` ya evaluado se reusa para no evaluar `base`
            // dos veces (importante si tiene efectos, ej. una lectura a `db`).
            if let Expr::FieldAccess { base, field } = &callee.node {
                if field == "isSome" || field == "isNone" {
                    let base_v = eval_expr(base, env, db, fns, checker, sessions, current_token, step_budget)?;
                    let shadowed_by_real_field = matches!(
                        &base_v,
                        Value::Struct(fields) | Value::Variant { fields, .. } if fields.iter().any(|(n, _)| n == field)
                    );
                    if !shadowed_by_real_field {
                        let is_null = matches!(base_v, Value::Null);
                        return Ok(Value::Bool(if field == "isSome" { !is_null } else { is_null }));
                    }
                    let callee_v = match &base_v {
                        Value::Struct(fields) | Value::Variant { fields, .. } => {
                            fields.iter().find(|(n, _)| n == field).map(|(_, v)| v.clone()).unwrap_or(Value::Null)
                        }
                        _ => unreachable!("shadowed_by_real_field solo es true para Struct/Variant"),
                    };
                    let arg_vs = eval_args(args, env, db, fns, checker, sessions, current_token, step_budget)?;
                    return call_callable(callee_v, arg_vs, db, fns, checker, sessions, current_token, step_budget);
                }
            }
            let callee_v = eval_expr(callee, env, db, fns, checker, sessions, current_token, step_budget)?;
            let arg_vs = eval_args(args, env, db, fns, checker, sessions, current_token, step_budget)?;
            match callee_v {
                Value::BoundMethod(receiver, method) => {
                    call_method(*receiver, &method, arg_vs, db, fns, checker, sessions, current_token, step_budget)
                }
                // Llamada INDIRECTA: `callee` fue una variable/parámetro que
                // contenía una referencia a función o un closure (GRAMMAR.md
                // §3.10), no el nombre escrito ahí mismo -- ej. dentro de
                // `apply_twice(f, x) { f(f(x)) }`, `f` llega como FnRef;
                // `list.filter(f)` con `f` un closure ya evaluado, como
                // Closure. `call_callable` despacha ambos casos (y produce
                // el mismo error de "no se puede llamar" para cualquier otra
                // cosa, ver su propio fallback).
                other => call_callable(other, arg_vs, db, fns, checker, sessions, current_token, step_budget),
            }
        }
        Expr::StructLit { name, variant, fields } => {
            let mut evaluated = fields
                .iter()
                .map(|(k, e)| Ok((k.clone(), eval_expr(e, env, db, fns, checker, sessions, current_token, step_budget)?)))
                .collect::<Result<Vec<_>, RuntimeError>>()?;
            let ast_fields = field_annotations_for(checker, name, variant.as_deref());
            // `= expr` (GRAMMAR.md §3.74): un campo con default que el
            // literal fuente NO mencionó se completa acá -- el checker ya
            // garantizó que el default tipa contra el campo
            // (`check_field_defaults`), así que evaluarlo no puede producir
            // algo que rompa el `Value::Struct`/`Value::Variant` resultante.
            // `Env::new()` vacío, MISMO criterio que ya usa el default de un
            // parámetro de función/rpc más abajo (ver el otro `Env::new()`
            // en este archivo) -- un default no ve otros campos del mismo
            // literal ni el entorno que lo rodea.
            if let Some(fs) = ast_fields {
                for f in fs {
                    if evaluated.iter().any(|(n, _)| n == &f.name) {
                        continue;
                    }
                    if let Some(default) = &f.default {
                        let v = eval_expr(default, &Env::new(), db, fns, checker, sessions, current_token, step_budget)?;
                        evaluated.push((f.name.clone(), v));
                    }
                }
            }
            // `@validate(...)` (GRAMMAR.md §3.73): acá, no solo en el
            // decode del wire -- ver `apply_field_validators` para por qué
            // este es el punto que de verdad cubre el caso común. Después
            // de completar los defaults, así un default también se valida
            // (nada obliga a que el default del autor sea válido).
            if let Some(fs) = ast_fields {
                apply_field_validators(fs, &Value::Struct(evaluated.clone()), name)?;
            }
            match variant {
                Some(v) => {
                    Ok(Value::Variant { enum_name: name.clone(), variant: v.clone(), fields: evaluated })
                }
                None => Ok(Value::Struct(evaluated)),
            }
        }
        Expr::Match { scrutinee, arms } => {
            let v = eval_expr(scrutinee, env, db, fns, checker, sessions, current_token, step_budget)?;
            for arm in arms {
                if let Some(bindings) = try_match_pattern(&arm.pattern, &v, checker) {
                    let mut arm_env = env.clone();
                    arm_env.extend(bindings.into_iter().map(|(k, v)| (k, cell(v))));
                    if let Some(guard) = &arm.guard {
                        match eval_expr(guard, &arm_env, db, fns, checker, sessions, current_token, step_budget)? {
                            Value::Bool(true) => {}
                            // El patrón matcheó pero el guard no se cumplió --
                            // se sigue probando el resto de los arms, no se
                            // considera "sin match" (GRAMMAR.md §3.3).
                            Value::Bool(false) => continue,
                            other => return Err(err(format!("el guard de 'match' no es Bool en runtime: {other:?}"))),
                        }
                    }
                    return match &arm.body {
                        MatchArmBody::Expr(e) => eval_expr(e, &arm_env, db, fns, checker, sessions, current_token, step_budget),
                        MatchArmBody::Block(b) => eval_block(b, &arm_env, db, fns, checker, sessions, current_token, step_budget),
                    };
                }
            }
            // No debería pasar: el checker ya garantizó exhaustividad
            // (GRAMMAR.md §3.3) antes de que este código llegara a ejecutarse.
            Err(err("ningún arm de match coincidió — el checker debería haber impedido esto"))
        }
        Expr::If { cond, then_block, else_block } => {
            let c = eval_expr(cond, env, db, fns, checker, sessions, current_token, step_budget)?;
            match c {
                Value::Bool(true) => eval_block(then_block, env, db, fns, checker, sessions, current_token, step_budget),
                Value::Bool(false) => eval_block(else_block, env, db, fns, checker, sessions, current_token, step_budget),
                other => Err(err(format!("la condición de 'if' no es Bool en runtime: {other:?}"))),
            }
        }
        Expr::Binary { op, left, right } => eval_binary(*op, left, right, env, db, fns, checker, sessions, current_token, step_budget),
        Expr::Unary { op, operand } => eval_unary(*op, operand, env, db, fns, checker, sessions, current_token, step_budget),
        Expr::ArrayLit(items) => {
            let vs = items
                .iter()
                .map(|e| eval_expr(e, env, db, fns, checker, sessions, current_token, step_budget))
                .collect::<Result<Vec<_>, RuntimeError>>()?;
            Ok(Value::List(vs))
        }
        Expr::Index { base, index } => {
            let base_v = eval_expr(base, env, db, fns, checker, sessions, current_token, step_budget)?;
            let idx = as_int(&eval_expr(index, env, db, fns, checker, sessions, current_token, step_budget)?)?;
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
                .map(|e| eval_expr(e, env, db, fns, checker, sessions, current_token, step_budget))
                .collect::<Result<Vec<_>, RuntimeError>>()?;
            Ok(Value::Tuple(vs))
        }
        Expr::TupleIndex { base, index } => {
            let base_v = eval_expr(base, env, db, fns, checker, sessions, current_token, step_budget)?;
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
    left: &Spanned<Expr>,
    right: &Spanned<Expr>,
    env: &Env,
    db: &Db,
    fns: &Fns,
    checker: &Checker,
    sessions: &SessionStore,
    current_token: Option<&str>,
    step_budget: &Cell<u64>,
) -> Result<Value, RuntimeError> {
    use BinaryOp::*;
    // && / || cortocircuitan: el lado derecho no se evalúa si ya se sabe el
    // resultado, igual que en cualquier lenguaje con estos operadores.
    if matches!(op, And | Or) {
        let l = as_bool(&eval_expr(left, env, db, fns, checker, sessions, current_token, step_budget)?)?;
        return match (op, l) {
            (And, false) => Ok(Value::Bool(false)),
            (Or, true) => Ok(Value::Bool(true)),
            _ => Ok(Value::Bool(as_bool(&eval_expr(right, env, db, fns, checker, sessions, current_token, step_budget)?)?)),
        };
    }
    // `a ?? b` (GRAMMAR.md §3.9) también cortocircuita: `b` nunca se evalúa
    // si `a` ya tiene un valor -- mismo espíritu que `&&`/`||` arriba, útil
    // si `b` es una llamada costosa (una lectura a `db`, por ejemplo) que no
    // hace falta pagar cuando `a` ya resolvió el caso.
    if op == Coalesce {
        let l = eval_expr(left, env, db, fns, checker, sessions, current_token, step_budget)?;
        return match l {
            Value::Null => eval_expr(right, env, db, fns, checker, sessions, current_token, step_budget),
            other => Ok(other),
        };
    }

    let l = eval_expr(left, env, db, fns, checker, sessions, current_token, step_budget)?;
    let r = eval_expr(right, env, db, fns, checker, sessions, current_token, step_budget)?;
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
        And | Or | Coalesce => unreachable!("manejado arriba con cortocircuito"),
    }
}

#[allow(clippy::too_many_arguments)]
fn eval_unary(
    op: UnaryOp,
    operand: &Spanned<Expr>,
    env: &Env,
    db: &Db,
    fns: &Fns,
    checker: &Checker,
    sessions: &SessionStore,
    current_token: Option<&str>,
    step_budget: &Cell<u64>,
) -> Result<Value, RuntimeError> {
    let v = eval_expr(operand, env, db, fns, checker, sessions, current_token, step_budget)?;
    match op {
        UnaryOp::Neg => match v {
            Value::Int(n) => Ok(Value::Int(-n)),
            Value::Int64(n) => Ok(Value::Int64(-n)),
            Value::Float(n) => Ok(Value::Float(-n)),
            other => Err(err(format!("'-' unario requiere Int, Int64 o Float en runtime: {other:?}"))),
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
        (Value::Int64(a), Value::Int64(b)) => Ok(Value::Int64(int_op(a, b))),
        (Value::Float(a), Value::Float(b)) => Ok(Value::Float(float_op(a, b))),
        (l, r) => Err(err(format!(
            "operador aritmético requiere Int+Int, Int64+Int64 o Float+Float en runtime: {l:?} y {r:?}"
        ))),
    }
}

fn compare(l: Value, r: Value, accept: impl Fn(std::cmp::Ordering) -> bool) -> Result<Value, RuntimeError> {
    let ordering = match (&l, &r) {
        (Value::Int(a), Value::Int(b)) => a.cmp(b),
        (Value::Int64(a), Value::Int64(b)) => a.cmp(b),
        (Value::Timestamp(a), Value::Timestamp(b)) => a.cmp(b),
        (Value::Float(a), Value::Float(b)) => {
            a.partial_cmp(b).ok_or_else(|| err("comparación con NaN"))?
        }
        _ => return Err(err(format!("operador relacional requiere Int+Int, Int64+Int64, Float+Float o Timestamp+Timestamp: {l:?} y {r:?}"))),
    };
    Ok(Value::Bool(accept(ordering)))
}

#[allow(clippy::too_many_arguments)]
fn eval_args(
    args: &[Spanned<Expr>],
    env: &Env,
    db: &Db,
    fns: &Fns,
    checker: &Checker,
    sessions: &SessionStore,
    current_token: Option<&str>,
    step_budget: &Cell<u64>,
) -> Result<Vec<Value>, RuntimeError> {
    args.iter().map(|a| eval_expr(a, env, db, fns, checker, sessions, current_token, step_budget)).collect()
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
    step_budget: &Cell<u64>,
) -> Result<Value, RuntimeError> {
    let mut fn_env = Env::new();
    for (p, v) in decl.params.iter().zip(arg_vs) {
        fn_env.insert(p.name.clone(), cell(v));
    }
    eval_block(&decl.body, &fn_env, db, fns, checker, sessions, current_token, step_budget)
}

#[allow(clippy::too_many_arguments)]
fn call_rpc_decl(
    decl: &RpcDecl,
    arg_vs: Vec<Value>,
    db: &Db,
    fns: &Fns,
    checker: &Checker,
    sessions: &SessionStore,
    current_token: Option<&str>,
    step_budget: &Cell<u64>,
) -> Result<Value, RuntimeError> {
    let mut rpc_env = Env::new();
    for (p, v) in decl.params.iter().zip(arg_vs) {
        rpc_env.insert(p.name.clone(), cell(v));
    }
    eval_block(&decl.body, &rpc_env, db, fns, checker, sessions, current_token, step_budget)
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
        (LiteralPattern::Null, Value::Null) => true,
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
        Type::Int64 => matches!(v, Value::Int64(_)),
        Type::Timestamp => matches!(v, Value::Timestamp(_)),
        Type::Float => matches!(v, Value::Float(_)),
        Type::String => matches!(v, Value::Str(_)),
        Type::Uuid => matches!(v, Value::Uuid(_)),
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
    step_budget: &Cell<u64>,
) -> Result<Value, RuntimeError> {
    let mut call_env = captured_env.clone();
    for (name, v) in param_names.iter().zip(arg_vs) {
        call_env.insert(name.clone(), cell(v));
    }
    eval_block(body, &call_env, db, fns, checker, sessions, current_token, step_budget)
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
    step_budget: &Cell<u64>,
) -> Result<Value, RuntimeError> {
    match v {
        Value::FnRef(name) => {
            if name == "now" && !fns.contains_key("now") {
                if !arg_vs.is_empty() {
                    return Err(err("'now' no toma argumentos"));
                }
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
                return Ok(Value::Timestamp(now_ms));
            }
            if name == "assert" && !fns.contains_key("assert") {
                let cond = match arg_vs.first() {
                    Some(Value::Bool(b)) => *b,
                    _ => return Err(err("'assert' requiere un primer argumento Bool")),
                };
                if !cond {
                    let msg = match arg_vs.get(1) {
                        Some(Value::Str(s)) => format!("asercion fallida: {s}"),
                        _ => "asercion fallida".to_string(),
                    };
                    return Err(err(msg));
                }
                return Ok(Value::Null);
            }
            if name == "panic" && !fns.contains_key("panic") {
                let msg = match arg_vs.first() {
                    Some(Value::Str(s)) => format!("panic: {s}"),
                    _ => "panic".to_string(),
                };
                return Err(err(msg));
            }
            let decl = fns
                .get(name.as_str())
                .ok_or_else(|| err(format!("fn desconocida: '{name}'")))?;
            call_fn_decl(decl, arg_vs, db, fns, checker, sessions, current_token, step_budget)
        }
        Value::Closure(params, body, captured_env) => {
            call_closure(&params, &body, &captured_env, arg_vs, db, fns, checker, sessions, current_token, step_budget)
        }
        other => Err(err(format!("no se puede llamar un valor {other:?}"))),
    }
}

/// Escapa los 5 caracteres que HTML interpreta como marcado, no como texto
/// (GRAMMAR.md §3.45) -- mismo set que cualquier escapador de HTML estándar
/// (`html.escape` de Python, las guías de OWASP). `&` va PRIMERO a
/// propósito: si se escapara después de `<`/`>`/etc, el `&` que esas
/// mismas entidades acaban de insertar (`&amp;`, `&lt;`, ...) se
/// escaparía DE NUEVO, dejando `&amp;amp;` en vez de `&amp;`.
fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

/// Extrae `(name, value)` de cada `Value::Struct` de la lista que
/// `http.getWithHeaders`/`http.postWithHeaders` reciben como argumento de
/// headers (GRAMMAR.md §3.47) -- el checker ya garantizó la forma
/// (`http_header_type()`, checker.rs) vía subtipado estructural, así que el
/// error de acá es defensivo, no un caso esperado en la práctica (mismo
/// criterio que el `unwrap_or_else` de `@content_type` en server.rs).
/// Arma el `{status, headers, body}` que `getWithStatus`/`postWithStatus`
/// devuelven (GRAMMAR.md §3.60) a partir de una respuesta HTTP real -- se usa
/// tanto para el camino 2xx (`Ok`) como para el 4xx/5xx (`Err(Status(..))`
/// de `ureq`, que TAMBIÉN trae la `Response` completa, no solo el código).
/// Los headers se leen ANTES de `into_string()` porque esa llamada consume
/// `resp`.
fn ureq_response_to_value(resp: ureq::Response) -> Value {
    let status = resp.status() as i64;
    let headers: Vec<Value> = resp
        .headers_names()
        .iter()
        .filter_map(|name| resp.header(name).map(|value| (name.clone(), value.to_string())))
        .map(|(name, value)| Value::Struct(vec![("name".to_string(), Value::Str(name)), ("value".to_string(), Value::Str(value))]))
        .collect();
    let body = resp.into_string().unwrap_or_default();
    Value::Struct(vec![
        ("status".to_string(), Value::Int(status)),
        ("headers".to_string(), Value::List(headers)),
        ("body".to_string(), Value::Str(body)),
    ])
}

/// Arma y manda un email de verdad vía `lettre`, compartido por `smtp.send`
/// (1 destinatario, texto plano -- sin cambios de comportamiento), y las dos
/// variantes nuevas de la ronda `sendToMany`/`sendHtml` (GRAMMAR.md §3.63):
/// `to` siempre es una lista (de 1 o más), `is_html` elige el `Content-Type`
/// del cuerpo. Conexión y remitente salen del ENTORNO del proceso, nunca de
/// argumentos del rpc (GRAMMAR.md §3.43) -- mismo criterio que
/// `LINK_DATABASE_URL`: un `.link` no debería poder hardcodear ni filtrar
/// credenciales de un relay SMTP, y dejar que cualquier caller elija el
/// remitente abriría la puerta a spoofear el `From:` con datos de la
/// request.
fn send_email(to: &[String], subject: &str, body: &str, is_html: bool) -> Result<(), RuntimeError> {
    if to.is_empty() {
        return Err(err("smtp: 'to' no puede ser una lista vacía -- hace falta al menos un destinatario"));
    }
    let url = std::env::var("LINK_SMTP_URL")
        .map_err(|_| err("smtp: falta la variable de entorno LINK_SMTP_URL (ej. 'smtps://usuario:clave@smtp.proveedor.com')"))?;
    let from = std::env::var("LINK_SMTP_FROM").map_err(|_| err("smtp: falta la variable de entorno LINK_SMTP_FROM (la dirección remitente)"))?;

    let from_mbox: lettre::message::Mailbox =
        from.parse().map_err(|e| err(format!("smtp: LINK_SMTP_FROM ('{from}') no es una dirección válida: {e}")))?;

    let mut builder = lettre::Message::builder().from(from_mbox);
    for addr in to {
        let mbox: lettre::message::Mailbox =
            addr.parse().map_err(|e| err(format!("smtp: 'to' ('{addr}') no es una dirección válida: {e}")))?;
        builder = builder.to(mbox);
    }
    builder = builder.subject(subject);
    if is_html {
        builder = builder.header(lettre::message::header::ContentType::TEXT_HTML);
    }
    let email = builder.body(body.to_string()).map_err(|e| err(format!("smtp: no se pudo armar el mensaje: {e}")))?;

    use lettre::Transport;
    let mailer = lettre::SmtpTransport::from_url(&url).map_err(|e| err(format!("smtp: LINK_SMTP_URL inválida: {e}")))?.build();
    mailer.send(&email).map_err(|e| err(format!("smtp: no se pudo mandar el email: {e}")))?;
    Ok(())
}

fn http_headers_from_value(items: &[Value]) -> Result<Vec<(String, String)>, RuntimeError> {
    items
        .iter()
        .map(|item| {
            let Value::Struct(fields) = item else {
                return Err(err("cada header tiene que ser un struct con campos 'name' y 'value', ambos String"));
            };
            let name = fields.iter().find(|(n, _)| n == "name").map(|(_, v)| v);
            let value = fields.iter().find(|(n, _)| n == "value").map(|(_, v)| v);
            match (name, value) {
                (Some(Value::Str(n)), Some(Value::Str(v))) => Ok((n.clone(), v.clone())),
                _ => Err(err("cada header tiene que ser un struct con campos 'name' y 'value', ambos String")),
            }
        })
        .collect()
}

/// Bytes del CSPRNG del sistema (BCryptGenRandom en Windows, getrandom(2) en
/// Linux, random_get en WASI). Todo lo que en este lenguaje se llame
/// "aleatorio" o "seguro" sale de acá, nunca del reloj.
fn os_random_bytes(n: usize) -> Result<Vec<u8>, RuntimeError> {
    let mut buf = vec![0u8; n];
    getrandom::getrandom(&mut buf)
        .map_err(|e| err(format!("el sistema no pudo generar bytes aleatorios: {e}")))?;
    Ok(buf)
}

/// Comparación que no corta en el primer byte distinto: dos secretos se comparan
/// en tiempo constante para no filtrar, vía la duración, cuánto del valor
/// esperado adivinó quien está probando.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    // La diferencia de LARGO no es secreta (el formato del hash es público); lo
    // que no debe filtrarse es en qué posición difieren dos del mismo largo.
    if a.len() != b.len() {
        return false;
    }
    a.ct_eq(b).into()
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
    step_budget: &Cell<u64>,
) -> Result<Value, RuntimeError> {
    match receiver {
        Value::DbCollection(coll) => match method {

            "findWhere" => {
                let f = args.into_iter().next().ok_or_else(|| err("'findWhere' requiere 1 argumento"))?;
                let all_val = db.call(&coll, "all", vec![])?;
                let Value::List(items) = all_val else { return Ok(Value::List(vec![])); };
                let mut kept = Vec::new();
                for item in items {
                    if as_bool(&call_callable(f.clone(), vec![item.clone()], db, fns, checker, sessions, current_token, step_budget)?)? {
                        kept.push(item);
                    }
                }
                Ok(Value::List(kept))
            }
            "deleteWhere" => {
                let f = args.into_iter().next().ok_or_else(|| err("'deleteWhere' requiere 1 argumento"))?;
                let all_val = db.call(&coll, "all", vec![])?;
                let Value::List(items) = all_val else { return Ok(Value::Int(0)); };
                let mut count = 0i64;
                for item in items {
                    if as_bool(&call_callable(f.clone(), vec![item.clone()], db, fns, checker, sessions, current_token, step_budget)?)? {
                        if let Value::Struct(fields) = &item {
                            if let Some((_, Value::Int(id))) = fields.iter().find(|(n, _)| n == "id") {
                                if let Ok(Value::Bool(true)) = db.call(&coll, "delete", vec![Value::Int(*id)]) {
                                    count += 1;
                                }
                            }
                        }
                    }
                }
                Ok(Value::Int(count))
            }
            // GRAMMAR.md §3.75: `matchFn` corre en el intérprete sobre
            // TODAS las filas (mismo criterio que `findWhere`/`deleteWhere`
            // arriba -- no empujado a SQL, ver PLAN.md §9.3.4), se queda con
            // la PRIMERA que matchea. `updateFn` se llama con la fila
            // EXISTENTE completa y devuelve un valor `Omit<T,"id">`
            // COMPLETO (no un `Patch<T>` parcial -- ver el porqué en
            // checker.rs::check_db_method) que se aplica entero vía
            // `applyPatch` sobre el MISMO id -- nunca borra e inserta de
            // nuevo, así que el id de la fila actualizada no cambia.
            "upsert" => {
                let mut it = args.into_iter();
                let (Some(match_fn), Some(insert_value), Some(update_fn)) = (it.next(), it.next(), it.next()) else {
                    return Err(err("'upsert' requiere 3 argumentos (matchFn, insertValue, updateFn)"));
                };
                let all_val = db.call(&coll, "all", vec![])?;
                let Value::List(items) = all_val else {
                    return Err(err("'upsert': no se pudo leer la colección"));
                };
                let mut existing = None;
                for item in items {
                    if as_bool(&call_callable(match_fn.clone(), vec![item.clone()], db, fns, checker, sessions, current_token, step_budget)?)? {
                        existing = Some(item);
                        break;
                    }
                }
                match existing {
                    Some(row) => {
                        let Value::Struct(row_fields) = &row else {
                            return Err(err("'upsert': la fila existente no es un struct"));
                        };
                        let Some((_, Value::Int(id))) = row_fields.iter().find(|(n, _)| n == "id") else {
                            return Err(err("'upsert': la fila existente no tiene 'id'"));
                        };
                        let id = *id;
                        let new_value =
                            call_callable(update_fn, vec![row], db, fns, checker, sessions, current_token, step_budget)?;
                        db.call(&coll, "applyPatch", vec![Value::Int(id), new_value])
                    }
                    None => db.call(&coll, "insert", vec![insert_value]),
                }
            }
            _ => db.call(&coll, method, args),
        },

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
                    if as_bool(&call_callable(f.clone(), vec![item.clone()], db, fns, checker, sessions, current_token, step_budget)?)? {
                        kept.push(item);
                    }
                }
                Ok(Value::List(kept))
            }
            "map" => {
                let f = args.into_iter().next().ok_or_else(|| err("'map' requiere 1 argumento"))?;
                let mut mapped = Vec::with_capacity(items.len());
                for item in items {
                    mapped.push(call_callable(f.clone(), vec![item], db, fns, checker, sessions, current_token, step_budget)?);
                }
                Ok(Value::List(mapped))
            }
            "join" => {
                let sep = match args.first() {
                    Some(Value::Str(s)) => s.as_str(),
                    _ => return Err(err("'join' requiere un separador String")),
                };
                let rendered: Vec<String> = items.iter().map(|item| match item {
                    Value::Str(s) => s.clone(),
                    Value::Int(n) => n.to_string(),
                    Value::Float(f) => f.to_string(),
                    Value::Bool(b) => b.to_string(),
                    other => format!("{other:?}"),
                }).collect();
                Ok(Value::Str(rendered.join(sep)))
            }
            "reverse" => {
                let mut rev = items;
                rev.reverse();
                Ok(Value::List(rev))
            }
            other => Err(err(format!("método de lista desconocido: '{other}'"))),
        },
        Value::Int(n) => match method {
            "toFloat" => Ok(Value::Float(n as f64)),
            "toInt64" => Ok(Value::Int64(n)),
            "toString" => Ok(Value::Str(n.to_string())),
            other => Err(err(format!("método desconocido sobre Int: '{other}'"))),
        },
        Value::Int64(n) => match method {
            "toInt" => Ok(Value::Int(n)),
            "toString" => Ok(Value::Str(n.to_string())),
            other => Err(err(format!("método desconocido sobre Int64: '{other}'"))),
        },
        Value::Float(n) => match method {
            "toInt" => Ok(Value::Int(n as i64)), // trunca hacia cero, no redondea (GRAMMAR.md §3.8)
            "toString" => Ok(Value::Str(n.to_string())),
            other => Err(err(format!("método desconocido sobre Float: '{other}'"))),
        },
        Value::Bool(b) => match method {
            "toString" => Ok(Value::Str(b.to_string())),
            other => Err(err(format!("método desconocido sobre Bool: '{other}'"))),
        },
        Value::Uuid(s) => match method {
            "toString" => Ok(Value::Str(s)),
            other => Err(err(format!("método desconocido sobre Uuid: '{other}' -- '.toString()' lo baja a String"))),
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
            "startsWith" => {
                let needle = match args.first() {
                    Some(Value::Str(n)) => n,
                    _ => return Err(err("'startsWith' requiere un argumento String")),
                };
                Ok(Value::Bool(s.starts_with(needle.as_str())))
            }
            "endsWith" => {
                let needle = match args.first() {
                    Some(Value::Str(n)) => n,
                    _ => return Err(err("'endsWith' requiere un argumento String")),
                };
                Ok(Value::Bool(s.ends_with(needle.as_str())))
            }
            "trim" => Ok(Value::Str(s.trim().to_string())),
            "toUpper" => Ok(Value::Str(s.to_uppercase())),
            "toLower" => Ok(Value::Str(s.to_lowercase())),
            "escapeHtml" => Ok(Value::Str(escape_html(&s))),
            other => Err(err(format!("método desconocido sobre String: '{other}'"))),
        },
        Value::Timestamp(ms) => match method {
            "toMillis" => Ok(Value::Int64(ms)),
            "diffMillis" => {
                let other_ms = match args.first() {
                    Some(Value::Timestamp(t)) => *t,
                    _ => return Err(err("'diffMillis' requiere un argumento Timestamp")),
                };
                Ok(Value::Int64(ms - other_ms))
            }
            "toIsoString" => Ok(Value::Str(timestamp::format_iso8601_millis(ms))),
            other => Err(err(format!("método desconocido sobre Timestamp: '{other}'"))),
        },
        Value::Math => match method {
            "sqrt" => {
                let x = as_float(args.first().ok_or_else(|| err("math.sqrt requiere 1 argumento"))?)?;
                Ok(Value::Float(x.sqrt()))
            }
            "abs" => {
                let x = as_float(args.first().ok_or_else(|| err("math.abs requiere 1 argumento"))?)?;
                Ok(Value::Float(x.abs()))
            }
            "floor" => {
                let x = as_float(args.first().ok_or_else(|| err("math.floor requiere 1 argumento"))?)?;
                Ok(Value::Int(x.floor() as i64))
            }
            "ceil" => {
                let x = as_float(args.first().ok_or_else(|| err("math.ceil requiere 1 argumento"))?)?;
                Ok(Value::Int(x.ceil() as i64))
            }
            "round" => {
                let x = as_float(args.first().ok_or_else(|| err("math.round requiere 1 argumento"))?)?;
                Ok(Value::Int(x.round() as i64))
            }
            "min" => {
                let a = as_float(args.first().ok_or_else(|| err("math.min requiere 2 argumentos"))?)?;
                let b = as_float(args.get(1).ok_or_else(|| err("math.min requiere 2 argumentos"))?)?;
                Ok(Value::Float(a.min(b)))
            }
            "max" => {
                let a = as_float(args.first().ok_or_else(|| err("math.max requiere 2 argumentos"))?)?;
                let b = as_float(args.get(1).ok_or_else(|| err("math.max requiere 2 argumentos"))?)?;
                Ok(Value::Float(a.max(b)))
            }
            "pow" => {
                let a = as_float(args.first().ok_or_else(|| err("math.pow requiere 2 argumentos"))?)?;
                let b = as_float(args.get(1).ok_or_else(|| err("math.pow requiere 2 argumentos"))?)?;
                Ok(Value::Float(a.powf(b)))
            }
            other => Err(err(format!("método desconocido sobre math: '{other}'"))),
        },
        Value::Crypto => match method {
            "hashSha256" => {
                let data = match args.first() {
                    Some(Value::Str(s)) => s,
                    _ => return Err(err("crypto.hashSha256 requiere un argumento String")),
                };
                use sha2::{Digest, Sha256};
                let mut hasher = Sha256::new();
                hasher.update(data.as_bytes());
                let hex_str: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
                Ok(Value::Str(hex_str))
            }
            "hmacSha256" => {
                let (secret, message) = match (args.first(), args.get(1)) {
                    (Some(Value::Str(s)), Some(Value::Str(m))) => (s, m),
                    _ => return Err(err("crypto.hmacSha256 requiere dos argumentos String (secret, message)")),
                };
                use hmac::{Hmac, Mac};
                use sha2::Sha256;
                let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
                    .map_err(|e| err(format!("clave HMAC inválida: {e}")))?;
                mac.update(message.as_bytes());
                let hex_str: String = mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect();
                Ok(Value::Str(hex_str))
            }
            "randomToken" => {
                let length = match args.first() {
                    Some(Value::Int(n)) => *n as usize,
                    _ => return Err(err("crypto.randomToken requiere un argumento Int")),
                };
                // El token sale del CSPRNG del sistema. La versión anterior era
                // SHA-256 del reloj (SystemTime::now().as_nanos()), lo que hacía
                // que un token fuese adivinable para quien pudiera acotar el
                // instante en que se emitió -- y que dos llamadas dentro del
                // mismo nanosegundo devolvieran el MISMO token.
                let length = length.max(8);
                let bytes = os_random_bytes(length.div_ceil(2))?;
                let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
                Ok(Value::Str(hex.chars().take(length).collect()))
            }
            "hashPassword" => {
                let pwd = match args.first() {
                    Some(Value::Str(s)) => s,
                    _ => return Err(err("crypto.hashPassword requiere un argumento String")),
                };
                // Argon2id con sal aleatoria POR CONTRASEÑA, en formato PHC
                // ($argon2id$v=19$m=...,t=...,p=...$sal$hash).
                //
                // Lo anterior era un solo SHA-256 sobre la constante
                // "link_salt_2026" concatenada con la contraseña: la MISMA sal
                // para todos los programas escritos en este lenguaje, sin
                // iteraciones. Dos usuarios con la misma contraseña producían el
                // mismo hash, y una única rainbow table servía contra cualquier
                // aplicación Link que existiera. Un KDF con costo de memoria
                // configurable (Argon2id es el que recomienda el RFC 9106 para
                // contraseñas) es la diferencia entre "hashear" y "resistir a
                // quien se robó la base".
                //
                // Los parámetros de costo (m/t/p) salen de `db.argon2_params()`
                // -- default de la crate hasta que `--argon2-memory-kib`/
                // `--argon2-iterations` los suba (GRAMMAR.md §3.55). El hash
                // PHC resultante los EMBEBE (`$argon2id$v=19$m=...,t=...,p=...$`),
                // así que `verifyPassword` no necesita saber cuáles eran --
                // los lee del propio hash guardado.
                use argon2::password_hash::{PasswordHasher, SaltString};
                use argon2::Argon2;
                let salt_bytes = os_random_bytes(16)?;
                let salt = SaltString::encode_b64(&salt_bytes)
                    .map_err(|e| err(format!("no se pudo generar la sal: {e}")))?;
                let argon2 = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, db.argon2_params());
                let hash = argon2
                    .hash_password(pwd.as_bytes(), &salt)
                    .map_err(|e| err(format!("no se pudo hashear la contraseña: {e}")))?;
                Ok(Value::Str(hash.to_string()))
            }
            "verifyPassword" => {
                let pwd = match args.first() {
                    Some(Value::Str(s)) => s,
                    _ => return Err(err("crypto.verifyPassword requiere contraseña")),
                };
                let stored = match args.get(1) {
                    Some(Value::Str(s)) => s,
                    _ => return Err(err("crypto.verifyPassword requiere hash")),
                };

                // Los hashes viejos (sha256$<sal>$<hex>) siguen verificando: si
                // esto los rechazara, actualizar el compilador dejaría afuera a
                // todos los usuarios ya registrados de una app en producción. Se
                // aceptan para poder migrar -- la próxima vez que esa contraseña
                // se guarde, hashPassword la escribe ya en Argon2id.
                if let Some(rest) = stored.strip_prefix("sha256$") {
                    let Some((salt, expected)) = rest.split_once('$') else {
                        return Ok(Value::Bool(false));
                    };
                    use sha2::{Digest, Sha256};
                    let mut hasher = Sha256::new();
                    hasher.update(salt.as_bytes());
                    hasher.update(pwd.as_bytes());
                    let hex_str: String =
                        hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
                    // Comparación en tiempo constante: el == de String corta en
                    // el primer byte distinto, y ese tiempo le dice a quien mide
                    // cuántos caracteres del hash acertó.
                    return Ok(Value::Bool(constant_time_eq(
                        hex_str.as_bytes(),
                        expected.as_bytes(),
                    )));
                }

                use argon2::password_hash::{PasswordHash, PasswordVerifier};
                use argon2::Argon2;
                let Ok(parsed) = PasswordHash::new(stored) else {
                    return Ok(Value::Bool(false));
                };
                Ok(Value::Bool(
                    Argon2::default()
                        .verify_password(pwd.as_bytes(), &parsed)
                        .is_ok(),
                ))
            }
            "isLegacyHash" => {
                let hash = match args.first() {
                    Some(Value::Str(s)) => s,
                    _ => return Err(err("crypto.isLegacyHash requiere un argumento String")),
                };
                // "Legado" es exactamente lo que `verifyPassword` sigue
                // aceptando por compatibilidad (`sha256$<sal>$<hex>`, §3.34)
                // -- cualquier otra cosa que no sea un hash Argon2id de
                // verdad tampoco cuenta como "legado migrable": es un valor
                // que ni siquiera `verifyPassword` va a reconocer.
                Ok(Value::Bool(hash.starts_with("sha256$")))
            }
            "uuid" => {
                // UUIDv4 de verdad: 122 bits del CSPRNG del sistema. Antes era
                // SHA-256 del reloj disfrazado de v4 -- dos llamadas en el mismo
                // nanosegundo devolvían el mismo "identificador único".
                let b = os_random_bytes(16)?;
                let s = format!(
                    "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-4{:01x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
                    b[0], b[1], b[2], b[3],
                    b[4], b[5],
                    b[6] & 0x0f, b[7],
                    (b[8] & 0x3f) | 0x80, b[9],
                    b[10], b[11], b[12], b[13], b[14], b[15]
                );
                Ok(Value::Uuid(s))
            }
            "randomInt" => {
                let (min, max) = match (args.first(), args.get(1)) {
                    (Some(Value::Int(a)), Some(Value::Int(b))) => (*a, *b),
                    _ => return Err(err("crypto.randomInt requiere dos argumentos Int (min, max)")),
                };
                if min > max {
                    return Err(err("crypto.randomInt: min no puede ser mayor que max"));
                }
                // Rango inclusivo [min, max] del CSPRNG del sistema, sin sesgo de
                // módulo: se descarta cualquier u64 que caiga en el resto que no
                // divide exacto al tamaño del rango -- si no, los primeros valores
                // del rango saldrían levemente más probables que los últimos. Hace
                // falta para un OTP numérico de verdad (randomToken da hex, no
                // dígitos), donde un sesgo mediría en intentos de fuerza bruta.
                let range = (max as i128 - min as i128) as u128 + 1;
                let offset = if range > u64::MAX as u128 {
                    let bytes = os_random_bytes(8)?;
                    u64::from_le_bytes(bytes.try_into().unwrap()) as u128
                } else {
                    let range = range as u64;
                    let limit = u64::MAX - (u64::MAX % range);
                    loop {
                        let bytes = os_random_bytes(8)?;
                        let n = u64::from_le_bytes(bytes.try_into().unwrap());
                        if n < limit {
                            break (n % range) as u128;
                        }
                    }
                };
                Ok(Value::Int((min as i128 + offset as i128) as i64))
            }
            "timingSafeEqual" => {
                let (a, b) = match (args.first(), args.get(1)) {
                    (Some(Value::Str(a)), Some(Value::Str(b))) => (a, b),
                    _ => return Err(err("crypto.timingSafeEqual requiere dos argumentos String")),
                };
                // Expone `constant_time_eq` (ya usado internamente por
                // `verifyPassword`) al código de usuario -- comparar un secreto de
                // webhook o una API key con `==` filtra, vía cuánto tarda la
                // respuesta, en qué posición difiere del valor esperado.
                Ok(Value::Bool(constant_time_eq(a.as_bytes(), b.as_bytes())))
            }
            other => Err(err(format!("método desconocido sobre crypto: '{other}'"))),
        },
        Value::Json => match method {
            "parse" => {
                let text = match args.first() {
                    Some(Value::Str(s)) => s,
                    _ => return Err(err("json.parse requiere un argumento String")),
                };
                let parsed: serde_json::Value = serde_json::from_str(text)
                    .map_err(|e| err(format!("error al parsear JSON: {e}")))?;
                Ok(json_to_value(&parsed))
            }
            "stringify" => {
                let val = args.first().ok_or_else(|| err("json.stringify requiere 1 argumento"))?;
                let json_v = value_to_json(val, &std::collections::HashSet::new());
                let s = serde_json::to_string(&json_v)
                    .map_err(|e| err(format!("error al serializar a JSON: {e}")))?;
                Ok(Value::Str(s))
            }
            other => Err(err(format!("método desconocido sobre json: '{other}'"))),
        },
        Value::Base64 => match method {
            "encode" => {
                let data = match args.first() {
                    Some(Value::Str(s)) => s,
                    _ => return Err(err("base64.encode requiere un argumento String")),
                };
                use base64::Engine;
                let encoded = base64::engine::general_purpose::STANDARD.encode(data.as_bytes());
                Ok(Value::Str(encoded))
            }
            "decode" => {
                let data = match args.first() {
                    Some(Value::Str(s)) => s,
                    _ => return Err(err("base64.decode requiere un argumento String")),
                };
                use base64::Engine;
                let decoded_bytes = base64::engine::general_purpose::STANDARD.decode(data.as_bytes())
                    .map_err(|e| err(format!("error al decodificar base64: {e}")))?;
                let s = String::from_utf8(decoded_bytes)
                    .map_err(|e| err(format!("la secuencia decodificada no es UTF-8 válido: {e}")))?;
                Ok(Value::Str(s))
            }
            other => Err(err(format!("método desconocido sobre base64: '{other}'"))),
        },
        Value::Env => match method {
            "get" => {
                let name = match args.first() {
                    Some(Value::Str(s)) => s,
                    _ => return Err(err("env.get requiere un argumento String")),
                };
                // `Ok` -> presente y UTF-8 válido -> Some. Cualquier otro
                // caso (ausente, o presente pero no-UTF-8) es `None`: para
                // un programa c-script "no está seteada" y "no se puede leer
                // como texto" son la misma cosa práctica -- no hay ningún
                // uso real que distinga entre las dos.
                match std::env::var(name.as_str()) {
                    Ok(v) => Ok(Value::Str(v)),
                    Err(_) => Ok(Value::Null),
                }
            }
            other => Err(err(format!("método desconocido sobre env: '{other}'"))),
        },
        Value::Request => match method {
            "rawBody" => Ok(Value::Str(db.current_request_body())),
            "header" => {
                let name = match args.first() {
                    Some(Value::Str(s)) => s,
                    _ => return Err(err("request.header requiere un argumento String")),
                };
                Ok(db.current_request_header(name).map(Value::Str).unwrap_or(Value::Null))
            }
            other => Err(err(format!("método desconocido sobre request: '{other}'"))),
        },
        Value::Smtp => match method {
            "send" => {
                let (Some(Value::Str(to)), Some(Value::Str(subject)), Some(Value::Str(body))) =
                    (args.first(), args.get(1), args.get(2))
                else {
                    return Err(err("smtp.send requiere 3 argumentos String (to, subject, body)"));
                };
                send_email(std::slice::from_ref(to), subject, body, false)?;
                Ok(Value::Null)
            }
            "sendToMany" => {
                let (Some(Value::List(to)), Some(Value::Str(subject)), Some(Value::Str(body))) =
                    (args.first(), args.get(1), args.get(2))
                else {
                    return Err(err("smtp.sendToMany requiere (to: String[], subject: String, body: String)"));
                };
                let to: Vec<String> = to
                    .iter()
                    .map(|v| match v {
                        Value::Str(s) => Ok(s.clone()),
                        other => Err(err(format!("smtp.sendToMany: 'to' tiene que ser una lista de String, se encontró {other:?}"))),
                    })
                    .collect::<Result<_, _>>()?;
                send_email(&to, subject, body, false)?;
                Ok(Value::Null)
            }
            "sendHtml" => {
                let (Some(Value::List(to)), Some(Value::Str(subject)), Some(Value::Str(html))) =
                    (args.first(), args.get(1), args.get(2))
                else {
                    return Err(err("smtp.sendHtml requiere (to: String[], subject: String, html: String)"));
                };
                let to: Vec<String> = to
                    .iter()
                    .map(|v| match v {
                        Value::Str(s) => Ok(s.clone()),
                        other => Err(err(format!("smtp.sendHtml: 'to' tiene que ser una lista de String, se encontró {other:?}"))),
                    })
                    .collect::<Result<_, _>>()?;
                send_email(&to, subject, html, true)?;
                Ok(Value::Null)
            }
            other => Err(err(format!("método desconocido sobre smtp: '{other}'"))),
        },
        Value::Response => match method {
            "setStatus" => {
                let code = match args.first() {
                    Some(Value::Int(n)) => *n,
                    _ => return Err(err("response.setStatus requiere un argumento Int")),
                };
                if !(100..=599).contains(&code) {
                    return Err(err(format!("response.setStatus({code}): un status HTTP válido está entre 100 y 599")));
                }
                db.set_response_status(code as u16);
                Ok(Value::Null)
            }
            other => Err(err(format!("método desconocido sobre response: '{other}'"))),
        },
        Value::Http => match method {
            "get" => {
                let url = match args.first() {
                    Some(Value::Str(s)) => s,
                    _ => return Err(err("http.get requiere un argumento URL String")),
                };
                match ureq::get(url).call() {
                    Ok(resp) => {
                        let text = resp.into_string().unwrap_or_default();
                        Ok(Value::Str(text))
                    }
                    Err(e) => Err(err(format!("error HTTP al hacer GET a {url}: {e}"))),
                }
            }
            "post" => {
                let url = match args.first() {
                    Some(Value::Str(s)) => s,
                    _ => return Err(err("http.post requiere un argumento URL String")),
                };
                let body = match args.get(1) {
                    Some(Value::Str(s)) => s,
                    _ => return Err(err("http.post requiere un argumento Body String")),
                };
                match ureq::post(url).send_string(body) {
                    Ok(resp) => {
                        let text = resp.into_string().unwrap_or_default();
                        Ok(Value::Str(text))
                    }
                    Err(e) => Err(err(format!("error HTTP al hacer POST a {url}: {e}"))),
                }
            }
            "getWithHeaders" => {
                let url = match args.first() {
                    Some(Value::Str(s)) => s,
                    _ => return Err(err("http.getWithHeaders requiere un argumento URL String")),
                };
                let headers = match args.get(1) {
                    Some(Value::List(items)) => http_headers_from_value(items)?,
                    _ => return Err(err("http.getWithHeaders requiere una lista de headers como segundo argumento")),
                };
                let mut req = ureq::get(url);
                for (name, value) in &headers {
                    req = req.set(name, value);
                }
                match req.call() {
                    Ok(resp) => {
                        let text = resp.into_string().unwrap_or_default();
                        Ok(Value::Str(text))
                    }
                    Err(e) => Err(err(format!("error HTTP al hacer GET a {url}: {e}"))),
                }
            }
            "getWithStatus" => {
                let url = match args.first() {
                    Some(Value::Str(s)) => s,
                    _ => return Err(err("http.getWithStatus requiere un argumento URL String")),
                };
                let headers = match args.get(1) {
                    Some(Value::List(items)) => http_headers_from_value(items)?,
                    _ => return Err(err("http.getWithStatus requiere una lista de headers como segundo argumento")),
                };
                let mut req = ureq::get(url);
                for (name, value) in &headers {
                    req = req.set(name, value);
                }
                // A diferencia de `get`/`getWithHeaders`, un 4xx/5xx NO es un
                // error de runtime acá -- es justamente el dato que este
                // método existe para exponer. `ureq::Error::Status` trae la
                // `Response` completa (status + headers + body), no solo el
                // código; solo un error de RED de verdad (DNS, conexión
                // rechazada, timeout) sigue siendo `Err`.
                match req.call() {
                    Ok(resp) => Ok(ureq_response_to_value(resp)),
                    Err(ureq::Error::Status(_, resp)) => Ok(ureq_response_to_value(resp)),
                    Err(e) => Err(err(format!("error de red al hacer GET a {url}: {e}"))),
                }
            }
            "postWithStatus" => {
                let url = match args.first() {
                    Some(Value::Str(s)) => s,
                    _ => return Err(err("http.postWithStatus requiere un argumento URL String")),
                };
                let body = match args.get(1) {
                    Some(Value::Str(s)) => s,
                    _ => return Err(err("http.postWithStatus requiere un argumento Body String")),
                };
                let headers = match args.get(2) {
                    Some(Value::List(items)) => http_headers_from_value(items)?,
                    _ => return Err(err("http.postWithStatus requiere una lista de headers como tercer argumento")),
                };
                let mut req = ureq::post(url);
                for (name, value) in &headers {
                    req = req.set(name, value);
                }
                match req.send_string(body) {
                    Ok(resp) => Ok(ureq_response_to_value(resp)),
                    Err(ureq::Error::Status(_, resp)) => Ok(ureq_response_to_value(resp)),
                    Err(e) => Err(err(format!("error de red al hacer POST a {url}: {e}"))),
                }
            }
            "postWithHeaders" => {
                let url = match args.first() {
                    Some(Value::Str(s)) => s,
                    _ => return Err(err("http.postWithHeaders requiere un argumento URL String")),
                };
                let body = match args.get(1) {
                    Some(Value::Str(s)) => s,
                    _ => return Err(err("http.postWithHeaders requiere un argumento Body String")),
                };
                let headers = match args.get(2) {
                    Some(Value::List(items)) => http_headers_from_value(items)?,
                    _ => return Err(err("http.postWithHeaders requiere una lista de headers como tercer argumento")),
                };
                let mut req = ureq::post(url);
                for (name, value) in &headers {
                    req = req.set(name, value);
                }
                match req.send_string(body) {
                    Ok(resp) => {
                        let text = resp.into_string().unwrap_or_default();
                        Ok(Value::Str(text))
                    }
                    Err(e) => Err(err(format!("error HTTP al hacer POST a {url}: {e}"))),
                }
            }
            other => Err(err(format!("método desconocido sobre http: '{other}'"))),
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
            "createSessionWithId" => {
                let mut it = args.into_iter();
                let role = it.next().ok_or_else(|| err("createSessionWithId requiere 2 argumentos (role, userId)"))?;
                let user_id_val = it.next().ok_or_else(|| err("createSessionWithId requiere 2 argumentos (role, userId)"))?;
                let Value::Variant { enum_name, variant, .. } = role else {
                    return Err(err("createSessionWithId requiere un valor de un enum declarado como primer argumento"));
                };
                let user_id = as_int(&user_id_val)?;
                Ok(Value::Str(sessions.create_with_user_id(enum_name, variant, Some(user_id))))
            }
            "destroySession" => {
                if let Some(tok) = current_token {
                    sessions.destroy(tok);
                }
                Ok(Value::Null)
            }
            // `null` para "sin sesión" y "token inválido/vencido" por
            // igual, a propósito -- mismo criterio de indistinguibilidad
            // que ya rige el 401 de `check_auth_gate` (GRAMMAR.md §3.50):
            // un cuerpo de rpc no debería poder distinguir "nadie se
            // autenticó" de "algo estaba mal con el token" mirando esto.
            // Disponible SIEMPRE, no solo bajo `@requires`/`@authenticated`
            // -- mismo criterio que `request.rawBody()`/`request.header()`
            // (§3.38), que tampoco están atados a una anotación.
            "currentRole" => {
                let role = current_token.and_then(|tok| sessions.role_for(tok)).map(|(_, variant)| variant);
                Ok(role.map(Value::Str).unwrap_or(Value::Null))
            }
            "currentUserId" => {
                let user_id = current_token.and_then(|tok| sessions.user_id_for(tok));
                Ok(user_id.map(Value::Int).unwrap_or(Value::Null))
            }
            other => Err(err(format!("método desconocido sobre auth: '{other}'"))),
        },
        Value::Service(s_name) => {
            let service = checker.service_decls.get(&s_name).ok_or_else(|| err(format!("service desconocido: '{s_name}'")))?;
            let rpc = service.members.iter().find_map(|m| match m {
                Member::Rpc(r) | Member::Stream(r) if r.name == method => Some(r),
                _ => None,
            }).ok_or_else(|| err(format!("rpc desconocido: '{s_name}.{method}'")))?;
            call_rpc_decl(rpc, args, db, fns, checker, sessions, current_token, step_budget)
        }
        other => Err(err(format!("no se puede invocar '{method}' sobre {other:?}"))),
    }
}

pub(crate) fn as_int(v: &Value) -> Result<i64, RuntimeError> {
    match v {
        Value::Int(n) => Ok(*n),
        other => Err(err(format!("se esperaba un entero, se encontró {other:?}"))),
    }
}

pub(crate) fn as_float(v: &Value) -> Result<f64, RuntimeError> {
    match v {
        Value::Float(n) => Ok(*n),
        Value::Int(n) => Ok(*n as f64),
        other => Err(err(format!("se esperaba un Float, se encontró {other:?}"))),
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
#[derive(Debug, Clone, PartialEq)]
pub struct TestSummary {
    pub total: usize,
    pub passed: usize,
    pub failed: Vec<(String, String)>,
}

/// Ejecuta todos los bloques `test "nombre" { ... }` declarados en el programa
/// (PLAN.md §5, Eje 2). Cada test corre con su propio entorno y base de datos
/// SQLite en memoria (`:memory:`) aislada, asegurando independencia total.
pub fn run_program_tests(program: &Program) -> Result<TestSummary, RuntimeError> {
    let (checker, errors) = Checker::check_program_full(program, &[]);
    if let Some(first) = errors.into_iter().next() {
        return Err(RuntimeError::new(first.message));
    }
    let fns: Fns = program
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Fn(f) => Some((f.name.clone(), f)),
            _ => None,
        })
        .collect();

    let tests: Vec<&TestDecl> = program
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Test(t) => Some(t),
            _ => None,
        })
        .collect();

    let mut passed = 0;
    let mut failed = Vec::new();

    for test in &tests {
        let db = Db::new(program, std::path::Path::new(":memory:"));
        let sessions = SessionStore::new();
        let step_budget = Cell::new(1_000_000);
        let env = Env::new();
        match eval_block(&test.body, &env, &db, &fns, &checker, &sessions, None, &step_budget) {
            Ok(_) => passed += 1,
            Err(e) => failed.push((test.name.clone(), e.message)),
        }
    }

    Ok(TestSummary {
        total: tests.len(),
        passed,
        failed,
    })
}

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
    // Una sola cota de iteraciones de `while` por invocación (GRAMMAR.md
    // §3.15) -- creada acá, el ORIGEN, y enhebrada por todo el árbol de
    // evaluación de abajo (incluida la de los valores por default de los
    // parámetros). `Cell`, no `Mutex`/`Atomic*`: el intérprete corre
    // siempre en este único hilo.
    let step_budget = Cell::new(0u64);
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
                    eval_expr(default_expr, &Env::new(), db, &fns, &checker, sessions, current_token, &step_budget)?
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

    let result = eval_block(&rpc.body, &env, db, &fns, &checker, sessions, current_token, &step_budget)?;
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
/// `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` -- 36 caracteres, hex en las 32
/// posiciones que no son guión, guiones exactamente en 8/13/18/23 (índices
/// 0-based). Sin crate de regex nueva (`validators.ts` sí usa una regex real
/// de TS -- acá, sin esa dependencia, un escaneo manual de byte es más
/// simple que sumar `regex` solo para esto). No valida el nibble de
/// versión/variante -- mismo criterio que `validators.ts`, acepta cualquier
/// UUID RFC 4122 real, rechaza basura con la forma general equivocada.
fn is_canonical_uuid(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (i, &b) in bytes.iter().enumerate() {
        let expected_dash = matches!(i, 8 | 13 | 18 | 23);
        if expected_dash {
            if b != b'-' {
                return false;
            }
        } else if !b.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

pub(crate) fn json_to_typed_value(
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
        // Siempre string en el wire, nunca un número JSON nativo -- eso es
        // justo lo que evita la pérdida de precisión que Int64 existe para
        // resolver (GRAMMAR.md §3.30): un número JSON grande ya perdió
        // precisión del lado del cliente JS antes de llegar acá.
        Type::Int64 => j
            .as_str()
            .and_then(|s| s.parse::<i64>().ok())
            .map(Value::Int64)
            .ok_or_else(mismatch),
        // String ISO-8601 de forma fija (GRAMMAR.md §3.31) -- rechaza
        // cualquier otra variante (offset de timezone, sin milisegundos,
        // fecha de calendario inexistente) en vez de aceptarla a medias.
        Type::Timestamp => j
            .as_str()
            .and_then(timestamp::parse_iso8601_millis)
            .map(Value::Timestamp)
            .ok_or_else(mismatch),
        Type::Float => j.as_f64().map(Value::Float).ok_or_else(mismatch),
        Type::String => j.as_str().map(|s| Value::Str(s.to_string())).ok_or_else(mismatch),
        // Forma canónica 8-4-4-4-12 en hex (GRAMMAR.md §3.70) -- acá es
        // donde de verdad se hace cumplir; `crypto.uuid()` ya produce algo
        // válido por construcción, así que este es el único punto de
        // entrada real para un Uuid que puede estar mal formado (un cliente
        // mandando cualquier string).
        Type::Uuid => j
            .as_str()
            .filter(|s| is_canonical_uuid(s))
            .map(|s| Value::Uuid(s.to_string()))
            .ok_or_else(mismatch),
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
        Type::Struct { fields, name } => {
            let v = struct_from_json(j, fields, checker, path, &mismatch)?;
            // `@validate(...)` (GRAMMAR.md §3.73): `Type::Struct` es
            // ESTRUCTURAL (fields, sin anotaciones) -- `name` (solo para
            // mensajes de error en el resto del checker, ver types.rs) es
            // el único hilo que queda hasta la declaración `ast::Field`
            // ORIGINAL, que sí carga `@validate`. Sin `name` (struct
            // anónimo inline) no hay ninguna declaración a la que
            // atribuirle un validador, así que simplemente no hay nada que
            // aplicar -- no es un caso de error.
            if let Some(n) = name {
                if let Some(ast_fields) = field_annotations_for(checker, n, None) {
                    apply_field_validators(ast_fields, &v, path)?;
                }
            }
            Ok(v)
        }
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

/// Los campos `ast::Field` (con sus `@validate`) de la declaración `name`,
/// si hay -- struct (`type name = {...}`) cuando `variant` es `None`,
/// campos de esa variante puntual cuando `variant` es `Some` (GRAMMAR.md
/// §3.73). `None` cuando `name` no resuelve a nada con esa forma -- mismo
/// motivo en los dos casos: nada a lo que atribuirle un validador.
fn field_annotations_for<'a>(checker: &'a Checker, name: &str, variant: Option<&str>) -> Option<&'a Vec<Field>> {
    match variant {
        None => {
            let decl = checker.types.get(name)?;
            match &decl.ty {
                TypeExpr::Struct(fields) => Some(fields),
                _ => None,
            }
        }
        Some(v) => {
            let decl = checker.enums.get(name)?;
            decl.variants.iter().find(|variant| variant.name == v)?.fields.as_ref()
        }
    }
}

/// Aplica `@validate(...)` (GRAMMAR.md §3.73) sobre los campos de `value`
/// (ya decodificado) contra la lista de `ast::Field` de la declaración
/// ORIGINAL -- el único lugar donde `@validate` vive, nunca en
/// `types::FieldType` (estructural, sin anotaciones). Se llama desde DOS
/// puntos: al decodificar el wire (`json_to_typed_value`, un `rpc` recibe
/// el struct COMPLETO como parámetro) y al CONSTRUIR un literal en el
/// intérprete (`Expr::StructLit`, el caso más común -- un `rpc` arma
/// `NewX { campo: valorEscalar }` adentro del cuerpo a partir de parámetros
/// sueltos, que nunca pasan por `json_to_typed_value` como struct). Sin el
/// segundo punto, `@validate` solo protegería el caso menos común (un rpc
/// que recibe el struct entero como parámetro) y dejaría pasar el más
/// típico sin avisar -- encontrado probando el servidor real, no leyendo
/// el código.
fn apply_field_validators(ast_fields: &[Field], value: &Value, path: &str) -> Result<(), RuntimeError> {
    let Value::Struct(entries) = value else { return Ok(()) };
    for af in ast_fields {
        let Some(validator) = af.validator() else { continue };
        // Ausente (campo opcional que no vino) o presente pero `Null`: nada
        // que validar -- `@validate` no vuelve requerido un campo opcional.
        let Some((_, v)) = entries.iter().find(|(n, _)| n == &af.name) else { continue };
        let Value::Str(s) = v else { continue };
        match validator {
            FieldValidator::Email => {
                if !is_plausible_email(s) {
                    return Err(bad_req(format!(
                        "'{path}.{}': '{s}' no es un email válido (@validate(email))",
                        af.name
                    )));
                }
            }
            FieldValidator::Regex(pattern) => {
                // El patrón ya se validó en `linkc build`
                // (checker::check_field_validators) -- si llegó hasta acá,
                // compilar de nuevo no puede fallar.
                let re = regex::Regex::new(pattern).expect("patrón de @validate ya validado en compilación");
                if !re.is_match(s) {
                    return Err(bad_req(format!(
                        "'{path}.{}': '{s}' no matchea @validate(regex, \"{pattern}\")",
                        af.name
                    )));
                }
            }
        }
    }
    Ok(())
}

/// Forma general de un email (GRAMMAR.md §3.73 "Límites honestos") -- NO
/// RFC 5322 completo (eso admite formas raras -- local-part entre comillas,
/// IP literal entre corchetes -- que casi ningún email real usa, y que
/// complicarían mucho esta función sin beneficio real). Exige: exactamente
/// un '@', local-part no vacío y sin espacios, dominio con al menos un '.'
/// y ningún segmento (separado por '.') vacío.
fn is_plausible_email(s: &str) -> bool {
    let Some((local, domain)) = s.split_once('@') else { return false };
    if local.is_empty() || local.contains(char::is_whitespace) {
        return false;
    }
    if domain.contains('@') || domain.contains(char::is_whitespace) || !domain.contains('.') {
        return false;
    }
    domain.split('.').all(|seg| !seg.is_empty())
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
        Type::Int64 => "Int64".into(),
        Type::Timestamp => "Timestamp".into(),
        Type::Float => "Float".into(),
        Type::String => "String".into(),
        Type::Uuid => "Uuid".into(),
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
            Member::Rpc(r) | Member::Stream(r) if r.name == rpc_name => r.auth(),
            _ => None,
        }),
        _ => None,
    })
}

/// Anotación `@rate_limit("N/ventana")` de `{service_name}.{rpc_name}`, si
/// tiene una -- hermana de `required_auth` (mismo archivo/patrón, mismo uso
/// desde `server.rs` antes de invocar nada). El texto crudo, sin parsear:
/// `server.rs` lo pasa a `rate_limit::RateLimitSpec::parse`, que el checker
/// ya validó que nunca falla para un programa que compiló (GRAMMAR.md §3.39).
pub fn required_rate_limit<'a>(program: &'a Program, service_name: &str, rpc_name: &str) -> Option<&'a str> {
    program.items.iter().find_map(|i| match i {
        Item::Service(s) if s.name == service_name => s.members.iter().find_map(|m| match m {
            Member::Rpc(r) | Member::Stream(r) if r.name == rpc_name => r.rate_limit(),
            _ => None,
        }),
        _ => None,
    })
}

/// Si el CUERPO de `service_name.rpc_name` matchea el shape de push real
/// v0 (GRAMMAR.md §3.16), el nombre de la colección a la que se suscribe.
/// Otra hermana de `is_stream_member`/`required_auth`: `server.rs` la usa
/// para decidir el routing ANTES de invocar `invoke_rpc_with_sessions` --
/// ese cuerpo nunca llega a `eval_block` (ver `ast::recognize_live_subscribe`,
/// y `Db::subscribe`, que es lo que de verdad atiende esta forma de
/// stream). `None` cubre por igual "no es un stream", "es un stream
/// clásico List<T>", y "rpc desconocido" -- ese último lo detecta
/// `invoke_rpc_with_sessions` como siempre si de verdad se llega a invocar.
pub fn live_subscribe_collection<'a>(program: &'a Program, service_name: &str, rpc_name: &str) -> Option<&'a str> {
    program.items.iter().find_map(|i| match i {
        Item::Service(s) if s.name == service_name => s.members.iter().find_map(|m| match m {
            Member::Stream(r) if r.name == rpc_name => crate::ast::recognize_live_subscribe(&r.body),
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
pub(crate) fn simple_enum_names(program: &Program) -> std::collections::HashSet<String> {
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
        // String, no número -- ver la nota simétrica en json_to_typed_value.
        Value::Int64(n) => json!(n.to_string()),
        Value::Timestamp(n) => json!(timestamp::format_iso8601_millis(*n)),
        Value::Float(n) => json!(n),
        Value::Str(s) => json!(s),
        // Mismo texto plano que un String -- ver la nota simétrica en
        // json_to_typed_value (que sí exige el formato al DECODIFICAR).
        Value::Uuid(s) => json!(s),
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
        Value::Db | Value::DbCollection(_) | Value::Auth | Value::Service(_) | Value::Math | Value::Crypto | Value::Http | Value::Json | Value::Base64 | Value::Env | Value::Request | Value::Smtp | Value::Response | Value::BoundMethod(_, _) | Value::FnRef(_) | Value::Closure(..) => {
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
            &json!({"input": {"name": "Grace Hopper", "email": "grace@example.com", "createdAt": "2026-01-01T00:00:00.000Z"}}),
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
        let db = Db::new(&program, std::path::Path::new(":memory:"));

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
            &json!({"input": {"name": "Sin Email", "email": "", "createdAt": "2026-01-01T00:00:00.000Z"}}),
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
            &json!({"input": {"name": "", "email": "valido@example.com", "createdAt": "2026-01-01T00:00:00.000Z"}}),
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

    // ---- narrowing real de `T?` vía 'match', '??', '.isSome()'/'.isNone()' (GRAMMAR.md §3.9) ----

    #[test]
    fn match_narrowing_over_an_optional_struct_reads_the_real_field_in_both_branches() {
        let program = program_from(
            r#"
            type Coupon = { id: Int, code: String }
            service S {
                rpc describe(c: Coupon?) -> String {
                    match c {
                        cc: Coupon => "activo: " + cc.code,
                        null => "sin coupon",
                    }
                }
            }
        "#,
        );
        let db = Db::seeded();
        assert_eq!(
            invoke_rpc(&program, "S", "describe", &json!({"c": {"id": 1, "code": "AHORRO10"}}), &db).unwrap(),
            json!("activo: AHORRO10")
        );
        assert_eq!(invoke_rpc(&program, "S", "describe", &json!({"c": null}), &db).unwrap(), json!("sin coupon"));
    }

    #[test]
    fn match_narrowing_over_a_primitive_optional_works_too() {
        let program = program_from(
            r#"
            service S {
                rpc describe(x: Int?) -> String {
                    match x {
                        n: Int => "valor: " + n.toString(),
                        null => "ausente",
                    }
                }
            }
        "#,
        );
        let db = Db::seeded();
        assert_eq!(invoke_rpc(&program, "S", "describe", &json!({"x": 5}), &db).unwrap(), json!("valor: 5"));
        assert_eq!(invoke_rpc(&program, "S", "describe", &json!({"x": null}), &db).unwrap(), json!("ausente"));
    }

    #[test]
    fn coalesce_returns_the_value_when_present_and_the_default_when_null() {
        let program = program_from(
            r#"
            service S {
                rpc nameOrDefault(name: String?) -> String {
                    name ?? "anonimo"
                }
            }
        "#,
        );
        let db = Db::seeded();
        assert_eq!(invoke_rpc(&program, "S", "nameOrDefault", &json!({"name": "Ada"}), &db).unwrap(), json!("Ada"));
        assert_eq!(invoke_rpc(&program, "S", "nameOrDefault", &json!({"name": null}), &db).unwrap(), json!("anonimo"));
    }

    #[test]
    fn coalesce_short_circuits_and_never_evaluates_the_right_side_when_left_is_present() {
        // El lado derecho de '??' es una llamada a assert(false, ...) --
        // si esto se evaluara igual (sin cortocircuito real), el test
        // fallaría con ese mensaje en vez de pasar.
        let program = program_from(
            r#"
            service S {
                rpc firstOrBoom(x: String?) -> String {
                    x ?? panic("el lado derecho de ?? no debería evaluarse")
                }
            }
        "#,
        );
        let db = Db::seeded();
        assert_eq!(invoke_rpc(&program, "S", "firstOrBoom", &json!({"x": "presente"}), &db).unwrap(), json!("presente"));
    }

    #[test]
    fn coalesce_chains_across_several_optionals_left_to_right() {
        let program = program_from(
            r#"
            service S {
                rpc firstNonNull(a: String?, b: String?) -> String {
                    a ?? b ?? "los dos ausentes"
                }
            }
        "#,
        );
        let db = Db::seeded();
        assert_eq!(
            invoke_rpc(&program, "S", "firstNonNull", &json!({"a": null, "b": null}), &db).unwrap(),
            json!("los dos ausentes")
        );
        assert_eq!(invoke_rpc(&program, "S", "firstNonNull", &json!({"a": null, "b": "b"}), &db).unwrap(), json!("b"));
        assert_eq!(invoke_rpc(&program, "S", "firstNonNull", &json!({"a": "a", "b": "b"}), &db).unwrap(), json!("a"));
    }

    #[test]
    fn is_some_and_is_none_reflect_presence_for_a_struct_shaped_optional() {
        let program = program_from(
            r#"
            type Item = { id: Int, name: String }
            service S {
                rpc present(x: Item?) -> Bool { x.isSome() }
                rpc absent(x: Item?) -> Bool { x.isNone() }
            }
        "#,
        );
        let db = Db::seeded();
        let item = json!({"id": 1, "name": "Ada"});
        assert_eq!(invoke_rpc(&program, "S", "present", &json!({"x": item}), &db).unwrap(), json!(true));
        assert_eq!(invoke_rpc(&program, "S", "present", &json!({"x": null}), &db).unwrap(), json!(false));
        assert_eq!(invoke_rpc(&program, "S", "absent", &json!({"x": null}), &db).unwrap(), json!(true));
    }

    #[test]
    fn is_some_does_not_get_shadowed_by_a_real_struct_field_of_the_same_name() {
        // El caso adversarial que motivó el chequeo explícito en
        // eval_expr::Expr::Call: un struct PLANO (no opcional) con un campo
        // de verdad llamado 'isSome' (una closure) tiene que seguir
        // llamándose como ESE campo, no como el atajo del opcional.
        let program = program_from(
            r#"
            type Weird = { id: Int, isSome: (Int) -> Bool }
            service S {
                rpc callIt() -> Bool {
                    let w = Weird { id: 1, isSome: |n: Int| { n > 100 } };
                    w.isSome(0)
                }
            }
        "#,
        );
        let db = Db::seeded();
        assert_eq!(invoke_rpc(&program, "S", "callIt", &json!({}), &db).unwrap(), json!(false));
    }

    // ---- tipo nativo `Uuid` (GRAMMAR.md §3.70) ----

    #[test]
    fn a_malformed_uuid_over_the_wire_is_rejected_as_a_bad_request() {
        let program = program_from(
            r#"
            service S {
                rpc echo(u: Uuid) -> Uuid { u }
            }
        "#,
        );
        let db = Db::seeded();
        for bad in [
            json!({"u": "not-a-uuid"}),
            json!({"u": "550e8400-e29b-41d4-a716-44665544000"}),  // 35 caracteres, falta uno
            json!({"u": "550e8400-e29b-41d4-a716-4466554400000"}), // 37 caracteres, uno de más
            json!({"u": "550e8400e29b41d4a716446655440000"}),      // sin guiones
            json!({"u": "zzzzzzzz-e29b-41d4-a716-446655440000"}),  // no hex
            json!({"u": 12345}),                                    // número, no string
            json!({"u": null}),
        ] {
            let e = invoke_rpc(&program, "S", "echo", &bad, &db).expect_err(&format!("echo({bad}) debería rechazarse"));
            assert_eq!(e.kind, ErrorKind::BadRequest, "echo({bad}): {e}");
        }
    }

    #[test]
    fn a_well_formed_uuid_round_trips_through_the_wire_exactly() {
        let program = program_from(
            r#"
            service S {
                rpc echo(u: Uuid) -> Uuid { u }
            }
        "#,
        );
        let db = Db::seeded();
        // Mayúsculas incluidas: la validación es case-insensitive.
        for good in ["550e8400-e29b-41d4-a716-446655440000", "550E8400-E29B-41D4-A716-446655440000"] {
            assert_eq!(invoke_rpc(&program, "S", "echo", &json!({"u": good}), &db).unwrap(), json!(good));
        }
    }

    #[test]
    fn crypto_uuid_generates_a_real_uuid_that_round_trips_through_db_insert_and_find() {
        let program = program_from(
            r#"
            type Session = { id: Int, token: Uuid }
            type NewSession = { token: Uuid }
            db { sessions: Session[] }
            service S {
                rpc create() -> Session { db.sessions.insert(NewSession { token: crypto.uuid() }) }
                rpc get(id: Int) -> Session? { db.sessions.find(id) }
            }
        "#,
        );
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        let created = invoke_rpc(&program, "S", "create", &json!({}), &db).unwrap();
        let token = created["token"].as_str().expect("token generado por crypto.uuid()");
        assert_eq!(token.len(), 36, "formato uuid: {token}");
        let id = created["id"].as_i64().unwrap();
        let fetched = invoke_rpc(&program, "S", "get", &json!({"id": id}), &db).unwrap();
        assert_eq!(fetched["token"], json!(token), "el mismo uuid vuelve identico al leerlo de la base");
    }

    // ---- `@validate(...)` (GRAMMAR.md §3.73) ----

    #[test]
    fn validate_email_rejects_a_malformed_address_and_accepts_a_real_one() {
        let program = program_from(
            r#"
            type Signup = { @validate(email) email: String }
            service S {
                rpc register(s: Signup) -> String { s.email }
            }
        "#,
        );
        let db = Db::seeded();
        for bad in ["not-an-email", "no-at-sign.com", "@no-local-part.com", "trailing-dot@x.", "spa ces@x.com", "two@@x.com"] {
            let e = invoke_rpc(&program, "S", "register", &json!({"s": {"email": bad}}), &db)
                .expect_err(&format!("'{bad}' debería rechazarse"));
            assert_eq!(e.kind, ErrorKind::BadRequest, "'{bad}': {e}");
        }
        assert_eq!(
            invoke_rpc(&program, "S", "register", &json!({"s": {"email": "a@b.com"}}), &db).unwrap(),
            json!("a@b.com")
        );
    }

    #[test]
    fn validate_regex_rejects_a_non_matching_value_and_accepts_a_matching_one() {
        let program = program_from(
            r#"
            type Order = { @validate(regex, "^[A-Z]{3}-[0-9]{4}$") sku: String }
            service S {
                rpc place(o: Order) -> String { o.sku }
            }
        "#,
        );
        let db = Db::seeded();
        for bad in ["abc-1234", "AB-1234", "ABC-12345", "ABC1234"] {
            let e = invoke_rpc(&program, "S", "place", &json!({"o": {"sku": bad}}), &db)
                .expect_err(&format!("'{bad}' debería rechazarse"));
            assert_eq!(e.kind, ErrorKind::BadRequest, "'{bad}': {e}");
        }
        assert_eq!(
            invoke_rpc(&program, "S", "place", &json!({"o": {"sku": "ABC-1234"}}), &db).unwrap(),
            json!("ABC-1234")
        );
    }

    /// Un campo `String?` con `@validate` sigue siendo genuinamente opcional
    /// -- ausente no dispara ninguna validación, solo un valor PRESENTE se
    /// valida.
    #[test]
    fn validate_on_an_optional_field_only_runs_when_the_value_is_present() {
        let program = program_from(
            r#"
            type Contact = { @validate(email) email?: String }
            service S {
                rpc echo(c: Contact) -> String? { c.email }
            }
        "#,
        );
        let db = Db::seeded();
        assert_eq!(invoke_rpc(&program, "S", "echo", &json!({"c": {}}), &db).unwrap(), json!(null));
        let e = invoke_rpc(&program, "S", "echo", &json!({"c": {"email": "not-an-email"}}), &db)
            .expect_err("un valor presente sigue validándose");
        assert_eq!(e.kind, ErrorKind::BadRequest, "{e}");
    }

    /// El caso REAL más común, no el sintético: un rpc no recibe el struct
    /// entero como parámetro (eso es lo que cubren los tests de arriba, vía
    /// `json_to_typed_value`) -- recibe campos SUELTOS y arma el struct
    /// (típicamente el shape "New*" que omite `id`, mismo patrón que
    /// `insert` usa en todo el proyecto) ADENTRO del cuerpo del rpc. Ese
    /// literal nunca pasa por el decode del wire como struct -- solo
    /// `Expr::StructLit` lo ve. Encontrado probando contra un servidor
    /// real con `curl`: sin este chequeo en `Expr::StructLit`, un email
    /// invalido pasaba de largo con 200 pese a `@validate(email)` estar
    /// declarado -- el caso de uso que motivó todo el ítem quedaba
    /// completamente sin protección.
    #[test]
    fn validate_fires_when_the_struct_is_constructed_inside_the_rpc_body_from_loose_params() {
        let program = program_from(
            r#"
            type Signup = { id: Int, @validate(email) email: String }
            type NewSignup = { @validate(email) email: String }
            db { signups: Signup[] }
            service S {
                rpc register(email: String) -> Signup { db.signups.insert(NewSignup { email: email }) }
            }
        "#,
        );
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        let e = invoke_rpc(&program, "S", "register", &json!({"email": "not-an-email"}), &db)
            .expect_err("un email invalido construido adentro del cuerpo debe rechazarse igual");
        assert_eq!(e.kind, ErrorKind::BadRequest, "{e}");
        assert!(e.message.contains("NewSignup.email"), "{e}");
        let ok = invoke_rpc(&program, "S", "register", &json!({"email": "a@b.com"}), &db).unwrap();
        assert_eq!(ok["email"], json!("a@b.com"));
    }

    /// `@validate` solo protege la declaración donde está escrito -- si el
    /// shape "New*" NO repite la anotación (solo el struct "completo" la
    /// tiene), construir el "New*" no valida nada. No es un bug: es la
    /// misma regla que ya aplica a `@deprecated` y a toda anotación de
    /// campo (atada a la declaración, nunca "estructural" entre dos tipos
    /// distintos aunque tengan el mismo campo) -- documentado como límite
    /// honesto en GRAMMAR.md §3.73 precisamente porque es fácil de pisar
    /// sin darse cuenta con el patrón "New*" que el resto del proyecto usa
    /// en todos lados para `insert`.
    #[test]
    fn validate_on_the_full_type_does_not_protect_a_differently_named_create_shape_missing_the_same_annotation() {
        let program = program_from(
            r#"
            type Signup = { id: Int, @validate(email) email: String }
            type NewSignup = { email: String }
            db { signups: Signup[] }
            service S {
                rpc register(email: String) -> Signup { db.signups.insert(NewSignup { email: email }) }
            }
        "#,
        );
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        let result = invoke_rpc(&program, "S", "register", &json!({"email": "not-an-email"}), &db);
        assert!(result.is_ok(), "documenta el límite: sin @validate en NewSignup, no hay nada que lo rechace");
    }

    // ---- valores por defecto en campos de struct (GRAMMAR.md §3.74) ----

    #[test]
    fn a_struct_literal_omitting_a_defaulted_field_fills_it_in_at_construction() {
        let program = program_from(
            r#"
            type Task = { id: Int, title: String, status: String = "pending" }
            type NewTask = { title: String, status: String = "pending" }
            db { tasks: Task[] }
            service S {
                rpc create(title: String) -> Task { db.tasks.insert(NewTask { title: title }) }
            }
        "#,
        );
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        let created = invoke_rpc(&program, "S", "create", &json!({"title": "comprar leche"}), &db).unwrap();
        assert_eq!(created["status"], json!("pending"));
        assert_eq!(created["title"], json!("comprar leche"));
    }

    #[test]
    fn an_explicit_value_overrides_the_field_default() {
        let program = program_from(
            r#"
            type Task = { id: Int, title: String, status: String = "pending" }
            type NewTask = { title: String, status: String = "pending" }
            db { tasks: Task[] }
            service S {
                rpc create(title: String, status: String) -> Task {
                    db.tasks.insert(NewTask { title: title, status: status })
                }
            }
        "#,
        );
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        let created = invoke_rpc(&program, "S", "create", &json!({"title": "urgente", "status": "active"}), &db).unwrap();
        assert_eq!(created["status"], json!("active"));
    }

    /// El default se EVALÚA de nuevo en cada construcción, no una sola vez
    /// -- probado con `crypto.uuid()` (no-constante): dos literales
    /// separados, sin dar el campo, tienen que salir con valores DISTINTOS.
    #[test]
    fn a_non_constant_default_is_evaluated_fresh_on_each_construction() {
        let program = program_from(
            r#"
            type Session = { id: Int, token: Uuid = crypto.uuid() }
            type NewSession = { token: Uuid = crypto.uuid() }
            db { sessions: Session[] }
            service S {
                rpc create() -> Session { db.sessions.insert(NewSession { }) }
            }
        "#,
        );
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        let a = invoke_rpc(&program, "S", "create", &json!({}), &db).unwrap();
        let b = invoke_rpc(&program, "S", "create", &json!({}), &db).unwrap();
        assert_ne!(a["token"], b["token"], "cada construcción evalúa su propio default");
    }

    // ---- db.<c>.upsert(matchFn, insertValue, updateFn) (GRAMMAR.md §3.75) ----

    #[test]
    fn upsert_inserts_when_no_row_matches() {
        let program = program_from(
            r#"
            type Counter = { id: Int, name: String, count: Int }
            type NewCounter = { name: String, count: Int }
            db { counters: Counter[] }
            service S {
                rpc bump(name: String) -> Counter {
                    db.counters.upsert(
                        |c: Counter| { c.name == name },
                        NewCounter { name: name, count: 1 },
                        |c: Counter| { NewCounter { name: c.name, count: c.count + 1 } }
                    )
                }
            }
        "#,
        );
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        let created = invoke_rpc(&program, "S", "bump", &json!({"name": "clics"}), &db).unwrap();
        assert_eq!(created["name"], json!("clics"));
        assert_eq!(created["count"], json!(1));
    }

    /// El segundo `upsert` sobre el MISMO `name` tiene que actualizar la
    /// fila existente -- mismo `id`, `count` incrementado vía `updateFn`,
    /// nunca una fila nueva.
    #[test]
    fn upsert_updates_the_same_row_in_place_when_a_row_matches() {
        let program = program_from(
            r#"
            type Counter = { id: Int, name: String, count: Int }
            type NewCounter = { name: String, count: Int }
            db { counters: Counter[] }
            service S {
                rpc bump(name: String) -> Counter {
                    db.counters.upsert(
                        |c: Counter| { c.name == name },
                        NewCounter { name: name, count: 1 },
                        |c: Counter| { NewCounter { name: c.name, count: c.count + 1 } }
                    )
                }
            }
        "#,
        );
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        let first = invoke_rpc(&program, "S", "bump", &json!({"name": "clics"}), &db).unwrap();
        let second = invoke_rpc(&program, "S", "bump", &json!({"name": "clics"}), &db).unwrap();
        assert_eq!(first["id"], second["id"], "misma fila, mismo id");
        assert_eq!(second["count"], json!(2));

        let all = invoke_rpc(&program, "S", "bump", &json!({"name": "otro"}), &db).unwrap();
        assert_ne!(all["id"], second["id"], "un name distinto SÍ inserta una fila nueva");
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

    // ---- Int64 (GRAMMAR.md §3.30) ----

    fn int64_demo() -> Program {
        program_from(
            r#"
            service S {
                rpc echoInt64(n: Int64) -> Int64 { n }
            }
        "#,
        )
    }

    #[test]
    fn int64_round_trips_exactly_at_i64_extremes_as_a_wire_string() {
        let program = int64_demo();
        let db = Db::seeded();
        for extreme in [i64::MIN.to_string(), i64::MAX.to_string(), "0".to_string()] {
            let result = invoke_rpc(&program, "S", "echoInt64", &json!({"n": extreme}), &db)
                .unwrap_or_else(|e| panic!("echoInt64({extreme}) debería aceptarse: {e}"));
            assert_eq!(result, json!(extreme), "Int64 debe viajar como string, exacto, ida y vuelta");
        }
    }

    #[test]
    fn int64_rejects_a_native_json_number_and_malformed_or_out_of_range_strings() {
        // Un número JSON nativo se rechaza a propósito -- aceptarlo
        // reabriría exactamente la pérdida de precisión que Int64 existe
        // para evitar (un cliente ya mandó un f64 antes de llegar acá).
        let program = int64_demo();
        let db = Db::seeded();
        for bad in [
            json!({"n": 123}),                            // número JSON nativo, no string
            json!({"n": "no es un entero"}),               // string no numérico
            json!({"n": "1.5"}),                            // string numérico pero no entero
            json!({"n": "99999999999999999999999999999"}), // string numérico, fuera de rango i64
        ] {
            let e = invoke_rpc(&program, "S", "echoInt64", &bad, &db)
                .expect_err(&format!("echoInt64({bad}) debería rechazarse"));
            assert_eq!(e.kind, ErrorKind::BadRequest, "echoInt64({bad}): {e}");
        }
    }

    // ---- Timestamp (GRAMMAR.md §3.31) ----

    fn timestamp_demo() -> Program {
        program_from(
            r#"
            service S {
                rpc echoTimestamp(t: Timestamp) -> Timestamp { t }
            }
        "#,
        )
    }

    #[test]
    fn timestamp_round_trips_exactly_through_the_wire() {
        let program = timestamp_demo();
        let db = Db::seeded();
        for s in [
            "1970-01-01T00:00:00.000Z",
            "2024-02-29T12:00:00.000Z", // año bisiesto
            "2000-02-29T00:00:00.000Z", // frontera de siglo SÍ bisiesto
            "1969-12-31T23:59:59.999Z", // pre-1970, negativo internamente
        ] {
            let result = invoke_rpc(&program, "S", "echoTimestamp", &json!({"t": s}), &db)
                .unwrap_or_else(|e| panic!("echoTimestamp({s}) debería aceptarse: {e}"));
            assert_eq!(result, json!(s));
        }
    }

    #[test]
    fn timestamp_rejects_a_century_non_leap_date_and_other_malformed_or_wrong_shape_strings() {
        let program = timestamp_demo();
        let db = Db::seeded();
        for bad in [
            json!({"t": "1900-02-29T00:00:00.000Z"}), // 1900 NO es bisiesto -- el bug clásico "divisible por 4"
            json!({"t": "2026-13-01T00:00:00.000Z"}), // mes inválido
            json!({"t": "2026-08-08T10:00:00Z"}),      // sin milisegundos
            json!({"t": "2026-08-08T10:00:00.000+02:00"}), // offset de timezone, no 'Z'
            json!({"t": 1234567890}),                  // número JSON nativo, no string
            json!({"t": "no es una fecha"}),
        ] {
            let e = invoke_rpc(&program, "S", "echoTimestamp", &bad, &db)
                .expect_err(&format!("echoTimestamp({bad}) debería rechazarse"));
            assert_eq!(e.kind, ErrorKind::BadRequest, "echoTimestamp({bad}): {e}");
        }
    }

    #[test]
    fn now_returns_valid_iso8601_timestamp_in_runtime() {
        let tokens = crate::lexer::tokenize("service S { rpc current() -> Timestamp { now() } }").unwrap();
        let program = crate::parser::parse(tokens).unwrap();
        let db = Db::seeded();
        let res = invoke_rpc(&program, "S", "current", &json!({}), &db).unwrap();
        let s = res.as_str().expect("now() debe devolver string ISO-8601");
        let parsed = timestamp::parse_iso8601_millis(s);
        assert!(parsed.is_some(), "now() devolvió un timestamp inválido: {s}");
        // Debe ser posterior al año 2024 (milisegundos > 1_700_000_000_000)
        assert!(parsed.unwrap() > 1_700_000_000_000);
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
        let db = Db::new(&program, std::path::Path::new(":memory:"));
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
            Some(&Annotation::Requires { enum_name: "Role".to_string(), variant_names: vec!["Admin".to_string()] })
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

    // ---- constructo de loop: `while` (GRAMMAR.md §3.15) ----

    #[test]
    fn while_loop_aggregates_a_list_without_recursion() {
        let program = program_from(
            r#"
            service S {
                rpc sum(xs: Int[]) -> Int {
                    let mut total = 0;
                    let mut i = 0;
                    while i < xs.length() {
                        total = total + xs[i];
                        i = i + 1;
                    }
                    total
                }
            }
        "#,
        );
        let result = invoke_rpc(&program, "S", "sum", &json!({"xs": [1, 2, 3, 4]}), &Db::seeded()).unwrap();
        assert_eq!(result, json!(10));
    }

    #[test]
    fn assignment_inside_a_while_body_propagates_across_iterations() {
        // Análogo directo de assignment_inside_if_branch_propagates_to_outer_scope,
        // pero para `while`: confirma que la mutación Rc<RefCell<Value>>
        // persiste de una vuelta del loop a la siguiente, no que cada
        // iteración vea su propia copia descartable de "i".
        let program = program_from(
            r#"
            service S {
                rpc count_to(n: Int) -> Int {
                    let mut i = 0;
                    while i < n {
                        i = i + 1;
                    }
                    i
                }
            }
        "#,
        );
        let result = invoke_rpc(&program, "S", "count_to", &json!({"n": 5}), &Db::seeded()).unwrap();
        assert_eq!(result, json!(5));
    }

    #[test]
    fn while_condition_that_is_not_bool_is_a_runtime_error() {
        let program = program_from(
            r#"
            service S {
                rpc f() -> Int { while 1 { } 0 }
            }
        "#,
        );
        let result = invoke_rpc(&program, "S", "f", &json!({}), &Db::seeded());
        assert!(result.is_err());
    }

    #[test]
    fn a_while_loop_that_never_terminates_hits_the_iteration_cap_instead_of_hanging() {
        let program = program_from(
            r#"
            service S {
                rpc f() -> Int { while true { } 0 }
            }
        "#,
        );
        let result = invoke_rpc(&program, "S", "f", &json!({}), &Db::seeded());
        assert!(result.is_err(), "un 'while true {{ }}' debería chocar contra la cota, no colgar el test");
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("iteraciones"), "el error debería mencionar el límite de iteraciones: {msg}");
    }

    // ---- pub-sub sobre `db`: push real para `stream` (GRAMMAR.md §3.16) ----

    fn live_subscribe_program() -> Program {
        program_from(
            r#"
            type Item = { id: Int, name: String }
            type NewItem = { name: String }
            db { items: Item[] }
            service S {
                rpc add(name: String) -> Item { db.items.insert(NewItem { name: name }) }
            }
        "#,
        )
    }

    #[test]
    fn subscribing_then_inserting_delivers_the_new_row_as_an_event() {
        let program = live_subscribe_program();
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        let (snapshot, events) = db.subscribe("items").unwrap();
        assert!(snapshot.is_empty(), "una colección recién creada no debería tener filas todavía");

        invoke_rpc(&program, "S", "add", &json!({"name": "primero"}), &db).unwrap();

        let event = events
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("se esperaba un evento publicado tras el insert");
        assert_eq!(event["name"], json!("primero"));
        assert_eq!(event["id"], json!(1));
    }

    #[test]
    fn a_disconnected_subscriber_does_not_stop_others_from_receiving_events() {
        // No hay forma pública de observar que el Vec interno de
        // suscriptores se podó (es privado a propósito) -- lo que sí es una
        // garantía de comportamiento real, y lo que este test prueba, es
        // que un suscriptor muerto no rompe ni salta la publicación al
        // resto: `publish` sigue entregando a cualquier otro suscriptor
        // todavía vivo.
        let program = live_subscribe_program();
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        let (_, dead_events) = db.subscribe("items").unwrap();
        let (_, alive_events) = db.subscribe("items").unwrap();
        drop(dead_events);

        invoke_rpc(&program, "S", "add", &json!({"name": "x"}), &db).unwrap();

        let event = alive_events
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("el suscriptor vivo debería seguir recibiendo eventos aunque otro se haya desconectado");
        assert_eq!(event["name"], json!("x"));
    }

    #[test]
    fn subscribing_to_an_unknown_collection_is_a_runtime_error() {
        let program = live_subscribe_program();
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        assert!(db.subscribe("noExiste").is_err());
    }

    // ---- persistencia real: SQLite (GRAMMAR.md §3.17) ----

    fn optional_shapes_program() -> Program {
        program_from(
            r#"
            type Item = {
                id: Int,
                name: String,
                hint?: String,
                note: String?,
                tag?: String?,
            }
            type NewItem = { name: String, hint?: String, note: String?, tag?: String? }
            db { items: Item[] }
            service S {
                rpc add(x: NewItem) -> Item { db.items.insert(x) }
                rpc get(id: Int) -> Item? { db.items.find(id) }
            }
        "#,
        )
    }

    #[test]
    fn seeded_grace_hopper_has_no_bio_key_in_the_json_wire_shape() {
        // Grace Hopper (Db::seeded()) omite 'bio' del todo -- opcional POR
        // CLAVE (GRAMMAR.md §3.4), no nullable. Ningún test anterior a esta
        // ronda llegaba a chequear esto de verdad contra el shape de wire --
        // confirma que el round-trip por SQL preserva la distinción:
        // ausente sigue siendo ausente, nunca se filtra como `null`.
        let program = users_demo();
        let db = Db::seeded();
        let result = invoke_rpc(&program, "Users", "getById", &json!({"id": 2}), &db).unwrap();
        assert_eq!(result["name"], json!("Grace Hopper"));
        assert!(result.get("bio").is_none(), "se esperaba 'bio' AUSENTE, no presente: {result}");
        assert_eq!(result["deletedAt"], serde_json::Value::Null, "'deletedAt' es nullable-por-tipo (x: T?), sigue presente con null");
    }

    #[test]
    fn a_nullable_typed_field_round_trips_null_as_a_present_key_with_null_value() {
        let program = optional_shapes_program();
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        let created = invoke_rpc(&program, "S", "add", &json!({"x": {"name": "x", "note": null}}), &db).unwrap();
        assert!(created.get("note").is_some(), "la clave 'note' siempre tiene que estar presente (x: T?, no x?: T)");
        assert_eq!(created["note"], serde_json::Value::Null);

        let fetched = invoke_rpc(&program, "S", "get", &json!({"id": created["id"]}), &db).unwrap();
        assert_eq!(fetched["note"], serde_json::Value::Null, "el valor null tiene que sobrevivir un round-trip completo por SQL");
    }

    #[test]
    fn a_field_that_is_both_optional_by_key_and_nullable_round_trips_all_three_states() {
        let program = optional_shapes_program();
        let db = Db::new(&program, std::path::Path::new(":memory:"));

        let absent = invoke_rpc(&program, "S", "add", &json!({"x": {"name": "a", "note": null}}), &db).unwrap();
        assert!(absent.get("tag").is_none(), "se esperaba 'tag' AUSENTE: {absent}");

        let present_null = invoke_rpc(&program, "S", "add", &json!({"x": {"name": "b", "note": null, "tag": null}}), &db).unwrap();
        assert!(present_null.get("tag").is_some(), "se esperaba 'tag' PRESENTE (con null): {present_null}");
        assert_eq!(present_null["tag"], serde_json::Value::Null);

        let present_value = invoke_rpc(&program, "S", "add", &json!({"x": {"name": "c", "note": null, "tag": "urgente"}}), &db).unwrap();
        assert_eq!(present_value["tag"], json!("urgente"));

        // Los 3 estados tienen que sobrevivir un SEGUNDO round-trip (releer
        // de SQL, no solo la respuesta que ya devolvió el propio insert).
        let refetched = invoke_rpc(&program, "S", "get", &json!({"id": absent["id"]}), &db).unwrap();
        assert!(refetched.get("tag").is_none());
        let refetched = invoke_rpc(&program, "S", "get", &json!({"id": present_null["id"]}), &db).unwrap();
        assert_eq!(refetched["tag"], serde_json::Value::Null);
                    let refetched = invoke_rpc(&program, "S", "get", &json!({"id": present_value["id"]}), &db).unwrap();
        assert_eq!(refetched["tag"], json!("urgente"));
    }

    #[test]
    fn test_delete_where_and_find_where_respect_predicate() {
        let code = r#"
        enum Role { Admin, Guest }
        type User = { id: Int, name: String, role: Role }
        db { users: User[] }
        service Users {
          rpc seed() -> Void {
            db.users.insert(User { id: 0, name: "AdminUser", role: Role.Admin {} });
            db.users.insert(User { id: 0, name: "GuestUser1", role: Role.Guest {} });
            db.users.insert(User { id: 0, name: "GuestUser2", role: Role.Guest {} });
          }


          rpc findGuests() -> User[] {
            db.users.findWhere(|u: User| { u.role == Role.Guest {} })
          }
          rpc deleteGuests() -> Int {
            db.users.deleteWhere(|u: User| { u.role == Role.Guest {} })
          }


          rpc remaining() -> User[] {
            db.users.all()
          }
        }
        "#;
        let program = crate::parser::parse(crate::lexer::tokenize(code).unwrap()).unwrap();
        let db = Db::new(&program, std::path::Path::new(":memory:"));

        invoke_rpc(&program, "Users", "seed", &json!({}), &db).unwrap();

        let guests = invoke_rpc(&program, "Users", "findGuests", &json!({}), &db).unwrap();
        assert_eq!(guests.as_array().unwrap().len(), 2, "findWhere debe devolver SOLAMENTE los 2 Guests");

        let deleted_count = invoke_rpc(&program, "Users", "deleteGuests", &json!({}), &db).unwrap();
        assert_eq!(deleted_count, json!(2), "deleteWhere debe eliminar exactamente 2 usuarios Guest");

        let remaining = invoke_rpc(&program, "Users", "remaining", &json!({}), &db).unwrap();
        let arr = remaining.as_array().unwrap();
        assert_eq!(arr.len(), 1, "deleteWhere NO debe tocar al usuario Admin");
        assert_eq!(arr[0]["name"], json!("AdminUser"));
    }

    #[test]
    fn reopening_the_same_file_after_dropping_the_connection_still_has_the_previously_inserted_rows() {
        let program = optional_shapes_program();
        let path = std::env::temp_dir().join("c_script_test_reopen_persists.db");
        let _ = std::fs::remove_file(&path); // por si quedó de una corrida anterior interrumpida

        {
            let db = Db::new(&program, &path);
            invoke_rpc(&program, "S", "add", &json!({"x": {"name": "persistente", "note": null}}), &db).unwrap();
        } // `db` (y con él la Connection real) se dropea acá

        let db2 = Db::new(&program, &path);
        let Value::List(rows) = db2.call("items", "all", vec![]).unwrap() else { panic!("se esperaba una lista") };
        assert_eq!(rows.len(), 1, "la fila insertada en la conexión anterior tiene que seguir ahí al reabrir el mismo archivo");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    #[should_panic(expected = "schema incompatible que no se puede migrar automáticamente")]
    fn reopening_with_an_incompatible_schema_panics_instead_of_silently_proceeding() {
        let path = std::env::temp_dir().join("c_script_test_schema_mismatch.db");
        let _ = std::fs::remove_file(&path);

        let original = program_from("type Item = { id: Int, name: String } db { items: Item[] }");
        drop(Db::new(&original, &path));

        let changed = program_from("type Item = { id: Int, name: String, extra: Int } db { items: Item[] }");
        let _ = Db::new(&changed, &path); // tiene que hacer panic acá porque 'extra: Int' es NOT NULL sin default
    }

    #[test]
    fn reopening_with_new_optional_field_auto_migrates_successfully() {
        let path = std::env::temp_dir().join("c_script_test_schema_auto_migration.db");
        let _ = std::fs::remove_file(&path);

        let original = program_from("type Item = { id: Int, name: String } db { items: Item[] }");
        {
            let db = Db::new(&original, &path);
            let _ = db.call("items", "insert", vec![Value::Struct(vec![("name".into(), Value::Str("Primer Item".into()))])]).unwrap();
        }

        // Abrir con un nuevo campo opcional: se auto-migra con ALTER TABLE ADD COLUMN sin error ni pérdida de datos
        let evolved = program_from("type Item = { id: Int, name: String, note?: String } db { items: Item[] }");
        let db_evolved = Db::new(&evolved, &path);
        let items = db_evolved.call("items", "all", vec![]).unwrap();
        let Value::List(rows) = items else { panic!("se esperaba lista"); };
        assert_eq!(rows.len(), 1);
        let _ = std::fs::remove_file(&path);
    }

    // ---- matriz de comportamiento de auto-migrate (PLAN.md §9.1.1): las 4
    // clases de cambio de schema que NO son "agregar una columna opcional
    // nueva" -- todas fallan fuerte, con el mismo criterio ya documentado en
    // GRAMMAR.md §3.17 para SQLite. Cada test confirma un caso puntual con
    // el mensaje real, no solo "algo falló".

    #[test]
    #[should_panic(expected = "schema incompatible que no se puede migrar automáticamente")]
    fn reopening_after_dropping_a_column_from_the_link_panics_instead_of_orphaning_it_silently() {
        let path = std::env::temp_dir().join("c_script_test_schema_dropped_column.db");
        let _ = std::fs::remove_file(&path);

        let original = program_from("type Item = { id: Int, name: String, legacy?: String } db { items: Item[] }");
        drop(Db::new(&original, &path));

        // "legacy" ya no está declarada -- la columna física queda huérfana,
        // y eso por sí solo es suficiente para que la comparación de schema
        // (por CONJUNTO, no solo por columnas faltantes) falle.
        let dropped = program_from("type Item = { id: Int, name: String } db { items: Item[] }");
        let _ = Db::new(&dropped, &path);
    }

    #[test]
    #[should_panic(expected = "schema incompatible que no se puede migrar automáticamente")]
    fn renaming_a_column_panics_because_the_old_name_stays_orphaned_even_if_the_new_one_could_auto_add() {
        let path = std::env::temp_dir().join("c_script_test_schema_renamed_column.db");
        let _ = std::fs::remove_file(&path);

        let original = program_from("type Item = { id: Int, note?: String } db { items: Item[] }");
        drop(Db::new(&original, &path));

        // "note" -> "comment": aunque "comment" sea opcional (auto-agregable
        // por sí sola), "note" sigue existiendo físicamente y ya no está
        // declarada -- el mismo caso que el drop de arriba, disparado por un
        // rename en vez de una eliminación. No hay detección de rename: para
        // el runtime esto es indistinguible de "borré una columna y agregué
        // otra sin relación".
        let renamed = program_from("type Item = { id: Int, comment?: String } db { items: Item[] }");
        let _ = Db::new(&renamed, &path);
    }

    #[test]
    #[should_panic(expected = "schema incompatible que no se puede migrar automáticamente")]
    fn changing_a_columns_type_panics_instead_of_altering_it() {
        let path = std::env::temp_dir().join("c_script_test_schema_type_change.db");
        let _ = std::fs::remove_file(&path);

        let original = program_from("type Item = { id: Int, userId: Int } db { items: Item[] }");
        drop(Db::new(&original, &path));

        // Int -> String sobre una columna EXISTENTE: el loop de auto-migrate
        // solo agrega columnas FALTANTES, nunca altera el tipo de una que ya
        // existe -- confirma el caso real reportado (userId: Int -> String).
        let retyped = program_from("type Item = { id: Int, userId: String } db { items: Item[] }");
        let _ = Db::new(&retyped, &path);
    }

    #[test]
    #[should_panic(expected = "schema incompatible que no se puede migrar automáticamente")]
    fn making_a_required_field_optional_panics_because_the_column_stays_not_null() {
        let path = std::env::temp_dir().join("c_script_test_schema_required_to_optional.db");
        let _ = std::fs::remove_file(&path);

        let original = program_from("type Item = { id: Int, name: String } db { items: Item[] }");
        drop(Db::new(&original, &path));

        // La columna física sigue NOT NULL -- el auto-migrate no toca
        // constraints de una columna que ya existe, solo agrega las que
        // faltan.
        let optional = program_from("type Item = { id: Int, name?: String } db { items: Item[] }");
        let _ = Db::new(&optional, &path);
    }

    #[test]
    #[should_panic(expected = "schema incompatible que no se puede migrar automáticamente")]
    fn making_an_optional_field_required_panics_because_the_column_stays_nullable() {
        let path = std::env::temp_dir().join("c_script_test_schema_optional_to_required.db");
        let _ = std::fs::remove_file(&path);

        let original = program_from("type Item = { id: Int, name?: String } db { items: Item[] }");
        drop(Db::new(&original, &path));

        let required = program_from("type Item = { id: Int, name: String } db { items: Item[] }");
        let _ = Db::new(&required, &path);
    }

    // ---- test runner integrado y aislamiento (PLAN.md §5, Eje 2) ----

    #[test]
    fn run_program_tests_executes_tests_with_isolated_db_state() {
        let code = r#"
        type Item = { id: Int, name: String }
        db { items: Item[] }

        service ItemsService {
            rpc create(name: String) -> Item {
                db.items.insert(Item { id: 0, name: name })
            }
            rpc count() -> Int {
                db.items.all().length()
            }
        }

        test "primer test inserta y valida cuenta 1" {
            let item = ItemsService.create("Primer Item");
            assert(item.name == "Primer Item");
            assert(ItemsService.count() == 1, "deberia haber 1 item");
        }

        test "segundo test empieza con base de datos limpia y aislada" {
            // El estado de la DB anterior no debe contaminar este test
            assert(ItemsService.count() == 0, "la base de datos debe estar limpia");
            let item = ItemsService.create("Segundo Item");
            assert(ItemsService.count() == 1);
        }
        "#;
        let program = crate::parser::parse(crate::lexer::tokenize(code).unwrap()).unwrap();
        let summary = run_program_tests(&program).expect("los tests deberian correr");
        assert_eq!(summary.total, 2);
        assert_eq!(summary.passed, 2);
        assert!(summary.failed.is_empty());
    }

    #[test]
    fn run_program_tests_reports_assertion_and_panic_failures() {
        let code = r#"
        test "test que pasa" {
            assert(1 + 1 == 2);
        }

        test "test que falla por asercion" {
            assert(false, "condicion esperada falsa");
        }

        test "test que falla por panic" {
            panic("algo exploto");
        }
        "#;
        let program = crate::parser::parse(crate::lexer::tokenize(code).unwrap()).unwrap();
        let summary = run_program_tests(&program).expect("ejecucion de tests");
        assert_eq!(summary.total, 3);
        assert_eq!(summary.passed, 1);
        assert_eq!(summary.failed.len(), 2);
        assert_eq!(summary.failed[0].0, "test que falla por asercion");
        assert!(summary.failed[0].1.contains("condicion esperada falsa"));
        assert_eq!(summary.failed[1].0, "test que falla por panic");
        assert!(summary.failed[1].1.contains("algo exploto"));
    }

    #[test]
    fn aggregate_by_methods_group_and_aggregate_for_real_against_sqlite() {
        // GRAMMAR.md §3.52: sumBy/countBy/avgBy/maxBy/minBy -- corren contra
        // el SQLite en memoria real de `test` (no un mock), incluido el caso
        // de agrupar por un campo ENUM, que tiene que devolver el enum REAL
        // como key (no degradar a String) -- `scalar_cell_to_value` es lo
        // que hace eso en runtime/db.rs.
        let code = r#"
        enum Plan { Free, Pro, Enterprise }
        type Order = { id: Int, planId: String, plan: Plan, amountCents: Int, score: Float }
        db { orders: Order[] }

        service Orders {
            rpc create(planId: String, plan: Plan, amountCents: Int, score: Float) -> Order {
                db.orders.insert(Order { id: 0, planId: planId, plan: plan, amountCents: amountCents, score: score })
            }
        }

        test "sumBy/countBy/avgBy/maxBy/minBy contra datos reales" {
            Orders.create("pro", Plan.Pro {}, 2000, 4.5);
            Orders.create("pro", Plan.Pro {}, 2000, 3.5);
            Orders.create("free", Plan.Free {}, 0, 5.0);
            Orders.create("ent", Plan.Enterprise {}, 10000, 2.0);
            Orders.create("ent", Plan.Enterprise {}, 15000, 1.0);

            let revenue = db.orders.sumBy(|o: Order| { o.planId }, |o: Order| { o.amountCents });
            assert(revenue.length() == 3, "una fila por planId distinto");

            let counts = db.orders.countBy(|o: Order| { o.plan });
            assert(counts.length() == 3, "una fila por variante de Plan distinta");

            let avgs = db.orders.avgBy(|o: Order| { o.planId }, |o: Order| { o.score });
            assert(avgs.length() == 3);

            let maxes = db.orders.maxBy(|o: Order| { o.planId }, |o: Order| { o.amountCents });
            assert(maxes.length() == 3);

            let mins = db.orders.minBy(|o: Order| { o.planId }, |o: Order| { o.amountCents });
            assert(mins.length() == 3);
        }
        "#;
        let program = crate::parser::parse(crate::lexer::tokenize(code).unwrap()).unwrap();
        let summary = run_program_tests(&program).expect("ejecucion de tests");
        assert_eq!(summary.total, 1);
        assert_eq!(summary.passed, 1, "{:?}", summary.failed);
    }

    #[test]
    fn count_by_on_an_enum_field_returns_the_real_enum_variant_as_key() {
        // Verificación de valor exacto, no solo longitud -- confirma que
        // `scalar_cell_to_value` reconstruye `Value::Variant` (no
        // `Value::Str`) para una key agrupada por un campo enum.
        let code = r#"
        enum Plan { Free, Pro }
        type Order = { id: Int, plan: Plan }
        type PlanCount = { key: Plan, value: Int }
        db { orders: Order[] }

        service Orders {
            rpc create(plan: Plan) -> Order { db.orders.insert(Order { id: 0, plan: plan }) }
        }

        test "la key de countBy sobre un enum es el enum real, no un String" {
            Orders.create(Plan.Pro {});
            Orders.create(Plan.Pro {});
            Orders.create(Plan.Free {});

            let counts = db.orders.countBy(|o: Order| { o.plan });
            let proCount = counts.filter(|row: PlanCount| { row.key == Plan.Pro {} });
            assert(proCount.length() == 1, "un solo grupo Pro");
            assert(proCount[0].value == 2, "dos orders con plan Pro");
        }
        "#;
        let program = crate::parser::parse(crate::lexer::tokenize(code).unwrap()).unwrap();
        let summary = run_program_tests(&program).expect("ejecucion de tests");
        assert_eq!(summary.total, 1);
        assert_eq!(summary.passed, 1, "{:?}", summary.failed);
    }

    #[test]
    fn aggregation_supports_int64_as_group_key_and_as_aggregated_value() {
        // GRAMMAR.md §3.65: antes de esta ronda, Int64 estaba EXPLÍCITAMENTE
        // rechazado como key y como value en sumBy/etc. -- y si hubiera
        // colado, `scalar_cell_to_value` tampoco distinguía Int64 de Int,
        // así que el valor real hubiera llegado mal etiquetado (Value::Int
        // en vez de Value::Int64). Este test fija las dos cosas: que
        // compila Y que el tipo del resultado es Int64 de verdad, no un Int
        // que solo "parece" andar porque el valor cabe en los dos.
        let code = r#"
        type Sale = { id: Int, region: Int64, amount: Int64 }
        type RegionTotal = { key: Int64, value: Int64 }
        db { sales: Sale[] }

        service Sales {
            rpc create(region: Int64, amount: Int64) -> Sale {
                db.sales.insert(Sale { id: 0, region: region, amount: amount })
            }
        }

        test "Int64 como key y como value en sumBy" {
            Sales.create(1.toInt64(), 500.toInt64());
            Sales.create(1.toInt64(), 700.toInt64());
            Sales.create(2.toInt64(), 300.toInt64());

            let byRegion = db.sales.sumBy(|s: Sale| { s.region }, |s: Sale| { s.amount });
            assert(byRegion.length() == 2, "una fila por region distinta");

            let total = db.sales.sumBy(|s: Sale| { s.region }, |s: Sale| { s.amount })
                .filter(|row: RegionTotal| { row.key == 1.toInt64() })[0]
                .value;
            assert(total == 1200.toInt64(), "500+700 para la region 1, como Int64 real");
        }
        "#;
        let program = crate::parser::parse(crate::lexer::tokenize(code).unwrap()).unwrap();
        let summary = run_program_tests(&program).expect("ejecucion de tests");
        assert_eq!(summary.total, 1);
        assert_eq!(summary.passed, 1, "{:?}", summary.failed);
    }

    #[test]
    fn math_and_crypto_and_string_methods_work_in_runtime() {
        let code = r#"
        test "stdlib builtins math, crypto and strings" {
            // Math
            assert(math.sqrt(16.0) == 4.0);
            assert(math.abs(-5.5) == 5.5);
            assert(math.floor(3.7) == 3);
            assert(math.ceil(3.2) == 4);
            assert(math.round(3.6) == 4);
            assert(math.min(10.0, 20.0) == 10.0);
            assert(math.max(10.0, 20.0) == 20.0);
            assert(math.pow(2.0, 3.0) == 8.0);

            // Crypto
            let sha = crypto.hashSha256("hello");
            assert(sha == "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
            // HMAC-SHA256(key="key", msg="The quick brown fox jumps over the
            // lazy dog") -- vector de referencia calculado con Python
            // (hmac.new(b"key", b"...", hashlib.sha256).hexdigest()), no
            // inventado a mano.
            let mac = crypto.hmacSha256("key", "The quick brown fox jumps over the lazy dog");
            assert(mac == "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8");
            let pwd = "secret_password_123";
            let hash = crypto.hashPassword(pwd);
            assert(crypto.verifyPassword(pwd, hash) == true);
            assert(crypto.verifyPassword("wrong", hash) == false);

            // String
            let s = "  Hola Mundo  ";
            assert(s.trim() == "Hola Mundo");
            assert(s.trim().toUpper() == "HOLA MUNDO");
            assert(s.trim().toLower() == "hola mundo");
            assert(s.trim().startsWith("Hola") == true);
            assert(s.trim().endsWith("Mundo") == true);
            assert("<script>alert(1)</script>".escapeHtml() == "&lt;script&gt;alert(1)&lt;/script&gt;");
            assert("a & b \"quoted\" 'single'".escapeHtml() == "a &amp; b &quot;quoted&quot; &#39;single&#39;");

            // UUID & Crypto -- crypto.uuid() devuelve Type::Uuid, no
            // String (GRAMMAR.md §3.70): .toString() lo baja explícitamente
            // para poder usar métodos de String sobre él.
            let u = crypto.uuid();
            let uStr = u.toString();
            assert(uStr.length() == 36);
            assert(uStr.contains("-"));

            // JSON
            let json_str = json.stringify("hola json");
            assert(json_str == "\"hola json\"");
            let parsed = json.parse("{\"status\": \"ok\", \"count\": 42}");
            assert(parsed.status == "ok");
            assert(parsed.count == 42);

            // Base64
            let encoded = base64.encode("Link Language");
            assert(encoded == "TGluayBMYW5ndWFnZQ==");
            let decoded = base64.decode(encoded);
            assert(decoded == "Link Language");

            // List utilities
            let items: String[] = ["uno", "dos", "tres"];
            assert(items.join(", ") == "uno, dos, tres");
            let rev = items.reverse();
            assert(rev.join("-") == "tres-dos-uno");

            // Timestamp
            let t = now();
            let ms = t.toMillis();
            assert(ms.toInt() > 0);
            assert(t.toIsoString().contains("T"));
            assert(t.diffMillis(t).toInt() == 0);
        }
        "#;
        let program = crate::parser::parse(crate::lexer::tokenize(code).unwrap()).unwrap();
        let summary = run_program_tests(&program).expect("ejecucion de tests stdlib");
        assert_eq!(summary.total, 1);
        assert_eq!(summary.passed, 1, "todos los asserts de stdlib debieron pasar");
    }

    #[test]
    fn int_int64_float_and_bool_convert_to_string() {
        // GRAMMAR.md §3.55: hasta esta ronda no había NINGUNA forma de
        // interpolar un número o un bool en un mensaje -- ni siquiera
        // "codigo: " + n compilaba, porque '+' exige String+String sin
        // coercion implicita (§3.7). Mismo patron que toInt64()/toIsoString():
        // conversion EXPLICITA, nunca automatica.
        let code = r#"
        test "conversiones a String" {
            assert(42.toString() == "42");
            assert((-7).toString() == "-7");
            let big: Int64 = 42.toInt64();
            assert(big.toString() == "42");
            assert(3.5.toString() == "3.5");
            assert(true.toString() == "true");
            assert(false.toString() == "false");
            // Compone con '+' de String una vez convertido: eso es
            // justamente lo que estaba bloqueado antes de esta ronda.
            let n = 5;
            assert("codigo: " + n.toString() == "codigo: 5");
        }
        "#;
        let program = crate::parser::parse(crate::lexer::tokenize(code).unwrap()).unwrap();
        let summary = run_program_tests(&program).expect("ejecucion de tests de toString");
        assert_eq!(summary.passed, 1, "fallaron asserts de toString: {summary:?}");
    }

    /// Que un round-trip de contraseña funcione no prueba NADA sobre seguridad:
    /// el SHA-256 con sal fija que había antes también pasaba ese test. Lo que
    /// hay que fijar son las propiedades que lo hacen resistente, y cada una de
    /// estas fallaba con la implementación anterior.
    #[test]
    fn crypto_properties_that_the_old_implementation_did_not_have() {
        let code = r#"
        test "propiedades de crypto" {
            // 1. La MISMA contraseña hasheada dos veces da hashes DISTINTOS:
            //    la sal es aleatoria por contraseña. Antes la sal era la
            //    constante "link_salt_2026" para todo programa Link del mundo,
            //    asi que dos usuarios con la misma clave compartian hash y una
            //    sola rainbow table los rompia a todos.
            let a = crypto.hashPassword("misma-contrasena");
            let b = crypto.hashPassword("misma-contrasena");
            assert(a != b, "dos hashes de la misma contrasena deben diferir");

            // 2. Y aun asi, las dos verifican.
            assert(crypto.verifyPassword("misma-contrasena", a), "verifica contra el primero");
            assert(crypto.verifyPassword("misma-contrasena", b), "verifica contra el segundo");
            assert(crypto.verifyPassword("otra", a) == false, "rechaza la equivocada");

            // 3. Es un KDF de verdad, no un digest: el formato PHC lo declara.
            assert(a.startsWith("$argon2id$"), "el hash declara el algoritmo y sus parametros");

            // 4. Los hashes viejos siguen verificando, para poder migrar sin
            //    dejar afuera a los usuarios ya registrados de una app en
            //    produccion. Se reconstruye aca uno exactamente como lo escribia
            //    la implementacion anterior: sha256(sal_fija + contrasena).
            let hex = crypto.hashSha256("link_salt_2026" + "clave-vieja");
            let legado = "sha256$link_salt_2026$" + hex;
            assert(crypto.verifyPassword("clave-vieja", legado), "un hash legado valido verifica");
            assert(crypto.verifyPassword("otra-clave", legado) == false, "y uno que no corresponde, no");

            // 4b. crypto.isLegacyHash(): distingue el formato legado del
            //     Argon2id real -- la señal que faltaba para migrar
            //     proactivamente en vez de mirar el prefijo a mano.
            assert(crypto.isLegacyHash(legado), "sha256$... es legado");
            assert(crypto.isLegacyHash(a) == false, "un hash Argon2id real no es legado");

            // 5. Dos tokens seguidos son distintos. Antes salian de SHA-256 del
            //    reloj: dos llamadas en el mismo nanosegundo devolvian el mismo
            //    token, y quien pudiera acotar el instante podia adivinarlo.
            let t1 = crypto.randomToken(32);
            let t2 = crypto.randomToken(32);
            assert(t1 != t2, "dos tokens consecutivos deben diferir");
            assert(t1.length() == 32, "respeta el largo pedido");

            // 6. Lo mismo para uuid(), que ademas dice ser v4 en su formato.
            let u1 = crypto.uuid();
            let u2 = crypto.uuid();
            assert(u1 != u2, "dos uuid consecutivos deben diferir");
            assert(u1.toString().length() == 36, "formato uuid");
        }
        "#;
        let program = crate::parser::parse(crate::lexer::tokenize(code).unwrap()).unwrap();
        let summary = run_program_tests(&program).expect("ejecucion de tests de crypto");
        assert_eq!(summary.passed, 1, "fallaron asserts de crypto: {summary:?}");
    }

    #[test]
    fn crypto_random_int_and_timing_safe_equal() {
        let code = r#"
        test "randomInt y timingSafeEqual" {
            // 1. Cae siempre dentro del rango pedido, extremos incluidos.
            let n = crypto.randomInt(1, 6);
            assert(n >= 1 && n <= 6, "cae dentro del rango [min, max]");

            // 2. Rango degenerado: min == max siempre devuelve ese unico valor.
            assert(crypto.randomInt(7, 7) == 7, "rango de un solo valor");

            // 3. Sobre varias muestras de un OTP de 6 digitos no siempre sale
            //    el mismo valor -- una implementacion sesgada (ej. al primer
            //    valor del rango) pasaria el test de rango pero fallaria este.
            let a = crypto.randomInt(100000, 999999);
            let b = crypto.randomInt(100000, 999999);
            let c = crypto.randomInt(100000, 999999);
            assert(a != b || b != c, "no siempre el mismo valor en llamadas consecutivas");

            // 4. timingSafeEqual compara como == en el caso feliz...
            assert(crypto.timingSafeEqual("secreto123", "secreto123") == true);
            assert(crypto.timingSafeEqual("secreto123", "otro-valor") == false);
            // ...incluyendo largos distintos, sin crashear ni comparar mal.
            assert(crypto.timingSafeEqual("corto", "un-valor-mas-largo") == false);
        }
        "#;
        let program = crate::parser::parse(crate::lexer::tokenize(code).unwrap()).unwrap();
        let summary = run_program_tests(&program).expect("ejecucion de tests de randomInt/timingSafeEqual");
        assert_eq!(summary.passed, 1, "fallaron asserts: {summary:?}");
    }

}
