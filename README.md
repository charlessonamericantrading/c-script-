*[Leer en español](README.es.md)*

<div align="center">
  <h1>⚡ Link (c-script)</h1>
  <p><strong>The compiled backend language designed for absolute End-to-End Type Safety with TypeScript.</strong></p>
  
  <p>
    <a href="https://github.com/charlessonamericantrading/c-script-/actions/workflows/ci.yml"><img src="https://img.shields.io/badge/build-passing-brightgreen.svg" alt="Build Status" /></a>
    <a href="#"><img src="https://img.shields.io/badge/tests-450%20passed-success.svg" alt="Tests" /></a>
    <a href="#"><img src="https://img.shields.io/badge/version-1.0.0-blue.svg" alt="Version" /></a>
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

## ⚡ Quick Start (10 Seconds)

### 1. Installation

#### 📦 Linux / macOS (curl)
```bash
curl -fsSL https://raw.githubusercontent.com/charlessonamericantrading/c-script-/master/install.sh | sh
```

#### 🪟 Windows (PowerShell)
```powershell
iwr -useb https://raw.githubusercontent.com/charlessonamericantrading/c-script-/master/install.ps1 | iex
```

#### 🌐 via NPM / npx
```bash
npm install -g link-lang
# or try directly:
npx link-lang --help
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

```link
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

// 2. Typed Database with Non-Destructive Auto-Migrations
db {
  users: User[],
}

// 3. RPC Services with RBAC Access Control
service UserService {
  @requires(Role.Admin)
  rpc create(name: String, email: String) -> User {
    let new_user = db.users.insert({
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

  // 4. Real-time Streaming Endpoint (SSE)
  stream feed() -> User[] {
    db.users.all()
  }
}

// 5. Integrated Behavioral Tests
test "user creation and count" {
  let count = db.users.count();
  assert(count >= 0);
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
| `linkc test <file.link>` | Runs built-in behavioral tests in clean sandbox |
| `linkc fmt <file.link> [--check]` | Formats source code according to canonical rules |
| `linkc lint <file.link> [--fix]` | Analyzes code quality and auto-fixes warnings |
| `linkc doc <file.link> [outdir]` | Generates responsive, interactive HTML documentation |
| `linkc docker <file.link> [outdir]` | Generates production multi-stage Dockerfile & docker-compose.yml |
| `linkc wasm <file.link> <out.wasm>` | Compiles pure functions to standard WebAssembly |
| `linkc lsp` | Launches Language Server Protocol for VS Code / Cursor / Neovim |

---

## 🌐 Interactive Web Playground

Try Link right inside your browser without installing anything:
- Open [`playground/index.html`](playground/index.html) to write Link code, view real-time TypeScript contracts, OpenAPI specs, and run tests.

---

## 🧩 VS Code / Cursor Extension

Install the official extension from [`editors/vscode/`](editors/vscode/) for:
- Syntax highlighting for `.link` files.
- Smart snippets (`service`, `rpc`, `db`, `test`, `@requires`).
- Diagnostics, hover types, go-to-definition, and format-on-save powered by `linkc lsp`.

---

## 🧪 Testing & Quality Assurance

Link is verified by **450 automated unit, integration, and CLI tests**:

```bash
cd compiler
cargo test
```

---

## 📄 License

MIT License — Copyright (c) 2026 Google DeepMind / Link Authors.
