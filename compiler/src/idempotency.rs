//! Estado en memoria para `@idempotent` (GRAMMAR.md §3.140, PLAN.md §9.3).
//!
//! Mismo criterio y mismo modelo de concurrencia que `rate_limit.rs`: una
//! sola instancia por proceso servidor. Desde GRAMMAR.md §3.158 (v1.114.0,
//! un hilo real por request) vive detrás de
//! `Arc<parking_lot::Mutex<IdempotencyStore>>` en `server.rs`, ya no mutada
//! desde un único hilo principal -- y no sobrevive un restart (mismo límite
//! que el rate limiter: aceptable acá porque una `Idempotency-Key`
//! reintentada DESPUÉS de un restart del servidor simplemente vuelve a
//! ejecutar el rpc, ni peor ni mejor que sin esta feature).
//!
//! La clave es (service, rpc, idempotency_key) -- nunca solo la key, para
//! que dos rpcs distintos nunca compartan namespace por casualidad si un
//! caller reusa el mismo string de clave en llamadas a endpoints
//! diferentes.
//!
//! GRAMMAR.md §3.166 (AUDIT-2026-08-27.md #4): hasta esa auditoría, "mirar
//! si la clave ya corrió" (`lookup`) y "grabar que corrió" (`store`) eran
//! dos adquisiciones de candado SEPARADAS, con el cuerpo entero del rpc
//! corriendo SIN ningún candado sostenido entre medio -- dos requests con
//! la misma `Idempotency-Key` casi simultáneas veían las dos un `Miss` y
//! las dos corrían el cuerpo, duplicando la escritura que la anotación
//! existe para impedir (confirmado en vivo: 30 requests concurrentes con la
//! misma clave insertaron 2 filas). `reserve` reemplaza a `lookup`: es una
//! única operación atómica de "revisar Y marcar en vuelo" bajo el MISMO
//! candado -- dos hilos concurrentes nunca pueden ver los dos `Reserved`
//! para la misma clave. El segundo (y cualquier otro) recibe `InFlight` en
//! vez de correr el cuerpo -- mismo criterio que la API real de Stripe, que
//! documenta un 409 explícito para una `Idempotency-Key` con una request ya
//! en vuelo, en vez de dejar correr dos ejecuciones concurrentes.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Resultado de `reserve` -- mira el store Y marca la clave en vuelo, las
/// dos cosas atómicamente bajo el mismo candado.
pub enum Lookup {
    /// Primera vez que se ve esta clave para este (service, rpc) -- la
    /// clave queda marcada EN VUELO (ver `Entry::InFlight`) y el caller
    /// corre el rpc como siempre. Tiene que llamar a `complete`/`release`
    /// cuando termine, sea cual sea el resultado.
    Reserved,
    /// Ya se vio esta clave con el MISMO hash de request y ya terminó --
    /// repetir esta respuesta tal cual, sin correr el cuerpo de nuevo.
    Hit { status: u16, body: String, content_type: String },
    /// Ya se vio esta clave, pero con un hash de request DISTINTO -- el
    /// caller está reusando una `Idempotency-Key` para una operación
    /// distinta, casi siempre un bug del lado cliente (una clave generada
    /// una sola vez y reusada entre requests que deberían ser independientes).
    Conflict,
    /// Otra request con esta MISMA clave está corriendo AHORA MISMO (todavía
    /// no llamó a `complete`/`release`) -- nunca correr el cuerpo acá, el
    /// caller tiene que esperar y reintentar.
    InFlight,
}

enum Entry {
    /// Una request reservó esta clave y todavía no terminó. `started_at` es
    /// lo que permite reclamar una entrada húerfana (ver `IN_FLIGHT_STALE_
    /// AFTER`) si el hilo que la reservó murió sin llamar a `complete`/
    /// `release` -- un panic en el cuerpo, por ejemplo.
    InFlight { request_hash: String, started_at: Instant },
    Done { request_hash: String, status: u16, body: String, content_type: String, stored_at: Instant },
}

/// Una entrada TERMINADA vive como mucho 24hs -- suficiente para cubrir el
/// caso real (un cliente reintentando tras un timeout/desconexión, minutos u
/// horas después, no días), sin dejar crecer el mapa para siempre en un
/// proceso de larga vida. Mismo orden de magnitud que Stripe (que documenta
/// 24hs para sus propias `Idempotency-Key`) -- no un número inventado.
const ENTRY_TTL: Duration = Duration::from_secs(24 * 3600);

