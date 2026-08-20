# Changelog

Todos los cambios notables en este proyecto serán documentados en este archivo.
El formato está basado en [Keep a Changelog](https://keepachangelog.com/es-ES/1.0.0/), y este proyecto adhiere a [Semantic Versioning](https://semver.org/lang/es/).

## [1.6.0] - 2026-08-20

### ✨ Nuevo
- **`@route` con múltiples parámetros, en cualquier posición.** v0 (v1.2.0) solo permitía un segmento dinámico, y tenía que ser el último — `/blog/:categoria/:slug` se rechazaba. Ahora cualquier cantidad de segmentos `:nombre`, en cualquier posición, se bindean por NOMBRE (no por orden) contra los parámetros del rpc. La precedencia de siempre ("una ruta literal le gana a una dinámica que también matchearía") se generalizó a especificidad: gana la ruta con más segmentos literales fijos, determinísticamente. La detección de conflictos también se generalizó más allá de comparar formas exactas: dos rutas de forma DISTINTA que podrían igual matchear el mismo path real, empatadas en especificidad, se rechazan en compilación (`/blog/:categoria/latest` y `/blog/featured/:slug` matchean las dos `/blog/featured/latest`, y ninguna es más específica). Detalle completo, con el caso de conflicto cruzado explicado paso a paso: GRAMMAR.md §3.42.

## [1.5.0] - 2026-08-20

### ✨ Nuevo
- **CORS configurable y headers de seguridad fijos.** `linkc serve` mandaba `Access-Control-Allow-Origin: *` en toda respuesta, sin forma de acotarlo — sobre una API con auth Bearer, cualquier sitio podía leer una respuesta si el navegador de quien lo visita ya tenía el token guardado. `--cors-origin <origen>` (repetible) o `LINK_CORS_ORIGINS` (separados por coma) cambian el default a un allowlist real: el `Origin` de la request se compara exacto, se ecoa literal (nunca `*`) más `Vary: Origin` si matchea, y se omite el header por completo si no — la request se procesa igual, es el navegador quien bloquea la lectura. Sin configurar nada, el comportamiento no cambia. Además, toda respuesta —incluidos errores y un `stream` SSE, que arma su header a mano y antes no pasaba por el mismo camino— lleva ahora `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY` y `Referrer-Policy: no-referrer`. CSP y HSTS quedan afuera a propósito (CSP depende del contenido de cada página; HSTS le corresponde al reverse proxy que termina TLS, nunca a `linkc serve`). Detalle completo: GRAMMAR.md §3.41.

## [1.4.0] - 2026-08-20

### ✨ Nuevo
- **PostgreSQL: TLS oportunista y reconexión automática.** Los dos gaps reales que quedaban de una misma lista de bloqueos de migración ("Postgres sin pool/TLS/reconexión") — el tercero, pool de conexiones, no aplica: el intérprete es single-threaded, atiende una request a la vez, así que más de una conexión no compraría nada. TLS es `rustls` puro (crates `rustls` + `tokio-postgres-rustls`, backend `ring`), sin OpenSSL ni ninguna librería nativa del sistema, para que los 4 targets de release sigan compilando sin instalar nada — `sslmode` sale de la URL de conexión (`disable` = texto plano de siempre; sin especificar o `prefer` = intenta TLS y cae solo a texto plano si el servidor no lo ofrece, el nuevo default; `require` = TLS obligatorio). Antes de esta ronda, conectar a cualquier proveedor administrado que exige TLS (Supabase, Neon, RDS) era simplemente imposible. Reconexión: una conexión cortada ya no tira abajo el servidor hasta un reinicio manual — la request que la encuentra sigue fallando (nunca se reintenta a ciegas: podría duplicar un INSERT que el servidor ya había aplicado antes de cortarse), pero la conexión se reemplaza antes de devolver ese error, así que la request SIGUIENTE ya encuentra la base sana. Detalle completo, con los límites honestos que quedan (sin backoff más allá de un intento por request, sin fixture de CI con TLS real todavía, sigue sin `LISTEN`/`NOTIFY`): GRAMMAR.md §3.40.

## [1.3.0] - 2026-08-20

### ✨ Nuevo
- **`env.get`, `request.rawBody`/`request.header` y `crypto.hmacSha256`: verificar webhooks de terceros.** Disparado por un análisis de factibilidad de migración real que encontró un bloqueo concreto — sin leer una variable de entorno, ver el body crudo de una request ni calcular un HMAC, ningún rpc podía verificar la firma de un webhook entrante (Stripe, GitHub, o cualquiera que firme sus callbacks) y tenía que confiar en el body a ciegas. Las tres piezas juntas cierran eso. El contexto de la request (body + headers) vive en un `RefCell` sobre `Db` (ya threadeada en todo `runtime/mod.rs`), llenado por `runtime/server.rs` al principio de cada request — mismo criterio que ya usa `Db::subscribers`, en vez de sumar un parámetro más a las ~11 firmas que threadean `db`/`fns`/`checker`/`sessions`. Límite honesto: `rawBody()` requiere que el body sea JSON válido (aunque el rpc no use sus campos), porque el parseo de argumentos corre antes que cualquier rpc sin importar cuántos declare. Detalle completo, con el hallazgo de por qué CSRF NO aplica a este modelo de auth (Bearer-only, sin cookies) en vez de construir middleware para eso: GRAMMAR.md §3.38.
- **`@rate_limit("20/1m")`: límite de requests por cliente.** Como mucho N requests por ventana de tiempo, por `(ip del cliente, servicio, rpc)` — 429 al exceder, mismo shape de error que cualquier otro rechazo. Token bucket con refill continuo (no un contador de ventana fija, que deja pasar el doble en el borde de la ventana). La IP sale de la conexión TCP real (`Request::remote_addr`), nunca de `X-Forwarded-For` sin un proxy de confianza configurado (v0 no lo tiene). Combina con `@authenticated`/`@requires`/`@content_type`/`@route`; corre ANTES que el gate de auth, a propósito, para que un rpc protegido tampoco deje probar credenciales sin límite. Límite honesto: el estado vive en memoria de UN proceso, sin coordinación entre réplicas ni persistencia entre reinicios. Detalle completo: GRAMMAR.md §3.39.

## [1.2.0] - 2026-08-20

### ✨ Nuevo
- **`@route("/blog/:slug")`: URLs amigables para SEO.** `@content_type` (v1.1.0) ya permitía devolver HTML de verdad; el ruteo seguía siendo siempre `/Servicio/rpc`. Ahora un rpc puede declarar una URL alternativa, limpia y rastreable por GET, que convive con la dirección de siempre sin reemplazarla — el cliente TypeScript generado sigue llamando a `/Servicio/rpc`. Un segmento final `:nombre` se bindea a un parámetro `String` o `Int` del rpc con ese mismo nombre; sin parámetro, el rpc no puede pedir ninguno (v0 no lee query string ni body en una `@route`, a propósito, para que sirva tal cual a un crawler). Combina con `@authenticated`/`@requires`. Dos rutas con la misma forma se rechazan en compilación; una literal (`/blog/featured`) siempre gana sobre una dinámica que también matchearía (`/blog/:slug`) con el mismo criterio de precedencia que cualquier router HTTP común -- ese fue, de hecho, el primer bug real que este mismo feature encontró en su propio desarrollo (`cli_route.rs` lo fija como test explícito). Límites de esta ronda (un solo segmento dinámico y tiene que ser el último; no aparece en `openapi.json`; sin trailing slash): GRAMMAR.md §3.37.

## [1.1.1] - 2026-08-20

### 🔥 Arreglo crítico
- **Una tabla PostgreSQL preexistente con `id` no entero tiraba abajo el servidor completo.** Bug introducido en la propia v1.1.0, encontrado en un intento real de migración desde un backend que ya usaba UUID como clave primaria. `CREATE TABLE IF NOT EXISTS` es un no-op sobre una tabla que ya existía y nunca miraba sus columnas; el primer `insert` contra ella hacía panic (`Row::get` de `tokio-postgres` panickea si el valor no convierte al tipo pedido) en el hilo principal del servidor -- así que no fallaba esa request, fallaba el proceso entero, para todos los clientes conectados. Ahora `linkc serve` rechaza el arranque con un mensaje claro (qué tabla, qué tipo encontró) antes de aceptar una sola conexión, y ninguna lectura de PostgreSQL en `store.rs` puede panickear (defensa en profundidad). Detalle completo y verificación contra un PostgreSQL real: GRAMMAR.md §3.36.

## [1.1.0] - 2026-08-20

Versión menor y no de parche porque hay features nuevas (PostgreSQL en runtime,
`@content_type`), no solo correcciones. La única incompatibilidad práctica es
para quien use `linkc` como biblioteca de Rust: `runtime::server::serve` ahora
recibe un `DbSource` en vez de un `PathBuf`. Los programas `.link` no cambian.

### 🔐 Seguridad
- **`crypto.hashPassword` ahora es Argon2id** (RFC 9106) con sal aleatoria por contraseña y salida en formato PHC. Antes era un solo SHA-256 sobre la constante `"link_salt_2026"` — la misma sal para toda aplicación escrita en el lenguaje, sin iteraciones: dos usuarios con la misma contraseña compartían hash y una sola rainbow table las rompía todas.
- **`crypto.verifyPassword` compara en tiempo constante.** La comparación anterior (`==` de `String`) cortaba en el primer byte distinto y filtraba, por tiempo de respuesta, cuánto del hash había acertado quien probaba. Sigue aceptando los hashes del formato viejo para no dejar afuera a los usuarios ya registrados de una app en producción.
- **`crypto.randomToken` y `crypto.uuid` salen del CSPRNG del sistema.** Antes derivaban de `SystemTime::now().as_nanos()`: eran adivinables para quien pudiera acotar el instante de emisión, y dos llamadas dentro del mismo nanosegundo devolvían el mismo valor.
- **Los tokens de sesión piden entropía directo al SO** (`getrandom`), reemplazando el rodeo del hilo descartable sobre `RandomState` que documenta GRAMMAR.md §3.14.
- Detalle completo, con los límites que quedan (parámetros de Argon2id no configurables desde el lenguaje, sin señal de re-hash, el hashing bloquea el hilo del servidor ~15 ms): GRAMMAR.md §3.34.

### ✨ Nuevo
- **PostgreSQL como base de runtime, no solo como DDL generado.** `linkc serve app.link 8787 --db postgres://...` (o `LINK_DATABASE_URL`) corre el programa contra un PostgreSQL real, con el mismo `.link`, el mismo contrato generado y los mismos `test`. Hasta ahora `runtime/postgres.rs` solo emitía `schema.postgres.sql` y `linkc serve` usaba SQLite siempre, sin excepción — el "adaptador enterprise" del README no tenía el otro extremo. SQLite sigue siendo el default. Incluye auto-migración no destructiva (`ADD COLUMN IF NOT EXISTS`), enums simples legibles como texto y structs anidados en JSONB consultable. Límites (una sola conexión sin pool ni TLS, sin `LISTEN`/`NOTIFY`, la columna migrada queda nullable): GRAMMAR.md §3.36.
- Verificado contra un PostgreSQL de verdad en CI (`postgres:16`): CRUD completo por HTTP, persistencia entre reinicios, la tabla leída desde SQL plano, el esquema real comparado contra el que emite `linkc build`, y una migración que agrega un campo sin perder filas.

### ✨ Nuevo
- **`@content_type("...")`: respuestas que no son JSON.** Un rpc que devuelve `String` puede declarar el Content-Type de su respuesta, y entonces el cuerpo se escribe tal cual: HTML, sitemaps XML, CSV, texto plano. Antes el Content-Type estaba literal en el binario (`application/json` para rpcs, `text/event-stream` para streams) y un programa c-script no podía devolver una página, lo que dejaba fuera cualquier render en servidor y cualquier historia de SEO. Las tres capas cambiaron juntas: el servidor manda el header, el cliente TypeScript generado lee `res.text()` en vez de `res.json()`, y el spec OpenAPI declara el mismo tipo. Detalle y límites (sin rutas limpias, sin escapado de HTML, los errores siguen en JSON): GRAMMAR.md §3.35.
- **Las anotaciones de un rpc pasaron a ser una lista.** `@requires(Role.Admin) @content_type("text/html")` es válido — auth y Content-Type son dimensiones distintas, y un panel de administración necesita las dos. El checker sigue rechazando dos anotaciones de la misma dimensión.

### 🩺 Diagnósticos
- **Los tipos en los errores del compilador se escriben como en c-script.** Interpolaban el `Debug` de Rust, así que un error sobre un `T?` mostraba `Optional(Struct { name: Some("Todo"), fields: [FieldType { name: "id", optional: false, ty: Int }, ...] })`. Ahora dice `Todo?`.
- **El acceso a un campo sobre `T?` explica qué hacer**, no solo que no se puede: es el error más frecuente, porque en TypeScript `if (x != null)` sí angosta y en c-script no.

### 📚 Documentación
- **Los ejemplos de la documentación ahora los compila el compilador.** `compiler/tests/docs_examples.rs` toma cada bloque de código c-script publicado en README, `llms.txt`, `llms-full.txt`, `AGENTS.md`, las reglas de Cursor/Copilot y `docs/`, lo compila con el binario real y, si declara un `test "..."`, lo ejecuta. Casi ninguno compilaba: el ejemplo insignia del README usaba `role: Role.Member` sin llaves, y `llms.txt` enseñaba closures con tipo de retorno y lectura de campos sobre un `T?`.
- **Nuevo `AGENTS.md`**: mapa del repo, comandos reales, convenciones y la lista de lo que está roto a sabiendas, para Claude Code / Codex.
- **Sección "Estado" en ambos README**: qué funciona y qué no, con el motivo técnico de cada límite.
- **Índice navegable en GRAMMAR.md** (190 KB, 44 secciones) con anclas compatibles con GitHub.
- Correcciones de honestidad: badge de CI real en vez de una imagen estática que decía "passing" con el CI en rojo, número de tests real (453), el playground etiquetado como la maqueta que es, `npm install -g link-lang` marcado como no publicado, y el copyright del README alineado con LICENSE (decía "Google DeepMind").

## [1.0.0] - 2026-08-15

### 🚀 Novedades y Características Principales
- **Sistema de Tipos Bidireccional Completo**:
  - Inferencia y síntesis bidireccional (⇒ / ⇐).
  - Subtipado estructural para structs y nominal para enums.
  - Tipos Unión (`A | B`) con value-flow subtyping y narrowing exhaustivo en `match`.
  - Genéricos definidos por el usuario con monomorfización estricta.
  - Funciones de primera clase y closures léxicos reales (`|params| { ... }`) con subtyping contravariante en parámetros y covariante en retornos.
  - Tipos `Timestamp` (con precisión de milisegundos en UTC ISO-8601) e `Int64` (mismo rango 64-bit que no pierde precisión en TS).
  - Builtin `now() -> Timestamp` para instanciar fechas del sistema en tiempo de ejecución.
  - Builtins de colección y listas: `.map()`, `.filter()`, `.length()`, `.contains()`, etc.

- **Persistencia SQLite Automática (`db { ... }`)**:
  - Declaración embebida que genera esquemas SQLite en disco automáticamente sin necesidad de migraciones manuales en v1.0.
  - Métodos CRUD nativos fuertemente tipados: `.all()`, `.find(id)`, `.insert(row)`, `.applyPatch(id, patch)`, `.delete(id)`, `.findWhere(fn)`, `.deleteWhere(fn)`.
  - Aislamiento en memoria (`:memory:`) automático para tests.

- **Streaming en Tiempo Real y Push SSE (`stream`)**:
  - Soporte para Server-Sent Events nativo (`Transfer-Encoding: chunked` con flush por evento).
  - Push reactivo con pub-sub interno mediante `while true { db.<col>.subscribe() }` reconociendo mutaciones en vivo (`insert`, `delete`, `applyPatch`).
  - Consumo en cliente TypeScript como `AsyncIterable<T>`.

- **Autenticación de Primera Clase (`auth`)**:
  - Decoradores declarativos `@authenticated` y `@requires(Role.<Variant>)`.
  - Almacén de sesiones opacas de 128 bits seguras sin dependencias externas (`auth.createSession(role)`, `auth.destroySession()`).
  - Validación de cabeceras `Authorization: Bearer <token>` tanto en HTTP como en preflights CORS OPTIONS.

- **Runner de Pruebas Integrado (`test`)**:
  - Bloques de prueba de comportamiento de primer nivel `test "nombre" { ... }`.
  - Builtins `assert(cond, [msg])` y `panic(msg)`.
  - Invocación directa de servicios `Service.rpc(...)`.
  - CLI `linkc test <archivo.link>` con aislamiento de DB por prueba y códigos de salida estándar.
  - Tests de contratos contra snapshots con diff LCS línea a línea (`linkc test <file.link> <file.snap> [--update]`).

- **Generador de TypeScript y SDK Cliente (`linkc build`)**:
  - Generación simétrica de `contract.d.ts`, `client.ts` y validadores en tiempo de ejecución `validators.ts`.
  - Detección y rechazo de respuestas malformadas en el cliente (`LinkValidationError`).
  - Cliente tipado con soporte de streams asíncronos y gestión de tokens (`.setToken()`).

- **Herramientas de Lenguaje y LSP 2.0**:
  - Servidor LSP oficial (`linkc lsp`) con JSON-RPC 2.0 stdio: diagnósticos con spans multilínea UTF-16, autocompletado sensible al tipo del receptor (`x.`), hover de expresiones intermedias y goto-definition entre archivos.
  - Gestor de paquetes con lockfile SHA-256 (`link.lock`) y resolución de dependencias Git directas (`git+https://...#rev`).
  - Hot reload interactivo con `linkc dev <file> <outDir> [port]`.
  - Diagnósticos amigables con sugerencias *"Did you mean?"* para errores de tipeo en variables, campos y tipos.
  - Extensión oficial para Visual Studio Code (`c-script-vscode-1.0.0.vsix`).

---

[1.0.0]: https://github.com/charlessonamericantrading/c-script-/releases/tag/v1.0.0
