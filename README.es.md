*[Read in English](README.md)*

<div align="center">
  <h1>⚡ Link (c-script)</h1>
  <p><strong>El lenguaje compilado de backend diseñado para garantizar Seguridad de Tipos Extremo a Extremo (End-to-End Type Safety) con TypeScript.</strong></p>
  
  <p>
    <a href="https://github.com/charlessonamericantrading/c-script-/actions/workflows/ci.yml"><img src="https://github.com/charlessonamericantrading/c-script-/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
    <a href="#-testing--quality-assurance"><img src="https://img.shields.io/badge/tests-679-success.svg" alt="Tests" /></a>
    <a href="https://github.com/charlessonamericantrading/c-script-/releases"><img src="https://img.shields.io/badge/versión-1.40.0-blue.svg" alt="Versión" /></a>
    <a href="LICENSE"><img src="https://img.shields.io/badge/licencia-MIT-purple.svg" alt="Licencia" /></a>
  </p>
</div>

---

## 💡 ¿Por qué Link?

Cada vez que renombras un campo en el backend o en la base de datos, tu frontend no debería romperse silenciosamente en producción. Con **Link**, el frontend falla inmediatamente al compilar (`npx tsc --noEmit`) en tu entorno de desarrollo.

```
┌─────────────────┐       linkc build        ┌─────────────────────────────────────────┐
│   main.link     │ ───────────────────────► │ 📄 contract.d.ts  (Tipos TypeScript)     │
│                 │                          │ 🔌 client.ts      (Cliente RPC tipado)   │
│ • Structs/Enums │                          │ 🛡️ validators.ts  (Validación en runtime)│
│ • DB Tipada     │                          │ ⚛️ hooks.ts       (React SSR/Streaming)  │
│ • Auth & RBAC   │                          │ 📜 openapi.json   (Especificación OAS3.1)│
│ • Streams (SSE) │                          │ 🗄️ schema.pg.sql  (DDL PostgreSQL)       │
└─────────────────┘                          └─────────────────────────────────────────┘
```

---

## 📊 Estado — qué funciona y qué no

Esta sección es la verdad de fondo. Si cualquier otra parte de este README la contradice,
gana esta. Verificado el 24/08/2026 corriendo el compilador, no leyéndolo.

**Funciona hoy**, cubierto por 740 pruebas automáticas:

