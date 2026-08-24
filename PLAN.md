# Plan de Desarrollo: **c-script** — Lenguaje Backend con End-to-End Type Safety para TypeScript

> Documento de ingeniería. Versión 1.0 · Objetivo: plan realista, detallado y honesto para diseñar y construir un lenguaje backend compilado cuyo diferenciador es la **interoperabilidad de tipos nativa y automática con frontends TypeScript**.

---

## 0. Resumen ejecutivo (léelo primero)

**La idea es buena y llega en buen momento.** El mercado ya demostró que valora la type-safety de extremo a extremo (tRPC, Server Actions, Convex, Encore). El "punto ciego" entre backend y frontend es un dolor real.

**Pero la recomendación honesta es: NO empieces construyendo un lenguaje.** El valor diferencial de tu propuesta no vive en la sintaxis ni en el runtime — vive en **el puente de tipos** (la generación automática de `.d.ts` + cliente RPC + validadores). Puedes entregar el 80% del valor con el 5% del esfuerzo construyendo primero ese puente como **framework + codegen sobre Rust o Go**. Un lenguaje nuevo es la forma más cara, lenta y arriesgada de conseguir ese valor.

| | Construir un lenguaje nuevo | Construir el "puente de tipos" primero |
|---|---|---|
| Tiempo a primer valor | 6–12 meses | 3–8 semanas |
| Esfuerzo hasta algo usable | 4–8 persona-año | 0.3–1 persona-año |
| Riesgo de adopción | Muy alto | Bajo (usas un lenguaje que ya tiene ecosistema) |
| Diferenciador que pruebas | Todo a la vez | Exactamente el que importa (E2E type safety) |

Este documento cubre **ambos caminos**: el plan del lenguaje completo (porque lo pediste) y, en la sección 8, la ruta pragmática que recomiendo de verdad.

---

## 1. Análisis y Research

### 1.1 Comparativa con soluciones existentes

| Solución | Lenguaje backend | Type-safety E2E | Rendimiento | Codegen | Límite principal |
|---|---|---|---|---|---|
| **tRPC** | TS/JS (Node/Bun/Deno) | ✅ inferencia | Runtime JS | No | Backend atado a TS; sin perf de sistemas |
| **Encore.ts** | TS + runtime Rust | ✅ genera cliente | Alto (I/O en Rust) | Parcial | Sigue siendo TS; opinado |
| **Convex** | TS | ✅ | Medio (gestionado) | No | Plataforma cerrada; TS |
| **Elysia + Eden** | TS (Bun) | ✅ inferencia | Alto (Bun) | No | Solo TS |
| **Nestia / typia** | TS (Nest) | ✅ SDK gen | Runtime JS | Sí | Solo TS |
| **Rust + ts-rs / specta** | Rust | ⚠️ solo tipos | Muy alto | Sí | No genera cliente RPC ni valida el cable |
| **Go + tygo / oapi-codegen** | Go | ⚠️ vía OpenAPI | Muy alto | Sí | Contrato indirecto, no nativo |
| **gRPC + protobuf / Connect** | Cualquiera | ✅ vía IDL | Muy alto | Sí | IDL aparte; DX web pobre; no idiomático en TS |
| **Fern / Smithy** | Cualquiera | ✅ vía IDL | N/A | Sí | Escribes un IDL separado del código |
| **WASM Component Model (WIT)** | Cualquiera | ✅ vía WIT | Alto | Sí | Inmaduro; overhead de bindings |
| **c-script (propuesto)** | **Nuevo, compilado** | **✅ nativo** | **Alto (nativo/WASM)** | **Automático** | **Coste de crear un lenguaje** |

### 1.2 El gap que justificaría un lenguaje nuevo

Ninguna solución actual reúne **las tres cosas a la vez**:

1. **Rendimiento y seguridad de lenguaje de sistemas** (Rust/Go), y
2. **Type-safety E2E con TypeScript sin escribir un IDL aparte ni interfaces duplicadas**, y
3. **DX de "llamada a función" transparente** (no `fetch`, no `.proto`, no anotaciones manuales).

- Las soluciones **TS-first** (tRPC, Convex, Elysia) dan (2) y (3) pero no (1).
- Las soluciones **codegen desde Rust/Go** (ts-rs, tygo, gRPC) dan (1) pero pagan un IDL/anotación separado y no logran del todo (3).

El hueco real es: *"un lenguaje donde el sistema de tipos se diseñó desde el día 1 para ser isomórfico con TypeScript, de modo que el contrato ES el código."*

### 1.3 Riesgos principales (resumen; detalle en §7)

- **Adopción**: el mayor asesino de lenguajes nuevos. Sin ecosistema (DB, auth, libs) nadie migra.
- **Coste**: se subestima sistemáticamente en 3–5×.
- **Mantenimiento**: un lenguaje es un compromiso de años, no de meses.

---

## 2. Diseño del Lenguaje

### 2.1 Filosofía

1. **El contrato es el código.** Definir un tipo en el backend *es* definir su interfaz TS. Cero duplicación.
2. **Isomorfismo con TypeScript por diseño.** Cada construcción del sistema de tipos tiene un mapeo canónico y estable a TS.
3. **Romper en compilación, nunca en producción.** Un cambio incompatible en el backend debe romper el `tsc` del frontend.
4. **Seguro y rápido por defecto.** Memory-safe, sin data races evidentes, rendimiento de sistemas.
5. **Serialización predecible.** Todo tipo público tiene una forma JSON determinista y validable.

### 2.2 Sintaxis (borrador)

```rust
// users.link — un módulo = un contrato
type User = {
  id: Int,
  name: String,
  email: String,
  role: Role,
  createdAt: Timestamp,
}

// enum simple  ->  unión de literales string en TS
enum Role { Admin, Member, Guest }

// enum con datos (ADT)  ->  unión discriminada en TS
enum Result {
  Ok  { value: User },
  Err { code: Int, message: String },
}

// `service` expone RPCs. Cada rpc público entra al contrato .d.ts
service Users {
  // parámetro con default -> parámetro opcional en TS
  rpc list(limit: Int = 20) -> [User] {
    db.users.all().take(limit)
  }

  // T? -> T | null (decisión de diseño, ver §2.3)
  rpc getById(id: Int) -> User? {
    db.users.find(id)
  }

  rpc create(input: NewUser) -> Result {
    match validate(input) {
      Ok(u)  => Result.Ok { value: db.users.insert(u) },
      Err(e) => Result.Err { code: 422, message: e },
    }
  }

  // streaming -> AsyncIterable en el cliente TS
  stream watch(id: Int) -> User {
    db.users.subscribe(id)
  }
}
```

### 2.3 Sistema de tipos — el mapeo 1:1 a TypeScript (el corazón del proyecto)

> **Esta tabla es el borrador original de la propuesta, no el estado real.**
> La tabla de mapeo vigente y exhaustiva vive en [GRAMMAR.md §4](GRAMMAR.md);
> ahí está lo que el compilador de verdad emite. Se conservan acá las filas
> tal como se propusieron, marcando cuáles no se construyeron -- borrarlas
> escondería qué se prometió al principio y qué no llegó.

| Tipo en c-script | Tipo en TypeScript | Forma JSON | ¿Existe? |
|---|---|---|---|
| `Int`, `Float` | `number` | número | sí |
| `Int64`, `BigInt` | `bigint` \| `string`* | string (para no perder precisión) | sí, como `Int64` — pero TS `string`, no `bigint` (GRAMMAR.md §3.30: el cliente generado no tiene hoy ningún punto de (de)serialización dirigido por tipo, y `string` no necesita ninguno; `bigint` real queda como ronda futura separada) |
| `String` | `string` | string | sí |
| `Bool` | `boolean` | bool | sí |
| `[T]` | `T[]` | array | la sintaxis real es postfija: `T[]` |
| `{K: V}` (map) | `Record<K, V>` | objeto | la sintaxis real es `Map<K, V>` — `{K: V}` como literal de tipo se descartó por ambigüedad con un struct de un campo (GRAMMAR.md §2.2) |
| `(A, B)` (tupla) | `[A, B]` | array | sí |
| `type X = { ... }` | `type X = { ... }` (estructural) | objeto | sí (se emite como `interface`) |
| `enum E { A, B }` | `type E = "A" \| "B"` | string | sí |
| `enum` con datos | unión discriminada con `type` tag | objeto etiquetado | sí |
| `T?` | `T \| null` **(decisión, ver abajo)** | `null` presente | sí |
| `field?: T` | `field?: T` (clave ausente) | clave omitida | sí |
| `Timestamp` | `string` (ISO-8601) \| branded | string | sí -- string plano, no branded (GRAMMAR.md §3.31: mismo criterio minimalista que el resto del proyecto, revisar si aparece un caso real que pida la distinción nominal) |
| `Void` / `Unit` | `void` | — | `Void` sí; solo como retorno completo de un rpc (GRAMMAR.md §4.1) |

**\* La decisión de diseño más importante y con más matices** es cómo representar **ausencia**. TypeScript distingue tres cosas que JSON no distingue bien:

- `field: T | null`  → la clave **existe**, el valor es `null`.
- `field?: T`        → la clave puede **no existir** (`undefined`).
- `field?: T | null` → ambos.

Esto afecta a la serialización, a los validadores generados y a la DX. Es una bifurcación real con trade-offs, no un detalle. (En §8.3 esta es exactamente la decisión que te propongo implementar tú en el PoC.)

**ADT → unión discriminada.** El `enum Result` de arriba genera:

```typescript
export type Result =
  | { type: "Ok";  value: User }
  | { type: "Err"; code: number; message: string };
```

La convención del tag (`type` vs `kind` vs `_tag`) debe ser configurable pero con un default estable.

### 2.4 Modelo de ejecución

Tres candidatos, en orden de "rápido de construir" → "rápido de ejecutar":