/// Una entrada EN VUELO se considera huérfana (el hilo que la reservó murió
/// sin liberarla -- un panic, por ejemplo) después de este tiempo, mucho más
/// corto que `ENTRY_TTL`: cubre generosamente cualquier rpc real, incluyendo
/// uno con `http.*` lento contra el default de `--http-timeout` (30s), sin
/// dejar una clave bloqueada para siempre por un hilo que ya no existe.
const IN_FLIGHT_STALE_AFTER: Duration = Duration::from_secs(120);

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
        self.entries.retain(|_, e| match e {
            Entry::Done { stored_at, .. } => now.saturating_duration_since(*stored_at) <= ENTRY_TTL,
            Entry::InFlight { started_at, .. } => now.saturating_duration_since(*started_at) <= IN_FLIGHT_STALE_AFTER,
        });
    }

    /// Revisa el store para (service, rpc, key) con el hash de ESTA request
    /// Y, si no hay nada (o lo que había ya venció), la marca en vuelo --
    /// atómico bajo el mismo candado que el resto de `IdempotencyStore`, así
    /// que dos hilos concurrentes nunca pueden ver los dos `Reserved` para
    /// la misma clave. Quien recibe `Reserved` es responsable de llamar a
    /// `complete` (éxito) o `release` (falla) cuando termine.
    pub fn reserve(&mut self, service: &str, rpc: &str, key: &str, request_hash: &str) -> Lookup {
        let now = Instant::now();
        self.sweep_if_due(now);
        let k = (service.to_string(), rpc.to_string(), key.to_string());
        let reclaim = match self.entries.get(&k) {
            None => true,
            Some(Entry::Done { stored_at, .. }) => now.saturating_duration_since(*stored_at) > ENTRY_TTL,
            Some(Entry::InFlight { started_at, .. }) => now.saturating_duration_since(*started_at) > IN_FLIGHT_STALE_AFTER,
        };
        if reclaim {
            self.entries.insert(k, Entry::InFlight { request_hash: request_hash.to_string(), started_at: now });
            return Lookup::Reserved;
        }
        match self.entries.get(&k).expect("ya se confirmó Some arriba") {
            Entry::Done { request_hash: h, status, body, content_type, .. } => {
                if h != request_hash {
                    return Lookup::Conflict;
                }
                Lookup::Hit { status: *status, body: body.clone(), content_type: content_type.clone() }
            }
            Entry::InFlight { request_hash: h, .. } => {
                if h != request_hash {
                    return Lookup::Conflict;
                }
                Lookup::InFlight
            }
        }
    }

    /// Graba el resultado de una ejecución EXITOSA, reemplazando la marca
    /// "en vuelo" que `reserve` dejó -- el caller (`server.rs`) es
    /// responsable de solo llamar acá para un status 2xx; un error llama a
    /// `release` en su lugar, para que el caller pueda corregir y reintentar
    /// con la misma clave (mismo criterio que Stripe: una `Idempotency-Key`
    /// protege contra un DUPLICADO de una operación que funcionó, no contra
    /// reintentar una que falló).
    #[allow(clippy::too_many_arguments)]
    pub fn complete(&mut self, service: &str, rpc: &str, key: &str, request_hash: &str, status: u16, body: String, content_type: String) {
        self.entries.insert(
            (service.to_string(), rpc.to_string(), key.to_string()),
            Entry::Done { request_hash: request_hash.to_string(), status, body, content_type, stored_at: Instant::now() },
        );
    }

    /// Libera una clave reservada por `reserve` sin grabar nada -- el rpc
    /// terminó en error (o el proceso se está por caer a mitad de camino),
    /// así que un reintento con la misma clave tiene que poder correr de
    /// nuevo enseguida, no esperar a `IN_FLIGHT_STALE_AFTER`.
    pub fn release(&mut self, service: &str, rpc: &str, key: &str) {
        self.entries.remove(&(service.to_string(), rpc.to_string(), key.to_string()));
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
    fn a_key_never_seen_before_is_reserved() {
        let mut store = IdempotencyStore::new();
        assert!(matches!(store.reserve("Orders", "create", "key-1", "hash-a"), Lookup::Reserved));
    }

    #[test]
    fn a_completed_success_replays_on_the_same_key_and_hash() {
        let mut store = IdempotencyStore::new();
        assert!(matches!(store.reserve("Orders", "create", "key-1", "hash-a"), Lookup::Reserved));
        store.complete("Orders", "create", "key-1", "hash-a", 200, "{\"id\":1}".to_string(), "application/json".to_string());
        match store.reserve("Orders", "create", "key-1", "hash-a") {
            Lookup::Hit { status, body, content_type } => {
                assert_eq!(status, 200);
                assert_eq!(body, "{\"id\":1}");
                assert_eq!(content_type, "application/json");
            }
            _ => panic!("se esperaba un Hit"),
        }
    }

    #[test]
    fn the_same_key_with_a_different_request_hash_is_a_conflict_once_completed() {
        let mut store = IdempotencyStore::new();
        assert!(matches!(store.reserve("Orders", "create", "key-1", "hash-a"), Lookup::Reserved));
        store.complete("Orders", "create", "key-1", "hash-a", 200, "ok".to_string(), "application/json".to_string());
        assert!(matches!(store.reserve("Orders", "create", "key-1", "hash-b"), Lookup::Conflict));
    }

    #[test]
    fn the_same_key_is_independent_per_service_and_rpc() {
        let mut store = IdempotencyStore::new();
        assert!(matches!(store.reserve("Orders", "create", "key-1", "hash-a"), Lookup::Reserved));
        store.complete("Orders", "create", "key-1", "hash-a", 200, "ok".to_string(), "application/json".to_string());
        // Mismo string de clave, otro rpc: se puede reservar, no Conflict --
        // namespaces separados.
        assert!(matches!(store.reserve("Orders", "update", "key-1", "hash-a"), Lookup::Reserved));
        // Mismo string de clave, otro service: también separado.
        assert!(matches!(store.reserve("Invoices", "create", "key-1", "hash-a"), Lookup::Reserved));
    }

    /// El caso central de AUDIT-2026-08-27.md #4: `reserve` es la única
    /// operación válida para "primera vez que veo esta clave" -- un segundo
    /// `reserve` con la MISMA clave, ANTES de que el primero llame a
    /// `complete`/`release`, tiene que dar `InFlight`, nunca otro `Reserved`.
    /// Eso es lo que cierra la carrera: sin esto, dos hilos concurrentes
    /// podían ver los dos un `Miss`/`Reserved` y correr el cuerpo dos veces.
    #[test]
    fn a_second_reserve_for_a_key_still_in_flight_never_sees_reserved_again() {
        let mut store = IdempotencyStore::new();
        assert!(matches!(store.reserve("Payments", "charge", "key-1", "hash-a"), Lookup::Reserved));
        // El primero todavía no llamó a complete/release -- el segundo (y
        // un tercero, un cuarto...) tienen que ver InFlight, no Reserved.
        assert!(matches!(store.reserve("Payments", "charge", "key-1", "hash-a"), Lookup::InFlight));
        assert!(matches!(store.reserve("Payments", "charge", "key-1", "hash-a"), Lookup::InFlight));
    }

    #[test]
    fn a_key_still_in_flight_with_a_different_hash_is_a_conflict_not_in_flight() {
        let mut store = IdempotencyStore::new();
        assert!(matches!(store.reserve("Payments", "charge", "key-1", "hash-a"), Lookup::Reserved));
        assert!(matches!(store.reserve("Payments", "charge", "key-1", "hash-b"), Lookup::Conflict));
    }

    #[test]
    fn release_frees_the_key_immediately_for_a_failed_attempt() {
        let mut store = IdempotencyStore::new();
        assert!(matches!(store.reserve("Payments", "charge", "key-1", "hash-a"), Lookup::Reserved));
        // El rpc terminó en error -- no se graba, se libera.
        store.release("Payments", "charge", "key-1");
        // Un reintento inmediato con la misma clave puede volver a correr.
        assert!(matches!(store.reserve("Payments", "charge", "key-1", "hash-a"), Lookup::Reserved));
    }

    #[test]
    fn hash_request_body_is_deterministic_and_sensitive_to_content() {
        assert_eq!(hash_request_body("{\"a\":1}"), hash_request_body("{\"a\":1}"));
        assert_ne!(hash_request_body("{\"a\":1}"), hash_request_body("{\"a\":2}"));
    }
}
