//! Estado en memoria para `@idempotent` (GRAMMAR.md §3.140, PLAN.md §9.3).
//!
//! Mismo criterio y mismo modelo de concurrencia que `rate_limit.rs`: una
//! sola instancia por proceso servidor, mutada request a request en el hilo
//! principal (GRAMMAR.md §3.9 -- `linkc serve` es single-threaded a
//! propósito) -- no hace falta `Mutex`, nunca se accede desde otro hilo, y
//! no sobrevive un restart (mismo límite que el rate limiter: aceptable acá
//! porque una `Idempotency-Key` reintentada DESPUÉS de un restart del
//! servidor simplemente vuelve a ejecutar el rpc, ni peor ni mejor que sin
//! esta feature).
//!
//! La clave es (service, rpc, idempotency_key) -- nunca solo la key, para
//! que dos rpcs distintos nunca compartan namespace por casualidad si un
//! caller reusa el mismo string de clave en llamadas a endpoints
//! diferentes.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Resultado de mirar el store ANTES de correr el cuerpo del rpc.
pub enum Lookup {
    /// Primera vez que se ve esta clave para este (service, rpc) -- correr
    /// el rpc como siempre.
    Miss,
    /// Ya se vio esta clave con el MISMO hash de request -- repetir esta
    /// respuesta tal cual, sin correr el cuerpo de nuevo.
    Hit { status: u16, body: String, content_type: String },
    /// Ya se vio esta clave, pero con un hash de request DISTINTO -- el
    /// caller está reusando una `Idempotency-Key` para una operación
    /// distinta, casi siempre un bug del lado cliente (una clave generada
    /// una sola vez y reusada entre requests que deberían ser independientes).
    Conflict,
}

struct Entry {
    request_hash: String,
    status: u16,
    body: String,
    content_type: String,
    stored_at: Instant,
}

/// Una entrada vive como mucho 24hs -- suficiente para cubrir el caso real
/// (un cliente reintentando tras un timeout/desconexión, minutos u horas
/// después, no días), sin dejar crecer el mapa para siempre en un proceso
/// de larga vida. Mismo orden de magnitud que Stripe (que documenta 24hs
/// para sus propias `Idempotency-Key`) -- no un número inventado.
const ENTRY_TTL: Duration = Duration::from_secs(24 * 3600);

/// Mismo criterio que `rate_limit::SWEEP_EVERY`/`SWEEP_MAX_IDLE`: barrer
/// entradas vencidas cada tantos checks en vez de en cada uno, para no
/// pesar en el hot path de una request que sí matchea.
const SWEEP_EVERY: u32 = 1000;

pub struct IdempotencyStore {
    entries: HashMap<(String, String, String), Entry>,
    checks_since_sweep: u32,
}

impl IdempotencyStore {
    pub fn new() -> Self {
        IdempotencyStore { entries: HashMap::new(), checks_since_sweep: 0 }
    }

    fn sweep_if_due(&mut self, now: Instant) {
        self.checks_since_sweep += 1;
        if self.checks_since_sweep < SWEEP_EVERY {
            return;
        }
        self.checks_since_sweep = 0;
        if let Some(cutoff) = now.checked_sub(ENTRY_TTL) {
            self.entries.retain(|_, e| e.stored_at >= cutoff);
        }
    }

    /// Mira el store para (service, rpc, key) con el hash de ESTA request.
    /// Una entrada vencida (más de `ENTRY_TTL` desde que se grabó) cuenta
    /// como `Miss`, no como `Hit` -- se recalcula fresca la próxima vez que
    /// se grabe, mismo criterio que el sweep periódico usa para decidir qué
    /// tirar.
    pub fn lookup(&mut self, service: &str, rpc: &str, key: &str, request_hash: &str) -> Lookup {
        let now = Instant::now();
        self.sweep_if_due(now);
        let Some(entry) = self.entries.get(&(service.to_string(), rpc.to_string(), key.to_string())) else {
            return Lookup::Miss;
        };
        if now.saturating_duration_since(entry.stored_at) > ENTRY_TTL {
            return Lookup::Miss;
        }
        if entry.request_hash != request_hash {
            return Lookup::Conflict;
        }
        Lookup::Hit { status: entry.status, body: entry.body.clone(), content_type: entry.content_type.clone() }
    }

    /// Graba el resultado de una ejecución EXITOSA -- el caller (`server.rs`)
    /// es responsable de solo llamar acá para un status 2xx; un error no se
    /// graba, para que el caller pueda corregir y reintentar con la misma
    /// clave (mismo criterio que Stripe: una `Idempotency-Key` protege
    /// contra un DUPLICADO de una operación que funcionó, no contra
    /// reintentar una que falló).
    #[allow(clippy::too_many_arguments)]
    pub fn store(&mut self, service: &str, rpc: &str, key: &str, request_hash: &str, status: u16, body: String, content_type: String) {
        self.entries.insert(
            (service.to_string(), rpc.to_string(), key.to_string()),
            Entry { request_hash: request_hash.to_string(), status, body, content_type, stored_at: Instant::now() },
        );
    }
}

impl Default for IdempotencyStore {
    fn default() -> Self {
        Self::new()
    }
}

/// SHA-256 hex del body crudo de la request -- mismo algoritmo y mismo
/// formato hex (`{b:02x}`) que `crypto.hashSha256` ya usa en
/// `runtime/mod.rs`, para no tener dos convenciones de hashing distintas en
/// el mismo binario.
pub fn hash_request_body(body: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(body.as_bytes());
    hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_never_seen_before_is_a_miss() {
        let mut store = IdempotencyStore::new();
        assert!(matches!(store.lookup("Orders", "create", "key-1", "hash-a"), Lookup::Miss));
    }

    #[test]
    fn a_stored_success_replays_on_the_same_key_and_hash() {
        let mut store = IdempotencyStore::new();
        store.store("Orders", "create", "key-1", "hash-a", 200, "{\"id\":1}".to_string(), "application/json".to_string());
        match store.lookup("Orders", "create", "key-1", "hash-a") {
            Lookup::Hit { status, body, content_type } => {
                assert_eq!(status, 200);
                assert_eq!(body, "{\"id\":1}");
                assert_eq!(content_type, "application/json");
            }
            _ => panic!("se esperaba un Hit"),
        }
    }

    #[test]
    fn the_same_key_with_a_different_request_hash_is_a_conflict() {
        let mut store = IdempotencyStore::new();
        store.store("Orders", "create", "key-1", "hash-a", 200, "ok".to_string(), "application/json".to_string());
        assert!(matches!(store.lookup("Orders", "create", "key-1", "hash-b"), Lookup::Conflict));
    }

    #[test]
    fn the_same_key_is_independent_per_service_and_rpc() {
        let mut store = IdempotencyStore::new();
        store.store("Orders", "create", "key-1", "hash-a", 200, "ok".to_string(), "application/json".to_string());
        // Mismo string de clave, otro rpc: MISS, no Conflict -- namespaces separados.
        assert!(matches!(store.lookup("Orders", "update", "key-1", "hash-a"), Lookup::Miss));
        // Mismo string de clave, otro service: también separado.
        assert!(matches!(store.lookup("Invoices", "create", "key-1", "hash-a"), Lookup::Miss));
    }

    #[test]
    fn hash_request_body_is_deterministic_and_sensitive_to_content() {
        assert_eq!(hash_request_body("{\"a\":1}"), hash_request_body("{\"a\":1}"));
        assert_ne!(hash_request_body("{\"a\":1}"), hash_request_body("{\"a\":2}"));
    }
}
