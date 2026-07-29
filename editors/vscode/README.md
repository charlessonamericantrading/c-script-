# c-script VS Code Extension

Extensión oficial para **c-script**, el lenguaje backend compilado con **End-to-End Type Safety nativa a TypeScript**.

## Características

- **Resaltado de sintaxis**: Coloreado sintáctico completo para tipos, `service`, `rpc`, `stream`, `db`, `enum` ADT y decoradores `@authenticated` / `@requires`.
- **Servidor LSP Integrado (`linkc lsp`)**:
  - Diagnósticos y reporte de errores sintácticos y de tipos en tiempo real.
  - Tooltips al pasar el cursor (`hover`).
  - Autocompletado inteligente de palabras clave, métodos de colecciones `db` y tipos (`completion`).
  - Ir a la definición de símbolos (`definition`).

## Instalación Local

1. Asegúrate de tener instalado el compilador `linkc` en tu sistema.
2. Abre la carpeta `editors/vscode` en tu terminal e instala las dependencias:
   ```bash
   cd editors/vscode
   npm install
   npm run compile
   ```
3. Para empaquetar la extensión en un archivo `.vsix`:
   ```bash
   npx vsce package
   ```
4. Instala el `.vsix` en VS Code desde la pestaña de Extensiones -> **Install from VSIX...**.