| Estrategia | Perf runtime | Esfuerzo | Cuándo |
|---|---|---|---|
| **Intérprete** (tree-walking / bytecode) | Baja–media | Bajo | **MVP (Fase 0)** — prueba el valor ya |
| **Transpilar a Go/Rust** y usar su toolchain | Muy alta | Medio | Alternativa de Fase 1 con menos riesgo |
| **Compilar a WASM** (`wasm-encoder` directo, o recompilar el runtime a `wasm32-wasip1`) | Alta | Alto | Edge/serverless; Fase 1–2 |
| **Compilar a nativo** (LLVM) | Máxima | Muy alto | Fase 2–3 |

**Recomendación:** MVP interpretado o transpilado a Go. El backend de compilación "de verdad" hacia WASM llega en Fase 1, **después** de haber probado el killer feature. No inviertas en LLVM hasta que la propuesta de valor esté validada.

**Corrección (post-Fase-0):** esta sección originalmente decía "Cranelift → WASM" como si fueran la misma cosa — no lo son. Cranelift solo genera código de máquina NATIVO (x86/arm/etc.); `cranelift-wasm` CONSUME bytes `.wasm` como entrada para traducirlos a Cranelift IR (así es como wasmtime hace JIT), no los produce. Para emitir un `.wasm` real hay dos caminos genuinos: `wasm-encoder` (autoría directa de bajo nivel, activamente mantenido, usado por 800+ crates) para codegen nativo hacia wasm; o, como v0 pragmático — y el que efectivamente se usó en Fase 1 — recompilar el runtime interpretado existente al target `wasm32-wasip1` y alimentarle el programa ya parseado como dato, el mismo camino que siguieron RustPython y Boa para su primera versión real en wasm. Cranelift sigue siendo la opción correcta si lo que se quiere es codegen nativo (vía `cranelift-jit`/`cranelift-object`), no como atajo hacia wasm.

**v0 implementado y verificado (Fase 1):** `compiler/src/bin/wasm_demo.rs` -- el intérprete existente (`Rc<RefCell<Value>>` en runtime/mod.rs, `Mutex<Vec<Value>>` en db.rs) compila tal cual para `wasm32-wasip1` y ejecuta una llamada RPC real (`Users.getById`) de punta a punta dentro de `wasmtime`, devolviendo el JSON correcto. Requirió partir el paquete en `[lib]` + `[[bin]]` (`compiler/src/lib.rs`) porque un binario en `src/bin/*.rs` compila como un crate aparte del binario principal, aunque viva en el mismo paquete Cargo. El demo usa `include_str!` (embebido en tiempo de compilación) en vez de leer el archivo `.link` en runtime, a propósito: **no probado, y deliberadamente no perseguido más allá de esta ronda** -- correr la suite de tests completa (`cargo test --target wasm32-wasip1`) falla, porque varios tests existentes sí leen archivos de fixture del disco en runtime (`env!("CARGO_MANIFEST_DIR")` + `fs::read_to_string`), y un sandbox WASI no tiene acceso al filesystem del host salvo que se le otorgue explícitamente ("preopens" de directorio) -- eso es trabajo real y separado (configurar qué directorios exponer y a qué tests), no algo que debiera colarse a medio hacer acá. Tampoco prueba nada de concurrencia real (wasm32 es de un solo hilo) ni toca networking (`tiny_http` asume sockets de SO reales; WASI Preview 2 (`wasi:http`) es la vía idiomática si se retoma esto, adaptador aparte).

---

## 3. Arquitectura Técnica del Compilador

### 3.1 Pipeline completo

```
código .link
   │
   ▼
[1] Lexer ───────────► tokens
   │
   ▼
[2] Parser ──────────► AST (sintaxis)
   │
   ▼
[3] Análisis semántico
    · resolución de nombres/módulos
    · type checking / inferencia
    · chequeos de "contrato" (¿es serializable?) ─► AST TIPADO
   │
   ├──────────────► [4a] Codegen backend ──► WASM / nativo / bytecode  (SERVIDOR)
   │
   └──────────────► [4b] Emisor de contrato ──► .d.ts + cliente TS + validadores  (CLIENTE)
```

La clave arquitectónica: **[4b] es un pass de primera clase**, no un añadido. Recorre el AST tipado y emite:
1. `contract.d.ts` — los tipos.
2. `client.ts` — un cliente RPC tipado (thin wrapper sobre `fetch`/WebSocket).
3. `validators.ts` — guardas runtime generadas desde los tipos (esto es lo que hace la seguridad *real* en el borde, no solo compile-time).

### 3.2 Tecnologías recomendadas por etapa

| Etapa | Recomendación | Alternativas |
|---|---|---|
| Lenguaje del compilador | **Rust** (ecosistema de compiladores maduro) | Zig, OCaml, TS |
| Lexer | `logos` (Rust) o hand-written | — |
| Parser | recursivo descendente hand-written o `chumsky` | `lalrpop`, tree-sitter |
| Representación | AST tipado propio + arena (`id-arena`) | — |
| Type checker | propio (algoritmo bidireccional) | — |
| Backend de código | **Cranelift** (compila rápido, código NATIVO — no emite `.wasm` directamente, ver nota de §2.4) | LLVM (más perf, más complejo; sí tiene target `wasm32` real), transpilar |
| Emisión `.d.ts` | pass propio sobre AST tipado | — |
| Validadores runtime | generar TS estilo Zod/typia | — |
| Serialización wire | JSON (MVP) → luego formato binario opcional | MessagePack, Protobuf |

### 3.3 Cómo se implementa la generación de contratos (concreto)

Un mapeo puro `TipoLink → TipoTS` recorriendo el AST tipado. Núcleo (pseudocódigo Rust):

```rust
fn emit_ts(t: &Type) -> String {
    match t {
        Type::Int | Type::Float => "number".into(),
        Type::String            => "string".into(),
        Type::Bool              => "boolean".into(),
        Type::List(inner)       => format!("{}[]", emit_ts(inner)),
        Type::Optional(inner)   => format!("{} | null", emit_ts(inner)), // ← decisión §2.3
        Type::Named(name)       => name.clone(),
        Type::Enum(variants)    => emit_discriminated_union(variants),
        // ...
    }
}
```

El servidor RPC y el `client.ts` comparten el mismo emisor de tipos, garantizando que **no puedan divergir**.

---

## 4. Roadmap por Fases

| Fase | Duración | Objetivo | Entregable clave | Equipo mínimo |
|---|---|---|---|---|
| **Fase 0 · MVP** | 2–4 meses | Probar el killer feature E2E | Lexer + parser + checker mínimo · emisor `.d.ts` + cliente · runtime interpretado o transpilado · **1 demo full-stack donde cambiar el backend rompe `tsc`** | 1–2 ing. de compiladores |
| **Fase 1 · Alpha** | +4–6 meses | Compilar de verdad | Backend a WASM (v0: runtime recompilado a `wasm32-wasip1`; codegen directo vía `wasm-encoder` como evolución) · runtime RPC HTTP · std mínima (los builtins ya existentes en v0: `.length()`/`.contains()`/`.toFloat()`/`.toInt()`/`db.*`) · CLI (`linkc new/dev/build`) · LSP básico | 2–3 |
| **Fase 2 · Beta** | +6–9 meses | Usable en proyectos reales | DB tipada · auth · WebSocket/SSE · validadores runtime · hot reload · LSP completo · package manager · observabilidad | 3–5 |
| **Fase 3 · 1.0** | +6–12 meses | Producción | Estabilidad de sintaxis · ecosistema · docs · deploy edge/serverless · debugging con source maps | 4–6+ |

**v0 implementado y verificado (Fase 2, de los ítems de esta fila: validadores runtime, DB tipada, WebSocket/SSE y auth):** ver GRAMMAR.md §3.11 (`validators.ts`), §3.12 (`db { ... }`, tipado -- ver §3.17 para la persistencia real), §3.13 (`stream` sobre SSE real, no WebSocket -- HTTP chunked transfer alcanza para "una secuencia ya calculada, sin suscripción a eventos futuros", que era el alcance de esa ronda), §3.15 (constructo de loop `while`, prerrequisito del ítem siguiente), §3.16 (push real: eventos genuinamente futuros, resuelto TAMBIÉN sobre SSE -- la suposición original de que esto necesitaría WebSocket resultó innecesaria para el shape fijo `while true { db.<col>.subscribe() }`; WebSocket seguiría haciendo falta solo para push BIDIRECCIONAL, que sigue sin construirse), §3.17 (`db` sobre SQLite real -- `rusqlite`, persistencia genuina entre reinicios, único backend nativo+wasm32 verificado con un spike real) y §3.14 (`@authenticated`/`@requires(Role.X)` + sesiones opacas en memoria, sin JWT).

**`LSP completo` de esta misma fila: RESUELTO, Nivel 1+2+3 -- los 3 niveles, no solo los primeros dos.** Los 3 prerrequisitos (spans+diagnósticos, recuperación de errores del parser, spans en AST/checker) más el protocolo en sí (`linkc lsp` -- JSON-RPC sobre stdio, diagnósticos con imports resueltos de verdad vía `load_program_with_overlay`) ya estaban -- ver GRAMMAR.md §3.19 para el diseño Nivel 1+2 (hover/completion/goto-def a nivel de declaración). El Nivel 3 que esa sección dejaba explícitamente pendiente (goto-def de un nombre de tipo en una firma, hover de una expresión arbitraria en medio de un body, completion sensible al tipo real tras `x.`) se resolvió completo en 3 rondas encadenadas -- GRAMMAR.md §3.21, §3.24, §3.25 -- la última (hover/completion) reusando una única instrumentación mínima del checker bidireccional (`Checker::hover_type_at`, un "probe" en los dos puntos de entrada unificados `synth_expr`/`check_expr`) en vez de reimplementar las reglas de scoping en el LSP. Identidad de archivo en `Span` (GRAMMAR.md §3.22) y spans en `Field`/`Param` (§3.23) fueron los dos prerrequisitos que esa ronda expuso y cerró de paso.