- `linkc build` / `serve` / `test` / `dev` / `lint` / `doc` / `docker` / `lsp` / `new`
- `db.<c>.upsert(matchFn, insertValue, updateFn)`: actualizar-en-el-lugar-o-insertar sin reimplementar a mano buscar+borrar+reinsertar (que ni siquiera preserva el id de la fila con un autoincrement real). `matchFn` recorre toda la colección en el intérprete (mismo límite que `findWhere`/`deleteWhere` -- no empujado a SQL todavía); con match, `updateFn` recibe la fila existente completa y su resultado se aplica sobre ESE MISMO id. `updateFn` devuelve un valor `Omit<T,"id">` completo, no un `Patch<T>` parcial -- deliberado, ya que `Patch<T>` no tiene sintaxis de literal y no se podría construir adentro de un cuerpo de función
- Valores por defecto en campos de struct: `status: String = "pending"` -- misma sintaxis y mecanismo que un default de parámetro de función/rpc. Un campo con default se puede omitir de un literal `Struct { ... }` sin volverse `Optional` -- sigue siendo el mismo tipo declarado. Lo completa el intérprete al construir, evaluado de nuevo cada vez (`token: Uuid = crypto.uuid()` da un valor distinto por literal, no uno cacheado). Se propaga como campo opcional a `contract.d.ts`/`schemas.ts` (Zod), y fuera de `required` en `openapi.json` (más un valor `"default"` literal cuando es una constante simple). Sin acceso a otros campos del mismo literal, y sin soporte todavía en un `type` genérico
- `@validate(email)` / `@validate(regex, "...")` sobre un campo `String`/`String?`, con enforcement real en cuatro lugares -- el servidor real (400 ante un valor inválido, chequeado tanto cuando un rpc recibe el struct entero como parámetro como cuando lo arma adentro del cuerpo a partir de parámetros sueltos -- un `curl` real contra un servidor corriendo fue lo que reveló que ese segundo camino faltaba al principio), `openapi.json` (`format`/`pattern`, keywords estándar de JSON Schema), `schemas.ts`/Zod (`.email()` / `.regex(new RegExp(...))`, encadenado correctamente ANTES de `.nullable()` en un campo opcional) y un comentario JSDoc informativo en `contract.d.ts`. Un patrón regex mal formado es un error de compilación, nunca una sorpresa en el primer request. El límite real: `@validate` está atado a la declaración exacta donde se escribe -- el shape "New*" (`Omit<T,"id">`) que se usa en todos lados para `insert` es un tipo aparte, así que hay que repetir la anotación ahí también. Las funciones `isX()` hand-escritas de `validators.ts` todavía no lo enforce -- todo lo demás sí
- Docstrings `///` sobre un rpc/stream, propagados como `description` en el `openapi.json` generado y como bloque JSDoc multilínea en `contract.d.ts` -- puramente aditivo: `///` ya era válido en cualquier posición (misma trivia que `//`), así que ningún programa existente deja de compilar; el parser solo lee el texto capturado justo arriba de un `rpc`/`stream` (a través de una `@annotation` en el medio, si hay). Se combina con `@deprecated` sobre el mismo rpc en un solo campo/bloque en vez de que uno pise al otro
- `@deprecated("usa X en su lugar")` sobre un campo de struct o un rpc/stream -- puramente informativo, sin efecto en runtime ni en la subtipificación estructural (un struct sigue siendo el mismo tipo lleve o no un campo esta anotación). Se propaga como comentario JSDoc `/** @deprecated ... */` justo antes del campo/método en el `contract.d.ts` generado, y como `deprecated: true` + `description` nativos (keywords de Operation Object / JSON Schema 2020-12, sin extensión `x-*` propia) en `openapi.json`. Sobre un campo es la ÚNICA anotación que se acepta ahí -- cualquier otro nombre (`@authenticated`, etc.) es un error de sintaxis en esa posición, no algo que se ignore en silencio
- Narrowing real de `T?` dentro de un cuerpo de rpc: `match x { v: T => v.campo, null => ... }` liga `v` al `T` real (no `T?`) dentro de esa rama -- reusa la misma maquinaria de patrones exhaustivos que ya narrowaba uniones, así que faltar el caso `null` o el caso de valor es un error de compilación, no una sorpresa en runtime. `if x != null { x.campo }` sigue sin angostar -- eso queda deliberado -- pero `match` sí. `x ?? default` cubre el caso común "dame un default" (encadena de izquierda a derecha: `a ?? b ?? c`), y `x.isSome()`/`x.isNone()` cubren "solo necesito saber si hay valor", los dos sin necesitar un `match` completo
- Tipo nativo `Uuid`: valida la forma canónica `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` en cada borde que un valor puede cruzar -- decodificar una request entrante, `validators.ts`, y el schema Zod generado, los tres con exactamente el mismo chequeo para que nunca puedan discrepar. Tipo aparte de `String`, sin mezcla implícita (misma regla que `Int64` vs `Int`) -- `crypto.uuid()` devuelve `Uuid`, y `.toString()` es la bajada explícita a un string plano
- `linkc introspect <url-postgres>` genera un `.link` de partida (types + `db {...}`) desde el schema de una base PostgreSQL ya existente -- para adoptar un sistema con datos reales en vez de escribir cada campo a mano. Punto de partida para revisar, no listo para producción tal cual: cualquier columna que no pueda mapear con confianza (`jsonb`, `uuid`, un `timestamp`/`timestamptz` nativo) igual sale con un tipo válido (`String`) más una advertencia en stderr, nunca se omite en silencio. Solo PostgreSQL, sin generar ningún `service`
- SQLite embebido con persistencia real entre reinicios y auto-migraciones no destructivas
- Push en vivo sobre Server-Sent Events (`stream` + `db.<c>.subscribe()`)
- Auth declarativa: `@authenticated`, `@requires(Role.Admin)` (o `@requires(Role.Admin | Role.Agent)` para cualquiera de varios roles, todos del mismo enum), tokens de sesión desde el CSPRNG del sistema. `linkc serve --session-ttl 7d` (o `LINK_SESSION_TTL`) hace que las sesiones expiren solas -- sin configurar, siguen viviendo hasta `destroySession()` o un reinicio del proceso, como antes. `auth.currentRole() -> String?` lee qué rol autenticó la request actual desde adentro del cuerpo de un rpc -- deja que un endpoint `Role.Admin | Role.Agent` se comporte distinto según el rol, no solo permitir/denegar; funciona también sin ninguna anotación de auth, `null` si no hay sesión válida. `auth.createSessionWithId(role, userId)` asocia el id del usuario a la sesión y `auth.currentUserId() -> Int?` lo inspecciona desde el cuerpo de cualquier rpc (`null` si no hay sesión o se creó sin id)
- Auth externo: `linkc serve --jwt-secret <secreto>` (o `LINK_JWT_SECRET`) verifica un JWT HS256 ya emitido por un backend existente -- junto con, nunca en vez de, las sesiones propias de Link. `@requires`/`@authenticated`/`auth.currentRole()`/`auth.currentUserId()` funcionan igual sin importar cuál de los dos autenticó la request. `--jwt-role-claim`/`--jwt-user-id-claim` (default `role`/`sub`) eligen qué claims traen el rol y el id de usuario; `sub` acepta un número JSON o un string de dígitos (convención real de OIDC). Solo HS256 -- cualquier otro `alg`, incluido `"none"`, se rechaza antes de siquiera revisar una firma
- PostgreSQL como base de runtime: `linkc serve app.link 8787 --db postgres://usuario:clave@host/base` (o `LINK_DATABASE_URL`), con auto-migración no destructiva (una columna nueva siempre queda nullable, incluso una requerida -- una fila preexistente con `NULL` ahí ahora falla ESA lectura con un error limpio que nombra la fila y el campo, nunca con un `null` silencioso mandado a un cliente tipado ni con el proceso cayéndose), TLS oportunista (rustls puro, sin OpenSSL -- conecta contra proveedores administrados como Supabase/Neon/RDS que lo exigen), reconexión automática tras una conexión cortada, y LISTEN/NOTIFY para que un `stream` conectado a una instancia de `linkc serve` vea una escritura que entró por otra instancia contra la misma base. El mismo programa, el mismo contrato generado — SQLite sigue siendo el default. El `schema.postgres.sql` generado nunca requiere `CREATE EXTENSION` para nada -- verificado aplicándolo con un rol de Postgres real sin privilegios de superusuario/createrole, el tipo que de verdad se tiene en un proveedor gestionado
- Adoptar una base existente sin tocarla: `linkc serve --adopt-existing` (o `LINK_ADOPT_EXISTING`) hace que cada colección declarada asuma que su tabla ya existe -- nunca ejecuta `CREATE TABLE` ni `ALTER TABLE`, ni siquiera el tipo no destructivo de siempre, solo chequeos de solo lectura de que cada columna declarada realmente esté ahí. Para un rol de base sin permiso de DDL (común en producción), o una tabla SQLite/Postgres que ya trae columnas que este programa no modela (que ahora simplemente ignora en vez de negarse a arrancar)
- Respuestas que no son JSON: `@content_type("text/html; charset=utf-8")` sobre un rpc que devuelve `String` manda ese cuerpo tal cual — páginas HTML, sitemaps XML, CSV — y se combina con `@requires(Role.Admin)` para páginas detrás de auth. `"...".escapeHtml()` sanitiza datos no confiables antes de meterlos en una página (no es automático -- se llama donde se interpola). `response.setStatus(code)` elige el status HTTP del camino de éxito (ej. una página 404 propia para un `@route` que no encontró nada, o 201 en un `create` JSON común) — los errores de transporte siguen saliendo siempre en JSON, sin cambios
- URLs amigables: `@route("/blog/:slug")` le da a un rpc una URL limpia y rastreable por GET, además de (nunca en vez de) su dirección normal `/Servicio/rpc` — el cliente generado sigue usando esta última. Cualquier cantidad de segmentos `:parámetro`, en cualquier posición (`/blog/:categoria/:slug`), bindeados por nombre; una ruta más específica (más segmentos fijos) le gana determinísticamente a una totalmente dinámica que también matchearía. Un segmento catch-all final (`:nombre*`) captura el resto del path, unido con `/`. Cualquier parámetro del rpc que NO esté en el path se lee de la query string -- `String`/`Int` obligatorio, `String?`/`Int?` opcional (`null` si no vino) -- un filtro como `?page=2` ya no necesita un rpc aparte; `body` sigue sin leerse, a propósito, porque el punto es una URL que un crawler abre con un GET simple
- Verificar webhooks de terceros: `env.get(name)`, `request.rawBody()` / `request.header(name)` y `crypto.hmacSha256(secret, message)` le dan a un rpc todo lo necesario para chequear la firma de un callback de Stripe/GitHub/etc. antes de confiar en él
- Llamar APIs de terceros: `http.get(url)` / `http.post(url, body)`, más `http.getWithHeaders(url, headers)` / `http.postWithHeaders(url, body, headers)` para llamadas que necesitan `Authorization` u otro header -- `headers` es cualquier `{name: String, value: String}[]` que declares vos, sin ningún tipo builtin de por medio. La respuesta es el body como `String`; un status que no sea 2xx se vuelve un error de runtime normal, no un panic. Cuando importa el status code o los headers de la respuesta (ej. reintentar solo en 429), `http.getWithStatus(url, headers)` / `http.postWithStatus(url, body, headers)` devuelven `{status: Int, headers: {name: String, value: String}[], body: String}` -- mismo principio de tipo estructural, un 4xx/5xx es un dato, no un error
- Paginación real: `db.<c>.page(limit, offset)` empuja `LIMIT`/`OFFSET` al SQL de verdad (SQLite y Postgres, los dos) en vez de traer la tabla entera y cortarla en memoria -- mismo orden que `.all()`, así que las páginas nunca se solapan ni se saltean una fila. `db.<c>.pageAfter(cursor, limit)` es una alternativa por cursor para scroll infinito/paginación secuencial -- el cursor es el último `id` visto (`null` para la primera página), estable ante inserciones concurrentes a diferencia de `OFFSET`, que cuenta filas desde el principio en cada llamada
- Agregación real: `db.<c>.sumBy(selectorDeGrupo, selectorDeValor)` / `countBy(selectorDeGrupo)` / `avgBy` / `maxBy` / `minBy` empujan un `GROUP BY` al SQL de verdad -- MRR por plan, conteos por estado -- en vez de traer cada fila a memoria. Los selectores tienen que ser un acceso de campo simple (`|o: Order| { o.planId }`); agrupar es por `String`/`Int`/`Int64`/`Bool`/`enum` (sin truncado de fechas todavía), el campo agregado tiene que ser `Int`/`Int64`/`Float` -- `Int64` sigue siendo `Int64` en el resultado, nunca se degrada a `Int` en silencio. Agrupar por un campo `enum` devuelve el enum real como key, no un string
- Límite de requests por cliente: `@rate_limit("20/1m")` acota un rpc a N requests por ventana de tiempo, por `(ip del cliente, servicio, rpc)`, con 429 al exceder — token bucket con refill continuo
- Mandar email: `smtp.send(to, subject, body)` — la conexión (`LINK_SMTP_URL`) y el remitente (`LINK_SMTP_FROM`) salen del entorno del proceso, nunca de argumentos del rpc. TLS con rustls puro, mismo stack que el driver de PostgreSQL. `smtp.sendToMany(to: String[], subject, body)` manda un solo mensaje con un `RCPT TO` por destinatario; `smtp.sendHtml(to: String[], subject, html)` manda un body HTML (`Content-Type: text/html`) a uno o varios destinatarios -- `send` en sí queda sin cambios
- CORS configurable y headers de seguridad fijos: `--cors-origin <origen>` (repetible, o `LINK_CORS_ORIGINS`) pasa del `*` abierto a un allowlist real (match exacto, ecoado literal + `Vary: Origin`); toda respuesta -- incluidos errores y un `stream` SSE -- lleva `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`
- `linkc fmt`, `linkc --help`, y el emisor de cliente TypeScript para archivos multi-service funcionan correctamente ahora
- Hashing de contraseñas real: `crypto.hashPassword` es Argon2id (RFC 9106) con sal aleatoria por contraseña, en formato PHC; `verifyPassword` compara en tiempo constante y sigue aceptando los hashes de la versión anterior para no dejar afuera a los usuarios ya registrados
- Aleatoriedad numérica y comparación en tiempo constante para código de usuario: `crypto.randomInt(min, max)` da un `Int` uniforme en ese rango inclusive desde el CSPRNG del sistema (con rechazo de muestreo contra el sesgo de módulo) — alcanza para un OTP de verdad, a diferencia del alfabeto hex de `randomToken`; `crypto.timingSafeEqual(a, b)` expone la misma comparación en tiempo constante que `verifyPassword` ya usaba internamente, para comparar un secreto de webhook o una API key sin filtrar nada por el tiempo de respuesta
- `.toString()` sobre `Int`/`Int64`/`Float`/`Bool` — conversión explícita, nunca automática (mismo principio que `toInt64()`); `Bool` no tenía ni un solo método antes de esto. `response.setStatus` dentro de un `stream` ahora es error de compilación en vez de no-op silencioso. `@route` soporta un segmento catch-all final (`:nombre*`) que captura cero o más segmentos restantes del path unidos por `/`, para rutas de profundidad variable (documentación, un CMS) — siempre `String`, nunca `Int`, y siempre el último segmento de la ruta
- Costo de hashing de contraseñas configurable: `linkc serve --argon2-memory-kib <N> --argon2-iterations <N>` (o `LINK_ARGON2_MEMORY_KIB`/`LINK_ARGON2_ITERATIONS`) sube el costo de Argon2id de `crypto.hashPassword` por encima del default de la crate; sin configurar, el comportamiento no cambia. `crypto.isLegacyHash(hash: String) -> Bool` le dice a quien llama si un hash guardado es el formato legado pre-Argon2id, para re-hashear de forma proactiva en el login en vez de mirar el prefijo a mano. Una tabla de PostgreSQL con un id autoincremental preexistente de 32 o 16 bits (`SERIAL`/`IDENTITY`, no solo `BIGSERIAL`) ya no falla en el primer insert — conectar ya lo aceptaba, leer la columna del id ahora también
- Contrato TypeScript, cliente tipado, validadores runtime, hooks de React, schemas Zod y OpenAPI 3.1 generados

