# Arquitectura Interna del Compilador **c-script** (`linkc`)

El compilador de `c-script` está desarrollado en **Rust** sin dependencias complejas de terceros, priorizando la velocidad de compilación, la solidez del sistema de tipos y la simplicidad operacional.

---

## Pipeline del Compilador

```text
código .link
   │
   ▼
[1] Lexer ───────────────► Vec<Token> (Spans precisos de línea/columna)
   │
   ▼
[2] Parser Resiliente ──► AST (Spanned<Expr>, Spanned<Stmt>, Program)
   │                       └─► Acumulación de ParseErrors
   │
   ▼
[3] Type Checker ────────► AST Tipado & Inferencia Bidireccional
   │                       └─► Mapeo estructural/nominal & monomorfización
   │
   ├─────────────────────► [4a] Emisor de Contratos TS (contract.d.ts, client.ts, validators.ts)
   ├─────────────────────► [4b] Emisor WASM Nativo (wasm-encoder -> bytecode .wasm)
   ├─────────────────────► [4c] Intérprete Tree-Walking (Runtime SQLite + SSE Streaming)
   └─────────────────────► [4d] Servidor LSP (stdio JSON-RPC 2.0)
```

---

## Componentes Principales

### 1. Sistema de Diagnósticos y Recuperación de Errores
- **Spans**: Cada token y nodo del AST almacena su ubicación exacta (`Span { line, col, start, end }`).
- **Parser Resiliente**: No aborta al encontrar el primer error; resincroniza a nivel de ítem (`type`, `service`, `fn`) reportando múltiples errores en una sola pasada.

### 2. Persistencia en SQLite (`rusqlite`)
- El esquema SQL de las colecciones `db { ... }` se **deriva automáticamente** del tipo `Type::Struct` declarado en el código c-script.
- Mapeo nativo de primitivos (`Int` -> `INTEGER`, `String` -> `TEXT`, `Bool` -> `INTEGER`) y serialización JSON transparente para estructuras anidadas o tipos complejos.

### 3. Emisión de Contratos TypeScript
- **`contract.d.ts`**: Mapeo 1:1 de los tipos del backend a definiciones TypeScript exportables.
- **`client.ts`**: Cliente RPC en TypeScript sobre `fetch` / `AsyncIterable` (SSE streaming).
- **`validators.ts`**: Validaciones en tiempo de ejecución para asegurar la integridad de los datos antes de ser consumidos por el frontend.

### 4. Servidor Language Server Protocol (LSP)
- Módulo [`compiler/src/lsp.rs`](../compiler/src/lsp.rs) expuesto vía `linkc lsp`.
- Protocolo JSON-RPC 2.0 sobre `stdio` con soporte para diagnósticos, autocompletado, hover e ir a la definición.
