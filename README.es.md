*[Read in English](README.md)*

<div align="center">
  <h1>⚡ Link (c-script)</h1>
  <p><strong>El lenguaje compilado de backend diseñado para garantizar Seguridad de Tipos Extremo a Extremo (End-to-End Type Safety) con TypeScript.</strong></p>
  
  <p>
    <a href="https://github.com/charlessonamericantrading/c-script-/actions/workflows/ci.yml"><img src="https://github.com/charlessonamericantrading/c-script-/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
    <a href="#-testing--quality-assurance"><img src="https://img.shields.io/badge/tests-555-success.svg" alt="Tests" /></a>
    <a href="https://github.com/charlessonamericantrading/c-script-/releases"><img src="https://img.shields.io/badge/versión-1.14.0-blue.svg" alt="Versión" /></a>
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
gana esta. Verificado el 21/08/2026 corriendo el compilador, no leyéndolo.

**Funciona hoy**, cubierto por 555 pruebas automáticas:

- `linkc build` / `serve` / `test` / `dev` / `lint` / `doc` / `docker` / `lsp` / `new`
- SQLite embebido con persistencia real entre reinicios y auto-migraciones no destructivas
- Push en vivo sobre Server-Sent Events (`stream` + `db.<c>.subscribe()`)
- Auth declarativa: `@authenticated`, `@requires(Role.Admin)` (o `@requires(Role.Admin | Role.Agent)` para cualquiera de varios roles, todos del mismo enum), tokens de sesión desde el CSPRNG del sistema. `linkc serve --session-ttl 7d` (o `LINK_SESSION_TTL`) hace que las sesiones expiren solas -- sin configurar, siguen viviendo hasta `destroySession()` o un reinicio del proceso, como antes
- PostgreSQL como base de runtime: `linkc serve app.link 8787 --db postgres://usuario:clave@host/base` (o `LINK_DATABASE_URL`), con auto-migración no destructiva, TLS oportunista (rustls puro, sin OpenSSL -- conecta contra proveedores administrados como Supabase/Neon/RDS que lo exigen), reconexión automática tras una conexión cortada, y LISTEN/NOTIFY para que un `stream` conectado a una instancia de `linkc serve` vea una escritura que entró por otra instancia contra la misma base. El mismo programa, el mismo contrato generado — SQLite sigue siendo el default
- Respuestas que no son JSON: `@content_type("text/html; charset=utf-8")` sobre un rpc que devuelve `String` manda ese cuerpo tal cual — páginas HTML, sitemaps XML, CSV — y se combina con `@requires(Role.Admin)` para páginas detrás de auth. `"...".escapeHtml()` sanitiza datos no confiables antes de meterlos en una página (no es automático -- se llama donde se interpola). `response.setStatus(code)` elige el status HTTP del camino de éxito (ej. una página 404 propia para un `@route` que no encontró nada, o 201 en un `create` JSON común) — los errores de transporte siguen saliendo siempre en JSON, sin cambios
- URLs amigables: `@route("/blog/:slug")` le da a un rpc una URL limpia y rastreable por GET, además de (nunca en vez de) su dirección normal `/Servicio/rpc` — el cliente generado sigue usando esta última. Cualquier cantidad de segmentos `:parámetro`, en cualquier posición (`/blog/:categoria/:slug`), bindeados por nombre; una ruta más específica (más segmentos fijos) le gana determinísticamente a una totalmente dinámica que también matchearía
- Verificar webhooks de terceros: `env.get(name)`, `request.rawBody()` / `request.header(name)` y `crypto.hmacSha256(secret, message)` le dan a un rpc todo lo necesario para chequear la firma de un callback de Stripe/GitHub/etc. antes de confiar en él
- Llamar APIs de terceros: `http.get(url)` / `http.post(url, body)`, más `http.getWithHeaders(url, headers)` / `http.postWithHeaders(url, body, headers)` para llamadas que necesitan `Authorization` u otro header -- `headers` es cualquier `{name: String, value: String}[]` que declares vos, sin ningún tipo builtin de por medio. La respuesta es el body como `String`; un status que no sea 2xx se vuelve un error de runtime normal, no un panic
- Paginación real: `db.<c>.page(limit, offset)` empuja `LIMIT`/`OFFSET` al SQL de verdad (SQLite y Postgres, los dos) en vez de traer la tabla entera y cortarla en memoria -- mismo orden que `.all()`, así que las páginas nunca se solapan ni se saltean una fila
- Límite de requests por cliente: `@rate_limit("20/1m")` acota un rpc a N requests por ventana de tiempo, por `(ip del cliente, servicio, rpc)`, con 429 al exceder — token bucket con refill continuo
- Mandar email: `smtp.send(to, subject, body)` — la conexión (`LINK_SMTP_URL`) y el remitente (`LINK_SMTP_FROM`) salen del entorno del proceso, nunca de argumentos del rpc. TLS con rustls puro, mismo stack que el driver de PostgreSQL
- CORS configurable y headers de seguridad fijos: `--cors-origin <origen>` (repetible, o `LINK_CORS_ORIGINS`) pasa del `*` abierto a un allowlist real (match exacto, ecoado literal + `Vary: Origin`); toda respuesta -- incluidos errores y un `stream` SSE -- lleva `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`
- `linkc fmt`, `linkc --help`, y el emisor de cliente TypeScript para archivos multi-service funcionan correctamente ahora
- Hashing de contraseñas real: `crypto.hashPassword` es Argon2id (RFC 9106) con sal aleatoria por contraseña, en formato PHC; `verifyPassword` compara en tiempo constante y sigue aceptando los hashes de la versión anterior para no dejar afuera a los usuarios ya registrados
- Contrato TypeScript, cliente tipado, validadores runtime, hooks de React, schemas Zod y OpenAPI 3.1 generados

