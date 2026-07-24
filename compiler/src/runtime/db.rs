// "Base de datos" en memoria, solo para que la demo E2E responda con datos
// reales. NO es el modelo de datos del lenguaje — eso es "DB tipada",
// explícitamente Fase 2 en PLAN.md §4. `db` es tratado como Dynamic/unknown
// tanto por el checker (checker.rs) como acá: el runtime solo reconoce un
// puñado fijo de métodos por convención (all/find/insert/applyPatch/take).

use super::{as_int, RuntimeError, Value};
use std::sync::Mutex;

pub struct Db {
    users: Mutex<Vec<Value>>,
}

impl Db {
    pub fn seeded() -> Self {
        Db {
            users: Mutex::new(vec![
                Value::Struct(vec![
                    ("id".into(), Value::Int(1)),
                    ("name".into(), Value::Str("Ada Lovelace".into())),
                    ("email".into(), Value::Str("ada@example.com".into())),
                    ("role".into(), Value::Str("Admin".into())),
                    ("bio".into(), Value::Str("Pionera de la programación".into())),
                    ("deletedAt".into(), Value::Null),
                ]),
                Value::Struct(vec![
                    ("id".into(), Value::Int(2)),
                    ("name".into(), Value::Str("Grace Hopper".into())),
                    ("email".into(), Value::Str("grace@example.com".into())),
                    ("role".into(), Value::Str("Member".into())),
                    ("bio".into(), Value::Null),
                    ("deletedAt".into(), Value::Null),
                ]),
            ]),
        }
    }

    pub fn call(&self, collection: &str, method: &str, args: Vec<Value>) -> Result<Value, RuntimeError> {
        if collection != "users" {
            return Err(RuntimeError(format!("colección desconocida: '{collection}'")));
        }
        let mut users = self.users.lock().expect("lock de db envenenado");
        match method {
            "all" => Ok(Value::List(users.clone())),
            "find" => {
                let id = as_int(
                    args.first()
                        .ok_or_else(|| RuntimeError("find requiere 1 argumento".into()))?,
                )?;
                Ok(users
                    .iter()
                    .find(|u| field_int(u, "id") == Some(id))
                    .cloned()
                    .unwrap_or(Value::Null))
            }
            "insert" => {
                let mut v = args
                    .into_iter()
                    .next()
                    .ok_or_else(|| RuntimeError("insert requiere 1 argumento".into()))?;
                let new_id = users.len() as i64 + 1;
                if let Value::Struct(fields) = &mut v {
                    fields.insert(0, ("id".to_string(), Value::Int(new_id)));
                    if !fields.iter().any(|(n, _)| n == "deletedAt") {
                        fields.push(("deletedAt".to_string(), Value::Null));
                    }
                }
                users.push(v.clone());
                Ok(v)
            }
            "applyPatch" => {
                let mut it = args.into_iter();
                let id = as_int(
                    &it.next()
                        .ok_or_else(|| RuntimeError("applyPatch requiere 2 argumentos".into()))?,
                )?;
                let patch = it
                    .next()
                    .ok_or_else(|| RuntimeError("applyPatch requiere 2 argumentos".into()))?;
                let Value::Struct(patch_fields) = patch else {
                    return Err(RuntimeError("applyPatch: el patch debe ser un objeto".into()));
                };
                let user = users
                    .iter_mut()
                    .find(|u| field_int(u, "id") == Some(id))
                    .ok_or_else(|| RuntimeError(format!("usuario {id} no encontrado")))?;
                if let Value::Struct(fields) = user {
                    for (k, v) in patch_fields {
                        match fields.iter_mut().find(|(n, _)| *n == k) {
                            Some(slot) => slot.1 = v,
                            None => fields.push((k, v)),
                        }
                    }
                }
                Ok(user.clone())
            }
            other => Err(RuntimeError(format!("método desconocido: 'db.{collection}.{other}'"))),
        }
    }
}

fn field_int(v: &Value, name: &str) -> Option<i64> {
    match v {
        Value::Struct(fields) => fields.iter().find(|(n, _)| n == name).and_then(|(_, v)| match v {
            Value::Int(n) => Some(*n),
            _ => None,
        }),
        _ => None,
    }
}