**"Backend a WASM" de esta fila: decisión de roadmap tomada, ambos caminos con su rol final.** El "codegen directo vía `wasm-encoder`" que esta fila nombraba como evolución futura existe en v0 (`linkc wasm`, GRAMMAR.md §3.20) -- alcance mínimo a propósito (solo aritmética entera sobre `Int`/`Bool`, una función es una sola expresión). Auditoría post-push: se decide CONGELARLO ahí, no seguir invirtiendo -- cerrar la brecha hasta soportar un programa real (statements, `String`/structs/`db`, llamadas entre funciones) es en la práctica escribir un backend de codegen nativo completo, meses de trabajo, no una extensión incremental. El target `wasm32-wasip1` (recompilar el intérprete entero) es y sigue siendo el ÚNICO camino real de producción -- ya corre un programa real de punta a punta dentro de `wasmtime` (§2.4). Ver GRAMMAR.md §3.20 para el razonamiento completo, incluyendo por qué `cranelift-jit`/`cranelift-object` (no `wasm-encoder`) seguiría siendo la herramienta correcta si algún día hace falta codegen nativo de verdad.

**`package manager` de esta fila: RESUELTO en su forma "Git-as-registry", no un registro en red centralizado.** `link.json` ahora acepta `git+<url>#<rev>` como dependencia real, además de una ruta local -- resuelto invocando el binario `git` real (clonar/fetch/checkout, GRAMMAR.md §2.1) contra un caché local por proyecto, con `link.lock` grabando el commit exacto resuelto. Sin un registro centralizado tipo npm/crates.io (decisión consciente, mismo espíritu que evitar infraestructura nueva sin necesidad real) -- la URL git ES el "registro", igual que en Go. Queda como límite de v0, no como brecha a cerrar de forma incremental: `link.lock` es informativo (no un pin real que sobreviva a un `rev` de rama que avanzó) y no hay locking entre procesos concurrentes -- ver GRAMMAR.md §2.1 para el detalle completo.

**`observabilidad` de esta misma fila: RESUELTO, v0.** Tracing estructurado por RPC (`[req N] method=Users.create status=200 duration_ms=7`, GRAMMAR.md §3.26) sobre el `req_id` que ya existía como prerrequisito parcial -- formato `clave=valor` greppable, sin sumar `tracing`/OpenTelemetry todavía. Queda para una ronda futura: salida JSON, niveles de log configurables, y métricas agregadas -- ver GRAMMAR.md §3.26 para el detalle completo de qué falta.

**`hot reload` de esta misma fila: RESUELTO, v0.** `linkc dev <archivo> <outdir> [puerto]` (GRAMMAR.md §3.27) -- con el puerto opcional, cada rebuild exitoso reinicia un `linkc serve` hijo real con el programa actualizado (restart de proceso, no un hot-swap en memoria -- decisión deliberada para no tocar el modelo de threading de `runtime/server.rs`, ver §3.13). Un rebuild fallido nunca tira abajo el servidor. Con esto, los OCHO ítems de esta fila de Fase 2 (DB tipada, auth, WebSocket/SSE -- vía SSE real, no WebSocket, ver §3.13/§3.16 -- validadores runtime, hot reload, LSP completo, package manager, observabilidad) tienen al menos una v0 real y verificada -- cada uno con sus propios límites honestos documentados en su sección de GRAMMAR.md correspondiente, no pendientes de "empezar" en el sentido en que lo estaban al escribir este roadmap originalmente.

**`Testing` de §5 ("runner integrado + tests de contrato"): RESUELTO EN AMBAS MITADES, v1.0.0.**
1. **Tests de contrato**: `linkc test <archivo> <snapshot> [--update]` (GRAMMAR.md §3.29) compara el contrato emitido contra un snapshot commiteado a git con diff LCS línea a línea. Dogfooded en CI en cada commit (`examples/users.link.snap`).
2. **Runner integrado de comportamiento**: `test "nombre" { ... }` (GRAMMAR.md §3.33) con builtins `assert(cond, msg)` y `panic(msg)`, invocación directa de servicios `Service.rpc(...)`, y aislamiento automático de base de datos (`:memory:`) por test al ejecutar `linkc test <archivo.link>`.

**`Fase 3 · 1.0 (Producción)`: RESUELTO y publicado en v1.0.0.**
- Sistema de tipos bidireccional completo con inferencia, subtipado estructural para structs, nominal para enums, uniones `A | B`, genéricos monomorfizados y closures reales de primera clase.
- Tipos `Timestamp` (ISO-8601 UTC) e `Int64` (mismo rango 64-bit sin pérdida de precisión en TS), más builtin `now() -> Timestamp` (GRAMMAR.md §3.30–§3.32).
- Toolchain integral: `linkc build`, `serve`, `test`, `dev`, `lint`, `doc`, `docker`, `lsp`, `new`, `fmt`.

**Evolución Post-1.0 (v1.1.0 a v1.28.0) — Capacidades Enterprise y Cierre de Gaps Reales:**
- **PostgreSQL en Runtime** (v1.1.0/v1.4.0/v1.8.0, GRAMMAR.md §3.36/§3.40/§3.44): Soporte de base de datos PostgreSQL real (`--db postgres://...`), auto-migraciones de esquema no destructivas, TLS oportunista y obligatorio vía `rustls` puro (compatible con Supabase/Neon/RDS), auto-reconexión transparente tras corte y LISTEN/NOTIFY en hilo dedicado para sincronizar `stream` SSE entre múltiples instancias.
- **Criptografía y Seguridad** (v1.1.0/v1.3.0/v1.5.0, GRAMMAR.md §3.34/§3.38/§3.41): Argon2id (RFC 9106) con sal aleatoria en formato PHC y verificación en tiempo constante; tokens de sesión y UUIDs alimentados por el CSPRNG del sistema operativo (`getrandom`); cálculo de HMAC-SHA256 para verificación de webhooks; CORS con allowlist configurable (`--cors-origin`) y cabeceras de seguridad estrictas fijas (`nosniff`, `DENY`, `no-referrer`).
- **Extensibilidad Web y SEO** (v1.1.0/v1.2.0/v1.6.0/v1.9.0/v1.10.0, GRAMMAR.md §3.35/§3.37/§3.42/§3.45/§3.46): Decorador `@content_type("...")` para respuestas no-JSON (HTML, XML, CSV); URLs amigables `@route("/...")` con múltiples parámetros dinámicos y precedencia determinística; sanitización explícita `String.escapeHtml()`; y selección de status HTTP en éxito `response.setStatus(code)` (e.g. páginas 404 personalizadas o 201 Created).
- **Integraciones y Operaciones** (v1.3.0/v1.7.0/v1.11.0, GRAMMAR.md §3.38/§3.43/§3.47): Lectura de entorno `env.get()`, inspección de cuerpo crudo y cabeceras entrantes `request.rawBody()` / `request.header()`; límite de peticiones `@rate_limit("N/ventana")` con token bucket continuo; envío de correos vía relay `smtp.send(to, subject, body)` con TLS; y peticiones HTTP salientes con cabeceras `http.getWithHeaders` / `http.postWithHeaders`.
- **Motor de Consultas y Autorización Avanzada** (v1.12.0 a v1.18.0, GRAMMAR.md §3.48–§3.54): Paginación empujada a SQL nativo `db.<c>.page(limit, offset)`; agregación analítica nativa con `GROUP BY` en base de datos (`sumBy`, `countBy`, `avgBy`, `maxBy`, `minBy`) preservando tipos reales; autorización con OR de roles `@requires(Role.Admin | Role.Agent)`; expiración temporal de sesiones `--session-ttl` (o `LINK_SESSION_TTL`); introspección de sesión `auth.currentRole() -> String?`, emisión con identidad `auth.createSessionWithId(role, userId)` y lectura de identificador `auth.currentUserId() -> Int?`; y aleatoriedad numérica/comparación segura para código de usuario `crypto.randomInt(min, max)` / `crypto.timingSafeEqual(a, b)`.
- **Ergonomía del Lenguaje y Rutas Avanzadas** (v1.19.0, GRAMMAR.md §3.55–§3.57): Conversión explícita `.toString()` sobre `Int`/`Int64`/`Float`/`Bool` (primer método que existe sobre `Bool` en todo el lenguaje); `response.setStatus` ahora se rechaza en COMPILACIÓN dentro de un `stream` (antes era un no-op silencioso que solo se notaba en producción); segmento catch-all `:nombre*` en `@route` para rutas de profundidad variable (documentación, CMS), con precedencia determinística frente a rutas más específicas y detección de conflictos extendida.
- **Seguridad Configurable y Adopción de Bases Existentes** (v1.20.0, GRAMMAR.md §3.58–§3.59): Costo de `crypto.hashPassword` configurable vía `--argon2-memory-kib`/`--argon2-iterations` (o sus env vars), sin cambiar el comportamiento por default; `crypto.isLegacyHash(hash) -> Bool` para migrar contraseñas viejas de forma proactiva; y una tabla PostgreSQL preexistente con `id SERIAL`/`IDENTITY` de 32 o 16 bits (no solo `BIGSERIAL`) ya no falla en el primer `insert` -- corrige un desacuerdo real entre la validación al conectar (que sí los aceptaba) y la lectura de la columna (que exigía el OID exacto de 64 bits).
- **Inspección de Respuestas HTTP Salientes** (v1.21.0, GRAMMAR.md §3.60): `http.getWithStatus`/`http.postWithStatus` devuelven `{status: Int, headers: {...}[], body: String}` -- un 4xx/5xx de la API llamada deja de ser un error de runtime genérico y pasa a ser un dato que el programa puede inspeccionar (ej. reintentar solo en 429). `http.get`/`http.post`/`http.getWithHeaders`/`http.postWithHeaders` quedan sin cambios.
- **Paginación por Cursor** (v1.22.0, GRAMMAR.md §3.61): `db.<c>.pageAfter(cursor, limit)` -- el cursor es el `id` del último elemento visto (`null` para la primera página), estable bajo escritura concurrente a diferencia de `page(limit, offset)`, que cuenta filas desde el principio en cada llamada. `page` queda sin cambios, sigue siendo la opción correcta para saltar a una página arbitraria.
- **`@route` con Query String** (v1.23.0, GRAMMAR.md §3.62): cualquier parámetro del rpc que no esté en el path se lee de la query string por nombre (`String`/`Int` obligatorio, `String?`/`Int?` opcional). De paso, corrigió un bug real: la query string se coleaba entera dentro del último segmento de path capturado (ej. `/blog/slug?utm_source=x` corrompía `:slug`) -- ahora se separa antes de partir en segmentos, para toda ruta, tenga o no parámetros de query declarados.
- **`smtp` a Varios Destinatarios y HTML** (v1.24.0, GRAMMAR.md §3.63): `smtp.sendToMany(to, subject, body)` manda un mensaje con un `RCPT TO` por destinatario; `smtp.sendHtml(to, subject, html)` manda cuerpo HTML. `send` queda sin cambios. Envío asíncrono sigue pendiente (§8.3.3).
- **Auth Externo (JWT HS256)** (v1.25.0, GRAMMAR.md §3.64): `linkc serve --jwt-secret <secreto>` verifica un JWT ya emitido por un backend existente, junto con -- nunca en vez de -- las sesiones propias. `@requires`/`@authenticated`/`auth.currentRole()`/`currentUserId()` funcionan igual sin importar cuál de los dos autenticó. Solo HS256 (allowlist, no blocklist -- `"alg":"none"` y cualquier otro se rechazan); sin RS256/JWKS, eso queda para una ronda propia si hace falta un proveedor de identidad completo.
- **Agregación: soporte de `Int64`** (v1.26.0, GRAMMAR.md §3.65): `sumBy`/`countBy`/`avgBy`/`maxBy`/`minBy` aceptan `Int64` como campo de agrupación Y de valor -- de paso corrigió un bug real (`scalar_cell_to_value` nunca distinguía `Int64` de `Int` a nivel de storage, así que un resultado `Int64` habría llegado mal etiquetado y serializado como número en vez de string). Truncado de fechas sigue pendiente, ronda propia (§8.2.1) -- los dos backends divergen de verdad para truncar.
- **`linkc introspect`** (v1.27.0, GRAMMAR.md §3.66): genera un `.link` de partida (`type`/`db {...}`) leyendo `information_schema` de una base PostgreSQL ya existente -- solo PostgreSQL, sin FKs/índices/constraints, sin generar ningún `service`, y cualquier columna sin mapeo confiable (`jsonb`, `uuid`, timestamp nativo) sale como `String` con una advertencia explícita en vez de omitirse.
- **`--adopt-existing`: adoptar tablas sin auto-migrar** (v1.28.0, GRAMMAR.md §3.67): `linkc serve --adopt-existing` (o `LINK_ADOPT_EXISTING`) hace que cada colección declarada asuma que su tabla ya existe -- nunca ejecuta `CREATE TABLE` ni `ALTER TABLE`, ni siquiera el tipo no destructivo de siempre, solo SELECTs de solo lectura que confirman que cada columna declarada esté ahí. Resuelve dos bloqueos reales: un rol de base sin permiso de DDL (común en producción), y una tabla SQLite con columnas físicas que el `.link` no modela (antes hacía panic ante cualquier columna de más). Todo o nada por proceso, no valida `NOT NULL`/tipo columna por columna más allá de `"id"` -- límites honestos documentados en GRAMMAR.md §3.67.

