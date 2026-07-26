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
| `Int64`, `BigInt` | `bigint` \| `string`* | string (para no perder precisión) | **no** — nunca se implementó; `Int` es i64 y se emite como `number` |
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
| `Timestamp` | `string` (ISO-8601) \| branded | string | **no** — nunca se implementó |
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

**`LSP completo` de esta misma fila: los 3 prerrequisitos (spans+diagnósticos, recuperación de errores del parser, spans en AST/checker) ya están -- ver README.md/README.es.md, sección Estado (esto es infraestructura del compilador, no semántica del lenguaje, así que no tiene su propia sección en GRAMMAR.md). Lo único que falta es el protocolo del LSP en sí (JSON-RPC sobre stdio, `textDocument/didOpen`, `publishDiagnostics`, autocompletado, hover, ir-a-definición) -- todavía sin empezar ni investigar.** `package manager` en red y `observabilidad` de fondo de esta misma fila siguen sin empezar -- ver GRAMMAR.md §2.1 y el README para el detalle de qué falta y por qué se dejó afuera de esta ronda.

**Hitos "go / no-go":**
- Fin de Fase 0: ¿la demo E2E convence a 5 devs externos? Si no, replantear.
- Fin de Fase 1: ¿alguien construye algo real sin abandonar? Si no, seguir como framework, no como lenguaje.

**Estimación de esfuerzo total:**
- MVP: ~0.5–1 persona-año.
- Hasta 1.0 usable-pero-nicho: ~4–8 persona-año.
- Competir de verdad con Go/Rust: **20+ persona-año** (y sobre todo, comunidad).

---

## 5. Ecosistema y Herramientas

- **Package manager** (`linkc add`): resolución de dependencias, lockfile. *Aprende de Cargo; no reinventes npm.*
- **CLI**: `linkc new`, `linkc dev` (hot reload + regenera contrato), `linkc build`, `linkc deploy`, `linkc gen` (solo contrato).
- **LSP**: autocompletado, diagnósticos, go-to-def. **Imprescindible desde Alpha** — sin buen editor, no hay adopción.
- **Testing**: runner integrado + tests de contrato (que el `.d.ts` generado no rompa sin querer).
- **Debugging / observabilidad**: source maps, OpenTelemetry, logs estructurados desde Beta.
- **Integraciones (Beta)**: Postgres (queries tipadas), auth (JWT/sesiones), colas, cache. Al principio, **interop nativa con crates de Rust / paquetes de Go** para no construir todo el ecosistema desde cero.

---

## 6. Estrategia de Adopción y Comunidad

- **Cuña inicial**: equipos full-stack TypeScript que ya usan tRPC pero necesitan más rendimiento en el backend. No compitas con Go/Rust de frente; compite en *"la mejor experiencia backend para un frontend TS"*.
- **"Time to wow" < 5 minutos**: `link new` → editas un tipo → el frontend deja de compilar. Ese momento vende el proyecto.
- **Docs y templates**: starter Next.js + c-script, ejemplos reales, migración desde tRPC.
- **Open source con gobernanza clara** (licencia permisiva, RFCs públicos). La comunidad es el activo, no el compilador.

---

## 7. Riesgos, Mitigaciones y Costes

| Riesgo | Prob. | Impacto | Mitigación |
|---|---|---|---|
| Adopción nula | Alta | Crítico | Empezar como framework/codegen sobre Rust; ganar usuarios **antes** del lenguaje |
| Coste/tiempo subestimado 3–5× | Alta | Alto | MVP acotado; reutilizar crates existentes (`wasm-encoder`/`serde`) en vez de escribir un codegen o un parser JSON propios; no reinventar |
| Type system no mapea 1:1 a TS | Media | Alto | Diseñar el sistema de tipos **partiendo de TS**; validadores generados; suite de tests de isomorfismo |
| Ecosistema ausente (DB, auth) | Alta | Alto | Interop nativa con Rust/Go al inicio |
| Mantenimiento a largo plazo | Media | Alto | Open source + gobernanza; foco en un nicho |
| Debugging/observabilidad pobre | Media | Medio | Source maps y OpenTelemetry desde Beta |
| Un competidor TS-first cierra el hueco | Media | Alto | Moverse rápido en la cuña; el diferencial es *perf + E2E sin IDL* |

---

## 8. Recomendaciones Finales

### 8.1 ¿Lenguaje completo o herramienta primero?

**Herramienta primero.** Construye el **puente de tipos** como framework + codegen sobre Rust (o Go). Razones:
- Entregas el diferencial (E2E type safety con perf de sistemas) en semanas, no años.
- Heredas un ecosistema completo (DB, auth, crates) gratis.
- Validas demanda con riesgo mínimo.
- Si topas con límites que *solo* un lenguaje resuelve, entonces —y solo entonces— justifica el lenguaje, con usuarios ya en la mano.

### 8.2 Próximos 30 días (ruta pragmática)

1. **Semana 1** — Define el sistema de tipos y su mapeo exacto a TS (tabla §2.3). Este documento es el contrato del contrato.
2. **Semana 2** — PoC del **emisor**: dado un modelo de tipos (structs Rust con `serde` + `ts-rs`/`specta`), genera `.d.ts` + un `client.ts` tipado que hable con un servidor RPC mínimo (axum).
3. **Semana 3** — Cierra el loop E2E: un frontend Next.js consume el cliente; cambia un tipo en el backend → `tsc` del frontend falla. **Ese es el momento "wow".**
4. **Semana 4** — Enséñaselo a 5 devs. Decide con datos si escalar a framework o a lenguaje.

### 8.3 La primera decisión de diseño que debes tomar tú

Antes de escribir el emisor, hay que decidir **cómo se representa la ausencia** (`T?`): `T | null`, `field?: T`, o ambos. Es la decisión que más condiciona serialización, validadores y DX. Está desarrollada en §2.3 y es el primer punto donde tu criterio manda.

---

### Sobre el nombre

Decidido: **c-script**, en minúsculas. La extensión de archivo (`.link`) y el nombre del binario del compilador (`linkc`) se mantienen como está — no hace falta que deletreen la marca, igual que `.rs` no dice "rust". Ver GRAMMAR.md para el resto de las convenciones de nomenclatura.
