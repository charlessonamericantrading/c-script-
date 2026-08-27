# Plan de arreglos — auditoría 2026-08-27

Plan de trabajo derivado de `AUDIT-2026-08-27.md` (16 hallazgos, verificados a mano
contra el binario real). Cada ítem acá es un checklist accionable: qué cambiar, dónde,
y cómo verificarlo — el detalle completo (repro, código exacto, razonamiento) queda en
el markdown de la auditoría, este archivo es solo el plan de ejecución.

Convención de esta sesión: cada ítem cerrado = commit propio + bump MINOR de versión +
test de regresión + verificación manual contra el binario real (nunca solo "compila")
+ entrada en CHANGELOG.md/GRAMMAR.md.

---

## Ronda 1 — Crítico (hacer primero, sin excepción)

- [x] **1. `crypto.randomToken(length)` puede tumbar el proceso entero** -- v1.122.0, GRAMMAR.md §3.165
  - Dónde: `compiler/src/runtime/mod.rs:2329-2342` (case `"randomToken"`)
  - Qué: rechazar `length` negativo y ponerle un techo explícito (unas pocas KB, ningún caso de uso real pasa de ahí) ANTES de llamar a `os_random_bytes` — mismo criterio que `crypto.randomInt`/`dateFromParts` ya usan para sus propios rangos.
  - Verificar: repetir el repro del audit (`length: -1` y `length: 9223372036854775807`) contra un `linkc serve` real — las dos deben dar un 400 limpio, el proceso debe seguir respondiendo `/health` después.
  - Test de regresión: unitario en `runtime/mod.rs` (longitud negativa y longitud absurdamente grande → `RuntimeError`, no panic) + repro end-to-end si aplica.

- [x] **2. `@cache` + `@authenticated`/`@requires` filtra datos entre usuarios** -- v1.122.0, GRAMMAR.md §3.165 (opción b: rechazado en compilación)
  - Dónde: `compiler/src/runtime/server.rs:982-985` (clave de caché), `compiler/src/cache.rs` (`CacheStore`), posiblemente `compiler/src/checker.rs` (`check_cache_annotation`)
  - Qué: decidir el diseño real antes de tocar código — dos caminos válidos:
    (a) incluir el identificador de sesión (o `userId`) en la clave de caché cuando el rpc lleva auth, o
    (b) rechazar en el checker la combinación `@cache` + `@authenticated`/`@requires` hasta que exista un diseño con scoping real.
    Recomendado: (b) primero (cierra el hueco de seguridad YA, sin arriesgar un diseño apurado), (a) como mejora posterior si hay demanda real.
  - Verificar: repetir el repro del audit (Alice/Bob, `myProfile` cacheado) — Bob NO debe ver los datos de Alice.
  - Test de regresión: end-to-end en `server_http.rs` con dos sesiones distintas contra el mismo rpc `@cache`+`@authenticated`.

---

## Ronda 2 — Alto (siguiente, mismo paquete de trabajo o el inmediato posterior)

- [x] **3. `Patch<T>`/`applyPatch` nunca corre `@validate` (ni `@check` en `--adopt-existing`)** -- v1.122.0, GRAMMAR.md §3.166
  - Dónde: `compiler/src/runtime/mod.rs`, brazo `Type::PatchOf(inner)` de `json_to_typed_value`
  - Qué: cablear `field_annotations_for` + `apply_field_validators` ahí, igual que el brazo `Type::Struct` — tolerando que el patch no traiga todos los campos (la función ya solo valida las claves presentes).
  - Verificar: repetir el repro del audit — `update` con un email inválido en el patch debe dar 400, igual que `create`.
  - Test de regresión: unitario que compare `create` vs `applyPatch` con el mismo valor inválido, los dos deben rechazar igual.