**Hitos "go / no-go":**
- Fin de Fase 0: ✅ Demo E2E probada y verificada.
- Fin de Fase 1: ✅ Herramientas CLI, LSP inicial y soporte WASI validados.
- Fin de Fase 2: ✅ DB SQLite embebida, SSE reactivo, auth y generador de contratos validados.
- Fin de Fase 3: ✅ Suite 1.0 lista con 573 pruebas automatizadas continuas.

---

## 5. Ecosistema y Herramientas

- **Package manager**: `link.json` con dependencias locales y dependencias Git remotas (`git+https://...#rev`), con lockfile criptográfico `link.lock` (SHA-256).
- **CLI**: `linkc new` (scaffolding Next.js/Vite/Minimal), `linkc dev` (hot reload interactivo con reinicio de servidor), `linkc build`, `linkc serve`, `linkc test` (unitario + snapshots), `linkc lint`, `linkc doc`, `linkc docker`, `linkc fmt`, `linkc lsp`.
- **LSP**: Protocolo JSON-RPC 2.0 completo en stdio con Nivel 1, 2 y 3 (diagnósticos en tiempo real, spans UTF-16, autocompletado sensible al tipo del receptor `x.`, hover de tipos de expresiones y salto a definición multi-archivo). Extensión oficial para VS Code y Cursor (`c-script-vscode-1.0.0.vsix`).
- **Testing**: Runner de tests de comportamiento `test "..." { assert(...) }` con DB aislada por test y verificación de snapshots de contrato con diff LCS (`linkc test <file.link> <file.snap>`).
- **Observabilidad**: Tracing estructurado por RPC con identificador de petición (`req_id`), método, código de estado y duración en milisegundos.
- **Integraciones de Almacenamiento y Auth**: SQLite nativo embebido con auto-migración y PostgreSQL con TLS, auto-reconexión y LISTEN/NOTIFY distribuido. Autenticación declarativa RBAC con sesiones opacas, Argon2id, roles (`auth.currentRole`), id de usuario (`auth.createSessionWithId`/`auth.currentUserId`) y expiración configurable.

---

## 6. Estrategia de Adopción y Comunidad

- **Cuña inicial**: Equipos full-stack TypeScript que buscan el rendimiento y robustez de un backend de sistemas sin sacrificar la inferencia de tipos inmediata en el frontend.
- **"Time to wow" < 5 minutos**: `linkc new my-app` → modificar un campo en `main.link` → `tsc` del frontend falla al instante en desarrollo.
- **Docs y plantillas**: Integración oficial con Next.js 14 App Router, Vite+React y Backend puro.
- **Transparencia y Calidad**: Documentación probada por el propio compilador (`compiler/tests/docs_examples.rs`), 574 tests en CI continuo y límites documentados sin promesas falsas.

---

## 7. Riesgos, Mitigaciones y Costes

| Riesgo | Prob. | Impacto | Mitigación Aplicada / Estado |
|---|---|---|---|
| Adopción nula | Alta | Crítico | Experiencia DX superior: cero IDLs separados, cliente TS + validadores Zod generados automáticamente |
| Type system no mapea 1:1 a TS | Media | Alto | Isomorfismo desde el diseño: suite exhaustiva de tests que compilan y validan el cliente emitido contra `tsc` |
| Ecosistema ausente (DB, auth, net) | Alta | Alto | Baterías incluidas: drivers SQLite y Postgres nativos, auth Argon2id, HTTP cliente/servidor, SMTP y SSE integrados |
| Divergencia checker vs runtime | Media | Alto | Verificación empírica continua: cada feature se prueba contra servidores y bases de datos reales en CI |
| Mantenimiento y complejidad | Media | Alto | Arquitectura modular en Rust (lexer/parser/checker/codegen/runtime) sin dependencias nativas pesadas (p.ej. pure `rustls`) |

---

## 8. Hoja de Ruta Futura (Fase 4 · Hacia c-script 2.0)

Con las Fases 0 a 3 completadas y el núcleo v1.28.0 plenamente operativo, las siguientes prioridades definen la evolución hacia la versión 2.0:

### 8.1 Ecosistema y Distribución
1. **Publicación en el registro npm**: Empaquetar y publicar `link-lang` en npm para permitir ejecución vía `npx linkc` o instalación global estándar.
2. **Ampliación del Playground Web**: Soporte para resolución de múltiples archivos y ejecución simulada de tests en el navegador mediante `wasm32-unknown-unknown`.

### 8.2 Base de Datos y Consultas Avanzadas
1. **Agregación con truncamiento temporal**: Soporte para agrupar por fechas truncadas (`date_trunc` para cohortes diarias/mensuales) en `sumBy`/`countBy`/etc. -- deliberadamente separado del soporte de `Int64` (ya resuelto, GRAMMAR.md §3.65): los dos backends divergen de verdad para truncar una fecha (Postgres necesita `to_timestamp`/`EXTRACT(EPOCH ...)` antes de `DATE_TRUNC`; SQLite trunca con `strftime` y devuelve texto, no milisegundos), así que necesita su propia ronda con tests dedicados en los dos motores.
2. **Filtrado con pushdown a SQL (`db.<c>.filter(predicate)`)**: Un predicado estructural -- mismo criterio de "nombre por forma" que ya usa `sumBy`/etc. -- que se traduzca a una cláusula `WHERE` real, en vez de obligar a traer la tabla entera con `.all()` y filtrar en memoria.
3. **Transacciones sobre múltiples escrituras `db.<c>`**: Hoy cada escritura es su propio commit implícito; falta una forma de agrupar varias escrituras relacionadas en una sola transacción con rollback ante error.

