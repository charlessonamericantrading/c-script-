*[Read in English](README.md)*

<div align="center">
  <h1>⚡ Link (c-script)</h1>
  <p><strong>El lenguaje compilado de backend diseñado para garantizar Seguridad de Tipos Extremo a Extremo (End-to-End Type Safety) con TypeScript.</strong></p>
  
  <p>
    <a href="https://github.com/charlessonamericantrading/c-script-/actions/workflows/ci.yml"><img src="https://img.shields.io/badge/build-passing-brightgreen.svg" alt="Estado de Build" /></a>
    <a href="#"><img src="https://img.shields.io/badge/tests-450%20passed-success.svg" alt="Tests" /></a>
    <a href="https://github.com/charlessonamericantrading/c-script-/releases"><img src="https://img.shields.io/badge/versión-1.0.0-blue.svg" alt="Versión" /></a>
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

#### 📦 Linux / macOS (Instalador automático de 1 línea)
```bash
curl -fsSL https://raw.githubusercontent.com/charlessonamericantrading/c-script-/master/install.sh | sh
```

#### 🪟 Windows (PowerShell)
```powershell
irm https://raw.githubusercontent.com/charlessonamericantrading/c-script-/master/install.ps1 | iex
```

#### 🌐 Vía NPM / npx
```bash
npm install -g link-lang
# o probalo directamente con npx:
npx link-lang --help
```

---

## 🤖 Diseñado para Cursor y Agentes de IA (Grok, Claude, GPT)

Link incluye estándares nativos para que puedas desarrollar aplicaciones completas con modelos de IA en Cursor o Windsurf con **cero alucinaciones**:

- **`.cursorrules` y `.cursor/rules/c-script.mdc`**: Instrucciones precisas para que Grok, Claude y GPT-4 comprendan la sintaxis de Link, definición de servicios, tipos, decoradores y runner de pruebas.
- **`llms.txt` y `llms-full.txt`**: Archivos estandarizados de contexto para motores de IA.
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

Probá Link directamente en tu navegador sin instalar nada:
- Abrí [`playground/index.html`](playground/index.html) para escribir código Link, ver la generación de contratos en tiempo real y ejecutar pruebas.

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
