//! Auth v0 (GRAMMAR.md §3.14): sesión opaca en memoria + roles. Sin JWT.
//!
//! Se resolvió originalmente sin agregar dependencias, con lo que
//! tiny_http+serde_json ya daban; la única pieza que ese criterio dejaba
//! incómoda era la entropía del token (ver `fresh_128_bits`). Desde que
//! `crypto.hashPassword` pasó a Argon2id y el proyecto tomó `getrandom`, los
//! tokens de sesión salen del CSPRNG del sistema como corresponde.

use std::cell::RefCell;
use std::collections::HashMap;

/// 128 bits del CSPRNG del sistema (BCryptGenRandom/ProcessPrng en Windows,
/// getrandom(2) en Linux, random_get en WASI).
///
/// HISTORIA, porque el reemplazo importa: esto llamaba a
/// RandomState::new() para sacar entropia. El hallazgo central de la ronda de
/// auth fue que eso NO da un secreto nuevo por llamada -- RandomState cachea
/// (k0,k1) por hilo la primera vez y despues solo incrementa k0, asi que en el
/// hilo principal (el interprete es single-threaded por diseno, ver
/// runtime/server.rs) cada token nuevo aportaba ~0 bits de entropia, no ~128.
/// La solucion de entonces fue spawnear un hilo descartable por token, porque
/// un hilo recien creado si pega contra el RNG del SO en su primer
/// RandomState::new(); funcionaba, pero era un rodeo alrededor de no tener una
/// dependencia de RNG. Ahora que crypto.hashPassword necesita sales de verdad,
/// getrandom entro al proyecto y este rodeo sobra: se pide directo al SO.
fn fresh_128_bits() -> (u64, u64) {
    let mut buf = [0u8; 16];
    // Si el SO no puede dar entropia, la respuesta correcta es cortar: emitir un
    // token de sesion "por defecto" seria emitir uno adivinable.
    getrandom::getrandom(&mut buf)
        .expect("el sistema no pudo generar entropia para un token de sesion");
    (
        u64::from_le_bytes(buf[..8].try_into().unwrap()),
        u64::from_le_bytes(buf[8..].try_into().unwrap()),
    )
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
