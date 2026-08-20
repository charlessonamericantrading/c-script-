# Changelog

Todos los cambios notables en este proyecto serán documentados en este archivo.
El formato está basado en [Keep a Changelog](https://keepachangelog.com/es-ES/1.0.0/), y este proyecto adhiere a [Semantic Versioning](https://semver.org/lang/es/).

## [Sin publicar]

### 🔐 Seguridad
- **`crypto.hashPassword` ahora es Argon2id** (RFC 9106) con sal aleatoria por contraseña y salida en formato PHC. Antes era un solo SHA-256 sobre la constante `"link_salt_2026"` — la misma sal para toda aplicación escrita en el lenguaje, sin iteraciones: dos usuarios con la misma contraseña compartían hash y una sola rainbow table las rompía todas.
- **`crypto.verifyPassword` compara en tiempo constante.** La comparación anterior (`==` de `String`) cortaba en el primer byte distinto y filtraba, por tiempo de respuesta, cuánto del hash había acertado quien probaba. Sigue aceptando los hashes del formato viejo para no dejar afuera a los usuarios ya registrados de una app en producción.
- **`crypto.randomToken` y `crypto.uuid` salen del CSPRNG del sistema.** Antes derivaban de `SystemTime::now().as_nanos()`: eran adivinables para quien pudiera acotar el instante de emisión, y dos llamadas dentro del mismo nanosegundo devolvían el mismo valor.
- **Los tokens de sesión piden entropía directo al SO** (`getrandom`), reemplazando el rodeo del hilo descartable sobre `RandomState` que documenta GRAMMAR.md §3.14.
- Detalle completo, con los límites que quedan (parámetros de Argon2id no configurables desde el lenguaje, sin señal de re-hash, el hashing bloquea el hilo del servidor ~15 ms): GRAMMAR.md §3.34.

### 📚 Documentación
- **Los ejemplos de la documentación ahora los compila el compilador.** `compiler/tests/docs_examples.rs` toma cada bloque de código c-script publicado en README, `llms.txt`, `llms-full.txt`, `AGENTS.md`, las reglas de Cursor/Copilot y `docs/`, lo compila con el binario real y, si declara un `test "..."`, lo ejecuta. Casi ninguno compilaba: el ejemplo insignia del README usaba `role: Role.Member` sin llaves, y `llms.txt` enseñaba closures con tipo de retorno y lectura de campos sobre un `T?`.
- **Nuevo `AGENTS.md`**: mapa del repo, comandos reales, convenciones y la lista de lo que está roto a sabiendas, para Claude Code / Codex.
- **Sección "Estado" en ambos README**: qué funciona y qué no, con el motivo técnico de cada límite.
- **Índice navegable en GRAMMAR.md** (190 KB, 44 secciones) con anclas compatibles con GitHub.
- Correcciones de honestidad: badge de CI real en vez de una imagen estática que decía "passing" con el CI en rojo, número de tests real (453), el playground etiquetado como la maqueta que es, `npm install -g link-lang` marcado como no publicado, y el copyright del README alineado con LICENSE (decía "Google DeepMind").

## [1.0.0] - 2026-08-15

### 🚀 Novedades y Características Principales
- **Sistema de Tipos Bidireccional Completo**:
  - Inferencia y síntesis bidireccional (⇒ / ⇐).
  - Subtipado estructural para structs y nominal para enums.
  - Tipos Unión (`A | B`) con value-flow subtyping y narrowing exhaustivo en `match`.
  - Genéricos definidos por el usuario con monomorfización estricta.
  - Funciones de primera clase y closures léxicos reales (`|params| { ... }`) con subtyping contravariante en parámetros y covariante en retornos.
  - Tipos `Timestamp` (con precisión de milisegundos en UTC ISO-8601) e `Int64` (mismo rango 64-bit que no pierde precisión en TS).
  - Builtin `now() -> Timestamp` para instanciar fechas del sistema en tiempo de ejecución.
  - Builtins de colección y listas: `.map()`, `.filter()`, `.length()`, `.contains()`, etc.

- **Persistencia SQLite Automática (`db { ... }`)**:
  - Declaración embebida que genera esquemas SQLite en disco automáticamente sin necesidad de migraciones manuales en v1.0.
  - Métodos CRUD nativos fuertemente tipados: `.all()`, `.find(id)`, `.insert(row)`, `.applyPatch(id, patch)`, `.delete(id)`, `.findWhere(fn)`, `.deleteWhere(fn)`.
  - Aislamiento en memoria (`:memory:`) automático para tests.

- **Streaming en Tiempo Real y Push SSE (`stream`)**:
  - Soporte para Server-Sent Events nativo (`Transfer-Encoding: chunked` con flush por evento).
  - Push reactivo con pub-sub interno mediante `while true { db.<col>.subscribe() }` reconociendo mutaciones en vivo (`insert`, `delete`, `applyPatch`).
  - Consumo en cliente TypeScript como `AsyncIterable<T>`.

- **Autenticación de Primera Clase (`auth`)**:
  - Decoradores declarativos `@authenticated` y `@requires(Role.<Variant>)`.
  - Almacén de sesiones opacas de 128 bits seguras sin dependencias externas (`auth.createSession(role)`, `auth.destroySession()`).
  - Validación de cabeceras `Authorization: Bearer <token>` tanto en HTTP como en preflights CORS OPTIONS.

- **Runner de Pruebas Integrado (`test`)**:
  - Bloques de prueba de comportamiento de primer nivel `test "nombre" { ... }`.
  - Builtins `assert(cond, [msg])` y `panic(msg)`.
  - Invocación directa de servicios `Service.rpc(...)`.
  - CLI `linkc test <archivo.link>` con aislamiento de DB por prueba y códigos de salida estándar.
  - Tests de contratos contra snapshots con diff LCS línea a línea (`linkc test <file.link> <file.snap> [--update]`).

- **Generador de TypeScript y SDK Cliente (`linkc build`)**:
  - Generación simétrica de `contract.d.ts`, `client.ts` y validadores en tiempo de ejecución `validators.ts`.
  - Detección y rechazo de respuestas malformadas en el cliente (`LinkValidationError`).
  - Cliente tipado con soporte de streams asíncronos y gestión de tokens (`.setToken()`).

- **Herramientas de Lenguaje y LSP 2.0**:
  - Servidor LSP oficial (`linkc lsp`) con JSON-RPC 2.0 stdio: diagnósticos con spans multilínea UTF-16, autocompletado sensible al tipo del receptor (`x.`), hover de expresiones intermedias y goto-definition entre archivos.
  - Gestor de paquetes con lockfile SHA-256 (`link.lock`) y resolución de dependencias Git directas (`git+https://...#rev`).
  - Hot reload interactivo con `linkc dev <file> <outDir> [port]`.
  - Diagnósticos amigables con sugerencias *"Did you mean?"* para errores de tipeo en variables, campos y tipos.
  - Extensión oficial para Visual Studio Code (`c-script-vscode-1.0.0.vsix`).

---

[1.0.0]: https://github.com/charlessonamericantrading/c-script-/releases/tag/v1.0.0
