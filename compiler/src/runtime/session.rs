//! Auth v0 (GRAMMAR.md §3.14): sesión opaca en memoria + roles. Sin JWT, y
//! sin ninguna dependencia nueva PARA ESTA FEATURE puntual -- el criterio de
//! "sin agregar dependencias" no rige para todo el proyecto (la persistencia
//! real de `db`, GRAMMAR.md §3.17, sí trajo una -- `rusqlite` -- a propósito
//! y de forma explícita), pero auth v0 en particular se resolvió entero con
//! lo que tiny_http+serde_json ya daban.

use std::cell::RefCell;
use std::collections::HashMap;

/// 128 bits genuinamente sembrados por el SO, vía un truco de hilo
/// descartable -- NO un CSPRNG auditado, alcanza para v0/demo.
///
/// `std::collections::hash_map::RandomState` cachea sus keys (k0,k1) POR
/// HILO: la primera vez que se pide en un hilo dado, lee del SO; cada
/// llamada SUBSIGUIENTE en ESE MISMO hilo solo incrementa k0 en 1 -- k1
/// nunca cambia. Como el intérprete de c-script corre siempre en el hilo
/// principal (single-threaded por diseño, ver runtime/server.rs), llamar
/// `RandomState::new()` repetidas veces ahí NO da un secreto nuevo por
/// llamada: da un único secreto fijado una vez al arrancar el proceso,
/// reusado con un contador chico encima -- insuficiente para un token de
/// sesión (~0 bits de entropía nueva por token, no ~128 como el nombre del
/// tipo podría sugerir). Este fue el hallazgo de seguridad central de la
/// ronda de auth: dos revisores adversariales llegaron, cada uno por su
/// cuenta, a esta misma causa raíz.
///
/// Un hilo RECIÉN CREADO nunca inicializó ese cache thread-local, así que
/// su PRIMER `RandomState::new()` sí pega contra el RNG real del SO
/// (`BCryptGenRandom`/`ProcessPrng` en Windows, `getrandom(2)` en Linux).
/// Acá se fuerza exactamente eso: se crea un hilo descartable, y DENTRO de
/// él se derivan 2 hashes de 64 bits de la MISMA `RandomState` (sin volver a
/// llamar `::new()`, que reincidiría en el problema) -- ambos derivados de
/// las mismas (k0,k1) frescas, mezclados vía SipHash con un discriminador
/// distinto cada uno. El costo del spawn+join (decenas de microsegundos) es
/// irrelevante para una operación de login. Una implementación real
/// necesitaría el crate `rand`/`getrandom`.
fn fresh_128_bits() -> (u64, u64) {
    std::thread::spawn(|| {
        use std::collections::hash_map::RandomState;
        use std::hash::{BuildHasher, Hasher};
        let rs = RandomState::new();
        let mut h1 = rs.build_hasher();
        h1.write_u8(1);
        let mut h2 = rs.build_hasher();
        h2.write_u8(2);
        (h1.finish(), h2.finish())
    })
    .join()
    .expect("el hilo de generación de tokens no debería poder paniquear")
}

fn fresh_token() -> String {
    let (a, b) = fresh_128_bits();
    format!("{a:016x}{b:016x}")
}

/// Sesiones en memoria: token opaco -> rol (nombre de enum + variante). Se
/// guarda el rol como par de `String`, NO el `Value` completo del intérprete
/// -- evita cualquier discusión sobre Send/Sync de `Value` (hoy no es Send
/// por `Value::Closure`, que contiene `Rc`) y es exactamente lo mismo que ya
/// se serializa al wire para un enum simple.
///
/// `RefCell`, no `Mutex` (a diferencia de `Db`): todo esto corre siempre en
/// el hilo principal (mismo argumento que ya vale para `Env`/`Db` en este
/// intérprete), y un re-lock accidental con `RefCell` panica con un mensaje
/// claro en vez del comportamiento "unspecified" que documenta
/// `std::sync::Mutex` para un re-lock del mismo hilo.
pub struct SessionStore {
    sessions: RefCell<HashMap<String, (String, String)>>,
}

impl SessionStore {
    pub fn new() -> Self {
        SessionStore { sessions: RefCell::new(HashMap::new()) }
    }

    pub fn create(&self, enum_name: String, variant_name: String) -> String {
        let token = fresh_token();
        self.sessions.borrow_mut().insert(token.clone(), (enum_name, variant_name));
        token
    }

    /// Idempotente -- destruir una sesión que ya no existe (o nunca existió)
    /// no es un error.
    pub fn destroy(&self, token: &str) {
        self.sessions.borrow_mut().remove(token);
    }

    pub fn role_for(&self, token: &str) -> Option<(String, String)> {
        self.sessions.borrow().get(token).cloned()
    }
}

impl Default for SessionStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_then_role_for_round_trips() {
        let store = SessionStore::new();
        let token = store.create("Role".to_string(), "Admin".to_string());
        assert_eq!(store.role_for(&token), Some(("Role".to_string(), "Admin".to_string())));
    }

    #[test]
    fn unknown_token_has_no_role() {
        let store = SessionStore::new();
        assert_eq!(store.role_for("no-existe"), None);
    }

    #[test]
    fn destroy_removes_the_session() {
        let store = SessionStore::new();
        let token = store.create("Role".to_string(), "Admin".to_string());
        store.destroy(&token);
        assert_eq!(store.role_for(&token), None);
    }

    #[test]
    fn destroying_an_unknown_token_does_not_panic() {
        let store = SessionStore::new();
        store.destroy("no-existe"); // no debería paniquear
    }

    #[test]
    fn tokens_are_128_bits_of_hex_and_distinct_across_many_calls() {
        // No prueba "es criptográficamente seguro" (no se puede probar con
        // un test) -- prueba las dos propiedades mecánicas que si fallaran
        // serían un bug real: longitud (32 hex = 128 bits) y que llamadas
        // repetidas no devuelvan siempre el mismo valor.
        let store = SessionStore::new();
        let tokens: Vec<String> =
            (0..20).map(|_| store.create("Role".to_string(), "Admin".to_string())).collect();
        for t in &tokens {
            assert_eq!(t.len(), 32, "token debería ser 32 caracteres hex (128 bits): {t}");
            assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
        }
        let unique: std::collections::HashSet<&String> = tokens.iter().collect();
        assert_eq!(unique.len(), tokens.len(), "20 tokens generados no deberían tener duplicados");
    }
}
