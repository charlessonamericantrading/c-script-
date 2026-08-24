//! Parseo de `@rate_limit("20/1m")` y el token bucket que lo hace cumplir en
//! runtime (GRAMMAR.md §3.39).
//!
//! Mismo motivo que `route.rs` para vivir en un módulo aparte: el checker
//! valida el FORMATO en compilación y el servidor aplica el LÍMITE en
//! runtime -- un solo parser evita que las dos capas terminen de acuerdo en
//! qué es "20/1m" por casualidad en vez de por construcción (GRAMMAR.md
//! §3.9).
//!
//! La clave del bucket es (ip_del_cliente, servicio, rpc) -- la IP sale de
//! `Request::remote_addr()` (la conexión TCP real) por default, NUNCA de un
//! header como `X-Forwarded-For` que cualquier cliente puede mandar con el
//! valor que quiera. Detrás de un proxy/balanceador eso limitaría por la IP
//! del proxy, no la del usuario final -- `--trust-proxy`/`LINK_TRUST_PROXY`
//! (GRAMMAR.md §3.89, `runtime/server.rs::client_ip_for_rate_limit`) es el
//! opt-in explícito para usar `X-Forwarded-For` en su lugar, apagado por
//! default -- prenderlo sin tener de verdad un proxy de confianza delante
//! deja que cualquier cliente directo evada el límite mandando un header
//! distinto en cada request.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Un `@rate_limit("N/ventana")` ya parseado: como mucho `count` requests
/// por `window`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateLimitSpec {
    pub count: u32,
    pub window: Duration,
}

impl RateLimitSpec {
    /// Parsea "N/Ns", "N/Nm" o "N/Nh" (ej. "20/1m" = 20 requests por
    /// minuto). El número de la ventana es opcional (`"5/m"` = "5/1m"), por
    /// simetría con cómo se suele escribir esto en docs de APIs reales.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let invalid = || {
            format!(
                "formato de @rate_limit inválido: '{raw}' (se esperaba 'N/Ns', 'N/Nm' o 'N/Nh', ej. '20/1m' -- 20 requests por minuto)"
            )
        };
        let (count_str, window_str) = raw.split_once('/').ok_or_else(invalid)?;
        let count: u32 = count_str.parse().map_err(|_| invalid())?;
        if count == 0 {
            return Err(invalid());
        }
        if window_str.is_empty() {
            return Err(invalid());
        }
        let (num_str, unit) = window_str.split_at(window_str.len() - 1);
        let num: u64 = if num_str.is_empty() { 1 } else { num_str.parse().map_err(|_| invalid())? };
        if num == 0 {
            return Err(invalid());
        }
        let window = match unit {
            "s" => Duration::from_secs(num),
            "m" => Duration::from_secs(num * 60),
            "h" => Duration::from_secs(num * 3600),
            _ => return Err(invalid()),
        };
        Ok(RateLimitSpec { count, window })
    }
}

/// Un token bucket por (ip, servicio, rpc). Vive una sola instancia por
/// proceso servidor, mutada request a request en el hilo principal (mismo
/// modelo de concurrencia que el resto de `serve()`, ver runtime/server.rs)
/// -- no hace falta `Mutex`, ya que nunca se accede desde otro hilo.
pub struct RateLimiter {
    buckets: HashMap<(String, String, String), Bucket>,
    checks_since_sweep: u32,
}

struct Bucket {
    tokens: f64,
    capacity: f64,
    refill_per_sec: f64,
    last_seen: Instant,
}

/// Cada cuántos checks se barren buckets inactivos -- sin esto, un proceso
/// de larga vida con muchos clientes distintos (ej. detrás de un CDN que
/// rota IPs) acumula entradas para siempre. 1000 es simplemente "no tan
/// seguido como para pesar en el hot path, no tan poco como para dejar
/// crecer el mapa sin límite".
const SWEEP_EVERY: u32 = 1000;
/// Un bucket inactivo por más de una hora no va a volver a rellenarse de
/// forma observable -- se puede recrear desde cero la próxima vez que ese
/// cliente golpee este rpc, sin cambiar el comportamiento percibido.
const SWEEP_MAX_IDLE: Duration = Duration::from_secs(3600);