**Todavía no funciona** — no planifiques sobre esto:

| Límite | Detalle |
|---|---|
| `@rate_limit` es por proceso, en memoria | Sin persistencia entre reinicios, sin coordinación entre réplicas si el mismo `.link` corre en más de un proceso; la IP del cliente sale de la conexión TCP real, nunca de `X-Forwarded-For` (sin config de proxy de confianza todavía, así que detrás de un proxy esto limita por la IP del proxy). |
| `request.rawBody()` necesita un body JSON | El parseo de argumentos corre antes que cualquier rpc, sin importar cuántos parámetros declare, así que un body que no sea JSON (form-encoded, XML) nunca llega al rpc — un payload de webhook en JSON con campos de más funciona bien. |
| Sin CSP ni HSTS | CSP depende del contenido real de cada página (sin eso, no hay default seguro posible); HSTS solo tiene sentido sobre una conexión que YA es HTTPS, y `linkc serve` nunca lo es por sí mismo -- las dos le corresponden al reverse proxy que termina TLS delante. Las entradas del allowlist de CORS son match exacto únicamente, sin wildcards de subdominio. |
| `smtp` sin adjuntos, cc/bcc, ni envío asíncrono | `smtp.send`/`sendToMany`/`sendHtml` cubren texto plano y HTML a uno o varios destinatarios (desde esta ronda), pero ninguno de los tres acepta un adjunto ni una lista cc/bcc, y los tres son sincrónicos -- un relay lento hace lento a TODO el servidor (single-threaded) mientras dura esa request. |
| `--session-ttl` limpia de forma perezosa | Una sesión vencida se borra de memoria recién la próxima vez que se usa su token -- una creada y nunca vuelta a usar queda en memoria hasta que el proceso reinicia. |
| La estructura completa de usuario no se auto-carga en sesión | `auth.currentRole()` y `auth.currentUserId()` exponen el rol autenticado y el id numérico del usuario, pero cargar el struct `User` completo en memoria se hace explícitamente vía `db.users.find(uid)`. |
| La agregación (`sumBy`/etc.) no tiene truncado de fechas | No se puede agrupar por una fecha truncada (cohortes mensuales, por ejemplo) -- agrupar por un campo `Timestamp` desnudo no se acepta, y no hay ningún método de truncado para acotarlo antes. Soporte de `Int64` ya está -- ver Funciona hoy. |
| El push de `stream` entre instancias (LISTEN/NOTIFY) tiene límites reales | Una fila cambiada de más de 8000 bytes (el límite de payload de NOTIFY que impone el propio Postgres) no se propaga a otras instancias -- sigue publicándose local donde se escribió. NOTIFY es best-effort, sin cola de reintento; un servidor inactivo puede tardar hasta 200ms en notar un cambio remoto; cada instancia abre una conexión extra a Postgres solo para LISTEN; SQLite no participa en absoluto. |
| Sin paquete npm | `link-lang` todavía no está en el registro de npm. Los releases de GitHub sí funcionan — ver Instalación más abajo. |
| `linkc wasm` | Congelado a propósito en funciones escalares de enteros/booleanos; el camino de producción es `wasm32-wasip1`. |
| El playground web compila un solo archivo | Corre el lexer/parser/checker/generadores reales (compilados a `wasm32-unknown-unknown`), pero sin pasar por el cargador de módulos: sin `import` entre archivos, y sin ejecutar `test` (eso necesita el intérprete nativo). |

