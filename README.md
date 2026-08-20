*[Leer en español](README.es.md)*

<div align="center">
  <h1>⚡ Link (c-script)</h1>
  <p><strong>The compiled backend language designed for absolute End-to-End Type Safety with TypeScript.</strong></p>
  
  <p>
    <a href="https://github.com/charlessonamericantrading/c-script-/actions/workflows/ci.yml"><img src="https://github.com/charlessonamericantrading/c-script-/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
    <a href="#-testing--quality-assurance"><img src="https://img.shields.io/badge/tests-501-success.svg" alt="Tests" /></a>
    <a href="https://github.com/charlessonamericantrading/c-script-/releases"><img src="https://img.shields.io/badge/version-1.2.0-blue.svg" alt="Version" /></a>
    <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-purple.svg" alt="License" /></a>
  </p>
</div>

---

## 💡 Why Link?

Whenever you rename a field in your backend or database, your frontend shouldn't silently break in production. With **Link**, your frontend fails to compile (`tsc`) immediately during development.

```
┌─────────────────┐       linkc build        ┌─────────────────────────────────────────┐
│   main.link     │ ───────────────────────► │ 📄 contract.d.ts  (TypeScript types)     │
│                 │                          │ 🔌 client.ts      (Type-safe RPC client) │
│ • Structs/Enums │                          │ 🛡️ validators.ts  (Runtime validation)   │
│ • Typed DB      │                          │ ⚛️ hooks.ts       (React SSR/Streaming)  │
│ • Auth & RBAC   │                          │ 📜 openapi.json   (OpenAPI 3.1 spec)     │
│ • Streams (SSE) │                          │ 🗄️ schema.pg.sql  (PostgreSQL DDL)       │
└─────────────────┘                          └─────────────────────────────────────────┘
```

---

## 📊 Status — what works and what does not

This section is the ground truth. If any other section of this README disagrees with it,
this section wins. Verified on 2026-08-20 by running the compiler, not by reading it.

**Works today**, covered by 501 automated tests:

- `linkc build` / `serve` / `test` / `dev` / `lint` / `doc` / `docker` / `lsp` / `new`
- Embedded SQLite with real persistence across restarts, and non-destructive auto-migrations
- Live push over Server-Sent Events (`stream` + `db.<c>.subscribe()`)
- Declarative auth: `@authenticated`, `@requires(Role.Admin)`, session tokens from the OS CSPRNG
- PostgreSQL as the runtime database: `linkc serve app.link 8787 --db postgres://user:pass@host/db` (or `LINK_DATABASE_URL`), with non-destructive auto-migration. Same program, same generated contract — SQLite remains the default
- Non-JSON responses: `@content_type("text/html; charset=utf-8")` on an rpc returning `String` sends that body verbatim — HTML pages, XML sitemaps, CSV — and stacks with `@requires(Role.Admin)` for pages behind auth
- Friendly URLs: `@route("/blog/:slug")` gives an rpc a clean, crawlable GET path alongside (never instead of) its normal `/Service/rpc` address — the generated client keeps using the latter
- `linkc fmt`, `linkc --help`, and the TypeScript client emitter for multi-service files all work correctly now
- Real password hashing: `crypto.hashPassword` is Argon2id (RFC 9106) with a random per-password salt, in PHC format; `verifyPassword` compares in constant time and still accepts hashes written by the previous version so existing users are not locked out
- Generated TypeScript contract, typed client, runtime validators, React hooks, Zod schemas, OpenAPI 3.1

**Does not work yet** — do not plan around these:

| Limitation | Detail |
|---|---|
| No multi-parameter routes | `@route` supports at most one dynamic segment, and it has to be the last one — `/blog/:category/:slug` is rejected. Model that as separate rpcs for now. |
| No custom 404 page | Errors always come back as JSON, even for an HTML `@route`, so there is no way to render a branded error page. |
| No HTML escaping | Pages are built by concatenating `String`; nothing escapes interpolated data for you. |
| PostgreSQL has no pool, TLS or LISTEN/NOTIFY | The runtime driver works (see below), but it opens a single plain connection and does not reconnect; two server instances against one database do not see each other's writes on `stream`. |
| No npm package | `link-lang` is not on the npm registry yet. GitHub releases work — see Installation below. |
| `linkc wasm` | Deliberately frozen at integer/boolean scalar functions; the production path is `wasm32-wasip1`. |
| The web playground | A static mockup: it does not run the compiler. |

## ⚡ Quick Start (10 Seconds)

### 1. Installation

#### 📦 Linux / macOS (Automatic 1-Line Installer)
```bash
curl -fsSL https://raw.githubusercontent.com/charlessonamericantrading/c-script-/master/install.sh | sh
```

#### 🪟 Windows (PowerShell)
```powershell
irm https://raw.githubusercontent.com/charlessonamericantrading/c-script-/master/install.ps1 | iex
```

#### 🌐 via NPM / npx

