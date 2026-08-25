//! `GET /metrics` en formato de exposición de Prometheus (GRAMMAR.md
//! §3.149). Mismo modelo de un solo proceso, mutado en el hilo principal,
//! que `rate_limit::RateLimiter`/`cache::CacheStore`/
//! `idempotency::IdempotencyStore` -- no persiste entre reinicios, no hace
//! falta `Mutex`.

use std::collections::HashMap;
use std::time::Duration;

/// Conteo + suma de duración por `{Servicio}.{rpc}` -- la forma mínima que
/// Prometheus necesita para calcular tasa de requests y latencia PROMEDIO
/// vía `rate(..._sum[5m]) / rate(..._count[5m])`, sin declarar buckets de
/// histograma (una decisión de diseño que este proyecto no tiene por qué
/// tomar por quien lo opera -- los buckets "correctos" dependen del SLA de
/// cada adoptador).
pub struct MetricsStore {
    by_method: HashMap<String, (u64, f64)>,
    /// (conteo, suma de latencia en segundos) de propagación NOTIFY
    /// (GRAMMAR.md §3.150) -- cuánto tardó un cambio en llegar de la
    /// instancia que escribió a ESTA, vía LISTEN/NOTIFY de Postgres. Mismo
    /// par conteo+suma que `by_method`, mismo motivo (`rate(sum)/rate(count)`
    /// sin declarar buckets).
    notify_latency: (u64, f64),
}

impl MetricsStore {
    pub fn new() -> Self {
        MetricsStore { by_method: HashMap::new(), notify_latency: (0, 0.0) }
    }

    /// Registra la latencia de UN evento de propagación NOTIFY recibido
    /// (GRAMMAR.md §3.150) -- `runtime/server.rs` la calcula al drenar el
    /// canal de cambios remotos, restando `sent_at_ms` (viajó en el propio
    /// payload del NOTIFY) de "ahora".
    pub fn record_notify_latency(&mut self, duration: Duration) {
        self.notify_latency.0 += 1;
        self.notify_latency.1 += duration.as_secs_f64();
    }

    /// Alcance v0 deliberado: solo se llama desde el camino de dispatch
    /// NORMAL de un `rpc` (`server.rs`, hilo principal) -- un hit de
    /// `@idempotent`/`@cache` y un `stream` no suman acá (ver GRAMMAR.md
    /// §3.149 para el porqué de cada uno).
    pub fn record(&mut self, method: &str, duration: Duration) {
        let entry = self.by_method.entry(method.to_string()).or_insert((0, 0.0));
        entry.0 += 1;
        entry.1 += duration.as_secs_f64();
    }

