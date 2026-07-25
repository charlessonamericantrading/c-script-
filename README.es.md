*[Read in English](README.md)*

# c-script

Un lenguaje backend compilado cuyo diferenciador es la **type-safety de extremo a extremo con TypeScript**: cambiar un tipo en el backend rompe la compilación (`tsc`) del frontend, en vez de fallar en producción.

Este repo es el **MVP de Fase 0** (ver [PLAN.md](PLAN.md) §4): prueba el killer feature completo, de punta a punta. No es un lenguaje de producción — es la prueba de que la idea funciona.

## Qué hay acá

| | |
|---|---|
| [`PLAN.md`](PLAN.md) | Propuesta, roadmap por fases, análisis de riesgos |
| [`GRAMMAR.md`](GRAMMAR.md) | Especificación formal: EBNF, sistema de tipos, tabla de mapeo a TypeScript |
| [`compiler/`](compiler/) | El compilador (`linkc`), en Rust, sin dependencias externas salvo `tiny_http`/`serde_json` para el runtime de la demo |
| [`examples/users.link`](examples/users.link) | El programa de ejemplo: un CRUD de usuarios |
| [`frontend/`](frontend/) | Un frontend TypeScript real que consume el contrato generado |
| [`gen/`](gen/) | Salida de `linkc build` — `contract.d.ts` + `client.ts` + `validators.ts` (generado, no editar a mano) |

## Probar el killer feature vos mismo

```bash
cd compiler
cargo build

# 1. Generar el contrato TypeScript desde el backend
./target/debug/linkc build ../examples/users.link ../gen

# 2. Confirmar que el frontend tipa limpio
cd ../frontend && npm install && npx tsc --noEmit   # exit 0

# 3. Levantar el servidor y correr el frontend de verdad
cd ../compiler && ./target/debug/linkc serve ../examples/users.link 8787 &
cd ../frontend && node src/main.ts                  # llama al server real, tipado end-to-end
```

El servidor arranca con la base **vacía** — crea una colección vacía por cada una que tu programa declare en `db { ... }`, y nada más. Por eso la primera corrida del demo crea su propio usuario y después lo lee: que el runtime de un lenguaje invente filas que nunca escribiste sería mentir sobre lo que tu programa hace.

Ahora rompé algo: en `examples/users.link`, renombrá `name` a `fullName` dentro de `type User`. Volvé a correr `linkc build` y `npx tsc --noEmit` **sin tocar `frontend/src/main.ts`**. `tsc` va a fallar en cada línea que usaba `.name` — exactamente el punto ciego que c-script existe para eliminar (ver [PLAN.md](PLAN.md) §3).

Arrancar un proyecto de cero es más rápido: `linkc new mi-app` scaffoldea un `.link` mínimo más un `frontend/` a juego; `linkc dev mi-app/main.link mi-app/gen` lo observa (y a todo lo que importe) y regenera el contrato en cada guardado, en vez de correr `build` a mano cada vez.

## Estado

Completo (Fase 0): lexer, parser, type checker bidireccional (subtipado estructural/nominal, `Result<T,E>` y `Patch<T>` como builtins, operadores aritmético-lógicos, `if/else`, asignación y mutabilidad, arrays, tuplas, conversión numérica explícita, `Map<K,V>`, métodos builtin de `String`), genéricos definidos por el usuario vía monomorfización, uniones de tipo (`A | B`) con subtipado de flujo de valor Y narrowing de vuelta a un miembro concreto vía `match` (patrones `nombre: Tipo`, reusando el mismo `:` que ya significa "tipo declarado" en todos lados, con uniones cuyos miembros no se puedan distinguir en runtime rechazadas en tiempo de compilación en vez de matchear mal en silencio), funciones como valores de primera clase -- referencias con nombre Y closures léxicos reales (`|params| { block }`, con subtipado de funciones contravariante/covariante real) -- más métodos de orden superior sobre `List` (`.map`/`.filter`), exhaustividad de `match` extendida con patrones de literales, or-patterns y guardas, declaraciones `const`, emisor de contrato, runtime interpretado mínimo.