### 8.3 Runtime y Comunicaciones
1. **Limpieza proactiva de sesiones**: Implementar recolección periódica en segundo plano para sesiones expiradas bajo `--session-ttl`.
2. **Rate limiting distribuido**: adaptadores para estado compartido (e.g. Redis) en despliegues con réplicas -- la mitad "proxy de confianza" (`X-Forwarded-For`) de este ítem se resolvió como `--trust-proxy`, ver §9.5 Hecho y GRAMMAR.md §3.89.
3. **Envío asíncrono no bloqueante para `smtp`**: `send`/`sendToMany`/`sendHtml` (GRAMMAR.md §3.63) siguen siendo sincrónicos -- un relay lento hace lento al servidor entero (de un solo hilo) mientras dura esa request. Múltiples destinatarios y cuerpo HTML ya están resueltos.

### 8.4 Autenticación y Seguridad
1. **Carga opcional del `User` completo en sesión**: `auth.currentRole()`/`currentUserId()` exponen rol e id, pero cargar el struct completo sigue requiriendo `db.users.find(uid)` explícito en cada rpc que lo necesite.

### 8.5 Almacenamiento
1. **Módulo `storage`/S3`**: No existe ninguna integración de almacenamiento de archivos hoy -- ni presigned URLs, ni upload directo, nada. Bloquea cualquier caso de uso con archivos adjuntos.

**Origen de 8.4–8.5** (23/08/2026): 15 gaps nuevos, verificados contra el código real (no contra la documentación), a partir de dos fuentes externas -- un reporte de adopción real (app financiera "MyFinance" sobre una base Postgres ya existente) y una auditoría propia de los "límites honestos" que cada sección `§3.X` de GRAMMAR.md ya se admite a sí misma. Quedaron fuera de esta ronda por ser más especializados o de menor demanda general (identificados, no descartados): WebSocket bidireccional, jobs en background/cron, caché a nivel de app, búsqueda full-text, subida de archivos multipart, retry/backoff para `smtp`/`http` salientes, export OpenTelemetry, GraphQL, i18n, y migraciones más allá de agregar columna (rename/retype/drop). Conversión `.toString()`, `response.setStatus` en `stream` y el catch-all de `@route` (originalmente 8.6/8.7.1) se implementaron el 23/08/2026 -- ver v1.19.0 y GRAMMAR.md §3.55–§3.57. Argon2id configurable, señal de hash legado (originalmente 8.4.1/8.4.2) y aceptar PK autoincremental de 32/16 bits (originalmente 8.5.1) se implementaron el mismo día -- ver v1.20.0 y GRAMMAR.md §3.58–§3.59. `@route` con query string (originalmente 8.6.1) se implementó el mismo día -- ver v1.23.0 y GRAMMAR.md §3.62. **Cobertura de `escapeHtml()` (originalmente 8.6.2, hoy 8.7): auditado el 23/08/2026 y descartado como gap real** -- el método ya escapaba `'` además de `"` desde su introducción (v1.9.0), así que texto/atributo con CUALQUIER estilo de comillas ya estaba cubierto; lo único que quedaba (interpolar dentro de `<script>`/`<style>`, o un atributo sin comillas) son contextos que NINGÚN escapador de HTML resuelve por diseño -- necesitan escape de JS/CSS o directamente no escribir atributos sin comillas, no "más cobertura" de este método. Se corrigió únicamente la redacción de GRAMMAR.md §3.45, que decía "solo comillas dobles" por error. Auth externo/JWT (originalmente 8.4.2) se implementó el 24/08/2026 -- ver v1.25.0 y GRAMMAR.md §3.64. `linkc introspect` (originalmente 8.5.2) se implementó el mismo día -- ver v1.27.0 y GRAMMAR.md §3.66. Modo "adoptar tabla existente sin auto-migrar" (originalmente 8.5.1, renumerado de §8.6 a §8.5 tras esto) se implementó el mismo día -- ver v1.28.0 y GRAMMAR.md §3.67.

---

## 9. Hoja de Ruta Extendida (Fase 5 · Gaps de Adopción Real a Escala)

**Origen** (24/08/2026): dos reportes de adopción real, compartidos por el usuario -- `c-script-wishlist.md` (164 ítems, app financiera "MyFinance" sobre Postgres ya existente) y `peticiones-c-script-linkc.md` (108 ítems, migración real de 17 microservicios dentro de "IgnisLove"). 272 ítems en total, verificados uno por uno contra el código real (GRAMMAR.md §3.1–§3.67 y `README.md`) antes de sumarlos acá -- varios ya estaban resueltos y se descartaron (ver la lista al final de esta sección). El resto se depuró (mucha superposición entre los dos reportes) y se ordenó siguiendo el pedido explícito: **primero todo lo que no bloquea nada externo, al final lo ya trackeado en §8 y lo genuinamente bloqueado**. Dentro de cada subsección, el orden es aproximadamente el de urgencia que los propios reportes señalan.

### 9.1 Documentación (bajo costo, alto valor -- hacer esto primero, sin importar el tamaño del resto)

**§9.1 completo** (24/08/2026). **Matriz de comportamiento de auto-migrate** (originalmente 9.1.1) se implementó junto con un bug real que la auditoría encontró -- ver v1.29.0, GRAMMAR.md §3.17 (matriz completa SQLite+Postgres) y §3.68 (NULL en columna requerida tras migración de Postgres ya no se serializa en silencio, ahora es un error de runtime limpio). **Comportamiento ante colisión de colección** (originalmente 9.1.2) verificado contra un PostgreSQL real -- ver v1.30.0, GRAMMAR.md §3.36. **`CREATE EXTENSION "pgcrypto"`** (originalmente 9.1.3): auditado y sacado por completo del DDL generado en vez de solo documentado -- no se usaba para nada, verificado aplicando el schema con un rol Postgres real sin privilegios de superusuario -- ver v1.31.0, GRAMMAR.md §3.36. **`link.lock`** (originalmente 9.1.4) y **comportamiento de `SIGTERM`** (originalmente 9.1.9): ya estaba mayormente documentado (`link.lock`, GRAMMAR.md §2.1) o ya se comportaba bien sin necesitar cambios (`SIGTERM`: sin manejador de señales, terminación inmediata del SO, pero ninguna escritura ya confirmada puede perderse porque cada una es autocommit) -- se agregó el gotcha real que faltaba (`.link` compilado fuera de su carpeta real) y la confirmación explícita de `SIGTERM` -- ver v1.32.0, GRAMMAR.md §2.1 y §3.17. Las 4 guías restantes (SQLite vs PostgreSQL, despliegue multi-servicio, adopción incremental, integrar un servicio ya generado desde afuera) se escribieron como `docs/*.md` nuevos, cross-referenciados entre sí y con GRAMMAR.md, con sus propios hallazgos verificados contra un servidor real (forma exacta de `/health`, de un error 401/403, de `{"error": "..."}`) -- ver v1.33.0.

### 9.2 Núcleo del Lenguaje
1. **Tipo `Decimal`/`Money` de precisión exacta**: `Float` es una fuente de error de redondeo conocida y confirmada por dos adoptantes financieros distintos -- necesita su propio diseño (representación interna, mapeo en los dos backends SQL). Único ítem que queda abierto en esta subsección.

**Hecho** (24/08/2026): **narrowing/desreferencia de `T?`** (originalmente 9.2.1, el gap más repetido y con más fricción de los dos reportes) se resolvió de punta a punta, no solo el mínimo viable -- `match x { v: T => ..., null => ... }` (narrowing real, no solo `??`/`isSome`/`isNone`), reusando el mismo mecanismo de patrones que ya narrowaba uniones (§3.9). De paso: `a ?? b` (con encadenado real, `a ?? b ?? default`), `.isSome()`/`.isNone()` (con un caso adversarial de shadowing por un campo real resuelto), completion del LSP para `T?`, y el **mensaje de error** (originalmente 9.2.2) actualizado para señalar las alternativas reales en vez de solo "no se puede". Ver v1.34.0, GRAMMAR.md §3.69. **Tipo `Uuid` nativo** (originalmente 9.2.3): forma canónica validada en los tres bordes (runtime, `validators.ts`, `schemas.ts`/Zod, con la misma regex en los tres), tipo aparte de `String` sin mezcla implícita (mismo criterio que `Int64` vs `Int`), `crypto.uuid()` ahora devuelve `Uuid`. Ver v1.35.0, GRAMMAR.md §3.70. **`@deprecated("usa X en su lugar")`** (originalmente 9.2.2 en esta lista, renumerado): en un campo de struct o en un rpc/stream -- sobre un campo es la ÚNICA anotación que un campo admite hoy (`Field` no tiene el `Vec<Annotation>` genérico de `RpcDecl`), puramente informativo (no afecta subtipificación estructural ni runtime), propagado como JSDoc `@deprecated` en `contract.d.ts` y como `deprecated: true` + `description` nativo de OpenAPI/JSON Schema en `openapi.json`. Ver v1.36.0, GRAMMAR.md §3.71. **Docstrings `///` propagados a OpenAPI**: nueva infraestructura de lexer (`Token::leading_doc`, exactamente 3 slashes -- ni `//` ni `////`) que no rompe ningún programa existente (un `///` sigue siendo trivia válida en cualquier posición, la captura es puramente aditiva). Se propaga como `description` del Operation Object en `openapi.json` y como bloque JSDoc multilínea en `contract.d.ts`; si el mismo rpc también lleva `@deprecated`, las dos cosas se combinan en un solo campo/bloque en vez de pisarse. Alcance de esta ronda: solo `rpc`/`stream`, no `type`/campo de struct. Ver v1.37.0, GRAMMAR.md §3.72. **Validadores declarativos por campo** (originalmente el ítem 2 de esta lista, renumerado): `@validate(email)` / `@validate(regex, "...")` sobre un campo `String`/`String?` -- enforcement real en CUATRO lugares (servidor real vía `linkc serve`, `openapi.json` con `format`/`pattern` estándar de JSON Schema, `schemas.ts`/Zod con `.email()`/`.regex(new RegExp(...))`, y JSDoc informativo en `contract.d.ts`), no solo documentación. Única excepción de la sesión a "cero dependencias nuevas": trajo la crate `regex` (pura Rust, sin dependencias C) -- un patrón de usuario es texto arbitrario, a diferencia de las formas FIJAS (UUID/SHA-256/ISO-8601) que sí se hand-rollean en el resto del proyecto. Alcance de esta ronda: `validators.ts` (las funciones `isX()` hand-escritas) todavía no lo enforce, trabajan sobre el tipo estructural sin anotaciones. Ver v1.38.0, GRAMMAR.md §3.73. **Valores por defecto en campos de `struct`** (originalmente el ítem 2 de esta lista): `nombre: Tipo = expr`, mismo mecanismo que `Param::default` -- un campo con default se puede omitir de un literal sin volverse `Optional`; se completa en `Expr::StructLit`, evaluado DE NUEVO en cada construcción (`crypto.uuid()` como default da un valor distinto cada vez, verificado). Propagado a `contract.d.ts`/`schemas.ts` como campo opcional y a `openapi.json` (fuera de `required`, más `"default"` cuando es un literal simple). Sin soporte en un `type` genérico ni con acceso a otros campos del mismo literal -- alcance de esta ronda. Ver v1.39.0, GRAMMAR.md §3.74.

