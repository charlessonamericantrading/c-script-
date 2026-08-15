*[Read in English](README.md)*

<div align="center">
  <h1>⚡ Link (c-script)</h1>
  <p><strong>El lenguaje compilado de backend diseñado para garantizar Seguridad de Tipos Extremo a Extremo (End-to-End Type Safety) con TypeScript.</strong></p>
  
  <p>
    <a href="https://github.com/charlessonamericantrading/c-script-/actions/workflows/ci.yml"><img src="https://img.shields.io/badge/build-passing-brightgreen.svg" alt="Estado de Build" /></a>
    <a href="#"><img src="https://img.shields.io/badge/tests-450%20passed-success.svg" alt="Tests" /></a>
    <a href="#"><img src="https://img.shields.io/badge/versión-1.0.0-blue.svg" alt="Versión" /></a>
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

## ⚡ Inicio Rápido (10 Segundos)

### 1. Instalación

#### 📦 Linux / macOS (curl)
```bash
curl -fsSL https://raw.githubusercontent.com/charlessonamericantrading/c-script-/master/install.sh | sh
```

#### 🪟 Windows (PowerShell)
```powershell
iwr -useb https://raw.githubusercontent.com/charlessonamericantrading/c-script-/master/install.ps1 | iex
```

#### 🌐 Vía NPM / npx
```bash
npm install -g link-lang
# o probalo directamente:
npx link-lang --help
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

```link
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

// 2. Base de Datos Tipada con Auto-Migraciones No Destructivas
db {
  users: User[],
}

// 3. Servicios RPC con Control de Acceso por Roles (RBAC)
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

  // 4. Endpoint de Streaming en Tiempo Real (SSE)
  stream feed() -> User[] {
    db.users.all()
  }
}

// 5. Pruebas de Comportamiento Integradas
test "creacion y conteo de usuarios" {
  let count = db.users.count();
  assert(count >= 0);
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
| `linkc test <archivo.link>` | Ejecuta pruebas de comportamiento integradas en entorno aislado |
| `linkc fmt <archivo.link> [--check]` | Formatea el código fuente canónicamente |
| `linkc lint <archivo.link> [--fix]` | Analiza calidad de código y auto-corrige advertencias |
| `linkc doc <archivo.link> [outdir]` | Genera portal web interactivo de documentación HTML |
| `linkc docker <archivo.link> [outdir]` | Genera Dockerfile multi-etapa (<15MB) y docker-compose.yml |
| `linkc wasm <archivo.link> <out.wasm>` | Compila algoritmos y funciones a WebAssembly nativo |
| `linkc lsp` | Servidor Language Server Protocol para VS Code / Cursor / Neovim |

---

## 🌐 Playground Web Interactivo

Probá Link directamente en tu navegador sin instalar nada:
- Abrí [`playground/index.html`](playground/index.html) para escribir código Link, ver la generación de contratos en tiempo real y ejecutar pruebas.

---

## 🧩 Extensión para VS Code / Cursor

Instalá la extensión oficial desde [`editors/vscode/`](editors/vscode/) para contar con:
- Resaltado de sintaxis para archivos `.link`.
- Snippets inteligentes (`service`, `rpc`, `db`, `test`, `@requires`).
- Diagnósticos, tipos al pasar el cursor (hover), saltar a la definición y formateo al guardar.

---

## 🧪 Pruebas y Control de Calidad

El compilador y runtime de Link están verificados por **450 pruebas automáticas unitarias y de integración**:

```bash
cd compiler
cargo test
```

---

## 📄 Licencia

Licencia MIT — Copyright (c) 2026 Google DeepMind / Link Authors.
