//! `GET /metrics` en formato de exposición de Prometheus (GRAMMAR.md
//! §3.149). Mismo modelo de un solo proceso que
//! `rate_limit::RateLimiter`/`cache::CacheStore`/
//! `idempotency::IdempotencyStore` -- no persiste entre reinicios. Desde
//! GRAMMAR.md §3.158 (v1.114.0, un hilo real por request) vive detrás de
//! `Arc<parking_lot::Mutex<MetricsStore>>` en `server.rs`, ya no mutado
//! desde un único hilo principal.

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
    /// Rechazos `429` de `@rate_limit`, por rpc -- landmine del barrido de
    /// "límites honestos" (GRAMMAR.md §3.39): el límite vive en memoria por
    /// PROCESO, así que correr N réplicas detrás de un balanceador diluye
    /// el límite real de forma silenciosa (cada proceso sigue cumpliendo
    /// SU cupo, pero la suma entre réplicas puede ser N veces más alta de
    /// lo que el `.link` pidió). No hay forma de arreglar la dilución sin
    /// estado compartido entre procesos (fuera de alcance) -- pero contar
    /// los 429 reales por rpc, agregable entre réplicas en Prometheus, es
    /// la señal que le permite a un operador NOTAR el problema en vez de
    /// enterarse por un endpoint caro sin protección real.
    rate_limit_rejections: HashMap<String, u64>,
    /// (corridas OK, corridas fallidas) de cada tarea `@cron` (GRAMMAR.md
    /// §3.159), por `Servicio.rpc` -- una tarea recurrente corre sola, sin
    /// ningún caller HTTP que note un 5xx si su cuerpo empieza a fallar;
    /// sin esto, la única señal seria leer stdout/stderr bajo `pm2`/
    /// `systemd`, mismo problema ya resuelto para NOTIFY oversized
    /// (GRAMMAR.md §3.150).
    cron_runs: HashMap<String, (u64, u64)>,
}

impl MetricsStore {
    pub fn new() -> Self {
        MetricsStore {
            by_method: HashMap::new(),
            notify_latency: (0, 0.0),
            rate_limit_rejections: HashMap::new(),
            cron_runs: HashMap::new(),
        }
    }

    /// Registra UNA corrida de una tarea `@cron` -- `runtime/server.rs` la
    /// llama en cada tick, junto con `log_cron_tick`.
    pub fn record_cron_run(&mut self, method: &str, ok: bool) {
        let entry = self.cron_runs.entry(method.to_string()).or_insert((0, 0));
        if ok {
            entry.0 += 1;
        } else {
            entry.1 += 1;
        }
    }