## ⚡ Inicio Rápido (10 Segundos)

### 1. Instalación

#### 📦 Linux / macOS (Instalador automático de 1 línea)
```bash
curl -fsSL https://raw.githubusercontent.com/charlessonamericantrading/c-script-/master/install.sh | sh
```

#### 🪟 Windows (PowerShell)
```powershell
irm https://raw.githubusercontent.com/charlessonamericantrading/c-script-/master/install.ps1 | iex
```

#### 🌐 Vía NPM / npx

> **Todavía no está publicado.** `link-lang` no está en el registro de npm. Usá alguno de
> los instaladores de arriba (bajan el binario real precompilado desde
> [GitHub Releases](https://github.com/charlessonamericantrading/c-script-/releases)), o
> compilá desde el código:

```bash
git clone https://github.com/charlessonamericantrading/c-script-.git
cd c-script-/compiler
cargo build --release        # target/release/linkc
```

---

## 🤖 Diseñado para Cursor y Agentes de IA (Grok, Claude, GPT)

Link trae las reglas del lenguaje en el formato que lee cada herramienta, y **cada ejemplo
de esas reglas lo compila el binario real en cada corrida de CI** (`compiler/tests/docs_examples.rs`)
— que es lo que de verdad baja las alucinaciones: que lo que el agente lee sea lo que el
compilador acepta, no una promesa.

- **[`AGENTS.md`](AGENTS.md)**: lo que leen primero Claude Code y Codex — mapa del repo, comandos reales, convenciones del proyecto, y la lista de lo que está roto a sabiendas para que un agente no lo reporte como hallazgo nuevo.
- **[`llms.txt`](llms.txt) y [`llms-full.txt`](llms-full.txt)**: la referencia condensada del lenguaje, con los errores de sintaxis que comete todo LLM (las variantes de enum necesitan llaves como valor, los closures no llevan tipo de retorno, un `T?` no se puede desreferenciar).
- **`CLAUDE.md`, `.cursorrules`, `.cursor/rules/c-script.mdc`, `.windsurfrules`, `.github/copilot-instructions.md`**: las mismas reglas en el formato de cada herramienta — `CLAUDE.md` es lo que Claude Code carga solo al abrir el repo; es un puntero chico a `AGENTS.md`, no un duplicado.
- **Instalar la extensión del editor en 1 clic**:
  ```bash
  # Para Cursor
  cursor --install-extension editors/vscode/c-script-vscode-1.0.0.vsix

  # Para VS Code
  code --install-extension editors/vscode/c-script-vscode-1.0.0.vsix
  ```

---

## 🚀 Creá tu Primer Proyecto Fullstack

Scaffoldeá un proyecto completo con **Next.js 14**, **Vite+React** o **Backend Minimal**:

```bash
# Next.js 14 App Router + Backend Link Tipado
linkc new my-app --template nextjs

# React + Vite Single Page Application
linkc new my-app --template vite

# Backend Minimal
linkc new my-app --template minimal
```

Compilá y levantá el servidor:

```bash
cd my-app
linkc build main.link gen    # Genera contratos tipados, cliente y OpenAPI
linkc serve main.link 3000   # Inicia servidor HTTP con SQLite embebido y auto-migraciones
```

---

## 🧠 El Lenguaje de un Vistazo

<!-- linkc:check -->
```rust
// 1. Modelos de Datos y Enums
type User = {
  id: Int,
  name: String,
  email: String,
  role: Role,
  created_at: Timestamp,
}

enum Role {
  Admin,
  Member,
}

// 2. Base de Datos Tipada con Esquemas SQLite Automáticos
db {
  users: User[],
}

// 3. Servicios RPC con Control de Acceso por Roles (RBAC) y Streaming SSE
service UserService {
  @requires(Role.Admin)
  rpc create(name: String, email: String) -> User {
    let new_user = db.users.insert(User {
      id: 0,
      name: name,
      email: email,
      role: Role.Member {},
      created_at: now(),
    });
    new_user
  }

  rpc list() -> User[] {
    db.users.all()
  }

  // 4. Endpoint de Streaming Push en Tiempo Real (SSE)
  stream watchUsers() -> User {
    while true {
      db.users.subscribe()
    }
  }
}

// 5. Pruebas de Comportamiento Integradas
test "creacion y conteo de usuarios" {
  let count = db.users.all().length();
  assert(count >= 0, "conteo no negativo de usuarios");
}
```

---

## 🛠️ Suite Completa de Herramientas CLI

Link incluye de forma nativa todas las herramientas que necesitas:

| Subcomando | Descripción |
|---|---|
| `linkc new <nombre> [--template nextjs\|vite\|minimal]` | Scaffoldea proyectos fullstack o backend |
| `linkc build <archivo.link> <outdir>` | Genera contratos TS, cliente, validadores, hooks React, Zod y OpenAPI |
| `linkc serve <archivo.link> <puerto>` | Inicia servidor HTTP con SQLite embebido y SSE streams |
| `linkc dev <archivo.link> <outdir> [puerto]` | Modo desarrollo interactivo con hot reload y reinicio de servidor |
| `linkc test <archivo.link>` | Ejecuta pruebas de comportamiento integradas en entorno aislado |
| `linkc fmt <archivo.link> [--check]` | Formatea el código fuente canónicamente |
| `linkc lint <archivo.link> [--fix]` | Analiza calidad de código y auto-corrige advertencias |
| `linkc doc <archivo.link> [outdir]` | Genera portal web interactivo de documentación HTML |
| `linkc docker <archivo.link> [outdir]` | Genera Dockerfile multi-etapa (<15MB) y docker-compose.yml |
| `linkc wasm <archivo.link> <out.wasm>` | Compila algoritmos y funciones a WebAssembly nativo |
| `linkc lsp` | Servidor Language Server Protocol para VS Code / Cursor / Neovim |

---

## 🌐 Playground Web Interactivo

[`playground/index.html`](playground/index.html) corre el lexer, parser, checker de tipos y
generadores REALES de `linkc` en tu navegador -- compilados a `wasm32-unknown-unknown` vía el
crate [`playground-wasm`](compiler/playground-wasm), no una demo enlatada. Compila un solo
archivo (sin `import` entre archivos) y no ejecuta `test` (eso necesita el intérprete nativo --
para eso, `linkc test` local). Para probarlo:

```bash
cd playground && python3 -m http.server 8000   # cualquier servidor estático sirve
# abrí http://localhost:8000/ -- abrir index.html directo por file:// NO funciona,
# los navegadores bloquean fetch() de archivos locales, y el módulo wasm carga vía fetch()
```

Para regenerar `playground/pkg/` después de tocar el compilador:

```bash
cd compiler/playground-wasm
cargo build --target wasm32-unknown-unknown --release
wasm-bindgen --target web --out-dir ../../playground/pkg --out-name playground_wasm \
  target/wasm32-unknown-unknown/release/playground_wasm.wasm
```

---

## 🧪 Pruebas y Control de Calidad

El compilador y el runtime de Link están verificados por **679 pruebas automáticas** unitarias,
de integración y de CLI, incluidas pruebas que levantan el binario real como subproceso, manejan
un servidor HTTP real, y compilan cada ejemplo de c-script publicado en la documentación de este repo:

```bash
cd compiler
cargo test
```

---

## 📄 Licencia

Licencia MIT — Copyright (c) 2026 Charlesson UK Consulting Group LTD. Ver [LICENSE](LICENSE).