impl RateLimiter {
    pub fn new() -> Self {
        RateLimiter { buckets: HashMap::new(), checks_since_sweep: 0 }
    }

    /// `true` si esta request consume un token y puede seguir; `false` si
    /// hay que responder 429. Refill continuo (no por ventanas fijas): un
    /// bucket de capacidad `count` se rellena a razón de `count/window` por
    /// segundo, así que ráfagas cortas al principio de una ventana no dejan
    /// "muerto" el resto de la ventana como pasaría con un contador que
    /// resetea de golpe.
    pub fn check(&mut self, client_ip: &str, service: &str, rpc: &str, spec: RateLimitSpec) -> bool {
        let now = Instant::now();
        self.checks_since_sweep += 1;
        if self.checks_since_sweep >= SWEEP_EVERY {
            self.checks_since_sweep = 0;
            if let Some(cutoff) = now.checked_sub(SWEEP_MAX_IDLE) {
                self.buckets.retain(|_, b| b.last_seen >= cutoff);
            }
        }

        let capacity = spec.count as f64;
        let refill_per_sec = capacity / spec.window.as_secs_f64();
        let key = (client_ip.to_string(), service.to_string(), rpc.to_string());
        let bucket = self.buckets.entry(key).or_insert_with(|| Bucket { tokens: capacity, capacity, refill_per_sec, last_seen: now });

        let elapsed = now.saturating_duration_since(bucket.last_seen).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * bucket.refill_per_sec).min(bucket.capacity);
        bucket.last_seen = now;

        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_specs() {
        assert_eq!(RateLimitSpec::parse("20/1m").unwrap(), RateLimitSpec { count: 20, window: Duration::from_secs(60) });
        assert_eq!(RateLimitSpec::parse("5/s").unwrap(), RateLimitSpec { count: 5, window: Duration::from_secs(1) });
        assert_eq!(RateLimitSpec::parse("100/2h").unwrap(), RateLimitSpec { count: 100, window: Duration::from_secs(7200) });
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(RateLimitSpec::parse("").is_err());
        assert!(RateLimitSpec::parse("20").is_err());
        assert!(RateLimitSpec::parse("0/1m").is_err());
        assert!(RateLimitSpec::parse("20/0m").is_err());
        assert!(RateLimitSpec::parse("20/1d").is_err());
        assert!(RateLimitSpec::parse("abc/1m").is_err());
    }

    #[test]
    fn bucket_allows_up_to_capacity_then_blocks() {
        let mut rl = RateLimiter::new();
        let spec = RateLimitSpec { count: 3, window: Duration::from_secs(60) };
        assert!(rl.check("1.2.3.4", "Sys", "ping", spec));
        assert!(rl.check("1.2.3.4", "Sys", "ping", spec));
        assert!(rl.check("1.2.3.4", "Sys", "ping", spec));
        assert!(!rl.check("1.2.3.4", "Sys", "ping", spec));
    }

    #[test]
    fn bucket_is_independent_per_key() {
        let mut rl = RateLimiter::new();
        let spec = RateLimitSpec { count: 1, window: Duration::from_secs(60) };
        assert!(rl.check("1.2.3.4", "Sys", "ping", spec));
        assert!(!rl.check("1.2.3.4", "Sys", "ping", spec));
        // Misma IP, otro rpc: bucket distinto, no afectado por el de arriba.
        assert!(rl.check("1.2.3.4", "Sys", "other", spec));
        // Otra IP, mismo rpc: también un bucket distinto.
        assert!(rl.check("5.6.7.8", "Sys", "ping", spec));
    }
}