**Todavía no funciona** — no planifiques sobre esto:

| Límite | Detalle |
|---|---|
| `@rate_limit` es por proceso, en memoria | Sin persistencia entre reinicios, sin coordinación entre réplicas si el mismo `.link` corre en más de un proceso; la IP del cliente sale de la conexión TCP real, nunca de `X-Forwarded-For` (sin config de proxy de confianza todavía, así que detrás de un proxy esto limita por la IP del proxy). |
| `request.rawBody()` necesita un body JSON | El parseo de argumentos corre antes que cualquier rpc, sin importar cuántos parámetros declare, así que un body que no sea JSON (form-encoded, XML) nunca llega al rpc — un payload de webhook en JSON con campos de más funciona bien. |
| Sin CSP ni HSTS | CSP depende del contenido real de cada página (sin eso, no hay default seguro posible); HSTS solo tiene sentido sobre una conexión que YA es HTTPS, y `linkc serve` nunca lo es por sí mismo -- las dos le corresponden al reverse proxy que termina TLS delante. Las entradas del allowlist de CORS son match exacto únicamente, sin wildcards de subdominio. |
| `smtp.send` es texto plano, un destinatario, bloqueante | Sin body HTML, sin adjuntos, sin cc/bcc; mandar a varios es una llamada por destinatario. Es sincrónico -- un relay lento hace lento a TODO el servidor (single-threaded) mientras dura esa request. |
| `http.get`/`http.post` (con o sin headers) solo devuelven el body | Sin acceso al status code ni a los headers de la RESPUESTA -- un 4xx/5xx de la API llamada se vuelve un error de runtime genérico, no un valor sobre el que el programa pueda ramificar (ej. reintentar solo en 429). |
| `db.<c>.page` no tiene cursor | El caller calcula el próximo `offset` a mano (`offset + limit`); sin token de continuación opaco, y sin total de páginas incorporado -- para eso, `count()`. |
| `--session-ttl` limpia de forma perezosa | Una sesión vencida se borra de memoria recién la próxima vez que se usa su token -- una creada y nunca vuelta a usar queda en memoria hasta que el proceso reinicia. |
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

El compilador y el runtime de Link están verificados por **555 pruebas automáticas** unitarias,
de integración y de CLI, incluidas pruebas que levantan el binario real como subproceso, manejan
un servidor HTTP real, y compilan cada ejemplo de c-script publicado en la documentación de este repo:

```bash
cd compiler
cargo test
```

---

## 📄 Licencia

Licencia MIT — Copyright (c) 2026 Charlesson UK Consulting Group LTD. Ver [LICENSE](LICENSE).