Completo (Fase 1, parcial): CLI `linkc new`/`linkc dev`, imports multi-archivo con un package manager mínimo por rutas locales (sin lockfile, sin registro en red todavía — ver GRAMMAR.md §2.1), y un v0 de target WASM -- el intérprete existente recompilado a `wasm32-wasip1`, probado corriendo una llamada RPC real dentro de `wasmtime` de punta a punta (`compiler/src/bin/wasm_demo.rs`; ver PLAN.md §2.4 para el detalle exacto de qué prueba esto y qué no).

Completo (Fase 2, parcial): validadores runtime (`validators.ts` -- el tercer output del emisor planeado desde el primer borrador de `PLAN.md`; cada respuesta de un rpc se valida contra el contrato declarado antes de que el cliente la devuelva, lanzando `LinkValidationError` si no matchea en vez de devolver silenciosamente un dato malformado), un v0 de `db` tipada (`db { users: User[] }` reemplaza a `Type::Dynamic` -- `all/find/insert/applyPatch` ahora se chequean contra el tipo de elemento real, sigue siendo enteramente en memoria, sin ningún driver SQL), streaming real por SSE para `stream` (framing de verdad -- `Transfer-Encoding: chunked` con flush por evento, no un solo JSON -- para repetir una secuencia ya calculada; el cliente generado la consume como `AsyncIterable<T>` real, validando cada evento), y auth v0 (decorators `@authenticated`/`@requires(Role.Admin)` sobre un `rpc`/`stream`, con sesiones opacas en memoria -- sin JWT, sin ninguna dependencia nueva, verificación de contraseña/credenciales fuera de alcance a propósito; ver [GRAMMAR.md](GRAMMAR.md) §3.14 para la debilidad de generación de tokens que dos revisiones adversariales encontraron, y el fix).

Completo (Fase 2, prerrequisito 1 de 3 para un LSP): los tokens ahora cargan una columna real (no solo línea), se arreglaron dos bugs reales de posición en el lexer (el span de error caía un carácter tarde en `lex_punct`/`lex_string`/`lex_number`, y un string/comentario de bloque sin cerrar mezclaba su línea de apertura con una posición de EOF -- ambos invisibles hasta que existió un renderer real que los expusiera), y un módulo nuevo `diagnostics` renderiza errores de lexer/parser como un snippet + caret estilo gcc/rustc, sin ninguna dependencia nueva. Un error de sintaxis dentro de un archivo IMPORTADO ahora también nombra ese archivo (antes colapsaba a un número de línea pelado, sin decir cuál de varios archivos tenía el problema).

Pendiente (ver PLAN.md §4): un LSP todavía necesita dos cosas más antes de que el protocolo del servidor en sí tenga sentido -- recuperación real en el parser (hoy es estrictamente fail-fast: el primer error de sintaxis aborta todo el parseo, así que corregís un typo solo para toparte con el siguiente de a uno, en vez de verlos todos juntos) y spans enhebrados hasta el AST/checker (un error de TIPOS todavía no tiene ninguna posición, solo los de sintaxis). También sigue faltando: un backend de codegen que emita `.wasm` de verdad (el target WASM de hoy recompila el intérprete tree-walking en vez de generar instrucciones wasm nativas), push real (WebSocket, o SSE de larga duración) para que un `stream` avise de eventos FUTUROS -- hoy solo repite una secuencia ya calculada, suscribirse a cambios necesitaría una capa de pub-sub sobre `db` que no existe, y un generador perezoso de verdad necesitaría además un constructo de loop, que el lenguaje todavía no tiene (la recursión vía una `fn` con nombre o un closure autorreferenciado ya funciona hoy, pero no hay sintaxis `for`/`while`) --, y una DB real con SQL detrás. Ver [GRAMMAR.md](GRAMMAR.md) §2.1 (imports/package manager), §3.12 (`db`) y §3.13 (streaming) para el detalle exacto de cada uno y por qué. 258 tests, todos pasando.

## Licencia

MIT — ver [LICENSE](LICENSE).
