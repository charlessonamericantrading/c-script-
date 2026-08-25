//! Parseo de `@cache("60s")` y el store en memoria que lo hace cumplir en
//! runtime (GRAMMAR.md §3.144).
//!
//! Mismo motivo que `rate_limit.rs`/`idempotency.rs` para vivir en un módulo
//! aparte: el checker valida el FORMATO en compilación y el servidor aplica
//! el cache en runtime -- un solo parser evita que las dos capas terminen de
//! acuerdo en qué es "60s" por casualidad en vez de por construcción
//! (GRAMMAR.md §3.9).

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Parsea "Ns"/"Nm"/"Nh"/"Nd" -- mismo formato que `--session-ttl`/
/// `--http-timeout` (main.rs::parse_duration), reimplementado acá a
/// propósito: ese vive del lado del binario (parseo de flags de CLI), este
/// del lado de la librería (`checker`/`runtime`) -- mismo patrón ya
/// establecido de que `rate_limit::RateLimitSpec::parse` tampoco comparte
/// código con `parse_duration` pese a superponerse en formato.
pub fn parse_ttl(raw: &str) -> Result<Duration, String> {
    let invalid = || format!("formato de @cache inválido: '{raw}' (se esperaba 'Ns', 'Nm', 'Nh' o 'Nd', ej. '60s' o '5m')");
    if raw.is_empty() {
        return Err(invalid());
    }
    let (num_str, unit) = raw.split_at(raw.len() - 1);
    let num: u64 = num_str.parse().map_err(|_| invalid())?;
    if num == 0 {
        return Err(invalid());
    }
    match unit {
        "s" => Ok(Duration::from_secs(num)),
        "m" => Ok(Duration::from_secs(num * 60)),
        "h" => Ok(Duration::from_secs(num * 3600)),
        "d" => Ok(Duration::from_secs(num * 86400)),
        _ => Err(invalid()),
    }
}

struct Entry {
    status: u16,
    body: String,
    content_type: String,
    expires_at: Instant,
}

const SWEEP_EVERY: u32 = 1000;

/// Una entrada por (service, rpc, argumentos-serializados-como-JSON) --
/// mismo modelo de concurrencia que `RateLimiter`/`IdempotencyStore`: un
/// solo proceso servidor, mutado en el hilo principal, sin `Mutex`. No
/// persiste entre reinicios (aceptado a propósito, mismo criterio que el
/// resto de este estado de proceso).
pub struct CacheStore {
    entries: HashMap<(String, String, String), Entry>,
    checks_since_sweep: u32,
}

impl CacheStore {
    pub fn new() -> Self {
        CacheStore { entries: HashMap::new(), checks_since_sweep: 0 }
    }

    fn sweep_if_due(&mut self, now: Instant) {
        self.checks_since_sweep += 1;
        if self.checks_since_sweep < SWEEP_EVERY {
            return;
        }
        self.checks_since_sweep = 0;
        self.entries.retain(|_, e| e.expires_at > now);
    }

    /// `Some((status, body, content_type))` si hay una entrada viva para
    /// esta clave; `None` en un miss O una entrada vencida (se recalcula
    /// fresca la próxima vez que se grabe).
    pub fn get(&mut self, service: &str, rpc: &str, args_key: &str) -> Option<(u16, String, String)> {
        let now = Instant::now();
        self.sweep_if_due(now);
        let entry = self.entries.get(&(service.to_string(), rpc.to_string(), args_key.to_string()))?;
        if entry.expires_at <= now {
            return None;
        }
        Some((entry.status, entry.body.clone(), entry.content_type.clone()))
    }

    pub fn put(&mut self, service: &str, rpc: &str, args_key: &str, status: u16, body: String, content_type: String, ttl: Duration) {
        self.entries.insert(
            (service.to_string(), rpc.to_string(), args_key.to_string()),
            Entry { status, body, content_type, expires_at: Instant::now() + ttl },
        );
    }
}

impl Default for CacheStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ttl_accepts_seconds_minutes_hours_days() {
        assert_eq!(parse_ttl("60s").unwrap(), Duration::from_secs(60));
        assert_eq!(parse_ttl("5m").unwrap(), Duration::from_secs(300));
        assert_eq!(parse_ttl("2h").unwrap(), Duration::from_secs(7200));
        assert_eq!(parse_ttl("1d").unwrap(), Duration::from_secs(86400));
    }

    #[test]
    fn parse_ttl_rejects_garbage() {
        assert!(parse_ttl("").is_err());
        assert!(parse_ttl("60").is_err());
        assert!(parse_ttl("0s").is_err());
        assert!(parse_ttl("abc").is_err());
        assert!(parse_ttl("60w").is_err());
    }

    #[test]
    fn a_key_never_seen_before_is_a_miss() {
        let mut store = CacheStore::new();
        assert!(store.get("Stats", "summary", "{}").is_none());
    }

    #[test]
    fn a_stored_entry_replays_within_its_ttl() {
        let mut store = CacheStore::new();
        store.put("Stats", "summary", "{}", 200, "{\"total\":1}".to_string(), "application/json".to_string(), Duration::from_secs(60));
        let (status, body, content_type) = store.get("Stats", "summary", "{}").expect("debe haber un hit");
        assert_eq!(status, 200);
        assert_eq!(body, "{\"total\":1}");
        assert_eq!(content_type, "application/json");
    }

    #[test]
    fn an_expired_entry_is_a_miss() {
        let mut store = CacheStore::new();
        store.put("Stats", "summary", "{}", 200, "old".to_string(), "application/json".to_string(), Duration::from_millis(0));
        std::thread::sleep(Duration::from_millis(5));
        assert!(store.get("Stats", "summary", "{}").is_none());
    }

    #[test]
    fn different_args_are_independent_entries() {
        let mut store = CacheStore::new();
        store.put("Stats", "summary", "{\"id\":1}", 200, "one".to_string(), "application/json".to_string(), Duration::from_secs(60));
        assert!(store.get("Stats", "summary", "{\"id\":2}").is_none());
    }
}
