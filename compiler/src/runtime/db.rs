// "Base de datos" en memoria -- "DB tipada" v0 (GRAMMAR.md §2.1): el
// checker ahora conoce la forma real de cada colección declarada en
// `db { ... }` (Type::DbCollection en vez de Dynamic), pero acá el
// runtime sigue siendo puramente en memoria, sin ningún driver SQL real
// -- eso queda fuera de alcance a propósito (ver PLAN.md §4, Fase 2).

use super::{as_int, RuntimeError, Value};
use crate::ast::{Item, Program};
use std::collections::HashMap;
use std::sync::Mutex;

pub struct Db {
    collections: HashMap<String, Mutex<Vec<Value>>>,
}

impl Db {
    /// Una colección vacía por cada una declarada en `db { ... }` -- el uso
    /// real (no de tests/demo). Si el programa no declara ninguna `db`, el
    /// mapa queda vacío; cualquier acceso a una colección ya lo habría
    /// rechazado el checker antes de que esto se ejecute.
    pub fn new(program: &Program) -> Self {
        let mut collections = HashMap::new();
        for item in &program.items {
            if let Item::Db(db) = item {
                for coll in &db.collections {
                    collections.insert(coll.name.clone(), Mutex::new(Vec::new()));
                }
            }
        }
        Db { collections }
    }

    /// Conveniencia para tests/demo: siembra los mismos dos usuarios de
    /// siempre bajo la colección "users" -- así los tests existentes (y el
    /// demo E2E) no necesitan repetir la siembra a mano cada vez.
    pub fn seeded() -> Self {
        let mut collections = HashMap::new();
        collections.insert(
            "users".to_string(),
            Mutex::new(vec![
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
                    // 'bio' se OMITE del todo -- `bio?: String` es opcional
                    // por CLAVE (ausente = "no tiene"), no nullable (GRAMMAR.md
                    // §3.4). Un bug real y preexistente (nunca antes se validó
                    // el shape de wire de verdad) tenía acá `Value::Null`, que
                    // es la forma correcta de `deletedAt: String?` (clave
                    // siempre presente, valor nullable) -- no la de `bio?`.
                    ("deletedAt".into(), Value::Null),
                ]),
            ]),
        );
        Db { collections }
    }

    pub fn call(&self, collection: &str, method: &str, args: Vec<Value>) -> Result<Value, RuntimeError> {
        let cell = self
            .collections
            .get(collection)
            .ok_or_else(|| RuntimeError(format!("colección desconocida: '{collection}'")))?;
        let mut rows = cell.lock().expect("lock de db envenenado");
        match method {
            "all" => Ok(Value::List(rows.clone())),
            "find" => {
                let id = as_int(
                    args.first()
                        .ok_or_else(|| RuntimeError("find requiere 1 argumento".into()))?,
                )?;
                Ok(rows
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
                let new_id = rows.len() as i64 + 1;
                if let Value::Struct(fields) = &mut v {
                    // El checker ya exige Omit<T,"id"> (checker.rs,
                    // check_db_method) -- un valor bien tipado no debería
                    // traer "id", pero subtipado estructural con más campos
                    // de los pedidos SÍ es válido (width subtyping,
                    // GRAMMAR.md §3.2), así que un `User` completo con su
                    // propio id igual podría llegar acá. Se descarta
                    // cualquier "id" preexistente antes de asignar uno
                    // fresco, para no terminar con dos campos "id".
                    fields.retain(|(n, _)| n != "id");
                    fields.insert(0, ("id".to_string(), Value::Int(new_id)));
                }
                rows.push(v.clone());
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
                let user = rows
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
