# Guía de Contribución para c-script (Link)

¡Gracias por tu interés en contribuir a **c-script**!

## 🛠️ Entorno de Desarrollo Local

El compilador y runtime están implementados en **Rust (edición 2021)**:

1. **Clonar el repositorio**:
   ```bash
   git clone https://github.com/charlessonamericantrading/c-script-.git
   cd c-script-/compiler
   ```

2. **Ejecutar la suite completa de pruebas**:
   ```bash
   cargo test
   ```

3. **Compilar el binario en modo Release**:
   ```bash
   cargo build --release
   ```

## 📐 Estructura del Compilador (`compiler/src/`)
- `lexer.rs` & `token.rs`: Tokenización y palabras clave.
- `parser.rs` & `ast.rs`: Parser recursivo descendente y Abstract Syntax Tree.
- `checker.rs` & `types.rs`: Inferencia y síntesis bidireccional de tipos, análisis estático de RBAC.
- `codegen/`: Generadores de TypeScript (`ts_emit.rs`), validadores runtime (`validators_emit.rs`), OpenAPI (`openapi_emit.rs`), Zod (`zod_emit.rs`) y WebAssembly (`wasm_emit.rs`).
- `runtime/`: Servidor HTTP multihilo (`server.rs`), motor SQLite con auto-migraciones (`db.rs`), dialecto PostgreSQL (`postgres.rs`) y sesiones (`session.rs`).
- `lsp.rs`: Servidor Language Server Protocol (JSON-RPC 2.0 stdio).
- `fmt.rs` & `lint.rs`: Formateador canónico y linter estático.

## 🧪 Añadir Pruebas
Cualquier cambio de sintaxis o tipo debe acompañarse de:
- Un test unitario en el módulo correspondiente.
- Un test de contrato en `compiler/tests/` o actualización de snapshots con `linkc test <file.link> <file.snap> --update`.

## 📜 Licencia
Al contribuir, aceptas que tus contribuciones se licencien bajo la licencia **MIT** del proyecto.