**Hecho** (24/08/2026, segunda ronda): **`dateFromParts(year, month, day, hour, minute, second) -> Timestamp`** -- gap NUEVO, encontrado por un segundo reporte de MyFinance (`myf errores.md`, backend de cálculo de Modelos tributarios 130/303/347) que no había quedado capturado en el wishlist original de 164 ítems: §3.31 documentaba a propósito que un `Timestamp` v0 solo podía llegar de un parámetro de rpc o de la base, nunca construirse arbitrariamente -- `now()` (§3.32) resolvía el instante ACTUAL, pero no una fecha arbitraria (el límite de un trimestre, por ejemplo), que es exactamente lo que Modelo 130/303 necesitaba calcular. Builtin sin receptor, mismo mecanismo que `now()`; una fecha inválida (mes 13, 30 de febrero) es `bad_request` (400), nombrando el campo mal formado. Ver v1.55.0, GRAMMAR.md §3.90.

**§9.2 completo** salvo `Decimal`/`Money` (necesita su propio diseño, ítem 1 arriba).

### 9.3 Base de Datos y Consultas
1. **`db.<c>.count(predicate)` sin traer filas**: hoy `findWhere(...).length()` trae la tabla entera a memoria solo para contar. Bloqueado en la práctica por el mismo trabajo que el ítem 2 (predicado -> SQL) -- un `count(predicate)` que siga evaluando en el intérprete no cumpliría el "sin traer filas" que pide el nombre.
2. **`findWhere`/`deleteWhere` empujados a SQL de verdad**: hoy se evalúan en el intérprete (confirmado en `db.rs`: a diferencia de `sumBy`/etc., no bajan a una cláusula `WHERE`) -- fusionar con el pushdown de `db.<c>.filter(predicate)` ya trackeado en §8.2.2, misma máquina de "predicado estructural a SQL". Ítem grande (un mini compilador de predicado a SQL, parametrizado de forma segura contra los dos backends) -- no se atacó esta ronda, queda para una ronda dedicada.
3. **Índices/constraints ÚNICOS COMPUESTOS (de varios campos) declarativos**: `@index`/`@unique` de un solo campo ya está resuelto (ver Hecho, v1.45.0) -- la forma equivalente sobre VARIOS campos a la vez (`@unique(["email", "tenantId"])`, por ejemplo) sigue pendiente, necesitaría una anotación a nivel de `type`, no de campo (`TypeDecl` no tiene `annotations` hoy).
4. **Constraints `@check` declarativos** en el `.link`.
5. **Detección de colisión de nombre de tabla**: que `linkc build`/`linkc serve --db postgres://...` avise (o aborte con `--strict`) si una colección mapea a una tabla que YA EXISTE en la base de destino y no fue creada por ese mismo `.link` -- encontrado en producción real (`telemetry.link` habría chocado con una tabla `events` real de un pipeline de analítica).
6. **`--db-schema <nombre>` o `--db-prefix`**: namespacing para compartir una base Postgres entre varios `.link` sin pensar en colisiones de nombre.
7. **`linkc migrate --dry-run`**: mostrar el DDL exacto que se ejecutaría sin aplicarlo, más comportamiento configurable ante una migración que perdería datos (¿aborta por defecto? ¿hace falta `--allow-destructive`?).
8. **`@cache("60s")` declarativo** sobre un rpc, para lecturas costosas y poco cambiantes.
9. **Idempotency keys nativas** en rpcs de escritura: hoy hay que implementar la comprobación de "¿ya existe?" a mano antes de cada inserción en un backfill con reintentos.

**Hecho** (24/08/2026): **`db.<c>.upsert(matchFn, insertValue, updateFn)`**: ver v1.40.0, GRAMMAR.md §3.75. **`db.<c>.insertMany(items)`**: cada elemento pasa por el mismo `insert` real de siempre (una sentencia SQL autocommit por fila) -- lo que ahorra es la ida y vuelta HTTP N veces desde el cliente, no el costo de N inserts contra la base. Sin transacción envolvente (mismo criterio "autocommit por sentencia" del resto del lenguaje): si un ítem falla a mitad de la lista, los anteriores quedan insertados. Ver v1.41.0, GRAMMAR.md §3.76. **`createdAt`/`updatedAt` automáticos**: resuelto SIN ninguna anotación mágica por nombre de campo -- es la composición de dos primitivas ya existentes (`now()` builtin + default de campo `= now()`, §3.74) para "asignado una sola vez al crear", más una anotación chica y nueva, `@autoUpdate` (solo sobre `Timestamp`), para la única parte que de verdad faltaba: "pisar a `now()` en CADA `applyPatch`/`upsert`-actualización, incluso si el patch no lo menciona". Ver v1.42.0, GRAMMAR.md §3.77. **Soft-delete nativo**: un campo `@softDelete` sobre `Timestamp?` -- `delete(id)` deja de ser un `DELETE` SQL, pasa a un `UPDATE` idempotente que fija el campo a `now()`; toda lectura que devuelve lista o conteo (`all`/`page`/`pageAfter`/`count`/`sumBy`-etc., más `findWhere`/`deleteWhere` que las reusan por dentro) filtra automáticamente. Límite deliberado: `find(id)` NO filtra -- una fila borrada sigue siendo encontrable por id directo (mismo criterio que Django/Rails para el mismo problema), necesario además para que la re-consulta interna de `insert`/`applyPatch` no explote si un patch toca justo ese campo. Ver v1.43.0, GRAMMAR.md §3.78. **`linkc build --diff <archivo-anterior>`** (originalmente el ítem 8 de esta lista): reusa el mismo diff LCS que `linkc test` ya tenía para mostrar por qué un snapshot cambió -- ahora también se puede pedir desde `linkc build`, comparando el `contract.d.ts` recién generado contra un archivo guardado aparte (típicamente `git show <rev>:ruta > archivo`). Puramente informativo, nunca hace fallar el build. Ver v1.44.0, GRAMMAR.md §3.79. **Índices declarativos de un solo campo: `@index`/`@unique`** (originalmente parte del ítem 3 de esta lista, la parte de VARIOS campos a la vez sigue abierta -- ver arriba): dos anotaciones de campo sin paréntesis, a lo sumo una por campo (rechazado en el parser, mismo criterio que `@autoUpdate`/`@softDelete`) -- ninguna exige un tipo de campo particular, a diferencia de esas dos. El índice se crea de verdad al arrancar en LOS DOS backends (`CREATE [UNIQUE] INDEX IF NOT EXISTS`, idempotente, nombre determinístico `idx_<tabla>_<campo>`), y `linkc build` emite la misma sentencia en el DDL estático de Postgres. Una violación de `@unique` en `insert`/`applyPatch` se traduce a 400 (detectando el mensaje específico que cada motor devuelve para esta violación puntual), no a un 500 genérico. `--adopt-existing` nunca ejecuta este DDL, mismo criterio que el resto del schema. Ver v1.45.0, GRAMMAR.md §3.80. **`Timestamp` decodifica `date`/`timestamp`/`timestamptz` nativos de Postgres** -- gap NUEVO, la otra mitad del mismo segundo reporte de MyFinance que motivó `dateFromParts` (§9.2): una tabla YA EXISTENTE adoptada (el caso normal, no una tabla que `linkc build` creó) casi siempre tiene sus columnas de fecha en el tipo NATIVO de Postgres, no en el `BIGINT` propio de c-script -- auditando `runtime/store.rs` apareció que esto estaba roto en los DOS sentidos (declarado `String`: falla por wire binario no-UTF8; declarado `Timestamp`: fallaba TAMBIÉN, el OID nativo no matchea ninguno de los anchos de entero que se probaban). `ColumnKind::Timestamp` nuevo, decodificado a mano contra el wire binario de Postgres (sin sumar `chrono`) con el mismo espíritu que el algoritmo de calendario de Hinnant ya existente. `linkc introspect` (§3.66) ahora recomienda `Timestamp` sin advertencia para estas columnas -- antes recomendaba `String`, una recomendación que en los hechos también estaba rota. Alcance: solo LECTURA -- escribir contra una columna nativa adoptada sigue sin funcionar, no era parte del caso reportado. Ver v1.55.0, GRAMMAR.md §3.91.

### 9.4 HTTP y Diseño de API
1. **Hooks de middleware de usuario**: lógica "antes"/"después" de cualquier rpc, para logging o métricas transversales -- cambio real en el modelo de ejecución.
2. **Webhooks salientes declarativos**: registrar una URL de terceros y que el runtime reintente/firme automáticamente -- simétrico a la verificación de webhooks entrantes que ya existe (`crypto.hmacSha256`).
3. **Compresión gzip/brotli** de la respuesta, opcional vía flag.
4. **CORS configurable por ruta**, no solo global (`--cors-origin`).
5. **HSTS configurable** desde `linkc serve` para cuando el propio proceso termina TLS (hoy asume que siempre hay un proxy delante).

