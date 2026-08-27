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

- [x] **11. `@cache` con la misma carrera que `@idempotent`** — evaluado y NO atacado a propósito: la semántica correcta sería "esperar al primero" (no rechazar con 409, que rompería el contrato de `@cache`), y esperar sincrónico acopla la latencia de requests no relacionados sin evidencia real de que importe. Documentado en GRAMMAR.md §3.144.
- [x] **12. `@unique`/`@index` no son índices parciales respecto a `@softDelete`** — evaluado y NO atacado a propósito: requiere índices parciales en los dos backends MÁS una migración segura para bases ya desplegadas (`DROP`+`CREATE` del índice viejo, riesgo real sobre datos de producción). Discovery hecho, documentado como límite honesto en GRAMMAR.md §3.80, diseño/implementación quedan para una ronda propia.
- [x] **13. `--jwt-secret ""` / `--service-api-key ""` (string vacío por flag) activa la feature con secreto vacío** -- v1.124.0, GRAMMAR.md §3.169. Fix puntual en `resolve_service_api_key`/`resolve_jwt_config`, no en `read_flag_or_env` (otros flags como `--host` tienen el contrato inverso, deliberado).
- [x] **14. Panics de tipo-incompatible decodificando filas de tablas `--adopt-existing` con datos legado** -- v1.124.0, GRAMMAR.md §3.169. Los 3 sitios reales de `row_to_fields` (JSON legado que no calza, `Cell` inesperada en columna JSON, tipo nativo no coincide) convertidos a `RuntimeError` limpio.
- [x] **15. Carrera check-then-act en la composición de `recordFailedLogin`/`failedLoginCount`** — documentado como límite honesto en GRAMMAR.md §3.152, no atacado con un mecanismo nuevo sin evidencia real de que importe.
- [x] **16. `+`/`-`/`*` sobre `Int`/`Int64` sin `checked_*`** -- v1.124.0, GRAMMAR.md §3.169. `checked_int_numeric_op` generalizada para cubrir los tres operadores (y el `-` unario, y `List<Int>.sum()`) -- ya no queda ningún operador aritmético entero sin `checked_*`. Efecto colateral: dos tests de §3.163/§3.164 que usaban desborde de `+` como disparador de panic se actualizaron (ese disparador específico ya no panica, ahora es un `RuntimeError` limpio).

---

## Estado: plan completo

Los 16 hallazgos de `AUDIT-2026-08-27.md` quedaron todos resueltos o documentados
explícitamente como límites conocidos, ninguno silenciado:

- **10 hallazgos cerrados con código real** (#1-10, #13, #14, #16 -- 13 en total),
  verificados con test unitario propio + repetición en vivo contra el binario real
  donde había repro directo.
- **3 hallazgos evaluados y documentados a propósito, sin cambio de código** (#11, #12,
  #15) -- cada uno con su razonamiento explícito en GRAMMAR.md sobre por qué no se
  atacó, no un olvido.

Shippeado en 4 versiones (v1.122.0 Ronda 1+2, v1.123.0 Ronda 3, v1.124.0 Ronda 4),
cada una con su propio ciclo completo (tests + docs + verificación manual + CI verde).
