// Runtime mínimo interpretado (PLAN.md §2.4, Fase 0): un tree-walking
// interpreter que ejecuta cuerpos de rpc/fn contra un "db" en memoria.
// No es el runtime final del lenguaje — Fase 1+ compila a WASM/nativo
// (PLAN.md §4) — esto solo alcanza para que la demo E2E responda de verdad.

pub mod db;
pub mod server;

use crate::ast::*;
use db::Db;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Null,
    Struct(Vec<(String, Value)>),
    Variant { variant: String, fields: Vec<(String, Value)> },
    List(Vec<Value>),
    /// Marcadores internos — nunca deberían llegar a `value_to_json` (ver la
    /// salvaguarda ahí). Representan `db`, `db.coleccion`, y un método ligado
    /// (`recv.metodo`) a la espera de ser invocado, ej. `db.users.find`.
    Db,
    DbCollection(String),
    BoundMethod(Box<Value>, String),
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

type Env = HashMap<String, Value>;
type Fns<'a> = HashMap<String, &'a FnDecl>;

pub fn eval_block(block: &Block, env: &Env, db: &Db, fns: &Fns) -> Result<Value, RuntimeError> {
    let mut local = env.clone();
    for stmt in &block.stmts {
        match stmt {
            Stmt::Let { name, value, .. } => {
                let v = eval_expr(value, &local, db, fns)?;
                local.insert(name.clone(), v);
            }
            Stmt::Return(Some(e)) => return eval_expr(e, &local, db, fns),
            Stmt::Return(None) => return Ok(Value::Null),
            Stmt::Expr(e) => {
                eval_expr(e, &local, db, fns)?;
            }
        }
    }
    match &block.tail {
        Some(e) => eval_expr(e, &local, db, fns),
        None => Ok(Value::Null),
    }
}

pub fn eval_expr(e: &Expr, env: &Env, db: &Db, fns: &Fns) -> Result<Value, RuntimeError> {
    match e {
        Expr::Int(n) => Ok(Value::Int(*n)),
        Expr::Float(n) => Ok(Value::Float(*n)),
        Expr::Str(s) => Ok(Value::Str(s.clone())),
        Expr::Bool(b) => Ok(Value::Bool(*b)),
        Expr::Null => Ok(Value::Null),
        Expr::Paren(inner) => eval_expr(inner, env, db, fns),
        Expr::Ident(name) => {
            if name == "db" {
                return Ok(Value::Db);
            }
            env.get(name)
                .cloned()
                .ok_or_else(|| err(format!("variable no declarada en runtime: '{name}'")))
        }
        Expr::FieldAccess { base, field } => {
            let base_v = eval_expr(base, env, db, fns)?;
            match base_v {
                Value::Struct(fields) | Value::Variant { fields, .. } => fields
                    .into_iter()
                    .find(|(n, _)| n == field)
                    .map(|(_, v)| v)
                    .ok_or_else(|| err(format!("no existe el campo '{field}'"))),
                Value::Db => Ok(Value::DbCollection(field.clone())),
                Value::DbCollection(_) | Value::List(_) => {
                    Ok(Value::BoundMethod(Box::new(base_v), field.clone()))
                }
                other => Err(err(format!("no se puede acceder al campo '{field}' sobre {other:?}"))),
            }
        }
        Expr::Call { callee, args } => {
            // Llamada a una `fn` de usuario -- se resuelve por nombre ANTES
            // de evaluar `callee` genéricamente, porque una fn no tiene un
            // Value propio (a diferencia de db.*, que sí usa BoundMethod).
            if let Expr::Ident(name) = &**callee {
                if let Some(decl) = fns.get(name.as_str()) {
                    let arg_vs = eval_args(args, env, db, fns)?;
                    let mut fn_env = Env::new();
                    for (p, v) in decl.params.iter().zip(arg_vs) {
                        fn_env.insert(p.name.clone(), v);
                    }
                    return eval_block(&decl.body, &fn_env, db, fns);
                }
            }
            let callee_v = eval_expr(callee, env, db, fns)?;
            let arg_vs = eval_args(args, env, db, fns)?;
            match callee_v {
                Value::BoundMethod(receiver, method) => call_method(*receiver, &method, arg_vs, db),
                other => Err(err(format!("no se puede llamar un valor {other:?}"))),
            }
        }
        Expr::StructLit { variant, fields, .. } => {
            let evaluated = fields
                .iter()
                .map(|(k, e)| Ok((k.clone(), eval_expr(e, env, db, fns)?)))
                .collect::<Result<Vec<_>, RuntimeError>>()?;
            match variant {
                Some(v) => Ok(Value::Variant { variant: v.clone(), fields: evaluated }),
                None => Ok(Value::Struct(evaluated)),
            }
        }
        Expr::Match { scrutinee, arms } => {
            let v = eval_expr(scrutinee, env, db, fns)?;
            for arm in arms {
                if let Some(bindings) = try_match_pattern(&arm.pattern, &v) {
                    let mut arm_env = env.clone();
                    arm_env.extend(bindings);
                    return match &arm.body {
                        MatchArmBody::Expr(e) => eval_expr(e, &arm_env, db, fns),
                        MatchArmBody::Block(b) => eval_block(b, &arm_env, db, fns),
                    };
                }
            }
            // No debería pasar: el checker ya garantizó exhaustividad
            // (GRAMMAR.md §3.3) antes de que este código llegara a ejecutarse.
            Err(err("ningún arm de match coincidió — el checker debería haber impedido esto"))
        }
        Expr::If { cond, then_block, else_block } => {
            let c = eval_expr(cond, env, db, fns)?;
            match c {
                Value::Bool(true) => eval_block(then_block, env, db, fns),
                Value::Bool(false) => eval_block(else_block, env, db, fns),
                other => Err(err(format!("la condición de 'if' no es Bool en runtime: {other:?}"))),
            }
        }
        Expr::Binary { op, left, right } => eval_binary(*op, left, right, env, db, fns),
        Expr::Unary { op, operand } => eval_unary(*op, operand, env, db, fns),
    }
}

