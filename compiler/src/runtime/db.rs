// "Base de datos" en memoria -- "DB tipada" v0 (GRAMMAR.md §2.1): el
// checker ahora conoce la forma real de cada colección declarada en
// `db { ... }` (Type::DbCollection en vez de Dynamic), pero acá el
// runtime sigue siendo puramente en memoria, sin ningún driver SQL real
// -- eso queda fuera de alcance a propósito (ver PLAN.md §4, Fase 2).

use super::{as_int, simple_enum_names, value_to_json, RuntimeError, Value};
use crate::ast::{Item, Program};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::Mutex;

/// Cuántos eventos sin consumir tolera un suscriptor de push real
/// (GRAMMAR.md §3.16) antes de ser desconectado. Un canal ILIMITADO sería
/// un vector real de agotamiento de memoria si un cliente se queda lento
/// (o la conexión se cuelga) sin cerrarse -- `try_send` (nunca bloqueante,
/// ver `publish`) más este tope acotan el costo de un suscriptor colgado a
/// una cantidad fija, a costa de desconectarlo si se atrasa demasiado (el
/// mismo trade-off que la mayoría de sistemas de pub-sub/broadcast reales
/// hacen). No es un número investigado a fondo, es un default razonable.
const LIVE_STREAM_BUFFER: usize = 1024;

pub struct Db {
    collections: HashMap<String, Mutex<Vec<Value>>>,
    /// Para que un evento PUBLICADO (`publish`, más abajo) serialice
    /// EXACTAMENTE igual que cualquier respuesta normal del mismo programa
    /// (mismo `value_to_json` que usa `invoke_rpc_with_sessions`).
    simple_enums: HashSet<String>,
    /// Suscriptores activos por colección, para push real (GRAMMAR.md
    /// §3.16). `RefCell`, no `Mutex` -- por la MISMA razón que ya vale para
    /// `SessionStore`: esto solo se toca desde el hilo principal
    /// (`Db::call`/`Db::subscribe`), nunca desde ningún hilo escritor (que
    /// solo recibe el `Receiver`, ya extraído, nunca vuelve a tocar `Db`).
    subscribers: RefCell<HashMap<String, Vec<SyncSender<serde_json::Value>>>>,
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
        Db {
            collections,
            simple_enums: simple_enum_names(program),
            subscribers: RefCell::new(HashMap::new()),
        }
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
        // `simple_enums` queda vacío a propósito: este fixture no tiene un
        // `&Program` real del que derivarlo. Límite conocido: un test que
        // ejercite `publish` (push real) contra datos sembrados acá vería
        // un enum como `{"type":"Admin"}` en vez del string pelado que
        // `linkc serve` produciría de verdad -- los tests de pub-sub de
        // esta ronda usan `Db::new(&program)` con un programa real en vez
        // de este fixture, precisamente para evitar ese hueco.
        Db {
            collections,
            simple_enums: HashSet::new(),
            subscribers: RefCell::new(HashMap::new()),
        }
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
                // DESPUÉS de que la fila ya está firme en `rows`, nunca
                // antes -- publicar una mutación que en realidad falló más
                // adelante sería anunciar algo que no pasó. Todos los pasos
                // falibles de este arm ya corrieron arriba.
                self.publish(collection, &v);
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
                self.publish(collection, user);
                Ok(user.clone())
            }
            other => Err(RuntimeError::new(format!("método desconocido: 'db.{collection}.{other}'"))),
        }
    }

    /// Nuevo suscriptor de TODA mutación futura (`insert`/`applyPatch`) de
    /// `collection`, más una foto de lo que ya hay ADENTRO en este mismo
    /// instante (GRAMMAR.md §3.16, push real v0). Nunca lo llama el
    /// intérprete -- solo `runtime/server.rs`, directo sobre el `&Db` que
    /// ya tiene, ANTES de decidir si invoca `invoke_rpc_with_sessions` (ver
    /// `ast::recognize_live_subscribe`).
    ///
    /// Sacar la foto y registrarse son las dos líneas de ESTA MISMA llamada
    /// sincrónica, sin ningún punto de suspensión entre ellas -- y la única
    /// otra cosa que podría "colarse" (una mutación) solo pasa dentro de
    /// `Db::call`, en el mismo único hilo que corre esto. El servidor entero
    /// procesa una request a la vez en ese hilo, así que no hay forma de
    /// que una mutación se intercale entre las dos líneas de acá: el
    /// single-threading del servidor ES el lock, no hace falta agregar uno.
    pub fn subscribe(&self, collection: &str) -> Result<(Vec<serde_json::Value>, Receiver<serde_json::Value>), RuntimeError> {
        let cell = self
            .collections
            .get(collection)
            .ok_or_else(|| RuntimeError::new(format!("colección desconocida: '{collection}'")))?;
        let snapshot: Vec<serde_json::Value> = {
            let rows = cell.lock().expect("lock de db envenenado");
            rows.iter().map(|v| value_to_json(v, &self.simple_enums)).collect()
        };
        let (tx, rx) = mpsc::sync_channel(LIVE_STREAM_BUFFER);
        self.subscribers.borrow_mut().entry(collection.to_string()).or_default().push(tx);
        Ok((snapshot, rx))
    }

    /// Llamado SOLO desde el final de los arms `"insert"`/`"applyPatch"` de
    /// `call`, DESPUÉS de que la fila ya está firme en `rows` -- nunca
    /// antes, para no anunciar una mutación que en realidad falló más
    /// adelante (ambos arms ya tienen todos sus pasos falibles ANTES de la
    /// mutación real).
    fn publish(&self, collection: &str, row: &Value) {
        let json = value_to_json(row, &self.simple_enums);
        let mut subs = self.subscribers.borrow_mut();
        if let Some(list) = subs.get_mut(collection) {
            // `try_send` -- NUNCA bloqueante: publicar no puede colgar el
            // único hilo que atiende todas las requests, ni siquiera si un
            // suscriptor está lento. `Full` (suscriptor demasiado atrasado,
            // ver LIVE_STREAM_BUFFER) o `Disconnected` (el cliente ya se
            // fue) se podan igual -- lazy, recién en la próxima publicación
            // a esta colección, no eager (un mecanismo eager necesitaría un
            // hilo aparte tocando `Db`, reabriendo la pregunta de Send/Sync
            // que todo este diseño evita).
            list.retain(|tx| tx.try_send(json.clone()).is_ok());
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
