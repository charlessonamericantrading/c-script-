//! Parseo de `@cron("5m")` (GRAMMAR.md §3.159) -- tarea recurrente nativa
//! dentro de `linkc serve`.
//!
//! Mismo motivo que `rate_limit.rs`/`cache.rs`/`idempotency.rs` para vivir
//! en un módulo aparte: el checker valida el FORMATO en compilación y el
//! servidor arma el scheduler real en runtime -- un solo parser evita que
//! las dos capas terminen de acuerdo en qué es "5m" por casualidad en vez
//! de por construcción (GRAMMAR.md §3.9).

use std::time::Duration;

/// Parsea "Ns"/"Nm"/"Nh"/"Nd" -- mismo formato que `--session-ttl`/
/// `@cache`, reimplementado acá a propósito (mismo criterio que el resto
/// de estos parsers chicos, ver el comentario de `cache::parse_ttl`).
pub fn parse_interval(raw: &str) -> Result<Duration, String> {
    let invalid = || format!("formato de @cron inválido: '{raw}' (se esperaba 'Ns', 'Nm', 'Nh' o 'Nd', ej. '5m' -- cada 5 minutos)");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_interval_accepts_seconds_minutes_hours_days() {
        assert_eq!(parse_interval("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_interval("5m").unwrap(), Duration::from_secs(300));
        assert_eq!(parse_interval("2h").unwrap(), Duration::from_secs(7200));
        assert_eq!(parse_interval("1d").unwrap(), Duration::from_secs(86400));
    }

    #[test]
    fn parse_interval_rejects_garbage() {
        assert!(parse_interval("").is_err());
        assert!(parse_interval("5").is_err());
        assert!(parse_interval("0m").is_err());
        assert!(parse_interval("5w").is_err());
        assert!(parse_interval("-5m").is_err());
    }
}