**Hecho** (24/08/2026): **Timeout configurable en `http.*`** (originalmente el ítem 1 de esta lista): auditando `runtime/mod.rs` apareció que `http.get`/`post`/`getWithHeaders`/`getWithStatus`/`postWithStatus`/`postWithHeaders` no fijaban NINGÚN timeout de lectura/escritura propio -- `ureq` (la crate) trae 30s de timeout de CONEXIÓN por default, pero el de lectura/escritura es "nunca" por default, documentado así por la propia crate. Para un intérprete de un solo hilo, eso significaba que una request saliente a un servidor lento o colgado bloqueaba el proceso ENTERO para siempre, ni siquiera `/health` respondía mientras tanto -- un bug de disponibilidad real, no solo un gap de documentación. `--http-timeout`/`LINK_HTTP_TIMEOUT` (mismo formato `Ns`/`Nm`/`Nh`/`Nd` que `--session-ttl`, default 30s -- el mismo número que `ureq` ya usaba para conexión) fija un timeout total por llamada, guardado en `Db` con el mismo mecanismo que `argon2_params`. **Reintentos** (la otra mitad de este ítem) sigue sin resolver -- un timeout u otro error de red falla la llamada, reintentar sigue siendo responsabilidad del código de usuario. Ver v1.51.0, GRAMMAR.md §3.86.

### 9.5 Autenticación y Seguridad
1. **Bloqueo de cuenta configurable** tras N intentos fallidos.
2. **Log de auditoría de autorización estructurado**: quién llamó a qué rpc, con qué rol, y si se permitió o denegó.
3. **API keys de servicio**: para llamadas servidor-a-servidor, distintas de las sesiones de usuario -- confirmado como gap real (IgnisLove usa `fetch` sin autenticación entre su app Node y cada `linkc serve`, confiando solo en que el puerto no sea alcanzable).
4. **Escaneo de secretos en tiempo de compilación**: que `linkc build`/`lint` avise si detecta una URL de conexión o API key literal en el código.
5. **Lint sobre "autorización de fachada" -- reformulado tras auditarlo (24/08/2026).** La forma original ("`@requires(Role.X)` que nunca llama a `auth.currentRole()`/`currentUserId()`") resultó ser un lint de mala señal: el caso MÁS COMÚN y CORRECTO de `@requires(Role.Admin)` (gating de un solo rol, sin necesitar diferenciar comportamiento adentro) nunca llama a ninguno de los dos -- implementarlo tal cual habría generado ruido constante sobre código perfectamente bien escrito, no una señal real. Queda abierto, pero repensado: la version con mejor señal sería la INVERSA -- código que llama a `auth.currentRole()`/`currentUserId()` para hacer su PROPIA verificación manual de rol adentro del cuerpo, sin ningún `@requires`/`@authenticated` en el rpc que lo contiene (el chequeo real no está en la anotación central, así que un bug en la lógica manual bypasea todo en silencio). Sin atacar esta ronda -- necesita su propio diseño.
6. **Cifrado de campo a nivel de columna** (`@encrypted` en un `String` sensible).
7. **RBAC por recurso**: permisos más allá de todo-o-nada por rol.
8. **ABAC**: reglas basadas en atributos del propio recurso (ej. "solo el dueño de la factura").

**Hecho** (24/08/2026): **Revocar todas las sesiones de un usuario** (originalmente el ítem 2 de esta lista): hasta esta ronda solo existía `destroySession()`, que opera sobre la sesión que ya autenticó la request ACTUAL (deliberadamente sin tomar un token como argumento, para que nadie pueda revocar la sesión de otro adivinando su token) -- no había forma de cerrar TODAS las sesiones de un usuario dado a la vez (útil tras un cambio de contraseña, o para un admin que banea a alguien). `auth.destroyAllSessions(userId: Int) -> Int` es el nuevo builtin -- a diferencia de `destroySession`, SÍ toma un identificador (mismo criterio que `createSessionWithId`: un `user_id` es una clave de aplicación, no un secreto adivinable como un token). Devuelve cuántas sesiones se borraron. Quién puede LLAMARLO es responsabilidad de quien escribe el `.link` (típicamente `@requires(Role.Admin)`) -- el método en sí no impone ninguna política. Ver v1.49.0, GRAMMAR.md §3.84. **Lint: comparación `==` sobre un campo `secret`/`token`/`password`** (originalmente el ítem 6 de esta lista): `==`/`!=` donde cualquiera de los dos lados es un `Ident`/campo cuyo nombre sugiere un secreto (substring laxo, sin distinguir mayúsculas) recomienda `crypto.timingSafeEqual` (§3.54) -- comparar contra `null` (chequeo de presencia, no de valor) queda afuera a propósito. Recorre todo el cuerpo, cualquier nivel de anidamiento. Ver v1.53.0, GRAMMAR.md §3.88. **`@rate_limit` con `X-Forwarded-For` de confianza** (originalmente el ítem 1 de esta lista, también la mitad "proxy de confianza" de §8.3.2): `remote_addr()` (la conexión TCP real) sigue siendo el default -- detrás de un proxy/balanceador de verdad (confirmado como bloqueo real, todo corre detrás de nginx en la adopción de IgnisLove) eso es siempre la IP del proxy, compartiendo el límite entre TODOS los usuarios reales a la vez. `--trust-proxy`/`LINK_TRUST_PROXY` (apagado por default, mismo criterio que `--adopt-existing`) usa el PRIMER valor de `X-Forwarded-For` en su lugar -- opt-in explícito: prenderlo sin un proxy de confianza real delante deja evadir el límite por completo. v0 sin validar cuántos proxies hay en el medio ni de qué IP vienen (sin CIDR/N-hops configurable). Ver v1.54.0, GRAMMAR.md §3.89.

### 9.6 Almacenamiento y Comunicaciones
1. **`smtp` con adjuntos y cc/bcc**: `sendToMany`/`sendHtml` ya resueltos (GRAMMAR.md §3.63); adjuntos y cc/bcc siguen sin cubrir. El módulo `storage`/S3 (§8.5) y el envío asíncrono de `smtp` (§8.3.3) ya están trackeados -- este ítem es específicamente lo que falta de `smtp.send` en sí.

### 9.7 CLI y Experiencia de Desarrollador
1. **`linkc doctor`**: diagnóstico de entorno (versión, PATH, permisos, conectividad a la DB configurada) antes de un despliegue.
2. **Unix domain sockets** (`--socket /run/app.sock`) como alternativa a TCP.
3. **Suite de administración de datos**: `linkc db inspect`/`db shell`/`export`/`import`/`seed` -- listar tablas y conteo de filas, REPL de solo lectura, exportar/importar entre entornos o motores, poblar una base nueva desde un fichero.
4. **RPCs de administración estándar opcionales** (`_admin.vacuum()`, `_admin.tableStats()`) detrás de `@requires(Role.Admin)`.
5. **`linkc systemd <archivo> <puerto>`**: generador de unidad systemd, a la par de `linkc docker` que ya existe.
6. **`linkc pm2-config <archivo> <puerto> -o ecosystem.json`**: generador de configuración PM2.

**Hecho** (24/08/2026): **`--host <dirección>`/`LINK_HOST`** (originalmente el ítem 4 de esta lista): `linkc serve` escuchaba SIEMPRE en `0.0.0.0` (todas las interfaces), sin ninguna alternativa -- gap de seguridad real, no solo de conveniencia, para un proceso que solo necesita aceptar conexiones locales. Mismo orden de precedencia que el resto de los flags de `serve` (`--host` primero, después `LINK_HOST`, después el default `"0.0.0.0"` de siempre). El valor se pasa tal cual a `tiny_http::Server::http`, sin resolución propia -- una dirección que no le pertenece a ninguna interfaz local hace fallar el bind al arrancar, nombrando la dirección exacta, nunca cae en silencio a `0.0.0.0`. Ver v1.46.0, GRAMMAR.md §3.81. **`linkc test <archivo> --filter <nombre>`** (originalmente el ítem 1 de esta lista): substring sobre el nombre del test, sensible a mayúsculas -- mismo criterio que `cargo test <substring>`. Solo aplica al test runner integrado (`test "..." { ... }`), nunca al testing de contrato (`linkc test archivo.link archivo.snap`) -- combinar los dos es un error de uso claro, no un `--filter` ignorado en silencio. Un filtro que no matchea ningún nombre corre cero tests sin fallar. Ver v1.47.0, GRAMMAR.md §3.82. **`linkc --version` por archivo generado** (originalmente el ítem 1 de esta lista): `linkc` no tenía NINGUNA forma de reportar su propia versión -- ni `--version`/`-v`/`version` estaban despachados, ni ningún archivo generado decía con qué versión se había generado. `linkc::VERSION` (`env!("CARGO_PKG_VERSION")`, tomada de `Cargo.toml` en tiempo de compilación) alimenta LAS DOS cosas a la vez, así que nunca pueden desincronizarse entre sí: `linkc --version` la imprime, y el header de `contract.d.ts`/`client.ts`/`hooks.ts`/`validators.ts`/`schemas.ts` queda estampado con ella. `openapi.json` (que no admite comentarios) lleva la misma info en `x-generated-by`, una extensión de vendor estándar -- deliberadamente NO en `info.version`, que es la versión del API documentada, un concepto distinto. Puramente informativo: nada compara la versión estampada en un `gen/` viejo contra el binario que lo sirve o reconstruye. Ver v1.48.0, GRAMMAR.md §3.83. **Límite configurable de tamaño máximo de body** (originalmente el ítem 3 de esta lista): `linkc serve` leía el body de CUALQUIER request entero a memoria sin ningún límite -- un vector real de agotamiento de memoria. `--max-body-bytes`/`LINK_MAX_BODY_BYTES` (default 10 MiB) corta la lectura con `Read::take(max_body_bytes + 1)` y responde `413 Payload Too Large` ANTES de que auth/rate-limit/parseo del JSON compitan por memoria con un body ya sabido demasiado grande -- no lo lee completo primero. Límite de proceso, no por rpc; no se drena el resto de un body rechazado (si el cliente reusa la conexión, el próximo intento da 400 y cierra, nunca un colgado ni una fuga). Ver v1.50.0, GRAMMAR.md §3.85.

