*[Read in English](README.md)*

<div align="center">
  <h1>⚡ Link (c-script)</h1>
  <p><strong>El lenguaje compilado de backend diseñado para garantizar Seguridad de Tipos Extremo a Extremo (End-to-End Type Safety) con TypeScript.</strong></p>
  
  <p>
    <a href="https://github.com/charlessonamericantrading/c-script-/actions/workflows/ci.yml"><img src="https://github.com/charlessonamericantrading/c-script-/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
    <a href="#-testing--quality-assurance"><img src="https://img.shields.io/badge/tests-501-success.svg" alt="Tests" /></a>
    <a href="https://github.com/charlessonamericantrading/c-script-/releases"><img src="https://img.shields.io/badge/versión-1.2.0-blue.svg" alt="Versión" /></a>
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
gana esta. Verificado el 20/08/2026 corriendo el compilador, no leyéndolo.

**Funciona hoy**, cubierto por 501 pruebas automáticas:

- `linkc build` / `serve` / `test` / `dev` / `lint` / `doc` / `docker` / `lsp` / `new`
- SQLite embebido con persistencia real entre reinicios y auto-migraciones no destructivas
- Push en vivo sobre Server-Sent Events (`stream` + `db.<c>.subscribe()`)
- Auth declarativa: `@authenticated`, `@requires(Role.Admin)`, tokens de sesión desde el CSPRNG del sistema
- PostgreSQL como base de runtime: `linkc serve app.link 8787 --db postgres://usuario:clave@host/base` (o `LINK_DATABASE_URL`), con auto-migración no destructiva. El mismo programa, el mismo contrato generado — SQLite sigue siendo el default
- Respuestas que no son JSON: `@content_type("text/html; charset=utf-8")` sobre un rpc que devuelve `String` manda ese cuerpo tal cual — páginas HTML, sitemaps XML, CSV — y se combina con `@requires(Role.Admin)` para páginas detrás de auth
- URLs amigables: `@route("/blog/:slug")` le da a un rpc una URL limpia y rastreable por GET, además de (nunca en vez de) su dirección normal `/Servicio/rpc` — el cliente generado sigue usando esta última
- `linkc fmt`, `linkc --help`, y el emisor de cliente TypeScript para archivos multi-service funcionan correctamente ahora
- Hashing de contraseñas real: `crypto.hashPassword` es Argon2id (RFC 9106) con sal aleatoria por contraseña, en formato PHC; `verifyPassword` compara en tiempo constante y sigue aceptando los hashes de la versión anterior para no dejar afuera a los usuarios ya registrados
- Contrato TypeScript, cliente tipado, validadores runtime, hooks de React, schemas Zod y OpenAPI 3.1 generados

**Todavía no funciona** — no planifiques sobre esto:

| Límite | Detalle |
|---|---|
| Sin rutas multi-parámetro | `@route` soporta como mucho un segmento dinámico, y tiene que ser el último — `/blog/:categoria/:slug` se rechaza. Por ahora, modelá eso como rpc separados. |
| Sin página 404 propia | Los errores siempre vuelven en JSON, incluso en una `@route` de HTML, así que no hay forma de renderizar una página de error propia. |
| Sin escapado de HTML | Las páginas se arman concatenando `String`; nada escapa por vos los datos que interpolás. |
| PostgreSQL sin pool, TLS ni LISTEN/NOTIFY | El driver de runtime funciona (ver abajo), pero abre una sola conexión sin cifrar y no reconecta; dos instancias del servidor contra la misma base no ven las escrituras de la otra en un `stream`. |
| Sin paquete npm | `link-lang` todavía no está en el registro de npm. Los releases de GitHub sí funcionan — ver Instalación más abajo. |
| `linkc wasm` | Congelado a propósito en funciones escalares de enteros/booleanos; el camino de producción es `wasm32-wasip1`. |
| El playground web | Es una maqueta estática: no ejecuta el compilador. |

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

> **Es una maqueta estática, no un playground que funcione.** [`playground/index.html`](playground/index.html)
> muestra la forma que tiene la salida generada para un ejemplo enlatado; no ejecuta el
> compilador y no lee lo que escribís. Para probar el lenguaje de verdad, compilá `linkc`
> desde el código y corré `linkc build` sobre un archivo `.link`.

---

## 🧪 Pruebas y Control de Calidad

El compilador y el runtime de Link están verificados por **501 pruebas automáticas** unitarias,
de integración y de CLI, incluidas pruebas que levantan el binario real como subproceso, manejan
un servidor HTTP real, y compilan cada ejemplo de c-script publicado en la documentación de este repo:

```bash
cd compiler
cargo test
```

---

## 📄 Licencia

Licencia MIT — Copyright (c) 2026 Charlesson UK Consulting Group LTD. Ver [LICENSE](LICENSE).
