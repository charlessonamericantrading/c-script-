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

Completo (Fase 2, prerrequisito 2 de 3 para un LSP): el parser ahora se recupera de un error de sintaxis en vez de abortar en el primero -- reporta cada error independiente que encuentra en una sola pasada (a granularidad de ítem de nivel superior: un `service`/`type`/`fn`/etc. roto no frena que se sigan chequeando los demás, aunque se descarta entero en vez de salvar los miembros bien formados de adentro). Encontrado durante la revisión de diseño: una versión anterior del paso de recuperación avanzaba un token incondicionalmente antes de resincronizar, lo que se comía en silencio el error del próximo ítem real cada vez que un error de sintaxis pasaba anidado dentro de algo (el caso común -- una llave de cierre faltante); arreglado chequeando antes de avanzar, no después.

Completo (Fase 2, prerrequisito 3 de 3 para un LSP): cada nodo `Expr`/`Stmt` del AST ahora carga su propia posición real (`Spanned<T>`, la más precisa -- y más cara -- de tres granularidades consideradas), y el type checker la usa de verdad: un error de TIPOS (un operando que no matchea, un campo de struct faltante, un rpc cuya firma no puede cruzar la red, ...) ahora se renderiza con el mismo snippet+caret estilo gcc/rustc que los errores de sintaxis ya tenían desde el prerrequisito 1, no solo un mensaje pelado. Se hizo en dos rondas: primero una migración puramente mecánica (todos los tests existentes siguieron pasando sin ningún cambio de comportamiento -- la señal de que el refactor de ~155 sitios entre parser/checker/runtime/codegen no alteró nada), después el checker mismo estampando y renderizando esas posiciones. `Span` todavía no tiene identidad de archivo, así que un error de tipos dentro de un archivo IMPORTADO de un programa multi-archivo cae al texto plano de siempre en vez de arriesgarse a renderizar un snippet plausible pero del archivo equivocado -- la proveniencia real por archivo queda como trabajo de seguimiento, no resuelto acá. El protocolo del LSP en sí (JSON-RPC sobre stdio, `textDocument/didOpen`, `publishDiagnostics`, autocompletado, hover) es una ronda aparte, más grande, que todavía no arrancó. 269 tests, todos pasando.

Completo (Fase 2): un constructo de loop `while` (`Stmt`, nunca `Expr`; sin `for`/`break`/`continue`; con una cota dura de iteraciones porque el servidor, single-threaded y sin timeout, se colgaría para todos los clientes con un solo loop infinito) y, construido encima, push real para `stream`: un cuerpo que es exactamente `while true { db.<coleccion>.subscribe() }` se reconoce como UN ÚNICO shape sintáctico fijo en tiempo de compilación -- elegido en vez de construir un mecanismo general de corutinas/`yield` para lógica arbitraria por evento -- y se intercepta antes de que el intérprete lo llegue a correr. Un registro de pub-sub sobre `Db` (canal acotado, publish no bloqueante, poda lazy de suscriptores desconectados) entrega una foto inicial seguida de eventos en vivo de verdad, sobre el mismo wire format SSE de la ronda de streaming anterior, sin ningún cambio de codegen del cliente (el cliente generado ya leía de forma indefinida). Solo colección entera en v0 -- sin `subscribe(id)` por fila, sin filtrado/transformación de eventos dentro del cuerpo del stream (el cliente ya puede filtrar por id gratis). Verificado de punta a punta con el cliente generado real: entrega de la foto inicial, un evento en vivo llegando por una conexión ya abierta tras un insert separado, y un suscriptor desconectado podado de forma lazy en la próxima escritura sin crashear ni colgar el servidor. Ver [GRAMMAR.md](GRAMMAR.md) §3.15 (`while`) y §3.16 (pub-sub) para el diseño completo, el argumento de concurrencia de por qué no hizo falta ningún lock nuevo, y qué queda explícitamente afuera.

Pendiente (ver PLAN.md §4): el protocolo del LSP en sí (ver arriba -- los tres prerrequisitos ya están, pero el servidor todavía no arrancó). También sigue faltando: un backend de codegen que emita `.wasm` de verdad (el target WASM de hoy recompila el intérprete tree-walking en vez de generar instrucciones wasm nativas), y una DB real con SQL detrás. Ver [GRAMMAR.md](GRAMMAR.md) §2.1 (imports/package manager) y §3.12 (`db`) para el detalle exacto de cada uno y por qué.

## Licencia

MIT — ver [LICENSE](LICENSE).
