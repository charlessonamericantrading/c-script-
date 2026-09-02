// Runtime mínimo interpretado (PLAN.md §2.4, Fase 0): un tree-walking
// interpreter que ejecuta cuerpos de rpc/fn contra un "db" en memoria.
// No es el runtime final del lenguaje — Fase 1+ compila a WASM/nativo
// (PLAN.md §4) — esto solo alcanza para que la demo E2E responda de verdad.

pub mod db;
pub(crate) mod encryption;
pub(crate) mod excel;
pub mod mcp;
pub(crate) mod pdf;
pub mod server;
pub mod session;
pub(crate) mod store;
pub(crate) mod timestamp;

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
    /// Escalado ×`DECIMAL_SCALE` (10.000, 4 decimales) -- ver la doc de
    /// `Type::Decimal` (types.rs, GRAMMAR.md §3.184).
    Decimal(i128),
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
    /// `db.<c>.orderBy(...)` ya aplicado (GRAMMAR.md §3.230): la colección
    /// más sus claves de orden, a la espera del `all()`/`page()`/
    /// `findWhere()` que las convierta en un ORDER BY real. Nunca es el
    /// resultado de un rpc (el checker lo garantiza) ni cruza a JSON.
    DbQuery(DbQuery),
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
    /// Marcador interno para el módulo `pdf` (GRAMMAR.md §3.201)
    Pdf,
    /// Marcador interno para el módulo `excel` (GRAMMAR.md §3.202)
    Excel,
    /// Marcador interno para el módulo `ai` (GRAMMAR.md §3.235)
    Ai,
    /// Marcador interno para el módulo `mcp` (GRAMMAR.md §3.203)
    Mcp,
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

/// GRAMMAR.md §3.206: exhaustivo A PROPÓSITO, sin brazo `_`. Decide si
/// `Expr::FieldAccess` sobre este `Value` puede volverse un
/// `Value::BoundMethod` a la espera de que el `Expr::Call` que lo envuelve
/// corra -- este chequeo es un allowlist SEPARADO del `match method` real
/// de cada tipo (más abajo en este archivo), y por eso mismo es exactamente
/// donde ya se coló un bug real una vez: `Value::Decimal` tuvo un método
/// bien implementado ahí abajo pero INALCANZABLE durante meses porque
/// faltaba acá (GRAMMAR.md §3.199). Antes, este chequeo era una unión de
/// patrones inline con un brazo `other => Err(...)` -- agregar una
/// variante `Value` nueva con métodos propios y olvidar sumarla ahí
/// compilaba limpio y fallaba en runtime, silencioso hasta que alguien
/// llamara al método. Extraído a una función propia SIN `_` para que ese
/// mismo olvido sea un error de `cargo build` -- toda variante de `Value`
/// tiene que clasificarse acá, explícitamente, la próxima vez que se
/// agregue una.
fn supports_bound_method_access(v: &Value) -> bool {
    match v {
        Value::Service(_)
        | Value::DbCollection(_)
        | Value::DbQuery(_)
        | Value::List(_)
        | Value::Int(_)
        | Value::Int64(_)
        | Value::Decimal(_)
        | Value::Float(_)
        | Value::Bool(_)
        | Value::Str(_)
        | Value::Uuid(_)
        | Value::Timestamp(_)
        | Value::Auth
        | Value::Math
        | Value::Crypto
        | Value::Http
        | Value::Json
        | Value::Base64
        | Value::Pdf
        | Value::Excel
        | Value::Ai
        | Value::Mcp
        | Value::Env
        | Value::Request
        | Value::Smtp
        | Value::Response => true,
        // Estos ya se manejan ANTES de llegar a `supports_bound_method_access`
        // (Struct/Variant/Db), o genuinamente no tienen ningún método propio
        // hoy (Null/Tuple/BoundMethod/FnRef/Closure) -- `false` es la
        // clasificación correcta para las cinco últimas, no un catch-all
        // perezoso.
        Value::Struct(_)
        | Value::Variant { .. }
        | Value::Db
        | Value::Null
        | Value::Tuple(_)
        | Value::BoundMethod(_, _)
        | Value::FnRef(_)
        | Value::Closure(_, _, _) => false,
    }
}

/// GRAMMAR.md §3.206: exhaustivo A PROPÓSITO, sin brazo `_` -- clasifica
/// cada variante `Value` que es un marcador interno singleton sin datos
/// propios (los módulos `db`/`auth`/`math`/`crypto`/`http`/`json`/`base64`/
/// `pdf`/`excel`/`mcp`/`env`/`request`/`smtp`/`response`). `impl PartialEq
/// for Value` la usa para tratar `X == X` como `true` para cualquiera de
/// ellos, nunca entre sí -- antes esto era 14 brazos `(X, X) => true`
/// escritos a mano, y 7 de ellos (`Pdf`/`Excel`/`Mcp`/`Env`/`Request`/
/// `Smtp`/`Response`) faltaron durante meses, cayendo en el `_ => false`
/// final sin que nada lo marcara (GRAMMAR.md §3.162, v1.162.0). Al vivir
/// acá, sin `_`, agregar un marcador interno nuevo a `Value` sin sumarlo a
/// esta clasificación es ahora un error de `cargo build`, no un `X == X`
/// silenciosamente `false`.
fn is_marker_singleton(v: &Value) -> bool {
    match v {
        Value::Db
        | Value::Auth
        | Value::Math
        | Value::Crypto
        | Value::Http
        | Value::Json
        | Value::Base64
        | Value::Pdf
        | Value::Excel
        | Value::Ai
        | Value::Mcp
        | Value::Env
        | Value::Request
        | Value::Smtp
        | Value::Response => true,
        Value::Int(_)
        | Value::Int64(_)
        | Value::Decimal(_)
        | Value::Timestamp(_)
        | Value::Float(_)
        | Value::Str(_)
        | Value::Uuid(_)
        | Value::Bool(_)
        | Value::Null
        | Value::Struct(_)
        | Value::Variant { .. }
        | Value::List(_)
        | Value::Tuple(_)
        | Value::DbCollection(_)
        | Value::DbQuery(_)
        | Value::Service(_)
        | Value::BoundMethod(_, _)
        | Value::FnRef(_)
        | Value::Closure(_, _, _) => false,
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        use Value::*;
        match (self, other) {
            (Int(a), Int(b)) => a == b,
            (Int64(a), Int64(b)) => a == b,
            (Decimal(a), Decimal(b)) => a == b,
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
            (DbCollection(a), DbCollection(b)) => a == b,
            (DbQuery(a), DbQuery(b)) => a == b,
            (Service(a), Service(b)) => a == b,
            // Ver `is_marker_singleton` -- cualquiera de los 14 módulos
            // internos comparado consigo mismo da `true`, nunca entre sí.
            (a, b) if is_marker_singleton(a) && is_marker_singleton(b) => std::mem::discriminant(a) == std::mem::discriminant(b),
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
            Value::Decimal(n) => f.debug_tuple("Decimal").field(&format_decimal(*n)).finish(),
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
            Value::DbQuery(q) => f.debug_tuple("DbQuery").field(q).finish(),
            Value::Auth => write!(f, "Auth"),
            Value::Service(name) => f.debug_tuple("Service").field(name).finish(),
            Value::Math => write!(f, "Math"),
            Value::Crypto => write!(f, "Crypto"),
            Value::Http => write!(f, "Http"),
            Value::Json => write!(f, "Json"),
            Value::Base64 => write!(f, "Base64"),
            Value::Pdf => write!(f, "Pdf"),
            Value::Excel => write!(f, "Excel"),
            Value::Ai => write!(f, "Ai"),
            Value::Mcp => write!(f, "Mcp"),
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

/// Extrae un mensaje legible del payload de un panic atrapado con
/// `catch_unwind` -- usado tanto por `Expr::Transaction` (GRAMMAR.md
/// §3.163) como por el loop de `@cron` (server.rs, GRAMMAR.md §3.164). El
/// payload es `Box<dyn Any + Send>`: casi siempre un `&str` (`panic!("...")`
/// literal) o un `String` (`panic!("{}", x)`/`.expect(fmt)`) -- cualquier
/// otro tipo (raro; alguien hizo `panic_any` con un valor propio) cae al
/// mensaje genérico en vez de fallar a su vez tratando de formatearlo.
pub(crate) fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic sin mensaje de texto".to_string()
    }
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
            if name == "pdf" {
                return Ok(Value::Pdf);
            }
            if name == "excel" {
                return Ok(Value::Excel);
            }
            if name == "ai" {
                return Ok(Value::Ai);
            }
            if name == "mcp" {
                return Ok(Value::Mcp);
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
            if name == "dateFromParts" {
                return Ok(Value::FnRef("dateFromParts".to_string()));
            }
            if name == "sitemapXml" {
                return Ok(Value::FnRef("sitemapXml".to_string()));
            }
            if name == "robotsTxt" {
                return Ok(Value::FnRef("robotsTxt".to_string()));
            }
            if name == "metaTags" {
                return Ok(Value::FnRef("metaTags".to_string()));
            }
            if name == "openGraphTags" {
                return Ok(Value::FnRef("openGraphTags".to_string()));
            }
            if name == "canonicalLink" {
                return Ok(Value::FnRef("canonicalLink".to_string()));
            }
            if name == "jsonLd" {
                return Ok(Value::FnRef("jsonLd".to_string()));
            }
            if name == "staticRoutes" {
                return Ok(Value::FnRef("staticRoutes".to_string()));
            }
            if name == "hreflangLinks" {
                return Ok(Value::FnRef("hreflangLinks".to_string()));
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
            // GRAMMAR.md §3.209: espejo exacto del chequeo del checker
            // (checker.rs, mismo brazo) -- `Enum.Variante` sin campos, sin
            // `{}`, es azúcar por `Enum.Variante {}`. El checker ya
            // garantizó en compile-time que esto es válido (rechaza el
            // caso con campos y el de una variante inexistente), así que
            // acá alcanza con reconstruir el mismo `StructLit` sintético y
            // delegar a la construcción real -- nunca diverge de lo que el
            // checker aprobó, porque es literalmente el mismo camino de
            // evaluación que `Enum.Variante {}` ya usaba.
            if let Expr::Ident(base_name) = &base.node {
                if env.get(base_name).is_none() {
                    if let Some(decl) = checker.enums.get(base_name) {
                        if let Some(variant) = decl.variants.iter().find(|v| &v.name == field) {
                            if variant.fields.as_ref().is_none_or(|fs| fs.is_empty()) {
                                let synthetic = Spanned {
                                    node: Expr::StructLit { name: base_name.clone(), variant: Some(field.clone()), fields: vec![] },
                                    span: e.span,
                                };
                                return eval_expr(&synthetic, env, db, fns, checker, sessions, current_token, step_budget);
                            }
                        }
                    }
                }
            }
            let base_v = eval_expr(base, env, db, fns, checker, sessions, current_token, step_budget)?;
            match base_v {
                Value::Struct(fields) | Value::Variant { fields, .. } => Ok(fields
                    .into_iter()
                    .find(|(n, _)| n == field)
                    .map(|(_, v)| v)
                    .unwrap_or(Value::Null)),
                Value::Db => Ok(Value::DbCollection(field.clone())),
                // GRAMMAR.md §3.199: este allowlist es un segundo lugar,
                // SEPARADO del `match method` real de cada tipo (más abajo
                // en este archivo), que decide si `x.metodo` puede siquiera
                // volverse un `Value::BoundMethod` a la espera de que el
                // `Expr::Call` que lo envuelve corra. `Value::Decimal` tuvo
                // un método bien implementado ahí abajo pero INALCANZABLE
                // durante meses porque faltaba acá -- al agregar una
                // variante `Value` nueva con métodos propios, sumarla ACÁ
                // también, no solo en su `match method`.
                ref v if supports_bound_method_access(v) => Ok(Value::BoundMethod(Box::new(base_v), field.clone())),
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
                    if name == "dateFromParts" {
                        let arg_vs = eval_args(args, env, db, fns, checker, sessions, current_token, step_budget)?;
                        return call_date_from_parts(arg_vs);
                    }
                    if name == "sitemapXml" {
                        let arg_vs = eval_args(args, env, db, fns, checker, sessions, current_token, step_budget)?;
                        return call_sitemap_xml(arg_vs);
                    }
                    if name == "robotsTxt" {
                        let arg_vs = eval_args(args, env, db, fns, checker, sessions, current_token, step_budget)?;
                        return call_robots_txt(arg_vs);
                    }
                    if name == "metaTags" {
                        let arg_vs = eval_args(args, env, db, fns, checker, sessions, current_token, step_budget)?;
                        return call_meta_tags(arg_vs);
                    }
                    if name == "openGraphTags" {
                        let arg_vs = eval_args(args, env, db, fns, checker, sessions, current_token, step_budget)?;
                        return call_open_graph_tags(arg_vs);
                    }
                    if name == "canonicalLink" {
                        let arg_vs = eval_args(args, env, db, fns, checker, sessions, current_token, step_budget)?;
                        return call_canonical_link(arg_vs);
                    }
                    if name == "jsonLd" {
                        let arg_vs = eval_args(args, env, db, fns, checker, sessions, current_token, step_budget)?;
                        return call_json_ld(arg_vs);
                    }
                    if name == "staticRoutes" {
                        let arg_vs = eval_args(args, env, db, fns, checker, sessions, current_token, step_budget)?;
                        return call_static_routes(arg_vs, db);
                    }
                    if name == "hreflangLinks" {
                        let arg_vs = eval_args(args, env, db, fns, checker, sessions, current_token, step_budget)?;
                        return call_hreflang_links(arg_vs);
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
            // `db.vacuum()`/`db.tableStats()` (GRAMMAR.md §3.151): mismo
            // motivo que el atajo de `isSome`/`isNone` arriba -- interceptar
            // ACÁ, antes de la evaluación genérica de `callee` como
            // FieldAccess, evita que `db.vacuum` (evaluado como base de una
            // llamada MÁS LARGA, ej. `db.vacuum.insert(x)` sobre una
            // colección de verdad llamada "vacuum") se malinterprete como
            // el builtin -- este atajo solo dispara cuando `db.vacuum`/
            // `db.tableStats` es DIRECTAMENTE lo que se está llamando, la
            // MISMA distinción que el checker ya hace en `try_builtin_method`
            // (que solo mira `callee` de un `Call`, nunca un `FieldAccess`
            // intermedio).
            if let Expr::FieldAccess { base, field } = &callee.node {
                if (field == "vacuum" || field == "tableStats") && matches!(&base.node, Expr::Ident(n) if n == "db") && !env.contains_key("db")
                {
                    let arg_vs = eval_args(args, env, db, fns, checker, sessions, current_token, step_budget)?;
                    if !arg_vs.is_empty() {
                        return Err(err(format!("'db.{field}' no toma argumentos")));
                    }
                    return match field.as_str() {
                        "vacuum" => {
                            db.run_vacuum().map_err(|e| err(format!("db.vacuum falló: {e}")))?;
                            Ok(Value::Null)
                        }
                        _ => {
                            let stats = db.table_stats().map_err(|e| err(format!("db.tableStats falló: {e}")))?;
                            Ok(Value::Struct(stats.into_iter().map(|(name, count)| (name, Value::Int(count))).collect()))
                        }
                    };
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
            // GRAMMAR.md §3.173: `@check(<expr>)` de nivel type -- solo
            // aplica cuando ESTE literal es un struct puro (`variant:
            // None`); `checker.types` (adentro de `apply_type_level_checks`)
            // no tiene entradas para un enum de todos modos, así que llamar
            // siempre acá sería un no-op inofensivo para `Some(v)`, pero
            // ser explícito documenta la intención sin depender de ese
            // detalle.
            if variant.is_none() {
                apply_type_level_checks(checker, name, &Value::Struct(evaluated.clone()), name)?;
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
        // GRAMMAR.md §3.154: `BEGIN` real antes de evaluar el cuerpo,
        // `COMMIT` (con la publicación diferida de escrituras, ver
        // `Db::commit_transaction`) si termina de correr normal,
        // `ROLLBACK` si CUALQUIER `RuntimeError` se propaga desde adentro
        // -- el checker ya garantizó que no hay ningún `return` alcanzable
        // (block_has_return, checker.rs) ni ninguna otra `transaction`
        // anidada SINTÁCTICA, así que acá no hace falta ningún manejo
        // especial más allá de propagar lo que `eval_block` devuelva.
        //
        // Pilar 1 del roadmap de concurrencia (26/08/2026): BEGIN+cuerpo+
        // COMMIT/ROLLBACK corren adentro de `with_exclusive_connection` --
        // sostiene el candado reentrante de la conexión física por esa
        // duración COMPLETA, para que otro hilo (otra request) nunca pueda
        // intercalar una escritura suya en la MISMA conexión a mitad de
        // esta transacción. Sin esto, con un hilo por request, dos
        // `transaction { }` concurrentes en la MISMA conexión podrían
        // interlear sus escrituras entre sí -- exactamente el bug que una
        // transacción SQL existe para impedir.
        //
        // La entrega de los eventos DIFERIDOS de `stream` (`pending`, ver
        // `commit_transaction`) pasa a propósito FUERA de ese candado --
        // bug real encontrado auditando esta misma sección después de
        // shippear GRAMMAR.md §3.158: `deliver_local` pide el candado de
        // `subscribers`, y `subscribe()` pide esos DOS candados en el orden
        // opuesto (subscribers primero, conexión después -- ver su propio
        // comentario en db.rs) para no perder un evento contra un `insert`
        // concurrente. Entregar acá adentro, con la conexión todavía
        // tomada, habría sido conexión→subscribers en este camino y
        // subscribers→conexión en el otro -- un deadlock clásico de orden
        // de candados cruzado, no una carrera rara: se hubiera disparado la
        // primera vez que un `transaction{}` confirmando y un `stream`
        // suscribiéndose a la misma colección coincidieran en el tiempo.
        //
        // GRAMMAR.md §3.163: `eval_block` del cuerpo va envuelto en
        // `catch_unwind` -- antes de esta ronda, CUALQUIER panic ahí adentro
        // (no solo la división/módulo por cero que ya se volvió
        // `RuntimeError` limpio en §3.162) se llevaba puesto el `BEGIN` real:
        // el hilo de la request muere en el unwind, pero `transaction_
        // pending_publishes` se queda en `Some(...)` para siempre (nadie
        // corre el `*...lock() = None` de `rollback_transaction`) y la
        // conexión SQL compartida se queda con una transacción abierta que
        // nunca ve `COMMIT` ni `ROLLBACK`. Encontrado auditando esta misma
        // sección: todo intento de `transaction{}` POSTERIOR en el proceso
        // falla para siempre con "ya hay una transacción abierta", y toda
        // escritura no transaccional posterior corre sobre esa conexión
        // corrupta y se pierde en silencio al reiniciar (confirmado a mano:
        // 3 filas insertadas, 1 sola sobrevivió el restart). `AssertUnwindSafe`
        // es necesario porque `env`/`db` cargan `Rc<RefCell<_>>`/`Mutex` que
        // el compilador no puede probar `UnwindSafe` en abstracto -- lo que
        // de verdad garantiza que esto es seguro es el `rollback_transaction()`
        // explícito del brazo `Err` de más abajo, que deja el estado de la
        // transacción limpio pase lo que pase adentro.
        Expr::Transaction(block) => {
            let outcome = db.with_exclusive_connection(|| {
                db.begin_transaction().map_err(|e| err(format!("no se pudo iniciar la transacción: {e}")))?;
                let unwind_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    eval_block(block, env, db, fns, checker, sessions, current_token, step_budget)
                }));
                let block_result = match unwind_result {
                    Ok(r) => r,
                    Err(payload) => {
                        let msg = panic_payload_message(&*payload);
                        Err(err(format!("la transacción abortó por un error interno inesperado: {msg}")))
                    }
                };
                match block_result {
                    Ok(value) => match db.commit_transaction() {
                        Ok(pending) => Ok((value, pending)),
                        Err(e) => {
                            db.rollback_transaction();
                            Err(err(format!("no se pudo confirmar la transacción: {e}")))
                        }
                    },
                    Err(e) => {
                        db.rollback_transaction();
                        Err(e)
                    }
                }
            });
            match outcome {
                Ok((value, pending)) => {
                    for (collection, json) in pending {
                        db.deliver_local(&collection, &json);
                        if db.is_postgres() {
                            db.notify_remote(&collection, &json);
                        }
                    }
                    Ok(value)
                }
                Err(e) => Err(e),
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
        // AUDIT-2026-08-27.md #16: `+`/`-`/`*` sobre `Int`/`Int64` pasaban
        // por `numeric_op` con `a+b`/`a-b`/`a*b` crudo -- el mismo riesgo de
        // panic (perfil `dev`) o wrap silencioso (perfil `release`) que
        // `/`/`%` ya tenían antes de GRAMMAR.md §3.162, solo que el
        // disparador es más raro (un valor cerca de `i64::MAX`/`MIN`, no
        // "cualquier cero") -- pero igual de real con datos de usuario
        // (IDs tipo snowflake, montos grandes, un contador que crece sin
        // límite declarado). Mismo mecanismo que Div/Rem: `checked_*` +
        // `RuntimeError` limpio en vez de panic/wrap.
        // PLAN.md §9.14 ítem 2: concatenación pura -- a diferencia de
        // `.sum()` (GRAMMAR.md §3.101), nunca necesita inspeccionar el tipo
        // de elemento (una lista vacía no es ambigua para "pegar dos Vec",
        // solo lo es para "sumar sus elementos"), así que no hereda esa
        // limitación.
        Add => match (l, r) {
            (Value::Str(a), Value::Str(b)) => Ok(Value::Str(a + &b)),
            (Value::Decimal(a), Value::Decimal(b)) => decimal_add(a, b),
            (Value::List(mut a), Value::List(b)) => {
                a.extend(b);
                Ok(Value::List(a))
            }
            (l, r) => checked_int_numeric_op(l, r, i64::checked_add, |a, b| a + b, |a, b| {
                err(format!("desborde aritmético al sumar {a} y {b}"))
            }),
        },
        Sub => match (l, r) {
            (Value::Decimal(a), Value::Decimal(b)) => decimal_sub(a, b),
            (l, r) => checked_int_numeric_op(l, r, i64::checked_sub, |a, b| a - b, |a, b| {
                err(format!("desborde aritmético al restar {b} de {a}"))
            }),
        },
        // GRAMMAR.md §3.184: `*`/`/` sobre Decimal necesitan re-escalar
        // (redondeo half-up, ver `decimal_mul`/`decimal_div`) -- una
        // operación distinta de "la misma cuenta en un ancho más grande"
        // que `checked_int_numeric_op` asume para Int/Int64/Float, así que
        // Decimal se intercepta ANTES de llegar ahí.
        Mul => match (l, r) {
            (Value::Decimal(a), Value::Decimal(b)) => decimal_mul(a, b),
            (l, r) => checked_int_numeric_op(l, r, i64::checked_mul, |a, b| a * b, |a, b| {
                err(format!("desborde aritmético al multiplicar {a} por {b}"))
            }),
        },
        // GRAMMAR.md §3.162: sobre ENTEROS, `a / 0` y `i64::MIN / -1` son
        // panics de Rust, no valores. Un panic acá no es un error de
        // runtime normal: mata el hilo de la request sin pasar por ningún
        // camino de limpieza, y adentro de un `transaction { }` deja la
        // transacción SQL abierta para siempre (bug real reproducido, con
        // pérdida silenciosa de datos ya confirmados al cliente). El
        // divisor casi siempre viene de datos del usuario, así que esto era
        // trivialmente alcanzable. El camino de `Float` no necesita guarda:
        // IEEE-754 define /0 como inf/NaN, nunca panica.
        Div => match (l, r) {
            (Value::Decimal(a), Value::Decimal(b)) => decimal_div(a, b),
            (l, r) => checked_int_numeric_op(l, r, i64::checked_div, |a, b| a / b, |a, b| div_or_rem_overflow_message("dividir", a, b)),
        },
        // `%` sobre Decimal ya queda rechazado por el checker (GRAMMAR.md
        // §3.184) -- nunca alcanzable acá con un Value::Decimal real.
        Rem => {
            checked_int_numeric_op(l, r, i64::checked_rem, |a, b| a % b, |a, b| div_or_rem_overflow_message("calcular el resto de", a, b))
        }
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
        // AUDIT-2026-08-27.md #16: `i64::MIN` es el único valor cuyo
        // negativo no representa -- `-i64::MIN` desborda (mismo motivo que
        // `i64::MIN / -1` en Div, §3.162). `checked_neg()` en vez de `-n`
        // crudo, mismo criterio que el resto de esta ronda.
        UnaryOp::Neg => match v {
            Value::Int(n) => n.checked_neg().map(Value::Int).ok_or_else(|| err(format!("desborde aritmético al negar {n}"))),
            Value::Int64(n) => n.checked_neg().map(Value::Int64).ok_or_else(|| err(format!("desborde aritmético al negar {n}"))),
            Value::Decimal(n) => {
                n.checked_neg().map(Value::Decimal).ok_or_else(|| err(format!("desborde aritmético al negar {}", format_decimal(n))))
            }
            Value::Float(n) => Ok(Value::Float(-n)),
            other => Err(err(format!("'-' unario requiere Int, Int64, Decimal o Float en runtime: {other:?}"))),
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

/// `db.call(&coll, "all", ...)` ya devuelve `Value::List`, siempre -- este
/// helper solo evita repetir el `match` de desempaquetado en cada uno de los
/// caminos de fallback de `findWhere`/`countWhere`/`deleteWhere` que caen al
/// camino interpretado de siempre.
fn all_items(db: &Db, coll: &str) -> Result<Vec<Value>, RuntimeError> {
    match db.call(coll, "all", vec![])? {
        Value::List(items) => Ok(items),
        _ => Ok(vec![]),
    }
}

/// Árbol de un predicado pusheable, ya EVALUADO (`Value`, no la expresión
/// cruda) -- espejo en runtime de `ast::PredicateExpr`, GRAMMAR.md §3.170.
/// `db.rs` genera SQL directo desde esta forma (`Db::condition_expr_sql`),
/// con `And`/`Or` traduciendo a `AND`/`OR` reales, parentizados solo donde
/// hace falta para preservar la precedencia.
pub(crate) enum ConditionExpr {
    Leaf(String, BinaryOp, Value),
    /// `item.campoA OP item.campoB` (GRAMMAR.md §3.171) -- a diferencia de
    /// `Leaf`, el lado derecho no es un `Value` para bindear como parámetro
    /// sino OTRA columna de la misma fila: `db.rs` genera `"campoA" OP
    /// "campoB"` directo, sin placeholder. Solo los cuatro operadores
    /// relacionales llegan hasta acá -- ver `ast::recognize_predicate_tree`.
    FieldPair(String, BinaryOp, String),
    And(Vec<ConditionExpr>),
    Or(Vec<ConditionExpr>),
}

/// Reconoce el `matchFn`/predicado de `findWhere`/`countWhere`/`deleteWhere`/
/// `upsert` y, si tiene la forma pusheable (`ast::recognize_predicate_expr`),
/// evalúa cada hoja contra el `Env` capturado por el closure -- `None` si
/// CUALQUIER hoja no es lo bastante simple (un `Ident` que no resuelve ahí,
/// una expresión que no es literal/`Ident`), igual que la versión anterior
/// de un solo nivel.
fn recognize_pushable_predicate(f: &Value) -> Option<ConditionExpr> {
    let Value::Closure(params, body, captured_env) = f else { return None };
    let tree = crate::ast::recognize_predicate_expr(params, body)?;
    evaluate_predicate_tree(tree, captured_env)
}

fn evaluate_predicate_tree(tree: crate::ast::PredicateExpr, captured_env: &Env) -> Option<ConditionExpr> {
    match tree {
        crate::ast::PredicateExpr::Leaf(field, op, operand) => match operand {
            crate::ast::PredicateOperand::Field(other_field) => {
                Some(ConditionExpr::FieldPair(field.to_string(), op, other_field.to_string()))
            }
            crate::ast::PredicateOperand::Bool(b) => Some(ConditionExpr::Leaf(field.to_string(), op, Value::Bool(b))),
            crate::ast::PredicateOperand::Expr(value_expr) => {
                let value = match &value_expr.node {
                    Expr::Int(n) => Value::Int(*n),
                    Expr::Float(x) => Value::Float(*x),
                    Expr::Str(s) => Value::Str(s.clone()),
                    Expr::Bool(b) => Value::Bool(*b),
                    Expr::Ident(name) => captured_env.get(name.as_str())?.borrow().clone(),
                    _ => return None,
                };
                Some(ConditionExpr::Leaf(field.to_string(), op, value))
            }
        },
        crate::ast::PredicateExpr::And(items) => {
            Some(ConditionExpr::And(items.into_iter().map(|i| evaluate_predicate_tree(i, captured_env)).collect::<Option<Vec<_>>>()?))
        }
        crate::ast::PredicateExpr::Or(items) => {
            Some(ConditionExpr::Or(items.into_iter().map(|i| evaluate_predicate_tree(i, captured_env)).collect::<Option<Vec<_>>>()?))
        }
    }
}

/// Cubre `+`/`-`/`*`/`/`/`%` -- todo operador aritmético entero de c-script
/// pasa por acá, ninguno queda con aritmética cruda sin `checked_*`
/// (originalmente solo `/`/`%`, GRAMMAR.md §3.162; extendida a `+`/`-`/`*`
/// en AUDIT-2026-08-27.md #16 -- mismo riesgo real: `a + b` crudo sobre
/// `i64` panica en desborde bajo `overflow-checks`, el perfil `dev` que
/// corre `cargo test`/CI, y en `release` wrappea en silencio, un bug de
/// CORRECCIÓN en vez de estabilidad, pero igual de real con datos de
/// usuario). `int_op` es la variante `checked_*` de `i64` correspondiente,
/// que devuelve `None` en cualquier desborde -- se traduce a un
/// `RuntimeError` normal (500 limpio, el hilo de la request sobrevive) en
/// vez de a un panic o un wrap silencioso. `float_op` va sin guarda a
/// propósito: IEEE-754 ya define overflow/`/0` como infinito/NaN, nunca
/// panica. `bad` arma el mensaje -- cada operador tiene su propia forma
/// natural (`/`/`%` distinguen divisor cero de desborde real; `+`/`-`/`*`
/// solo pueden desbordar, nunca hay un caso "por cero" que mencionar).
fn checked_int_numeric_op(
    l: Value,
    r: Value,
    int_op: impl Fn(i64, i64) -> Option<i64>,
    float_op: impl Fn(f64, f64) -> f64,
    bad: impl Fn(i64, i64) -> RuntimeError,
) -> Result<Value, RuntimeError> {
    match (l, r) {
        (Value::Int(a), Value::Int(b)) => int_op(a, b).map(Value::Int).ok_or_else(|| bad(a, b)),
        (Value::Int64(a), Value::Int64(b)) => int_op(a, b).map(Value::Int64).ok_or_else(|| bad(a, b)),
        (Value::Float(a), Value::Float(b)) => Ok(Value::Float(float_op(a, b))),
        (l, r) => Err(err(format!(
            "operador aritmético requiere Int+Int, Int64+Int64 o Float+Float en runtime: {l:?} y {r:?}"
        ))),
    }
}

/// GRAMMAR.md §3.196: `Timestamp.addSeconds`/`addMinutes`/`addHours`/`addDays`
/// -- convierte `n` a milisegundos y lo suma al valor ya envuelto, con
/// `checked_*` en las DOS operaciones (la multiplicación por la escala del
/// unit Y la suma final pueden desbordar por separado con un `n`
/// adversarial) -- mismo criterio obligatorio que el resto del runtime desde
/// AUDIT-2026-08-27.md #16, nunca aritmética cruda.
fn checked_timestamp_offset(ms: i64, n: i64, unit_millis: i64, verb: &str) -> Result<Value, RuntimeError> {
    n.checked_mul(unit_millis)
        .and_then(|delta| ms.checked_add(delta))
        .map(Value::Timestamp)
        .ok_or_else(|| err(format!("desborde aritmético al sumar {verb} a un Timestamp")))
}

/// GRAMMAR.md §3.198: `String.padStart`/`padEnd` -- rellena `s` con `pad`
/// (repetido y truncado según haga falta) hasta `target_length` CARACTERES,
/// al principio o al final según `at_start`. Ya cumple o se pasa: se
/// devuelve tal cual, sin truncar (mismo criterio "nunca se acorta un valor
/// que el caller no pidió acortar" implícito en el resto del lenguaje).
/// `target_length` acotado a un tope generoso pero real -- sin esto, un
/// `length` adversarial (`i64::MAX`) intentaría asignar un string gigante
/// en vez de fallar limpio, el mismo tipo de incidente que ya se encontró y
/// cerró para `crypto.randomToken` (AUDIT-2026-08-27.md).
fn pad_to_length(method_name: &str, s: &str, target_length: i64, pad: &str, at_start: bool) -> Result<Value, RuntimeError> {
    const MAX_PAD_LENGTH: i64 = 1_000_000;
    if target_length < 0 {
        return Err(err(format!("'{method_name}': 'length' no puede ser negativo, se recibió {target_length}")));
    }
    if target_length > MAX_PAD_LENGTH {
        return Err(err(format!("'{method_name}': 'length' no puede superar {MAX_PAD_LENGTH}, se recibió {target_length}")));
    }
    let current_len = s.chars().count() as i64;
    if current_len >= target_length {
        return Ok(Value::Str(s.to_string()));
    }
    let needed = (target_length - current_len) as usize;
    if pad.is_empty() {
        return Err(err(format!("'{method_name}': 'pad' no puede ser un string vacío -- hace falta rellenar hasta length={target_length}")));
    }
    let pad_chars: Vec<char> = pad.chars().collect();
    let fill: String = (0..needed).map(|i| pad_chars[i % pad_chars.len()]).collect();
    Ok(Value::Str(if at_start { format!("{fill}{s}") } else { format!("{s}{fill}") }))
}

/// Mensaje para `/`/`%`: distingue divisor cero (el caso casi siempre
/// alcanzado con datos de usuario) del desborde real (`i64::MIN / -1`).
fn div_or_rem_overflow_message(verb: &str, a: i64, b: i64) -> RuntimeError {
    if b == 0 {
        err(format!("no se puede {verb} {a} por cero"))
    } else {
        err(format!("desborde aritmético al {verb} {a} por {b}"))
    }
}

/// GRAMMAR.md §3.232: quita los campos `@hidden` del JSON que sale del
/// proceso, guiado por el TIPO declarado (un `Value::Struct` no lleva el
/// nombre de su type). Único punto para rpc, stream y MCP (todos pasan por
/// `invoke_rpc_with_sessions`) y para las filas en vivo
/// (`Db::deliver_local`/`subscribe`). Un array bajo un tipo que no es lista
/// se recorre elemento a elemento: un `stream` declara `T` y devuelve
/// `T[]` (§3.16). Una unión elige el primer miembro struct cuyos campos
/// requeridos visibles están todos presentes; un enum con datos no se
/// recorre (límite documentado en §3.232).
pub(crate) fn strip_hidden_json(json: serde_json::Value, ty: &crate::types::Type, checker: &Checker) -> serde_json::Value {
    use crate::types::Type;
    use serde_json::Value as J;
    match (json, ty) {
        (J::Null, _) => J::Null,
        (j, Type::Optional(inner)) => strip_hidden_json(j, inner, checker),
        (J::Array(items), Type::List(inner)) => J::Array(items.into_iter().map(|i| strip_hidden_json(i, inner, checker)).collect()),
        (J::Array(items), Type::Tuple(parts)) => J::Array(
            items
                .into_iter()
                .zip(parts.iter().chain(std::iter::repeat(&Type::Dynamic)))
                .map(|(i, t)| strip_hidden_json(i, t, checker))
                .collect(),
        ),
        (J::Array(items), ty) => J::Array(items.into_iter().map(|i| strip_hidden_json(i, ty, checker)).collect()),
        (J::Object(map), Type::Struct { name, fields }) => strip_hidden_struct(map, name.as_deref(), fields, checker),
        (J::Object(map), Type::Generic(name, args)) => match checker.expand_generic_struct(name, args) {
            Ok(fields) => strip_hidden_struct(map, Some(name), &fields, checker),
            Err(_) => J::Object(map),
        },
        (J::Object(map), Type::MapOf(_, v)) => {
            J::Object(map.into_iter().map(|(k, j)| (k, strip_hidden_json(j, v, checker))).collect())
        }
        (J::Object(map), Type::Union(members)) => {
            let pick = members.iter().find(|m| match m {
                Type::Struct { name, fields } => fields
                    .iter()
                    .filter(|f| !f.optional && !field_is_hidden(checker, name.as_deref(), &f.name))
                    .all(|f| map.contains_key(&f.name)),
                _ => false,
            });
            match pick {
                Some(m) => strip_hidden_json(J::Object(map), m, checker),
                None => J::Object(map),
            }
        }
        (j, _) => j,
    }
}

fn field_is_hidden(checker: &Checker, type_name: Option<&str>, field: &str) -> bool {
    type_name.and_then(|n| checker.hidden_fields.get(n)).is_some_and(|set| set.contains(field))
}

fn strip_hidden_struct(
    mut map: serde_json::Map<String, serde_json::Value>,
    name: Option<&str>,
    fields: &[crate::types::FieldType],
    checker: &Checker,
) -> serde_json::Value {
    if let Some(set) = name.and_then(|n| checker.hidden_fields.get(n)) {
        for hidden in set {
            map.remove(hidden);
        }
    }
    // `get_mut` + reemplazo, no remove+insert: con `preserve_order` eso
    // movería la clave al final y cambiaría el orden del JSON.
    for f in fields {
        if let Some(slot) = map.get_mut(&f.name) {
            let taken = std::mem::take(slot);
            *slot = strip_hidden_json(taken, &f.ty, checker);
        }
    }
    serde_json::Value::Object(map)
}

/// GRAMMAR.md §3.236: un `AiToken { token, done }` como `Value`.
pub(crate) fn ai_token_value(token: &str, done: bool) -> Value {
    Value::Struct(vec![("token".to_string(), Value::Str(token.to_string())), ("done".to_string(), Value::Bool(done))])
}

/// GRAMMAR.md §3.236: lo que un `stream` cuyo cuerpo es exactamente
/// `ai.stream(model, messages, maxTokens)` le pide al motor, con los
/// argumentos ya EVALUADOS contra los parámetros de la request.
#[cfg(feature = "inference")]
pub struct AiStreamSpec {
    pub alias: String,
    pub request: crate::inference::AiRequest,
    pub max_tokens: i64,
}

/// GRAMMAR.md §3.236: ¿el cuerpo de este `stream` es `ai.stream(...)`?
pub fn ai_stream_member(program: &Program, service_name: &str, rpc_name: &str) -> bool {
    program.items.iter().any(|i| match i {
        Item::Service(s) if s.name == service_name => s.members.iter().any(|m| match m {
            Member::Stream(r) if r.name == rpc_name => crate::ast::recognize_ai_stream(&r.body).is_some(),
            _ => false,
        }),
        _ => false,
    })
}

/// GRAMMAR.md §3.236: bindea los parámetros del `stream` como
/// `invoke_rpc_with_sessions` y evalúa los tres argumentos de
/// `ai.stream(...)` -- sin generar nada: el servidor es el que corre el
/// motor, escribiendo cada token por SSE.
#[cfg(feature = "inference")]
pub fn eval_ai_stream_request(
    program: &Program,
    service_name: &str,
    rpc_name: &str,
    args_json: &serde_json::Value,
    db: &Db,
    sessions: &SessionStore,
    current_token: Option<&str>,
) -> Result<AiStreamSpec, RuntimeError> {
    let rpc = program
        .items
        .iter()
        .find_map(|i| match i {
            Item::Service(s) if s.name == service_name => s.members.iter().find_map(|m| match m {
                Member::Stream(r) if r.name == rpc_name => Some(r),
                _ => None,
            }),
            _ => None,
        })
        .ok_or_else(|| err(format!("stream desconocido: '{service_name}.{rpc_name}'")))?;
    let arg_exprs = crate::ast::recognize_ai_stream(&rpc.body).ok_or_else(|| err("el cuerpo del stream no es ai.stream(...)"))?;
    let fns: Fns = program
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Fn(f) => Some((f.name.clone(), f)),
            _ => None,
        })
        .collect();
    let (checker, symbol_errors) = crate::checker::Checker::build_symbols(program);
    if let Some(e) = symbol_errors.into_iter().next() {
        return Err(err(format!("programa inválido: {e}")));
    }
    let empty = serde_json::Map::new();
    let args_obj = args_json.as_object().unwrap_or(&empty);
    let step_budget = Cell::new(0u64);
    let mut env = Env::new();
    for p in &rpc.params {
        let declared = checker
            .resolve_type(&p.ty)
            .map_err(|e| err(format!("no se pudo resolver el tipo del parámetro '{}': {e}", p.name)))?;
        let v = match args_obj.get(&p.name) {
            Some(j) => json_to_typed_value(j, &declared, &checker, &p.name)?,
            None => match &p.default {
                Some(default_expr) => eval_expr(default_expr, &Env::new(), db, &fns, &checker, sessions, current_token, &step_budget)?,
                None if matches!(declared, crate::types::Type::Optional(_)) => Value::Null,
                None => return Err(bad_req(format!("falta el parámetro requerido '{}' (se esperaba {})", p.name, describe_type(&declared)))),
            },
        };
        env.insert(p.name.clone(), cell(v));
    }
    let mut values = Vec::with_capacity(3);
    for e in arg_exprs {
        values.push(eval_expr(e, &env, db, &fns, &checker, sessions, current_token, &step_budget)?);
    }
    let mut it = values.into_iter();
    let alias = match it.next() {
        Some(Value::Str(s)) => s,
        _ => return Err(err("ai.stream: el modelo tiene que ser un String")),
    };
    let Some(Value::List(items)) = it.next() else {
        return Err(err("ai.stream: messages tiene que ser AiMessage[]"));
    };
    let max_tokens = as_int(&it.next().ok_or_else(|| err("ai.stream requiere 3 argumentos"))?)?;
    let request = crate::inference::AiRequest::Chat(items.iter().map(ai_message_from_value).collect::<Result<Vec<_>, _>>()?);
    Ok(AiStreamSpec { alias, request, max_tokens })
}

/// GRAMMAR.md §3.235: un `AiMessage` (o cualquier struct con `role` y
/// `content`, subtipado estructural) -> el `ChatMessage` del motor.
#[cfg(feature = "inference")]
fn ai_message_from_value(v: &Value) -> Result<crate::inference::ChatMessage, RuntimeError> {
    let Value::Struct(fields) = v else {
        return Err(err("ai.chat: cada mensaje tiene que ser un AiMessage { role, content }"));
    };
    let get = |name: &str| -> Result<String, RuntimeError> {
        match fields.iter().find(|(n, _)| n == name).map(|(_, v)| v) {
            Some(Value::Str(s)) => Ok(s.clone()),
            _ => Err(err(format!("ai.chat: el campo '{name}' de AiMessage tiene que ser un String"))),
        }
    };
    Ok(crate::inference::ChatMessage { role: get("role")?, content: get("content")? })
}

/// GRAMMAR.md §3.230: la consulta ordenada que `db.<c>.orderBy(...)`
/// devuelve -- ver `Value::DbQuery`.
#[derive(Debug, Clone, PartialEq)]
pub struct DbQuery {
    pub collection: String,
    pub order: Vec<db::OrderKey>,
}

/// GRAMMAR.md §3.230: la clave de `orderBy`/`orderByDesc` a partir del
/// closure selector -- misma forma reconocida que `maxRow`
/// (`closure_field_name`); el checker ya garantizó la forma, esto solo la
/// vuelve a leer del `Value::Closure` real.
fn order_key(method: &str, args: &[Value]) -> Result<db::OrderKey, RuntimeError> {
    let field = db::closure_field_name(args.first(), "de orden")?;
    Ok(db::OrderKey { field, desc: method == "orderByDesc" })
}

/// GRAMMAR.md §3.230: orden total para `List<T>.sortBy`/`sortByDesc` --
/// el espejo EN MEMORIA de `ORDER BY ... NULLS LAST`: `null` va siempre al
/// final, en las dos direcciones (`desc` invierte solo la comparación entre
/// dos valores presentes). Mismos tipos que `checker.rs::is_orderable_key`
/// -- si un lado aprende un tipo, el otro tiene que aprenderlo el mismo día.
fn order_cmp(a: &Value, b: &Value, desc: bool) -> Result<std::cmp::Ordering, RuntimeError> {
    use std::cmp::Ordering;
    let ordering = match (a, b) {
        (Value::Null, Value::Null) => return Ok(Ordering::Equal),
        (Value::Null, _) => return Ok(Ordering::Greater),
        (_, Value::Null) => return Ok(Ordering::Less),
        (Value::Int(x), Value::Int(y)) => x.cmp(y),
        (Value::Int64(x), Value::Int64(y)) => x.cmp(y),
        (Value::Decimal(x), Value::Decimal(y)) => x.cmp(y),
        (Value::Timestamp(x), Value::Timestamp(y)) => x.cmp(y),
        (Value::Float(x), Value::Float(y)) => x.partial_cmp(y).ok_or_else(|| err("sortBy: clave de orden NaN"))?,
        (Value::Str(x), Value::Str(y)) => x.cmp(y),
        (Value::Uuid(x), Value::Uuid(y)) => x.cmp(y),
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        _ => return Err(err(format!("sortBy: claves de orden de tipos distintos o sin orden total: {a:?} y {b:?}"))),
    };
    Ok(if desc { ordering.reverse() } else { ordering })
}

fn compare(l: Value, r: Value, accept: impl Fn(std::cmp::Ordering) -> bool) -> Result<Value, RuntimeError> {
    let ordering = match (&l, &r) {
        (Value::Int(a), Value::Int(b)) => a.cmp(b),
        (Value::Int64(a), Value::Int64(b)) => a.cmp(b),
        (Value::Decimal(a), Value::Decimal(b)) => a.cmp(b),
        (Value::Timestamp(a), Value::Timestamp(b)) => a.cmp(b),
        (Value::Float(a), Value::Float(b)) => {
            a.partial_cmp(b).ok_or_else(|| err("comparación con NaN"))?
        }
        _ => return Err(err(format!("operador relacional requiere Int+Int, Int64+Int64, Decimal+Decimal, Float+Float o Timestamp+Timestamp: {l:?} y {r:?}"))),
    };
    Ok(Value::Bool(accept(ordering)))
}

/// GRAMMAR.md §3.184: 4 decimales fijos, global -- `Value::Decimal(raw)`
/// representa el valor lógico `raw as f64 / DECIMAL_SCALE as f64`, siempre
/// exacto (aritmética entera, nunca de punto flotante en el camino normal).
pub(crate) const DECIMAL_SCALE: i128 = 10_000;

/// `raw` escalado -> string con EXACTAMENTE 4 decimales (ej. `"1234.5600"`,
/// nunca `"1234.56"`) -- formateado a mano desde el i128, sin tocar `f64` en
/// ningún punto. Usado tanto para el wire (`value_to_json`) como para
/// mensajes de error legibles (nunca se muestra el i128 crudo a un humano).
pub(crate) fn format_decimal(raw: i128) -> String {
    let negative = raw < 0;
    let abs = raw.unsigned_abs();
    let int_part = abs / (DECIMAL_SCALE as u128);
    let frac_part = abs % (DECIMAL_SCALE as u128);
    format!("{}{int_part}.{frac_part:04}", if negative { "-" } else { "" })
}

/// Inversa de `format_decimal` -- exige la forma EXACTA (signo opcional,
/// uno o más dígitos, punto, EXACTAMENTE 4 decimales); cualquier otra forma
/// (`"19.9"`, `"19.99900"`, notación científica) es `None`, nunca una
/// reinterpretación laxa -- mismo criterio de "wire sin ambigüedad" que el
/// resto del formato.
pub(crate) fn parse_decimal(s: &str) -> Option<i128> {
    let (negative, s) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let (int_part, frac_part) = s.split_once('.')?;
    if int_part.is_empty() || !int_part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if frac_part.len() != 4 || !frac_part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let int_val: i128 = int_part.parse().ok()?;
    let frac_val: i128 = frac_part.parse().ok()?;
    let raw = int_val.checked_mul(DECIMAL_SCALE)?.checked_add(frac_val)?;
    Some(if negative { -raw } else { raw })
}

/// División entera con redondeo al más cercano, EMPATE SE ALEJA DE CERO
/// (GRAMMAR.md §3.184 -- mismo criterio que la mayoría del software
/// financiero/comercial: `-2.5` redondea a `-3`, no a `-2`). General sobre
/// el signo de `denominator` (necesario para `/`, donde el divisor es un
/// valor de usuario que puede ser negativo -- no solo para el re-escalado
/// de `*`, donde el denominador siempre es `DECIMAL_SCALE`, positivo).
/// `None` en cualquier desborde/división por cero (vía los `checked_*`).
pub(crate) fn div_round(numerator: i128, denominator: i128) -> Option<i128> {
    let q = numerator.checked_div(denominator)?;
    let r = numerator.checked_rem(denominator)?;
    if r == 0 {
        return Some(q);
    }
    let double_r_abs = r.checked_abs()?.checked_mul(2)?;
    let denom_abs = denominator.checked_abs()?;
    if double_r_abs < denom_abs {
        return Some(q);
    }
    // Empate o más -- el signo del cociente REAL (antes de truncar) es
    // positivo cuando numerator/denominator tienen el mismo signo.
    let same_sign = (numerator < 0) == (denominator < 0);
    if same_sign { q.checked_add(1) } else { q.checked_sub(1) }
}

fn decimal_add(a: i128, b: i128) -> Result<Value, RuntimeError> {
    a.checked_add(b).map(Value::Decimal).ok_or_else(|| {
        err(format!("desborde aritmético al sumar {} y {}", format_decimal(a), format_decimal(b)))
    })
}

fn decimal_sub(a: i128, b: i128) -> Result<Value, RuntimeError> {
    a.checked_sub(b).map(Value::Decimal).ok_or_else(|| {
        err(format!("desborde aritmético al restar {} de {}", format_decimal(b), format_decimal(a)))
    })
}

/// `(a × b) / DECIMAL_SCALE`, redondeado -- el producto crudo de dos
/// valores ya escalados ×10.000 tiene 8 decimales lógicos (`10.000²`), hay
/// que volver a escalar a 4. Un solo redondeo, no iterativo.
fn decimal_mul(a: i128, b: i128) -> Result<Value, RuntimeError> {
    let bad = || err(format!("desborde aritmético al multiplicar {} por {}", format_decimal(a), format_decimal(b)));
    let raw = a.checked_mul(b).ok_or_else(bad)?;
    div_round(raw, DECIMAL_SCALE).map(Value::Decimal).ok_or_else(bad)
}

/// `(a × DECIMAL_SCALE) / b`, redondeado -- reescala el numerador ANTES de
/// dividir (en vez de dividir crudo y perder los 4 decimales de precisión
/// del resultado). Mismo redondeo que `decimal_mul`, un solo paso.
fn decimal_div(a: i128, b: i128) -> Result<Value, RuntimeError> {
    if b == 0 {
        return Err(err(format!("no se puede dividir {} por cero", format_decimal(a))));
    }
    let bad = || err(format!("desborde aritmético al dividir {} por {}", format_decimal(a), format_decimal(b)));
    let scaled_numerator = a.checked_mul(DECIMAL_SCALE).ok_or_else(bad)?;
    div_round(scaled_numerator, b).map(Value::Decimal).ok_or_else(bad)
}

/// `Int.toDecimal()` -- exacto, nunca lossy (i128 tiene rango de sobra
/// sobre i64×10.000).
fn decimal_from_int(n: i64) -> Result<Value, RuntimeError> {
    (n as i128)
        .checked_mul(DECIMAL_SCALE)
        .map(Value::Decimal)
        .ok_or_else(|| err(format!("{n} no entra en el rango de Decimal al escalar a 4 decimales")))
}

/// `Float.toDecimal()` -- redondea el f64 YA PARSEADO al 4to decimal
/// (mismo criterio de redondeo que el resto de Decimal: `f64::round()` ya
/// redondea empate-se-aleja-de-cero). Seguro en la práctica para cualquier
/// magnitud financiera real -- la precisión de f64 (~15-17 dígitos
/// significativos) excede por muchísimo la resolución de 4 decimales;
/// límite honesto documentado en GRAMMAR.md §3.184 para el caso patológico.
pub(crate) fn decimal_from_float(f: f64) -> Result<Value, RuntimeError> {
    if !f.is_finite() {
        return Err(err(format!("no se puede convertir {f} a Decimal -- no es un número finito")));
    }
    let scaled = (f * DECIMAL_SCALE as f64).round();
    if scaled < i128::MIN as f64 || scaled > i128::MAX as f64 {
        return Err(err(format!("{f} no entra en el rango de Decimal al escalar a 4 decimales")));
    }
    Ok(Value::Decimal(scaled as i128))
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
        Type::Decimal => matches!(v, Value::Decimal(_)),
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
            if name == "dateFromParts" && !fns.contains_key("dateFromParts") {
                return call_date_from_parts(arg_vs);
            }
            if name == "sitemapXml" && !fns.contains_key("sitemapXml") {
                return call_sitemap_xml(arg_vs);
            }
            if name == "robotsTxt" && !fns.contains_key("robotsTxt") {
                return call_robots_txt(arg_vs);
            }
            if name == "metaTags" && !fns.contains_key("metaTags") {
                return call_meta_tags(arg_vs);
            }
            if name == "openGraphTags" && !fns.contains_key("openGraphTags") {
                return call_open_graph_tags(arg_vs);
            }
            if name == "canonicalLink" && !fns.contains_key("canonicalLink") {
                return call_canonical_link(arg_vs);
            }
            if name == "jsonLd" && !fns.contains_key("jsonLd") {
                return call_json_ld(arg_vs);
            }
            if name == "staticRoutes" && !fns.contains_key("staticRoutes") {
                return call_static_routes(arg_vs, db);
            }
            if name == "hreflangLinks" && !fns.contains_key("hreflangLinks") {
                return call_hreflang_links(arg_vs);
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
/// GRAMMAR.md §3.223: envuelve UNA llamada `ureq` saliente para registrar
/// host + clase de status + duración en `Db` (lo que `/metrics` expone
/// como `linkc_http_outbound_*`). Devuelve el resultado INTACTO -- cada
/// arm de `http.*` sigue decidiendo qué hacer con un 4xx/5xx exactamente
/// como antes (`get` lo trata como error, `getWithStatus` como dato). Un
/// `ureq::Error::Status` cuenta por su código real; cualquier otro error
/// (DNS, conexión rechazada, timeout) cuenta como `error`.
///
/// `started` se toma en el call site (`Instant::now()` como argumento, que
/// Rust evalúa antes que el `req.call()` que le sigue, de izquierda a
/// derecha) en vez de recibir un closure: `ureq::Error` pesa ~272 bytes y
/// clippy (`result_large_err`) rechaza un closure que lo devuelva. El
/// `Result` es el de `ureq`, no nuestro, y boxearlo obligaría a reescribir
/// los 7 arms que hacen match sobre `ureq::Error::Status` -- de ahí el
/// `allow` acotado a esta única función.
#[allow(clippy::result_large_err)]
pub(crate) fn outbound_http(db: &Db, url: &str, started: std::time::Instant, result: Result<ureq::Response, ureq::Error>) -> Result<ureq::Response, ureq::Error> {
    let status = match &result {
        Ok(resp) => outbound_status_class(resp.status()),
        Err(ureq::Error::Status(code, _)) => outbound_status_class(*code),
        Err(_) => "error",
    };
    db.record_outbound_http(&outbound_host(url), status, started.elapsed());
    result
}

/// `2xx`/`3xx`/`4xx`/`5xx` (o `other` para un código fuera de 200-599).
fn outbound_status_class(code: u16) -> &'static str {
    match code {
        200..=299 => "2xx",
        300..=399 => "3xx",
        400..=499 => "4xx",
        500..=599 => "5xx",
        _ => "other",
    }
}

/// La autoridad de una URL (`host` o `host:puerto`, sin credenciales, sin
/// path ni query) -- la etiqueta `host` de `/metrics`. Sin crate de URLs
/// (mismo criterio que `crypto.awsS3PresignedUrl`): el corte es textual y
/// una URL malformada da su texto hasta la primera `/`, nunca un error --
/// registrar la métrica jamás puede hacer fallar la llamada real.
fn outbound_host(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    authority.to_ascii_lowercase()
}

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

/// Un adjunto ya validado, listo para `Attachment::new(...).body(bytes,
/// content_type)` -- `bytes` viene de DECODIFICAR el `contentBase64` del
/// struct, sin pasar por `base64.decode` (§3.43) porque ESE builtin exige
/// UTF-8 válido en el resultado (piensa en un `String` de c-script), algo
/// que un adjunto binario real (PDF, PNG, ...) casi nunca es -- acá se
/// decodifica directo a `Vec<u8>` con el MISMO engine/alfabeto
/// (`base64::engine::general_purpose::STANDARD`), sin esa restricción.
struct SmtpAttachment {
    filename: String,
    content_type: String,
    bytes: Vec<u8>,
}

fn smtp_attachments_from_value(items: &[Value]) -> Result<Vec<SmtpAttachment>, RuntimeError> {
    items
        .iter()
        .map(|item| {
            let Value::Struct(fields) = item else {
                return Err(err("smtp.sendMessage: cada adjunto tiene que ser un struct con 'filename'/'contentType'/'contentBase64'"));
            };
            let filename = match fields.iter().find(|(n, _)| n == "filename") {
                Some((_, Value::Str(s))) => s.clone(),
                _ => return Err(err("smtp.sendMessage: falta el campo 'filename' de un adjunto, o no es String")),
            };
            let content_type = match fields.iter().find(|(n, _)| n == "contentType") {
                Some((_, Value::Str(s))) => s.clone(),
                _ => return Err(err("smtp.sendMessage: falta el campo 'contentType' de un adjunto, o no es String")),
            };
            let content_base64 = match fields.iter().find(|(n, _)| n == "contentBase64") {
                Some((_, Value::Str(s))) => s,
                _ => return Err(err("smtp.sendMessage: falta el campo 'contentBase64' de un adjunto, o no es String")),
            };
            use base64::Engine;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(content_base64.as_bytes())
                .map_err(|e| err(format!("smtp.sendMessage: 'contentBase64' del adjunto '{filename}' no es base64 válido: {e}")))?;
            Ok(SmtpAttachment { filename, content_type, bytes })
        })
        .collect()
}

/// `smtp.sendMessage({ to, cc?, bcc?, subject, body, html?, attachments? })`
/// (GRAMMAR.md §3.141) -- variante "kitchen sink" que cubre lo que
/// `send`/`sendToMany`/`sendHtml` (arriba) no podían: copia oculta/visible y
/// adjuntos reales. Función APARTE de `send_email` en vez de generalizarla
/// -- las tres funciones simples cubren el caso común (texto o HTML a una
/// lista de destinatarios) sin la complejidad de MIME multipart; agregar
/// cc/bcc/attachments a esa función habría significado que el 99% de sus
/// llamadas (sin ninguna de las tres) paguen el costo de un `Vec` vacío por
/// parámetro nuevo para nada.
fn send_email_advanced(fields: &[(String, Value)]) -> Result<(), RuntimeError> {
    let to = match fields.iter().find(|(n, _)| n == "to") {
        Some((_, Value::List(items))) => strings_from_value_list("smtp.sendMessage", "to", items)?,
        _ => return Err(err("smtp.sendMessage: falta el campo 'to' o no es String[]")),
    };
    if to.is_empty() {
        return Err(err("smtp.sendMessage: 'to' no puede ser una lista vacía -- hace falta al menos un destinatario"));
    }
    let cc = match fields.iter().find(|(n, _)| n == "cc") {
        Some((_, Value::List(items))) => strings_from_value_list("smtp.sendMessage", "cc", items)?,
        _ => Vec::new(),
    };
    let bcc = match fields.iter().find(|(n, _)| n == "bcc") {
        Some((_, Value::List(items))) => strings_from_value_list("smtp.sendMessage", "bcc", items)?,
        _ => Vec::new(),
    };
    let subject = match fields.iter().find(|(n, _)| n == "subject") {
        Some((_, Value::Str(s))) => s.as_str(),
        _ => return Err(err("smtp.sendMessage: falta el campo 'subject' o no es String")),
    };
    let body = match fields.iter().find(|(n, _)| n == "body") {
        Some((_, Value::Str(s))) => s.as_str(),
        _ => return Err(err("smtp.sendMessage: falta el campo 'body' o no es String")),
    };
    let is_html = matches!(fields.iter().find(|(n, _)| n == "html"), Some((_, Value::Bool(true))));
    let attachments = match fields.iter().find(|(n, _)| n == "attachments") {
        Some((_, Value::List(items))) => smtp_attachments_from_value(items)?,
        _ => Vec::new(),
    };

    let url = std::env::var("LINK_SMTP_URL")
        .map_err(|_| err("smtp: falta la variable de entorno LINK_SMTP_URL (ej. 'smtps://usuario:clave@smtp.proveedor.com')"))?;
    let from = std::env::var("LINK_SMTP_FROM").map_err(|_| err("smtp: falta la variable de entorno LINK_SMTP_FROM (la dirección remitente)"))?;
    let from_mbox: lettre::message::Mailbox =
        from.parse().map_err(|e| err(format!("smtp: LINK_SMTP_FROM ('{from}') no es una dirección válida: {e}")))?;

    let mut builder = lettre::Message::builder().from(from_mbox).subject(subject);
    for addr in &to {
        let mbox: lettre::message::Mailbox =
            addr.parse().map_err(|e| err(format!("smtp.sendMessage: 'to' ('{addr}') no es una dirección válida: {e}")))?;
        builder = builder.to(mbox);
    }
    for addr in &cc {
        let mbox: lettre::message::Mailbox =
            addr.parse().map_err(|e| err(format!("smtp.sendMessage: 'cc' ('{addr}') no es una dirección válida: {e}")))?;
        builder = builder.cc(mbox);
    }
    for addr in &bcc {
        let mbox: lettre::message::Mailbox =
            addr.parse().map_err(|e| err(format!("smtp.sendMessage: 'bcc' ('{addr}') no es una dirección válida: {e}")))?;
        builder = builder.bcc(mbox);
    }

    use lettre::message::{header::ContentType, Attachment, MultiPart, SinglePart};
    let body_content_type = if is_html { ContentType::TEXT_HTML } else { ContentType::TEXT_PLAIN };
    let mut multipart = MultiPart::mixed().singlepart(SinglePart::builder().header(body_content_type).body(body.to_string()));
    for att in attachments {
        let content_type = ContentType::parse(&att.content_type)
            .map_err(|e| err(format!("smtp.sendMessage: 'contentType' ('{}') del adjunto '{}' no es un mime type válido: {e}", att.content_type, att.filename)))?;
        multipart = multipart.singlepart(Attachment::new(att.filename).body(att.bytes, content_type));
    }
    let email = builder.multipart(multipart).map_err(|e| err(format!("smtp.sendMessage: no se pudo armar el mensaje: {e}")))?;

    use lettre::Transport;
    let mailer = lettre::SmtpTransport::from_url(&url).map_err(|e| err(format!("smtp: LINK_SMTP_URL inválida: {e}")))?.build();
    mailer.send(&email).map_err(|e| err(format!("smtp: no se pudo mandar el email: {e}")))?;
    Ok(())
}

/// Extrae `Vec<String>` de una `Value::List` ya confirmada -- mismo patrón
/// que `sendToMany`/`sendHtml` ya usaban inline, factorizado acá porque
/// `send_email_advanced` lo necesita tres veces (`to`/`cc`/`bcc`).
fn strings_from_value_list(method: &str, field: &str, items: &[Value]) -> Result<Vec<String>, RuntimeError> {
    items
        .iter()
        .map(|v| match v {
            Value::Str(s) => Ok(s.clone()),
            other => Err(err(format!("{method}: '{field}' tiene que ser una lista de String, se encontró {other:?}"))),
        })
        .collect()
}

/// Backoff exponencial de `http.postWithRetry` (GRAMMAR.md §3.160) --
/// FIJO, no configurable, mismo espíritu que `MAX_WHILE_ITERATIONS`
/// (§3.15): un backstop razonable, no un sistema fino de política de
/// reintentos por llamada. `attempt` es 1-based (el intento 0 nunca
/// espera, es el primero); dobla cada fallo consecutivo, techo de 5s --
/// deliberadamente mucho más corto que `MAX_RESTART_BACKOFF` (30s,
/// main.rs), porque esto bloquea el hilo de UNA request (§3.158), no
/// reintenta un proceso servidor entero.
fn http_retry_backoff(attempt: i64) -> std::time::Duration {
    const BASE: std::time::Duration = std::time::Duration::from_millis(200);
    const CAP: std::time::Duration = std::time::Duration::from_secs(5);
    // `attempt` es 1-based: intento 1 espera BASE (200ms), intento 2 espera
    // 2*BASE, etc. -- el shift se acota a 8 (256x BASE, ya muy por encima
    // del CAP de 5s) para nunca desbordar el shift aunque `maxAttempts` sea
    // un número enorme.
    let shift = (attempt - 1).clamp(0, 8) as u32;
    BASE.saturating_mul(1u32 << shift).min(CAP)
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
pub(crate) fn os_random_bytes(n: usize) -> Result<Vec<u8>, RuntimeError> {
    let mut buf = vec![0u8; n];
    getrandom::getrandom(&mut buf)
        .map_err(|e| err(format!("el sistema no pudo generar bytes aleatorios: {e}")))?;
    Ok(buf)
}

/// UUIDv4 de verdad: 122 bits del CSPRNG del sistema (`os_random_bytes`).
/// Antes era SHA-256 del reloj disfrazado de v4 -- dos llamadas en el
/// mismo nanosegundo devolvían el mismo "identificador único". Extraído
/// de `crypto.uuid()` (GRAMMAR.md §3.70) para que `runtime/db.rs` lo
/// reuse también al generar la PK de una fila nueva en una colección con
/// `id: Uuid` (GRAMMAR.md §3.177) -- mismo generador, un solo lugar que
/// conoce el layout de bytes de un UUIDv4, nunca una segunda copia que
/// pueda desalinearse de esta.
pub(crate) fn generate_uuid_v4() -> Result<String, RuntimeError> {
    let b = os_random_bytes(16)?;
    Ok(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-4{:01x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3],
        b[4], b[5],
        b[6] & 0x0f, b[7],
        (b[8] & 0x3f) | 0x80, b[9],
        b[10], b[11], b[12], b[13], b[14], b[15]
    ))
}

/// `UriEncode()` exacto de AWS Signature V4 (GRAMMAR.md §3.110, spec en
/// docs.aws.amazon.com/general/latest/gr/sigv4-signed-request-examples.html):
/// cada BYTE (no cada carácter -- un caracter UTF-8 multibyte se codifica
/// byte por byte, que es lo correcto) se deja tal cual si es "sin reservar"
/// (`A-Za-z0-9-._~`), si no se codifica `%XX` con hex EN MAYÚSCULA. `/` es
/// el único caso especial: se codifica en un VALOR de query string
/// (`encode_slash: true`, ej. dentro de `X-Amz-Credential`) pero se
/// preserva tal cual en el componente de path de la URI (`encode_slash:
/// false`, para el nombre del objeto -- un "folder/archivo.pdf" real no
/// debe convertirse en un solo segmento codificado).
// Los dos primeros brazos del `if` hacen lo mismo (`push(c)`) por MOTIVOS
// distintos documentados arriba -- colapsarlos en una sola condición
// enterraría la distinción "sin reservar" vs "el caso especial de `/`" que
// la spec de AWS trata por separado. Falso positivo deliberadamente.
#[allow(clippy::if_same_then_else)]
fn aws_uri_encode(s: &str, encode_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        let c = b as char;
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~') {
            out.push(c);
        } else if c == '/' && !encode_slash {
            out.push(c);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// HMAC-SHA256 devolviendo los 32 bytes CRUDOS del digest, no su hex --
/// GRAMMAR.md §3.226: los tres prefijos de bcrypt que existen en la
/// práctica (`$2a$` original, `$2b$` el actual -- bcryptjs 3.x, OpenBSD --,
/// `$2y$` el de PHP `password_hash`/crypt_blowfish). `$2x$` (la variante
/// con el bug histórico de crypt_blowfish) queda afuera a propósito: no
/// hay ninguna app moderna emitiéndolo, y aceptarlo sería verificar contra
/// un algoritmo deliberadamente incorrecto.
fn is_bcrypt_hash(hash: &str) -> bool {
    hash.starts_with("$2a$") || hash.starts_with("$2b$") || hash.starts_with("$2y$")
}

/// `crypto.hmacSha256` (GRAMMAR.md §3.38) siempre devuelve `String`/// `crypto.hmacSha256` (GRAMMAR.md §3.38) siempre devuelve `String`
/// (hex), lo que alcanza para verificar la firma de un webhook pero NO
/// para encadenar HMACs usando el resultado de uno como clave del
/// siguiente (AWS Signature V4 necesita exactamente eso -- ver
/// `awsS3PresignedUrl` más abajo). Esta función es privada al runtime,
/// nunca expuesta directo a un programa c-script -- ahí es donde
/// mantener la distinción "bytes crudos vs. hex" importa; el lenguaje en
/// sí sigue sin un tipo de bytes crudos, a propósito (GRAMMAR.md §2, "sin
/// tipo Bytes -- todo lo binario entra/sale como String codificado").
fn hmac_sha256_raw(key: &[u8], data: &[u8]) -> Result<Vec<u8>, RuntimeError> {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|e| err(format!("clave HMAC inválida: {e}")))?;
    mac.update(data);
    Ok(mac.finalize().into_bytes().to_vec())
}

/// AWS Signature V4 para una URL prefirmada de S3, compartida entre
/// `crypto.awsS3PresignedUrl` (GET, GRAMMAR.md §3.110) y
/// `crypto.awsS3PresignedUploadUrl` (PUT, GRAMMAR.md §3.194) -- las dos
/// difieren solo en el método HTTP firmado y en si hay un `content_type`
/// que firmar como header adicional (`None` para la descarga, obligatorio
/// para la subida: la URL resultante solo acepta un PUT con ESE
/// Content-Type exacto, no cualquiera). Todo lo demás -- derivación de la
/// clave de firma, codificación de URI, orden alfabético de parámetros --
/// es el mismo mecanismo ya verificado byte a byte contra el vector
/// oficial de AWS en `hmac_sha256_raw_chain_reproduces_the_official_aws_sigv4_test_vector`.
#[allow(clippy::too_many_arguments)]
fn aws_sigv4_presigned_url(
    builtin_name: &str,
    method: &str,
    access_key_id: &str,
    secret_access_key: &str,
    region: &str,
    bucket: &str,
    object_key: &str,
    expires_seconds: i64,
    content_type: Option<&str>,
) -> Result<Value, RuntimeError> {
    if !(1..=604_800).contains(&expires_seconds) {
        return Err(err(format!(
            "{builtin_name}: 'expiresSeconds' tiene que estar entre 1 y 604800 (7 días, el máximo que AWS acepta con credenciales de larga duración), se recibió {expires_seconds}"
        )));
    }
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let (date_stamp, amz_date) = timestamp::format_aws_sigv4_datetime(now_ms);
    let host = format!("{bucket}.s3.{region}.amazonaws.com");
    let canonical_uri = format!("/{}", aws_uri_encode(object_key, false));
    let credential_scope = format!("{date_stamp}/{region}/s3/aws4_request");
    let credential = format!("{access_key_id}/{credential_scope}");
    // Los headers firmados van en orden ALFABÉTICO por nombre -- "content-type"
    // antes que "host" -- tanto en la lista de `canonical_headers` como en el
    // valor de `X-Amz-SignedHeaders`, mismo requisito de SigV4 que ya aplicaba
    // al único header que existía hasta ahora (`host`).
    let signed_headers = if content_type.is_some() { "content-type;host" } else { "host" };
    let canonical_query_string = format!(
        "X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential={}&X-Amz-Date={amz_date}&X-Amz-Expires={expires_seconds}&X-Amz-SignedHeaders={}",
        aws_uri_encode(&credential, true),
        aws_uri_encode(signed_headers, true),
    );
    let canonical_headers = match content_type {
        Some(ct) => format!("content-type:{ct}\nhost:{host}\n"),
        None => format!("host:{host}\n"),
    };
    let canonical_request = format!("{method}\n{canonical_uri}\n{canonical_query_string}\n{canonical_headers}\n{signed_headers}\nUNSIGNED-PAYLOAD");
    use sha2::{Digest, Sha256};
    let hashed_canonical_request: String =
        Sha256::digest(canonical_request.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    let string_to_sign = format!("AWS4-HMAC-SHA256\n{amz_date}\n{credential_scope}\n{hashed_canonical_request}");
    // Derivación de la clave de firma: 4 HMAC-SHA256 encadenados donde el
    // resultado CRUDO (bytes, no su hex) de cada paso es la clave del
    // siguiente -- GRAMMAR.md §3.110 explica por qué `crypto.hmacSha256`
    // (String -> String) no alcanza para esto: no hay forma de volver a
    // meter sus bytes crudos como clave.
    let k_date = hmac_sha256_raw(format!("AWS4{secret_access_key}").as_bytes(), date_stamp.as_bytes())?;
    let k_region = hmac_sha256_raw(&k_date, region.as_bytes())?;
    let k_service = hmac_sha256_raw(&k_region, b"s3")?;
    let k_signing = hmac_sha256_raw(&k_service, b"aws4_request")?;
    let signature: String =
        hmac_sha256_raw(&k_signing, string_to_sign.as_bytes())?.iter().map(|b| format!("{b:02x}")).collect();
    Ok(Value::Str(format!("https://{host}{canonical_uri}?{canonical_query_string}&X-Amz-Signature={signature}")))
}

/// Comparación que no corta en el primer byte distinto: dos secretos se comparan
/// en tiempo constante para no filtrar, vía la duración, cuánto del valor
/// esperado adivinó quien está probando.
pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    // La diferencia de LARGO no es secreta (el formato del hash es público); lo
    // que no debe filtrarse es en qué posición difieren dos del mismo largo.
    if a.len() != b.len() {
        return false;
    }
    a.ct_eq(b).into()
}

/// `dateFromParts(year, month, day, hour, minute, second) -> Timestamp`
/// (GRAMMAR.md §3.90) -- compartido entre los dos caminos de llamada
/// (`Expr::Call` directo y `call_callable` cuando se lo pasa como valor de
/// primera clase, ej. `now`), mismo criterio que el resto de los builtins
/// sin receptor. Una fecha inválida (mes 13, 30 de febrero, ...) es un
/// `bad_request` (400) -- error del CALLER, no un bug del servidor.
fn call_date_from_parts(arg_vs: Vec<Value>) -> Result<Value, RuntimeError> {
    let [year, month, day, hour, minute, second]: [Value; 6] = arg_vs
        .try_into()
        .map_err(|_| err("'dateFromParts' requiere 6 argumentos (year, month, day, hour, minute, second)"))?;
    let ms = timestamp::date_from_parts(as_int(&year)?, as_int(&month)?, as_int(&day)?, as_int(&hour)?, as_int(&minute)?, as_int(&second)?)
        .map_err(RuntimeError::bad_request)?;
    Ok(Value::Timestamp(ms))
}

/// `sitemapXml(urls: {loc: String, lastmod: Timestamp?}[]) -> String`
/// (GRAMMAR.md §3.116): arma un `sitemap.xml` bien formado (protocolo
/// sitemaps.org) -- el rpc sigue siendo responsable de la lista de URLs
/// (viene de la base, `@route` no puede inferir rutas dinámicas por sí
/// solo), esto solo arma el XML. Reusa `escape_html` para `<loc>` -- mismo
/// conjunto de caracteres (`&`, `<`, `>`, `"`, `'`) que XML también exige
/// escapar en contenido de texto, y sus referencias numéricas (`&#39;`
/// incluido) son válidas en XML tal cual, no solo en HTML.
fn call_sitemap_xml(arg_vs: Vec<Value>) -> Result<Value, RuntimeError> {
    let [urls]: [Value; 1] =
        arg_vs.try_into().map_err(|_| err("'sitemapXml' requiere 1 argumento (urls: {loc, lastmod?}[])"))?;
    let Value::List(items) = urls else {
        return Err(err("'sitemapXml' requiere una lista de {loc, lastmod?}"));
    };
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n");
    for item in items {
        let Value::Struct(fields) = item else {
            return Err(err("'sitemapXml': cada entrada tiene que ser un struct con 'loc'"));
        };
        let Some((_, Value::Str(loc))) = fields.iter().find(|(n, _)| n == "loc") else {
            return Err(err("'sitemapXml': falta el campo 'loc' o no es String"));
        };
        out.push_str("  <url>\n    <loc>");
        out.push_str(&escape_html(loc));
        out.push_str("</loc>\n");
        if let Some((_, Value::Timestamp(ms))) = fields.iter().find(|(n, _)| n == "lastmod") {
            out.push_str("    <lastmod>");
            out.push_str(&timestamp::format_iso8601_millis(*ms));
            out.push_str("</lastmod>\n");
        }
        out.push_str("  </url>\n");
    }
    out.push_str("</urlset>");
    Ok(Value::Str(out))
}

/// `robotsTxt(rules: {userAgent, disallow?, allow?}[], sitemapUrl: String?)
/// -> String` (GRAMMAR.md §3.116): arma un `robots.txt` bien formado, un
/// bloque `User-agent: ...` por regla con sus `Disallow`/`Allow` (en ese
/// orden, mismo orden que el estándar de facto), y `Sitemap: <url>` al
/// final si se pasó una. `disallow`/`allow` AUSENTES (campo entero
/// faltante en el struct, o presente pero `null`) se tratan exactamente
/// igual que una lista vacía -- ningún `Disallow`/`Allow` para ese bloque
/// -- por eso el único match que le importa a este código es
/// `Some((_, Value::List(paths)))`, cualquier otra forma simplemente no
/// agrega nada, nunca un error.
fn call_robots_txt(arg_vs: Vec<Value>) -> Result<Value, RuntimeError> {
    let [rules, sitemap_url]: [Value; 2] =
        arg_vs.try_into().map_err(|_| err("'robotsTxt' requiere 2 argumentos (rules: {...}[], sitemapUrl: String?)"))?;
    let Value::List(rule_items) = rules else {
        return Err(err("'robotsTxt' requiere una lista de reglas"));
    };
    let mut blocks = Vec::with_capacity(rule_items.len());
    for item in rule_items {
        let Value::Struct(fields) = item else {
            return Err(err("'robotsTxt': cada regla tiene que ser un struct"));
        };
        let Some((_, Value::Str(user_agent))) = fields.iter().find(|(n, _)| n == "userAgent") else {
            return Err(err("'robotsTxt': falta el campo 'userAgent' o no es String"));
        };
        let mut block = format!("User-agent: {user_agent}");
        for (field_name, directive) in [("disallow", "Disallow"), ("allow", "Allow")] {
            if let Some((_, Value::List(paths))) = fields.iter().find(|(n, _)| n == field_name) {
                for p in paths {
                    if let Value::Str(p) = p {
                        block.push('\n');
                        block.push_str(directive);
                        block.push_str(": ");
                        block.push_str(p);
                    }
                }
            }
        }
        blocks.push(block);
    }
    let mut out = blocks.join("\n\n");
    if let Value::Str(url) = sitemap_url {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str("Sitemap: ");
        out.push_str(&url);
    }
    Ok(Value::Str(out))
}

/// `metaTags(tags: {name: String, content: String}[]) -> String`
/// (GRAMMAR.md §3.117): una línea `<meta name="..." content="...">` por
/// entrada, separadas por `\n`, lista para pegar dentro de `<head>`. Meta
/// tags clásicos (`description`, `robots`, `viewport`, ...) usan el
/// atributo `name` -- Open Graph usa `property` en cambio, ver
/// `call_open_graph_tags`. `escape_html` sobre AMBOS atributos: `content`
/// suele venir de datos de usuario (título/descripción de un producto), y
/// también cubre comillas dobles dentro del valor.
fn call_meta_tags(arg_vs: Vec<Value>) -> Result<Value, RuntimeError> {
    let [tags]: [Value; 1] =
        arg_vs.try_into().map_err(|_| err("'metaTags' requiere 1 argumento (tags: {name, content}[])"))?;
    let Value::List(items) = tags else {
        return Err(err("'metaTags' requiere una lista de {name, content}"));
    };
    let mut lines = Vec::with_capacity(items.len());
    for item in items {
        let Value::Struct(fields) = item else {
            return Err(err("'metaTags': cada entrada tiene que ser un struct con 'name' y 'content'"));
        };
        let Some((_, Value::Str(name))) = fields.iter().find(|(n, _)| n == "name") else {
            return Err(err("'metaTags': falta el campo 'name' o no es String"));
        };
        let Some((_, Value::Str(content))) = fields.iter().find(|(n, _)| n == "content") else {
            return Err(err("'metaTags': falta el campo 'content' o no es String"));
        };
        lines.push(format!("<meta name=\"{}\" content=\"{}\">", escape_html(name), escape_html(content)));
    }
    Ok(Value::Str(lines.join("\n")))
}

/// `openGraphTags(tags: {property: String, content: String}[]) -> String`
/// (GRAMMAR.md §3.117): mismo mecanismo que `call_meta_tags`, pero con el
/// atributo `property` en vez de `name` -- así distingue Open Graph
/// (`og:title`, `og:image`, ...) del resto del HTML real.
fn call_open_graph_tags(arg_vs: Vec<Value>) -> Result<Value, RuntimeError> {
    let [tags]: [Value; 1] =
        arg_vs.try_into().map_err(|_| err("'openGraphTags' requiere 1 argumento (tags: {property, content}[])"))?;
    let Value::List(items) = tags else {
        return Err(err("'openGraphTags' requiere una lista de {property, content}"));
    };
    let mut lines = Vec::with_capacity(items.len());
    for item in items {
        let Value::Struct(fields) = item else {
            return Err(err("'openGraphTags': cada entrada tiene que ser un struct con 'property' y 'content'"));
        };
        let Some((_, Value::Str(property))) = fields.iter().find(|(n, _)| n == "property") else {
            return Err(err("'openGraphTags': falta el campo 'property' o no es String"));
        };
        let Some((_, Value::Str(content))) = fields.iter().find(|(n, _)| n == "content") else {
            return Err(err("'openGraphTags': falta el campo 'content' o no es String"));
        };
        lines.push(format!("<meta property=\"{}\" content=\"{}\">", escape_html(property), escape_html(content)));
    }
    Ok(Value::Str(lines.join("\n")))
}

/// `canonicalLink(url: String) -> String` (GRAMMAR.md §3.117): un
/// `<link rel="canonical" href="...">` bien formado -- consolidar
/// contenido duplicado (misma página accesible por más de una URL) es SEO
/// básico, mismo espíritu que `response.redirect` (§3.111) pero como
/// elemento de `<head>` en vez de un redirect real.
fn call_canonical_link(arg_vs: Vec<Value>) -> Result<Value, RuntimeError> {
    let [url]: [Value; 1] = arg_vs.try_into().map_err(|_| err("'canonicalLink' requiere 1 argumento (url: String)"))?;
    let Value::Str(url) = url else {
        return Err(err("'canonicalLink' requiere un argumento String"));
    };
    Ok(Value::Str(format!("<link rel=\"canonical\" href=\"{}\">", escape_html(&url))))
}

/// `jsonLd(data: Dynamic) -> String` (GRAMMAR.md §3.117): un bloque
/// `<script type="application/ld+json">...</script>` con `data`
/// serializado a JSON -- mismo serializador que `json.stringify`
/// (`value_to_json` + `serde_json::to_string`), acepta `Dynamic` porque un
/// dato JSON-LD real (schema.org) no tiene una forma fija que el checker
/// pueda exigir de antemano. Cada `<` del JSON serializado se reemplaza por
/// su escape unicode de 4 dígitos hex (u+003C) DESPUÉS de serializar -- mitigación estándar
/// (recomendada por OWASP) contra que un valor de usuario dentro del JSON
/// contenga literalmente `</script>` y cierre la etiqueta antes de tiempo;
/// un JSON válido nunca depende de un `<` literal fuera de un string (no es
/// un delimitador de la gramática JSON), así que el reemplazo no rompe el
/// parseo del lado del navegador.
/// `staticRoutes(baseUrl: String) -> {loc: String}[]` (GRAMMAR.md §3.222):
/// cada `@route` ESTÁTICO y PÚBLICO del programa (sin `:param`, sin
/// catch-all, sin `@authenticated`/`@requires`) como URL absoluta. La lista
/// la calcula `Db::new` una sola vez a partir del AST (`Db::static_routes`)
/// -- el runtime no tiene el `Program` a mano en este punto, pero `Db` sí lo
/// vio al construirse, mismo criterio que `soft_delete_fields`. `baseUrl`
/// sin barra final; una ruta siempre empieza con `/`, así que se concatena
/// tal cual (se tolera una barra final de más quitándola).
fn call_static_routes(arg_vs: Vec<Value>, db: &Db) -> Result<Value, RuntimeError> {
    let [base]: [Value; 1] = arg_vs.try_into().map_err(|_| err("'staticRoutes' requiere 1 argumento (baseUrl: String)"))?;
    let Value::Str(base) = base else {
        return Err(err("'staticRoutes' requiere un argumento String"));
    };
    let base = base.trim_end_matches('/');
    let items = db
        .static_routes()
        .iter()
        .map(|path| Value::Struct(vec![("loc".to_string(), Value::Str(format!("{base}{path}")))]))
        .collect();
    Ok(Value::List(items))
}

/// `hreflangLinks(alternates: {lang: String, href: String}[]) -> String`
/// (GRAMMAR.md §3.222): un `<link rel="alternate" hreflang="..." href="...">`
/// por variante de idioma, uno por línea -- lo que Google exige para que
/// cada versión de una página multi-idioma se indexe como la misma página
/// y no como contenido duplicado. Mismo escape que `canonicalLink`.
fn call_hreflang_links(arg_vs: Vec<Value>) -> Result<Value, RuntimeError> {
    let [alts]: [Value; 1] =
        arg_vs.try_into().map_err(|_| err("'hreflangLinks' requiere 1 argumento (alternates: {lang, href}[])"))?;
    let Value::List(items) = alts else {
        return Err(err("'hreflangLinks' requiere una lista de {lang, href}"));
    };
    let mut lines = Vec::with_capacity(items.len());
    for item in items {
        let Value::Struct(fields) = item else {
            return Err(err("'hreflangLinks': cada entrada tiene que ser un struct con 'lang' y 'href'"));
        };
        let Some((_, Value::Str(lang))) = fields.iter().find(|(n, _)| n == "lang") else {
            return Err(err("'hreflangLinks': falta el campo 'lang' o no es String"));
        };
        let Some((_, Value::Str(href))) = fields.iter().find(|(n, _)| n == "href") else {
            return Err(err("'hreflangLinks': falta el campo 'href' o no es String"));
        };
        lines.push(format!("<link rel=\"alternate\" hreflang=\"{}\" href=\"{}\">", escape_html(lang), escape_html(href)));
    }
    Ok(Value::Str(lines.join("\n")))
}

fn call_json_ld(arg_vs: Vec<Value>) -> Result<Value, RuntimeError> {
    let [data]: [Value; 1] = arg_vs.try_into().map_err(|_| err("'jsonLd' requiere 1 argumento (data: Dynamic)"))?;
    let json_v = value_to_json(&data, &std::collections::HashSet::new());
    let s = serde_json::to_string(&json_v).map_err(|e| err(format!("'jsonLd': error al serializar a JSON: {e}")))?;
    let safe = s.replace('<', "\\u003c");
    Ok(Value::Str(format!("<script type=\"application/ld+json\">{safe}</script>")))
}

#[allow(clippy::too_many_arguments)]
/// Convierte un `Value::Variant { enum_name: "PdfBlock", ... }` (construido
/// por el checker contra el `EnumDecl` sintético de `pdf_block_enum_decl`,
/// checker.rs) a la forma plana que `pdf::build` espera. Valida
/// `enum_name`/`variant` defensivamente aunque el checker ya lo garantiza --
/// mismo criterio que el resto de este runtime, nunca confiar ciegamente en
/// que pasó por chequeo de tipos antes de llegar acá.
fn pdf_block_spec_from_value(v: Value) -> Result<pdf::PdfBlockSpec, RuntimeError> {
    let Value::Variant { enum_name, variant, fields } = v else {
        return Err(err("pdf.build: cada elemento de 'blocks' tiene que ser un PdfBlock"));
    };
    if enum_name != "PdfBlock" {
        return Err(err(format!("pdf.build: se esperaba un PdfBlock, se encontró un valor de '{enum_name}'")));
    }
    match variant.as_str() {
        "Text" => {
            let content = match fields.iter().find(|(n, _)| n == "content") {
                Some((_, Value::Str(s))) => s.clone(),
                _ => return Err(err("PdfBlock.Text: falta el campo 'content', o no es String")),
            };
            let bold = match fields.iter().find(|(n, _)| n == "bold") {
                Some((_, Value::Bool(b))) => *b,
                _ => return Err(err("PdfBlock.Text: falta el campo 'bold', o no es Bool")),
            };
            let size = match fields.iter().find(|(n, _)| n == "size") {
                Some((_, Value::Int(n))) => *n as f32,
                _ => return Err(err("PdfBlock.Text: falta el campo 'size', o no es Int")),
            };
            Ok(pdf::PdfBlockSpec::Text { content, bold, size })
        }
        "Table" => {
            let headers = match fields.iter().find(|(n, _)| n == "headers") {
                Some((_, Value::List(items))) => items
                    .iter()
                    .map(|v| match v {
                        Value::Str(s) => Ok(s.clone()),
                        _ => Err(err("PdfBlock.Table: 'headers' tiene que ser String[]")),
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                _ => return Err(err("PdfBlock.Table: falta el campo 'headers', o no es String[]")),
            };
            let rows = match fields.iter().find(|(n, _)| n == "rows") {
                Some((_, Value::List(items))) => items
                    .iter()
                    .map(|row| match row {
                        Value::List(cells) => cells
                            .iter()
                            .map(|v| match v {
                                Value::Str(s) => Ok(s.clone()),
                                _ => Err(err("PdfBlock.Table: cada celda de 'rows' tiene que ser String")),
                            })
                            .collect::<Result<Vec<_>, _>>(),
                        _ => Err(err("PdfBlock.Table: 'rows' tiene que ser String[][]")),
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                _ => return Err(err("PdfBlock.Table: falta el campo 'rows', o no es String[][]")),
            };
            Ok(pdf::PdfBlockSpec::Table { headers, rows })
        }
        other => Err(err(format!("pdf.build: variante desconocida de PdfBlock: '{other}'"))),
    }
}

/// `Value::Variant { enum_name: "ExcelCell", ... }` -> `excel::ExcelCellSpec`,
/// mismo patrón que `pdf_block_spec_from_value`.
fn excel_cell_spec_from_value(v: &Value) -> Result<excel::ExcelCellSpec, RuntimeError> {
    let Value::Variant { enum_name, variant, fields } = v else {
        return Err(err("excel.build: cada celda de 'rows' tiene que ser un ExcelCell"));
    };
    if enum_name != "ExcelCell" {
        return Err(err(format!("excel.build: se esperaba un ExcelCell, se encontró un valor de '{enum_name}'")));
    }
    let value_field = || fields.iter().find(|(n, _)| n == "value").map(|(_, v)| v);
    match variant.as_str() {
        "Text" => match value_field() {
            Some(Value::Str(s)) => Ok(excel::ExcelCellSpec::Text(s.clone())),
            _ => Err(err("ExcelCell.Text: falta el campo 'value', o no es String")),
        },
        "Number" => match value_field() {
            Some(Value::Decimal(n)) => Ok(excel::ExcelCellSpec::Number(*n)),
            _ => Err(err("ExcelCell.Number: falta el campo 'value', o no es Decimal")),
        },
        "Date" => match value_field() {
            Some(Value::Timestamp(ms)) => Ok(excel::ExcelCellSpec::Date(*ms)),
            _ => Err(err("ExcelCell.Date: falta el campo 'value', o no es Timestamp")),
        },
        "Bool" => match value_field() {
            Some(Value::Bool(b)) => Ok(excel::ExcelCellSpec::Bool(*b)),
            _ => Err(err("ExcelCell.Bool: falta el campo 'value', o no es Bool")),
        },
        "Empty" => Ok(excel::ExcelCellSpec::Empty),
        other => Err(err(format!("excel.build: variante desconocida de ExcelCell: '{other}'"))),
    }
}

fn excel_cell_spec_to_value(spec: excel::ExcelCellSpec) -> Value {
    let (variant, fields): (&str, Vec<(String, Value)>) = match spec {
        excel::ExcelCellSpec::Text(s) => ("Text", vec![("value".to_string(), Value::Str(s))]),
        excel::ExcelCellSpec::Number(n) => ("Number", vec![("value".to_string(), Value::Decimal(n))]),
        excel::ExcelCellSpec::Date(ms) => ("Date", vec![("value".to_string(), Value::Timestamp(ms))]),
        excel::ExcelCellSpec::Bool(b) => ("Bool", vec![("value".to_string(), Value::Bool(b))]),
        excel::ExcelCellSpec::Empty => ("Empty", vec![]),
    };
    Value::Variant { enum_name: "ExcelCell".to_string(), variant: variant.to_string(), fields }
}

/// `Value::Struct` con forma `ExcelSheet` -> `excel::ExcelSheetSpec`, mismo
/// patrón de lookup de campos que `smtp_attachments_from_value`.
fn excel_sheet_spec_from_value(v: &Value) -> Result<excel::ExcelSheetSpec, RuntimeError> {
    let Value::Struct(fields) = v else {
        return Err(err("excel.build: cada elemento de 'sheets' tiene que ser un ExcelSheet"));
    };
    let name = match fields.iter().find(|(n, _)| n == "name") {
        Some((_, Value::Str(s))) => s.clone(),
        _ => return Err(err("ExcelSheet: falta el campo 'name', o no es String")),
    };
    let headers = match fields.iter().find(|(n, _)| n == "headers") {
        Some((_, Value::List(items))) => items
            .iter()
            .map(|v| match v {
                Value::Str(s) => Ok(s.clone()),
                _ => Err(err("ExcelSheet: 'headers' tiene que ser String[]")),
            })
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err(err("ExcelSheet: falta el campo 'headers', o no es String[]")),
    };
    let rows = match fields.iter().find(|(n, _)| n == "rows") {
        Some((_, Value::List(items))) => items
            .iter()
            .map(|row| match row {
                Value::List(cells) => cells.iter().map(excel_cell_spec_from_value).collect::<Result<Vec<_>, _>>(),
                _ => Err(err("ExcelSheet: 'rows' tiene que ser ExcelCell[][]")),
            })
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err(err("ExcelSheet: falta el campo 'rows', o no es ExcelCell[][]")),
    };
    Ok(excel::ExcelSheetSpec { name, headers, rows })
}

fn excel_sheet_spec_to_value(spec: excel::ExcelSheetSpec) -> Value {
    Value::Struct(vec![
        ("name".to_string(), Value::Str(spec.name)),
        ("headers".to_string(), Value::List(spec.headers.into_iter().map(Value::Str).collect())),
        (
            "rows".to_string(),
            Value::List(
                spec.rows
                    .into_iter()
                    .map(|row| Value::List(row.into_iter().map(excel_cell_spec_to_value).collect()))
                    .collect(),
            ),
        ),
    ])
}

// Plomería del intérprete: el dispatch más caliente del runtime recibe el
// contexto completo (db, checker, sesiones, token, presupuesto) porque
// cualquier builtin puede necesitar cualquiera de ellos -- agruparlos en un
// struct solo movería la misma lista a otro lado, con una indirección más
// en cada uno de los ~96 brazos.
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
        // GRAMMAR.md §3.230: una consulta ya ordenada -- solo lecturas, con
        // el ORDER BY dentro del SQL. Los nombres de método repetidos con
        // el brazo de `DbCollection` de abajo son deliberadamente los
        // mismos (`page`, `findWhere`), con las mismas validaciones.
        Value::DbQuery(mut query) => match method {
            "orderBy" | "orderByDesc" => {
                query.order.push(order_key(method, &args)?);
                Ok(Value::DbQuery(query))
            }
            "all" => db.select_all_ordered(&query.collection, &query.order).map(Value::List),
            "page" => {
                let limit = as_int(args.first().ok_or_else(|| err("page requiere 2 argumentos (limit, offset)"))?)?;
                let offset = as_int(args.get(1).ok_or_else(|| err("page requiere 2 argumentos (limit, offset)"))?)?;
                if limit < 0 || offset < 0 {
                    return Err(err(format!("db.<c>.orderBy(...).page({limit}, {offset}): limit y offset tienen que ser >= 0")));
                }
                db.select_page_ordered(&query.collection, &query.order, limit, offset).map(Value::List)
            }
            "findWhere" => {
                let f = args.into_iter().next().ok_or_else(|| err("'findWhere' requiere 1 argumento"))?;
                if let Some(conditions) = recognize_pushable_predicate(&f) {
                    if let Some(rows) = db.find_where_conjunction_ordered(&query.collection, &conditions, &query.order)? {
                        return Ok(Value::List(rows));
                    }
                }
                // Predicado no empujable: el ORDER BY igual viaja en SQL
                // (`select_all_ordered`) y el filtro corre en memoria
                // conservando ese orden.
                let mut kept = Vec::new();
                for item in db.select_all_ordered(&query.collection, &query.order)? {
                    if as_bool(&call_callable(f.clone(), vec![item.clone()], db, fns, checker, sessions, current_token, step_budget)?)? {
                        kept.push(item);
                    }
                }
                Ok(Value::List(kept))
            }
            other => Err(err(format!(
                "'{other}' no existe sobre una consulta ordenada (db.<c>.orderBy(...)) -- solo all/page/findWhere/orderBy/orderByDesc (GRAMMAR.md §3.230)"
            ))),
        },
        Value::DbCollection(coll) => match method {
            // GRAMMAR.md §3.230: `orderBy`/`orderByDesc` no consultan nada
            // todavía -- devuelven la consulta ORDENADA (`Value::DbQuery`),
            // y es el `all()`/`page()`/`findWhere()` que venga después el
            // que lleva el ORDER BY dentro de su SQL.
            "orderBy" | "orderByDesc" => Ok(Value::DbQuery(DbQuery { collection: coll, order: vec![order_key(method, &args)?] })),

            // GRAMMAR.md §3.77: intercepta ACÁ (no en db.rs::Db::call) por
            // el mismo motivo que `findWhere`/`deleteWhere` -- necesita
            // `checker` para encontrar qué campos llevan `@autoUpdate`,
            // algo que `Db::call` no recibe. `upsert` (más abajo) llama a
            // `db.call(&coll, "applyPatch", ...)` DIRECTO, así que pasa por
            // `augment_with_auto_update_fields` a mano ahí también -- no a
            // través de este brazo (una única función compartida evita que
            // los dos caminos diverjan).
            "applyPatch" => {
                let mut it = args.into_iter();
                let id = it.next().ok_or_else(|| err("'applyPatch' requiere 2 argumentos"))?;
                let patch = it.next().ok_or_else(|| err("'applyPatch' requiere 2 argumentos"))?;
                let patch = augment_with_auto_update_fields(&coll, checker, patch);
                db.call(&coll, "applyPatch", vec![id, patch])
            }
            "findWhere" => {
                let f = args.into_iter().next().ok_or_else(|| err("'findWhere' requiere 1 argumento"))?;
                // GRAMMAR.md §3.95/§3.108/§3.109: `|x| x.campo OP valor`
                // (OP en ==/!=/</<=/>/>=), incluida una conjunción `&&` de
                // varias hojas así, empuja a un `SELECT ... WHERE` real --
                // solo las filas que matchean viajan del motor al proceso.
                // Cualquier otra forma de predicado cae al camino de
                // siempre, dos líneas más abajo.
                if let Some(conditions) = recognize_pushable_predicate(&f) {
                    if let Some(rows) = db.find_where_conjunction(&coll, &conditions)? {
                        return Ok(Value::List(rows));
                    }
                }
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
            // GRAMMAR.md §3.95/§3.108: `db.<c>.countWhere(fn(T) -> Bool) -> Int`
            // -- mismo empuje a SQL que `findWhere` de arriba cuando el
            // predicado tiene la forma `|x| x.campo OP valor`, esta vez un
            // `SELECT COUNT(*) ... WHERE` real (CERO filas viajan del motor
            // al proceso, ni siquiera las que matchean). El caso real que lo
            // motiva: `db.reviews.countWhere(|r| r.productId == productId)`
            // -- antes de esto, la única forma de contar era
            // `findWhere(...).length()`, que trae la colección ENTERA a
            // memoria solo para descartarla y quedarse con un número.
            "countWhere" => {
                let f = args.into_iter().next().ok_or_else(|| err("'countWhere' requiere 1 argumento"))?;
                if let Some(conditions) = recognize_pushable_predicate(&f) {
                    if let Some(n) = db.count_where_conjunction(&coll, &conditions)? {
                        return Ok(Value::Int(n));
                    }
                }
                let all_val = db.call(&coll, "all", vec![])?;
                let Value::List(items) = all_val else { return Ok(Value::Int(0)); };
                let mut count = 0i64;
                for item in items {
                    if as_bool(&call_callable(f.clone(), vec![item], db, fns, checker, sessions, current_token, step_budget)?)? {
                        count += 1;
                    }
                }
                Ok(Value::Int(count))
            }
            // GRAMMAR.md §3.145: mismo empuje a SQL que `findWhere`/
            // `countWhere` (§3.95/§3.108/§3.109) para la SELECCIÓN -- cuando
            // el predicado tiene la forma pusheable, `find_where_conjunction`
            // hace un `SELECT ... WHERE` real (respetando `@softDelete`
            // automáticamente, igual que `conjunction_condition` ya hace
            // para `countWhere`/`findWhere`) en vez de traer la colección
            // ENTERA a memoria solo para descartar la mayoría en el
            // intérprete. El BORRADO en sí sigue siendo fila por fila vía
            // `db.call(&coll, "delete", ...)` -- a propósito, no un `DELETE
            // ... WHERE` de una sola sentencia: cada `delete()` publica la
            // fila borrada a cualquier `stream` suscripto a esta colección
            // (GRAMMAR.md §3.16), y una sentencia bulk no tiene forma de dar
            // ese aviso por fila. Cuando la selección viene YA filtrada por
            // SQL, el predicado no se vuelve a evaluar en el intérprete
            // (`already_filtered`) -- confiar en el mismo WHERE que
            // `findWhere`/`countWhere` ya confían.
            "deleteWhere" => {
                let f = args.into_iter().next().ok_or_else(|| err("'deleteWhere' requiere 1 argumento"))?;
                let (items, already_filtered) = match recognize_pushable_predicate(&f) {
                    Some(conditions) => match db.find_where_conjunction(&coll, &conditions)? {
                        Some(rows) => (rows, true),
                        None => (all_items(db, &coll)?, false),
                    },
                    None => (all_items(db, &coll)?, false),
                };
                let mut count = 0i64;
                for item in items {
                    let matches = if already_filtered {
                        true
                    } else {
                        as_bool(&call_callable(f.clone(), vec![item.clone()], db, fns, checker, sessions, current_token, step_budget)?)?
                    };
                    if matches {
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
            // GRAMMAR.md §3.76: cada elemento pasa por el mismo `insert`
            // real de siempre (una sentencia SQL autocommit por fila) -- lo
            // que ahorra es la ida y vuelta HTTP N veces desde el cliente,
            // no el costo de N inserts contra la base. Sin transacción
            // envolvente (mismo criterio "autocommit por sentencia" que el
            // resto del lenguaje, ver GRAMMAR.md §3.17/§2.1): si el item 3
            // de 5 falla, los 2 primeros quedan insertados.
            "insertMany" => {
                let items = args.into_iter().next().ok_or_else(|| err("'insertMany' requiere 1 argumento"))?;
                let Value::List(items) = items else {
                    return Err(err("'insertMany': se esperaba una lista"));
                };
                let mut inserted = Vec::with_capacity(items.len());
                for item in items {
                    inserted.push(db.call(&coll, "insert", vec![item])?);
                }
                Ok(Value::List(inserted))
            }
            // GRAMMAR.md §3.75: `matchFn` corre en el intérprete sobre
            // TODAS las filas -- salvo que tenga la forma pusheable
            // reconocida (`recognize_pushable_predicate`, la MISMA que
            // `findWhere`/`countWhere`/`deleteWhere` ya usan), en cuyo caso
            // la SELECCIÓN se empuja a SQL igual que esos tres (26/08/2026,
            // landmine del barrido de "límites honestos": una colección que
            // crece de cientos a decenas de miles de filas hacía que un
            // `upsert` antes instantáneo empezara a tardar segundos, sin
            // ningún error ni aviso -- se notaba por quejas de latencia,
            // nunca por el compilador). Se queda con la PRIMERA fila que
            // matchea (pusheada o no). `updateFn` se llama con la fila
            // EXISTENTE completa y devuelve un valor `Omit<T,"id">`
            // COMPLETO (no un `Patch<T>` parcial -- ver el porqué en
            // checker.rs::check_db_method) que se aplica entero vía
            // `applyPatch` sobre el MISMO id -- nunca borra e inserta de
            // nuevo, así que el id de la fila actualizada no cambia.
            // Bug real, encontrado auditando GRAMMAR.md §3.158 (26/08/2026,
            // mismo día): buscar la fila existente y decidir insert-o-patch
            // eran dos pasos SEPARADOS, sin ningún candado compartido entre
            // ellos -- ya documentado como no-atómico ENTRE INSTANCIAS
            // distintas de `linkc serve` (§3.44), pero con un hilo real por
            // request (§3.158) la MISMA carrera se volvió posible DENTRO de
            // un solo proceso: dos hilos podían ver "no hay match" a la vez
            // y los dos insertar, duplicando la fila que `upsert` promete
            // que nunca se duplica. `with_exclusive_connection` (el mismo
            // candado reentrante que ya usa `transaction{}`) sostiene la
            // conexión durante TODO el ciclo buscar+decidir+escribir --
            // `match_fn`/`update_fn` pueden llamar de vuelta a `db.<c>.*`
            // sin deadlock porque el candado es reentrante para el MISMO
            // hilo. La carrera entre procesos distintos (§3.44) sigue sin
            // resolver -- eso necesitaría un constraint real de la base,
            // no un candado en memoria de un solo proceso.
            "upsert" => db.with_exclusive_connection(|| {
                let mut it = args.into_iter();
                let (Some(match_fn), Some(insert_value), Some(update_fn)) = (it.next(), it.next(), it.next()) else {
                    return Err(err("'upsert' requiere 3 argumentos (matchFn, insertValue, updateFn)"));
                };
                let (items, already_filtered) = match recognize_pushable_predicate(&match_fn) {
                    Some(conditions) => match db.find_where_conjunction(&coll, &conditions)? {
                        Some(rows) => (rows, true),
                        None => (all_items(db, &coll)?, false),
                    },
                    None => (all_items(db, &coll)?, false),
                };
                let mut existing = None;
                for item in items {
                    let matches = if already_filtered {
                        true
                    } else {
                        as_bool(&call_callable(match_fn.clone(), vec![item.clone()], db, fns, checker, sessions, current_token, step_budget)?)?
                    };
                    if matches {
                        existing = Some(item);
                        break;
                    }
                }
                match existing {
                    Some(row) => {
                        let Value::Struct(row_fields) = &row else {
                            return Err(err("'upsert': la fila existente no es un struct"));
                        };
                        // GRAMMAR.md §3.177: `id` puede ser `Value::Int` o
                        // `Value::Uuid` según la PK de la colección --
                        // `applyPatch` (checker.rs::check_db_method) ya
                        // acepta los dos, así que acá alcanza con pasar el
                        // `Value` de id tal cual, sin desenvolverlo a i64.
                        let Some((_, id_value)) = row_fields.iter().find(|(n, _)| n == "id") else {
                            return Err(err("'upsert': la fila existente no tiene 'id'"));
                        };
                        let id_value = id_value.clone();
                        let new_value =
                            call_callable(update_fn, vec![row], db, fns, checker, sessions, current_token, step_budget)?;
                        let new_value = augment_with_auto_update_fields(&coll, checker, new_value);
                        db.call(&coll, "applyPatch", vec![id_value, new_value])
                    }
                    None => db.call(&coll, "insert", vec![insert_value]),
                }
            }),
            _ => db.call(&coll, method, args),
        },

        Value::List(items) => match method {
            "take" => {
                let n = as_int(args.first().ok_or_else(|| err("take requiere 1 argumento"))?)? as usize;
                Ok(Value::List(items.into_iter().take(n).collect()))
            }
            // GRAMMAR.md §3.230: orden en memoria por una clave derivada.
            // Estable (`sort_by` de Rust): dos elementos con la misma clave
            // conservan su orden relativo. Las claves se evalúan UNA vez por
            // elemento, antes de ordenar -- un closure caro no se paga
            // O(n log n) veces -- y un error de comparación (tipos
            // mezclados, NaN) sale como RuntimeError limpio, nunca un panic
            // dentro del comparador.
            "sortBy" | "sortByDesc" => {
                let f = args.into_iter().next().ok_or_else(|| err(format!("'{method}' requiere 1 argumento")))?;
                let mut keyed = Vec::with_capacity(items.len());
                for item in items {
                    let key = call_callable(f.clone(), vec![item.clone()], db, fns, checker, sessions, current_token, step_budget)?;
                    keyed.push((key, item));
                }
                let desc = method == "sortByDesc";
                let mut failure: Option<RuntimeError> = None;
                keyed.sort_by(|(a, _), (b, _)| match order_cmp(a, b, desc) {
                    Ok(o) => o,
                    Err(e) => {
                        failure.get_or_insert(e);
                        std::cmp::Ordering::Equal
                    }
                });
                if let Some(e) = failure {
                    return Err(e);
                }
                Ok(Value::List(keyed.into_iter().map(|(_, item)| item).collect()))
            }
            "length" => Ok(Value::Int(items.len() as i64)),
            // GRAMMAR.md §3.101: checker.rs ya garantizó que esto es
            // `List<Int>` -- `Int64`/`Float` quedan afuera a propósito esta
            // ronda, ver la doc ahí para el motivo (una lista vacía no lleva
            // ningún tag de tipo de elemento en runtime).
            "sum" => {
                // AUDIT-2026-08-27.md #16: mismo riesgo que `+` binario --
                // `total += as_int(item)?` es una suma cruda de `i64`, así
                // que una lista cuyos elementos sumados superan `i64::MAX`
                // panicaba (perfil `dev`) o wrappeaba en silencio
                // (`release`) en vez de dar un `RuntimeError` limpio.
                let mut total: i64 = 0;
                for item in &items {
                    let n = as_int(item)?;
                    total = total
                        .checked_add(n)
                        .ok_or_else(|| err(format!("desborde aritmético: la suma de List<Int>.sum() supera el rango de Int ({total} + {n})")))?;
                }
                Ok(Value::Int(total))
            }
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
            // PLAN.md §9.14 ítem 2 -- el checker ya acotó el tipo de
            // elemento a los que tienen un `PartialEq` sólido (Decimal/
            // Struct/Variant quedan afuera, ver checker.rs), así que
            // reusar `==` acá es seguro sin ningún caso especial.
            "contains" => {
                let target = args.into_iter().next().ok_or_else(|| err("'contains' requiere 1 argumento"))?;
                Ok(Value::Bool(items.contains(&target)))
            }
            other => Err(err(format!("método de lista desconocido: '{other}'"))),
        },
        Value::Int(n) => match method {
            "toFloat" => Ok(Value::Float(n as f64)),
            "toInt64" => Ok(Value::Int64(n)),
            "toDecimal" => decimal_from_int(n),
            "toString" => Ok(Value::Str(n.to_string())),
            other => Err(err(format!("método desconocido sobre Int: '{other}'"))),
        },
        Value::Int64(n) => match method {
            "toInt" => Ok(Value::Int(n)),
            "toString" => Ok(Value::Str(n.to_string())),
            other => Err(err(format!("método desconocido sobre Int64: '{other}'"))),
        },
        // GRAMMAR.md §3.184: sin `.toInt()` -- a diferencia de Int64 (mismo
        // ancho que Int, ida y vuelta exacta), un Decimal con parte
        // fraccionaria distinta de cero perdería información silenciosa al
        // truncar a Int; sin caso real que lo justifique todavía, `.toFloat()`
        // (que ya declara su propia pérdida de precisión) cubre el mismo hueco.
        Value::Decimal(n) => match method {
            "toFloat" => Ok(Value::Float(n as f64 / DECIMAL_SCALE as f64)),
            "toString" => Ok(Value::Str(format_decimal(n))),
            other => Err(err(format!("método desconocido sobre Decimal: '{other}'"))),
        },
        Value::Float(n) => match method {
            "toInt" => Ok(Value::Int(n as i64)), // trunca hacia cero, no redondea (GRAMMAR.md §3.8)
            "toDecimal" => decimal_from_float(n),
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
            // GRAMMAR.md §3.198: indexado por CARACTER, no por byte -- igual
            // que `length()` (`chars().count()`, no `.len()`), para que las
            // dos formas de medir un string coincidan siempre en cualquier
            // string no-ASCII. Rango inválido rechazado ANTES de tocar el
            // string, mismo criterio que `dateFromParts`/`crypto.randomInt`.
            "substring" => {
                let (start, end) = match (args.first(), args.get(1)) {
                    (Some(Value::Int(a)), Some(Value::Int(b))) => (*a, *b),
                    _ => return Err(err("'substring' requiere dos argumentos Int (start, end)")),
                };
                let len = s.chars().count() as i64;
                if start < 0 || end > len || start > end {
                    return Err(err(format!(
                        "'substring' fuera de rango: start={start}, end={end}, longitud={len} (se exige 0 <= start <= end <= longitud)"
                    )));
                }
                Ok(Value::Str(s.chars().skip(start as usize).take((end - start) as usize).collect()))
            }
            "replace" => {
                let (target, replacement) = match (args.first(), args.get(1)) {
                    (Some(Value::Str(t)), Some(Value::Str(r))) => (t, r),
                    _ => return Err(err("'replace' requiere dos argumentos String (target, replacement)")),
                };
                Ok(Value::Str(s.replace(target.as_str(), replacement.as_str())))
            }
            // Separador vacío: mismo comportamiento que `str::split` nativo
            // de Rust (un elemento vacío antes del primer caracter y
            // después del último, cada caracter en el medio) -- definido y
            // testeado, no un caso especial inventado.
            "split" => {
                let separator = match args.first() {
                    Some(Value::Str(sep)) => sep,
                    _ => return Err(err("'split' requiere un argumento String (separator)")),
                };
                Ok(Value::List(s.split(separator.as_str()).map(|p| Value::Str(p.to_string())).collect()))
            }
            "padStart" => {
                let (length, pad) = match (args.first(), args.get(1)) {
                    (Some(Value::Int(l)), Some(Value::Str(p))) => (*l, p),
                    _ => return Err(err("'padStart' requiere un argumento Int (length) y un argumento String (pad)")),
                };
                pad_to_length("padStart", &s, length, pad, true)
            }
            "padEnd" => {
                let (length, pad) = match (args.first(), args.get(1)) {
                    (Some(Value::Int(l)), Some(Value::Str(p))) => (*l, p),
                    _ => return Err(err("'padEnd' requiere un argumento Int (length) y un argumento String (pad)")),
                };
                pad_to_length("padEnd", &s, length, pad, false)
            }
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
            "addMillis" => {
                let n = match args.first() {
                    Some(Value::Int64(n)) => *n,
                    _ => return Err(err("'addMillis' requiere un argumento Int64")),
                };
                ms.checked_add(n)
                    .map(Value::Timestamp)
                    .ok_or_else(|| err("desborde aritmético al sumar milisegundos a un Timestamp"))
            }
            "addSeconds" => {
                let n = match args.first() {
                    Some(Value::Int(n)) => *n,
                    _ => return Err(err("'addSeconds' requiere un argumento Int")),
                };
                checked_timestamp_offset(ms, n, 1_000, "segundos")
            }
            "addMinutes" => {
                let n = match args.first() {
                    Some(Value::Int(n)) => *n,
                    _ => return Err(err("'addMinutes' requiere un argumento Int")),
                };
                checked_timestamp_offset(ms, n, 60_000, "minutos")
            }
            "addHours" => {
                let n = match args.first() {
                    Some(Value::Int(n)) => *n,
                    _ => return Err(err("'addHours' requiere un argumento Int")),
                };
                checked_timestamp_offset(ms, n, 3_600_000, "horas")
            }
            "addDays" => {
                let n = match args.first() {
                    Some(Value::Int(n)) => *n,
                    _ => return Err(err("'addDays' requiere un argumento Int")),
                };
                checked_timestamp_offset(ms, n, 86_400_000, "días")
            }
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
            "awsS3PresignedUrl" => {
                let (access_key_id, secret_access_key, region, bucket, object_key, expires_seconds) =
                    match (args.first(), args.get(1), args.get(2), args.get(3), args.get(4), args.get(5)) {
                        (Some(Value::Str(a)), Some(Value::Str(s)), Some(Value::Str(r)), Some(Value::Str(b)), Some(Value::Str(k)), Some(Value::Int(e))) => (a, s, r, b, k, *e),
                        _ => {
                            return Err(err(
                                "crypto.awsS3PresignedUrl requiere (accessKeyId: String, secretAccessKey: String, region: String, bucket: String, objectKey: String, expiresSeconds: Int)",
                            ))
                        }
                    };
                aws_sigv4_presigned_url(
                    "crypto.awsS3PresignedUrl", "GET", access_key_id, secret_access_key, region, bucket, object_key, expires_seconds, None,
                )
            }
            "awsS3PresignedUploadUrl" => {
                let (access_key_id, secret_access_key, region, bucket, object_key, expires_seconds, content_type) =
                    match (args.first(), args.get(1), args.get(2), args.get(3), args.get(4), args.get(5), args.get(6)) {
                        (Some(Value::Str(a)), Some(Value::Str(s)), Some(Value::Str(r)), Some(Value::Str(b)), Some(Value::Str(k)), Some(Value::Int(e)), Some(Value::Str(c))) => {
                            (a, s, r, b, k, *e, c)
                        }
                        _ => {
                            return Err(err(
                                "crypto.awsS3PresignedUploadUrl requiere (accessKeyId: String, secretAccessKey: String, region: String, bucket: String, objectKey: String, expiresSeconds: Int, contentType: String)",
                            ))
                        }
                    };
                aws_sigv4_presigned_url(
                    "crypto.awsS3PresignedUploadUrl", "PUT", access_key_id, secret_access_key, region, bucket, object_key, expires_seconds, Some(content_type),
                )
            }
            "randomToken" => {
                let length = match args.first() {
                    Some(Value::Int(n)) => *n,
                    _ => return Err(err("crypto.randomToken requiere un argumento Int")),
                };
                // Auditoría adversarial (27/08/2026, AUDIT-2026-08-27.md #1):
                // antes de este chequeo, `length` pasaba directo de `i64` a
                // `usize` con `as` -- un valor negativo (ej. -1) reinterpreta
                // sus bits como un `usize` gigante (~1.8*10^19), y un valor
                // grande pero positivo (ej. i64::MAX) igual. `os_random_bytes`
                // hace `vec![0u8; n]` con eso -- para el primer caso, el
                // propio macro `vec!` detecta que el pedido excede
                // `isize::MAX` y panica con "capacity overflow" (panic
                // normal, mata solo el hilo); para el segundo, el pedido SÍ
                // llega al allocator real del sistema operativo, que no
                // tiene esa memoria -- Rust llama a `handle_alloc_error`, que
                // hace `std::process::abort()` **sin poder atraparse con
                // `catch_unwind`, tire el hilo que tire**. Confirmado contra
                // un `linkc serve` real: una sola request con
                // `{"length": 9223372036854775807}` mataba el proceso
                // ENTERO (bajo `serve-all`, todos los servicios coexistiendo
                // ahí). Mismo criterio que `crypto.randomInt`/`dateFromParts`
                // ya usan para sus propios rangos: rechazar ANTES de tocar
                // memoria, con un `RuntimeError` limpio. El tope de 1024 es
                // generoso a propósito -- ningún token real (sesiones, OTPs,
                // claves de idempotencia) necesita ni una fracción de eso.
                if !(1..=1024).contains(&length) {
                    return Err(err(format!(
                        "crypto.randomToken: 'length' tiene que estar entre 1 y 1024, se recibió {length}"
                    )));
                }
                let length = length as usize;
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

                // GRAMMAR.md §3.226: un hash bcrypt (`$2a$`/`$2b$`/`$2y$`) de
                // una app que migra desde bcryptjs/PHP/Spring/Devise -- se
                // VERIFICA para que el login siga funcionando el día del
                // corte, y `isLegacyHash` lo reporta como legado para que el
                // programa lo re-hashee a Argon2id en el próximo login
                // correcto (§3.58). La crate rechaza sola un hash malformado
                // o con un prefijo que no reconoce → `false`, nunca un error.
                if is_bcrypt_hash(stored) {
                    return Ok(Value::Bool(bcrypt::verify(pwd.as_bytes(), stored).unwrap_or(false)));
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
                // aceptando por compatibilidad (`sha256$<sal>$<hex>`, §3.34,
                // y bcrypt `$2a$`/`$2b$`/`$2y$`, §3.226) -- cualquier otra
                // cosa que no sea un hash Argon2id de verdad tampoco cuenta
                // como "legado migrable": es un valor que ni siquiera
                // `verifyPassword` va a reconocer.
                Ok(Value::Bool(hash.starts_with("sha256$") || is_bcrypt_hash(hash)))
            }
            "uuid" => Ok(Value::Uuid(generate_uuid_v4()?)),
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
        // GRAMMAR.md §3.235: inferencia local con el motor embebido (§3.233)
        // sobre los modelos de `ai { }` (§3.234). `models()` funciona
        // siempre (solo lee la declaración); `generate`/`chat` necesitan
        // el motor que `serve` fijó en el `Db` -- `linkc test` y el harness
        // no cargan modelos, y lo dicen.
        Value::Ai => match method {
            "models" => Ok(Value::List(db.ai_model_aliases().into_iter().map(Value::Str).collect())),
            "generate" | "chat" => {
                #[cfg(feature = "inference")]
                {
                    let mut it = args.into_iter();
                    let model = match it.next() {
                        Some(Value::Str(s)) => s,
                        _ => return Err(err(format!("ai.{method} requiere un alias de modelo (String) como primer argumento"))),
                    };
                    let second = it.next().ok_or_else(|| err(format!("ai.{method} requiere 3 argumentos (model, {}, maxTokens)", if method == "generate" { "prompt" } else { "messages" })))?;
                    let max_tokens = as_int(&it.next().ok_or_else(|| err(format!("ai.{method} requiere 3 argumentos")))?)?;
                    let request = if method == "generate" {
                        match second {
                            Value::Str(p) => crate::inference::AiRequest::Raw(p),
                            _ => return Err(err("ai.generate: el prompt tiene que ser un String")),
                        }
                    } else {
                        let Value::List(items) = second else {
                            return Err(err("ai.chat: messages tiene que ser AiMessage[]"));
                        };
                        crate::inference::AiRequest::Chat(items.iter().map(ai_message_from_value).collect::<Result<Vec<_>, _>>()?)
                    };
                    let engine = db.ai_engine().ok_or_else(|| {
                        err(format!(
                            "ai.{method}: este proceso no tiene el motor resuelto -- hace falta un bloque 'ai {{ }}' y un 'linkc serve' (linkc test y el harness no cargan modelos, GRAMMAR.md §3.235)"
                        ))
                    })?;
                    // Una generación a la vez por programa (GRAMMAR.md
                    // §3.235, límite honesto): dos a la vez en una CPU de 4
                    // núcleos se pisan y las dos salen peor.
                    let _one_at_a_time = db.ai_lock();
                    let result = crate::inference::generate(&engine, &model, request, max_tokens, db.ai_timeout());
                    db.record_ai(&model, &result);
                    let out = result.map_err(err)?;
                    Ok(Value::Str(out.text))
                }
                #[cfg(not(feature = "inference"))]
                {
                    let _ = args;
                    Err(err(format!("ai.{method}: este binario se compiló sin el feature 'inference' (GRAMMAR.md §3.233)")))
                }
            }
            // GRAMMAR.md §3.236: fuera de un `stream` reconocido, `ai.stream`
            // devuelve la lista entera de tokens (mismo motor, mismos
            // errores) -- el servidor es el que lo vuelve incremental.
            "stream" => {
                #[cfg(feature = "inference")]
                {
                    let mut it = args.into_iter();
                    let model = match it.next() {
                        Some(Value::Str(s)) => s,
                        _ => return Err(err("ai.stream requiere un alias de modelo (String) como primer argumento")),
                    };
                    let Some(Value::List(items)) = it.next() else {
                        return Err(err("ai.stream: messages tiene que ser AiMessage[]"));
                    };
                    let max_tokens = as_int(&it.next().ok_or_else(|| err("ai.stream requiere 3 argumentos (model, messages, maxTokens)"))?)?;
                    let request = crate::inference::AiRequest::Chat(items.iter().map(ai_message_from_value).collect::<Result<Vec<_>, _>>()?);
                    let engine = db.ai_engine().ok_or_else(|| {
                        err("ai.stream: este proceso no tiene el motor resuelto -- hace falta un bloque 'ai { }' y un 'linkc serve' (linkc test y el harness no cargan modelos, GRAMMAR.md §3.235)")
                    })?;
                    let _one_at_a_time = db.ai_lock();
                    let mut tokens = Vec::new();
                    let result = crate::inference::generate_with(&engine, &model, request, max_tokens, db.ai_timeout(), &mut |tok| {
                        tokens.push(ai_token_value(tok, false));
                        Ok(())
                    });
                    db.record_ai(&model, &result);
                    result.map_err(err)?;
                    tokens.push(ai_token_value("", true));
                    Ok(Value::List(tokens))
                }
                #[cfg(not(feature = "inference"))]
                {
                    let _ = args;
                    Err(err("ai.stream: este binario se compiló sin el feature 'inference' (GRAMMAR.md §3.233)"))
                }
            }
            other => Err(err(format!("método desconocido sobre ai: '{other}'"))),
        },
        Value::Pdf => match method {
            "build" => {
                let blocks = match args.into_iter().next() {
                    Some(Value::List(items)) => items,
                    _ => return Err(err("pdf.build requiere un argumento PdfBlock[]")),
                };
                let specs = blocks
                    .into_iter()
                    .map(pdf_block_spec_from_value)
                    .collect::<Result<Vec<_>, RuntimeError>>()?;
                let bytes = pdf::build(&specs).map_err(err)?;
                use base64::Engine;
                Ok(Value::Str(base64::engine::general_purpose::STANDARD.encode(bytes)))
            }
            other => Err(err(format!("método desconocido sobre pdf: '{other}'"))),
        },
        Value::Excel => match method {
            "build" => {
                let sheets = match args.into_iter().next() {
                    Some(Value::List(items)) => items,
                    _ => return Err(err("excel.build requiere un argumento ExcelSheet[]")),
                };
                let specs = sheets
                    .iter()
                    .map(excel_sheet_spec_from_value)
                    .collect::<Result<Vec<_>, RuntimeError>>()?;
                let bytes = excel::build(&specs).map_err(err)?;
                use base64::Engine;
                Ok(Value::Str(base64::engine::general_purpose::STANDARD.encode(bytes)))
            }
            "parse" => {
                let b64 = match args.into_iter().next() {
                    Some(Value::Str(s)) => s,
                    _ => return Err(err("excel.parse requiere un argumento String")),
                };
                use base64::Engine;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(b64.as_bytes())
                    .map_err(|e| err(format!("excel.parse: el argumento no es base64 válido: {e}")))?;
                let specs = excel::parse(&bytes).map_err(err)?;
                Ok(Value::List(specs.into_iter().map(excel_sheet_spec_to_value).collect()))
            }
            other => Err(err(format!("método desconocido sobre excel: '{other}'"))),
        },
        Value::Mcp => match method {
            "sample" => {
                let prompt = match args.into_iter().next() {
                    Some(Value::Str(s)) => s,
                    _ => return Err(err("mcp.sample requiere un argumento String")),
                };
                mcp::sample(&prompt).map(Value::Str).map_err(err)
            }
            other => Err(err(format!("método desconocido sobre mcp: '{other}'"))),
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
            "sendMessage" => {
                let Some(Value::Struct(fields)) = args.first() else {
                    return Err(err(
                        "smtp.sendMessage requiere 1 argumento: { to: String[], cc: String[]?, bcc: String[]?, subject: String, body: String, html: Bool?, attachments: {...}[]? }",
                    ));
                };
                send_email_advanced(fields)?;
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
            "redirect" => {
                let (url, permanent) = match (args.first(), args.get(1)) {
                    (Some(Value::Str(u)), Some(Value::Bool(p))) => (u, *p),
                    _ => return Err(err("response.redirect requiere (url: String, permanent: Bool)")),
                };
                if url.is_empty() {
                    return Err(err("response.redirect: 'url' no puede ser un string vacío"));
                }
                // Mismo motivo que filtrar el Origin de una request (arriba,
                // §3.41): un valor que termina en un header HTTP crudo nunca
                // puede llevar CR/LF sin abrir la puerta a inyectar headers
                // extra -- acá el riesgo es más directo, `url` es un String
                // arbitrario que el propio código c-script arma (podría venir
                // de un parámetro de rpc), no algo que ya pasó por el parser
                // de líneas HTTP de tiny_http como el Origin entrante.
                if url.contains('\r') || url.contains('\n') {
                    return Err(err("response.redirect: 'url' no puede contener un salto de línea"));
                }
                db.set_response_status(if permanent { 301 } else { 302 });
                db.set_response_location(url.clone());
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
                match outbound_http(db, url, std::time::Instant::now(), ureq::get(url).timeout(db.http_timeout()).call()) {
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
                match outbound_http(db, url, std::time::Instant::now(), ureq::post(url).timeout(db.http_timeout()).send_string(body)) {
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
                let mut req = ureq::get(url).timeout(db.http_timeout());
                for (name, value) in &headers {
                    req = req.set(name, value);
                }
                match outbound_http(db, url, std::time::Instant::now(), req.call()) {
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
                let mut req = ureq::get(url).timeout(db.http_timeout());
                for (name, value) in &headers {
                    req = req.set(name, value);
                }
                // A diferencia de `get`/`getWithHeaders`, un 4xx/5xx NO es un
                // error de runtime acá -- es justamente el dato que este
                // método existe para exponer. `ureq::Error::Status` trae la
                // `Response` completa (status + headers + body), no solo el
                // código; solo un error de RED de verdad (DNS, conexión
                // rechazada, timeout) sigue siendo `Err`.
                match outbound_http(db, url, std::time::Instant::now(), req.call()) {
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
                let mut req = ureq::post(url).timeout(db.http_timeout());
                for (name, value) in &headers {
                    req = req.set(name, value);
                }
                match outbound_http(db, url, std::time::Instant::now(), req.send_string(body)) {
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
                let mut req = ureq::post(url).timeout(db.http_timeout());
                for (name, value) in &headers {
                    req = req.set(name, value);
                }
                match outbound_http(db, url, std::time::Instant::now(), req.send_string(body)) {
                    Ok(resp) => {
                        let text = resp.into_string().unwrap_or_default();
                        Ok(Value::Str(text))
                    }
                    Err(e) => Err(err(format!("error HTTP al hacer POST a {url}: {e}"))),
                }
            }
            "postWithRetry" => {
                let url = match args.first() {
                    Some(Value::Str(s)) => s,
                    _ => return Err(err("http.postWithRetry requiere un argumento URL String")),
                };
                let body = match args.get(1) {
                    Some(Value::Str(s)) => s,
                    _ => return Err(err("http.postWithRetry requiere un argumento Body String")),
                };
                let headers = match args.get(2) {
                    Some(Value::List(items)) => http_headers_from_value(items)?,
                    _ => return Err(err("http.postWithRetry requiere una lista de headers como tercer argumento")),
                };
                let max_attempts = match args.get(3) {
                    Some(v) => as_int(v)?,
                    _ => return Err(err("http.postWithRetry requiere maxAttempts: Int como cuarto argumento")),
                };
                if max_attempts <= 0 {
                    return Err(err(format!(
                        "http.postWithRetry: 'maxAttempts' tiene que ser mayor a 0, se recibió {max_attempts}"
                    )));
                }
                let mut last_error = String::new();
                for attempt in 0..max_attempts {
                    if attempt > 0 {
                        std::thread::sleep(http_retry_backoff(attempt));
                    }
                    let mut req = ureq::post(url).timeout(db.http_timeout());
                    for (name, value) in &headers {
                        req = req.set(name, value);
                    }
                    match outbound_http(db, url, std::time::Instant::now(), req.send_string(body)) {
                        Ok(resp) => return Ok(Value::Str(resp.into_string().unwrap_or_default())),
                        Err(e) => last_error = e.to_string(),
                    }
                }
                Err(err(format!(
                    "error HTTP al hacer POST a {url} tras {max_attempts} intento(s): {last_error}"
                )))
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
            "destroyAllSessions" => {
                let user_id_val = args.into_iter().next().ok_or_else(|| err("destroyAllSessions requiere 1 argumento (userId)"))?;
                let user_id = as_int(&user_id_val)?;
                Ok(Value::Int(sessions.destroy_all_for_user(user_id) as i64))
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
            // GRAMMAR.md §3.197: accessor genérico de un claim JWT --
            // mismo criterio de indistinguibilidad que currentRole/
            // currentUserId (`null` para "sin sesión"/"token vencido"/
            // "claim ausente" por igual, nunca revela cuál de los tres).
            "claim" => {
                let name = match args.first() {
                    Some(Value::Str(s)) => s,
                    _ => return Err(err("auth.claim requiere un argumento String (name)")),
                };
                let claim = current_token.and_then(|tok| sessions.claim_for(tok, name));
                Ok(claim.map(Value::Str).unwrap_or(Value::Null))
            }
            // GRAMMAR.md §3.152: bloqueo de cuenta configurable -- tres
            // primitivas sobre `SessionStore` (mismo store que ya guarda
            // sesiones, un solo lugar en memoria de un solo proceso).
            "recordFailedLogin" => {
                let identifier = match args.first() {
                    Some(Value::Str(s)) => s,
                    _ => return Err(err("auth.recordFailedLogin requiere un argumento String (identifier)")),
                };
                sessions.record_failed_login(identifier);
                Ok(Value::Null)
            }
            "failedLoginCount" => {
                let (Some(Value::Str(identifier)), Some(window_seconds)) = (args.first(), args.get(1)) else {
                    return Err(err("auth.failedLoginCount requiere (identifier: String, windowSeconds: Int)"));
                };
                let window_seconds = as_int(window_seconds)?;
                if window_seconds < 0 {
                    return Err(err("auth.failedLoginCount: 'windowSeconds' no puede ser negativo"));
                }
                Ok(Value::Int(sessions.failed_login_count(identifier, std::time::Duration::from_secs(window_seconds as u64))))
            }
            "resetFailedLogins" => {
                let identifier = match args.first() {
                    Some(Value::Str(s)) => s,
                    _ => return Err(err("auth.resetFailedLogins requiere un argumento String (identifier)")),
                };
                sessions.reset_failed_logins(identifier);
                Ok(Value::Null)
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
    run_program_tests_filtered(program, None)
}

/// Como `run_program_tests`, pero corriendo solo los bloques `test "..."`
/// cuyo NOMBRE contiene `filter` (substring, sensible a mayúsculas -- mismo
/// criterio que `cargo test <substring>`, no un match exacto ni una regex)
/// -- `linkc test <archivo> --filter <nombre>` (PLAN.md §9.7, GRAMMAR.md
/// §3.82). `None` corre todos, comportamiento idéntico a
/// `run_program_tests` (que delega acá con `None` en vez de duplicar el
/// cuerpo).
pub fn run_program_tests_filtered(program: &Program, filter: Option<&str>) -> Result<TestSummary, RuntimeError> {
    run_tests_core(program, filter, None)
}

/// Como `run_program_tests_filtered`, pero corriendo TODOS los tests contra
/// el MISMO `db` ya conectado -- GRAMMAR.md §3.99, `linkc test --db
/// <url-postgres>`. El caso real que lo motiva: un bug de decodificación
/// del wire binario de PostgreSQL (§3.91) es invisible corriendo contra
/// SQLite `:memory:` -- los dos backends emiten SQL distinto para el mismo
/// `.link`, así que "pasa contra SQLite" no prueba nada sobre Postgres.
///
/// SIN el aislamiento por test que `:memory:` da gratis (una conexión
/// SQLite nueva, vacía, por cada test -- ver `run_program_tests_filtered`).
/// Postgres no tiene un equivalente de "`:memory:`": reconectar a la MISMA
/// URL para cada test daría el MISMO estado persistente, no uno fresco, así
/// que en vez de fingir un aislamiento que no existe, esta variante
/// comparte `db` a propósito -- lo que un test insertó sigue ahí para el
/// siguiente. Correr esto contra una base de TEST dedicada, nunca contra
/// producción, es responsabilidad de quien pasa la URL, no algo que esta
/// función pueda verificar.
pub fn run_program_tests_against_db(program: &Program, filter: Option<&str>, db: &Db) -> Result<TestSummary, RuntimeError> {
    run_tests_core(program, filter, Some(db))
}

/// Como `run_program_tests`, pero corriendo solo los bloques `test "..."`
/// cuyo NOMBRE contiene `filter` (substring, sensible a mayúsculas -- mismo
/// criterio que `cargo test <substring>`, no un match exacto ni una regex)
/// -- `linkc test <archivo> --filter <nombre>` (PLAN.md §9.7, GRAMMAR.md
/// §3.82). `None` corre todos, comportamiento idéntico a
/// `run_program_tests` (que delega acá con `None` en vez de duplicar el
/// cuerpo).
fn run_tests_core(program: &Program, filter: Option<&str>, shared_db: Option<&Db>) -> Result<TestSummary, RuntimeError> {
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
        .filter(|t| filter.is_none_or(|f| t.name.contains(f)))
        .collect();

    let mut passed = 0;
    let mut failed = Vec::new();

    for test in &tests {
        // `:memory:` fresca por test (comportamiento de siempre) si no hay
        // `shared_db` -- ver el doc de `run_program_tests_against_db` para
        // por qué el camino Postgres NO puede dar la misma garantía.
        let fresh_db;
        let db: &Db = match shared_db {
            Some(db) => db,
            None => {
                fresh_db = Db::new(program, std::path::Path::new(":memory:"));
                &fresh_db
            }
        };
        let sessions = SessionStore::new();
        let step_budget = Cell::new(0u64);
        let env = Env::new();
        match eval_block(&test.body, &env, db, &fns, &checker, &sessions, None, &step_budget) {
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
    // GRAMMAR.md §3.232: los campos `@hidden` se quitan ACÁ, en el único
    // borde por el que sale un resultado (rpc, stream y MCP pasan todos por
    // esta función) -- guiado por el tipo de retorno DECLARADO, porque un
    // `Value::Struct` no lleva el nombre de su type.
    let json = value_to_json(&result, &simple_enums);
    Ok(match checker.resolve_type(&rpc.return_type) {
        Ok(ret_ty) if !checker.hidden_fields.is_empty() => strip_hidden_json(json, &ret_ty, &checker),
        _ => json,
    })
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
        // Siempre string, forma fija de EXACTAMENTE 4 decimales (GRAMMAR.md
        // §3.184) -- mismo motivo que Int64: un `number` JSON perdería
        // exactitud del lado del cliente.
        Type::Decimal => j.as_str().and_then(parse_decimal).map(Value::Decimal).ok_or_else(mismatch),
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
                apply_type_level_checks(checker, n, &v, path)?;
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
            let Type::Struct { fields, name } = &**inner else {
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
            let v = Value::Struct(out);
            // AUDIT-2026-08-27.md #3: este era el ÚNICO lugar que construye
            // un struct a partir del wire SIN pasar por
            // `apply_field_validators` -- el brazo `Type::Struct` de acá
            // arriba sí lo hace. Consecuencia real, confirmada en vivo: un
            // `@validate(email)` que `create` rechazaba con 400 pasaba
            // derecho por `applyPatch`/`Patch<T>` y quedaba persistido tal
            // cual. `apply_field_validators` ya tolera un valor PARCIAL (un
            // patch nunca trae todos los campos) -- por diseño, solo valida
            // las claves que de verdad están presentes en `entries`, así que
            // no hace falta ningún cambio ahí, alcanza con dejar de saltarse
            // la llamada.
            if let Some(n) = name {
                if let Some(ast_fields) = field_annotations_for(checker, n, None) {
                    apply_field_validators(ast_fields, &v, path)?;
                }
                apply_type_level_checks(checker, n, &v, path)?;
            }
            Ok(v)
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

/// Pisa a `now()` (GRAMMAR.md §3.77) cualquier campo `@autoUpdate` de la
/// colección `coll` dentro de `patch` -- sin importar qué traía el patch
/// para ese campo (o si no traía nada). `coll` es un nombre de COLECCIÓN de
/// `db` (ej. "counters"), no el nombre del tipo -- primer paso es resolverlo
/// a su tipo de elemento vía `checker.db_collections()` (mismo mapa que usa
/// el resto del checker para esto) para poder reusar `field_annotations_for`.
/// Si `patch` no es un `Value::Struct` (no debería pasar, `applyPatch` ya lo
/// exige en otro lado) se devuelve tal cual, sin tocar nada.
fn augment_with_auto_update_fields(coll: &str, checker: &Checker, patch: Value) -> Value {
    let Value::Struct(mut fields) = patch else { return patch };
    let Some(crate::types::Type::Struct { name: Some(type_name), .. }) = checker.db_collections().get(coll) else {
        return Value::Struct(fields);
    };
    let Some(ast_fields) = field_annotations_for(checker, type_name, None) else {
        return Value::Struct(fields);
    };
    let auto_update_names: Vec<&str> = ast_fields.iter().filter(|f| f.auto_update()).map(|f| f.name.as_str()).collect();
    if auto_update_names.is_empty() {
        return Value::Struct(fields);
    }
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    for name in auto_update_names {
        fields.retain(|(n, _)| n != name);
        fields.push((name.to_string(), Value::Timestamp(now_ms)));
    }
    Value::Struct(fields)
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
        if let Some(validator) = af.validator() {
            // Ausente (campo opcional que no vino) o presente pero `Null`:
            // nada que validar -- `@validate` no vuelve requerido un campo
            // opcional.
            if let Some((_, Value::Str(s))) = entries.iter().find(|(n, _)| n == &af.name) {
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
                        // (checker::check_field_validators) -- si llegó
                        // hasta acá, compilar de nuevo no puede fallar.
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
        }
        // GRAMMAR.md §3.96: `@check(min/max/range, ...)` -- mismos DOS
        // puntos de entrada que `@validate` arriba (wire y `StructLit`
        // construido en un rpc), mismo criterio de "ausente o Null: nada
        // que validar, @check no vuelve requerido un campo opcional".
        if let Some(check) = af.check() {
            if let Some((_, v)) = entries.iter().find(|(n, _)| n == &af.name) {
                if let Some(n) = as_check_number(v) {
                    if let Err(msg) = check_number_bounds(n, check) {
                        return Err(bad_req(format!("'{path}.{}': {msg} (@check)", af.name)));
                    }
                }
                // GRAMMAR.md §3.146: `@check(minLength/maxLength, ...)`
                // sobre `String` -- mismo criterio que la rama numérica de
                // arriba, sobre `Value::Str` en vez de `as_check_number`.
                if let Value::Str(s) = v {
                    if let Err(msg) = check_string_length(s, check) {
                        return Err(bad_req(format!("'{path}.{}': {msg} (@check)", af.name)));
                    }
                }
            }
        }
    }
    Ok(())
}

/// `@check(<expr>)` de nivel `type` (GRAMMAR.md §3.173) -- mismos DOS
/// puntos de entrada que `apply_field_validators` arriba (wire y
/// `StructLit` construido en el cuerpo de un rpc/`applyPatch`), pero
/// operando sobre TODOS los campos que la expresión referencia a la vez,
/// no campo por campo. Un valor PARCIAL (un patch nunca trae todos los
/// campos) simplemente SALTEA una expresión completa si le falta CUALQUIER
/// campo que referencia -- generaliza el mismo criterio de "ausente: nada
/// que validar" que `apply_field_validators` ya aplica campo por campo.
fn apply_type_level_checks(checker: &Checker, type_name: &str, value: &Value, path: &str) -> Result<(), RuntimeError> {
    let Value::Struct(entries) = value else { return Ok(()) };
    let Some(decl) = checker.types.get(type_name) else { return Ok(()) };
    for ann in &decl.annotations {
        let TypeAnnotation::Check(expr) = ann else { continue };
        if !check_expr_fields_present(&expr.node, entries) {
            continue;
        }
        match eval_check_expr(&expr.node, entries)? {
            Value::Bool(true) => {}
            Value::Bool(false) => {
                return Err(bad_req(format!("'{path}': no cumple una restricción '@check(...)' de tipo (GRAMMAR.md §3.173)")))
            }
            other => unreachable!("el checker ya garantizó que '@check(...)' de tipo tipa a Bool, no {other:?}"),
        }
    }
    Ok(())
}

/// ¿Están presentes en `entries` TODOS los campos que `expr` referencia?
/// (Ver `apply_type_level_checks` -- un valor parcial no alcanza para
/// evaluar una expresión que necesita un campo ausente.)
fn check_expr_fields_present(expr: &Expr, entries: &[(String, Value)]) -> bool {
    match expr {
        Expr::Ident(name) => entries.iter().any(|(n, _)| n == name),
        Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Bool(_) | Expr::Null => true,
        Expr::Paren(inner) => check_expr_fields_present(&inner.node, entries),
        Expr::Unary { operand, .. } => check_expr_fields_present(&operand.node, entries),
        Expr::Binary { left, right, .. } => {
            check_expr_fields_present(&left.node, entries) && check_expr_fields_present(&right.node, entries)
        }
        // `ast::validate_check_expr_shape` ya garantizó que ninguna otra
        // forma llegue hasta acá.
        _ => false,
    }
}

/// Evalúa la expresión de un `@check(<expr>)` de nivel `type` (GRAMMAR.md
/// §3.173) contra los valores YA PRESENTES de una fila -- evaluador chico y
/// autocontenido (sin `db`/`fns`/`sessions`/`step_budget`: el shape que
/// `ast::validate_check_expr_shape` restringió en compilación nunca puede
/// necesitar ninguno de esos -- ni una llamada, ni acceso a `db`, ni un
/// closure) que de todos modos REUSA la MISMA aritmética/comparación que
/// el intérprete general (`checked_int_numeric_op`/`compare`/`as_bool`/
/// `div_or_rem_overflow_message`), para no mantener una segunda copia de
/// esas reglas (desborde, NULL-seguridad de `==`, etc.) por separado.
fn eval_check_expr(expr: &Expr, entries: &[(String, Value)]) -> Result<Value, RuntimeError> {
    match expr {
        Expr::Ident(name) => entries.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone()).ok_or_else(|| {
            err(format!("'@check(...)' de tipo: campo '{name}' no encontrado -- bug interno, ya debería haberse salteado"))
        }),
        Expr::Int(n) => Ok(Value::Int(*n)),
        Expr::Float(x) => Ok(Value::Float(*x)),
        Expr::Str(s) => Ok(Value::Str(s.clone())),
        Expr::Bool(b) => Ok(Value::Bool(*b)),
        Expr::Null => Ok(Value::Null),
        Expr::Paren(inner) => eval_check_expr(&inner.node, entries),
        Expr::Unary { op: UnaryOp::Not, operand } => Ok(Value::Bool(!as_bool(&eval_check_expr(&operand.node, entries)?)?)),
        Expr::Unary { op: UnaryOp::Neg, operand } => match eval_check_expr(&operand.node, entries)? {
            Value::Int(n) => n.checked_neg().map(Value::Int).ok_or_else(|| err(format!("desborde aritmético al negar {n}"))),
            Value::Int64(n) => n.checked_neg().map(Value::Int64).ok_or_else(|| err(format!("desborde aritmético al negar {n}"))),
            Value::Decimal(n) => {
                n.checked_neg().map(Value::Decimal).ok_or_else(|| err(format!("desborde aritmético al negar {}", format_decimal(n))))
            }
            Value::Float(n) => Ok(Value::Float(-n)),
            other => Err(err(format!("'-' unario requiere Int, Int64, Decimal o Float en '@check(...)' de tipo: {other:?}"))),
        },
        Expr::Binary { op, left, right } => {
            let l = eval_check_expr(&left.node, entries)?;
            let r = eval_check_expr(&right.node, entries)?;
            match op {
                BinaryOp::And => Ok(Value::Bool(as_bool(&l)? && as_bool(&r)?)),
                BinaryOp::Or => Ok(Value::Bool(as_bool(&l)? || as_bool(&r)?)),
                BinaryOp::Eq => Ok(Value::Bool(l == r)),
                BinaryOp::NotEq => Ok(Value::Bool(l != r)),
                BinaryOp::Lt => compare(l, r, |o| o == std::cmp::Ordering::Less),
                BinaryOp::LtEq => compare(l, r, |o| o != std::cmp::Ordering::Greater),
                BinaryOp::Gt => compare(l, r, |o| o == std::cmp::Ordering::Greater),
                BinaryOp::GtEq => compare(l, r, |o| o != std::cmp::Ordering::Less),
                BinaryOp::Add => match (l, r) {
                    (Value::Str(a), Value::Str(b)) => Ok(Value::Str(a + &b)),
                    (Value::Decimal(a), Value::Decimal(b)) => decimal_add(a, b),
                    (l, r) => checked_int_numeric_op(l, r, i64::checked_add, |a, b| a + b, |a, b| {
                        err(format!("desborde aritmético al sumar {a} y {b}"))
                    }),
                },
                BinaryOp::Sub => match (l, r) {
                    (Value::Decimal(a), Value::Decimal(b)) => decimal_sub(a, b),
                    (l, r) => checked_int_numeric_op(l, r, i64::checked_sub, |a, b| a - b, |a, b| {
                        err(format!("desborde aritmético al restar {b} de {a}"))
                    }),
                },
                BinaryOp::Mul => match (l, r) {
                    (Value::Decimal(a), Value::Decimal(b)) => decimal_mul(a, b),
                    (l, r) => checked_int_numeric_op(l, r, i64::checked_mul, |a, b| a * b, |a, b| {
                        err(format!("desborde aritmético al multiplicar {a} por {b}"))
                    }),
                },
                BinaryOp::Div => match (l, r) {
                    (Value::Decimal(a), Value::Decimal(b)) => decimal_div(a, b),
                    (l, r) => checked_int_numeric_op(l, r, i64::checked_div, |a, b| a / b, |a, b| div_or_rem_overflow_message("dividir", a, b)),
                },
                // `%` sobre Decimal ya queda rechazado por el checker --
                // nunca alcanzable acá con un Value::Decimal real.
                BinaryOp::Rem => {
                    checked_int_numeric_op(l, r, i64::checked_rem, |a, b| a % b, |a, b| div_or_rem_overflow_message("calcular el resto de", a, b))
                }
                // `ast::validate_check_expr_shape` ya filtra a estos trece --
                // cualquier otro operador (`??`) nunca llega hasta acá.
                other => unreachable!("operador no pusheable en @check de tipo: {other:?}"),
            }
        }
        other => unreachable!("forma no pusheable en @check de tipo: {other:?}"),
    }
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

/// `Value` -> `f64` para `@check(...)` (GRAMMAR.md §3.96) -- `None` para
/// cualquier `Value` que no sea `Int`/`Int64`/`Float` (nunca alcanzable en
/// la práctica: `check_field_checks` ya exigió en compilación que el campo
/// sea uno de esos tres, así que esto es defensivo, no un caso normal).
fn as_check_number(v: &Value) -> Option<f64> {
    match v {
        Value::Int(n) => Some(*n as f64),
        Value::Int64(n) => Some(*n as f64),
        Value::Decimal(n) => Some(*n as f64 / DECIMAL_SCALE as f64),
        Value::Float(f) => Some(*f),
        _ => None,
    }
}

/// El chequeo real de `@check(...)` (GRAMMAR.md §3.96) -- `Err(mensaje)`
/// nombrando el límite violado, sin el path del campo (el caller lo agrega).
fn check_number_bounds(n: f64, check: &FieldCheck) -> Result<(), String> {
    match check {
        FieldCheck::Min(min) if n < *min => Err(format!("{n} es menor que el mínimo permitido ({min})")),
        FieldCheck::Max(max) if n > *max => Err(format!("{n} es mayor que el máximo permitido ({max})")),
        FieldCheck::Range(min, max) if n < *min || n > *max => {
            Err(format!("{n} está fuera del rango permitido [{min}, {max}]"))
        }
        _ => Ok(()),
    }
}

/// El chequeo real de `@check(minLength/maxLength, ...)` (GRAMMAR.md
/// §3.146) -- `Err(mensaje)` nombrando el límite violado, mismo criterio
/// que `check_number_bounds`. Cuenta caracteres Unicode (`chars().count()`),
/// no bytes -- una longitud pensada para un humano leyendo el valor, no el
/// tamaño de su codificación UTF-8.
fn check_string_length(s: &str, check: &FieldCheck) -> Result<(), String> {
    let len = s.chars().count();
    match check {
        FieldCheck::MinLength(min) if (len as f64) < *min => {
            Err(format!("tiene {len} caracteres, menos que el mínimo permitido ({min})"))
        }
        FieldCheck::MaxLength(max) if (len as f64) > *max => {
            Err(format!("tiene {len} caracteres, más que el máximo permitido ({max})"))
        }
        _ => Ok(()),
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
/// GRAMMAR.md §3.238: ¿`service_name.rpc_name` existe en el programa (rpc o
/// stream)? Lo que `--fallback-upstream` usa para decidir "esto es mío" vs
/// "esto todavía es del backend viejo".
pub fn is_declared_member(program: &Program, service_name: &str, rpc_name: &str) -> bool {
    program.items.iter().any(|i| match i {
        Item::Service(s) if s.name == service_name => s.members.iter().any(|m| match m {
            Member::Rpc(r) | Member::Stream(r) => r.name == rpc_name,
        }),
        _ => false,
    })
}

pub fn is_stream_member(program: &Program, service_name: &str, rpc_name: &str) -> bool {
    program.items.iter().any(|i| match i {
        Item::Service(s) if s.name == service_name => s
            .members
            .iter()
            .any(|m| matches!(m, Member::Stream(r) if r.name == rpc_name)),
        _ => false,
    })
}

/// Si `service_name.rpc_name` declaró `@cron("...")` (GRAMMAR.md §3.159) --
/// mismo patrón que `is_stream_member`: `server.rs` lo consulta ANTES de
/// invocar nada, para devolver 404 en vez de correr una tarea que no está
/// pensada para recibir requests HTTP reales (el checker ya garantiza que
/// nunca coexiste con `@route`, pero el path por defecto
/// `POST /{Service}/{rpc}` sigue alcanzando a CUALQUIER rpc por nombre sin
/// este chequeo).
pub fn is_cron_member(program: &Program, service_name: &str, rpc_name: &str) -> bool {
    program.items.iter().any(|i| match i {
        Item::Service(s) if s.name == service_name => {
            s.members.iter().any(|m| matches!(m, Member::Rpc(r) if r.name == rpc_name && r.cron().is_some()))
        }
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

/// `(spec, key_param)` de `@rate_limit("N/ventana"[, key: <param>])` de
/// `{service_name}.{rpc_name}`, si tiene una -- hermana de `required_auth`
/// (mismo archivo/patrón, mismo uso desde `server.rs` antes de invocar
/// nada). `spec` es texto crudo, sin parsear: `server.rs` lo pasa a
/// `rate_limit::RateLimitSpec::parse`, que el checker ya validó que nunca
/// falla para un programa que compiló (GRAMMAR.md §3.39). `key_param`, si
/// está, ya fue validado por el checker como un parámetro real de tipo
/// `String`/`Int` (GRAMMAR.md §3.142).
pub fn required_rate_limit<'a>(program: &'a Program, service_name: &str, rpc_name: &str) -> Option<(&'a str, Option<&'a str>)> {
    program.items.iter().find_map(|i| match i {
        Item::Service(s) if s.name == service_name => s.members.iter().find_map(|m| match m {
            Member::Rpc(r) | Member::Stream(r) if r.name == rpc_name => r.rate_limit(),
            _ => None,
        }),
        _ => None,
    })
}

/// `true` si `{service_name}.{rpc_name}` declaró `@idempotent` (GRAMMAR.md
/// §3.140) -- hermana de `required_rate_limit` (mismo archivo/patrón, mismo
/// uso desde `server.rs` antes de invocar nada). El checker ya garantizó
/// que nunca aparece sobre un `stream`, así que `server.rs` solo necesita
/// consultarlo en el camino de un `rpc` normal.
/// El TTL crudo de `@cache("60s")` de `{service_name}.{rpc_name}`, si hay
/// (GRAMMAR.md §3.144) -- hermana de `required_idempotent`/
/// `required_rate_limit`, mismo patrón. `server.rs` lo pasa a
/// `cache::parse_ttl`, que el checker ya validó que nunca falla para un
/// programa que compiló.
pub fn required_cache<'a>(program: &'a Program, service_name: &str, rpc_name: &str) -> Option<&'a str> {
    program.items.iter().find_map(|i| match i {
        Item::Service(s) if s.name == service_name => s.members.iter().find_map(|m| match m {
            Member::Rpc(r) if r.name == rpc_name => r.cache(),
            _ => None,
        }),
        _ => None,
    })
}

/// El valor crudo de `@cors("...")` de `{service_name}.{rpc_name}`, si hay
/// (GRAMMAR.md §3.147) -- hermana de `required_cache`/`required_rate_limit`,
/// mismo patrón. Busca en `Member::Rpc` Y `Member::Stream` (a diferencia de
/// `required_cache`, que solo aplica a `rpc`) -- un stream SSE también manda
/// headers de CORS reales.
pub fn required_cors<'a>(program: &'a Program, service_name: &str, rpc_name: &str) -> Option<&'a str> {
    program.items.iter().find_map(|i| match i {
        Item::Service(s) if s.name == service_name => s.members.iter().find_map(|m| match m {
            Member::Rpc(r) | Member::Stream(r) if r.name == rpc_name => r.cors(),
            _ => None,
        }),
        _ => None,
    })
}

pub fn required_idempotent(program: &Program, service_name: &str, rpc_name: &str) -> bool {
    program.items.iter().any(|i| match i {
        Item::Service(s) if s.name == service_name => {
            s.members.iter().any(|m| matches!(m, Member::Rpc(r) if r.name == rpc_name && r.idempotent()))
        }
        _ => false,
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
        // String con exactamente 4 decimales -- ver la nota simétrica en
        // json_to_typed_value (GRAMMAR.md §3.184).
        Value::Decimal(n) => json!(format_decimal(*n)),
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
        Value::Db | Value::DbCollection(_) | Value::DbQuery(_) | Value::Auth | Value::Service(_) | Value::Math | Value::Crypto | Value::Http | Value::Json | Value::Base64 | Value::Pdf | Value::Excel | Value::Ai | Value::Mcp | Value::Env | Value::Request | Value::Smtp | Value::Response | Value::BoundMethod(_, _) | Value::FnRef(_) | Value::Closure(..) => {
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

    // ---- `Decimal` (GRAMMAR.md §3.184): aritmética, formateo, redondeo ----

    #[test]
    fn format_decimal_and_parse_decimal_round_trip() {
        for (raw, text) in [(1234500, "123.4500"), (-1234500, "-123.4500"), (0, "0.0000"), (1, "0.0001"), (-1, "-0.0001")] {
            assert_eq!(format_decimal(raw), text);
            assert_eq!(parse_decimal(text), Some(raw));
        }
    }

    #[test]
    fn parse_decimal_rejects_anything_without_exactly_four_fraction_digits() {
        assert_eq!(parse_decimal("19.9"), None, "menos de 4 decimales");
        assert_eq!(parse_decimal("19.99900"), None, "más de 4 decimales");
        assert_eq!(parse_decimal("19"), None, "sin punto decimal");
        assert_eq!(parse_decimal("abc.1234"), None, "parte entera no numérica");
        assert_eq!(parse_decimal(".1234"), None, "parte entera vacía");
        assert_eq!(parse_decimal("19.12ab"), None, "parte fraccionaria no numérica");
    }

    #[test]
    fn div_round_rounds_half_away_from_zero_including_negative_ties() {
        // Empates exactos -- el caso que una fórmula ingenua (truncar en
        // vez de redondear) rompe para negativos.
        assert_eq!(div_round(5, 2), Some(3), "2.5 -> 3 (arriba)");
        assert_eq!(div_round(-5, 2), Some(-3), "-2.5 -> -3 (lejos de cero, NO -2)");
        assert_eq!(div_round(5, -2), Some(-3), "mismo empate, denominador negativo");
        assert_eq!(div_round(-5, -2), Some(3), "los dos negativos -- cociente real positivo");
    }

    #[test]
    fn div_round_rounds_non_ties_to_the_nearest_integer() {
        assert_eq!(div_round(7, 4), Some(2), "1.75 -> 2");
        assert_eq!(div_round(-7, 4), Some(-2), "-1.75 -> -2");
        assert_eq!(div_round(-7, 3), Some(-2), "-2.333 -> -2 (el más cercano, no un empate)");
        assert_eq!(div_round(10, 5), Some(2), "división exacta, sin resto");
        assert_eq!(div_round(0, 5), Some(0));
    }

    #[test]
    fn decimal_add_and_sub_are_exact_integer_arithmetic() {
        let a = parse_decimal("19.9900").unwrap();
        let b = parse_decimal("0.0100").unwrap();
        let Ok(Value::Decimal(sum)) = decimal_add(a, b) else { panic!("se esperaba Decimal") };
        assert_eq!(format_decimal(sum), "20.0000", "19.99 + 0.01 exacto, sin el típico error binario de Float");
        let Ok(Value::Decimal(diff)) = decimal_sub(sum, b) else { panic!("se esperaba Decimal") };
        assert_eq!(diff, a);
    }

    #[test]
    fn decimal_mul_rescales_and_rounds_once() {
        // 19.99 * 0.21 = 4.1979 -- redondeado a 4 decimales, 4.1979 ya
        // entra exacto (sin redondeo real necesario acá).
        let price = parse_decimal("19.9900").unwrap();
        let rate = parse_decimal("0.2100").unwrap();
        let Ok(Value::Decimal(product)) = decimal_mul(price, rate) else { panic!("se esperaba Decimal") };
        assert_eq!(format_decimal(product), "4.1979");
    }

    #[test]
    fn decimal_mul_rounds_a_result_that_does_not_land_exactly_on_four_decimals() {
        let a = parse_decimal("0.3333").unwrap();
        let b = parse_decimal("0.3333").unwrap();
        // 0.3333 * 0.3333 = 0.11108889 -- el 5to decimal (8) redondea el
        // 4to hacia arriba: 0.1110|8889 -> 0.1111 (verificado a mano con
        // la división entera exacta: 11108889 / 10000 = 1110 resto 8889,
        // 2*8889=17778 >= 10000 -> empate/excede, redondea arriba).
        let Ok(Value::Decimal(product)) = decimal_mul(a, b) else { panic!("se esperaba Decimal") };
        assert_eq!(format_decimal(product), "0.1111");
    }

    #[test]
    fn decimal_div_rescales_the_numerator_before_dividing_and_rounds() {
        // 10 / 3 = 3.3333... -- redondea a 3.3333 (repetitivo, nunca
        // termina, el caso central que motiva el redondeo explícito).
        let ten = parse_decimal("10.0000").unwrap();
        let three = parse_decimal("3.0000").unwrap();
        let Ok(Value::Decimal(q)) = decimal_div(ten, three) else { panic!("se esperaba Decimal") };
        assert_eq!(format_decimal(q), "3.3333");
    }

    #[test]
    fn decimal_div_by_zero_is_a_clean_error_not_a_panic() {
        let a = parse_decimal("1.0000").unwrap();
        assert!(decimal_div(a, 0).is_err());
    }

    #[test]
    fn decimal_from_int_and_from_float_construct_the_expected_scaled_value() {
        let Ok(Value::Decimal(from_int)) = decimal_from_int(19) else { panic!("se esperaba Decimal") };
        assert_eq!(format_decimal(from_int), "19.0000");
        let Ok(Value::Decimal(from_float)) = decimal_from_float(19.99) else { panic!("se esperaba Decimal") };
        assert_eq!(format_decimal(from_float), "19.9900", "f64 tiene precisión de sobra para redondear exacto a 4 decimales");
    }

    #[test]
    fn decimal_from_float_rejects_nan_and_infinity() {
        assert!(decimal_from_float(f64::NAN).is_err());
        assert!(decimal_from_float(f64::INFINITY).is_err());
        assert!(decimal_from_float(f64::NEG_INFINITY).is_err());
    }

    #[test]
    fn decimal_add_reports_a_clean_overflow_error_not_a_panic() {
        assert!(decimal_add(i128::MAX, 1).is_err());
    }

    // Bug real encontrado validando el diseño de PLAN.md §9.14 (31/08/2026):
    // `impl PartialEq for Value` no tenía ningún arm para `(Decimal, Decimal)`
    // -- caía al `_ => false` genérico, así que TODA comparación `Decimal ==
    // Decimal` daba `false` en runtime (y `!=` daba `true`), incluso un valor
    // contra sí mismo, pese a que el checker lo dejaba tipar limpio y `<`/`>`
    // sí funcionaban (`compare()` sí tenía su arm). Para un lenguaje donde
    // `Decimal` es el tipo de dinero, esto es serio -- ver GRAMMAR.md §3.199.
    #[test]
    fn decimal_equals_decimal_compares_the_scaled_value_not_always_false() {
        let program = program_from(
            r#"
            service Money {
                rpc same(a: Decimal, b: Decimal) -> Bool { a == b }
                rpc different(a: Decimal, b: Decimal) -> Bool { a != b }
            }
        "#,
        );
        let db = Db::seeded();
        let ten_a = parse_decimal("10.0000").unwrap();
        let ten_b = parse_decimal("10.0000").unwrap();
        let eleven = parse_decimal("11.0000").unwrap();
        assert_eq!(Value::Decimal(ten_a), Value::Decimal(ten_b), "dos Decimal con el mismo valor escalado deben ser == en Rust también, no solo en el wire");
        assert_ne!(Value::Decimal(ten_a), Value::Decimal(eleven), "valores escalados distintos siguen siendo != en Rust");

        let same = invoke_rpc(&program, "Money", "same", &json!({"a": "10.0000", "b": "10.0000"}), &db).unwrap();
        assert_eq!(same, json!(true), "10.0000 == 10.0000 tiene que dar true -- antes del fix daba false siempre");

        let different = invoke_rpc(&program, "Money", "different", &json!({"a": "10.0000", "b": "11.0000"}), &db).unwrap();
        assert_eq!(different, json!(true), "10.0000 != 11.0000 sigue dando true (esto ya funcionaba, confirmar que el fix no lo rompió)");

        let not_different = invoke_rpc(&program, "Money", "different", &json!({"a": "10.0000", "b": "10.0000"}), &db).unwrap();
        assert_eq!(not_different, json!(false), "10.0000 != 10.0000 tiene que dar false -- antes del fix daba true siempre");
    }

    #[test]
    fn decimal_ordering_was_never_affected_by_the_equality_bug() {
        // compare() (usado por </>/<=/>=) ya tenía su arm de Decimal antes de
        // este fix -- confirmando que no se rompió nada al arreglar `==`.
        let program = program_from(
            r#"
            service Money {
                rpc lessThan(a: Decimal, b: Decimal) -> Bool { a < b }
            }
        "#,
        );
        let db = Db::seeded();
        let result = invoke_rpc(&program, "Money", "lessThan", &json!({"a": "9.0000", "b": "10.0000"}), &db).unwrap();
        assert_eq!(result, json!(true));
    }

    // Bug real reportado por la sesión fix-myf-audit-findings tras actualizar
    // a v1.152.0 (31/08/2026): `Decimal.toFloat()`/`.toString()` tipan limpio
    // en el checker y su dispatch en `Value::Decimal(n) => match method` más
    // abajo en este mismo archivo está bien escrito -- pero nunca se llega a
    // ejecutar. `Expr::FieldAccess` tiene su propio allowlist de variantes de
    // `Value` elegibles para envolverse en `Value::BoundMethod` (el mecanismo
    // que difiere la resolución de `x.method` hasta que el `Expr::Call` que
    // lo envuelve corre de verdad) -- y a `Value::Decimal` le faltaba estar
    // en esa lista, así que cualquier método sobre un Decimal fallaba ANTES
    // de llegar al dispatch real, con el mismo error genérico que un campo
    // inexistente: "no se puede acceder al campo 'toFloat' sobre Decimal(...)".
    // Reproducido en vivo contra un `linkc serve` real antes de este fix,
    // confirmando el mismo texto de error -- ver GRAMMAR.md §3.199.
    #[test]
    fn decimal_to_float_and_to_string_are_reachable_through_field_access() {
        let program = program_from(
            r#"
            service Money {
                rpc asFloat(a: Decimal) -> Float { a.toFloat() }
                rpc asString(a: Decimal) -> String { a.toString() }
                rpc chained() -> Float { 123.45.toDecimal().toFloat() }
            }
        "#,
        );
        let db = Db::seeded();

        let as_float = invoke_rpc(&program, "Money", "asFloat", &json!({"a": "10.5000"}), &db)
            .expect("Decimal.toFloat() via un local `let`-bound tiene que resolver, no fallar en FieldAccess");
        assert_eq!(as_float, json!(10.5));

        let as_string = invoke_rpc(&program, "Money", "asString", &json!({"a": "10.5000"}), &db)
            .expect("Decimal.toString() via un local `let`-bound tiene que resolver, no fallar en FieldAccess");
        assert_eq!(as_string, json!("10.5000"));

        let chained = invoke_rpc(&program, "Money", "chained", &json!({}), &db)
            .expect("Decimal.toFloat() encadenado directo sobre el resultado de .toDecimal() tambien tiene que resolver");
        assert_eq!(chained, json!(123.45));
    }

    #[test]
    fn http_retry_backoff_doubles_from_200ms_and_caps_at_5s() {
        assert_eq!(http_retry_backoff(1), std::time::Duration::from_millis(200));
        assert_eq!(http_retry_backoff(2), std::time::Duration::from_millis(400));
        assert_eq!(http_retry_backoff(3), std::time::Duration::from_millis(800));
        assert_eq!(http_retry_backoff(4), std::time::Duration::from_millis(1600));
        assert_eq!(http_retry_backoff(5), std::time::Duration::from_millis(3200));
        assert_eq!(http_retry_backoff(6), std::time::Duration::from_secs(5), "el 6to intento ya superaría el techo sin acotar (6.4s)");
        assert_eq!(http_retry_backoff(100), std::time::Duration::from_secs(5), "un maxAttempts enorme nunca desborda el shift ni supera el techo");
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

    // ---- `id: Uuid` como PK alternativa (GRAMMAR.md §3.177) ----

    /// El ciclo completo -- insert/find/applyPatch/aritmética/sumBy -- sobre
    /// un campo `Decimal` real, contra SQLite real (GRAMMAR.md §3.184).
    /// Confirma de punta a punta: el wire manda/recibe el string de 4
    /// decimales exacto (nunca un número JSON), `+`/`*` dan resultados
    /// EXACTOS (sin el error de redondeo binario que motivó todo el tipo),
    /// y `sumBy` empuja a SQL real sin perder precisión.
    #[test]
    fn decimal_field_supports_the_full_crud_cycle_and_sum_by_against_real_sqlite() {
        let program = program_from(
            r#"
            type LineItem = { id: Int, description: String, unitPrice: Decimal, qty: Int }
            type NewLineItem = { description: String, unitPrice: Decimal, qty: Int }
            db { items: LineItem[] }
            service Items {
                rpc create(description: String, unitPrice: Decimal, qty: Int) -> LineItem {
                    db.items.insert(NewLineItem { description: description, unitPrice: unitPrice, qty: qty })
                }
                rpc get(id: Int) -> LineItem? { db.items.find(id) }
                rpc reprice(id: Int, p: Patch<LineItem>) -> LineItem {
                    db.items.applyPatch(id, p)
                }
                rpc lineTotal(id: Int) -> Decimal? {
                    match db.items.find(id) {
                        item: LineItem => item.unitPrice * item.qty.toDecimal(),
                        null => null,
                    }
                }
                rpc totalValue() -> Decimal[] {
                    db.items.sumBy(|i: LineItem| { i.description }, |i: LineItem| { i.unitPrice })
                        .map(|g: {key: String, value: Decimal}| { g.value })
                }
            }
        "#,
        );
        let db = Db::new(&program, std::path::Path::new(":memory:"));

        let created = invoke_rpc(&program, "Items", "create", &json!({"description": "Widget", "unitPrice": "19.9900", "qty": 3}), &db).unwrap();
        assert_eq!(created["unitPrice"], json!("19.9900"), "el wire manda el string de 4 decimales exacto, nunca un number");
        let id = created["id"].as_i64().unwrap();

        let fetched = invoke_rpc(&program, "Items", "get", &json!({"id": id}), &db).unwrap();
        assert_eq!(fetched["unitPrice"], json!("19.9900"), "round-trip exacto por SQLite real, sin deriva binaria");

        let total = invoke_rpc(&program, "Items", "lineTotal", &json!({"id": id}), &db).unwrap();
        assert_eq!(total, json!("59.9700"), "19.99 * 3 = 59.97 exacto -- Float haría 59.96999999999999...");

        let repriced = invoke_rpc(&program, "Items", "reprice", &json!({"id": id, "p": {"unitPrice": "24.5000"}}), &db).unwrap();
        assert_eq!(repriced["unitPrice"], json!("24.5000"));

        invoke_rpc(&program, "Items", "create", &json!({"description": "Widget", "unitPrice": "0.0100", "qty": 1}), &db).unwrap();
        let sums = invoke_rpc(&program, "Items", "totalValue", &json!({}), &db).unwrap();
        assert_eq!(sums, json!(["24.5100"]), "sumBy real contra SQLite, exacto: 24.50 + 0.01 = 24.51");
    }

    /// El ciclo completo -- insert/find/applyPatch/increment/delete -- sobre
    /// una colección cuya PK es `Uuid`, no `Int`. `insert` nunca recibe un
    /// id (`Omit<T,"id">`, igual que siempre) -- lo genera el runtime del
    /// lado de la app, mismo generador que `crypto.uuid()`.
    #[test]
    fn uuid_pk_collection_supports_the_full_crud_cycle_against_real_sqlite() {
        let program = program_from(
            r#"
            type Lead = { id: Uuid, email: String, score: Int }
            type NewLead = { email: String, score: Int }
            db { leads: Lead[] }
            service Leads {
                rpc create(email: String) -> Lead { db.leads.insert(NewLead { email: email, score: 0 }) }
                rpc get(id: Uuid) -> Lead? { db.leads.find(id) }
                rpc patch(id: Uuid, p: Patch<Lead>) -> Lead { db.leads.applyPatch(id, p) }
                rpc bump(id: Uuid) -> Lead { db.leads.increment(id, |l: Lead| { l.score }, 5) }
                rpc remove(id: Uuid) -> Bool { db.leads.delete(id) }
            }
        "#,
        );
        let db = Db::new(&program, std::path::Path::new(":memory:"));

        let created = invoke_rpc(&program, "Leads", "create", &json!({"email": "a@example.com"}), &db).unwrap();
        let id = created["id"].as_str().expect("id generado, con forma de uuid").to_string();
        assert_eq!(id.len(), 36, "id: {id}");
        assert_eq!(created["score"], json!(0));

        let fetched = invoke_rpc(&program, "Leads", "get", &json!({"id": id}), &db).unwrap();
        assert_eq!(fetched["email"], json!("a@example.com"), "find por el mismo uuid encuentra la fila real");

        let patched = invoke_rpc(&program, "Leads", "patch", &json!({"id": id, "p": {"email": "b@example.com"}}), &db).unwrap();
        assert_eq!(patched["email"], json!("b@example.com"));
        assert_eq!(patched["id"], json!(id), "applyPatch nunca cambia el id");

        let bumped = invoke_rpc(&program, "Leads", "bump", &json!({"id": id}), &db).unwrap();
        assert_eq!(bumped["score"], json!(5), "increment sobre un campo Int de una colección con PK Uuid");

        let removed = invoke_rpc(&program, "Leads", "remove", &json!({"id": id}), &db).unwrap();
        assert_eq!(removed, json!(true));
        let gone = invoke_rpc(&program, "Leads", "get", &json!({"id": id}), &db).unwrap();
        assert_eq!(gone, serde_json::Value::Null, "borrada de verdad, find ya no la encuentra");

        // Dos inserts seguidos nunca chocan de id -- confirma que cada
        // `insert` genera un uuid FRESCO (CSPRNG real, no un contador).
        let a = invoke_rpc(&program, "Leads", "create", &json!({"email": "x@example.com"}), &db).unwrap();
        let b = invoke_rpc(&program, "Leads", "create", &json!({"email": "y@example.com"}), &db).unwrap();
        assert_ne!(a["id"], b["id"]);
    }

    /// `upsert` sobre una colección con PK `Uuid` -- las dos ramas (insert
    /// cuando no matchea, update en el lugar cuando sí) tienen que generar/
    /// preservar un id `Uuid` real, nunca un `Value::Int` (el bug real que
    /// este test hubiera atrapado: `upsert` desenvolvía `Value::Int`
    /// directo del id de la fila existente antes de esta ronda).
    #[test]
    fn upsert_works_on_a_uuid_pk_collection_for_both_branches() {
        let program = program_from(
            r#"
            type Counter = { id: Uuid, name: String, count: Int }
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
        assert_eq!(created["count"], json!(1));
        let created_id = created["id"].as_str().expect("id uuid").to_string();
        assert_eq!(created_id.len(), 36);

        let updated = invoke_rpc(&program, "S", "bump", &json!({"name": "clics"}), &db).unwrap();
        assert_eq!(updated["count"], json!(2), "la segunda llamada actualiza la MISMA fila, no inserta otra");
        assert_eq!(updated["id"], json!(created_id), "upsert nunca cambia el id de la fila que actualiza");
    }

    /// `page`/`maxRow`/`minRow` sobre una colección con PK `Uuid` -- ninguno
    /// de los tres ordena/filtra por `id`, pero los tres SELECCIONAN la
    /// columna `"id"` como parte de la fila completa, así que decodificarla
    /// con el `ColumnKind` equivocado (`select_rows_page`/`top_row`,
    /// encontrado real vía el test de arriba de `upsert`, mismo bug en dos
    /// lugares más) rompía estos tres métodos también.
    #[test]
    fn page_and_max_min_row_work_on_a_uuid_pk_collection() {
        let program = program_from(
            r#"
            type Lead = { id: Uuid, email: String, score: Int }
            type NewLead = { email: String, score: Int }
            db { leads: Lead[] }
            service Leads {
                rpc create(email: String, score: Int) -> Lead { db.leads.insert(NewLead { email: email, score: score }) }
                rpc list(limit: Int, offset: Int) -> Lead[] { db.leads.page(limit, offset) }
                rpc top() -> Lead? { db.leads.maxRow(|l: Lead| { l.score }) }
                rpc bottom() -> Lead? { db.leads.minRow(|l: Lead| { l.score }) }
            }
        "#,
        );
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        invoke_rpc(&program, "Leads", "create", &json!({"email": "a@example.com", "score": 3}), &db).unwrap();
        invoke_rpc(&program, "Leads", "create", &json!({"email": "b@example.com", "score": 9}), &db).unwrap();

        let page = invoke_rpc(&program, "Leads", "list", &json!({"limit": 10, "offset": 0}), &db).unwrap();
        assert_eq!(page.as_array().unwrap().len(), 2);
        for row in page.as_array().unwrap() {
            assert!(row["id"].as_str().map(|s| s.len() == 36).unwrap_or(false), "{row:?}");
        }

        let top = invoke_rpc(&program, "Leads", "top", &json!({}), &db).unwrap();
        assert_eq!(top["score"], json!(9));
        let bottom = invoke_rpc(&program, "Leads", "bottom", &json!({}), &db).unwrap();
        assert_eq!(bottom["score"], json!(3));
    }

    // ---- `crypto.awsS3PresignedUrl` (GRAMMAR.md §3.110) ----

    /// El vector de prueba OFICIAL de AWS ("get-vanilla", del
    /// `aws4_testsuite` publicado por Amazon, obtenido de un mirror en
    /// GitHub -- accessKeyId `AKIDEXAMPLE`, secretAccessKey
    /// `wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY`, fecha `2011-09-09
    /// 23:36:00 GMT`, región `us-east-1`, "servicio" `host`), NO contra un
    /// número inventado -- mismo estándar que `crypto.hmacSha256` (§3.38,
    /// verificado contra un vector de Python). Reconstruye a mano la
    /// derivación de clave + firma final de ESE caso exacto usando
    /// `hmac_sha256_raw` (la pieza nueva, encadenar HMACs con los bytes
    /// CRUDOS del paso anterior como clave del siguiente) y confirma que
    /// el resultado es BYTE A BYTE el que AWS publica. Esta es la parte
    /// más propensa a error de todo `awsS3PresignedUrl` -- si esto está
    /// mal, la URL generada es indistinguible de una bien formada hasta
    /// que S3 la rechaza con 403 en producción.
    #[test]
    fn hmac_sha256_raw_chain_reproduces_the_official_aws_sigv4_test_vector() {
        let secret_access_key = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";
        let date_stamp = "20110909";
        let region = "us-east-1";
        let service = "host";
        let string_to_sign = "AWS4-HMAC-SHA256\n\
             20110909T233600Z\n\
             20110909/us-east-1/host/aws4_request\n\
             366b91fb121d72a00f46bbe8d395f53a102b06dfb7e79636515208ed3fa606b1";

        let k_date = hmac_sha256_raw(format!("AWS4{secret_access_key}").as_bytes(), date_stamp.as_bytes()).unwrap();
        let k_region = hmac_sha256_raw(&k_date, region.as_bytes()).unwrap();
        let k_service = hmac_sha256_raw(&k_region, service.as_bytes()).unwrap();
        let k_signing = hmac_sha256_raw(&k_service, b"aws4_request").unwrap();
        let signature: String = hmac_sha256_raw(&k_signing, string_to_sign.as_bytes()).unwrap().iter().map(|b| format!("{b:02x}")).collect();

        assert_eq!(signature, "b27ccfbfa7df52a200ff74193ca6e32d4b48b8856fab7ebf1c595d0670a7e470", "no matchea el vector oficial de AWS (get-vanilla)");
    }

    /// `aws_uri_encode`: los caracteres "sin reservar" (`A-Za-z0-9-._~`)
    /// pasan tal cual -- confirmado contra el string EXACTO del vector
    /// oficial `get-vanilla-query-unreserved` del `aws4_testsuite` de AWS,
    /// que existe justamente para fijar cuáles caracteres NO se codifican.
    /// El resto de los casos (espacio, `/` con y sin `encode_slash`, un
    /// caracter reservado cualquiera) verifican la regla escrita de la
    /// documentación de AWS (%XX en hex MAYÚSCULA) donde no hay un vector
    /// público más específico para citar.
    #[test]
    fn aws_uri_encode_matches_the_official_aws_unreserved_character_vector() {
        let unreserved = "-._~0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
        assert_eq!(aws_uri_encode(unreserved, true), unreserved);
        assert_eq!(aws_uri_encode(unreserved, false), unreserved);

        assert_eq!(aws_uri_encode("a b", true), "a%20b");
        assert_eq!(aws_uri_encode("a/b", true), "a%2Fb", "en un valor de query, '/' SÍ se codifica");
        assert_eq!(aws_uri_encode("a/b", false), "a/b", "en el path del objeto, '/' se preserva");
        assert_eq!(aws_uri_encode("AKIDEXAMPLE/20110909/us-east-1/host/aws4_request", true), "AKIDEXAMPLE%2F20110909%2Fus-east-1%2Fhost%2Faws4_request");
    }

    /// El builtin completo, de punta a punta contra un servidor real -- el
    /// timestamp que arma internamente (`SystemTime::now()`) hace que un
    /// match byte a byte contra un vector fijo sea imposible (no hay forma
    /// de inyectar el reloj), así que esto verifica la ESTRUCTURA exacta
    /// que AWS exige: host virtual-hosted-style, los cinco parámetros
    /// `X-Amz-*` (en el orden alfabético que S3 espera), y una firma final
    /// de 64 caracteres hexadecimales. El caso real que lo motiva:
    /// `DocumentStorageService` de un adoptador tenía una firma FALSA
    /// (`?signature=hmac_verified`, un literal) porque `crypto.hmacSha256`
    /// (String -> String) no alcanza para el encadenado de bytes crudos
    /// que SigV4 exige -- ver GRAMMAR.md §3.110.
    #[test]
    fn aws_s3_presigned_url_has_the_exact_shape_s3_requires() {
        let program = program_from(
            r#"
            service Docs {
                rpc share() -> String {
                    crypto.awsS3PresignedUrl("AKIDEXAMPLE", "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY", "us-east-1", "mi-bucket", "facturas/2026/factura-42.pdf", 3600)
                }
            }
        "#,
        );
        let db = Db::seeded();
        let url = invoke_rpc(&program, "Docs", "share", &json!({}), &db).unwrap();
        let url = url.as_str().unwrap();

        assert!(url.starts_with("https://mi-bucket.s3.us-east-1.amazonaws.com/facturas/2026/factura-42.pdf?"), "{url}");
        assert!(url.contains("X-Amz-Algorithm=AWS4-HMAC-SHA256"), "{url}");
        assert!(url.contains("X-Amz-Credential=AKIDEXAMPLE%2F"), "{url}");
        assert!(url.contains("X-Amz-Date="), "{url}");
        assert!(url.contains("X-Amz-Expires=3600"), "{url}");
        assert!(url.contains("X-Amz-SignedHeaders=host"), "{url}");
        let sig = url.split("X-Amz-Signature=").nth(1).expect("la URL tiene que terminar con la firma");
        assert_eq!(sig.len(), 64, "la firma es un SHA-256 en hex: {sig}");
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()), "hex en minúscula: {sig}");
    }

    #[test]
    fn aws_s3_presigned_url_rejects_an_out_of_range_expiry() {
        let program = program_from(
            r#"
            service Docs {
                rpc share(seconds: Int) -> String {
                    crypto.awsS3PresignedUrl("AKID", "secret", "us-east-1", "b", "k", seconds)
                }
            }
        "#,
        );
        let db = Db::seeded();
        for bad in [0, -1, 604_801] {
            invoke_rpc(&program, "Docs", "share", &json!({"seconds": bad}), &db).expect_err(&format!("{bad} segundos debería rechazarse"));
        }
        // El máximo permitido (7 días) SÍ funciona.
        assert!(invoke_rpc(&program, "Docs", "share", &json!({"seconds": 604_800}), &db).is_ok());
    }

    // ---- `crypto.awsS3PresignedUploadUrl` (GRAMMAR.md §3.194) ----

    /// Mismo espíritu que `aws_s3_presigned_url_has_the_exact_shape_s3_requires`
    /// (arriba) -- estructura exacta, no un vector fijo (el timestamp interno
    /// lo impide) -- más las dos diferencias reales de la variante de subida:
    /// método `PUT` (nunca aparece firmado en la URL en sí, pero si el método
    /// firmado estuviera mal, S3 rechazaría CUALQUIER PUT real con 403 -- acá
    /// se confirma indirectamente reconstruyendo el `canonical_request` con
    /// las mismas piezas ya verificadas contra el vector oficial de AWS) y
    /// `Content-Type` como segundo header firmado, en orden alfabético antes
    /// de `host`.
    #[test]
    fn aws_s3_presigned_upload_url_has_the_exact_shape_s3_requires() {
        let program = program_from(
            r#"
            service Docs {
                rpc uploadUrl() -> String {
                    crypto.awsS3PresignedUploadUrl("AKIDEXAMPLE", "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY", "us-east-1", "mi-bucket", "facturas/2026/factura-42.pdf", 3600, "application/pdf")
                }
            }
        "#,
        );
        let db = Db::seeded();
        let url = invoke_rpc(&program, "Docs", "uploadUrl", &json!({}), &db).unwrap();
        let url = url.as_str().unwrap();

        assert!(url.starts_with("https://mi-bucket.s3.us-east-1.amazonaws.com/facturas/2026/factura-42.pdf?"), "{url}");
        assert!(url.contains("X-Amz-Algorithm=AWS4-HMAC-SHA256"), "{url}");
        assert!(url.contains("X-Amz-Credential=AKIDEXAMPLE%2F"), "{url}");
        assert!(url.contains("X-Amz-Date="), "{url}");
        assert!(url.contains("X-Amz-Expires=3600"), "{url}");
        assert!(url.contains("X-Amz-SignedHeaders=content-type%3Bhost"), "content-type tiene que ir firmado, antes que host: {url}");
        let sig = url.split("X-Amz-Signature=").nth(1).expect("la URL tiene que terminar con la firma");
        assert_eq!(sig.len(), 64, "la firma es un SHA-256 en hex: {sig}");
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()), "hex en minúscula: {sig}");
    }

    /// El `Content-Type` firmado tiene que cambiar la firma resultante --
    /// si no la cambiara, el header no estaría realmente atado a la URL y
    /// cualquiera podría subir con un Content-Type distinto igual.
    #[test]
    fn aws_s3_presigned_upload_url_signature_depends_on_content_type() {
        let program = program_from(
            r#"
            service Docs {
                rpc uploadUrl(contentType: String) -> String {
                    crypto.awsS3PresignedUploadUrl("AKIDEXAMPLE", "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY", "us-east-1", "mi-bucket", "k.pdf", 3600, contentType)
                }
            }
        "#,
        );
        let db = Db::seeded();
        let url_pdf = invoke_rpc(&program, "Docs", "uploadUrl", &json!({"contentType": "application/pdf"}), &db).unwrap();
        let url_png = invoke_rpc(&program, "Docs", "uploadUrl", &json!({"contentType": "image/png"}), &db).unwrap();
        let sig_pdf = url_pdf.as_str().unwrap().split("X-Amz-Signature=").nth(1).unwrap();
        let sig_png = url_png.as_str().unwrap().split("X-Amz-Signature=").nth(1).unwrap();
        assert_ne!(sig_pdf, sig_png, "un Content-Type distinto tiene que dar una firma distinta");
    }

    #[test]
    fn aws_s3_presigned_upload_url_rejects_an_out_of_range_expiry() {
        let program = program_from(
            r#"
            service Docs {
                rpc uploadUrl(seconds: Int) -> String {
                    crypto.awsS3PresignedUploadUrl("AKID", "secret", "us-east-1", "b", "k", seconds, "application/pdf")
                }
            }
        "#,
        );
        let db = Db::seeded();
        for bad in [0, -1, 604_801] {
            invoke_rpc(&program, "Docs", "uploadUrl", &json!({"seconds": bad}), &db).expect_err(&format!("{bad} segundos debería rechazarse"));
        }
        assert!(invoke_rpc(&program, "Docs", "uploadUrl", &json!({"seconds": 604_800}), &db).is_ok());
    }

    // ---- `response.redirect` (GRAMMAR.md §3.111) ----

    /// El status/Location reales que `response.redirect` deja armados solo
    /// se observan del lado de `server.rs` (headers HTTP de verdad, ver
    /// `cli_content_type.rs`) -- `invoke_rpc` (este módulo) no tiene forma
    /// de leerlos, solo el valor JSON de retorno. Lo que SÍ se puede probar
    /// acá es la validación de `url`: un string vacío o con un salto de
    /// línea (inyección de headers HTTP) tiene que fallar ANTES de guardar
    /// nada en el override, sin importar qué HTTP server lo consuma después.
    #[test]
    fn redirect_rejects_an_empty_or_newline_containing_url() {
        let program = program_from(
            r#"
            service Web {
                rpc go(url: String) -> Void { response.redirect(url, false) }
            }
        "#,
        );
        let db = Db::seeded();
        for bad in ["", "/a\r\nX-Injected: yes", "/a\nSet-Cookie: evil=1"] {
            invoke_rpc(&program, "Web", "go", &json!({"url": bad}), &db).expect_err(&format!("{bad:?} debería rechazarse"));
        }
        assert!(invoke_rpc(&program, "Web", "go", &json!({"url": "/ok"}), &db).is_ok());
    }

    // ---- `base64.encode`/`base64.decode` (GRAMMAR.md §3.112) ----

    /// `base64.encode`/`base64.decode` ya EXISTÍAN en el runtime antes de
    /// esta ronda (`checker.rs`/`runtime/mod.rs`, base64 estándar RFC 4648
    /// con padding vía la crate `base64`) pero sin un solo test que fijara
    /// su comportamiento -- esta es la primera prueba real de que la
    /// implementación existente hace lo que dice. Vector conocido
    /// (`"hello"` <-> `"aGVsbG8="`), no inventado a mano, mismo criterio
    /// que `crypto.hmacSha256` verificado contra un vector de Python.
    #[test]
    fn base64_encode_and_decode_round_trip_a_known_vector() {
        let program = program_from(
            r#"
            service Codec {
                rpc enc(s: String) -> String { base64.encode(s) }
                rpc dec(s: String) -> String { base64.decode(s) }
            }
        "#,
        );
        let db = Db::seeded();
        assert_eq!(invoke_rpc(&program, "Codec", "enc", &json!({"s": "hello"}), &db).unwrap(), json!("aGVsbG8="));
        assert_eq!(invoke_rpc(&program, "Codec", "dec", &json!({"s": "aGVsbG8="}), &db).unwrap(), json!("hello"));
        // Caso real que motiva documentar esto: `Authorization: Basic
        // base64(sid:token)`, el esquema de auth de Twilio y de cualquier
        // API con HTTP Basic Auth -- ningún caso especial, la misma
        // función de siempre sobre un string con ":" adentro.
        assert_eq!(
            invoke_rpc(&program, "Codec", "enc", &json!({"s": "ACxxxx:authtoken123"}), &db).unwrap(),
            json!("QUN4eHh4OmF1dGh0b2tlbjEyMw==")
        );
    }

    #[test]
    fn base64_decode_rejects_malformed_input_and_non_utf8_output() {
        let program = program_from(
            r#"
            service Codec {
                rpc dec(s: String) -> String { base64.decode(s) }
            }
        "#,
        );
        let db = Db::seeded();
        // Ni base64 válido en absoluto (padding/alfabeto mal formado)...
        invoke_rpc(&program, "Codec", "dec", &json!({"s": "no es base64!!"}), &db).expect_err("debería rechazarse");
        // ...ni base64 válido que decodifica a bytes que NO son UTF-8 --
        // `base64.decode` devuelve `String`, así que esto es un error
        // limpio, no bytes corruptos silenciosos. 0xFF 0xFE no es UTF-8 válido.
        invoke_rpc(&program, "Codec", "dec", &json!({"s": "//4="}), &db).expect_err("bytes no-UTF8 deberían rechazarse");
    }

    // ---- `sitemapXml`/`robotsTxt` (GRAMMAR.md §3.116) ----

    #[test]
    fn sitemap_xml_builds_a_well_formed_sitemap_with_and_without_lastmod() {
        let program = program_from(
            r#"
            type Page = { loc: String, lastmod?: Timestamp }
            service Site {
                rpc sitemap() -> String {
                    sitemapXml([
                        Page { loc: "https://x.com/", lastmod: dateFromParts(2026, 8, 25, 0, 0, 0) },
                        Page { loc: "https://x.com/about" },
                    ])
                }
            }
        "#,
        );
        let db = Db::seeded();
        let xml = invoke_rpc(&program, "Site", "sitemap", &json!({}), &db).unwrap();
        let xml = xml.as_str().unwrap();
        assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n"), "{xml}");
        assert!(xml.contains("<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">"), "{xml}");
        assert!(xml.contains("<loc>https://x.com/</loc>"), "{xml}");
        assert!(xml.contains("<lastmod>2026-08-25T00:00:00.000Z</lastmod>"), "{xml}");
        // La segunda entrada NO tiene lastmod -- su <url> no debe llevar
        // ningún <lastmod> (ni vacío, ni heredado de la entrada anterior).
        let (_, second_url) = xml.rsplit_once("https://x.com/about").unwrap();
        assert!(!second_url.contains("<lastmod>"), "{xml}");
        assert!(xml.trim_end().ends_with("</urlset>"), "{xml}");
    }

    #[test]
    fn sitemap_xml_escapes_a_loc_with_special_characters() {
        let program = program_from(
            r#"
            type Page = { loc: String, lastmod?: Timestamp }
            service Site {
                rpc sitemap(loc: String) -> String { sitemapXml([Page { loc: loc }]) }
            }
        "#,
        );
        let db = Db::seeded();
        let xml = invoke_rpc(&program, "Site", "sitemap", &json!({"loc": "https://x.com/a&b?c=<d>"}), &db).unwrap();
        let xml = xml.as_str().unwrap();
        assert!(xml.contains("<loc>https://x.com/a&amp;b?c=&lt;d&gt;</loc>"), "{xml}");
        assert!(!xml.contains("c=<d>"), "el '<'/'>' crudo no debe llegar sin escapar: {xml}");
    }

    #[test]
    fn sitemap_xml_on_an_empty_list_is_a_valid_empty_urlset() {
        let program = program_from(
            r#"
            type Page = { loc: String, lastmod?: Timestamp }
            service Site {
                rpc sitemap() -> String { sitemapXml([]) }
            }
        "#,
        );
        let db = Db::seeded();
        let xml = invoke_rpc(&program, "Site", "sitemap", &json!({}), &db).unwrap();
        assert_eq!(
            xml.as_str().unwrap(),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n</urlset>"
        );
    }

    #[test]
    fn robots_txt_builds_blocks_with_disallow_and_allow_in_order_plus_a_trailing_sitemap() {
        let program = program_from(
            r#"
            type Rule = { userAgent: String, disallow?: String[], allow?: String[] }
            service Site {
                rpc robots() -> String {
                    robotsTxt([
                        Rule { userAgent: "GPTBot", disallow: ["/"] },
                        Rule { userAgent: "*", allow: ["/"], disallow: ["/admin"] },
                    ], "https://x.com/sitemap.xml")
                }
            }
        "#,
        );
        let db = Db::seeded();
        let txt = invoke_rpc(&program, "Site", "robots", &json!({}), &db).unwrap();
        assert_eq!(
            txt.as_str().unwrap(),
            "User-agent: GPTBot\nDisallow: /\n\nUser-agent: *\nDisallow: /admin\nAllow: /\n\nSitemap: https://x.com/sitemap.xml"
        );
    }

    #[test]
    fn robots_txt_without_a_sitemap_url_and_without_disallow_or_allow_omits_both() {
        let program = program_from(
            r#"
            type Rule = { userAgent: String, disallow?: String[], allow?: String[] }
            service Site {
                rpc robots() -> String {
                    robotsTxt([Rule { userAgent: "*" }], null)
                }
            }
        "#,
        );
        let db = Db::seeded();
        let txt = invoke_rpc(&program, "Site", "robots", &json!({}), &db).unwrap();
        // Ni Disallow/Allow (ninguno de los dos se pasó) ni "Sitemap:" (se
        // pasó null) -- un bloque de user-agent solo, nada inventado.
        assert_eq!(txt.as_str().unwrap(), "User-agent: *");
    }

    // ---- `metaTags`/`openGraphTags`/`canonicalLink`/`jsonLd` (GRAMMAR.md §3.117) ----

    #[test]
    fn meta_tags_builds_one_line_per_entry_and_escapes_content() {
        let program = program_from(
            r#"
            type Meta = { name: String, content: String }
            service Site {
                rpc head() -> String {
                    metaTags([
                        Meta { name: "description", content: "Tienda de \"regalos\" & más" },
                        Meta { name: "robots", content: "index, follow" },
                    ])
                }
            }
        "#,
        );
        let db = Db::seeded();
        let html = invoke_rpc(&program, "Site", "head", &json!({}), &db).unwrap();
        assert_eq!(
            html.as_str().unwrap(),
            "<meta name=\"description\" content=\"Tienda de &quot;regalos&quot; &amp; más\">\n<meta name=\"robots\" content=\"index, follow\">"
        );
    }

    #[test]
    fn meta_tags_on_an_empty_list_is_an_empty_string() {
        let program = program_from(
            r#"
            type Meta = { name: String, content: String }
            service Site {
                rpc head() -> String { metaTags([]) }
            }
        "#,
        );
        let db = Db::seeded();
        let html = invoke_rpc(&program, "Site", "head", &json!({}), &db).unwrap();
        assert_eq!(html.as_str().unwrap(), "");
    }

    #[test]
    fn open_graph_tags_uses_property_instead_of_name() {
        let program = program_from(
            r#"
            type Og = { property: String, content: String }
            service Site {
                rpc head() -> String {
                    openGraphTags([
                        Og { property: "og:title", content: "Mi producto" },
                        Og { property: "og:image", content: "https://x.com/foto.jpg" },
                    ])
                }
            }
        "#,
        );
        let db = Db::seeded();
        let html = invoke_rpc(&program, "Site", "head", &json!({}), &db).unwrap();
        assert_eq!(
            html.as_str().unwrap(),
            "<meta property=\"og:title\" content=\"Mi producto\">\n<meta property=\"og:image\" content=\"https://x.com/foto.jpg\">"
        );
    }

    #[test]
    fn canonical_link_escapes_the_url() {
        let program = program_from(
            r#"
            service Site {
                rpc head() -> String { canonicalLink("https://x.com/a?b=1&c=2") }
            }
        "#,
        );
        let db = Db::seeded();
        let html = invoke_rpc(&program, "Site", "head", &json!({}), &db).unwrap();
        assert_eq!(html.as_str().unwrap(), "<link rel=\"canonical\" href=\"https://x.com/a?b=1&amp;c=2\">");
    }

    #[test]
    fn json_ld_serializes_dynamic_data_and_escapes_a_script_close_tag() {
        let program = program_from(
            r#"
            service Site {
                rpc head() -> String { jsonLd(json.parse("{\"name\": \"</script><script>alert(1)</script>\"}")) }
            }
        "#,
        );
        let db = Db::seeded();
        let html = invoke_rpc(&program, "Site", "head", &json!({}), &db).unwrap();
        let out = html.as_str().unwrap();
        assert!(out.starts_with("<script type=\"application/ld+json\">"));
        assert!(out.ends_with("</script>"));
        // El JSON serializado en el MEDIO nunca contiene un '<' literal --
        // cada uno salió como <, así que ningún </script> puede
        // aparecer ahí adentro y cortar el bloque antes de tiempo.
        let inner = out.strip_prefix("<script type=\"application/ld+json\">").unwrap().strip_suffix("</script>").unwrap();
        assert!(!inner.contains('<'));
        assert!(inner.contains("\\u003c"));
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

    /// AUDIT-2026-08-27.md #3: `Type::PatchOf` (el decodificador de
    /// `Patch<T>`, usado por `applyPatch`) era el ÚNICO lugar que construye
    /// un struct a partir del wire sin llamar a `apply_field_validators` --
    /// un `@validate(email)` que `create` (que sí pasa por `Type::Struct`)
    /// rechazaba con 400 pasaba derecho por `update`/`applyPatch` y quedaba
    /// persistido tal cual. Confirmado en vivo contra un `linkc serve` real
    /// antes de este fix.
    #[test]
    fn validate_fires_on_applypatch_via_patch_of_t_not_just_on_the_full_struct() {
        let program = program_from(
            r#"
            type User = { id: Int, @validate(email) email: String, name: String }
            db { users: User[] }
            service S {
                rpc create(email: String, name: String) -> User {
                    db.users.insert(User { id: 0, email: email, name: name })
                }
                rpc update(id: Int, patch: Patch<User>) -> User {
                    db.users.applyPatch(id, patch)
                }
            }
        "#,
        );
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        let created = invoke_rpc(&program, "S", "create", &json!({"email": "a@b.com", "name": "Ada"}), &db).unwrap();
        let id = created["id"].as_i64().unwrap();

        // `create` (Type::Struct) ya rechazaba esto -- confirma que el
        // camino de siempre sigue andando.
        let e = invoke_rpc(&program, "S", "create", &json!({"email": "not-an-email", "name": "Bad"}), &db).unwrap_err();
        assert_eq!(e.kind, ErrorKind::BadRequest, "{e}");

        // El bug real: antes de este fix, esto daba 200 y persistía el
        // email inválido.
        let e = invoke_rpc(&program, "S", "update", &json!({"id": id, "patch": {"email": "not-an-email"}}), &db)
            .expect_err("un email inválido en el patch debe rechazarse igual que en create");
        assert_eq!(e.kind, ErrorKind::BadRequest, "{e}");
        assert!(e.message.contains("email"), "{e}");

        // Un patch que NO toca el campo con @validate sigue funcionando
        // normal -- apply_field_validators no vuelve requerido un campo
        // ausente del patch.
        let ok = invoke_rpc(&program, "S", "update", &json!({"id": id, "patch": {"name": "Ada Lovelace"}}), &db).unwrap();
        assert_eq!(ok["name"], json!("Ada Lovelace"));
        assert_eq!(ok["email"], json!("a@b.com"), "el email original no se tocó");

        // Y un patch CON un email válido sigue aplicando normal.
        let ok = invoke_rpc(&program, "S", "update", &json!({"id": id, "patch": {"email": "c@d.com"}}), &db).unwrap();
        assert_eq!(ok["email"], json!("c@d.com"));
    }

    // ---- `@check(...)` sobre un campo (GRAMMAR.md §3.96) ----

    /// El caso real que motiva `@check` (PLAN.md §9.3, citado por el
    /// usuario): `reviews.link` solo evitaba un rating fuera de 1-5 porque
    /// el código de la aplicación lo forzaba, sin ninguna barrera si algún
    /// día otro rpc insertaba sin pasar por esa validación. `@check(range,
    /// 1, 5)` mueve esa barrera al nivel del propio `insert`/`applyPatch`.
    #[test]
    fn check_range_rejects_a_value_outside_the_declared_bounds() {
        let program = program_from(
            r#"
            type Review = { id: Int, @check(range, 1, 5) rating: Int }
            db { reviews: Review[] }
            service S {
                rpc add(rating: Int) -> Review { db.reviews.insert(Review { id: 0, rating: rating }) }
            }
        "#,
        );
        let db = Db::new(&program, std::path::Path::new(":memory:"));

        let ok = invoke_rpc(&program, "S", "add", &json!({"rating": 3}), &db).unwrap();
        assert_eq!(ok["rating"], json!(3));

        let too_high = invoke_rpc(&program, "S", "add", &json!({"rating": 6}), &db).expect_err("6 está fuera de [1,5]");
        assert_eq!(too_high.kind, ErrorKind::BadRequest, "{too_high}");
        assert!(too_high.message.contains("Review.rating"), "{too_high}");

        let too_low = invoke_rpc(&program, "S", "add", &json!({"rating": 0}), &db).expect_err("0 está fuera de [1,5]");
        assert_eq!(too_low.kind, ErrorKind::BadRequest, "{too_low}");
    }

    /// GRAMMAR.md §3.173: `@check(<expr>)` de nivel type -- una comparación
    /// entre DOS campos rechazada del lado de la aplicación, mismo camino
    /// (`Expr::StructLit`) que ejercita `check_range_rejects_a_value_outside_the_declared_bounds`
    /// arriba, pero para el `@check` de tipo en vez del de un solo campo.
    #[test]
    fn type_level_check_comparing_two_fields_rejects_an_invalid_row() {
        let program = program_from(
            r#"
            @check(endDay > startDay)
            type Booking = { id: Int, startDay: Int, endDay: Int }
            db { bookings: Booking[] }
            service S {
                rpc add(startDay: Int, endDay: Int) -> Booking {
                    db.bookings.insert(Booking { id: 0, startDay: startDay, endDay: endDay })
                }
            }
        "#,
        );
        let db = Db::new(&program, std::path::Path::new(":memory:"));

        let ok = invoke_rpc(&program, "S", "add", &json!({"startDay": 1, "endDay": 5}), &db).unwrap();
        assert_eq!(ok["startDay"], json!(1));

        let bad = invoke_rpc(&program, "S", "add", &json!({"startDay": 5, "endDay": 5}), &db).expect_err("endDay == startDay no es > ");
        assert_eq!(bad.kind, ErrorKind::BadRequest, "{bad}");
        assert!(bad.message.contains("@check"), "{bad}");

        let bad2 = invoke_rpc(&program, "S", "add", &json!({"startDay": 9, "endDay": 3}), &db).expect_err("endDay < startDay");
        assert_eq!(bad2.kind, ErrorKind::BadRequest, "{bad2}");
    }

    /// GRAMMAR.md §3.173: lo mismo que el test de arriba, pero recibiendo
    /// el struct COMPLETO por el wire (`json_to_typed_value`, el otro punto
    /// de entrada de `apply_type_level_checks` -- un rpc que toma el tipo
    /// entero como parámetro, no uno que lo arma campo por campo adentro
    /// del cuerpo).
    #[test]
    fn type_level_check_rejects_an_invalid_row_received_whole_over_the_wire() {
        let program = program_from(
            r#"
            @check(endDay > startDay)
            type Booking = { id: Int, startDay: Int, endDay: Int }
            service S {
                rpc validate(b: Booking) -> Booking { b }
            }
        "#,
        );
        let db = Db::new(&program, std::path::Path::new(":memory:"));

        let ok = invoke_rpc(&program, "S", "validate", &json!({"b": {"id": 1, "startDay": 1, "endDay": 5}}), &db).unwrap();
        assert_eq!(ok["startDay"], json!(1));

        let bad = invoke_rpc(&program, "S", "validate", &json!({"b": {"id": 1, "startDay": 5, "endDay": 1}}), &db)
            .expect_err("endDay < startDay recibido por el wire tiene que rechazarse igual que construido en el cuerpo");
        assert_eq!(bad.kind, ErrorKind::BadRequest, "{bad}");
    }

    /// GRAMMAR.md §3.173: un `applyPatch` PARCIAL que no toca NINGUNO de
    /// los dos campos que `@check(...)` referencia tiene que pasar sin
    /// evaluar la expresión (mismo criterio de "ausente: nada que validar"
    /// que ya usa `@check` de un solo campo) -- ni falso rechazo, ni
    /// evaluarla contra un campo que no está.
    #[test]
    fn type_level_check_is_skipped_for_a_patch_that_does_not_touch_either_referenced_field() {
        let program = program_from(
            r#"
            @check(endDay > startDay)
            type Booking = { id: Int, room: String, startDay: Int, endDay: Int }
            type NewBooking = { room: String, startDay: Int, endDay: Int }
            db { bookings: Booking[] }
            service S {
                rpc add(room: String, startDay: Int, endDay: Int) -> Booking {
                    db.bookings.insert(NewBooking { room: room, startDay: startDay, endDay: endDay })
                }
                rpc renameRoom(id: Int, patch: Patch<Booking>) -> Booking {
                    db.bookings.applyPatch(id, patch)
                }
            }
        "#,
        );
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        let created = invoke_rpc(&program, "S", "add", &json!({"room": "A", "startDay": 1, "endDay": 5}), &db).unwrap();
        let id = created["id"].as_i64().unwrap();

        // Renombra la sala sin tocar startDay/endDay -- el patch parcial
        // (recibido por el wire, `Type::PatchOf`) no trae ninguno de los dos
        // campos que la expresión necesita, así que tiene que pasar sin
        // evaluarla, no rechazarse ni reventar.
        let patched = invoke_rpc(&program, "S", "renameRoom", &json!({"id": id, "patch": {"room": "B"}}), &db).unwrap();
        assert_eq!(patched["room"], json!("B"));
        assert_eq!(patched["startDay"], json!(1));
        assert_eq!(patched["endDay"], json!(5));
    }

    /// Complementa el test de arriba: un `applyPatch` que sí trae LOS DOS
    /// campos referenciados (aunque no traiga `room`) tiene que evaluarse de
    /// verdad -- confirma que "salteado si falta un campo" no se volvió
    /// "salteado siempre" por accidente.
    #[test]
    fn type_level_check_still_applies_to_a_patch_that_touches_both_referenced_fields() {
        let program = program_from(
            r#"
            @check(endDay > startDay)
            type Booking = { id: Int, room: String, startDay: Int, endDay: Int }
            type NewBooking = { room: String, startDay: Int, endDay: Int }
            db { bookings: Booking[] }
            service S {
                rpc add(room: String, startDay: Int, endDay: Int) -> Booking {
                    db.bookings.insert(NewBooking { room: room, startDay: startDay, endDay: endDay })
                }
                rpc reschedule(id: Int, patch: Patch<Booking>) -> Booking {
                    db.bookings.applyPatch(id, patch)
                }
            }
        "#,
        );
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        let created = invoke_rpc(&program, "S", "add", &json!({"room": "A", "startDay": 1, "endDay": 5}), &db).unwrap();
        let id = created["id"].as_i64().unwrap();

        let bad = invoke_rpc(&program, "S", "reschedule", &json!({"id": id, "patch": {"startDay": 9, "endDay": 3}}), &db)
            .expect_err("el patch trae los dos campos -- tiene que evaluarse y rechazarse");
        assert_eq!(bad.kind, ErrorKind::BadRequest, "{bad}");
    }

    #[test]
    fn check_min_and_max_reject_only_the_side_they_declare() {
        let program = program_from(
            r#"
            type Product = { id: Int, @check(min, 0) stock: Int, @check(max, 100.0) discountPercent: Float }
            db { products: Product[] }
            service S {
                rpc add(stock: Int, discountPercent: Float) -> Product {
                    db.products.insert(Product { id: 0, stock: stock, discountPercent: discountPercent })
                }
            }
        "#,
        );
        let db = Db::new(&program, std::path::Path::new(":memory:"));

        assert!(invoke_rpc(&program, "S", "add", &json!({"stock": 0, "discountPercent": 100.0}), &db).is_ok());
        let neg_stock = invoke_rpc(&program, "S", "add", &json!({"stock": -1, "discountPercent": 0.0}), &db).expect_err("-1 < min 0");
        assert_eq!(neg_stock.kind, ErrorKind::BadRequest, "{neg_stock}");
        let over_discount =
            invoke_rpc(&program, "S", "add", &json!({"stock": 5, "discountPercent": 100.5}), &db).expect_err("100.5 > max 100");
        assert_eq!(over_discount.kind, ErrorKind::BadRequest, "{over_discount}");
    }

    /// GRAMMAR.md §3.151: `db.vacuum()`/`db.tableStats()` corren de verdad
    /// contra SQLite (`:memory:`) -- `vacuum` no debe fallar, y `tableStats`
    /// tiene que reflejar filas insertadas REALES, no un valor inventado.
    #[test]
    fn db_vacuum_and_table_stats_run_for_real_against_sqlite() {
        let program = program_from(
            r#"
            type Item = { id: Int, name: String }
            db { items: Item[] }
            service Admin {
                rpc add(name: String) -> Item { db.items.insert(Item { id: 0, name: name }) }
                rpc doVacuum() -> Void { db.vacuum() }
                rpc stats() -> Map<String, Int> { db.tableStats() }
            }
        "#,
        );
        let db = Db::new(&program, std::path::Path::new(":memory:"));

        invoke_rpc(&program, "Admin", "add", &json!({"name": "a"}), &db).unwrap();
        invoke_rpc(&program, "Admin", "add", &json!({"name": "b"}), &db).unwrap();

        assert_eq!(invoke_rpc(&program, "Admin", "doVacuum", &json!({}), &db).unwrap(), json!(null));
        assert_eq!(invoke_rpc(&program, "Admin", "stats", &json!({}), &db).unwrap(), json!({"items": 2}));
    }

    /// El bug real que este test fija (encontrado en la verificación manual
    /// de esta ronda, antes de shippear): una colección REAL llamada
    /// "vacuum" tiene que seguir andando normal -- `db.vacuum.all()` (con
    /// MÁS field access después, a diferencia de `db.vacuum()` a secas) NO
    /// puede confundirse con el builtin de arriba, o esta colección legítima
    /// se vuelve inalcanzable en runtime aunque tipe bien.
    #[test]
    fn a_real_collection_named_vacuum_still_works_at_runtime() {
        let program = program_from(
            r#"
            type Item = { id: Int, name: String }
            db { vacuum: Item[] }
            service Admin {
                rpc add(name: String) -> Item { db.vacuum.insert(Item { id: 0, name: name }) }
                rpc all() -> Item[] { db.vacuum.all() }
            }
        "#,
        );
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        invoke_rpc(&program, "Admin", "add", &json!({"name": "x"}), &db).unwrap();
        let rows = invoke_rpc(&program, "Admin", "all", &json!({}), &db).unwrap();
        assert_eq!(rows.as_array().unwrap().len(), 1, "{rows:?}");
    }

    /// GRAMMAR.md §3.146: `@check(minLength/maxLength, ...)` sobre
    /// `String` -- mismo mecanismo de aplicación que `@check(min/max,
    /// ...)` numérico de arriba, contando CARACTERES Unicode (no bytes).
    #[test]
    fn check_min_length_rejects_an_empty_string() {
        let program = program_from(
            r#"
            type Post = { id: Int, @check(minLength, 1) title: String }
            db { posts: Post[] }
            service S {
                rpc add(title: String) -> Post { db.posts.insert(Post { id: 0, title: title }) }
            }
        "#,
        );
        let db = Db::new(&program, std::path::Path::new(":memory:"));

        assert!(invoke_rpc(&program, "S", "add", &json!({"title": "ok"}), &db).is_ok());
        let empty = invoke_rpc(&program, "S", "add", &json!({"title": ""}), &db).expect_err("'' tiene 0 caracteres, menos que el mínimo 1");
        assert_eq!(empty.kind, ErrorKind::BadRequest, "{empty}");
        assert!(empty.message.contains("Post.title"), "{empty}");
    }

    #[test]
    fn check_max_length_rejects_a_string_over_the_limit_counting_unicode_characters_not_bytes() {
        let program = program_from(
            r#"
            type Post = { id: Int, @check(maxLength, 3) title: String }
            db { posts: Post[] }
            service S {
                rpc add(title: String) -> Post { db.posts.insert(Post { id: 0, title: title }) }
            }
        "#,
        );
        let db = Db::new(&program, std::path::Path::new(":memory:"));

        // "café" son 4 caracteres Unicode pero 5 bytes UTF-8 ('é' ocupa 2) --
        // si esto contara bytes, "ók" (3 bytes, 2 caracteres) fallaría y
        // "café" recortado a 3 bytes cortaría la 'é' a la mitad. Contando
        // caracteres, "ók" (2) pasa el límite de 3 y "café" (4) no.
        assert!(invoke_rpc(&program, "S", "add", &json!({"title": "ók"}), &db).is_ok());
        let too_long = invoke_rpc(&program, "S", "add", &json!({"title": "café"}), &db).expect_err("4 caracteres > máximo 3");
        assert_eq!(too_long.kind, ErrorKind::BadRequest, "{too_long}");
    }

    /// Mismo motivo que `validate_fires_when_the_struct_is_constructed_inside_the_rpc_body_from_loose_params`:
    /// el caso REAL de un `insert` (armar un "New*"/struct completo con
    /// parámetros sueltos ADENTRO del cuerpo del rpc, nunca decodificado
    /// del wire como struct) tiene que disparar `@check` igual que el
    /// struct completo recibido como parámetro.
    #[test]
    fn check_fires_when_the_struct_is_constructed_inside_the_rpc_body_from_loose_params() {
        let program = program_from(
            r#"
            type Review = { id: Int, @check(range, 1, 5) rating: Int }
            type NewReview = { @check(range, 1, 5) rating: Int }
            db { reviews: Review[] }
            service S {
                rpc add(rating: Int) -> Review { db.reviews.insert(NewReview { rating: rating }) }
            }
        "#,
        );
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        let e = invoke_rpc(&program, "S", "add", &json!({"rating": 99}), &db).expect_err("99 fuera de [1,5]");
        assert_eq!(e.kind, ErrorKind::BadRequest, "{e}");
        assert!(e.message.contains("NewReview.rating"), "{e}");
    }

    /// `@check` no vuelve requerido un campo opcional -- un valor AUSENTE
    /// (o `Null`) no dispara ninguna violación, mismo criterio que
    /// `@validate` ya documenta para `String?`.
    #[test]
    fn check_on_an_optional_field_does_not_fire_when_the_value_is_absent() {
        let program = program_from(
            r#"
            type Review = { id: Int, @check(range, 1, 5) score: Int? }
            db { reviews: Review[] }
            service S {
                rpc add() -> Review { db.reviews.insert(Review { id: 0, score: null }) }
            }
        "#,
        );
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        let ok = invoke_rpc(&program, "S", "add", &json!({}), &db).unwrap();
        assert_eq!(ok["score"], serde_json::Value::Null);
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

    /// GRAMMAR.md §3.75, landmine del barrido de "límites honestos"
    /// (26/08/2026): un `matchFn` con forma NO pusheable (acá, `||`, que
    /// `recognize_pushable_predicate` no reconoce) tiene que seguir
    /// funcionando IGUAL que antes de esta ronda -- camino interpretado de
    /// siempre, sin ningún cambio de comportamiento observable, solo sin el
    /// atajo de SQL.
    #[test]
    fn upsert_with_a_non_pushable_match_fn_still_works_via_the_interpreted_fallback() {
        let program = program_from(
            r#"
            type Counter = { id: Int, name: String, count: Int }
            type NewCounter = { name: String, count: Int }
            db { counters: Counter[] }
            service S {
                rpc bump(name: String) -> Counter {
                    db.counters.upsert(
                        |c: Counter| { c.name == name || c.name == "alias" },
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
        assert_eq!(first["id"], second["id"], "misma fila, mismo id, aunque el predicado no sea pusheable");
        assert_eq!(second["count"], json!(2));
    }

    /// Bug real, encontrado por una auditoría multi-agente adversarial
    /// (26/08/2026) sobre el propio pushdown de conjunciones que reusan
    /// `upsert`/`findWhere`/`countWhere`/`deleteWhere`: `"campo" = ?` ligado
    /// a un parámetro NULL nunca es cierto en SQL (NULL no es igual a nada),
    /// pero el camino INTERPRETADO trata `Value::Null == Value::Null` como
    /// `true` -- así que un `matchFn = |c: T| { c.opcional == variable }`
    /// donde `variable` resulta `null` en runtime encontraba la fila
    /// existente por el camino interpretado pero NO por el pusheado,
    /// insertando una fila duplicada en vez de actualizar. `conjunction_
    /// condition` (runtime/db.rs) ahora genera `IS NULL`/`IS NOT NULL` para
    /// una hoja `==`/`!=` cuyo operando resultó NULL, sin ningún parámetro
    /// ligado para esa hoja -- mismo resultado que el camino interpretado.
    #[test]
    fn upsert_pushdown_matches_an_existing_null_valued_optional_field_instead_of_duplicating_the_row() {
        let program = program_from(
            r#"
            type Item = { id: Int, name: String, note: String? }
            type NewItem = { name: String, note: String? }
            db { items: Item[] }
            service S {
                rpc upsertByNote(name: String, note: String?) -> Item {
                    db.items.upsert(
                        |c: Item| { c.note == note },
                        NewItem { name: name, note: note },
                        |c: Item| { NewItem { name: name, note: note } }
                    )
                }
            }
        "#,
        );
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        let first = invoke_rpc(&program, "S", "upsertByNote", &json!({"name": "first", "note": null}), &db).unwrap();
        let second = invoke_rpc(&program, "S", "upsertByNote", &json!({"name": "second", "note": null}), &db).unwrap();
        assert_eq!(first["id"], second["id"], "el segundo upsert con note=null debe ACTUALIZAR la misma fila, no insertar una nueva");
        assert_eq!(second["name"], json!("second"));

        // Regresión sobre el caso NO-null, que ya funcionaba: sigue andando
        // igual con el fix (una hoja no-NULL sigue ligando un parámetro
        // normal, ninguna fila con note distinto matchea por accidente).
        let third = invoke_rpc(&program, "S", "upsertByNote", &json!({"name": "third", "note": "real"}), &db).unwrap();
        assert_ne!(third["id"], second["id"], "un note real y distinto de null sigue insertando una fila nueva");
        let fourth = invoke_rpc(&program, "S", "upsertByNote", &json!({"name": "fourth", "note": "real"}), &db).unwrap();
        assert_eq!(third["id"], fourth["id"], "el mismo note real sigue actualizando la misma fila");
    }

    // ---- transacciones multi-escritura: `transaction { ... }` (GRAMMAR.md §3.154) ----

    const TXN_PROGRAM: &str = r#"
        type Order = { id: Int, productId: Int, qty: Int }
        type Stock = { id: Int, productId: Int, quantity: Int }
        db { orders: Order[], stock: Stock[] }
        service Shop {
            rpc seedStock(productId: Int, qty: Int) -> Stock {
                db.stock.insert(Stock { id: 0, productId: productId, quantity: qty })
            }
            rpc checkout(productId: Int, qty: Int) -> Order {
                transaction {
                    let matches = db.stock.findWhere(|s: Stock| { s.productId == productId });
                    if matches.length() == 0 {
                        panic("sin stock para ese producto");
                    } else {
                    }
                    let s = matches[0];
                    if s.quantity < qty {
                        panic("stock insuficiente");
                    } else {
                    }
                    db.stock.increment(s.id, |x: Stock| { x.quantity }, 0 - qty);
                    db.orders.insert(Order { id: 0, productId: productId, qty: qty })
                }
            }
            rpc orderCount() -> Int { db.orders.count() }
            rpc stockFor(productId: Int) -> Int {
                let matches = db.stock.findWhere(|s: Stock| { s.productId == productId });
                matches[0].quantity
            }
        }
    "#;

    #[test]
    fn a_successful_transaction_commits_every_write_inside_it() {
        let program = program_from(TXN_PROGRAM);
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        invoke_rpc(&program, "Shop", "seedStock", &json!({"productId": 1, "qty": 10}), &db).unwrap();
        let order = invoke_rpc(&program, "Shop", "checkout", &json!({"productId": 1, "qty": 3}), &db).unwrap();
        assert_eq!(order["qty"], json!(3), "{order:?}");
        let stock = invoke_rpc(&program, "Shop", "stockFor", &json!({"productId": 1}), &db).unwrap();
        assert_eq!(stock, json!(7), "el stock tiene que reflejar el descuento real");
        let count = invoke_rpc(&program, "Shop", "orderCount", &json!({}), &db).unwrap();
        assert_eq!(count, json!(1));
    }

    /// El caso real que motiva todo el ítem: un `panic` a MITAD del bloque
    /// (después de que `increment` YA corrió, si el checkout llegara a
    /// intentarlo) tiene que deshacer TODO lo que la transacción alcanzó a
    /// escribir -- acá el panic dispara ANTES del `increment`/`insert`
    /// (stock insuficiente), así que ninguno de los dos debe haber pasado.
    #[test]
    fn a_panic_inside_a_transaction_rolls_back_every_write_attempted_before_it() {
        let program = program_from(TXN_PROGRAM);
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        invoke_rpc(&program, "Shop", "seedStock", &json!({"productId": 2, "qty": 5}), &db).unwrap();
        let result = invoke_rpc(&program, "Shop", "checkout", &json!({"productId": 2, "qty": 999}), &db);
        assert!(result.is_err(), "stock insuficiente debe fallar, no devolver un pedido a medias");
        let stock = invoke_rpc(&program, "Shop", "stockFor", &json!({"productId": 2}), &db).unwrap();
        assert_eq!(stock, json!(5), "el stock NO debe haberse tocado -- el panic rollbackea todo");
        let count = invoke_rpc(&program, "Shop", "orderCount", &json!({}), &db).unwrap();
        assert_eq!(count, json!(0), "ningún pedido debe haberse insertado");
    }

    /// Después de un rollback, la MISMA base sigue perfectamente utilizable
    /// -- un checkout posterior con stock real tiene que funcionar normal,
    /// sin ningún rastro del intento fallido anterior (mismo id de stock,
    /// nada "atascado" en un estado de transacción a medio cerrar).
    #[test]
    fn the_database_stays_usable_normally_after_a_rolled_back_transaction() {
        let program = program_from(TXN_PROGRAM);
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        invoke_rpc(&program, "Shop", "seedStock", &json!({"productId": 3, "qty": 4}), &db).unwrap();
        assert!(invoke_rpc(&program, "Shop", "checkout", &json!({"productId": 3, "qty": 999}), &db).is_err());
        let order = invoke_rpc(&program, "Shop", "checkout", &json!({"productId": 3, "qty": 1}), &db).unwrap();
        assert_eq!(order["qty"], json!(1), "{order:?}");
        let stock = invoke_rpc(&program, "Shop", "stockFor", &json!({"productId": 3}), &db).unwrap();
        assert_eq!(stock, json!(3));
    }

    /// Bug real, encontrado por una auditoría multi-agente adversarial
    /// (26/08/2026): el checker SOLO rechaza el anidamiento SINTÁCTICO de
    /// `transaction` (un `transaction` escrito literalmente dentro de
    /// otro, en el mismo cuerpo de función) -- `in_transaction` no tiene
    /// visibilidad sobre lo que hace una `fn` auxiliar llamada desde
    /// adentro. Antes de esta ronda, alcanzar un `transaction` anidado a
    /// través de una llamada a otra función compilaba limpio y fallaba en
    /// RUNTIME con el error crudo del backend ("cannot start a transaction
    /// within a transaction"), sin ninguna pista útil. `Db::begin_
    /// transaction` ahora chequea ANTES de intentar el `BEGIN` real y da
    /// el mismo mensaje claro que el caso sintáctico -- el servidor sigue
    /// respondiendo con normalidad después (ninguna fila queda a medio
    /// escribir).
    #[test]
    fn a_transaction_reached_through_a_helper_function_call_is_a_clear_runtime_error_not_a_raw_backend_message() {
        let program = program_from(
            r#"
            type Item = { id: Int, name: String }
            db { items: Item[] }
            fn helper(name: String) -> Item {
                transaction { db.items.insert(Item { id: 0, name: name }) }
            }
            service S {
                rpc outer(name: String) -> Item {
                    transaction {
                        let x = helper(name);
                        db.items.insert(Item { id: 0, name: name });
                        x
                    }
                }
                rpc count() -> Int { db.items.all().length() }
            }
        "#,
        );
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        let err = invoke_rpc(&program, "S", "outer", &json!({"name": "x"}), &db).unwrap_err();
        assert!(err.message.contains("no admite anidamiento"), "{}", err.message);

        // La base sigue usable después -- ninguna fila quedó a medio
        // escribir, y una segunda llamada falla de la misma forma clara
        // (no corrompe estado ni deja la marca de "transacción abierta"
        // pegada para siempre).
        let count = invoke_rpc(&program, "S", "count", &json!({}), &db).unwrap();
        assert_eq!(count, json!(0), "{count:?}");
        let err2 = invoke_rpc(&program, "S", "outer", &json!({"name": "y"}), &db).unwrap_err();
        assert!(err2.message.contains("no admite anidamiento"), "{}", err2.message);
    }

    // ---- Pilar 1 del roadmap de concurrencia (26/08/2026, a partir del
    // pedido de skynet-d3): un hilo por request en `runtime/server.rs`,
    // `Db` ahora `Send + Sync`. Estos dos tests NO usan `linkc serve` --
    // usan hilos de SISTEMA OPERATIVO REALES (`std::thread::spawn`) sobre
    // un único `Arc<Db>` compartido, exactamente el mismo patrón de acceso
    // que `server.rs` usa contra requests concurrentes de verdad, pero sin
    // necesitar levantar un servidor HTTP real para probarlo en CI. La
    // verificación manual (con `linkc serve` real, `curl` concurrente, y
    // un endpoint HTTP local lento para probar que una pasarela de pago
    // lenta ya no bloquea a las demás requests) queda documentada en
    // GRAMMAR.md, no repetida acá.

    /// 40 hilos reales insertando a la vez sobre la MISMA colección --
    /// ninguna fila perdida, ningún id duplicado. El candado reentrante de
    /// la conexión (`Backend::execute`, `store.rs`) es lo que hace que esto
    /// sea correcto: cada `insert` se serializa brevemente contra los
    /// demás, sin que ninguno pise el trabajo de otro.
    #[test]
    fn forty_real_threads_inserting_concurrently_never_lose_a_row_or_duplicate_an_id() {
        let program = program_from(
            r#"
            type Item = { id: Int, name: String }
            db { items: Item[] }
            service S {
                rpc create(name: String) -> Item { db.items.insert(Item { id: 0, name: name }) }
                rpc all() -> Item[] { db.items.all() }
            }
        "#,
        );
        let db = std::sync::Arc::new(Db::new(&program, std::path::Path::new(":memory:")));
        let program = std::sync::Arc::new(program);
        let handles: Vec<_> = (0..40)
            .map(|i| {
                let db = std::sync::Arc::clone(&db);
                let program = std::sync::Arc::clone(&program);
                std::thread::spawn(move || {
                    invoke_rpc(&program, "S", "create", &json!({"name": format!("item-{i}")}), &db).unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let rows = invoke_rpc(&program, "S", "all", &json!({}), &db).unwrap();
        let arr = rows.as_array().unwrap();
        assert_eq!(arr.len(), 40, "no debe perderse ninguna fila: {arr:?}");
        let mut ids: Vec<i64> = arr.iter().map(|r| r["id"].as_i64().unwrap()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 40, "ningún id puede repetirse");
    }

    /// 40 hilos reales corriendo `transaction { increment(...) }` a la vez
    /// sobre la MISMA fila -- el resultado final tiene que ser EXACTAMENTE
    /// 40, ni menos (update perdido) ni más (dos transacciones
    /// entrelazadas escribiendo sobre la misma base de cálculo). Prueba
    /// directa de que `Db::with_exclusive_connection` sostiene el candado
    /// por TODA la transacción, no solo por cada operación suelta --
    /// mismo caso que rompería si `begin_transaction`/el cuerpo/`commit_
    /// transaction` lockearan por separado en vez de como una unidad.
    #[test]
    fn forty_real_threads_running_a_transaction_on_the_same_row_never_lose_an_update() {
        let program = program_from(
            r#"
            type Counter = { id: Int, hits: Int }
            db { counters: Counter[] }
            service S {
                rpc seed() -> Counter { db.counters.insert(Counter { id: 0, hits: 0 }) }
                rpc bump(id: Int) -> Counter {
                    transaction { db.counters.increment(id, |c: Counter| { c.hits }, 1) }
                }
                rpc get(id: Int) -> Counter? { db.counters.find(id) }
            }
        "#,
        );
        let db = std::sync::Arc::new(Db::new(&program, std::path::Path::new(":memory:")));
        let program = std::sync::Arc::new(program);
        let seeded = invoke_rpc(&program, "S", "seed", &json!({}), &db).unwrap();
        let id = seeded["id"].as_i64().unwrap();

        let handles: Vec<_> = (0..40)
            .map(|_| {
                let db = std::sync::Arc::clone(&db);
                let program = std::sync::Arc::clone(&program);
                std::thread::spawn(move || {
                    invoke_rpc(&program, "S", "bump", &json!({"id": id}), &db).unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let result = invoke_rpc(&program, "S", "get", &json!({"id": id}), &db).unwrap();
        assert_eq!(result["hits"], json!(40), "40 transacciones concurrentes tienen que dar exactamente 40, sin updates perdidos ni entrelazados: {result:?}");
    }

    /// Bug real, encontrado auditando la sección de arriba (no reportado
    /// externamente): `Db::subscribe` sacaba la foto y RECIÉN DESPUÉS se
    /// registraba como suscriptor, dos pasos separados sin ningún candado
    /// compartido con `publish`/`deliver_local` -- correcto cuando el
    /// servidor procesaba una request a la vez, pero con un hilo real por
    /// request (GRAMMAR.md §3.158) un `insert` de OTRO hilo podía commitear
    /// y publicar EXACTAMENTE en esa ventana: ni quedaba en la foto (ya
    /// tomada) ni llegaba por el canal (todavía sin registrar) -- una fila
    /// perdida en silencio. `std::sync::Barrier` fuerza a los dos hilos a
    /// arrancar en el mismo instante en CADA vuelta, maximizando las
    /// chances de pegarle a la ventana de la carrera; muchas vueltas en vez
    /// de una sola porque la ventana es angosta y no siempre se dispara.
    #[test]
    fn subscribing_concurrently_with_a_real_insert_never_loses_the_new_row() {
        for round in 0..300 {
            let program = program_from(
                r#"
                type Item = { id: Int, name: String }
                db { items: Item[] }
                service S {
                    rpc create(name: String) -> Item { db.items.insert(Item { id: 0, name: name }) }
                }
            "#,
            );
            let db = std::sync::Arc::new(Db::new(&program, std::path::Path::new(":memory:")));
            let program = std::sync::Arc::new(program);
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));

            let insert_handle = {
                let db = std::sync::Arc::clone(&db);
                let program = std::sync::Arc::clone(&program);
                let barrier = std::sync::Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    invoke_rpc(&program, "S", "create", &json!({"name": "x"}), &db).unwrap();
                })
            };
            let subscribe_handle = {
                let db = std::sync::Arc::clone(&db);
                let barrier = std::sync::Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    db.subscribe("items").unwrap()
                })
            };

            insert_handle.join().unwrap();
            let (snapshot, rx) = subscribe_handle.join().unwrap();

            // `try_recv` (no bloqueante), no `recv_timeout`: para cuando los
            // dos hilos ya hicieron `join()`, cualquier evento que exista
            // ya está sentado en el buffer del channel -- `deliver_local`
            // corre sincrónicamente dentro de `create`, antes de que
            // `invoke_rpc` devuelva. Esperar con timeout acá multiplicaría
            // por 300 vueltas sin necesidad.
            let mut seen_as_event = false;
            while let Ok(_ev) = rx.try_recv() {
                seen_as_event = true;
            }
            assert!(
                !snapshot.is_empty() || seen_as_event,
                "vuelta {round}: la fila insertada concurrentemente nunca llegó -- ni en la foto ni como evento (la carrera que rompía esto volvió)"
            );
        }
    }

    /// El fix del test de arriba (candado de `subscribers` sostenido
    /// durante `select_rows` en `subscribe`) casi introduce un deadlock
    /// nuevo: si `commit_transaction` entregara sus eventos diferidos
    /// DENTRO de `with_exclusive_connection` (como hacía antes de este
    /// mismo fix), un `transaction{}` confirmando (candado de conexión
    /// tomado, pidiendo el de `subscribers` para entregar) y un
    /// `subscribe()` concurrente a la MISMA colección (candado de
    /// `subscribers` tomado, pidiendo el de conexión para `select_rows`)
    /// se esperarían mutuamente para siempre -- orden de candados cruzado
    /// clásico. Este test no verifica ningún VALOR -- si el deadlock
    /// existiera, `.join()` nunca volvería y el test colgaría hasta que el
    /// runner lo mate por timeout, en vez de fallar con un mensaje. Que
    /// termine en tiempo razonable ES la prueba.
    #[test]
    fn a_transaction_committing_concurrently_with_a_subscribe_on_the_same_collection_never_deadlocks() {
        let program = program_from(
            r#"
            type Counter = { id: Int, hits: Int }
            db { counters: Counter[] }
            service S {
                rpc seed() -> Counter { db.counters.insert(Counter { id: 0, hits: 0 }) }
                rpc bump(id: Int) -> Counter {
                    transaction { db.counters.increment(id, |c: Counter| { c.hits }, 1) }
                }
            }
        "#,
        );
        let db = std::sync::Arc::new(Db::new(&program, std::path::Path::new(":memory:")));
        let program = std::sync::Arc::new(program);
        let seeded = invoke_rpc(&program, "S", "seed", &json!({}), &db).unwrap();
        let id = seeded["id"].as_i64().unwrap();

        for _round in 0..100 {
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
            let bump_handle = {
                let db = std::sync::Arc::clone(&db);
                let program = std::sync::Arc::clone(&program);
                let barrier = std::sync::Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    invoke_rpc(&program, "S", "bump", &json!({"id": id}), &db).unwrap();
                })
            };
            let subscribe_handle = {
                let db = std::sync::Arc::clone(&db);
                let barrier = std::sync::Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    db.subscribe("counters").unwrap();
                })
            };
            bump_handle.join().unwrap();
            subscribe_handle.join().unwrap();
        }
    }

    /// El MISMO deadlock que el test de arriba cubre para `transaction{}`,
    /// pero por el camino de `upsert` -- que desde la misma ronda también
    /// sostiene el candado de la conexión durante todo su cuerpo, y por lo
    /// tanto llega a `publish`/`deliver_local` (que pide `subscribers`) con
    /// la conexión ya tomada. Encontrado por una auditoría adversarial y
    /// REPRODUCIDO contra un `linkc serve` real antes de arreglarlo: el
    /// servidor quedaba vivo pero cualquier request que tocara la base
    /// colgaba para siempre (`ping` seguía respondiendo 200, `health` y
    /// `/metrics` no volvían nunca).
    ///
    /// Igual que su hermano: no verifica ningún VALOR -- si el deadlock
    /// existiera, `.join()` no volvería nunca y el test colgaría hasta que
    /// el runner lo mate. Que termine ES la prueba.
    #[test]
    fn an_upsert_publishing_concurrently_with_a_subscribe_on_the_same_collection_never_deadlocks() {
        let program = program_from(
            r#"
            type Item = { id: Int, key: String, hits: Int }
            db { items: Item[] }
            service S {
                rpc bump(k: String) -> Item {
                    db.items.upsert(
                        |x: Item| { x.key == k },
                        Item { id: 0, key: k, hits: 1 },
                        |x: Item| { Item { key: x.key, hits: x.hits + 1 } }
                    )
                }
            }
        "#,
        );
        let db = std::sync::Arc::new(Db::new(&program, std::path::Path::new(":memory:")));
        let program = std::sync::Arc::new(program);

        for _round in 0..100 {
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
            let bump_handle = {
                let db = std::sync::Arc::clone(&db);
                let program = std::sync::Arc::clone(&program);
                let barrier = std::sync::Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    invoke_rpc(&program, "S", "bump", &json!({"k": "k"}), &db).unwrap();
                })
            };
            let subscribe_handle = {
                let db = std::sync::Arc::clone(&db);
                let barrier = std::sync::Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    db.subscribe("items").unwrap();
                })
            };
            bump_handle.join().unwrap();
            subscribe_handle.join().unwrap();
        }
    }

    /// Otro bug real de la misma auditoría: `upsert` buscaba la fila
    /// existente y decidía insert-o-patch en dos pasos SEPARADOS, sin
    /// candado compartido -- ya documentado como no-atómico ENTRE
    /// instancias de `linkc serve` (GRAMMAR.md §3.44), pero con hilos
    /// reales la MISMA carrera se volvía posible DENTRO de un proceso: 20
    /// hilos corriendo `upsert(matchFn: |x| x.email == "a@b.com", ...)` a
    /// la vez tenían que dar UNA sola fila, no hasta 20 duplicadas. Fix:
    /// `upsert` entero corre bajo `with_exclusive_connection`.
    #[test]
    fn twenty_real_threads_upserting_on_the_same_match_fn_concurrently_never_duplicate_the_row() {
        let program = program_from(
            r#"
            type Profile = { id: Int, email: String, hits: Int }
            db { profiles: Profile[] }
            service S {
                rpc touch(email: String) -> Profile {
                    db.profiles.upsert(
                        |p: Profile| { p.email == email },
                        Profile { id: 0, email: email, hits: 1 },
                        |p: Profile| { Profile { email: p.email, hits: p.hits + 1 } }
                    )
                }
                rpc all() -> Profile[] { db.profiles.all() }
            }
        "#,
        );
        let db = std::sync::Arc::new(Db::new(&program, std::path::Path::new(":memory:")));
        let program = std::sync::Arc::new(program);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(20));
        let handles: Vec<_> = (0..20)
            .map(|_| {
                let db = std::sync::Arc::clone(&db);
                let program = std::sync::Arc::clone(&program);
                let barrier = std::sync::Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    invoke_rpc(&program, "S", "touch", &json!({"email": "a@b.com"}), &db).unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let rows = invoke_rpc(&program, "S", "all", &json!({}), &db).unwrap();
        let arr = rows.as_array().unwrap();
        assert_eq!(arr.len(), 1, "20 upserts concurrentes sobre el mismo matchFn tienen que dar UNA sola fila, no {}: {arr:?}", arr.len());
        assert_eq!(arr[0]["hits"], json!(20), "cada upsert que encontró la fila ya creada tenía que sumar 1: {:?}", arr[0]);
    }

    // ---- `@unique(campo1, campo2, ...)` a nivel de `type` (GRAMMAR.md §3.155) ----

    const COMPOSITE_UNIQUE_PROGRAM: &str = r#"
        @unique(profileId, slug)
        type Product = { id: Int, profileId: Int, slug: String, name: String }
        db { products: Product[] }
        service Products {
            rpc create(profileId: Int, slug: String, name: String) -> Product {
                db.products.insert(Product { id: 0, profileId: profileId, slug: slug, name: name })
            }
        }
    "#;

    /// El caso real que motiva el ítem: dos filas con el MISMO
    /// `(profileId, slug)` se rechazan, pero compartir SOLO uno de los dos
    /// campos con otra fila existente sigue siendo válido -- confirma que
    /// es un constraint COMPUESTO real (columna `(profileId, slug)` en el
    /// índice), no dos constraints de un solo campo por separado.
    #[test]
    fn a_composite_unique_constraint_is_enforced_for_real_against_sqlite() {
        let program = program_from(COMPOSITE_UNIQUE_PROGRAM);
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        invoke_rpc(&program, "Products", "create", &json!({"profileId": 1, "slug": "foo", "name": "A"}), &db).unwrap();

        let same_pair = invoke_rpc(&program, "Products", "create", &json!({"profileId": 1, "slug": "foo", "name": "B"}), &db);
        assert!(same_pair.is_err(), "el mismo (profileId, slug) debe rechazarse");

        let other_profile =
            invoke_rpc(&program, "Products", "create", &json!({"profileId": 2, "slug": "foo", "name": "C"}), &db).unwrap();
        assert_eq!(other_profile["profileId"], json!(2), "distinto profileId, mismo slug: tiene que aceptarse");

        let other_slug =
            invoke_rpc(&program, "Products", "create", &json!({"profileId": 1, "slug": "bar", "name": "D"}), &db).unwrap();
        assert_eq!(other_slug["slug"], json!("bar"), "mismo profileId, distinto slug: también tiene que aceptarse");
    }

    /// GRAMMAR.md §3.174: `@unique(...) where <expr>` -- el índice único
    /// compuesto se vuelve PARCIAL. Caso real motivador (citado desde el
    /// schema Drizzle de Glowapp): dos turnos con el mismo
    /// `(userId, appointmentDate, startTime)` chocan SOLO si ninguno está
    /// cancelado -- una vez cancelado, ese horario puede reusarse sin
    /// acumular filas basura.
    #[test]
    fn a_conditional_composite_unique_constraint_is_enforced_for_real_against_sqlite() {
        let program = program_from(
            r#"
            @unique(userId, appointmentDate, startTime) where status != "cancelled"
            type Appointment = { id: Int, userId: Int, appointmentDate: String, startTime: String, status: String }
            db { appointments: Appointment[] }
            service Appointments {
                rpc book(userId: Int, appointmentDate: String, startTime: String, status: String) -> Appointment {
                    db.appointments.insert(Appointment { id: 0, userId: userId, appointmentDate: appointmentDate, startTime: startTime, status: status })
                }
            }
        "#,
        );
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        invoke_rpc(
            &program,
            "Appointments",
            "book",
            &json!({"userId": 1, "appointmentDate": "2026-09-01", "startTime": "10:00", "status": "confirmed"}),
            &db,
        )
        .unwrap();

        // Mismo horario, todavía "confirmed" -- tiene que chocar.
        let clash = invoke_rpc(
            &program,
            "Appointments",
            "book",
            &json!({"userId": 1, "appointmentDate": "2026-09-01", "startTime": "10:00", "status": "confirmed"}),
            &db,
        );
        assert!(clash.is_err(), "el mismo horario, sin cancelar, tiene que rechazarse (400)");

        // Mismo horario, pero "cancelled" -- la fila existente NO participa
        // del índice parcial (la condición 'where' la excluye), así que
        // reusar el horario tiene que aceptarse.
        let reused = invoke_rpc(
            &program,
            "Appointments",
            "book",
            &json!({"userId": 1, "appointmentDate": "2026-09-01", "startTime": "10:00", "status": "cancelled"}),
            &db,
        );
        assert!(reused.is_ok(), "reusar un horario ya cancelado tiene que aceptarse -- el índice es parcial: {reused:?}");
    }

    /// Bug real, encontrado por una auditoría multi-agente adversarial
    /// (26/08/2026): el nombre de índice compuesto se armaba con
    /// `fields.join("_")`, ambiguo cuando un nombre de campo ya tenía un
    /// guion bajo -- `@unique(a_b, c)` y `@unique(a, b_c)` sobre el MISMO
    /// type generaban el mismo `idx_<t>_a_b_c`, así que `CREATE UNIQUE INDEX
    /// IF NOT EXISTS` volvía un no-op silencioso para el segundo -- su
    /// constraint nunca se creaba de verdad. `composite_unique_index_name`
    /// ahora codifica cada campo con prefijo de longitud, inyectivo por
    /// construcción.
    #[test]
    fn two_composite_unique_constraints_whose_joined_field_names_would_collide_both_enforce() {
        let program = program_from(
            r#"
            @unique(a_b, c)
            @unique(a, b_c)
            type T = { id: Int, a_b: Int, c: Int, a: Int, b_c: Int }
            db { ts: T[] }
            service Ts {
                rpc create(a_b: Int, c: Int, a: Int, b_c: Int) -> T {
                    db.ts.insert(T { id: 0, a_b: a_b, c: c, a: a, b_c: b_c })
                }
            }
        "#,
        );
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        invoke_rpc(&program, "Ts", "create", &json!({"a_b": 10, "c": 20, "a": 5, "b_c": 6}), &db).unwrap();

        let violates_second =
            invoke_rpc(&program, "Ts", "create", &json!({"a_b": 30, "c": 40, "a": 5, "b_c": 6}), &db);
        assert!(violates_second.is_err(), "el segundo @unique(a, b_c) debe rechazar (a=5, b_c=6) repetido, no ser un no-op silencioso");

        let violates_first =
            invoke_rpc(&program, "Ts", "create", &json!({"a_b": 10, "c": 20, "a": 99, "b_c": 98}), &db);
        assert!(violates_first.is_err(), "el primer @unique(a_b, c) sigue rechazando (a_b=10, c=20) repetido");
    }

    // ---- db.<c>.insertMany(items) (GRAMMAR.md §3.76) ----

    #[test]
    fn insert_many_inserts_every_item_and_returns_them_all_with_real_ids() {
        let program = program_from(
            r#"
            type Task = { id: Int, title: String }
            type NewTask = { title: String }
            db { tasks: Task[] }
            service S {
                rpc seed() -> Task[] {
                    db.tasks.insertMany([NewTask { title: "a" }, NewTask { title: "b" }, NewTask { title: "c" }])
                }
                rpc all() -> Task[] { db.tasks.all() }
            }
        "#,
        );
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        let result = invoke_rpc(&program, "S", "seed", &json!({}), &db).unwrap();
        let rows = result.as_array().unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["title"], json!("a"));
        assert_eq!(rows[2]["title"], json!("c"));
        // Ids reales asignados por la base, uno distinto por fila.
        let ids: Vec<i64> = rows.iter().map(|r| r["id"].as_i64().unwrap()).collect();
        assert_eq!(ids, vec![1, 2, 3]);

        // Confirma que quedaron persistidas de verdad, no solo devueltas.
        let persisted = invoke_rpc(&program, "S", "all", &json!({}), &db).unwrap();
        assert_eq!(persisted.as_array().unwrap().len(), 3);
    }

    // ---- createdAt/updatedAt automáticos: `= now()` + `@autoUpdate` (GRAMMAR.md §3.77) ----

    /// `createdAt: Timestamp = now()` (el default ya existente, GRAMMAR.md
    /// §3.74) alcanza para "asignado solo, sin tocarlo a mano en cada
    /// insert" -- sin ninguna anotación nueva.
    #[test]
    fn a_timestamp_field_defaulting_to_now_is_set_on_insert_without_being_given() {
        let program = program_from(
            r#"
            type Task = { id: Int, title: String, createdAt: Timestamp = now() }
            type NewTask = { title: String, createdAt: Timestamp = now() }
            db { tasks: Task[] }
            service S {
                rpc create(title: String) -> Task { db.tasks.insert(NewTask { title: title }) }
            }
        "#,
        );
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        let created = invoke_rpc(&program, "S", "create", &json!({"title": "a"}), &db).unwrap();
        assert!(created["createdAt"].is_string(), "{created}");
    }

    /// `@autoUpdate` pisa el campo a `now()` en CADA `applyPatch`, incluso
    /// cuando el patch enviado no lo menciona -- verificado contra un
    /// servidor real primero (curl), acá fijado como test.
    #[test]
    fn auto_update_bumps_the_field_on_apply_patch_even_when_the_patch_omits_it() {
        let program = program_from(
            r#"
            type Task = { id: Int, title: String, createdAt: Timestamp = now(), @autoUpdate updatedAt: Timestamp = now() }
            type NewTask = { title: String, createdAt: Timestamp = now(), @autoUpdate updatedAt: Timestamp = now() }
            db { tasks: Task[] }
            service S {
                rpc create(title: String) -> Task { db.tasks.insert(NewTask { title: title }) }
                rpc rename(id: Int, patch: Patch<Task>) -> Task { db.tasks.applyPatch(id, patch) }
            }
        "#,
        );
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        let created = invoke_rpc(&program, "S", "create", &json!({"title": "a"}), &db).unwrap();
        let id = created["id"].as_i64().unwrap();
        let created_at_before = created["createdAt"].clone();
        let updated_at_before = created["updatedAt"].clone();

        std::thread::sleep(std::time::Duration::from_millis(5));
        // El patch solo trae 'title' -- 'updatedAt' NO aparece acá.
        let renamed = invoke_rpc(&program, "S", "rename", &json!({"id": id, "patch": {"title": "b"}}), &db).unwrap();

        assert_eq!(renamed["createdAt"], created_at_before, "createdAt nunca cambia después del insert");
        assert_ne!(renamed["updatedAt"], updated_at_before, "updatedAt se pisó a pesar de no venir en el patch");
    }

    // ---- soft-delete nativo: `@softDelete` (GRAMMAR.md §3.78) ----

    fn soft_delete_program() -> crate::ast::Program {
        program_from(
            r#"
            type Task = { id: Int, title: String, @softDelete deletedAt: Timestamp? = null }
            type NewTask = { title: String, deletedAt: Timestamp? = null }
            db { tasks: Task[] }
            service S {
                rpc create(title: String) -> Task { db.tasks.insert(NewTask { title: title }) }
                rpc list() -> Task[] { db.tasks.all() }
                rpc remove(id: Int) -> Bool { db.tasks.delete(id) }
                rpc getById(id: Int) -> Task? { db.tasks.find(id) }
                rpc total() -> Int { db.tasks.count() }
            }
        "#,
        )
    }

    #[test]
    fn deleting_a_row_with_a_soft_delete_field_sets_it_instead_of_removing_the_row() {
        let program = soft_delete_program();
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        let created = invoke_rpc(&program, "S", "create", &json!({"title": "a"}), &db).unwrap();
        let id = created["id"].as_i64().unwrap();
        assert_eq!(created["deletedAt"], json!(null));

        let deleted = invoke_rpc(&program, "S", "remove", &json!({"id": id}), &db).unwrap();
        assert_eq!(deleted, json!(true));

        // La fila sigue existiendo -- `find` no filtra por soft-delete
        // (límite deliberado, ver GRAMMAR.md §3.78) -- pero ahora tiene
        // 'deletedAt' fijado.
        let fetched = invoke_rpc(&program, "S", "getById", &json!({"id": id}), &db).unwrap();
        assert!(!fetched["deletedAt"].is_null(), "{fetched}");
    }

    #[test]
    fn deleting_twice_is_idempotent_and_returns_false_the_second_time() {
        let program = soft_delete_program();
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        let created = invoke_rpc(&program, "S", "create", &json!({"title": "a"}), &db).unwrap();
        let id = created["id"].as_i64().unwrap();

        assert_eq!(invoke_rpc(&program, "S", "remove", &json!({"id": id}), &db).unwrap(), json!(true));
        assert_eq!(
            invoke_rpc(&program, "S", "remove", &json!({"id": id}), &db).unwrap(),
            json!(false),
            "una segunda vez sobre una fila ya borrada no debe re-tocarla"
        );
    }

    #[test]
    fn all_and_count_exclude_soft_deleted_rows() {
        let program = soft_delete_program();
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        let a = invoke_rpc(&program, "S", "create", &json!({"title": "a"}), &db).unwrap();
        let _b = invoke_rpc(&program, "S", "create", &json!({"title": "b"}), &db).unwrap();
        assert_eq!(invoke_rpc(&program, "S", "total", &json!({}), &db).unwrap(), json!(2));

        invoke_rpc(&program, "S", "remove", &json!({"id": a["id"]}), &db).unwrap();

        let list = invoke_rpc(&program, "S", "list", &json!({}), &db).unwrap();
        let titles: Vec<&str> = list.as_array().unwrap().iter().map(|r| r["title"].as_str().unwrap()).collect();
        assert_eq!(titles, vec!["b"], "'all()' no debe traer la fila borrada");
        assert_eq!(invoke_rpc(&program, "S", "total", &json!({}), &db).unwrap(), json!(1));
    }

    /// `findWhere`/`deleteWhere` reusan `all()` internamente (ver
    /// `call_method`) -- heredan el filtro gratis, sin ningún caso especial
    /// propio para soft-delete.
    #[test]
    fn find_where_and_delete_where_inherit_the_filter_via_all() {
        let program = program_from(
            r#"
            type Task = { id: Int, title: String, @softDelete deletedAt: Timestamp? = null }
            type NewTask = { title: String, deletedAt: Timestamp? = null }
            db { tasks: Task[] }
            service S {
                rpc create(title: String) -> Task { db.tasks.insert(NewTask { title: title }) }
                rpc remove(id: Int) -> Bool { db.tasks.delete(id) }
                rpc findAll() -> Task[] { db.tasks.findWhere(|t: Task| { true }) }
                rpc deleteAll() -> Int { db.tasks.deleteWhere(|t: Task| { true }) }
            }
        "#,
        );
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        let a = invoke_rpc(&program, "S", "create", &json!({"title": "a"}), &db).unwrap();
        invoke_rpc(&program, "S", "create", &json!({"title": "b"}), &db).unwrap();
        invoke_rpc(&program, "S", "remove", &json!({"id": a["id"]}), &db).unwrap();

        let found = invoke_rpc(&program, "S", "findAll", &json!({}), &db).unwrap();
        assert_eq!(found.as_array().unwrap().len(), 1, "findWhere no debe ver la fila ya soft-deleted");

        // deleteWhere(true) sobre lo que queda (solo "b") -- la ya borrada
        // ni siquiera entra al loop, así que el conteo devuelto es 1, no 2.
        let count = invoke_rpc(&program, "S", "deleteAll", &json!({}), &db).unwrap();
        assert_eq!(count, json!(1));
    }

    /// `page`/`pageAfter`/`sumBy` también filtran -- mismo criterio que
    /// `all()`/`count()`, no un caso especial de esos dos nada más.
    #[test]
    fn page_page_after_and_sum_by_exclude_soft_deleted_rows() {
        let program = program_from(
            r#"
            type Task = { id: Int, title: String, amount: Int, @softDelete deletedAt: Timestamp? = null }
            type NewTask = { title: String, amount: Int, deletedAt: Timestamp? = null }
            db { tasks: Task[] }
            service S {
                rpc create(title: String, amount: Int) -> Task { db.tasks.insert(NewTask { title: title, amount: amount }) }
                rpc remove(id: Int) -> Bool { db.tasks.delete(id) }
                rpc page() -> Task[] { db.tasks.page(10, 0) }
                rpc pageAfter() -> Task[] { db.tasks.pageAfter(null, 10) }
                rpc total() -> Int { db.tasks.sumBy(|t: Task| { t.title }, |t: Task| { t.amount }).length() }
            }
        "#,
        );
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        let a = invoke_rpc(&program, "S", "create", &json!({"title": "a", "amount": 10}), &db).unwrap();
        invoke_rpc(&program, "S", "create", &json!({"title": "b", "amount": 20}), &db).unwrap();
        invoke_rpc(&program, "S", "remove", &json!({"id": a["id"]}), &db).unwrap();

        assert_eq!(invoke_rpc(&program, "S", "page", &json!({}), &db).unwrap().as_array().unwrap().len(), 1);
        assert_eq!(invoke_rpc(&program, "S", "pageAfter", &json!({}), &db).unwrap().as_array().unwrap().len(), 1);
        // Un solo grupo restante ("b") -- si la fila borrada hubiera
        // sobrevivido al GROUP BY, habría 2.
        assert_eq!(invoke_rpc(&program, "S", "total", &json!({}), &db).unwrap(), json!(1));
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

    /// `dateFromParts` (GRAMMAR.md §3.90): el caso real que lo motivó -- un
    /// rpc que calcula el límite de un trimestre a partir de `año`/
    /// `trimestre`, algo imposible de escribir antes de esta ronda (§3.31
    /// documentaba "sin construcción desde código fuente" como límite
    /// deliberado).
    #[test]
    fn date_from_parts_builds_a_quarter_boundary_end_to_end() {
        let tokens = crate::lexer::tokenize(
            r#"
            service S {
                rpc quarterStart(year: Int, quarter: Int) -> Timestamp {
                    let month = (quarter - 1) * 3 + 1;
                    dateFromParts(year, month, 1, 0, 0, 0)
                }
            }
        "#,
        )
        .unwrap();
        let program = crate::parser::parse(tokens).unwrap();
        let db = Db::seeded();

        let q1 = invoke_rpc(&program, "S", "quarterStart", &json!({"year": 2026, "quarter": 1}), &db).unwrap();
        assert_eq!(q1, json!("2026-01-01T00:00:00.000Z"));

        let q3 = invoke_rpc(&program, "S", "quarterStart", &json!({"year": 2026, "quarter": 3}), &db).unwrap();
        assert_eq!(q3, json!("2026-07-01T00:00:00.000Z"));
    }

    #[test]
    fn date_from_parts_is_usable_as_a_first_class_value() {
        let tokens = crate::lexer::tokenize(
            r#"
            service S {
                rpc build() -> Timestamp {
                    let f = dateFromParts;
                    f(2026, 1, 1, 0, 0, 0)
                }
            }
        "#,
        )
        .unwrap();
        let program = crate::parser::parse(tokens).unwrap();
        let res = invoke_rpc(&program, "S", "build", &json!({}), &Db::seeded()).unwrap();
        assert_eq!(res, json!("2026-01-01T00:00:00.000Z"));
    }

    #[test]
    fn date_from_parts_rejects_an_invalid_calendar_date_as_a_bad_request() {
        let tokens = crate::lexer::tokenize(
            r#"
            service S {
                rpc bad() -> Timestamp { dateFromParts(2026, 2, 30, 0, 0, 0) }
            }
        "#,
        )
        .unwrap();
        let program = crate::parser::parse(tokens).unwrap();
        let e = invoke_rpc(&program, "S", "bad", &json!({}), &Db::seeded()).expect_err("30 de febrero no existe");
        assert_eq!(e.kind, ErrorKind::BadRequest, "una fecha inválida es error del CALLER, no del servidor: {e}");
        assert!(e.message.contains("2026-02-30"), "{e}");
    }

    // ---- GRAMMAR.md §3.196: aritmética de Timestamp ----

    #[test]
    fn timestamp_arithmetic_adds_the_exact_number_of_milliseconds_per_unit() {
        let program = program_from(
            r#"
            service S {
                rpc addMillis(t: Timestamp, n: Int64) -> Timestamp { t.addMillis(n) }
                rpc addSeconds(t: Timestamp, n: Int) -> Timestamp { t.addSeconds(n) }
                rpc addMinutes(t: Timestamp, n: Int) -> Timestamp { t.addMinutes(n) }
                rpc addHours(t: Timestamp, n: Int) -> Timestamp { t.addHours(n) }
                rpc addDays(t: Timestamp, n: Int) -> Timestamp { t.addDays(n) }
            }
        "#,
        );
        let db = Db::seeded();
        let base = "2026-01-01T00:00:00.000Z";
        assert_eq!(invoke_rpc(&program, "S", "addMillis", &json!({"t": base, "n": "500"}), &db).unwrap(), json!("2026-01-01T00:00:00.500Z"));
        assert_eq!(invoke_rpc(&program, "S", "addSeconds", &json!({"t": base, "n": 30}), &db).unwrap(), json!("2026-01-01T00:00:30.000Z"));
        assert_eq!(invoke_rpc(&program, "S", "addMinutes", &json!({"t": base, "n": 5}), &db).unwrap(), json!("2026-01-01T00:05:00.000Z"));
        assert_eq!(invoke_rpc(&program, "S", "addHours", &json!({"t": base, "n": 2}), &db).unwrap(), json!("2026-01-01T02:00:00.000Z"));
        assert_eq!(invoke_rpc(&program, "S", "addDays", &json!({"t": base, "n": 1}), &db).unwrap(), json!("2026-01-02T00:00:00.000Z"));
    }

    #[test]
    fn timestamp_arithmetic_with_a_negative_n_subtracts() {
        let program = program_from(
            r#"
            service S {
                rpc addMinutes(t: Timestamp, n: Int) -> Timestamp { t.addMinutes(n) }
            }
        "#,
        );
        let db = Db::seeded();
        let result = invoke_rpc(&program, "S", "addMinutes", &json!({"t": "2026-01-01T00:05:00.000Z", "n": -5}), &db).unwrap();
        assert_eq!(result, json!("2026-01-01T00:00:00.000Z"), "n negativo resta -- sin un método .subtract* separado");
    }

    /// Caso real reportado por un adoptador en producción (MyFinance):
    /// expiración de 5 minutos para un código OTP de 2FA, hoy imposible sin
    /// esto -- terminaban apoyándose solo en "de un solo uso".
    #[test]
    fn timestamp_arithmetic_solves_the_real_otp_expiry_use_case() {
        let program = program_from(
            r#"
            service S {
                rpc stillValid(issuedAt: Timestamp, checkedAt: Timestamp) -> Bool {
                    checkedAt < issuedAt.addMinutes(5)
                }
            }
        "#,
        );
        let db = Db::seeded();
        let issued = "2026-01-01T00:00:00.000Z";
        let within = invoke_rpc(&program, "S", "stillValid", &json!({"issuedAt": issued, "checkedAt": "2026-01-01T00:04:59.999Z"}), &db).unwrap();
        assert_eq!(within, json!(true), "4m59.999s después sigue vigente");
        let expired = invoke_rpc(&program, "S", "stillValid", &json!({"issuedAt": issued, "checkedAt": "2026-01-01T00:05:00.001Z"}), &db).unwrap();
        assert_eq!(expired, json!(false), "5m0.001s después ya venció");
    }

    #[test]
    fn timestamp_arithmetic_reports_a_clean_overflow_error_not_a_panic() {
        let program = program_from(
            r#"
            service S {
                rpc addDays(t: Timestamp, n: Int) -> Timestamp { t.addDays(n) }
            }
        "#,
        );
        let db = Db::seeded();
        let e = invoke_rpc(&program, "S", "addDays", &json!({"t": "2026-01-01T00:00:00.000Z", "n": i64::MAX}), &db)
            .expect_err("un n gigante tiene que desbordar limpio, no panickear");
        assert!(e.message.contains("desborde"), "{e}");
    }

    // ---- GRAMMAR.md §3.198: String.substring/replace/split/padStart/padEnd ----

    /// Caso no-ASCII real -- confirma indexado por CARACTER, no por byte, y
    /// que coincide con `.length()` sobre el mismo string (`length()` ya
    /// usa `chars().count()`; un `.substring()` indexado por byte
    /// discreparía en silencio acá).
    #[test]
    fn string_substring_indexes_by_character_not_by_byte() {
        let program = program_from(
            r#"
            service S {
                rpc slice(s: String, start: Int, end: Int) -> String { s.substring(start, end) }
                rpc len(s: String) -> Int { s.length() }
            }
        "#,
        );
        let db = Db::seeded();
        let s = "café niño"; // "é" y "ñ" son 2 bytes UTF-8 cada uno, 1 carácter cada uno
        let len = invoke_rpc(&program, "S", "len", &json!({"s": s}), &db).unwrap();
        assert_eq!(len, json!(9), "9 caracteres: c-a-f-é- -n-i-ñ-o");
        let result = invoke_rpc(&program, "S", "slice", &json!({"s": s, "start": 0, "end": 4}), &db).unwrap();
        assert_eq!(result, json!("café"), "corte por caracter, no por byte -- byte 4 caería a mitad de 'é'");
    }

    #[test]
    fn string_substring_rejects_each_out_of_range_case_cleanly() {
        let program = program_from(
            r#"
            service S {
                rpc slice(s: String, start: Int, end: Int) -> String { s.substring(start, end) }
            }
        "#,
        );
        let db = Db::seeded();
        invoke_rpc(&program, "S", "slice", &json!({"s": "abc", "start": -1, "end": 2}), &db).expect_err("start < 0");
        invoke_rpc(&program, "S", "slice", &json!({"s": "abc", "start": 0, "end": 4}), &db).expect_err("end > longitud");
        invoke_rpc(&program, "S", "slice", &json!({"s": "abc", "start": 2, "end": 1}), &db).expect_err("start > end");
        // Rango válido en el borde (todo el string) SÍ funciona.
        assert_eq!(invoke_rpc(&program, "S", "slice", &json!({"s": "abc", "start": 0, "end": 3}), &db).unwrap(), json!("abc"));
    }

    #[test]
    fn string_replace_replaces_every_occurrence() {
        let program = program_from(
            r#"
            service S {
                rpc r(s: String, target: String, replacement: String) -> String { s.replace(target, replacement) }
            }
        "#,
        );
        let db = Db::seeded();
        let result = invoke_rpc(&program, "S", "r", &json!({"s": "a;b;c;d", "target": ";", "replacement": ","}), &db).unwrap();
        assert_eq!(result, json!("a,b,c,d"), "TODAS las ocurrencias, no solo la primera");
    }

    #[test]
    fn string_split_matches_native_rust_semantics_including_the_empty_separator() {
        let program = program_from(
            r#"
            service S {
                rpc parts(s: String, sep: String) -> String[] { s.split(sep) }
            }
        "#,
        );
        let db = Db::seeded();
        let normal = invoke_rpc(&program, "S", "parts", &json!({"s": "a,b,c", "sep": ","}), &db).unwrap();
        assert_eq!(normal, json!(["a", "b", "c"]));
        // Separador vacío: comportamiento nativo de Rust, definido y
        // testeado -- no un panic, no un caso especial inventado.
        let empty_sep = invoke_rpc(&program, "S", "parts", &json!({"s": "abc", "sep": ""}), &db).unwrap();
        assert_eq!(empty_sep, json!(["", "a", "b", "c", ""]));
    }

    /// Los dos casos reales citados por un adoptador (MyFinance): sanear
    /// `;`/saltos de línea antes de unir con `;` (ContaPlus/XDIARIO), y
    /// padding fixed-width (A3 Contable).
    #[test]
    fn string_replace_and_pad_start_solve_the_real_contable_export_use_cases() {
        let program = program_from(
            r#"
            service S {
                rpc sanitizeForCsv(concepto: String) -> String {
                    concepto.replace(";", ",").replace("\n", " ")
                }
                rpc fixedWidthAmount(amount: String) -> String {
                    amount.padStart(8, "0")
                }
            }
        "#,
        );
        let db = Db::seeded();
        let sanitized = invoke_rpc(&program, "S", "sanitizeForCsv", &json!({"concepto": "pago;factura\nurgente"}), &db).unwrap();
        assert_eq!(sanitized, json!("pago,factura urgente"), "sin esto, un ';'/salto de línea real corrompería las columnas de ContaPlus");
        let padded = invoke_rpc(&program, "S", "fixedWidthAmount", &json!({"amount": "1234"}), &db).unwrap();
        assert_eq!(padded, json!("00001234"), "fixed-width de 8 para A3 Contable");
    }

    #[test]
    fn string_pad_start_and_pad_end_do_not_truncate_a_value_already_at_or_over_length() {
        let program = program_from(
            r#"
            service S {
                rpc padStart(s: String, n: Int) -> String { s.padStart(n, "0") }
                rpc padEnd(s: String, n: Int) -> String { s.padEnd(n, "0") }
            }
        "#,
        );
        let db = Db::seeded();
        assert_eq!(invoke_rpc(&program, "S", "padStart", &json!({"s": "12345", "n": 3}), &db).unwrap(), json!("12345"), "ya se pasa de largo -- se devuelve tal cual, sin acortar");
        assert_eq!(invoke_rpc(&program, "S", "padEnd", &json!({"s": "12345", "n": 5}), &db).unwrap(), json!("12345"), "ya está exacto");
    }

    #[test]
    fn string_pad_with_a_multi_char_pad_repeats_and_truncates_to_the_exact_fill() {
        let program = program_from(
            r#"
            service S {
                rpc padEnd(s: String, n: Int, pad: String) -> String { s.padEnd(n, pad) }
            }
        "#,
        );
        let db = Db::seeded();
        let result = invoke_rpc(&program, "S", "padEnd", &json!({"s": "x", "n": 6, "pad": "ab"}), &db).unwrap();
        assert_eq!(result, json!("xababa"), "pad repetido y truncado a los 5 caracteres que faltan, no 'ab' completo de más");
    }

    #[test]
    fn string_pad_rejects_an_empty_pad_when_padding_is_actually_needed() {
        let program = program_from(
            r#"
            service S {
                rpc padStart(s: String, n: Int) -> String { s.padStart(n, "") }
            }
        "#,
        );
        let db = Db::seeded();
        invoke_rpc(&program, "S", "padStart", &json!({"s": "x", "n": 5}), &db).expect_err("pad vacío pero hace falta rellenar");
        // Si ya cumple la longitud, un pad vacío nunca hace falta -- no debería fallar.
        assert!(invoke_rpc(&program, "S", "padStart", &json!({"s": "12345", "n": 3}), &db).is_ok());
    }

    #[test]
    fn string_pad_reports_a_clean_error_for_a_negative_or_absurdly_large_length() {
        let program = program_from(
            r#"
            service S {
                rpc padStart(s: String, n: Int) -> String { s.padStart(n, "0") }
            }
        "#,
        );
        let db = Db::seeded();
        invoke_rpc(&program, "S", "padStart", &json!({"s": "x", "n": -1}), &db).expect_err("length negativo");
        invoke_rpc(&program, "S", "padStart", &json!({"s": "x", "n": i64::MAX}), &db)
            .expect_err("un length gigante tiene que rechazarse limpio, no intentar asignar un string gigante");
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

    /// `auth.destroyAllSessions(userId)` (GRAMMAR.md §3.84) contra un rpc
    /// real -- a diferencia de `destroySession`, revoca TODAS las sesiones
    /// de OTRO usuario a la vez, típicamente gateado con
    /// `@requires(Role.Admin)` por quien escribe el `.link` (no es este
    /// método el que impone esa política).
    #[test]
    fn destroy_all_sessions_revokes_every_session_of_that_user_and_leaves_others_alone() {
        let program = program_from(
            r#"
            service S {
                rpc revoke(userId: Int) -> Int { auth.destroyAllSessions(userId) }
            }
        "#,
        );
        let sessions = SessionStore::new();
        let victim_a = sessions.create_with_user_id("Role".to_string(), "Member".to_string(), Some(7));
        let victim_b = sessions.create_with_user_id("Role".to_string(), "Member".to_string(), Some(7));
        let survivor = sessions.create_with_user_id("Role".to_string(), "Member".to_string(), Some(8));

        let result =
            invoke_rpc_with_sessions(&program, "S", "revoke", &json!({"userId": 7}), &Db::seeded(), &sessions, None)
                .unwrap();
        assert_eq!(result, json!(2), "user 7 tenía exactamente 2 sesiones abiertas");
        assert_eq!(sessions.role_for(&victim_a), None);
        assert_eq!(sessions.role_for(&victim_b), None);
        assert!(sessions.role_for(&survivor).is_some(), "la sesión de otro usuario no debe verse afectada");
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
            Some(&Annotation::Requires { enum_name: "Role".to_string(), variant_names: vec!["Admin".to_string()], ownership: None })
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

    #[test]
    fn list_sum_adds_every_element() {
        let program = program_from(
            r#"
            service S {
                rpc total(xs: Int[]) -> Int { xs.sum() }
            }
        "#,
        );
        let result = invoke_rpc(&program, "S", "total", &json!({"xs": [1200, 350, 7]}), &Db::seeded()).unwrap();
        assert_eq!(result, json!(1557));
    }

    // GRAMMAR.md §3.101: el caso real que motivó `.sum()` -- un reporte
    // financiero sumando montos ya filtrados en memoria (`incomeTx.length()
    // * tarifa` en vez de la suma real, porque no había forma de sumar sin
    // un `while` manual). Este test confirma explícitamente el caso vacío
    // (0 transacciones de un tipo en el período), el que un placeholder de
    // "cantidad * tarifa" jamás hubiera distinguido de "1 transacción
    // gratis" -- `.sum()` sobre una lista vacía siempre da 0.
    #[test]
    fn list_sum_on_an_empty_list_is_zero() {
        let program = program_from(
            r#"
            service S {
                rpc total(xs: Int[]) -> Int { xs.filter(|x: Int| { x > 1000000 }).sum() }
            }
        "#,
        );
        let result = invoke_rpc(&program, "S", "total", &json!({"xs": [1, 2, 3]}), &Db::seeded()).unwrap();
        assert_eq!(result, json!(0));
    }

    // ---- PLAN.md §9.14 ítem 2: List<T> + List<T> y .contains() (GRAMMAR.md §3.200) ----

    #[test]
    fn list_plus_list_concatenates_in_order() {
        let program = program_from(
            r#"
            service S {
                rpc merge(a: Int[], b: Int[]) -> Int[] { a + b }
            }
        "#,
        );
        let result = invoke_rpc(&program, "S", "merge", &json!({"a": [1, 2], "b": [3, 4, 5]}), &Db::seeded()).unwrap();
        assert_eq!(result, json!([1, 2, 3, 4, 5]));
    }

    // El caso real que motivó esta pieza: `let mut`/reasignación ya existía
    // para acumular escalares (ver `while_loop_aggregates_a_list_without_recursion`
    // más abajo) -- lo único que faltaba era que `+` aceptara List<T>, y con
    // eso el MISMO mecanismo ya existente resuelve "acumular una lista
    // creciendo en un loop" sin ningún constructo de mutación nuevo.
    #[test]
    fn while_loop_accumulates_a_growing_list_via_plus() {
        let program = program_from(
            r#"
            service S {
                rpc evens(xs: Int[]) -> Int[] {
                    let mut acc: Int[] = [];
                    let mut i = 0;
                    while i < xs.length() {
                        if xs[i] % 2 == 0 {
                            acc = acc + [xs[i]];
                        } else {
                        }
                        i = i + 1;
                    }
                    acc
                }
            }
        "#,
        );
        let result = invoke_rpc(&program, "S", "evens", &json!({"xs": [1, 2, 3, 4, 5, 6]}), &Db::seeded()).unwrap();
        assert_eq!(result, json!([2, 4, 6]));
    }

    #[test]
    fn list_contains_finds_a_present_element_and_not_an_absent_one() {
        let program = program_from(
            r#"
            service S {
                rpc has(xs: Int[], target: Int) -> Bool { xs.contains(target) }
            }
        "#,
        );
        let db = Db::seeded();
        let present = invoke_rpc(&program, "S", "has", &json!({"xs": [10, 20, 30], "target": 20}), &db).unwrap();
        assert_eq!(present, json!(true));
        let absent = invoke_rpc(&program, "S", "has", &json!({"xs": [10, 20, 30], "target": 99}), &db).unwrap();
        assert_eq!(absent, json!(false));
        let empty = invoke_rpc(&program, "S", "has", &json!({"xs": [], "target": 1}), &db).unwrap();
        assert_eq!(empty, json!(false));
    }

    // Caso real de MyFinance (PLAN.md §9.14 ítem 2): marcar facturas ya
    // conciliadas para no cruzar el mismo movimiento bancario contra dos
    // facturas del mismo importe exacto. Reproducido con datos de prueba: 2
    // movimientos de $100 (mismo importe), 2 facturas de $100 -- confirma
    // que cada factura se usa como máximo una vez, no que la primera
    // absorbe a las dos.
    #[test]
    fn list_plus_and_contains_solve_the_real_bank_reconciliation_dedup_use_case() {
        let program = program_from(
            r#"
            type Movimiento = { id: Int, monto: Int }
            type Factura = { id: Int, monto: Int }

            service Conciliacion {
                rpc conciliar(movimientos: Movimiento[], facturas: Factura[]) -> Int[] {
                    let mut usadas: Int[] = [];
                    let mut matched: Int[] = [];
                    let mut i = 0;
                    while i < movimientos.length() {
                        let mov = movimientos[i];
                        let mut j = 0;
                        while j < facturas.length() {
                            let fac = facturas[j];
                            if fac.monto == mov.monto && !usadas.contains(fac.id) {
                                usadas = usadas + [fac.id];
                                matched = matched + [mov.id];
                                j = facturas.length();
                            } else {
                            }
                            j = j + 1;
                        }
                        i = i + 1;
                    }
                    matched
                }
            }
        "#,
        );
        let result = invoke_rpc(
            &program,
            "Conciliacion",
            "conciliar",
            &json!({
                "movimientos": [{"id": 1, "monto": 100}, {"id": 2, "monto": 100}],
                "facturas": [{"id": 11, "monto": 100}, {"id": 12, "monto": 100}],
            }),
            &Db::seeded(),
        )
        .unwrap();
        assert_eq!(result, json!([1, 2]), "los dos movimientos de $100 tienen que conciliar contra facturas DISTINTAS, no la misma dos veces");
    }

    // ---- pdf.build (GRAMMAR.md §3.201) ----

    #[test]
    fn pdf_build_produces_bytes_that_start_with_the_pdf_magic_header() {
        let program = program_from(
            r#"
            service Docs {
                rpc make() -> String {
                    pdf.build([
                        PdfBlock.Text { content: "Factura #1", bold: true, size: 18 },
                        PdfBlock.Table { headers: ["Concepto", "Importe"], rows: [["Servicio", "100.00"]] },
                    ])
                }
            }
        "#,
        );
        let db = Db::seeded();
        let b64 = invoke_rpc(&program, "Docs", "make", &json!({}), &db).expect("pdf.build tiene que generar un PDF real");
        let b64 = b64.as_str().unwrap();
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64).expect("pdf.build tiene que devolver base64 válido");
        assert!(bytes.starts_with(b"%PDF-"), "un PDF real siempre arranca con la firma '%PDF-'");
    }

    #[test]
    fn pdf_build_handles_spanish_accented_characters_and_the_euro_sign() {
        // GRAMMAR.md §3.201: WinAnsiEncoding, no UTF-8 crudo -- si esto no
        // codificara bien, no crashearía (encode_winansi nunca falla), pero
        // sí perdería contenido en silencio. Confirma al menos que no
        // explota y que el PDF resultante sigue siendo válido.
        let program = program_from(
            r#"
            service Docs {
                rpc make() -> String {
                    pdf.build([PdfBlock.Text { content: "Facturación de servicios: 100€ (José Núñez)", bold: false, size: 12 }])
                }
            }
        "#,
        );
        let db = Db::seeded();
        let b64 = invoke_rpc(&program, "Docs", "make", &json!({}), &db).expect("pdf.build tiene que generar un PDF real");
        let b64 = b64.as_str().unwrap();
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64).expect("pdf.build tiene que devolver base64 válido");
        assert!(bytes.starts_with(b"%PDF-"));
    }

    #[test]
    fn pdf_build_paginates_automatically_when_content_overflows_a_page() {
        let program = program_from(
            r#"
            service Docs {
                rpc make() -> String {
                    let mut blocks: PdfBlock[] = [];
                    let mut i = 0;
                    while i < 80 {
                        blocks = blocks + [PdfBlock.Text { content: "linea de contenido de la factura", bold: false, size: 12 }];
                        i = i + 1;
                    }
                    pdf.build(blocks)
                }
            }
        "#,
        );
        let db = Db::seeded();
        let b64 = invoke_rpc(&program, "Docs", "make", &json!({}), &db).expect("pdf.build tiene que generar un PDF real");
        let b64 = b64.as_str().unwrap();
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64).unwrap();
        let page_count = bytes.windows(b"/MediaBox".len()).filter(|w| *w == b"/MediaBox").count();
        assert!(page_count >= 2, "80 líneas de texto deberían desbordar una sola página A4, se contaron {page_count} página(s) (por /MediaBox)");
    }

    #[test]
    fn pdf_build_table_with_a_mismatched_row_length_is_a_clean_runtime_error() {
        let program = program_from(
            r#"
            service Docs {
                rpc make() -> String {
                    pdf.build([PdfBlock.Table { headers: ["a", "b"], rows: [["1"]] }])
                }
            }
        "#,
        );
        let db = Db::seeded();
        let e = invoke_rpc(&program, "Docs", "make", &json!({}), &db).unwrap_err();
        assert!(e.message.contains("columna"), "mensaje inesperado: {}", e.message);
    }

    // ---- excel.build / excel.parse (GRAMMAR.md §3.202) ----

    #[test]
    fn excel_build_produces_bytes_that_start_with_the_zip_magic_header() {
        // `.xlsx` es un contenedor ZIP -- la firma real es "PK\x03\x04",
        // no "%PDF-" como el caso de PDF.
        let program = program_from(
            r#"
            service Docs {
                rpc make() -> String {
                    excel.build([ExcelSheet {
                        name: "Hoja1",
                        headers: ["Concepto", "Importe"],
                        rows: [[ExcelCell.Text { value: "Servicio" }, ExcelCell.Number { value: 100.00.toDecimal() }]],
                    }])
                }
            }
        "#,
        );
        let db = Db::seeded();
        let b64 = invoke_rpc(&program, "Docs", "make", &json!({}), &db).expect("excel.build tiene que generar un .xlsx real");
        let b64 = b64.as_str().unwrap();
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64).expect("excel.build tiene que devolver base64 válido");
        assert!(bytes.starts_with(b"PK\x03\x04"), "un .xlsx real siempre arranca con la firma ZIP 'PK\\x03\\x04'");
    }

    #[test]
    fn excel_build_and_parse_round_trip_all_five_cell_variants_exactly() {
        // Round-trip real: acá se controlan las dos puntas, así que se
        // puede confirmar exactitud (no solo "no crashea") -- el Decimal
        // tiene que volver EXACTO (no 1234.5678 -> 1234.5678000001 por el
        // viaje por f64) y la fecha al mismo milisegundo exacto.
        let program = program_from(
            r#"
            service Docs {
                rpc roundtrip() -> ExcelSheet[] {
                    excel.parse(excel.build([ExcelSheet {
                        name: "Hoja1",
                        headers: ["Texto", "Numero", "Fecha", "Booleano", "Vacio"],
                        rows: [[
                            ExcelCell.Text { value: "hola" },
                            ExcelCell.Number { value: 1234.5678.toDecimal() },
                            ExcelCell.Date { value: dateFromParts(2026, 3, 15, 10, 30, 0) },
                            ExcelCell.Bool { value: true },
                            ExcelCell.Empty {},
                        ]],
                    }]))
                }
            }
        "#,
        );
        let db = Db::seeded();
        let result = invoke_rpc(&program, "Docs", "roundtrip", &json!({}), &db)
            .expect("excel.build + excel.parse tienen que ir y volver sin error");
        assert_eq!(result.as_array().unwrap().len(), 1, "una sola hoja");
        let sheet = &result[0];
        assert_eq!(sheet["name"], json!("Hoja1"));
        assert_eq!(sheet["headers"], json!(["Texto", "Numero", "Fecha", "Booleano", "Vacio"]));
        let row = &sheet["rows"][0];
        assert_eq!(row[0], json!({"type": "Text", "value": "hola"}));
        assert_eq!(row[1], json!({"type": "Number", "value": "1234.5678"}), "el Decimal tiene que volver exacto, no aproximado por el viaje por f64");
        assert_eq!(row[2], json!({"type": "Date", "value": "2026-03-15T10:30:00.000Z"}), "la fecha tiene que volver al mismo milisegundo exacto");
        assert_eq!(row[3], json!({"type": "Bool", "value": true}));
        assert_eq!(row[4], json!({"type": "Empty"}));
    }

    #[test]
    fn excel_build_sheet_with_a_mismatched_row_length_is_a_clean_runtime_error() {
        let program = program_from(
            r#"
            service Docs {
                rpc make() -> String {
                    excel.build([ExcelSheet { name: "H", headers: ["a", "b"], rows: [[ExcelCell.Text { value: "1" }]] }])
                }
            }
        "#,
        );
        let db = Db::seeded();
        let e = invoke_rpc(&program, "Docs", "make", &json!({}), &db).unwrap_err();
        assert!(e.message.contains("columna"), "mensaje inesperado: {}", e.message);
    }

    #[test]
    fn excel_parse_on_bytes_that_are_not_a_real_xlsx_is_a_clean_runtime_error_not_a_panic() {
        let program = program_from(
            r#"
            service Docs {
                rpc make(b64: String) -> ExcelSheet[] { excel.parse(b64) }
            }
        "#,
        );
        let db = Db::seeded();
        // Base64 válido, pero el contenido decodificado no es un .xlsx real.
        let not_xlsx_b64 = "aG9sYSBtdW5kbw==";
        let e = invoke_rpc(&program, "Docs", "make", &json!({"b64": not_xlsx_b64}), &db).unwrap_err();
        assert!(e.message.contains("excel.parse"), "mensaje inesperado: {}", e.message);
    }

    // ---- mcp.sample (GRAMMAR.md §3.203, Pieza C) ----

    #[test]
    fn mcp_sample_is_reachable_through_field_access_and_fails_cleanly_without_a_session() {
        // GRAMMAR.md §3.199: mismo test de "no vuelvas a faltar en el
        // allowlist de Expr::FieldAccess" que Decimal ya tiene -- este test
        // corre FUERA de cualquier contexto MCP real (`invoke_rpc` no pasa
        // por `runtime/mcp.rs::handle_tools_call`), así que la única forma
        // de que esto falle con "no se puede acceder al campo 'sample'" es
        // que `Value::Mcp` vuelva a faltar en ese allowlist -- si en cambio
        // falla con el mensaje de "no hay sesión MCP activa", el método SÍ
        // se alcanzó, que es lo único que este test necesita confirmar.
        let program = program_from(
            r#"
            service Docs {
                rpc ask(prompt: String) -> String { mcp.sample(prompt) }
            }
        "#,
        );
        let db = Db::seeded();
        let e = invoke_rpc(&program, "Docs", "ask", &json!({"prompt": "hola"}), &db).unwrap_err();
        assert!(e.message.contains("mcp.sample"), "mensaje inesperado (¿mcp.sample sigue inalcanzable?): {}", e.message);
        assert!(e.message.contains("sesión MCP activa"), "mensaje inesperado: {}", e.message);
    }

    #[test]
    fn module_marker_singletons_compare_equal_to_themselves() {
        // Auditoría del lenguaje (2026-09-01): el checker tipa `pdf == pdf`
        // como válido (Eq exige tipos compatibles, mismo tipo a ambos
        // lados) -- pero `impl PartialEq for Value` nunca había extendido
        // el grupo "marcador interno singleton" (que sí cubre a
        // Db/Auth/Math/Crypto/Http/Json/Base64) a Pdf/Excel/Mcp/Env/
        // Request/Smtp/Response, así que caían en el `_ => false` final:
        // `math == math` daba `true` pero `pdf == pdf` daba `false`,
        // inconsistencia silenciosa reproducida contra el binario real
        // antes de este fix. Confirma que también valen `env`/`request`
        // (marcadores más viejos, con el mismo hueco).
        let program = program_from(
            r#"
            service Check {
                rpc pdfSelfEq() -> Bool { pdf == pdf }
                rpc excelSelfEq() -> Bool { excel == excel }
                rpc mcpSelfEq() -> Bool { mcp == mcp }
                rpc envSelfEq() -> Bool { env == env }
            }
        "#,
        );
        let db = Db::seeded();
        for rpc in ["pdfSelfEq", "excelSelfEq", "mcpSelfEq", "envSelfEq"] {
            let result = invoke_rpc(&program, "Check", rpc, &json!({}), &db).unwrap();
            assert_eq!(result, json!(true), "{rpc} debería dar true, igual que math == math");
        }
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

    /// GRAMMAR.md §3.162: `/` y `%` por cero sobre enteros eran un PANIC de
    /// Rust, no un error de runtime -- trivialmente alcanzable con un
    /// divisor que viene de datos del usuario. Con un hilo por request
    /// (§3.158) el panic ya no mata el proceso, pero mata el hilo SIN pasar
    /// por ningún camino de limpieza: adentro de un `transaction { }` dejaba
    /// la transacción SQL abierta para siempre, wedgeando todas las
    /// transacciones futuras del proceso y descartando en silencio
    /// escrituras ya confirmadas al cliente con un 200 (reproducido contra
    /// un servidor real antes de arreglarlo).
    #[test]
    fn integer_division_or_remainder_by_zero_is_a_clean_runtime_error_not_a_panic() {
        let program = program_from(
            r#"
            service S {
                rpc divZero(d: Int) -> Int { 100 / d }
                rpc remZero(d: Int) -> Int { 100 % d }
            }
        "#,
        );
        let db = Db::seeded();
        let e = invoke_rpc(&program, "S", "divZero", &json!({"d": 0}), &db).unwrap_err();
        assert!(e.message.contains("por cero"), "mensaje inesperado: {}", e.message);
        let e = invoke_rpc(&program, "S", "remZero", &json!({"d": 0}), &db).unwrap_err();
        assert!(e.message.contains("por cero"), "mensaje inesperado: {}", e.message);
        // El camino feliz no cambia.
        assert_eq!(invoke_rpc(&program, "S", "divZero", &json!({"d": 7}), &db).unwrap(), json!(14));
        assert_eq!(invoke_rpc(&program, "S", "remZero", &json!({"d": 7}), &db).unwrap(), json!(2));
    }

    /// El otro caso que panicaba: `i64::MIN / -1` no cabe en `i64`.
    /// `checked_div` lo cubre junto con el divisor cero, con un mensaje
    /// distinto (desborde, no división por cero).
    #[test]
    fn integer_division_overflow_is_a_clean_runtime_error_too() {
        let program = program_from(
            r#"
            service S {
                rpc f(a: Int64, b: Int64) -> Int64 { a / b }
            }
        "#,
        );
        let e = invoke_rpc(&program, "S", "f", &json!({"a": "-9223372036854775808", "b": "-1"}), &Db::seeded()).unwrap_err();
        assert!(e.message.contains("desborde"), "mensaje inesperado: {}", e.message);
    }

    /// AUDIT-2026-08-27.md #16: `/`/`%` ya tenían `checked_*` desde
    /// §3.162 -- `+`/`-`/`*` (y el `-` unario) seguían con aritmética
    /// cruda, mismo riesgo de panic/wrap silencioso con un valor cerca de
    /// `i64::MAX`/`MIN`.
    #[test]
    fn integer_add_sub_mul_and_unary_neg_overflow_are_clean_runtime_errors_too() {
        let program = program_from(
            r#"
            service S {
                rpc add(a: Int64, b: Int64) -> Int64 { a + b }
                rpc sub(a: Int64, b: Int64) -> Int64 { a - b }
                rpc mul(a: Int64, b: Int64) -> Int64 { a * b }
                rpc neg(a: Int64) -> Int64 { -a }
            }
        "#,
        );
        let db = Db::seeded();
        let e = invoke_rpc(&program, "S", "add", &json!({"a": "9223372036854775807", "b": "1"}), &db).unwrap_err();
        assert!(e.message.contains("desborde"), "add: {}", e.message);
        let e = invoke_rpc(&program, "S", "sub", &json!({"a": "-9223372036854775808", "b": "1"}), &db).unwrap_err();
        assert!(e.message.contains("desborde"), "sub: {}", e.message);
        let e = invoke_rpc(&program, "S", "mul", &json!({"a": "9223372036854775807", "b": "2"}), &db).unwrap_err();
        assert!(e.message.contains("desborde"), "mul: {}", e.message);
        let e = invoke_rpc(&program, "S", "neg", &json!({"a": "-9223372036854775808"}), &db).unwrap_err();
        assert!(e.message.contains("desborde"), "neg: {}", e.message);
        // El camino feliz no cambia.
        assert_eq!(invoke_rpc(&program, "S", "add", &json!({"a": "2", "b": "3"}), &db).unwrap(), json!("5"));
        assert_eq!(invoke_rpc(&program, "S", "sub", &json!({"a": "5", "b": "3"}), &db).unwrap(), json!("2"));
        assert_eq!(invoke_rpc(&program, "S", "mul", &json!({"a": "2", "b": "3"}), &db).unwrap(), json!("6"));
        assert_eq!(invoke_rpc(&program, "S", "neg", &json!({"a": "5"}), &db).unwrap(), json!("-5"));
    }

    /// `List<Int>.sum()` sobre valores cuya suma real supera `i64::MAX`
    /// (AUDIT-2026-08-27.md #16) -- mismo criterio, `checked_add` en vez de
    /// `+=` crudo.
    #[test]
    fn list_int_sum_overflow_is_a_clean_runtime_error() {
        let program = program_from(
            r#"
            service S {
                rpc total(xs: Int[]) -> Int { xs.sum() }
            }
        "#,
        );
        let e = invoke_rpc(&program, "S", "total", &json!({"xs": [9223372036854775807i64, 1]}), &Db::seeded()).unwrap_err();
        assert!(e.message.contains("desborde"), "{}", e.message);
    }

    /// Un `transaction { }` cuyo cuerpo falla por división por cero tiene
    /// que hacer ROLLBACK como cualquier otro error -- y, sobre todo, dejar
    /// la base usable para la SIGUIENTE transacción. Antes del fix la
    /// primera fallaba con un panic y todas las posteriores daban "ya hay
    /// una transacción abierta" para siempre.
    #[test]
    fn a_transaction_whose_body_divides_by_zero_rolls_back_and_leaves_the_db_usable() {
        let program = program_from(
            r#"
            type Item = { id: Int, name: String }
            db { items: Item[] }
            service S {
                rpc boom(d: Int) -> Int {
                    transaction {
                        db.items.insert(Item { id: 0, name: "boom" });
                        let x = 100 / d;
                    }
                    0
                }
                rpc ok(name: String) -> Int {
                    transaction { db.items.insert(Item { id: 0, name: name }); }
                    db.items.all().length()
                }
                rpc count() -> Int { db.items.all().length() }
            }
        "#,
        );
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        let e = invoke_rpc(&program, "S", "boom", &json!({"d": 0}), &db).unwrap_err();
        assert!(e.message.contains("por cero"), "{}", e.message);
        assert_eq!(invoke_rpc(&program, "S", "count", &json!({}), &db).unwrap(), json!(0), "la fila del cuerpo tiene que haberse rollbackeado");
        // Y la base sigue usable: una transacción POSTERIOR funciona.
        assert_eq!(invoke_rpc(&program, "S", "ok", &json!({"name": "a"}), &db).unwrap(), json!(1));
    }

    /// GRAMMAR.md §3.163 originalmente usaba un desborde de `+` sobre `i64`
    /// como disparador -- código de producción sin arreglar EN ESE MOMENTO,
    /// a propósito, para probar que el `catch_unwind` de `Expr::Transaction`
    /// protege contra un panic GENÉRICO, no solo contra el caso puntual de
    /// división por cero que ya tenía su propio `RuntimeError` limpio.
    /// AUDIT-2026-08-27.md #16 cerró ESE disparador también (`+` ahora usa
    /// `checked_add`) -- así que hoy este mismo escenario ya no panica: da
    /// un `RuntimeError` limpio por el camino normal, sin necesitar
    /// `catch_unwind` para nada. Este test quedó como regresión de esa
    /// composición (desborde dentro de `transaction{}` sigue rollbackeando
    /// y dejando la base usable), no como prueba del mecanismo de panic --
    /// para ESO, `a_transaction_whose_body_divides_by_zero_...` (arriba)
    /// alcanza igual de bien ahora que los dos disparadores dan el mismo
    /// tipo de error limpio.
    #[test]
    fn a_transaction_whose_body_overflows_still_rolls_back_and_leaves_the_db_usable() {
        let program = program_from(
            r#"
            type Item = { id: Int, name: String }
            db { items: Item[] }
            service S {
                rpc boom(a: Int) -> Int {
                    transaction {
                        db.items.insert(Item { id: 0, name: "boom" });
                        let x = a + 1;
                    }
                    0
                }
                rpc ok(name: String) -> Int {
                    transaction { db.items.insert(Item { id: 0, name: name }); }
                    db.items.all().length()
                }
                rpc count() -> Int { db.items.all().length() }
            }
        "#,
        );
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        let e = invoke_rpc(&program, "S", "boom", &json!({"a": i64::MAX}), &db).unwrap_err();
        assert!(e.message.contains("desborde"), "{}", e.message);
        assert_eq!(invoke_rpc(&program, "S", "count", &json!({}), &db).unwrap(), json!(0), "la fila del cuerpo tiene que haberse rollbackeado");
        // Y la base sigue usable: una transacción POSTERIOR funciona.
        assert_eq!(invoke_rpc(&program, "S", "ok", &json!({"name": "a"}), &db).unwrap(), json!(1));
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

    /// GRAMMAR.md §3.95: el caso real que motiva `countWhere` -- contar
    /// cuántas reseñas tiene un producto sin traer la tabla entera a
    /// memoria. `|r: Review| { r.productId == productId }` es exactamente
    /// el shape pusheable (`ast::recognize_predicate_expr`) -- este
    /// test prueba el resultado CORRECTO, no que el pushdown haya corrido
    /// (eso se verificó manualmente contra el SQL real emitido; acá lo que
    /// importa es que el número sea el que tiene que ser).
    #[test]
    fn count_where_counts_only_matching_rows_via_the_sql_pushdown_path() {
        let code = r#"
        type Review = { id: Int, productId: Int, rating: Int }
        db { reviews: Review[] }
        service Reviews {
          rpc add(productId: Int, rating: Int) -> Review {
            db.reviews.insert(Review { id: 0, productId: productId, rating: rating })
          }
          rpc countFor(productId: Int) -> Int {
            db.reviews.countWhere(|r: Review| { r.productId == productId })
          }
        }
        "#;
        let program = crate::parser::parse(crate::lexer::tokenize(code).unwrap()).unwrap();
        let db = Db::new(&program, std::path::Path::new(":memory:"));

        invoke_rpc(&program, "Reviews", "add", &json!({"productId": 1, "rating": 5}), &db).unwrap();
        invoke_rpc(&program, "Reviews", "add", &json!({"productId": 1, "rating": 3}), &db).unwrap();
        invoke_rpc(&program, "Reviews", "add", &json!({"productId": 2, "rating": 4}), &db).unwrap();

        assert_eq!(invoke_rpc(&program, "Reviews", "countFor", &json!({"productId": 1}), &db).unwrap(), json!(2));
        assert_eq!(invoke_rpc(&program, "Reviews", "countFor", &json!({"productId": 2}), &db).unwrap(), json!(1));
        assert_eq!(invoke_rpc(&program, "Reviews", "countFor", &json!({"productId": 999}), &db).unwrap(), json!(0));
    }

    /// `findWhere` con un predicado PUSHEABLE (`==` sobre un solo campo)
    /// tiene que seguir devolviendo exactamente las mismas filas que antes
    /// de GRAMMAR.md §3.95 -- el atajo de SQL es una optimización de
    /// EJECUCIÓN, invisible desde el resultado.
    #[test]
    fn find_where_with_a_pushable_predicate_returns_the_same_rows_as_before() {
        let code = r#"
        type Review = { id: Int, productId: Int, rating: Int }
        db { reviews: Review[] }
        service Reviews {
          rpc add(productId: Int, rating: Int) -> Review {
            db.reviews.insert(Review { id: 0, productId: productId, rating: rating })
          }
          rpc listFor(productId: Int) -> Review[] {
            db.reviews.findWhere(|r: Review| { r.productId == productId })
          }
        }
        "#;
        let program = crate::parser::parse(crate::lexer::tokenize(code).unwrap()).unwrap();
        let db = Db::new(&program, std::path::Path::new(":memory:"));

        invoke_rpc(&program, "Reviews", "add", &json!({"productId": 7, "rating": 5}), &db).unwrap();
        invoke_rpc(&program, "Reviews", "add", &json!({"productId": 8, "rating": 1}), &db).unwrap();

        let rows = invoke_rpc(&program, "Reviews", "listFor", &json!({"productId": 7}), &db).unwrap();
        let arr = rows.as_array().unwrap();
        assert_eq!(arr.len(), 1, "{arr:?}");
        assert_eq!(arr[0]["rating"], json!(5));
    }

    /// GRAMMAR.md §3.145: `deleteWhere` con un predicado PUSHEABLE selecciona
    /// las filas a borrar vía `find_where_conjunction` (SQL real) en vez de
    /// traer la colección entera -- el RESULTADO tiene que ser idéntico al
    /// camino interpretado de siempre: solo las filas que matchean se
    /// borran, el resto sobrevive.
    #[test]
    fn delete_where_with_a_pushable_predicate_deletes_only_matching_rows() {
        let code = r#"
        type Review = { id: Int, productId: Int, rating: Int }
        db { reviews: Review[] }
        service Reviews {
          rpc add(productId: Int, rating: Int) -> Review {
            db.reviews.insert(Review { id: 0, productId: productId, rating: rating })
          }
          rpc removeFor(productId: Int) -> Int {
            db.reviews.deleteWhere(|r: Review| { r.productId == productId })
          }
          rpc all() -> Review[] { db.reviews.all() }
        }
        "#;
        let program = crate::parser::parse(crate::lexer::tokenize(code).unwrap()).unwrap();
        let db = Db::new(&program, std::path::Path::new(":memory:"));

        invoke_rpc(&program, "Reviews", "add", &json!({"productId": 1, "rating": 5}), &db).unwrap();
        invoke_rpc(&program, "Reviews", "add", &json!({"productId": 1, "rating": 3}), &db).unwrap();
        invoke_rpc(&program, "Reviews", "add", &json!({"productId": 2, "rating": 4}), &db).unwrap();

        let deleted = invoke_rpc(&program, "Reviews", "removeFor", &json!({"productId": 1}), &db).unwrap();
        assert_eq!(deleted, json!(2), "deben borrarse las dos filas de productId=1");

        let remaining = invoke_rpc(&program, "Reviews", "all", &json!({}), &db).unwrap();
        let arr = remaining.as_array().unwrap();
        assert_eq!(arr.len(), 1, "{arr:?}");
        assert_eq!(arr[0]["productId"], json!(2), "la fila de productId=2 tiene que sobrevivir");
    }

    /// Un predicado NO pusheable (§3.171: comparar dos campos entre sí con
    /// `==`/`!=` -- a diferencia de los cuatro relacionales, que sí se
    /// empujan) tiene que caer al camino interpretado de siempre -- mismo
    /// resultado, solo más lento. Este mismo ejemplo usaba `<` hasta que
    /// §3.171 lo volvió pusheable; ver el comentario en el test hermano de
    /// `countWhere`/`findWhere` que advierte revisar el ejemplo cada vez que
    /// el alcance pusheable crece.
    #[test]
    fn delete_where_falls_back_correctly_for_a_non_pushable_predicate() {
        let code = r#"
        type Review = { id: Int, productId: Int, rating: Int }
        db { reviews: Review[] }
        service Reviews {
          rpc add(productId: Int, rating: Int) -> Review {
            db.reviews.insert(Review { id: 0, productId: productId, rating: rating })
          }
          rpc removeEqualRated() -> Int {
            // Compara dos campos del propio parámetro entre sí con `==` --
            // no tiene la forma pusheable (`ast::recognize_predicate_expr`
            // solo reconoce campo-vs-campo para los cuatro relacionales,
            // GRAMMAR.md §3.171), así que cae al camino interpretado a
            // propósito.
            db.reviews.deleteWhere(|r: Review| { r.rating == r.productId })
          }
          rpc all() -> Review[] { db.reviews.all() }
        }
        "#;
        let program = crate::parser::parse(crate::lexer::tokenize(code).unwrap()).unwrap();
        let db = Db::new(&program, std::path::Path::new(":memory:"));

        invoke_rpc(&program, "Reviews", "add", &json!({"productId": 1, "rating": 1}), &db).unwrap(); // 1 == 1: se borra
        invoke_rpc(&program, "Reviews", "add", &json!({"productId": 5, "rating": 10}), &db).unwrap(); // 10 != 5: sobrevive

        let deleted = invoke_rpc(&program, "Reviews", "removeEqualRated", &json!({}), &db).unwrap();
        assert_eq!(deleted, json!(1));
        let remaining = invoke_rpc(&program, "Reviews", "all", &json!({}), &db).unwrap();
        let arr = remaining.as_array().unwrap();
        assert_eq!(arr.len(), 1, "{arr:?}");
        assert_eq!(arr[0]["productId"], json!(5));
    }

    /// GRAMMAR.md §3.171: `deleteWhere` también empuja la SELECCIÓN (no el
    /// DELETE en sí, que sigue fila por fila a propósito -- ver el
    /// comentario del brazo `"deleteWhere"` en `call_method`) cuando el
    /// predicado es una comparación campo-vs-campo.
    #[test]
    fn delete_where_pushes_down_the_selection_for_a_field_vs_field_comparison() {
        let code = r#"
        type Booking = { id: Int, startDay: Int, endDay: Int }
        db { bookings: Booking[] }
        service Bookings {
          rpc add(startDay: Int, endDay: Int) -> Booking {
            db.bookings.insert(Booking { id: 0, startDay: startDay, endDay: endDay })
          }
          rpc removeInvalidRanges() -> Int {
            db.bookings.deleteWhere(|b: Booking| { b.endDay <= b.startDay })
          }
          rpc all() -> Booking[] { db.bookings.all() }
        }
        "#;
        let program = crate::parser::parse(crate::lexer::tokenize(code).unwrap()).unwrap();
        let db = Db::new(&program, std::path::Path::new(":memory:"));

        invoke_rpc(&program, "Bookings", "add", &json!({"startDay": 1, "endDay": 5}), &db).unwrap(); // válido, sobrevive
        invoke_rpc(&program, "Bookings", "add", &json!({"startDay": 5, "endDay": 3}), &db).unwrap(); // inválido, se borra

        let deleted = invoke_rpc(&program, "Bookings", "removeInvalidRanges", &json!({}), &db).unwrap();
        assert_eq!(deleted, json!(1));
        let remaining = invoke_rpc(&program, "Bookings", "all", &json!({}), &db).unwrap();
        let arr = remaining.as_array().unwrap();
        assert_eq!(arr.len(), 1, "{arr:?}");
        assert_eq!(arr[0]["startDay"], json!(1));
    }

    /// GRAMMAR.md §3.108: `countWhere`/`findWhere` empujan a SQL los cinco
    /// operadores relacionales además de `==` (`!=`/`<`/`<=`/`>`/`>=`) --
    /// caso real que lo motiva, `chat.link` de un adoptador real:
    /// `c.unreadCount > 0`. Un solo test cubriendo los cinco a la vez, más
    /// el caso del campo del lado DERECHO (`0 < c.unreadCount`, que tiene
    /// que dar el mismo resultado que `c.unreadCount > 0` -- el operador se
    /// invierte internamente, `ast::flip_comparison_operator`).
    #[test]
    fn count_where_and_find_where_push_down_every_relational_operator() {
        let code = r#"
        type Chat = { id: Int, name: String, unreadCount: Int }
        db { chats: Chat[] }
        service Chats {
          rpc add(name: String, unreadCount: Int) -> Chat {
            db.chats.insert(Chat { id: 0, name: name, unreadCount: unreadCount })
          }
          rpc gt() -> Int { db.chats.countWhere(|c: Chat| { c.unreadCount > 0 }) }
          rpc gtFlipped() -> Int { db.chats.countWhere(|c: Chat| { 0 < c.unreadCount }) }
          rpc lt() -> Int { db.chats.countWhere(|c: Chat| { c.unreadCount < 3 }) }
          rpc gte() -> Int { db.chats.countWhere(|c: Chat| { c.unreadCount >= 3 }) }
          rpc lte() -> Int { db.chats.countWhere(|c: Chat| { c.unreadCount <= 3 }) }
          rpc neq() -> Int { db.chats.countWhere(|c: Chat| { c.unreadCount != 0 }) }
          rpc gtRows() -> Chat[] { db.chats.findWhere(|c: Chat| { c.unreadCount > 0 }) }
        }
        "#;
        let program = crate::parser::parse(crate::lexer::tokenize(code).unwrap()).unwrap();
        let db = Db::new(&program, std::path::Path::new(":memory:"));

        invoke_rpc(&program, "Chats", "add", &json!({"name": "a", "unreadCount": 0}), &db).unwrap();
        invoke_rpc(&program, "Chats", "add", &json!({"name": "b", "unreadCount": 3}), &db).unwrap();
        invoke_rpc(&program, "Chats", "add", &json!({"name": "c", "unreadCount": 5}), &db).unwrap();

        assert_eq!(invoke_rpc(&program, "Chats", "gt", &json!({}), &db).unwrap(), json!(2), "unreadCount > 0: b y c");
        assert_eq!(invoke_rpc(&program, "Chats", "gtFlipped", &json!({}), &db).unwrap(), json!(2), "0 < unreadCount: mismo resultado que gt, campo a la derecha");
        assert_eq!(invoke_rpc(&program, "Chats", "lt", &json!({}), &db).unwrap(), json!(1), "unreadCount < 3: solo a");
        assert_eq!(invoke_rpc(&program, "Chats", "gte", &json!({}), &db).unwrap(), json!(2), "unreadCount >= 3: b y c");
        assert_eq!(invoke_rpc(&program, "Chats", "lte", &json!({}), &db).unwrap(), json!(2), "unreadCount <= 3: a y b");
        assert_eq!(invoke_rpc(&program, "Chats", "neq", &json!({}), &db).unwrap(), json!(2), "unreadCount != 0: b y c");
        let rows = invoke_rpc(&program, "Chats", "gtRows", &json!({}), &db).unwrap();
        assert_eq!(rows.as_array().unwrap().len(), 2);
    }

    /// `countWhere`/`findWhere` con un predicado NO pusheable (§3.171: una
    /// comparación `==`/`!=` -- a propósito distinta de los cuatro
    /// relacionales, que SÍ se empujan, ver
    /// `count_where_and_find_where_push_down_a_field_vs_field_comparison` --
    /// entre DOS campos del propio parámetro) sigue dando el resultado
    /// correcto por el camino interpretado de siempre -- el pushdown de
    /// GRAMMAR.md §3.95/§3.108/§3.109/§3.170/§3.171 es un atajo, nunca el
    /// único camino. Este mismo ejemplo YA tuvo que reescribirse dos veces
    /// antes (§3.108, §3.109) porque el alcance pusheable creció -- ver el
    /// comentario ahí mismo que advierte revisarlo cada vez que vuelve a
    /// crecer.
    #[test]
    fn count_where_and_find_where_fall_back_correctly_for_a_non_pushable_predicate() {
        let code = r#"
        type Review = { id: Int, productId: Int, rating: Int }
        db { reviews: Review[] }
        service Reviews {
          rpc add(productId: Int, rating: Int) -> Review {
            db.reviews.insert(Review { id: 0, productId: productId, rating: rating })
          }
          rpc countRatingEqualsProductId() -> Int {
            db.reviews.countWhere(|r: Review| { r.rating == r.productId })
          }
          rpc listRatingEqualsProductId() -> Review[] {
            db.reviews.findWhere(|r: Review| { r.rating == r.productId })
          }
        }
        "#;
        let program = crate::parser::parse(crate::lexer::tokenize(code).unwrap()).unwrap();
        let db = Db::new(&program, std::path::Path::new(":memory:"));

        invoke_rpc(&program, "Reviews", "add", &json!({"productId": 1, "rating": 5}), &db).unwrap(); // rating(5) != productId(1)
        invoke_rpc(&program, "Reviews", "add", &json!({"productId": 5, "rating": 5}), &db).unwrap(); // rating(5) == productId(5)
        invoke_rpc(&program, "Reviews", "add", &json!({"productId": 2, "rating": 2}), &db).unwrap(); // rating(2) == productId(2)
        invoke_rpc(&program, "Reviews", "add", &json!({"productId": 9, "rating": 3}), &db).unwrap(); // rating(3) != productId(9)

        assert_eq!(invoke_rpc(&program, "Reviews", "countRatingEqualsProductId", &json!({}), &db).unwrap(), json!(2));
        let rows = invoke_rpc(&program, "Reviews", "listRatingEqualsProductId", &json!({}), &db).unwrap();
        assert_eq!(rows.as_array().unwrap().len(), 2);
    }

    /// GRAMMAR.md §3.171: `countWhere`/`findWhere` empujan a SQL una
    /// comparación entre DOS campos del propio parámetro (`item.endDate >
    /// item.startDate`) para los cuatro operadores relacionales -- caso real
    /// motivador: filtrar rangos de fecha inválidos/válidos sin traer la
    /// tabla entera. Cubre los dos órdenes (`a < b` reconocido directo,
    /// también dentro de un `&&` con una hoja normal) y confirma que `==`
    /// entre dos campos NO toma este camino (test de arriba).
    #[test]
    fn count_where_and_find_where_push_down_a_field_vs_field_comparison() {
        let code = r#"
        type Booking = { id: Int, room: String, startDay: Int, endDay: Int }
        db { bookings: Booking[] }
        service Bookings {
          rpc add(room: String, startDay: Int, endDay: Int) -> Booking {
            db.bookings.insert(Booking { id: 0, room: room, startDay: startDay, endDay: endDay })
          }
          rpc countInvalidRanges() -> Int {
            db.bookings.countWhere(|b: Booking| { b.endDay <= b.startDay })
          }
          rpc listInvalidRanges() -> Booking[] {
            db.bookings.findWhere(|b: Booking| { b.endDay <= b.startDay })
          }
          // El mismo campo-vs-campo adentro de una conjunción con una hoja
          // normal -- confirma que el árbol And/Or no se rompe al mezclar
          // una hoja `FieldPair` con una hoja `Leaf` común.
          rpc invalidRangesInRoom(room: String) -> Int {
            db.bookings.countWhere(|b: Booking| { b.room == room && b.endDay <= b.startDay })
          }
        }
        "#;
        let program = crate::parser::parse(crate::lexer::tokenize(code).unwrap()).unwrap();
        let db = Db::new(&program, std::path::Path::new(":memory:"));

        invoke_rpc(&program, "Bookings", "add", &json!({"room": "A", "startDay": 1, "endDay": 5}), &db).unwrap(); // válido
        invoke_rpc(&program, "Bookings", "add", &json!({"room": "A", "startDay": 5, "endDay": 5}), &db).unwrap(); // inválido (==)
        invoke_rpc(&program, "Bookings", "add", &json!({"room": "B", "startDay": 9, "endDay": 3}), &db).unwrap(); // inválido (<)
        invoke_rpc(&program, "Bookings", "add", &json!({"room": "A", "startDay": 2, "endDay": 8}), &db).unwrap(); // válido

        assert_eq!(
            invoke_rpc(&program, "Bookings", "countInvalidRanges", &json!({}), &db).unwrap(),
            json!(2),
            "endDay <= startDay: las reservas 2 y 3"
        );
        let rows = invoke_rpc(&program, "Bookings", "listInvalidRanges", &json!({}), &db).unwrap();
        let ids: Vec<i64> = rows.as_array().unwrap().iter().map(|r| r["id"].as_i64().unwrap()).collect();
        assert_eq!(ids, vec![2, 3]);

        assert_eq!(
            invoke_rpc(&program, "Bookings", "invalidRangesInRoom", &json!({"room": "A"}), &db).unwrap(),
            json!(1),
            "solo la reserva 2 (room A, endDay <= startDay) -- la 3 es de room B"
        );
    }

    /// GRAMMAR.md §3.109: `countWhere`/`findWhere` empujan a SQL una
    /// conjunción `&&` de varias hojas simples, no solo un único operador
    /// (generaliza §3.108). Casos reales que lo motivan, dos de "CRM":
    /// `notifications.link` (`n.userId == uid && !n.read`) e
    /// `inventory.link` (`p.stock <= 5 && p.stock > 0`, el MISMO campo dos
    /// veces en la misma conjunción). De paso cubre las dos hojas
    /// booleanas nuevas que no necesitan ningún operador explícito:
    /// `!x.campo` (equivale a `== false`) y `x.campo` solo (`== true`).
    #[test]
    fn count_where_and_find_where_push_down_a_conjunction_of_several_leaves() {
        let code = r#"
        type Notification = { id: Int, userId: Int, read: Bool }
        type Product = { id: Int, stock: Int }
        db { notifications: Notification[], products: Product[] }
        service Notifications {
          rpc add(userId: Int, read: Bool) -> Notification {
            db.notifications.insert(Notification { id: 0, userId: userId, read: read })
          }
          rpc unreadFor(userId: Int) -> Int {
            db.notifications.countWhere(|n: Notification| { n.userId == userId && !n.read })
          }
          rpc unreadRowsFor(userId: Int) -> Notification[] {
            db.notifications.findWhere(|n: Notification| { n.userId == userId && !n.read })
          }
          rpc readCount() -> Int { db.notifications.countWhere(|n: Notification| { n.read }) }
        }
        service Products {
          rpc add(stock: Int) -> Product { db.products.insert(Product { id: 0, stock: stock }) }
          rpc lowStockCount() -> Int {
            db.products.countWhere(|p: Product| { p.stock <= 5 && p.stock > 0 })
          }
        }
        "#;
        let program = crate::parser::parse(crate::lexer::tokenize(code).unwrap()).unwrap();
        let db = Db::new(&program, std::path::Path::new(":memory:"));

        invoke_rpc(&program, "Notifications", "add", &json!({"userId": 1, "read": false}), &db).unwrap();
        invoke_rpc(&program, "Notifications", "add", &json!({"userId": 1, "read": true}), &db).unwrap();
        invoke_rpc(&program, "Notifications", "add", &json!({"userId": 2, "read": false}), &db).unwrap();

        assert_eq!(
            invoke_rpc(&program, "Notifications", "unreadFor", &json!({"userId": 1}), &db).unwrap(),
            json!(1),
            "usuario 1 tiene 1 no leída de sus 2 notificaciones"
        );
        let rows = invoke_rpc(&program, "Notifications", "unreadRowsFor", &json!({"userId": 1}), &db).unwrap();
        assert_eq!(rows.as_array().unwrap().len(), 1);
        assert_eq!(
            invoke_rpc(&program, "Notifications", "readCount", &json!({}), &db).unwrap(),
            json!(1),
            "hoja booleana suelta (x.campo, sin operador, equivale a == true)"
        );

        invoke_rpc(&program, "Products", "add", &json!({"stock": 0}), &db).unwrap();
        invoke_rpc(&program, "Products", "add", &json!({"stock": 3}), &db).unwrap();
        invoke_rpc(&program, "Products", "add", &json!({"stock": 5}), &db).unwrap();
        invoke_rpc(&program, "Products", "add", &json!({"stock": 10}), &db).unwrap();

        assert_eq!(
            invoke_rpc(&program, "Products", "lowStockCount", &json!({}), &db).unwrap(),
            json!(2),
            "stock <= 5 && stock > 0: solo 3 y 5 -- el MISMO campo dos veces en la conjunción"
        );
    }

    /// GRAMMAR.md §3.170: `||` combinando condiciones, en tres formas --
    /// una disyunción pura, `&&` mezclado con `||` respetando la
    /// precedencia real del lenguaje (`&&` liga más fuerte), y un `||` de
    /// más de dos hojas. Verifica CORRECCIÓN (el resultado tiene que ser
    /// idéntico al que daría el camino interpretado) -- el pushdown en sí
    /// es una optimización de rendimiento, `||` ya funcionaba antes vía el
    /// fallback interpretado.
    #[test]
    fn count_where_and_find_where_push_down_a_disjunction_and_mixed_and_or() {
        let code = r#"
        type Ticket = { id: Int, status: String, priority: Int, assignee: String }
        db { tickets: Ticket[] }
        service Tickets {
          rpc add(status: String, priority: Int, assignee: String) -> Ticket {
            db.tickets.insert(Ticket { id: 0, status: status, priority: priority, assignee: assignee })
          }
          // Disyunción pura de 3 hojas.
          rpc urgentOrCritical() -> Int {
            db.tickets.countWhere(|t: Ticket| { t.status == "urgent" || t.status == "critical" || t.priority == 1 })
          }
          // && mezclado con || -- tiene que parsear como (a && b) || c, no
          // como a && (b || c).
          rpc mineOrCritical(who: String) -> Ticket[] {
            db.tickets.findWhere(|t: Ticket| { t.assignee == who && t.status == "open" || t.status == "critical" })
          }
        }
        "#;
        let program = crate::parser::parse(crate::lexer::tokenize(code).unwrap()).unwrap();
        let db = Db::new(&program, std::path::Path::new(":memory:"));

        invoke_rpc(&program, "Tickets", "add", &json!({"status": "urgent", "priority": 3, "assignee": "ada"}), &db).unwrap();
        invoke_rpc(&program, "Tickets", "add", &json!({"status": "open", "priority": 1, "assignee": "bob"}), &db).unwrap();
        invoke_rpc(&program, "Tickets", "add", &json!({"status": "closed", "priority": 5, "assignee": "ada"}), &db).unwrap();
        invoke_rpc(&program, "Tickets", "add", &json!({"status": "open", "priority": 4, "assignee": "ada"}), &db).unwrap();
        invoke_rpc(&program, "Tickets", "add", &json!({"status": "critical", "priority": 5, "assignee": "bob"}), &db).unwrap();

        assert_eq!(
            invoke_rpc(&program, "Tickets", "urgentOrCritical", &json!({}), &db).unwrap(),
            json!(3),
            "urgent (1) + priority==1 (1) + critical (1) = 3"
        );

        // mineOrCritical("ada"): (assignee==ada && status==open) || status==critical
        // -> el ticket 4 (ada/open) + el ticket 5 (bob/critical), NUNCA el
        // ticket 1 (ada/urgent, no matchea ninguna mitad) ni el 3 (ada/closed).
        let rows = invoke_rpc(&program, "Tickets", "mineOrCritical", &json!({"who": "ada"}), &db).unwrap();
        let ids: Vec<i64> = rows.as_array().unwrap().iter().map(|r| r["id"].as_i64().unwrap()).collect();
        assert_eq!(ids, vec![4, 5], "precedencia real: (a && b) || c, no a && (b || c)");
    }

    /// GRAMMAR.md §3.170: una hoja `campo == variable` adentro de una rama
    /// `||` donde `variable` resulta `null` en runtime -- mismo caso NULL-
    /// seguro que ya cubría la conjunción pura (`IS NULL`, nunca `= ?` con
    /// un parámetro NULL, que en SQL nunca es cierto).
    #[test]
    fn a_null_valued_leaf_inside_an_or_branch_still_uses_is_null() {
        let code = r#"
        type Item = { id: Int, category: String?, archived: Bool }
        db { items: Item[] }
        service S {
          rpc add(category: String?, archived: Bool) -> Item {
            db.items.insert(Item { id: 0, category: category, archived: archived })
          }
          rpc uncategorizedOrArchived(cat: String?) -> Item[] {
            db.items.findWhere(|i: Item| { i.category == cat || i.archived })
          }
        }
        "#;
        let program = crate::parser::parse(crate::lexer::tokenize(code).unwrap()).unwrap();
        let db = Db::new(&program, std::path::Path::new(":memory:"));

        invoke_rpc(&program, "S", "add", &json!({"category": null, "archived": false}), &db).unwrap();
        invoke_rpc(&program, "S", "add", &json!({"category": "books", "archived": false}), &db).unwrap();
        invoke_rpc(&program, "S", "add", &json!({"category": "tools", "archived": true}), &db).unwrap();

        let rows = invoke_rpc(&program, "S", "uncategorizedOrArchived", &json!({"cat": null}), &db).unwrap();
        let ids: Vec<i64> = rows.as_array().unwrap().iter().map(|r| r["id"].as_i64().unwrap()).collect();
        assert_eq!(ids, vec![1, 3], "el item sin categoría (IS NULL) y el archivado -- nunca el de 'books'");
    }

    /// `countWhere("id" == valor)` es un caso especial (§3.95: `"id"` nunca
    /// vive en `Db::columns`, que es "todo menos id") -- confirma que el
    /// atajo de SQL también cubre ese campo, no solo los declarados.
    #[test]
    fn count_where_pushes_down_a_comparison_on_id_too() {
        let code = r#"
        type Item = { id: Int, name: String }
        db { items: Item[] }
        service Items {
          rpc add(name: String) -> Item { db.items.insert(Item { id: 0, name: name }) }
          rpc countById(target: Int) -> Int { db.items.countWhere(|i: Item| { i.id == target }) }
        }
        "#;
        let program = crate::parser::parse(crate::lexer::tokenize(code).unwrap()).unwrap();
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        let created = invoke_rpc(&program, "Items", "add", &json!({"name": "algo"}), &db).unwrap();
        let id = created["id"].as_i64().unwrap();

        assert_eq!(invoke_rpc(&program, "Items", "countById", &json!({"target": id}), &db).unwrap(), json!(1));
        assert_eq!(invoke_rpc(&program, "Items", "countById", &json!({"target": id + 999}), &db).unwrap(), json!(0));
    }

    /// El atajo de SQL de `countWhere`/`findWhere` (GRAMMAR.md §3.95) sigue
    /// respetando `@softDelete` (§3.78) igual que el camino interpretado --
    /// `comparison_condition` (`runtime/db.rs`) AND-ea la misma condición
    /// `"<campo>" IS NULL` que ya usa `count()`/`all()`. Sin este AND, una
    /// fila soft-deleteada aparecería en un `countWhere`/`findWhere`
    /// pusheado aunque ya hubiera "desaparecido" de todo lo demás.
    #[test]
    fn count_where_and_find_where_respect_soft_delete_even_when_pushed_down() {
        let code = r#"
        type Item = { id: Int, sessionId: String, @softDelete deletedAt: Timestamp? = null }
        db { items: Item[] }
        service Items {
          rpc add(sessionId: String) -> Item { db.items.insert(Item { id: 0, sessionId: sessionId, deletedAt: null }) }
          rpc removeAll(sessionId: String) -> Int { db.items.deleteWhere(|i: Item| { i.sessionId == sessionId }) }
          rpc countFor(sessionId: String) -> Int { db.items.countWhere(|i: Item| { i.sessionId == sessionId }) }
          rpc listFor(sessionId: String) -> Item[] { db.items.findWhere(|i: Item| { i.sessionId == sessionId }) }
        }
        "#;
        let program = crate::parser::parse(crate::lexer::tokenize(code).unwrap());
        let program = match program {
            Ok(p) => p,
            Err(e) => panic!("{e:?}"),
        };
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        invoke_rpc(&program, "Items", "add", &json!({"sessionId": "abc"}), &db).unwrap();
        assert_eq!(invoke_rpc(&program, "Items", "countFor", &json!({"sessionId": "abc"}), &db).unwrap(), json!(1));

        invoke_rpc(&program, "Items", "removeAll", &json!({"sessionId": "abc"}), &db).unwrap();
        assert_eq!(
            invoke_rpc(&program, "Items", "countFor", &json!({"sessionId": "abc"}), &db).unwrap(),
            json!(0),
            "una fila soft-deleteada no debe contar, ni siquiera por el camino pusheado a SQL"
        );
        let rows = invoke_rpc(&program, "Items", "listFor", &json!({"sessionId": "abc"}), &db).unwrap();
        assert_eq!(rows.as_array().unwrap().len(), 0);
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

    /// GRAMMAR.md §3.177: reabrir un archivo SQLite con una colección de PK
    /// `Uuid` tiene que pasar `check_schema_matches` -- la fila `TEXT NOT
    /// NULL` esperada para `"id"` en esa rama, exactamente igual que
    /// `INTEGER` para una PK `Int` de siempre. Sin este test, la rama
    /// `IdKind::Uuid` de `check_schema_matches` solo se ejercitaba en la
    /// PRIMERA apertura (tabla vacía, `existing.is_empty()` corta antes de
    /// comparar) -- nunca en un reinicio real contra un archivo ya escrito.
    #[test]
    fn reopening_a_uuid_pk_collection_matches_its_own_schema_and_keeps_the_row() {
        let path = std::env::temp_dir().join("c_script_test_uuid_pk_reopen.db");
        let _ = std::fs::remove_file(&path);

        let program = program_from("type Lead = { id: Uuid, email: String } db { leads: Lead[] }");
        let generated_id = {
            let db = Db::new(&program, &path);
            let inserted =
                db.call("leads", "insert", vec![Value::Struct(vec![("email".into(), Value::Str("a@example.com".into()))])]).unwrap();
            let Value::Struct(fields) = inserted else { panic!("se esperaba struct") };
            let Some((_, Value::Uuid(id))) = fields.into_iter().find(|(n, _)| n == "id") else { panic!("se esperaba id: Uuid") };
            id
        };

        // Reabrir el MISMO archivo -- `check_schema_matches` corre contra
        // una tabla ya poblada esta vez, no una recién creada.
        let db2 = Db::new(&program, &path);
        let found = db2.call("leads", "find", vec![Value::Uuid(generated_id.clone())]).unwrap();
        assert_ne!(found, Value::Null, "la fila insertada antes de reabrir sigue ahí, con el mismo id");
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

    /// `--filter <nombre>` (PLAN.md §9.7, GRAMMAR.md §3.82): substring sobre
    /// el NOMBRE del test, sensible a mayúsculas, mismo criterio que `cargo
    /// test <substring>`.
    #[test]
    fn run_program_tests_filtered_only_runs_tests_whose_name_contains_the_substring() {
        let code = r#"
        test "crear usuario exitoso" { assert(true); }
        test "actualizar usuario exitoso" { assert(true); }
        test "borrar item" { assert(true); }
        "#;
        let program = crate::parser::parse(crate::lexer::tokenize(code).unwrap()).unwrap();

        let filtered = run_program_tests_filtered(&program, Some("usuario")).expect("deberia correr");
        assert_eq!(filtered.total, 2, "{filtered:?}");
        assert_eq!(filtered.passed, 2);

        let none_matched = run_program_tests_filtered(&program, Some("no-existe")).expect("deberia correr");
        assert_eq!(none_matched.total, 0, "un filtro que no matchea nada corre cero tests, no es un error");

        let unfiltered = run_program_tests_filtered(&program, None).expect("deberia correr");
        assert_eq!(unfiltered.total, 3, "None corre TODOS -- mismo comportamiento que run_program_tests");
        assert_eq!(unfiltered, run_program_tests(&program).unwrap());
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

    // GRAMMAR.md §3.157: cierra el límite que §3.65 dejaba abierto -- agrupar
    // por un Timestamp truncado a día/mes/año, corriendo contra el SQLite en
    // memoria real de `test` (no un mock). Confirma tanto la SUMA agrupada
    // como que la `key` que vuelve es un `Timestamp` exacto -- comparado
    // contra `dateFromParts(...)`, no solo que el conteo de grupos cierre.
    #[test]
    fn sum_by_with_a_truncated_timestamp_group_key_groups_by_day_month_and_year() {
        let code = r#"
        type Sale = { id: Int, at: Timestamp, amount: Int }
        type DayTotal = { key: Timestamp, value: Int }
        db { sales: Sale[] }

        service Sales {
            rpc add(at: Timestamp, amount: Int) -> Sale {
                db.sales.insert(Sale { id: 0, at: at, amount: amount })
            }
        }

        test "sumBy agrupado por Timestamp truncado" {
            Sales.add(dateFromParts(2026, 3, 15, 10, 30, 0), 10);
            Sales.add(dateFromParts(2026, 3, 15, 23, 59, 59), 5);
            Sales.add(dateFromParts(2026, 3, 20, 0, 0, 0), 7);
            Sales.add(dateFromParts(2027, 1, 5, 5, 0, 0), 3);

            let byDay = db.sales.sumBy(|s: Sale| { s.at.truncateToDay() }, |s: Sale| { s.amount });
            assert(byDay.length() == 3, "3 dias distintos");

            let byMonth = db.sales.sumBy(|s: Sale| { s.at.truncateToMonth() }, |s: Sale| { s.amount });
            assert(byMonth.length() == 2, "2 meses distintos");

            let byYear = db.sales.sumBy(|s: Sale| { s.at.truncateToYear() }, |s: Sale| { s.amount });
            assert(byYear.length() == 2, "2 anios distintos");

            let mut found = false;
            let mut i = 0;
            while i < byDay.length() {
                if byDay[i].key == dateFromParts(2026, 3, 15, 0, 0, 0) {
                    assert(byDay[i].value == 15, "10 + 5 en el mismo dia");
                    found = true;
                } else {
                }
                i = i + 1;
            }
            assert(found, "el grupo del 15 de marzo debe existir con la key exacta truncada");
        }
        "#;
        let program = crate::parser::parse(crate::lexer::tokenize(code).unwrap()).unwrap();
        let summary = run_program_tests(&program).expect("ejecucion de tests");
        assert_eq!(summary.total, 1);
        assert_eq!(summary.passed, 1, "{:?}", summary.failed);
    }

    /// Bug real, encontrado por una auditoría multi-agente adversarial
    /// (26/08/2026): `"campo" / 1000` con los dos operandos enteros es
    /// división ENTERA de SQLite, que trunca hacia cero -- para un epoch
    /// PRE-1970 (negativo) con resto de milisegundos no nulo, redondea
    /// hacia 1970 en vez de hacia abajo, empujando la fila al día
    /// siguiente. `dateFromParts` no alcanza para este repro (no tiene
    /// parámetro de milisegundos) -- se manda el string ISO-8601 crudo vía
    /// `invoke_rpc`, igual que llegaría por HTTP real.
    #[test]
    fn truncate_to_day_floors_correctly_for_a_pre_1970_timestamp_with_a_sub_second_remainder() {
        let program = program_from(
            r#"
            type Sale = { id: Int, at: Timestamp, amount: Int }
            type DayTotal = { key: Timestamp, value: Int }
            db { sales: Sale[] }
            service Sales {
                rpc create(at: Timestamp, amount: Int) -> Sale {
                    db.sales.insert(Sale { id: 0, at: at, amount: amount })
                }
                rpc byDay() -> DayTotal[] {
                    db.sales.sumBy(|s: Sale| { s.at.truncateToDay() }, |s: Sale| { s.amount })
                }
            }
        "#,
        );
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        invoke_rpc(&program, "Sales", "create", &json!({"at": "1969-12-31T23:59:59.500Z", "amount": 1}), &db).unwrap();
        let by_day = invoke_rpc(&program, "Sales", "byDay", &json!({}), &db).unwrap();
        let rows = by_day.as_array().unwrap();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(
            rows[0]["key"], json!("1969-12-31T00:00:00.000Z"),
            "500ms antes de medianoche UTC del 31 sigue siendo 31 de diciembre, no 1 de enero: {rows:?}"
        );
    }

    // GRAMMAR.md §3.102: `maxRow`/`minRow` -- caso real que los motiva
    // (IgnisLove, `bandit_rewards.link`, `getBestArm()`): `db.arms.all()[0]`
    // devuelve la fila de menor `id`, NUNCA la de mejor recompensa, pese al
    // nombre del rpc -- un bug de producción real, no hipotético. Este test
    // confirma que `maxRow`/`minRow` sí encuentran la fila correcta, contra
    // el SQLite en memoria real de `test` (no un mock).
    #[test]
    fn max_row_and_min_row_find_the_row_with_the_best_and_worst_reward_not_the_lowest_id() {
        let code = r#"
        type Arm = { id: Int, name: String, avgRewardTenths: Int }
        db { arms: Arm[] }

        service Arms {
            rpc create(name: String, avgRewardTenths: Int) -> Arm {
                db.arms.insert(Arm { id: 0, name: name, avgRewardTenths: avgRewardTenths })
            }
        }

        test "maxRow/minRow encuentran la fila correcta, no la de menor id" {
            Arms.create("A", 10);
            Arms.create("B", 95);
            Arms.create("C", 40);

            let best = db.arms.maxRow(|a: Arm| { a.avgRewardTenths });
            match best {
                a: Arm => assert(a.name == "B", "el brazo insertado SEGUNDO tiene la mejor recompensa, no el de id mas bajo"),
                null => panic("maxRow no deberia dar null sobre una coleccion no vacia"),
            }

            let worst = db.arms.minRow(|a: Arm| { a.avgRewardTenths });
            match worst {
                a: Arm => assert(a.name == "A"),
                null => panic("minRow no deberia dar null sobre una coleccion no vacia"),
            }
        }
        "#;
        let program = crate::parser::parse(crate::lexer::tokenize(code).unwrap()).unwrap();
        let summary = run_program_tests(&program).expect("ejecucion de tests");
        assert_eq!(summary.total, 1);
        assert_eq!(summary.passed, 1, "{:?}", summary.failed);
    }

    #[test]
    fn max_row_on_an_empty_collection_is_null() {
        let code = r#"
            type Arm = { id: Int, avgRewardTenths: Int }
            db { arms: Arm[] }
            service S {
                rpc getBestArm() -> Arm? { db.arms.maxRow(|a: Arm| { a.avgRewardTenths }) }
            }
        "#;
        let program = crate::parser::parse(crate::lexer::tokenize(code).unwrap()).unwrap();
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        let result = invoke_rpc(&program, "S", "getBestArm", &json!({}), &db).unwrap();
        assert_eq!(result, json!(null));
    }

    // GRAMMAR.md §3.105: `db.<c>.increment(id, selector, delta) -> T` -- un
    // `UPDATE campo = campo + delta` atómico, sin ida y vuelta de lectura
    // previa (a diferencia de `upsert` con un `updateFn` que sí lee primero
    // -- el patrón que puede perder un incremento entre dos procesos, el
    // caso real de IgnisLove que motiva esto).

    #[test]
    fn increment_adds_delta_atomically_including_negative_deltas() {
        let code = r#"
            type Counter = { id: Int, name: String, hits: Int }
            db { counters: Counter[] }
            service S {
                rpc create(name: String) -> Counter { db.counters.insert(Counter { id: 0, name: name, hits: 10 }) }
                rpc bump(id: Int, delta: Int) -> Counter { db.counters.increment(id, |c: Counter| { c.hits }, delta) }
            }
        "#;
        let program = crate::parser::parse(crate::lexer::tokenize(code).unwrap()).unwrap();
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        let created = invoke_rpc(&program, "S", "create", &json!({"name": "views"}), &db).unwrap();
        let id = created["id"].as_i64().unwrap();

        let bumped = invoke_rpc(&program, "S", "bump", &json!({"id": id, "delta": 5}), &db).unwrap();
        assert_eq!(bumped["hits"], json!(15));

        let decremented = invoke_rpc(&program, "S", "bump", &json!({"id": id, "delta": -3}), &db).unwrap();
        assert_eq!(decremented["hits"], json!(12));
    }

    #[test]
    fn increment_on_a_missing_id_is_a_clean_error() {
        let code = r#"
            type Counter = { id: Int, hits: Int }
            db { counters: Counter[] }
            service S {
                rpc bump(id: Int) -> Counter { db.counters.increment(id, |c: Counter| { c.hits }, 1) }
            }
        "#;
        let program = crate::parser::parse(crate::lexer::tokenize(code).unwrap()).unwrap();
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        let err = invoke_rpc(&program, "S", "bump", &json!({"id": 999}), &db).unwrap_err();
        assert!(format!("{err:?}").contains("no hay ningún elemento con id"), "{err:?}");
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

    /// AUDIT-2026-08-27.md #1: `crypto.randomToken(length)` con `length`
    /// negativo o absurdamente grande pasaba directo de `i64` a `usize` con
    /// `as`, terminando en un pedido de memoria gigante -- para un valor
    /// negativo, el propio macro `vec!` panica ("capacity overflow", mata
    /// solo el hilo); para uno grande pero positivo (`i64::MAX`), el pedido
    /// llegaba al allocator real del sistema y `handle_alloc_error` hacía
    /// `std::process::abort()` -- tumbaba el PROCESO ENTERO, sin que
    /// `catch_unwind` pudiera hacer nada, confirmado contra un `linkc serve`
    /// real antes de este fix. Ahora los dos casos dan un `RuntimeError`
    /// limpio antes de tocar memoria.
    #[test]
    fn random_token_rejects_a_negative_or_absurdly_large_length_instead_of_crashing() {
        let program = program_from(
            r#"
            service S {
                rpc gen(length: Int) -> String { crypto.randomToken(length) }
            }
        "#,
        );
        let db = Db::seeded();
        let e = invoke_rpc(&program, "S", "gen", &json!({"length": -1}), &db).unwrap_err();
        assert!(e.message.contains("length"), "{}", e.message);
        let e = invoke_rpc(&program, "S", "gen", &json!({"length": 9223372036854775807i64}), &db).unwrap_err();
        assert!(e.message.contains("length"), "{}", e.message);
        let e = invoke_rpc(&program, "S", "gen", &json!({"length": 0}), &db).unwrap_err();
        assert!(e.message.contains("length"), "{}", e.message);
        // El camino feliz no cambia.
        let ok = invoke_rpc(&program, "S", "gen", &json!({"length": 32}), &db).unwrap();
        assert_eq!(ok.as_str().unwrap().chars().count(), 32);
    }

}

/// GRAMMAR.md §3.230 (PLAN.md §9.19 ítem 5) contra SQLite real: el ORDER BY
/// (con `NULLS LAST` en las dos direcciones) viaja en el SQL de `all`/
/// `page`/`findWhere`, la clave secundaria funciona, un predicado no
/// empujable conserva el orden, y `sortBy`/`sortByDesc` en memoria dan el
/// MISMO orden que SQL (null al final, orden estable). El harness saltea
/// el checker a propósito -- la parte de tipos está en
/// `checker.rs::order_by_tests`, y `tests/cli_order_by.rs` corre los dos.
#[cfg(test)]
mod order_by_tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::parse;
    use serde_json::json;

    fn amounts(v: &serde_json::Value) -> Vec<i64> {
        v.as_array().unwrap().iter().map(|r| r["amount"].as_i64().unwrap()).collect()
    }

    #[test]
    fn order_by_and_sort_by_agree_with_nulls_last_and_stable_ties() {
        let src = r#"
            type Event = { id: Int, kind: String, amount: Int, at: Timestamp? }
            type NewEvent = { kind: String, amount: Int, at: Timestamp? }
            db { events: Event[] }
            service S {
                rpc add(kind: String, amount: Int, at: Timestamp?) -> Event { db.events.insert(NewEvent { kind: kind, amount: amount, at: at }) }
                rpc newest(n: Int) -> Event[] { db.events.orderByDesc(|e: Event| { e.at }).page(n, 0) }
                rpc oldest() -> Event[] { db.events.orderBy(|e: Event| { e.at }).all() }
                rpc ofKind(k: String) -> Event[] { db.events.orderByDesc(|e: Event| { e.amount }).findWhere(|e: Event| { e.kind == k }) }
                rpc bigOnes() -> Event[] { db.events.orderByDesc(|e: Event| { e.amount }).findWhere(|e: Event| { e.amount + 0 > 1 }) }
                rpc twoKeys() -> Event[] { db.events.orderBy(|e: Event| { e.kind }).orderByDesc(|e: Event| { e.amount }).all() }
                rpc memDesc() -> Event[] { db.events.all().sortByDesc(|e: Event| { e.at }) }
                rpc memAsc() -> Event[] { db.events.all().sortBy(|e: Event| { e.at }) }
                rpc memStr() -> Event[] { db.events.all().sortBy(|e: Event| { e.kind }) }
            }
        "#;
        let tokens = tokenize(src).unwrap_or_else(|e| panic!("{e}"));
        let program = parse(tokens).unwrap_or_else(|e| panic!("{e:?}"));
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        for (kind, amount, at) in [
            ("a", 1, json!("2026-01-01T00:00:00.000Z")),
            ("b", 5, json!(null)),
            ("a", 3, json!("2026-03-01T00:00:00.000Z")),
            ("a", 2, json!("2026-02-01T00:00:00.000Z")),
        ] {
            invoke_rpc(&program, "S", "add", &json!({"kind": kind, "amount": amount, "at": at}), &db).unwrap();
        }
        let call = |name: &str, args: serde_json::Value| amounts(&invoke_rpc(&program, "S", name, &args, &db).unwrap());

        assert_eq!(call("newest", json!({"n": 2})), vec![3, 2], "los dos más nuevos -- el NULL nunca primero en DESC");
        assert_eq!(call("newest", json!({"n": 10})), vec![3, 2, 1, 5], "DESC con el NULL al final");
        assert_eq!(call("oldest", json!({})), vec![1, 2, 3, 5], "ASC con el NULL al final");
        assert_eq!(call("ofKind", json!({"k": "a"})), vec![3, 2, 1], "WHERE empujado + ORDER BY");
        assert_eq!(call("bigOnes", json!({})), vec![5, 3, 2], "predicado no empujable: filtra en memoria conservando el orden SQL");
        assert_eq!(call("twoKeys", json!({})), vec![3, 2, 1, 5], "clave secundaria: kind ASC, amount DESC");
        assert_eq!(call("memDesc", json!({})), vec![3, 2, 1, 5], "sortByDesc = mismo orden que orderByDesc");
        assert_eq!(call("memAsc", json!({})), vec![1, 2, 3, 5], "sortBy = mismo orden que orderBy");
        assert_eq!(call("memStr", json!({})), vec![1, 3, 2, 5], "sortBy estable: los 'a' conservan su orden por id");
    }
}

/// GRAMMAR.md §3.232 (PLAN.md §9.19 ítem 7): `@hidden` se quita en el
/// borde JSON de un rpc (también anidado en listas y structs) y en las
/// filas en vivo de `subscribe`, mientras el cuerpo del rpc lo sigue
/// leyendo. El harness saltea el checker; `checker.rs::hidden_tests` y
/// `tests/cli_hidden.rs` cubren la parte de tipos y el codegen.
#[cfg(test)]
mod hidden_tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::parse;
    use serde_json::json;

    #[test]
    fn hidden_fields_never_leave_the_process_but_stay_readable_inside() {
        let src = r#"
            type User = { id: Int, email: String, @hidden passwordHash: String }
            type NewUser = { email: String, passwordHash: String }
            type Report = { owner: User, n: Int }
            db { users: User[] }
            service S {
                rpc create(email: String, hash: String) -> User { db.users.insert(NewUser { email: email, passwordHash: hash }) }
                rpc all() -> User[] { db.users.all() }
                rpc report() -> Report[] { db.users.all().map(|u: User| { Report { owner: u, n: 1 } }) }
                rpc countWithHash(h: String) -> Int { db.users.all().filter(|u: User| { u.passwordHash == h }).length() }
            }
        "#;
        let tokens = tokenize(src).unwrap_or_else(|e| panic!("{e}"));
        let program = parse(tokens).unwrap_or_else(|e| panic!("{e:?}"));
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        let (snapshot_before, rx) = db.subscribe("users").unwrap();
        assert!(snapshot_before.is_empty());

        let created = invoke_rpc(&program, "S", "create", &json!({"email": "a@x", "hash": "h1"}), &db).unwrap();
        assert_eq!(created["email"], "a@x");
        assert!(created.get("passwordHash").is_none(), "{created}");

        let all = invoke_rpc(&program, "S", "all", &json!({}), &db).unwrap();
        assert!(all[0].get("passwordHash").is_none(), "{all}");
        let report = invoke_rpc(&program, "S", "report", &json!({}), &db).unwrap();
        assert_eq!(report[0]["n"], 1);
        assert!(report[0]["owner"].get("passwordHash").is_none(), "anidado en un struct: {report}");
        assert_eq!(report[0]["owner"]["email"], "a@x");

        // Dentro del proceso el campo sigue ahí.
        assert_eq!(invoke_rpc(&program, "S", "countWithHash", &json!({"h": "h1"}), &db).unwrap(), json!(1));

        // Fila en vivo y snapshot de `subscribe`: sin el campo.
        let live = rx.try_recv().expect("la inserción publica una fila en vivo");
        assert!(live.get("passwordHash").is_none(), "{live}");
        assert_eq!(live["email"], "a@x");
        let (snapshot, _rx2) = db.subscribe("users").unwrap();
        assert!(snapshot[0].get("passwordHash").is_none(), "{snapshot:?}");
    }
}

/// GRAMMAR.md §3.235: sin motor (harness, `linkc test`), `ai.models()` sigue
/// funcionando y `ai.generate` explica por qué no.
#[cfg(test)]
mod ai_builtin_tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::parse;
    use serde_json::json;

    #[test]
    fn models_lists_declared_aliases_and_generate_without_an_engine_is_a_clean_error() {
        let src = r#"
            ai { router: "qwen2.5:0.5b", coder: "./coder.gguf" }
            service S {
                rpc models() -> String[] { ai.models() }
                rpc ask() -> String { ai.generate("router", "hola", 8) }
            }
        "#;
        let tokens = tokenize(src).unwrap_or_else(|e| panic!("{e}"));
        let program = parse(tokens).unwrap_or_else(|e| panic!("{e:?}"));
        let db = Db::new(&program, std::path::Path::new(":memory:"));
        assert_eq!(invoke_rpc(&program, "S", "models", &json!({}), &db).unwrap(), json!(["router", "coder"]));
        let err = invoke_rpc(&program, "S", "ask", &json!({}), &db).unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("§3.235"), "{msg}");
    }
}
