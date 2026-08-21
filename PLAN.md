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

**Evolución Post-1.0 (v1.1.0 a v1.17.0) — Capacidades Enterprise y Cierre de Gaps Reales:**
- **PostgreSQL en Runtime** (v1.1.0/v1.4.0/v1.8.0, GRAMMAR.md §3.36/§3.40/§3.44): Soporte de base de datos PostgreSQL real (`--db postgres://...`), auto-migraciones de esquema no destructivas, TLS oportunista y obligatorio vía `rustls` puro (compatible con Supabase/Neon/RDS), auto-reconexión transparente tras corte y LISTEN/NOTIFY en hilo dedicado para sincronizar `stream` SSE entre múltiples instancias.
- **Criptografía y Seguridad** (v1.1.0/v1.3.0/v1.5.0, GRAMMAR.md §3.34/§3.38/§3.41): Argon2id (RFC 9106) con sal aleatoria en formato PHC y verificación en tiempo constante; tokens de sesión y UUIDs alimentados por el CSPRNG del sistema operativo (`getrandom`); cálculo de HMAC-SHA256 para verificación de webhooks; CORS con allowlist configurable (`--cors-origin`) y cabeceras de seguridad estrictas fijas (`nosniff`, `DENY`, `no-referrer`).
- **Extensibilidad Web y SEO** (v1.1.0/v1.2.0/v1.6.0/v1.9.0/v1.10.0, GRAMMAR.md §3.35/§3.37/§3.42/§3.45/§3.46): Decorador `@content_type("...")` para respuestas no-JSON (HTML, XML, CSV); URLs amigables `@route("/...")` con múltiples parámetros dinámicos y precedencia determinística; sanitización explícita `String.escapeHtml()`; y selección de status HTTP en éxito `response.setStatus(code)` (e.g. páginas 404 personalizadas o 201 Created).
- **Integraciones y Operaciones** (v1.3.0/v1.7.0/v1.11.0, GRAMMAR.md §3.38/§3.43/§3.47): Lectura de entorno `env.get()`, inspección de cuerpo crudo y cabeceras entrantes `request.rawBody()` / `request.header()`; límite de peticiones `@rate_limit("N/ventana")` con token bucket continuo; envío de correos vía relay `smtp.send(to, subject, body)` con TLS; y peticiones HTTP salientes con cabeceras `http.getWithHeaders` / `http.postWithHeaders`.
- **Motor de Consultas y Autorización Avanzada** (v1.12.0 a v1.17.0, GRAMMAR.md §3.48–§3.53): Paginación empujada a SQL nativo `db.<c>.page(limit, offset)`; agregación analítica nativa con `GROUP BY` en base de datos (`sumBy`, `countBy`, `avgBy`, `maxBy`, `minBy`) preservando tipos reales; autorización con OR de roles `@requires(Role.Admin | Role.Agent)`; expiración temporal de sesiones `--session-ttl` (o `LINK_SESSION_TTL`); e introspección de sesión `auth.currentRole() -> String?`, emisión con identidad `auth.createSessionWithId(role, userId)` y lectura de identificador `auth.currentUserId() -> Int?`.

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
- **Transparencia y Calidad**: Documentación probada por el propio compilador (`compiler/tests/docs_examples.rs`), 573 tests en CI continuo y límites documentados sin promesas falsas.

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

Con las Fases 0 a 3 completadas y el núcleo v1.17.0 plenamente operativo, las siguientes prioridades definen la evolución hacia la versión 2.0:

### 8.1 Ecosistema y Distribución
1. **Publicación en el registro npm**: Empaquetar y publicar `link-lang` en npm para permitir ejecución vía `npx linkc` o instalación global estándar.
2. **Ampliación del Playground Web**: Soporte para resolución de múltiples archivos y ejecución simulada de tests en el navegador mediante `wasm32-unknown-unknown`.

### 8.2 Base de Datos y Consultas Avanzadas
1. **Agregación con truncamiento temporal**: Soporte para agrupar por fechas truncadas (`date_trunc` para cohortes diarias/mensuales) en `sumBy`/`countBy`/etc.
2. **Soporte de `Int64` en agregaciones**: Permitir campos `Int64` en las cláusulas de agrupación y suma/promedio.
3. **Paginación por cursor**: Introducción de tokens de cursor opacos y determinísticos (`Cursor<T>`) para tablas de gran volumen.

### 8.3 Runtime y Comunicaciones
1. **Inspección de respuestas en `http.get`/`http.post`**: Exponer cabeceras de respuesta y códigos de estado HTTP numéricos para permitir lógica de reintentos selectiva (e.g. en 429 Too Many Requests).
2. **Limpieza proactiva de sesiones**: Implementar recolección periódica en segundo plano para sesiones expiradas bajo `--session-ttl`.
3. **Rate limiting distribuido**: Permitir configuración de proxies de confianza (`X-Forwarded-For`) y adaptadores para estado compartido (e.g. Redis) en despliegues con réplicas.
4. **Mejoras en `smtp.send`**: Soporte para envío asíncrono no bloqueante, múltiples destinatarios y cuerpos HTML.

---

### Sobre el nombre

Decidido: **c-script**, en minúsculas. La extensión de archivo (`.link`) y el nombre del binario del compilador (`linkc`) se mantienen como está — no hace falta que deletreen la marca, igual que `.rs` no dice "rust". Ver [GRAMMAR.md](GRAMMAR.md) para el resto de las convenciones de nomenclatura.