- [x] **4. `@idempotent` tiene una carrera TOCTOU (doble ejecución con la misma key)** -- v1.122.0, GRAMMAR.md §3.167. `@cache` (hallazgo #11, misma forma de carrera) quedó deliberadamente FUERA de esta ronda -- ver nota en Ronda 4.
  - Dónde: `compiler/src/runtime/server.rs` (el `lookup()`/`handle_rpc()`/`store()` de `@idempotent`), `compiler/src/idempotency.rs` (`IdempotencyStore`)
  - Qué: agregar un estado `Lookup::InFlight` — reservar la clave, correr el cuerpo, liberar/registrar el resultado, todo bajo una disciplina que impida que dos hilos vean `Miss` a la vez para la misma clave (mutex por clave, o el mismo candado sostenido de punta a punta). Mismo patrón que ya resolvió el problema análogo de `upsert` en v1.115.0.
  - Verificar: repetir el repro del audit — 30 requests concurrentes con la misma `Idempotency-Key` deben producir EXACTAMENTE 1 fila.
  - Test de regresión: test con hilos reales (`std::thread::spawn`, no simulado), mismo estilo que los tests de concurrencia ya existentes en `runtime/mod.rs`.
  - Nota: si el fix es genérico (un lock por clave), evaluar si conviene aplicar el mismo mecanismo a `@cache` (hallazgo #11) en el mismo commit — mismo bug, menor severidad.

---

## Ronda 3 — Medio (paquete de bugfix, estilo v1.119.0/v1.120.0)

- [x] **5. `insert()` panica si la fila se borra entre el INSERT y el SELECT de confirmación** -- v1.123.0, GRAMMAR.md §3.168
  - Dónde: `compiler/src/runtime/db.rs:1581-1588`
  - Qué: cambiar el `.expect("la fila recién insertada tiene que existir")` por un `.ok_or_else(...)` que devuelva `RuntimeError`, igual que ya hace `applyPatch` unas líneas más abajo.
  - Verificar: no hay repro trivial por timing — alcanza con el test unitario.
  - Test de regresión: simular la carrera a nivel de test (insertar y borrar entre medio con acceso directo a `Db`, sin depender de timing real de hilos).

- [x] **6. Agregaciones panican sobre una columna `NULL` heredada de una migración** -- v1.123.0, GRAMMAR.md §3.168
  - Dónde: `compiler/src/runtime/db.rs:2625-2650` (`scalar_cell_to_value`)
  - Qué: agregar un brazo para `Cell::Null` que devuelva `RuntimeError`, mismo mensaje/criterio que `row_to_fields` ya usa para "null_but_required".
  - Verificar: si hay Postgres disponible, repetir el repro de punta a punta (agregar campo requerido a colección con filas viejas, migrar, llamar `sumBy` agrupando por ese campo). Si no hay Postgres a mano, alcanza con un test unitario que llame `scalar_cell_to_value` directo con `Cell::Null`.
  - Test de regresión: unitario mínimo, + uno contra Postgres real si el entorno de CI lo permite.

- [x] **7. El checker acepta un rpc `@cron` como blanco de `@invalidates`** -- v1.123.0, GRAMMAR.md §3.168
  - Dónde: `compiler/src/checker.rs:1818-1852` (`check_invalidates_annotation`)
  - Qué: excluir explícitamente un blanco con `.cron().is_some()`, mismo criterio que los 6 sitios de codegen.
  - Verificar: repetir el repro del audit — `@invalidates(unRpcConCron)` debe ser un error de compilación, no un `OK` silencioso.
  - Test de regresión: unitario de checker (`invalidates_rejects_a_cron_target` o similar).

- [x] **8. `linkc doc` no muestra badges de auth/rate-limit/deprecated en un `stream`** -- v1.123.0, GRAMMAR.md §3.168
  - Dónde: `compiler/src/doc.rs`, brazo `Member::Stream(st)` de `render_service`
  - Qué: extraer el cálculo de badges a una función compartida entre los brazos `Rpc` y `Stream`, en vez de dos implementaciones independientes (la raíz del bug es duplicación, no solo el síntoma).
  - Verificar: repetir el repro del audit — un `stream` con `@requires`+`@rate_limit`+`@deprecated` debe mostrar los tres badges en el HTML generado.
  - Test de regresión: si `doc.rs` tiene tests existentes, sumar uno ahí; si no, al menos una verificación manual documentada en el commit/CHANGELOG.

- [x] **9. `GET /metrics` sostiene el candado de métricas mientras contiende por la conexión** -- v1.123.0, GRAMMAR.md §3.168
  - Dónde: `compiler/src/runtime/server.rs:819`
  - Qué: recolectar `db.subscriber_counts()`/`db.size_bytes()`/`db.oversized_notify_drop_counts()` ANTES de tomar `metrics_store.lock()`, sostenerlo solo para el formateo.
  - Verificar: no hay un repro de un solo request — es un cambio de orden de evaluación, revisar que `render_prometheus_text` siga recibiendo los mismos argumentos ya calculados.
  - Test de regresión: los tests de `/metrics` existentes (`cli_metrics.rs`) no deberían cambiar de comportamiento — correrlos como regresión, no hace falta un test nuevo salvo que se quiera medir contención directamente (opcional, bajo prioridad).

- [x] **10. `lint`: `mixed-service-auth` da falso positivo con `@cron` + rpcs protegidos** -- v1.123.0, GRAMMAR.md §3.168
  - Dónde: `compiler/src/lint.rs:23-28`
  - Qué: excluir `.cron().is_some()` del cálculo de `has_unauth`, mismo criterio que codegen.
  - Verificar: repetir el repro del audit — el programa con 2 rpcs `@authenticated` + 1 `@cron` no debe disparar el lint.
  - Test de regresión: unitario de lint (positivo: sigue disparando cuando hay un rpc genuinamente público; negativo: no dispara con `@cron` de por medio).

---

## Ronda 4 — Bajo / opcional (evaluar caso a caso, no urgente)

- [ ] **11. `@cache` con la misma carrera que `@idempotent`** — cubrir en el mismo commit que el #4 si el fix ahí es genérico; si no, evaluar aparte (menor severidad, "cache stampede" no es una escritura duplicada).
- [ ] **12. `@unique`/`@index` no son índices parciales respecto a `@softDelete`** — requiere índices parciales en los dos backends (`CREATE UNIQUE INDEX ... WHERE "campo" IS NULL`), cambio de arquitectura, no un one-liner. Como mínimo: documentar el límite en GRAMMAR.md si no se ataca esta ronda.
- [ ] **13. `--jwt-secret ""` / `--service-api-key ""` (string vacío por flag) activa la feature con secreto vacío** — aplicar el mismo `.filter(|v| !v.trim().is_empty())` al valor que viene de flag en `read_flag_or_env` (`main.rs:2122-2131`), igual que ya se aplica al de env var. Fix de una línea.
- [ ] **14. Panics de tipo-incompatible decodificando filas de tablas `--adopt-existing` con datos legado** — necesita su propio discovery (no verificado independientemente en el audit); revisar `row_to_fields`/`write_param` en `db.rs` y convertir los `panic!`/`.expect()` reachable a `RuntimeError`, mismo criterio que el resto del archivo.
- [ ] **15. Carrera check-then-act en la composición de `recordFailedLogin`/`failedLoginCount`** — probablemente aceptado por diseño (GRAMMAR.md §3.152 ya documenta que son piezas para componer a mano). Acción mínima: confirmar que el límite está explícitamente documentado en GRAMMAR.md; si no, agregar la nota. No priorizar un fix de runtime sin evidencia real de que importa a un adoptador.
- [ ] **16. `+`/`-`/`*` sobre `Int`/`Int64` sin `checked_*`** — ya conocido y aceptado fuera de `transaction{}`/`@cron` (GRAMMAR.md §3.163). En perfil `release` wrappea en silencio (bug de corrección, no de estabilidad). Evaluar si vale la pena cerrarlo del todo (`checked_add`/`checked_sub`/`checked_mul`, mismo patrón que `/` y `%` en v1.119.0) en una ronda futura — no es urgente, no es nuevo.

---

## Cómo seguir

Recomendación: atacar Ronda 1 completa en una sola sesión de trabajo (los dos hallazgos
críticos), shippear como versión propia con su propio ciclo completo (tests + docs +
verificación manual + CI verde) antes de tocar la Ronda 2. Rondas 2 y 3 pueden
empaquetarse juntas si el volumen de cambio por versión se mantiene razonable — mismo
criterio que ya se usó para v1.119.0 (3 bugs en un solo paquete).