    /// Arma el texto completo de exposición -- `stream_subscribers` (por
    /// colección), `db_size_bytes` y `oversized_notify_drops` (por
    /// colección, GRAMMAR.md §3.44) vienen de `Db` (lo único que los sabe),
    /// pasados por el caller en vez de que este módulo dependa de `db.rs`.
    pub fn render_prometheus_text(
        &self,
        stream_subscribers: &[(String, usize)],
        db_size_bytes: Option<i64>,
        oversized_notify_drops: &[(String, u64)],
    ) -> String {
        let mut out = String::new();
        out.push_str("# HELP linkc_http_requests_total Total de requests HTTP atendidas, por rpc.\n");
        out.push_str("# TYPE linkc_http_requests_total counter\n");
        for (method, (count, _)) in &self.by_method {
            out.push_str(&format!("linkc_http_requests_total{{method=\"{}\"}} {count}\n", escape_label(method)));
        }
        out.push_str("# HELP linkc_http_request_duration_seconds_sum Tiempo total gastado atendiendo requests, por rpc.\n");
        out.push_str("# TYPE linkc_http_request_duration_seconds_sum counter\n");
        for (method, (_, duration_sum)) in &self.by_method {
            out.push_str(&format!("linkc_http_request_duration_seconds_sum{{method=\"{}\"}} {duration_sum}\n", escape_label(method)));
        }
        out.push_str("# HELP linkc_stream_subscribers Clientes conectados a un stream ahora mismo, por colección.\n");
        out.push_str("# TYPE linkc_stream_subscribers gauge\n");
        for (collection, count) in stream_subscribers {
            out.push_str(&format!("linkc_stream_subscribers{{collection=\"{}\"}} {count}\n", escape_label(collection)));
        }
        if let Some(size) = db_size_bytes {
            out.push_str("# HELP linkc_db_size_bytes Tamaño de la base de datos en bytes.\n");
            out.push_str("# TYPE linkc_db_size_bytes gauge\n");
            out.push_str(&format!("linkc_db_size_bytes {size}\n"));
        }
        // Solo si se registró al menos un evento -- sin esto, una instancia
        // SQLite (que nunca usa NOTIFY) o una que arrancó sola mostraría
        // '0 0', indistinguible de "propagación funcionando perfecto, cero
        // latencia" en vez de "esto nunca aplicó acá".
        if self.notify_latency.0 > 0 {
            out.push_str("# HELP linkc_notify_latency_seconds_sum Latencia total de propagación NOTIFY entre instancias.\n");
            out.push_str("# TYPE linkc_notify_latency_seconds_sum counter\n");
            out.push_str(&format!("linkc_notify_latency_seconds_sum {}\n", self.notify_latency.1));
            out.push_str("# HELP linkc_notify_latency_seconds_count Cantidad de eventos NOTIFY recibidos de otras instancias.\n");
            out.push_str("# TYPE linkc_notify_latency_seconds_count counter\n");
            out.push_str(&format!("linkc_notify_latency_seconds_count {}\n", self.notify_latency.0));
        }
        if !oversized_notify_drops.is_empty() {
            out.push_str("# HELP linkc_notify_oversized_dropped_total Cambios que nunca se propagaron por superar el límite de NOTIFY de PostgreSQL, por colección.\n");
            out.push_str("# TYPE linkc_notify_oversized_dropped_total counter\n");
            for (collection, count) in oversized_notify_drops {
                out.push_str(&format!("linkc_notify_oversized_dropped_total{{collection=\"{}\"}} {count}\n", escape_label(collection)));
            }
        }
        out
    }
}

impl Default for MetricsStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Un nombre de rpc (`Servicio.metodo`) o de colección son identificadores
/// de c-script -- nunca pueden contener una comilla doble ni una barra
/// invertida (el lexer no los acepta en un `Ident`), así que esto es
/// defensa en profundidad, no una condición alcanzable hoy.
fn escape_label(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_method_never_recorded_does_not_appear() {
        let store = MetricsStore::new();
        let out = store.render_prometheus_text(&[], None, &[]);
        assert!(!out.contains("linkc_http_requests_total{"));
    }

    #[test]
    fn record_accumulates_count_and_duration_per_method() {
        let mut store = MetricsStore::new();
        store.record("Tasks.list", Duration::from_millis(100));
        store.record("Tasks.list", Duration::from_millis(200));
        store.record("Tasks.create", Duration::from_millis(50));
        let out = store.render_prometheus_text(&[], None, &[]);
        assert!(out.contains("linkc_http_requests_total{method=\"Tasks.list\"} 2"), "{out}");
        assert!(out.contains("linkc_http_requests_total{method=\"Tasks.create\"} 1"), "{out}");
        assert!(out.contains("linkc_http_request_duration_seconds_sum{method=\"Tasks.list\"} 0.3"), "{out}");
    }

    #[test]
    fn stream_subscribers_and_db_size_appear_only_when_provided() {
        let store = MetricsStore::new();
        let out = store.render_prometheus_text(&[("tasks".to_string(), 3)], Some(1024), &[]);
        assert!(out.contains("linkc_stream_subscribers{collection=\"tasks\"} 3"), "{out}");
        assert!(out.contains("linkc_db_size_bytes 1024"), "{out}");

        let out_without = store.render_prometheus_text(&[], None, &[]);
        assert!(!out_without.contains("linkc_stream_subscribers{"));
        assert!(!out_without.contains("linkc_db_size_bytes"));
    }

    #[test]
    fn oversized_notify_drops_appear_only_when_provided_and_are_per_collection() {
        let store = MetricsStore::new();
        let out = store.render_prometheus_text(&[], None, &[("catalog_facets".to_string(), 3)]);
        assert!(out.contains("linkc_notify_oversized_dropped_total{collection=\"catalog_facets\"} 3"), "{out}");

        let out_without = store.render_prometheus_text(&[], None, &[]);
        assert!(!out_without.contains("linkc_notify_oversized_dropped_total"));
    }
}
