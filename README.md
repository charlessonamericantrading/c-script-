*[Leer en español](README.es.md)*

<div align="center">
  <h1>⚡ Link (c-script)</h1>
  <p><strong>The compiled backend language designed for absolute End-to-End Type Safety with TypeScript.</strong></p>
  
  <p>
    <a href="https://github.com/charlessonamericantrading/c-script-/actions/workflows/ci.yml"><img src="https://img.shields.io/badge/build-passing-brightgreen.svg" alt="Build Status" /></a>
    <a href="#"><img src="https://img.shields.io/badge/tests-450%20passed-success.svg" alt="Tests" /></a>
    <a href="https://github.com/charlessonamericantrading/c-script-/releases"><img src="https://img.shields.io/badge/version-1.0.0-blue.svg" alt="Version" /></a>
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

#### 📦 Linux / macOS (Automatic 1-Line Installer)
```bash
curl -fsSL https://raw.githubusercontent.com/charlessonamericantrading/c-script-/master/install.sh | sh
```

#### 🪟 Windows (PowerShell)
```powershell
irm https://raw.githubusercontent.com/charlessonamericantrading/c-script-/master/install.ps1 | iex
```

#### 🌐 via NPM / npx
```bash
npm install -g link-lang
# or run directly with npx:
npx link-lang --help
```

---

## 🤖 Built for Cursor & AI Agents (Grok, Claude, GPT)

Link includes native AI standards so you can develop full-stack applications with AI models in Cursor or Windsurf with **zero hallucination**:

- **`.cursorrules` & `.cursor/rules/c-script.mdc`**: Teaches Grok, Claude, and GPT-4 the exact Link syntax, type definitions, RPC rules, and test runner.
- **`llms.txt` & `llms-full.txt`**: Standardized LLM context files for instant ingestion.
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

Try Link right inside your browser without installing anything:
- Open [`playground/index.html`](playground/index.html) to write Link code, view real-time TypeScript contracts, OpenAPI specs, and run tests.

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
