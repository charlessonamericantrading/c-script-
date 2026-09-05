// Estado en memoria para `@background` (GRAMMAR.md §3.262, PLAN.md §9.18 Eje
// F ítem 3 / §9.22 ítem 2). Mismo criterio y mismo modelo de concurrencia
// que `idempotency::IdempotencyStore`/`rate_limit`: una sola instancia por
// proceso servidor, sin sobrevivir un restart -- aceptable acá porque el v1
// de esta feature es explícitamente "sin cola distribuida (un proceso)"
// (PLAN.md), a diferencia de `@cache`/`@rate_limit` (que SÍ necesitan
// coordinarse entre instancias porque varias comparten la misma base). Un
// job encolado y un restart del servidor antes de que corra simplemente se
// pierde -- mismo límite que ya aceptan `@idempotent`/`@rate_limit` para su
// propio estado en memoria.
//
// Vive como campo de `Db` (no un `Arc` paralelo en `server.rs`, a diferencia
// de `IdempotencyStore`/`RateLimiter`): `background.status(jobId)` es un
// builtin invocado DESDE DENTRO de un cuerpo de rpc, y `call_method` solo
// tiene `db: &Db` a mano -- mismo motivo por el que `sessions`/`ai_engine`
// viven en `Db` en vez de enhebrarse aparte.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Una entrada TERMINADA (`Done`/`Failed`) vive como mucho 24hs -- mismo
/// criterio y mismo número que `idempotency::ENTRY_TTL`: cubre el caso real
/// (un cliente sondeando minutos u horas después de encolar) sin dejar
/// crecer el mapa para siempre en un proceso de larga vida.
const ENTRY_TTL: Duration = Duration::from_secs(24 * 3600);

#[derive(Clone)]
pub(crate) enum JobStatus {
    Pending,
    Running,
    Done { result_json: String },
    Failed { error: String },
}

struct JobRecord {
    service: String,
    rpc: String,
    args_json: String,
    /// El bearer token de quien encoló el job, si vino -- se reproduce tal
    /// cual como `current_token` cuando el worker corre el cuerpo de verdad
    /// (GRAMMAR.md §3.262), para que `auth.currentRole()`/`currentUserId()`
    /// dentro de ese cuerpo se comporten igual que si hubiera corrido
    /// sincrónicamente en la request original.
    token: Option<String>,
    status: JobStatus,
    finished_at: Option<Instant>,
}

pub(crate) struct BackgroundJobStore {
    jobs: HashMap<String, JobRecord>,
    /// FIFO de ids `Pending` -- separado de `jobs` para que `claim_next` no
    /// tenga que escanear el mapa entero buscando una entrada `Pending`.
    queue: std::collections::VecDeque<String>,
}

impl BackgroundJobStore {
    pub(crate) fn new() -> Self {
        BackgroundJobStore { jobs: HashMap::new(), queue: std::collections::VecDeque::new() }
    }

    fn sweep_expired(&mut self) {
        let now = Instant::now();
        self.jobs.retain(|_, j| j.finished_at.is_none_or(|t| now.saturating_duration_since(t) <= ENTRY_TTL));
    }

    /// Encola un job nuevo y devuelve su id -- `job_id` lo genera el caller
    /// (`crypto::generate_uuid_v4`, mismo generador que `crypto.uuid()`) para
    /// que este módulo no dependa de nada de `runtime/mod.rs` más que el tipo
    /// `Value` (que ni siquiera usa acá).
    pub(crate) fn enqueue(&mut self, job_id: String, service: String, rpc: String, args_json: String, token: Option<String>) {
        self.sweep_expired();
        self.jobs.insert(job_id.clone(), JobRecord { service, rpc, args_json, token, status: JobStatus::Pending, finished_at: None });
        self.queue.push_back(job_id);
    }

    /// Saca el próximo job `Pending` de la cola (si hay) y lo marca
    /// `Running` -- atómico bajo el mismo candado que el resto del store,
    /// así que dos workers nunca pueden reclamar el mismo id.
    #[allow(clippy::type_complexity)]
    pub(crate) fn claim_next(&mut self) -> Option<(String, String, String, String, Option<String>)> {
        let job_id = self.queue.pop_front()?;
        let record = self.jobs.get_mut(&job_id)?;
        record.status = JobStatus::Running;
        Some((job_id, record.service.clone(), record.rpc.clone(), record.args_json.clone(), record.token.clone()))
    }

    pub(crate) fn complete(&mut self, job_id: &str, result_json: String) {
        if let Some(record) = self.jobs.get_mut(job_id) {
            record.status = JobStatus::Done { result_json };
            record.finished_at = Some(Instant::now());
        }
    }

    pub(crate) fn fail(&mut self, job_id: &str, error: String) {
        if let Some(record) = self.jobs.get_mut(job_id) {
            record.status = JobStatus::Failed { error };
            record.finished_at = Some(Instant::now());
        }
    }

    /// `None` == el id no existe (nunca se encoló, o su entrada terminada ya
    /// venció) -- `background.status` lo mapea a un `status: "not_found"`
    /// explícito, nunca un error de runtime: un caller sondeando un id viejo
    /// o mal tipeado es un caso esperable, no excepcional.
    pub(crate) fn status(&self, job_id: &str) -> Option<JobStatus> {
        self.jobs.get(job_id).map(|r| r.status.clone())
    }
}