fn eval_binary(op: BinaryOp, left: &Expr, right: &Expr, env: &Env, db: &Db, fns: &Fns) -> Result<Value, RuntimeError> {
    use BinaryOp::*;
    // && / || cortocircuitan: el lado derecho no se evalúa si ya se sabe el
    // resultado, igual que en cualquier lenguaje con estos operadores.
    if matches!(op, And | Or) {
        let l = as_bool(&eval_expr(left, env, db, fns)?)?;
        return match (op, l) {
            (And, false) => Ok(Value::Bool(false)),
            (Or, true) => Ok(Value::Bool(true)),
            _ => Ok(Value::Bool(as_bool(&eval_expr(right, env, db, fns)?)?)),
        };
    }

    let l = eval_expr(left, env, db, fns)?;
    let r = eval_expr(right, env, db, fns)?;
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

fn eval_unary(op: UnaryOp, operand: &Expr, env: &Env, db: &Db, fns: &Fns) -> Result<Value, RuntimeError> {
    let v = eval_expr(operand, env, db, fns)?;
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

fn eval_args(args: &[Expr], env: &Env, db: &Db, fns: &Fns) -> Result<Vec<Value>, RuntimeError> {
    args.iter().map(|a| eval_expr(a, env, db, fns)).collect()
}

fn try_match_pattern(pattern: &Pattern, v: &Value) -> Option<Vec<(String, Value)>> {
    match pattern {
        Pattern::Bind(name) => Some(vec![(name.clone(), v.clone())]),
        Pattern::Variant { variant_name, fields, .. } => {
            let Value::Variant { variant, fields: value_fields } = v else {
                return None;
            };
            if variant != variant_name {
                return None;
            }
            let mut bindings = Vec::new();
            if let Some(field_patterns) = fields {
                for fp in field_patterns {
                    let field_v = value_fields.iter().find(|(n, _)| n == &fp.name).map(|(_, v)| v)?;
                    bindings.extend(try_match_pattern(&fp.pattern, field_v)?);
                }
            }
            Some(bindings)
        }
    }
}

fn call_method(receiver: Value, method: &str, args: Vec<Value>, db: &Db) -> Result<Value, RuntimeError> {
    match receiver {
        Value::DbCollection(coll) => db.call(&coll, method, args),
        Value::List(items) => match method {
            "take" => {
                let n = as_int(args.first().ok_or_else(|| err("take requiere 1 argumento"))?)? as usize;
                Ok(Value::List(items.into_iter().take(n).collect()))
            }
            other => Err(err(format!("método de lista desconocido: '{other}'"))),
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

    let empty = serde_json::Map::new();
    let args_obj = args_json.as_object().unwrap_or(&empty);
    let mut env = Env::new();
    for p in &rpc.params {
        let v = match args_obj.get(&p.name) {
            Some(j) => json_to_value(j),
            None => match &p.default {
                Some(default_expr) => eval_expr(default_expr, &Env::new(), db, &fns)?,
                None => Value::Null,
            },
        };
        env.insert(p.name.clone(), v);
    }

    let result = eval_block(&rpc.body, &env, db, &fns)?;
    Ok(value_to_json(&result))
}

pub fn value_to_json(v: &Value) -> serde_json::Value {
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
                m.insert(k.clone(), value_to_json(v));
            }
            serde_json::Value::Object(m)
        }
        Value::Variant { variant, fields } => {
            let mut m = serde_json::Map::new();
            m.insert("type".to_string(), json!(variant));
            for (k, v) in fields {
                m.insert(k.clone(), value_to_json(v));
            }
            serde_json::Value::Object(m)
        }
        Value::List(items) => serde_json::Value::Array(items.iter().map(value_to_json).collect()),
        // Salvaguarda: estos marcadores son internos del intérprete y nunca
        // deberían ser el resultado final de un rpc (ver eval_expr::Call).
        Value::Db | Value::DbCollection(_) | Value::BoundMethod(_, _) => serde_json::Value::Null,
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
        assert_eq!(result["error"]["min"], json!(1));
    }
}