> **Not published yet.** `link-lang` is not on the npm registry. Use one of the installers
> above (they download the real prebuilt binary from
> [GitHub Releases](https://github.com/charlessonamericantrading/c-script-/releases)), or
> build from source:

```bash
git clone https://github.com/charlessonamericantrading/c-script-.git
cd c-script-/compiler
cargo build --release        # target/release/linkc
```

---

## 🤖 Built for Cursor & AI Agents (Grok, Claude, GPT)

Link ships the language rules in the format each tool reads, and **every example in those
rules is compiled by the real binary on every CI run** — which is what actually reduces
hallucination: what the agent reads is what the compiler accepts, not a promise.

- **[`AGENTS.md`](AGENTS.md)**: what Claude Code and Codex read first — repository map, real commands, project conventions, and the list of what is knowingly broken so an agent does not report it as a new finding.
- **[`llms.txt`](llms.txt) & [`llms-full.txt`](llms-full.txt)**: the condensed language reference, including the syntax mistakes every LLM makes (enum variants need braces as values, closures take no return type, `T?` cannot be dereferenced).
- **`CLAUDE.md`, `.cursorrules`, `.cursor/rules/c-script.mdc`, `.windsurfrules`, `.github/copilot-instructions.md`**: the same rules in each tool's own format — `CLAUDE.md` is what Claude Code auto-loads on open; it's a thin pointer into `AGENTS.md`, not a duplicate.

Every c-script example in those files is compiled by the real binary on every CI run
(`compiler/tests/docs_examples.rs`), so what an agent reads here is what the compiler
actually accepts.
- **Install Editor Extension in 1-Click**:
  ```bash
  # For Cursor
  cursor --install-extension editors/vscode/c-script-vscode-1.0.0.vsix

  # For VS Code
  code --install-extension editors/vscode/c-script-vscode-1.0.0.vsix
  ```

---

## 🚀 Scaffold Your First App

Create a fullstack project with **Next.js 14**, **Vite+React**, or **Minimal Backend**:

```bash
# Next.js 14 App Router + Link Backend
linkc new my-app --template nextjs

# React + Vite Single Page Application
linkc new my-app --template vite

# Minimal Backend
linkc new my-app --template minimal
```

Then build and run:

```bash
cd my-app
linkc build main.link gen    # Generates typed contracts, client & OpenAPI
linkc serve main.link 3000   # Starts HTTP server with auto-migrating database
```

---

## 🧠 Language at a Glance

<!-- linkc:check -->
```rust
// 1. Data Models & Enums
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

// 2. Typed Database with Auto-Generated SQLite Schemas
db {
  users: User[],
}

// 3. RPC Services with RBAC Access Control & Live SSE Streams
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

  // 4. Real-time Streaming Push Endpoint (SSE)
  stream watchUsers() -> User {
    while true {
      db.users.subscribe()
    }
  }
}

// 5. Integrated Behavioral Tests
test "user creation and count" {
  let count = db.users.all().length();
  assert(count >= 0, "non-negative user count");
}
```

---

## 🛠️ Unified Tooling Suite

Link comes out-of-the-box with all the developer tooling you need:

| Command | Description |
|---|---|
| `linkc new <name> [--template nextjs\|vite\|minimal]` | Scaffolds a fullstack or minimal project |
| `linkc build <file.link> <outdir>` | Generates TS contracts, client, validators, React hooks, Zod & OpenAPI |
| `linkc serve <file.link> <port>` | Runs production HTTP server with embedded SQLite and SSE streaming |
| `linkc dev <file.link> <outdir> [port]` | Live hot-reloading dev mode with automatic server restart |
| `linkc test <file.link>` | Runs built-in behavioral tests in clean sandbox |
| `linkc fmt <file.link> [--check]` | Formats source code according to canonical rules |
| `linkc lint <file.link> [--fix]` | Analyzes code quality and auto-fixes warnings |
| `linkc doc <file.link> [outdir]` | Generates responsive, interactive HTML documentation |
| `linkc docker <file.link> [outdir]` | Generates production multi-stage Dockerfile & docker-compose.yml |
| `linkc wasm <file.link> <out.wasm>` | Compiles pure functions to standard WebAssembly |
| `linkc lsp` | Launches Language Server Protocol for VS Code / Cursor / Neovim |

---

## 🌐 Interactive Web Playground

> **This is a static mockup, not a working playground.** [`playground/index.html`](playground/index.html)
> shows the shape of the generated output for a canned example; it does not run the
> compiler and it does not read what you type. To actually try the language, build
> `linkc` from source and run `linkc build` on a `.link` file.

---

## 🧪 Testing & Quality Assurance

Link is verified by **501 automated unit, integration and CLI tests**, including tests that
spawn the real binary as a subprocess, drive a real HTTP server, and compile every c-script
example published in this repository's documentation:

```bash
cd compiler
cargo test
```

---

## 📄 License

MIT License — Copyright (c) 2026 Charlesson UK Consulting Group LTD. See [LICENSE](LICENSE).