**Hecho** (24/08/2026): **Modo "workspace" (`linkc serve-all`) + `--restart-backoff`** (originalmente los ítems 5 y 6 de esta lista, priorizados de nuevo por un tercer reporte del usuario citando el incidente puntual -- 68 reinicios de `telemetry` en un arranque en frío -- y pidiendo resolverlo antes de seguir con el resto del backlog): `linkc serve-all <directorio> --port-base N` descubre cada `.link` de un directorio, los compila TODOS antes de arrancar cualquiera, y levanta uno por hilo del sistema operativo dentro de un ÚNICO proceso -- puerto `N`+posición alfabética, SQLite propio por servicio preservado (`--db`/`LINK_DATABASE_URL` compartido rechazado de entrada, mismo motivo que §9.3.4 más abajo). Auditando `runtime::server::serve` para esto apareció que un fallo de conexión a Postgres usaba `std::process::exit(1)` -- dentro de UN proceso por servicio (como hoy) eso solo mataba a ese servicio, pero bajo `serve-all` se habría llevado puesto TODO el workspace por un solo servicio caído; `serve` ahora devuelve `Result<(), String>` en vez de terminar el proceso (`linkc serve` preserva el comportamiento externo de siempre). `--restart-backoff <duración>`/`LINK_RESTART_BACKOFF` (funciona en `linkc serve` y en `linkc serve-all`) agrega backoff exponencial NATIVO ante ese mismo fallo -- reemplaza la mitigación externa (`pm2 --restart-delay`, una espera fija) con una que dobla en cada fallo consecutivo (techo 30s, reseteada tras 60s estable). Reproducido el incidente real contra el binario (un puerto ocupado transitoriamente, backoff 1s/2s/4s, el otro servicio sano todo el tiempo, recuperación automática al liberarse el puerto) -- ver v1.56.0, GRAMMAR.md §3.92.

### 9.8 Observabilidad
1. **Logging estructurado en JSON** (`--log-format json`) + **nivel de log configurable** (`--log-level warn|info|debug`) -- hoy cada request exitosa deja una línea, ruidoso en producción con tráfico real.
2. **Métricas Prometheus nativas en `/metrics`**: latencia por rpc, conexiones activas, tamaño de la base -- confirmado, no existe hoy ningún `/metrics`.
3. **Métrica de clientes conectados a un `stream`**: para depurar streaming sin instrumentación externa.
4. **Métrica de latencia de propagación NOTIFY + cola de reintento acotada**: hoy es best-effort puro y un evento de más de 8000 bytes no llega a otras instancias sin ningún aviso visible.

**Hecho** (24/08/2026): **Health check real** (originalmente el ítem 3 de esta lista): `/health` (`/`/`/status`, mismo handler) devolvía `200` FIJO sin tocar la base para nada -- inútil para cualquier orquestador que lo usa para decidir si reiniciar el proceso. `Db::health_check()` ejecuta un `SELECT 1` real en CADA request a `/health`, sin caché -- `200`/`"status":"ok"` si respondió, `503`/`"status":"error"` si no, con el mensaje real en `"database"`. Del lado Postgres pasa por el MISMO `with_reconnect` (§3.40) que cualquier otra query -- una caída transitoria se autorepara ahí mismo. Alcance de esta ronda: solo la base, no "servicios externos declarados" -- c-script no tiene hoy ningún concepto declarativo de dependencias externas, así que esa mitad del ítem original queda pendiente de esa pieza previa. Ver v1.52.0, GRAMMAR.md §3.87.

### 9.9 Diferido -- ya trackeado en §8, prioridad menor que 9.1–9.8
Por pedido explícito, lo que ya estaba en la hoja de ruta antes de estos dos reportes pasa a continuación de todo lo de arriba, no porque sea menos importante en términos absolutos, sino porque los dos reportes de adopción real no lo señalan como más urgente que lo nuevo: **§8.1.2** (playground multi-archivo), **§8.2.1** (agregación con truncamiento de fecha -- reforzado por peticiones#49), **§8.2.2** (`db.<c>.filter` pushdown -- ahora incluye también `findWhere`/`deleteWhere`, ver §9.3.4), **§8.2.3** (transacciones), **§8.3.1** (limpieza proactiva de sesiones), **§8.3.2** (rate limiting distribuido -- la mitad de estado compartido/Redis; la mitad de `X-Forwarded-For` se adelantó a §9.5.1 por ser más simple y más citada), **§8.3.3** (smtp asíncrono), **§8.4.1** (carga completa de `User` en sesión), **§8.5.1** (módulo `storage`/S3).

### 9.10 Bloqueado -- requiere algo externo o una decisión previa
No autónomo por diseño, no por pereza -- cada ítem nombra qué falta:
1. **Publicación en npm** (`link-lang`): credenciales de la cuenta npm del usuario. Ya trackeado en §8.1.1.
2. **Publicación en VS Code Marketplace / JetBrains / Zed**: cuentas de publisher del usuario en cada marketplace.
3. **Imagen Docker oficial publicada**: credenciales de un registro (Docker Hub o similar) del usuario.
4. **Paquetes `apt`/`brew`**: cuentas y claves de firma de cada gestor de paquetes.
5. **GitHub Action oficial publicada** (`setup-linkc@v1`) en el Marketplace: requiere publicarla bajo la organización/cuenta de GitHub del usuario.
6. **Integraciones con GCS, Azure Blob, Stripe, Twilio, SQS/RabbitMQ**: el código se puede escribir, pero verificarlo de punta a punta (mismo estándar que el resto de este proyecto: contra el servicio real, no un mock) necesita que el usuario provea credenciales de prueba de cada proveedor.
7. **OAuth2/OIDC nativo (login social)**: el protocolo se puede implementar, pero verificarlo de verdad necesita un proveedor de identidad real (Google/GitHub/etc.) con una app de prueba registrada por el usuario.
8. **Backend MySQL/MariaDB**: un backend nuevo a la par de SQLite/Postgres, toca `storage.rs`/`db.rs`/codegen de punta a punta -- necesita decisión explícita de alcance antes de empezar, no es una ronda autocontenida.
9. **Intérprete multi-hilo**: cambio de arquitectura fundamental -- single-threaded es una decisión de diseño deliberada y documentada en todo GRAMMAR.md. Necesita decisión explícita, no solo una bandera.
10. **Sharding, réplicas de solo lectura, base separada por tenant**: subsistemas distribuidos genuinamente grandes -- necesitan diseño aprobado antes de implementar.
11. **Compilación AOT a binario nativo** independiente del intérprete: proyecto grande de backend de codegen, más allá del WASM actual limitado a escalares -- necesita decisión de alcance.
12. **GraphQL como transporte alternativo**: compite arquitectónicamente con el diseño RPC-first del proyecto -- necesita decisión de si encaja con la tesis del proyecto en absoluto, no solo una implementación.
13. **Auto-scaling hints, rolling updates, rollback automático tras health check fallido**: presuponen una plataforma de orquestación (k8s/ECS) que el proyecto no apunta hoy -- o es una decisión de alcance para que `linkc serve` la conozca, o es puramente un tema de guía de despliegue (ver §9.1).
14. **Canal de soporte (Discord/Slack), programa de "adoptantes de referencia", benchmark comparativo publicado contra terceros**: acciones organizacionales/de comunidad, no trabajo de código -- las tiene que decidir y ejecutar el usuario, no algo que se implemente en el compilador.

**Ítems de los dos reportes ya resueltos antes de esta ronda** (no reimplementar, verificado contra el código real): adoptar tabla existente sin auto-migrar (§3.67, v1.28.0), tipo de PK flexible en Postgres -- SERIAL/IDENTITY 16/32 bits (§3.59, v1.20.0), `linkc introspect` (§3.66, v1.27.0), puente de sesión JWT externo (§3.64, v1.25.0), status/headers en `http.get`/`post` salientes (§3.60, v1.21.0), `smtp` a varios destinatarios y HTML (§3.63, v1.24.0), `Int64` como campo agregable/agrupable (§3.65, v1.26.0), paginación empujada a SQL + cursor (§3.48/§3.61), `@route` con múltiples parámetros/catch-all/query string (§3.37/§3.42/§3.57/§3.62), `response.setStatus`/`@content_type`/`escapeHtml` (§3.45/§3.46/§3.35), Argon2id configurable + detección de hash legado (§3.58), `crypto.randomInt`/`timingSafeEqual` (§3.54), CORS allowlist + cabeceras de seguridad fijas (§3.41), PostgreSQL runtime + TLS + auto-reconexión + LISTEN/NOTIFY cross-instancia (§3.36/§3.40/§3.44), `--session-ttl` (§3.50), `auth.currentRole`/`createSessionWithId`/`currentUserId` (§3.51/§3.53).

---

### Sobre el nombre

Decidido: **c-script**, en minúsculas. La extensión de archivo (`.link`) y el nombre del binario del compilador (`linkc`) se mantienen como está — no hace falta que deletreen la marca, igual que `.rs` no dice "rust". Ver [GRAMMAR.md](GRAMMAR.md) para el resto de las convenciones de nomenclatura.

