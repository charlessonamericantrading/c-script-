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

    /// Fixture SOLO para tests y para el demo -- **no** es lo que usa
    /// `linkc serve` (ver `runtime/server.rs`, que usa `Db::new`). Siembra
    /// los mismos dos usuarios de siempre bajo la colección "users", así
    /// los tests no repiten la siembra a mano cada vez.
    ///
    /// Ojo con la forma de los valores: tienen que ser EXACTAMENTE los que
    /// produciría el propio lenguaje al construirlos, no una aproximación
    /// escrita a mano. De ahí que `role` sea un `Value::Variant` real y no
    /// un `Value::Str("Admin".into())`: las dos cosas serializan igual al wire
    /// (un enum simple sale como string plano, GRAMMAR.md §4), pero se
    /// comparan distinto con `==` contra un `Role.Admin {}` construido en
    /// el backend o recibido por el wire -- que es justo el bug de
    /// "dos representaciones internas del mismo valor del contrato" que la
    /// auditoría encontró y que la validación tipada del borde eliminó.
    pub fn seeded() -> Self {
        let role = |variant: &str| Value::Variant {
            enum_name: "Role".to_string(),
            variant: variant.to_string(),
            fields: Vec::new(),
        };
        let mut collections = HashMap::new();
        collections.insert(
            "users".to_string(),
            Mutex::new(vec![
                Value::Struct(vec![
                    ("id".into(), Value::Int(1)),
                    ("name".into(), Value::Str("Ada Lovelace".into())),
                    ("email".into(), Value::Str("ada@example.com".into())),
                    ("role".into(), role("Admin")),
                    ("bio".into(), Value::Str("Pionera de la programación".into())),
                    ("deletedAt".into(), Value::Null),
                ]),
                Value::Struct(vec![
                    ("id".into(), Value::Int(2)),
                    ("name".into(), Value::Str("Grace Hopper".into())),
                    ("email".into(), Value::Str("grace@example.com".into())),
                    ("role".into(), role("Member")),
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
            .ok_or_else(|| RuntimeError::new(format!("colección desconocida: '{collection}'")))?;
        let mut rows = cell.lock().expect("lock de db envenenado");
        match method {
            "all" => Ok(Value::List(rows.clone())),
            "find" => {
                let id = as_int(
                    args.first()
                        .ok_or_else(|| RuntimeError::new("find requiere 1 argumento"))?,
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
                    .ok_or_else(|| RuntimeError::new("insert requiere 1 argumento"))?;
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
                        .ok_or_else(|| RuntimeError::new("applyPatch requiere 2 argumentos"))?,
                )?;
                let patch = it
                    .next()
                    .ok_or_else(|| RuntimeError::new("applyPatch requiere 2 argumentos"))?;
                let Value::Struct(patch_fields) = patch else {
                    return Err(RuntimeError::new("applyPatch: el patch debe ser un objeto"));
                };
                let user = rows
                    .iter_mut()
                    .find(|u| field_int(u, "id") == Some(id))
                    // "usuario" era un resto de cuando la db solo tenía la
                    // colección "users" hardcodeada -- el mensaje mentía
                    // para cualquier otra colección.
                    .ok_or_else(|| RuntimeError::new(format!("no hay ningún elemento con id {id} en '{collection}'")))?;
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
            other => Err(RuntimeError::new(format!("método desconocido: 'db.{collection}.{other}'"))),
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