    /// Registra UN rechazo `429` de `@rate_limit` para `method`
    /// (`Servicio.rpc`) -- llamado por `runtime/server.rs` en el mismo
    /// punto que ya arma la respuesta 429, antes de devolverla.
    pub fn record_rate_limit_rejection(&mut self, method: &str) {
        *self.rate_limit_rejections.entry(method.to_string()).or_insert(0) += 1;
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
    /// NORMAL de un `rpc` (`server.rs`) -- un hit de
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
        outbound_http: &[(String, String, u64, f64)],
        ai: &[(String, AiStatsRow)],
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
        // GRAMMAR.md §3.223: llamadas `http.*` SALIENTES, por host y clase
        // de status -- la latencia y la tasa de error del proveedor del que
        // depende un rpc (un LLM, un pasarela de pago, un webhook). Vienen
        // de `Db` (lo único que las ve), mismo criterio que
        // `stream_subscribers`. Solo si hubo al menos una, como el resto.
        if !outbound_http.is_empty() {
            out.push_str("# HELP linkc_http_outbound_total Llamadas http.* salientes, por host y clase de status (2xx/3xx/4xx/5xx/error).\n");
            out.push_str("# TYPE linkc_http_outbound_total counter\n");
            for (host, status, count, _) in outbound_http {
                out.push_str(&format!("linkc_http_outbound_total{{host=\"{}\",status=\"{}\"}} {count}\n", escape_label(host), escape_label(status)));
            }
            out.push_str("# HELP linkc_http_outbound_duration_seconds_sum Tiempo total esperando llamadas http.* salientes, por host y clase de status.\n");
            out.push_str("# TYPE linkc_http_outbound_duration_seconds_sum counter\n");
            for (host, status, _, secs) in outbound_http {
                out.push_str(&format!("linkc_http_outbound_duration_seconds_sum{{host=\"{}\",status=\"{}\"}} {secs}\n", escape_label(host), escape_label(status)));
            }
        }
        // GRAMMAR.md §3.237: el motor embebido, por alias de `ai { }`. Solo
        // si hubo al menos una generación, como el resto. tokens/s de
        // decode = generated / decode_seconds_sum; de prefill = prompt /
        // prefill_seconds_sum -- las dos cifras que ROADMAP-PERF de origen
        // mide, ahora desde el `.link` en producción.
        if !ai.is_empty() {
            out.push_str("# HELP linkc_ai_requests_total Generaciones ai.* (generate/chat/stream) por modelo y resultado (ok/error).\n");
            out.push_str("# TYPE linkc_ai_requests_total counter\n");
            for (model, row) in ai {
                out.push_str(&format!("linkc_ai_requests_total{{model=\"{}\",result=\"ok\"}} {}\n", escape_label(model), row.ok));
                out.push_str(&format!("linkc_ai_requests_total{{model=\"{}\",result=\"error\"}} {}\n", escape_label(model), row.errors));
            }
            out.push_str("# HELP linkc_ai_tokens_total Tokens procesados por modelo: kind=prompt (prefill) o kind=generated (decode).\n");
            out.push_str("# TYPE linkc_ai_tokens_total counter\n");
            for (model, row) in ai {
                out.push_str(&format!("linkc_ai_tokens_total{{model=\"{}\",kind=\"prompt\"}} {}\n", escape_label(model), row.prompt_tokens));
                out.push_str(&format!("linkc_ai_tokens_total{{model=\"{}\",kind=\"generated\"}} {}\n", escape_label(model), row.generated_tokens));
            }
            out.push_str("# HELP linkc_ai_duration_seconds_sum Segundos del motor por modelo y fase (prefill/decode); tokens/s = linkc_ai_tokens_total / esto.\n");
            out.push_str("# TYPE linkc_ai_duration_seconds_sum counter\n");
            for (model, row) in ai {
                out.push_str(&format!("linkc_ai_duration_seconds_sum{{model=\"{}\",phase=\"prefill\"}} {}\n", escape_label(model), row.prefill_secs));
                out.push_str(&format!("linkc_ai_duration_seconds_sum{{model=\"{}\",phase=\"decode\"}} {}\n", escape_label(model), row.decode_secs));
            }
            out.push_str("# HELP linkc_ai_prefix_cache_hits_total Generaciones cuyo prompt reusó el KV de un prefijo reciente, por modelo.\n");
            out.push_str("# TYPE linkc_ai_prefix_cache_hits_total counter\n");
            for (model, row) in ai {
                out.push_str(&format!("linkc_ai_prefix_cache_hits_total{{model=\"{}\"}} {}\n", escape_label(model), row.prefix_hits));
            }
        }
        if !self.rate_limit_rejections.is_empty() {
            out.push_str("# HELP linkc_rate_limit_rejections_total Requests rechazadas 429 por @rate_limit, por rpc.\n");
            out.push_str("# TYPE linkc_rate_limit_rejections_total counter\n");
            for (method, count) in &self.rate_limit_rejections {
                out.push_str(&format!("linkc_rate_limit_rejections_total{{method=\"{}\"}} {count}\n", escape_label(method)));
            }
        }
        if !self.cron_runs.is_empty() {
            out.push_str("# HELP linkc_cron_runs_total Corridas de una tarea @cron, por rpc.\n");
            out.push_str("# TYPE linkc_cron_runs_total counter\n");
            for (method, (ok, _)) in &self.cron_runs {
                out.push_str(&format!("linkc_cron_runs_total{{method=\"{}\"}} {ok}\n", escape_label(method)));
            }
            // Mismo criterio que `rate_limit_rejections` arriba: solo los
            // rpcs que de verdad tuvieron al menos una falla, nunca un `0`
            // inventado para el resto.
            let any_failed = self.cron_runs.values().any(|(_, failed)| *failed > 0);
            if any_failed {
                out.push_str("# HELP linkc_cron_failures_total Corridas de una tarea @cron que terminaron en error, por rpc.\n");
                out.push_str("# TYPE linkc_cron_failures_total counter\n");
                for (method, (_, failed)) in &self.cron_runs {
                    if *failed > 0 {
                        out.push_str(&format!("linkc_cron_failures_total{{method=\"{}\"}} {failed}\n", escape_label(method)));
                    }
                }
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
/// GRAMMAR.md §3.237: contadores de `ai.*` por modelo (alias), acumulados
/// en el `Db` (`record_ai`) y leídos por `/metrics` vía `ai_stats()` --
/// mismo patrón que `outbound_http`. Sin `inference` en el struct: el
/// renderer no depende del motor.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AiStatsRow {
    pub ok: u64,
    pub errors: u64,
    pub prompt_tokens: u64,
    pub generated_tokens: u64,
    pub prefill_secs: f64,
    pub decode_secs: f64,
    pub prefix_hits: u64,
}

fn escape_label(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_method_never_recorded_does_not_appear() {
        let store = MetricsStore::new();
        let out = store.render_prometheus_text(&[], None, &[], &[], &[]);
        assert!(!out.contains("linkc_http_requests_total{"));
    }

    #[test]
    fn record_accumulates_count_and_duration_per_method() {
        let mut store = MetricsStore::new();
        store.record("Tasks.list", Duration::from_millis(100));
        store.record("Tasks.list", Duration::from_millis(200));
        store.record("Tasks.create", Duration::from_millis(50));
        let out = store.render_prometheus_text(&[], None, &[], &[], &[]);
        assert!(out.contains("linkc_http_requests_total{method=\"Tasks.list\"} 2"), "{out}");
        assert!(out.contains("linkc_http_requests_total{method=\"Tasks.create\"} 1"), "{out}");
        assert!(out.contains("linkc_http_request_duration_seconds_sum{method=\"Tasks.list\"} 0.3"), "{out}");
    }

    #[test]
    fn stream_subscribers_and_db_size_appear_only_when_provided() {
        let store = MetricsStore::new();
        let out = store.render_prometheus_text(&[("tasks".to_string(), 3)], Some(1024), &[], &[], &[]);
        assert!(out.contains("linkc_stream_subscribers{collection=\"tasks\"} 3"), "{out}");
        assert!(out.contains("linkc_db_size_bytes 1024"), "{out}");

        let out_without = store.render_prometheus_text(&[], None, &[], &[], &[]);
        assert!(!out_without.contains("linkc_stream_subscribers{"));
        assert!(!out_without.contains("linkc_db_size_bytes"));
    }

    #[test]
    fn rate_limit_rejections_accumulate_per_method_and_are_absent_until_recorded() {
        let mut store = MetricsStore::new();
        let out = store.render_prometheus_text(&[], None, &[], &[], &[]);
        assert!(!out.contains("linkc_rate_limit_rejections_total"), "{out}");

        store.record_rate_limit_rejection("Auth.login");
        store.record_rate_limit_rejection("Auth.login");
        store.record_rate_limit_rejection("Orders.create");
        let out = store.render_prometheus_text(&[], None, &[], &[], &[]);
        assert!(out.contains("linkc_rate_limit_rejections_total{method=\"Auth.login\"} 2"), "{out}");
        assert!(out.contains("linkc_rate_limit_rejections_total{method=\"Orders.create\"} 1"), "{out}");
    }

    #[test]
    fn oversized_notify_drops_appear_only_when_provided_and_are_per_collection() {
        let store = MetricsStore::new();
        let out = store.render_prometheus_text(&[], None, &[("catalog_facets".to_string(), 3)], &[], &[]);
        assert!(out.contains("linkc_notify_oversized_dropped_total{collection=\"catalog_facets\"} 3"), "{out}");

        let out_without = store.render_prometheus_text(&[], None, &[], &[], &[]);
        assert!(!out_without.contains("linkc_notify_oversized_dropped_total"));
    }

    #[test]
    fn cron_runs_accumulate_per_method_and_failures_only_appear_for_methods_that_actually_failed() {
        let mut store = MetricsStore::new();
        let out = store.render_prometheus_text(&[], None, &[], &[], &[]);
        assert!(!out.contains("linkc_cron_runs_total"), "{out}");

        store.record_cron_run("Jobs.sweep", true);
        store.record_cron_run("Jobs.sweep", true);
        store.record_cron_run("Jobs.sweep", false);
        store.record_cron_run("Jobs.reindex", true);
        let out = store.render_prometheus_text(&[], None, &[], &[], &[]);
        assert!(out.contains("linkc_cron_runs_total{method=\"Jobs.sweep\"} 2"), "{out}");
        assert!(out.contains("linkc_cron_runs_total{method=\"Jobs.reindex\"} 1"), "{out}");
        assert!(out.contains("linkc_cron_failures_total{method=\"Jobs.sweep\"} 1"), "{out}");
        assert!(!out.contains("linkc_cron_failures_total{method=\"Jobs.reindex\"}"), "un rpc sin fallas no debe llevar un 0 inventado: {out}");
    }
}

/// GRAMMAR.md §3.237: el bloque `linkc_ai_*` solo aparece con alguna
/// generación registrada, y trae las cuatro series por modelo.
#[cfg(test)]
mod ai_metrics_tests {
    use super::*;

    #[test]
    fn ai_block_renders_per_model_series_only_when_something_ran() {
        let store = MetricsStore::default();
        let without = store.render_prometheus_text(&[], None, &[], &[], &[]);
        assert!(!without.contains("linkc_ai_"), "{without}");
        let row = AiStatsRow { ok: 3, errors: 1, prompt_tokens: 40, generated_tokens: 24, prefill_secs: 0.5, decode_secs: 1.25, prefix_hits: 2 };
        let out = store.render_prometheus_text(&[], None, &[], &[], &[("router".to_string(), row)]);
        assert!(out.contains("linkc_ai_requests_total{model=\"router\",result=\"ok\"} 3"), "{out}");
        assert!(out.contains("linkc_ai_requests_total{model=\"router\",result=\"error\"} 1"), "{out}");
        assert!(out.contains("linkc_ai_tokens_total{model=\"router\",kind=\"generated\"} 24"), "{out}");
        assert!(out.contains("linkc_ai_duration_seconds_sum{model=\"router\",phase=\"decode\"} 1.25"), "{out}");
        assert!(out.contains("linkc_ai_prefix_cache_hits_total{model=\"router\"} 2"), "{out}");
        assert!(out.contains("# TYPE linkc_ai_tokens_total counter"), "{out}");
    }
}
