//! Auth v0 (GRAMMAR.md §3.14): sesión opaca en memoria + roles. Sin JWT.
//!
//! Se resolvió originalmente sin agregar dependencias, con lo que
//! tiny_http+serde_json ya daban; la única pieza que ese criterio dejaba
//! incómoda era la entropía del token (ver `fresh_128_bits`). Desde que
//! `crypto.hashPassword` pasó a Argon2id y el proyecto tomó `getrandom`, los
//! tokens de sesión salen del CSPRNG del sistema como corresponde.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

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

/// Una sesión guardada -- rol como par de `String`, NO el `Value` completo
/// del intérprete (evita cualquier discusión sobre Send/Sync de `Value`, hoy
/// no es Send por `Value::Closure` que contiene `Rc`, y es exactamente lo
/// mismo que ya se serializa al wire para un enum simple), más CUÁNDO expira
/// -- `None` significa "nunca" (comportamiento de siempre, sin `--session-
/// ttl`). Struct con nombres de campo, no una tupla más grande -- mismo
/// criterio que `RequestContext` (`db.rs`): más legible que posiciones.
struct SessionEntry {
    enum_name: String,
    variant_name: String,
    expires_at: Option<Instant>,
}

/// Sesiones en memoria: token opaco -> `SessionEntry`.
///
/// `RefCell`, no `Mutex` (a diferencia de `Db`): todo esto corre siempre en
/// el hilo principal (mismo argumento que ya vale para `Env`/`Db` en este
/// intérprete), y un re-lock accidental con `RefCell` panica con un mensaje
/// claro en vez del comportamiento "unspecified" que documenta
/// `std::sync::Mutex` para un re-lock del mismo hilo.
pub struct SessionStore {
    sessions: RefCell<HashMap<String, SessionEntry>>,
    /// Configurado UNA vez al construir el store (`--session-ttl`/
    /// `LINK_SESSION_TTL`, GRAMMAR.md §3.50) -- nunca cambia sesión a
    /// sesión, así que vivir en el store entero (no en cada `create`) es
    /// aditivo puro: todo caller existente que no pasa TTL (`linkc test`,
    /// los tests unitarios de este archivo) sigue viendo `None`, sin
    /// cambiar una sola firma.
    ttl: Option<Duration>,
}

impl SessionStore {
    pub fn new() -> Self {
        SessionStore { sessions: RefCell::new(HashMap::new()), ttl: None }
    }

    /// Como `new()`, pero cada sesión que este store cree expira `ttl`
    /// después de `create()` -- GRAMMAR.md §3.50. Único caller real:
    /// `runtime::server::serve`, cuando `--session-ttl`/`LINK_SESSION_TTL`
    /// están configurados.
    pub fn with_ttl(ttl: Duration) -> Self {
        SessionStore { sessions: RefCell::new(HashMap::new()), ttl: Some(ttl) }
    }

    pub fn create(&self, enum_name: String, variant_name: String) -> String {
        let token = fresh_token();
        let expires_at = self.ttl.map(|ttl| Instant::now() + ttl);
        self.sessions.borrow_mut().insert(token.clone(), SessionEntry { enum_name, variant_name, expires_at });
        token
    }

    /// Idempotente -- destruir una sesión que ya no existe (o nunca existió)
    /// no es un error.
    pub fn destroy(&self, token: &str) {
        self.sessions.borrow_mut().remove(token);
    }

    /// `None` tanto para un token que nunca existió como para uno que ya
    /// expiró -- desde afuera de este módulo las dos cosas son
    /// indistinguibles a propósito (el mismo 401 "se requiere
    /// autenticación" para ambas, `check_auth_gate` en `runtime/server.rs`,
    /// nunca revela CUÁL de las dos pasó). Un token expirado se BORRA acá
    /// -- limpieza perezosa en el próximo acceso, no un barrido de fondo:
    /// este intérprete no tiene ningún hilo de mantenimiento (single-
    /// threaded por diseño), así que "en el próximo acceso" es la única
    /// oportunidad real de liberar la memoria de una sesión vencida sin
    /// inventar un timer aparte.
    pub fn role_for(&self, token: &str) -> Option<(String, String)> {
        let mut sessions = self.sessions.borrow_mut();
        let entry = sessions.get(token)?;
        if entry.expires_at.is_some_and(|exp| Instant::now() >= exp) {
            sessions.remove(token);
            return None;
        }
        Some((entry.enum_name.clone(), entry.variant_name.clone()))
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

    #[test]
    fn a_store_without_ttl_never_expires_a_session() {
        // Comportamiento de siempre, sin `--session-ttl`: `new()` sigue sin
        // TTL -- esta ronda no le cambia nada a nadie que no pida esto
        // explícitamente.
        let store = SessionStore::new();
        let token = store.create("Role".to_string(), "Admin".to_string());
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(store.role_for(&token), Some(("Role".to_string(), "Admin".to_string())));
    }

    #[test]
    fn a_session_is_valid_before_the_ttl_and_gone_after_it() {
        let store = SessionStore::with_ttl(Duration::from_millis(30));
        let token = store.create("Role".to_string(), "Admin".to_string());
        assert_eq!(
            store.role_for(&token),
            Some(("Role".to_string(), "Admin".to_string())),
            "recién creada, todavía no expiró"
        );
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(store.role_for(&token), None, "pasado el TTL, la sesión ya no es válida");
    }

    #[test]
    fn an_expired_session_is_indistinguishable_from_one_that_never_existed() {
        // GRAMMAR.md §3.50: `check_auth_gate` da el mismo 401 para los dos
        // casos -- el contrato real es que `role_for` tampoco los
        // distinga, así que un caller no puede armar ese distingo por
        // accidente comparando `Option` contra algo más específico.
        let store = SessionStore::with_ttl(Duration::from_millis(10));
        let token = store.create("Role".to_string(), "Admin".to_string());
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(store.role_for(&token), store.role_for("un-token-que-nunca-existio"));
    }
}
