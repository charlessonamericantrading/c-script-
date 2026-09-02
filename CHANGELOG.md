# Changelog

Todos los cambios notables en este proyecto serán documentados en este archivo.
El formato está basado en [Keep a Changelog](https://keepachangelog.com/es-ES/1.0.0/), y este proyecto adhiere a [Semantic Versioning](https://semver.org/lang/es/).

## [1.172.0] - 2026-09-02

### 🔧 Interno
**Red de tests (PLAN.md §9.17 ítem 6): una cerca de código sin lenguaje en la documentación es ahora un error de `docs_examples.rs` -- y las 100 que ya existían quedaron clasificadas contra el binario real.** El test que garantiza "todo ejemplo publicado compila" solo miraba cercas ` ```rust `/` ```link `; una cerca pelada se saltaba en silencio como "otro lenguaje" -- GRAMMAR.md tenía 96, incluidos los ejemplos publicados de `pdf.build`, `excel.build/parse` y `mcp.sample`. Barrido completo: cada bloque pelado se probó compilar con `linkc test` -- 18 ejemplos nuevos entran a la red de CI como `linkc:check` (30→48 en GRAMMAR.md), 69 quedan `linkc:fragment` explícito, el resto tipado `bash`/`text`/`json`. La puerta queda cerrada: cualquier apertura pelada futura falla el test con un mensaje que dice exactamente qué poner. Bonus verificando el caso negativo: la regla nueva atrapó su propia documentación (una línea de prosa de AGENTS.md que empezaba con una cerca literal desincronizaba el parser) -- reescrita en palabras. Ver GRAMMAR.md §3.213.

## [1.171.0] - 2026-09-02

### 🐛 Arreglado
**Seguridad (PLAN.md §9.17 ítem 2, hallazgo MEDIO de la auditoría del 02/09/2026): la entrega de una respuesta correlacionada MCP exige la sesión dueña, y el id de correlación ya no puede degenerar en silencio.** Dos problemas del mismo camino (la Pieza C de §3.203, `mcp.sample`): (1) un `POST /mcp` con `id` y sin `method` entregaba la respuesta al `mcp.sample` bloqueado sin verificar `Mcp-Session-Id` ni nada -- la única barrera era adivinar el id de 128 bits; el propio test de round-trip entregaba con cero headers y pasaba. (2) `fresh_id` hacía `let _ = getrandom(...)`: si la fuente de aleatoriedad fallaba, el id quedaba en ceros predecibles -- la misma clase de bug que el hallazgo histórico de `RandomState` (§3.14). Ahora `fresh_id` corta el proceso si `getrandom` falla (mismo criterio que `session.rs::fresh_128_bits`), la tabla de pendientes guarda el `jti` de la sesión dueña, y la entrega exige un `Mcp-Session-Id` válido cuyo `jti` coincida -- sin header es 401, con una sesión válida pero ajena es el MISMO 404 que un id inexistente (anti-oráculo deliberado). Test nuevo con las tres entregas sobre un mismo sampling pendiente real. Ver GRAMMAR.md §3.212.

## [1.170.0] - 2026-09-02

### 🐛 Arreglado
**Seguridad (PLAN.md §9.17 ítem 1, hallazgo ALTO de la auditoría del 02/09/2026): `--trust-proxy` toma el ÚLTIMO valor de `X-Forwarded-For`, no el primero -- el rate limiter deja de ser evadible detrás de un proxy que appendea.** La semántica de §3.89 (primer elemento, "el más cercano al cliente original") era correcta sobre qué representa cada posición del header pero al revés sobre cuál es confiable: el default de nginx (`proxy_add_x_forwarded_for`) APPENDEA al header que llega, así que todo lo anterior al último elemento lo controla el cliente -- un atacante rotando `X-Forwarded-For: <aleatorio>` por request abría un bucket nuevo de `@rate_limit` cada vez y el límite quedaba inerte (brute-force de login sin freno). Ahora se toma el único elemento que escribió el proxy de confianza propio: con un solo proxy delante (el caso real que motivó el flag) es la IP del cliente; con una cadena de proxies confiables es la IP del proxy externo -- bucket compartido, más restrictivo de la cuenta pero nunca evadible, misma semántica que `trust proxy: 1` de Express. El test de cadena se reescribió con la semántica nueva y se agregó un test que reproduce el ataque exacto (prefijo falsificado rotando con último elemento fijo → 429 en la 4ta request). Ver GRAMMAR.md §3.211.

## [1.169.0] - 2026-09-01

### ✨ Nuevo
**Compatibilidad con IA (PLAN.md §9.16 ítem 6, último del plan): códigos de error estables (`error[L0001]: ...`, mismo formato que `error[E0308]: ...` de `rustc`) + `linkc explain <código>`.** Inspirado directamente por el `E0603` real que apareció en esta misma sesión implementando §3.204/§3.205 -- se resolvió rápido justamente porque `rustc` nombra sus errores con un código estable y documentado. Arranca con 5 códigos curados (`L0001`-`L0005`) sobre los errores que ya tenían su propia explicación extensa en GRAMMAR.md -- NO todo error tiene código, mismo criterio pragmático que `rustc`. `CheckError`/`ParseError` ganan un campo `code: Option<&'static str>` (mismo patrón que `span`/`file`); `--diagnostics-json` (§3.208) y el protocolo LSP (`Diagnostic.code`, un campo real de la spec) también lo exponen. `linkc explain L0001` imprime la explicación completa desde `error_codes::CODES`, la única fuente de verdad. Bug real encontrado verificando el repro de `L0005` antes de darlo por probado: `||` (sin espacio, la forma más común) lexeaba como un token `PipePipe` que el parser no reconocía como intento de closure vacío, así que el mensaje dirigido solo aparecía con `| |` (con espacio) -- cerrado con un brazo nuevo en el dispatch de expresión primaria. Con esto, PLAN.md §9.16 queda completo. Ver GRAMMAR.md §3.210.

## [1.168.0] - 2026-09-01

### ✨ Nuevo
**Compatibilidad con IA (PLAN.md §9.16 ítem 1(b)): una variante de enum sin campos ya no necesita `{}` para usarse como valor -- `Role.Member` funciona directo, igual que ya funcionaba dentro de una anotación o un patrón de `match`.** Cierra la asimetría de raíz que §3.206 solo había diagnosticado mejor sin eliminar. El discovery real: no hacía falta tocar el parser (que siempre produce la misma forma sintáctica, `Ident.Ident`, para este caso, sin importar si `Ident` es una variable o un enum) -- la desambiguación es puramente semántica, resuelta en el checker/runtime reusando la construcción de `StructLit` que ya existía para la forma con llaves, sin duplicar lógica. Una variante CON campos sigue necesitando llaves (no hay de dónde inferir los valores), ahora con un mensaje dirigido en vez del genérico de antes de §3.206; una variante inexistente da una sugerencia por distancia de edición. De paso, cerrado un caso encontrado en la propia verificación: un enum GENÉRICO (`Maybe<T>`) sin llaves en un contexto con tipo esperado (ej. una rama de `if`) no inferia sus argumentos de tipo -- la forma con llaves sí, la nueva forma sin llaves ahora también. `AGENTS.md`/`llms.txt`/`CLAUDE.md` actualizados para dejar de advertir de una regla que ya no aplica al caso común. Ver GRAMMAR.md §3.209.

## [1.167.0] - 2026-09-01

### ✨ Nuevo
**Compatibilidad con IA (PLAN.md §9.16 ítem 5): `--diagnostics-json`, flag global que imprime `[{file, line, column, message}]` a stdout en vez del texto humano de siempre a stderr, para cualquier subcomando que falle al cargar o tipar un programa.** Antes, la única salida era prosa en español con posición inline -- frágil de parsear programáticamente para un agente, un editor, o una integración de CI. `lsp.rs` ya convertía estos mismos errores a JSON para el protocolo LSP, confirmando que la forma es viable; se escribió un emisor propio más simple (línea/columna 1-indexed, sin estado de documento) en vez de reutilizarlo tal cual. Funciona en cualquier posición del argumento y para TODO subcomando que carga/tipa (no solo `build`/`test`/`check`) -- centralizado en las dos funciones de reporte ya compartidas por los 13 sitios de llamada, más barato que threadear un parámetro por cada uno. 3 tests de integración nuevos contra el binario real. Ver GRAMMAR.md §3.208.

## [1.166.0] - 2026-09-01

### 🔧 Interno
**Compatibilidad con IA (PLAN.md §9.16 ítem 3): dos allowlists sobre `Value` con catch-all genérico convertidas a funciones exhaustivas sin `_`, para que agregar una variante `Value` nueva sin clasificarla sea un error de `cargo build`.** No es una hipótesis -- es la clase de bug que ya rompió producción tres veces (v1.162.0, GRAMMAR.md §3.204/v1.163.0, GRAMMAR.md §3.199), siempre con la misma forma: una variante nueva se agrega a `Value`, se olvida en UN sitio con `_ =>`/`other =>`, tipa limpio y rompe en runtime. `impl PartialEq for Value` (el bug exacto de v1.162.0, 14 brazos `(X,X)=>true` a mano) y la eligibilidad de `Expr::FieldAccess` → `Value::BoundMethod` (el bug exacto de §3.199) ahora delegan en `is_marker_singleton`/`supports_bound_method_access`, dos funciones que clasifican las 32 variantes de `Value` sin ningún brazo `_` -- una variante nueva sin clasificar ahí no compila. Comportamiento idéntico antes/después, confirmado contra el binario real. Auditados ~56 sitios similares en total; los otros 4 (los emisores de codegen que ya excluyen `Type::Pdf`/etc.) resultaron YA exhaustivos sin `_`, nada que arreglar; el resto queda documentado como fuera de alcance de esta ronda (mayormente heurísticas genuinamente abiertas, no enums cerrados mal clasificados). Ver GRAMMAR.md §3.207.

**Nota de proceso**: v1.165.0 quedó con CI en rojo -- el snapshot de `examples/users.link.snap` no se regeneró antes de ese commit, así que seguía con el stamp de v1.164.0 mientras el binario ya se identificaba como v1.165.0, y el chequeo de deriva de contrato (GRAMMAR.md §3.29) lo detectó correctamente. Sin bug de código de producción -- mismo tipo de proceso ya documentado antes (v1.131.0 corrigiendo v1.130.0). Este mismo commit (v1.166.0) ya regenera el snapshot correctamente y su propia CI está verde.

## [1.165.0] - 2026-09-01

### 📝 Documentación
**Compatibilidad con IA (PLAN.md §9.16 ítem 4): `linkc test <archivo>` (sin segundo argumento) ya era el camino rápido de "solo parsear y tipar" cuando el programa no tiene ningún bloque `test { }` -- no estaba documentado así en ningún lado.** Verificado leyendo `run_tests_core`: la conexión SQLite (`Db::new(":memory:")`) vive dentro del loop por-test, así que con cero tests ese loop nunca ejecuta -- ningún archivo escrito, ninguna base tocada. Confirmado contra el binario real: 37ms, `"running 0 tests"`. No hacía falta un subcomando nuevo -- `--help`, `AGENTS.md` y `llms.txt` ahora lo nombran explícitamente como la alternativa barata a `linkc build` cuando la única pregunta es "¿esto tipa?". Ver GRAMMAR.md §3.206.

## [1.164.0] - 2026-09-01

### 🐛 Arreglado
**Compatibilidad con IA (PLAN.md §9.16): dos de los "3 errores que rompen casi todo primer intento" ya documentados en `AGENTS.md`/`llms.txt` daban un mensaje engañoso o directamente ininteligible.** `role: Role.Admin` (una variante de enum usada como valor sin `{}`) daba `"variable no declarada: 'Role'"` -- engañoso, no solo impreciso, porque `Role` SÍ está declarado (como `enum`); ningún agente leyendo ese mensaje llegaría al arreglo real (`Role.Admin {}`) a partir de él. `|u: User| -> Bool { ... }` (un closure con tipo de retorno anotado, que este lenguaje infiere siempre, nunca se anota) daba `"se esperaba LBrace, se encontró Arrow"` -- nombres de variante del `TokenKind` interno del lexer, filtrados sin traducir. Los dos ahora nombran el problema real y muestran la forma correcta, sin cambio de gramática (`Role.Admin` sin llaves sigue sin ser una expresión válida -- solo el diagnóstico mejora). 3 tests de regresión nuevos, incluida una guarda de no-regresión para una variable local que sombree el nombre de un enum. Ver GRAMMAR.md §3.206.

## [1.163.0] - 2026-09-01

### 🐛 Arreglado
**Auditoría del lenguaje: `PdfBlock`/`ExcelCell`/`ExcelSheet` (§3.201/§3.202) rompían `contract.d.ts`/`openapi.json`/`schemas.ts` cuando un `rpc` los usaba como tipo de parámetro/retorno.** Los tres son ADTs reservados por el compilador, pre-registrados directo en `checker.enums`/`checker.types` -- nunca aparecen en `program.items`. `ts_emit.rs`/`openapi_emit.rs`/`zod_emit.rs` descubren qué tipos declarar iterando `program.items`, así que nunca los veían: `contract.d.ts` referenciaba `PdfBlock` sin declararlo (`Cannot find name 'PdfBlock'` en `tsc` real), `openapi.json` tenía un `$ref` colgante con `components.schemas` vacío, y `schemas.ts` salía completamente vacío para cualquier programa que usara `pdf`/`excel`. Los tres confirmados a mano contra el binario real antes del fix. Se declaran ahora incondicionalmente en los tres emisores, mismo criterio que `Result<T,E>`/`Patch<T>`, reusando las mismas funciones constructoras que el checker ya usa para pre-registrarlos (nunca pueden divergir). 4 tests de regresión nuevos. Ver GRAMMAR.md §3.204.

**Auditoría del lenguaje: colisión silenciosa de nombres de tool MCP.** `tools/list`/`tools/call` (§3.203) aplanan `(service, rpc)` a `"{service}_{rpc}"` -- un espacio de nombres plano sin separador real, así que dos pares distintos con guiones bajos propios pueden generar el mismo string (`service A_B { rpc c() }` y `service A { rpc B_c() }` ambos dan `"A_B_c"`). `resolve_tool_name` hacía un primer-match lineal, así que una colisión enrutaba `tools/call` SILENCIOSAMENTE al primer `rpc` en orden de declaración -- riesgo real si el `rpc` "robado" tiene un `@requires` distinto del que el nombre del tool sugería. `linkc serve --mcp-jwt-secret`/`linkc serve-all` ahora rechazan arrancar, nombrando los dos `service.rpc` en colisión, antes de abrir el puerto. 2 tests de integración nuevos contra el binario real. Ver GRAMMAR.md §3.205.

## [1.162.0] - 2026-09-01

### 🐛 Arreglado
**Auditoría del lenguaje: `pdf == pdf` (y `excel`/`mcp`/`env`/`request`/`smtp`/`response`) daba `false` -- debería dar `true`, igual que `math == math`.** Estos identificadores son marcadores internos singleton (los mismos módulos `pdf`/`excel`/`mcp`/etc. de GRAMMAR.md §3.201-3.203, más `env`/`request`/`smtp`/`response` de §3.38/3.43/3.46). El checker tipa `X == X` como válido para cualquier tipo comparado consigo mismo (GRAMMAR.md, regla de `==`/`!=`), pero `impl PartialEq for Value` (`runtime/mod.rs`) nunca había extendido a estas 7 variantes el mismo grupo "marcador interno singleton -> siempre igual a sí mismo" que ya cubre a `Db`/`Auth`/`Math`/`Crypto`/`Http`/`Json`/`Base64` -- caían en el `_ => false` final. Mismo patrón recurrente de este proyecto (un sitio nuevo se agrega a 4 lugares pero se olvida un 5º) -- encontrado con una auditoría dedicada, no en producción: reproducido primero contra el binario real (`linkc test`) antes de tocar el código, con test de regresión nuevo (`module_marker_singletons_compare_equal_to_themselves`, `runtime/mod.rs`). Impacto real bajo (nadie escribe `pdf == pdf` a propósito) pero es una inconsistencia silenciosa alcanzable desde código de usuario válido, sin ningún error o warning de por medio.

## [1.161.0] - 2026-08-31

### ✨ Nuevo
**MCP real -- Pieza C: `mcp.sample(prompt: String) -> String` + streaming bidireccional** (PLAN.md §9.15 ítem 3, cierra la ronda posterior a MyFinance) -- `GET /mcp` abre una conexión SSE de larga duración (mismo patrón que `write_live_stream`, push real §3.16), y `mcp.sample` arma una request `sampling/createMessage` real, la empuja por esa conexión, y bloquea (30s de timeout) hasta que una respuesta correlacionada llega por un `POST /mcp` nuevo y separado, en otro hilo -- la coordinación cross-hilo que un spike aislado con `tiny_http` real validó antes de tocar producción, integrada de verdad vía un `thread_local!` (`CURRENT_MCP`, mismo mecanismo que `CURRENT_REQUEST` de `db.rs`).

**Bug real encontrado y corregido en el camino**: `tools/call` envolvía un resultado `String` con comillas JSON de más dentro del bloque de texto (`"hola"` en vez de `hola`) -- un cliente MCP real le mostraría las comillas literales.

**Verificado end-to-end contra el binario real**: un driver de test con dos conexiones reales (`GET /mcp` bloqueada leyendo eventos SSE + `POST /mcp` de `tools/call` en un hilo aparte) confirma el round-trip completo; un segundo test confirma el timeout real (30s, sin quedar colgado); un tercero confirma el error limpio sin conexión abierta. Suite completa sin regresiones. Con esto se cierra PLAN.md §9.15 completo (PDF v1.157.0, Excel v1.158.0, MCP v1.159.0-v1.161.0). Ver GRAMMAR.md §3.203.

## [1.160.0] - 2026-08-31

### ✨ Nuevo
**MCP real -- Pieza B: `tools/list` y `tools/call`** (PLAN.md §9.15 ítem 3) -- `role_for`/`user_id_for` (`session.rs`) se extendieron para reconocer un `Mcp-Session-Id` como una TERCERA fuente de identidad (junto a sesión interna y JWT externo), así `check_auth_gate`/`handle_rpc` (`runtime/server.rs`) se reusan tal cual para `tools/call`, sin ningún camino de auth paralelo -- un `@requires(Role.Admin)` que ya protege un `rpc` por REST aplica idéntico vía MCP, confirmado con un test dedicado. `tools/list` reusa `type_to_json_schema` (`codegen/openapi_emit.rs`, ahora `pub(crate)`), mismo mapeo Type->JSON Schema que `openapi.json` ya usa.

**Verificado**: 5 tests de integración nuevos contra el binario real (`cli_mcp.rs`). Suite completa sin regresiones. Ver GRAMMAR.md §3.203.

## [1.159.0] - 2026-08-31

### ✨ Nuevo
**MCP real -- Pieza A: sesión (`initialize`/`DELETE`)** (PLAN.md §9.15 ítem 3, primer ítem de MCP en la ronda posterior a MyFinance) -- `linkc serve --mcp-jwt-secret` habilita `/mcp`, sin ninguna anotación `.link` nueva (mismo criterio "sin opt-in" que `openapi.json` ya usa para exponer cada `service`). `POST /mcp` con `method: "initialize"` exige un `Authorization: Bearer` normal y firma una sesión MCP nueva embebiendo el mismo rol/`user_id` -- primera función de FIRMA de JWT en producción de este proyecto (antes solo se verificaban JWT externos, §3.64). `DELETE /mcp` revoca vía un registro chico de `jti` (`mcp_revoked_jti`).

**Verificado**: 7 tests de integración nuevos contra el binario real (`cli_mcp.rs`). Suite completa sin regresiones. Ver GRAMMAR.md §3.203.

## [1.158.0] - 2026-08-31

### ✨ Nuevo
**`excel.build(sheets: ExcelSheet[]) -> String` / `excel.parse(base64: String) -> ExcelSheet[]` -- generación y parsing real de `.xlsx`** (PLAN.md §9.15 ítem 2, segundo ítem de la ronda posterior a MyFinance) -- a diferencia de PDF, acá hacían falta las dos direcciones: MYF genera exports en `.xlsx` real y también parsea extractos bancarios para conciliar.

Dos crates nuevas, no una: `rust_xlsxwriter` (escritura) + `calamine` (lectura) -- quinta y sexta excepción conjunta a "cero dependencias nuevas", comparten `zip` como dependencia transitiva. `ExcelCell` (`Text`/`Number`/`Date`/`Bool`/`Empty`) es un ADT reservado por el compilador, mismo mecanismo que `PdfBlock` (§3.201) -- `Number` carga `Decimal`, no `Float`, coherente con que este lenguaje ya trata `Decimal` como el tipo de dinero. `ExcelSheet`, en cambio, es un struct y NO necesita reservarse por nombre -- este lenguaje subtipa structs estructuralmente (a diferencia de los enums, nominales), así que cualquier `type` de usuario con la misma forma tipa igual de bien.

**Bug real encontrado por un test de round-trip antes de shippear, no en producción**: `write_datetime` sin un `Format` con `set_num_format(...)` explícito escribe una fecha indistinguible de un número común al leerla de vuelta -- ni Excel real ni `calamine` la reconocen. Fix aplicado antes de cerrar el ítem.

**Verificado end-to-end contra un `linkc serve` real con `openpyxl`** (Python, implementación completamente independiente): fechas como `datetime` real, montos exactos, texto con acentos en UTF-8 nativo perfecto -- sin el límite de WinAnsiEncoding que tuvo `pdf.build`.

**Verificado además**: tests de checker (aridad/tipo, ADT, colisión de nombre reservado, y un test que confirma la subtipificación ESTRUCTURAL de `ExcelSheet` contra un `type` de usuario con otro nombre) + tests de runtime (firma ZIP `PK\x03\x04`, round-trip exacto de las 5 variantes de celda -- Decimal y fecha vuelven exactos, fila con columnas inconsistentes rechazada limpio, bytes que no son un `.xlsx` real dan un error limpio). Suite completa sin regresiones. Ver GRAMMAR.md §3.202.

## [1.157.0] - 2026-08-31

### ✨ Nuevo
**`pdf.build(blocks: PdfBlock[]) -> String` -- generación real de PDF** (PLAN.md §9.15 ítem 1, primer ítem de la ronda posterior a la de MyFinance) -- `c-script` no tenía ningún primitivo para producir bytes de PDF, solo podía adjuntar a un email un blob YA generado. `PdfBlock` (`Text { content, bold, size }` / `Table { headers, rows }`) es un ADT reservado por el compilador, no declarado por el usuario -- se pre-registra y reusa el mismo mecanismo genérico de ADT que ya tipa `ValidationError`/`Result<T,E>`, sin infraestructura nueva.

Cuarta excepción real a "cero dependencias nuevas": `pdf-writer` (ecosistema Typst) -- se aparta, con evidencia real, de los dos candidatos originalmente evaluados (`printpdf`: arrastra el framework GUI Azul completo, ~717K SLoC; `genpdf`: abandonada desde 2021). Alcance v1: página A4 fija, márgenes fijos, Helvetica sin embeber, paginación automática vertical, columnas de tabla de igual ancho (celdas truncadas, sin wrap). `contentBase64` de `smtp.sendMessage` (§3.141) recibe el resultado directo, sin fricción.

**Verificado end-to-end contra un `linkc serve` real, no solo tests**: una factura de prueba generada, decodificada a un `.pdf` real en disco y abierta con `pdftotext` (poppler). Los acentos en español ("José Núñez Peña", "Consultoría") se escriben y extraen perfectos. Se encontró y documentó un límite real en el camino: el símbolo € se escribe correcto según el estándar PDF (WinAnsiEncoding, byte `0x80`) pero `pdftotext` no lo extrae -- queda como best-effort para extracción de texto plano, no garantizado, documentado honestamente en vez de asumido.

**Verificado además**: tests de checker (aridad/tipo, ADT, colisión de nombre reservado) + tests de runtime (firma `%PDF-`, acentos/€ no rompen la generación, paginación real con 80 líneas forzando una segunda página, fila de tabla con columnas inconsistentes rechazada limpio). Suite completa sin regresiones. Ver GRAMMAR.md §3.201.

## [1.156.0] - 2026-08-31

### ✨ Nuevo
**`List<T>`: concatenación vía `+` y `.contains()`** (PLAN.md §9.14 ítem 2, última pieza de la ronda MyFinance) -- `List<T> + List<T> -> List<T>` (mismo `T`, sin mezclar) combinado con `let mut`/reasignación (ya existente) resuelve "acumular una lista creciendo en un loop" sin ningún constructo de mutación nuevo. Más `.contains(item: T) -> Bool`, acotado a `Int`/`Int64`/`Float`/`String`/`Bool`/`Uuid`/`Timestamp` (Decimal, Struct/Variant quedan afuera a propósito -- ver GRAMMAR.md §3.200 para el motivo de cada exclusión). Desbloquea un caso real reportado por un adoptador en producción (MyFinance): marcar facturas ya conciliadas durante conciliación bancaria, para no cruzar el mismo movimiento contra dos facturas del mismo importe exacto.

De paso, completa las completions del LSP para `List<T>` (le faltaban `join()`/`reverse()`, ya existentes, desde antes de esta ronda).

**Verificado**: tests de checker (concatenación, mezcla de tipos y `List + escalar` rechazados, `.contains()` tipa sobre `List<Int>` y rechaza sobre `List<Struct>`/`List<Function>`) + tests de runtime (concatenación preserva orden, un `while` real acumulando una lista creciente, `.contains()` con elemento presente/ausente/lista vacía, y el caso real de dedup de conciliación bancaria reproducido con datos de prueba). Suite completa sin regresiones. Ver GRAMMAR.md §3.200.

Con esto se cierra la ronda de 5 piezas de PLAN.md §9.14 (v1.151.0-v1.156.0): los 4 gaps reales reportados por MyFinance, más el bug crítico de `Decimal == Decimal` encontrado en el camino.

## [1.155.0] - 2026-08-31

### 🐛 Arreglado
**Bug crítico: `Decimal.toFloat()`/`.toString()` inalcanzables en runtime** -- tipaban limpio en el checker y su dispatch en `runtime/mod.rs` estaba correctamente escrito, pero `Expr::FieldAccess` (el paso previo que decide si `x.metodo` puede diferirse a `Value::BoundMethod`) tenía su propio allowlist de tipos elegibles, y a `Value::Decimal` le faltaba estar ahí -- así que cualquier método sobre un `Decimal` fallaba antes de llegar al dispatch real, con el mismo error que un campo inexistente. Reportado por la sesión `fix-myf-audit-findings` (adoptador MyFinance) inmediatamente tras actualizar a v1.152.0 para verificar el fix de `Decimal == Decimal` (v1.151.0) -- era un bug distinto y nuevo, no el mismo. Reproducido en vivo contra un `linkc serve` real antes y después del fix, confirmando el mismo texto de error desaparece.

**Verificado**: 1 test de runtime nuevo (`invoke_rpc`) cubriendo `.toFloat()`/`.toString()` sobre un `Decimal` recibido como parámetro y `.toFloat()` encadenado directo sobre `.toDecimal()`. Suite completa sin regresiones. Ver GRAMMAR.md §3.199.

## [1.154.0] - 2026-08-31

### ✨ Nuevo
**Métodos de `String`: `.substring()`, `.replace()`, `.split()`, `.padStart()`/`.padEnd()`** (PLAN.md §9.14 ítem 1) -- superficie que faltaba desbloqueaba en producción dos exports contables reales de un adoptador (MyFinance): formato A3 Contable (fixed-width, necesita `.padStart()`) y ContaPlus/XDIARIO (necesita `.replace()` para sanear `;`/saltos de línea antes de unir con `;`).

- `.substring(start, end)` indexa por CARACTER, no por byte -- consistente con `.length()`. Rango inválido rechazado limpio antes de tocar el string.
- `.replace()` reemplaza todas las ocurrencias.
- `.split(separator) -> String[]`, separador vacío con el comportamiento nativo de Rust (definido, testeado).
- `.padStart()`/`.padEnd()` rellenan con `pad` repetido y truncado a la medida exacta, sin acortar un valor que ya cumple. `length` acotado a 1.000.000 de caracteres.

De paso, completa las completions del LSP para `String` (tenía solo 2 de los 8 métodos que ya existían antes de esta ronda).

**Verificado**: tests de checker + tests de runtime (un caso no-ASCII real confirmando indexado por caracter, los tres casos de rango inválido, reemplazo de todas las ocurrencias, separador vacío, padding sin truncar/con pad multi-caracter/con `pad` vacío rechazado cuando hace falta/con `length` negativo o gigante rechazado, y los dos casos reales de MyFinance reproducidos end-to-end). Suite completa (1151 tests) sin regresiones. Ver GRAMMAR.md §3.198.

## [1.153.0] - 2026-08-31

### ✨ Nuevo
**`auth.claim(name: String) -> String?`** (PLAN.md §9.14 ítem 3) -- accessor genérico de un claim JWT por nombre. Hasta ahora solo `--jwt-role-claim`/`--jwt-user-id-claim` (slots fijos configurados una vez al arrancar) eran accesibles. `auth.claim` lee CUALQUIER claim, nombrado en cada llamada, del mismo mapa de claims que `verify_jwt` ya decodifica completo -- sin flag de CLI nuevo. Cierra un riesgo de seguridad real reportado por un adoptador en producción (MyFinance): revocar un JWT tras un reset de contraseña comparando un claim `tokenVersion` contra el valor real en DB, antes imposible (un token revocado seguía válido hasta su expiración natural).

Conversión a `String` consciente del caso real: un `Number` entero se imprime `"3"`, nunca `"3.0"` -- así `auth.claim("tokenVersion") == real.toString()` compara igual sin importar cómo el emisor serializó el claim.

**Verificado**: tests de checker + 7 tests unitarios en `session.rs` (string/number-entero/number-fraccionario/bool/claim ausente/no-escalar/sesión interna/sin JWT) + 1 test end-to-end contra un `linkc serve` real reproduciendo el caso de revocación exacto. Suite completa sin regresiones. Ver GRAMMAR.md §3.197.

## [1.152.0] - 2026-08-31

### ✨ Nuevo
**Aritmética de `Timestamp`** (PLAN.md §9.14 ítem 4) -- `.addMillis(n: Int64)`, `.addSeconds(n: Int)`, `.addMinutes(n: Int)`, `.addHours(n: Int)`, `.addDays(n: Int)`, todas devolviendo `Timestamp`. `n` negativo resta. `Value::Timestamp` ya es milisegundos planos desde epoch -- aritmética entera pura, `checked_mul`/`checked_add` en las dos operaciones (nunca cruda). Resuelve el caso real de un adoptador en producción (MyFinance): `now().addMinutes(5)` para expiración real de un código OTP de 2FA, hoy imposible.

De paso, corrige dos lugares que quedaron desactualizados desde que `toMillis`/`diffMillis`/`toIsoString` se agregaron sin nunca documentarse ni reflejarse en las completions del LSP (`Type::Timestamp => Some(vec![])` seguía devolviendo una lista vacía).

**Verificado**: tests de checker (aridad/tipos, `n` negativo tipa igual) + tests de runtime (suma/resta exacta por unidad, el caso real de expiración de OTP con datos de prueba, desborde con `n` gigante da error limpio no panic). Suite completa (1129 tests) sin regresiones. Ver GRAMMAR.md §3.196.

## [1.151.0] - 2026-08-31

### 🐛 Arreglado
**Bug crítico: `Decimal == Decimal` siempre daba `false` en runtime** (PLAN.md §9.14, encontrado validando el diseño de esta misma ronda) -- `impl PartialEq for Value` no tenía ningún arm para `(Decimal, Decimal)`, así que toda comparación `Decimal == Decimal` en un programa corriendo daba `false` en silencio (y `!=` daba `true`), pese a que el checker tipaba limpio y el orden (`<`/`>`) funcionaba bien. Para un lenguaje donde `Decimal` es el tipo de dinero, esto es serio -- un adoptador real (MyFinance) construyó cálculos fiscales y de facturación sobre `Decimal`. Fix de una línea. Ver GRAMMAR.md §3.195.

## [1.150.2] - 2026-08-30

### ✨ Nuevo
**Paquete npm `link-lang` listo para publicar** (PLAN.md §8.1 ítem 1) -- el instalador (`npm/bin/linkc.js`, descarga el binario correcto por plataforma desde GitHub Releases) ya existía desde v1.16.0, pero nunca quedó terminado de dejar publicable: versión desincronizada, sin `README.md`, sin verificación de integridad del binario descargado. Los tres cerrados.

- `SHA256SUMS.txt` generado en cada release (`release.yml`) y verificado contra el binario descargado ANTES de ejecutarlo nunca -- un mismatch aborta limpio, nunca corre un binario sin confirmar.
- `npm/README.md` nuevo para la página del paquete en npm.
- `package.json` sincronizado a la versión real, más `repository.directory`/`homepage`/`bugs`/`files`/`engines`.

**Verificado end-to-end contra un release real** (no solo leyendo el código): descarga limpia en frío (sin build local, sin cache, sin PATH), checksum verificado, extracción y ejecución del binario real, versión correcta impresa. Solo falta `npm publish`, que necesita las credenciales de la cuenta npm del usuario.

## [1.150.1] - 2026-08-30

### 🔧 Mantenimiento
Regenera el snapshot del demo insignia (`examples/users.link.snap`) y los artefactos del ejemplo `taskboard` -- el bump de v1.149.0 a v1.150.0 no los había regenerado, y el string de versión estampado en cada header (GRAMMAR.md §3.83) hizo que CI detectara deriva real (GRAMMAR.md §3.29) en ambos runners. Sin cambio de comportamiento, un solo commit atómico código+artefactos esta vez para no repetir el error.

## [1.150.0] - 2026-08-30

### ✨ Nuevo
**`crypto.awsS3PresignedUploadUrl(...)`** (PLAN.md §8.5.1) -- URL firmada AWS Signature V4 para SUBIR a S3 (`PUT`), cierra la mitad que `crypto.awsS3PresignedUrl` (§3.110, solo `GET`) había dejado deliberadamente abierta. Mismo mecanismo de firma ya verificado byte a byte contra el vector oficial `aws4_testsuite` de AWS -- reusado sin cambios vía un helper compartido -- con `Content-Type` firmado como header adicional: quien recibe la URL solo puede completar el upload con ESE Content-Type exacto.

- Séptimo parámetro (`contentType: String`) sobre la misma firma de 6 argumentos de `awsS3PresignedUrl`.
- Sin límite de tamaño de archivo (necesitaría una POST Policy de S3, mecanismo distinto) ni multipart directo -- límites honestos documentados, sin evidencia real de demanda todavía.

**Verificado**: 3 tests de tipos en `checker.rs` + 3 en `runtime/mod.rs` (estructura exacta, la firma cambia si el Content-Type cambia, límite de `expiresSeconds`). Suite completa sin regresiones. Ver GRAMMAR.md §3.194.

## [1.149.0] - 2026-08-30

### ✨ Nuevo
**`--db-schema <nombre>`/`LINK_DATABASE_SCHEMA`** (PLAN.md §9.3 ítem 4) -- namespacing de PostgreSQL para compartir una base entre varios `.link` sin colisión de nombre de tabla. `linkc serve app.link 8787 --db postgres://host/shared --db-schema tenant_a` y otro proceso con `--db-schema tenant_b` sobre la MISMA base, MISMA colección -- cada uno lee y escribe en su propio schema.

- `SET search_path` una vez al conectar (`connect_postgres_client`, el único punto de conexión de todo el proyecto) -- cubre `serve`/`db shell`/`inspect`/`export`/`import`/`migrate --dry-run`/`test --db`/el reconnect automático, todos gratis.
- `CREATE SCHEMA IF NOT EXISTS` solo en el constructor real de `serve`, nunca bajo `--adopt-existing` -- mismo criterio que ya rige `CREATE TABLE`.
- Solo PostgreSQL -- combinado con SQLite es un error de CLI limpio, nunca un no-op silencioso.
- `linkc introspect` compone con `options=` nativo de la URL en vez de un flag propio; `linkc serve-all` lo rechaza de entrada (nunca conecta a Postgres).

**Verificado**: 6 tests de CLI + 3 contra Postgres real, incluido el caso motivador exacto: dos programas con la MISMA colección, cada uno con su propio `--db-schema`, sin colisión. Suite completa sin regresiones. Ver GRAMMAR.md §3.193.

## [1.148.0] - 2026-08-30

### 🐛 Arreglado
**`information_schema.*` hardcodeaba `'public'` (o no filtraba schema en absoluto)** -- encontrado investigando PLAN.md §9.3 ítem 4 (`--db-schema`), antes de diseñar la feature: 8 consultas reales en `runtime/db.rs`/`migrate.rs`/`introspect.rs`, 4 de las 8 sin NINGÚN filtro de schema. Un bug real, alcanzable hoy por cualquiera con un `search_path` propio configurado -- una tabla en cualquier schema que no fuera `public` era invisible para `--adopt-existing`/`linkc introspect`/`linkc migrate --dry-run`/`db export`, o (peor) dos tablas del mismo nombre en schemas distintos podían leerse cruzadas en silencio.

- Fix: `table_schema = ANY(current_schemas(false))` en las 8 -- la función nativa de Postgres que da el `search_path` EFECTIVO de la sesión, la misma fuente que el propio motor usa para resolver un identificador sin calificar.
- Sin cambio de comportamiento para el caso `public` de siempre (la inmensa mayoría de las conexiones).

**Verificado**: 2 tests contra Postgres real -- `linkc introspect` ve una tabla en un schema no-`public` con el `search_path` correcto (y NO la ve sin él, control negativo); `--adopt-existing` adopta correctamente una tabla en un schema no-`public`. Suite completa sin regresiones. Ver GRAMMAR.md §3.192.

## [1.147.1] - 2026-08-30

### 🐛 Arreglado
**CI en rojo tras v1.147.0, por un bug de test -- no un bug real.** El nuevo test de `@encrypted` contra Postgres real (`an_encrypted_field_stores_ciphertext_in_postgres_but_round_trips_to_the_exact_plaintext_over_http`) leía la fila cruda con `WHERE id = $1` bindeando `id as i32`, pero la columna real es `BIGSERIAL` (`int8`) -- el driver de Postgres rechaza el tipo (`WrongType { postgres: Int8, rust: "i32" }`). Corregido a bindear `id` (`i64`) directo, sin cast. El cifrado en sí (`write_param`/`decode_row`) ya funcionaba: 76 de 77 tests pasaron en CI.

## [1.147.0] - 2026-08-30

### ✨ Nuevo
**`@encrypted`** (PLAN.md §9.5 ítem 2, última pieza de la ronda de seguridad completa) -- AES-256-GCM sobre un campo `String`/`String?`, puramente a nivel de almacenamiento. `type User = { id: Int, @encrypted ssn: String }` -- el `Value` que ve el resto del programa sigue siendo el `String` plano de siempre.

- `--encryption-key`/`LINK_ENCRYPTION_KEY` (32 bytes en base64) obligatoria SOLO si el programa declara algún campo `@encrypted` -- `linkc serve` rechaza arrancar sin ella, nunca falla recién en el primer uso.
- `nonce (12 bytes) || ciphertext || tag`, todo en base64, en la MISMA columna `TEXT` de siempre -- sin `ColumnKind` nuevo.
- `findWhere`/`countWhere`/`deleteWhere` sobre un campo `@encrypted` caen al camino interpretado de siempre (correcto, descifra antes de comparar) en vez de pushear una comparación contra ciphertext a SQL, que nunca podría matchear.
- `sumBy`/`countBy`/`avgBy`/`maxBy`/`minBy` agrupando por un campo `@encrypted`, y `@index`/`@unique` en el mismo campo, se rechazan en compile-time -- el nonce aleatorio los volvería garantías falsas, no solo redundantes.
- Tercera excepción real a "cero dependencias nuevas" (`aes-gcm`, tras `regex`/`flate2`) -- un cifrador simétrico nunca debería hand-rollearse.

**Límite honesto**: `db export`/`import` todavía no soportan una colección con campos `@encrypted` (rechazan de entrada, con mensaje claro); `db shell` no necesita cambios.

**Verificado**: 9 tests unitarios de cifrado + 8 de checker + 7 de CLI contra el binario real (SQLite) + 2 contra Postgres real. Suite completa sin regresiones. Ver GRAMMAR.md §3.191.

## [1.146.0] - 2026-08-30

### ✨ Nuevo
**`@requires(Role.X, ownerOf: <colección>, id: <parámetro>, field: <campo>)`** (PLAN.md §9.5 ítems 3 y 4, RBAC por recurso y ABAC -- resultaron ser la MISMA feature) -- una condición adicional, más allá del rol, evaluada contra un recurso real guardado en `db`. Ejemplo motivador: "solo el dueño de una factura puede leerla".

- `id:` nombra explícitamente el parámetro del propio rpc que trae el id del recurso -- nunca por posición, mismo criterio que `@rate_limit(..., key: ...)`.
- Comparación DIRECTA (`campo == currentUserId()`), sin máquina de expresiones -- deliberadamente angosto, auditable a simple vista.
- Etapa NUEVA y SEPARADA en runtime, después del chequeo de rol de siempre (que no cambia en nada): id mal formado -> 400, recurso inexistente -> 404, no sos el dueño -> 403.
- `ownerOf` aplica a TODOS los roles del mismo `@requires` -- un rol que necesite bypasear el chequeo se modela como un rpc separado, nunca una excepción implícita.
- Rechazado en compile-time sobre un `stream` (no se enforce ahí) y si `field` es `Int?` (evita un "siempre 403" silencioso).

**Verificado**: 8 tests de checker + 7 de parser + 5 contra el binario real. Suite completa sin regresiones. Ver GRAMMAR.md §3.190.

## [1.145.1] - 2026-08-30

### 🐛 Arreglado
**CI en rojo tras v1.145.0, por un bug real en `linkc db shell` -- no un bug de test.** `postgres::Error::to_string()` es deliberadamente parco para un error de tipo `Kind::Db` (confirmado leyendo la fuente de `tokio-postgres`): imprime literalmente `"db error"`, sin el mensaje real del servidor (severidad, texto, `DETAIL`, `HINT`), que vive aparte en el `DbError` accesible vía `.as_db_error()`. Cualquier error de Postgres contra `db shell` -- una escritura rechazada, SQL inválido, lo que sea -- se mostraba como el inútil `"error: db error"`, sin ninguna pista real de qué pasó. El test nuevo contra Postgres real (`db_shell_read_only_session_blocks_a_real_write_enforced_by_the_server_against_postgres`) lo agarró en CI antes de llegar a ningún usuario.

- `pg_error_text` (`db_admin.rs`), único punto de conversión de un `postgres::Error` a texto en `run_query_postgres`/`run_shell_postgres`, prefiere `DbError::to_string()` (mensaje real del servidor) cuando existe.

Suite completa sin regresiones. Ver GRAMMAR.md §3.189.

## [1.145.0] - 2026-08-30

### ✨ Nuevo
**`linkc db shell <archivo.link> [--db <url|archivo>]`** (PLAN.md §9.7 ítem 2) -- última pieza de la suite de administración de datos, cierra el ítem entero junto con `db inspect`/`db export`/`db import`. REPL de SOLO LECTURA sobre stdin/stdout: una línea de entrada es una consulta SQL completa, ejecutada contra la base real -- mismo criterio de "loop bloqueante, sin async" que `Lsp::run_stdio`, con framing mucho más simple.

- Camino nuevo de "SQL arbitrario, filas de tipo dinámico" -- SQLite vía `Row::get_ref`/`ValueRef`, Postgres vía dispatch manual por `Type` en `format_pg_cell`, reusando los decodificadores de rondas anteriores (`PgUuidText`/`PgDecimal`/`PgTimestampMicros`/`PgDateDays`/`PgJsonText`).
- Solo lectura real: SQLite abre `SQLITE_OPEN_READ_ONLY`, Postgres corre `SET default_transaction_read_only = on` una vez tras conectar -- el SERVIDOR rechaza cualquier escritura de la sesión, robusto contra cualquier truco de SQL que engañaría a un parser de palabras clave del lado del cliente.
- Un tipo Postgres no cubierto (`point`, `tsvector`, etc.) cae a un placeholder legible, nunca falla la consulta entera.

**Verificado**: 6 tests de CLI contra el binario real + 2 contra Postgres real. Suite completa sin regresiones. Ver GRAMMAR.md §3.189.

## [1.144.0] - 2026-08-30

### ✨ Nuevo
**Lint `manual-role-check-without-requires`** (PLAN.md §9.5 ítem 1, primer ítem de la ronda de seguridad) -- reformulación ya decidida en una ronda anterior (el lint original, "`@requires` que nunca llama a `auth.currentRole()`", era de mala señal: el caso más común y correcto de `@requires(Role.Admin)` nunca llama a ninguno de los dos). La versión implementada es la inversa: detecta un rpc que hace su PROPIA verificación manual de rol adentro del cuerpo (`auth.currentRole()`/`currentUserId()`), sin `@requires`/`@authenticated` en su propia anotación -- el chequeo real vive en lógica ad-hoc del cuerpo, un bug ahí bypasea todo en silencio.

- Recorrido exhaustivo por variante de `Expr`/`Stmt` (mismo patrón que `unused-var` ya usa) -- una llamada escondida en una closure o un `match` sigue siendo detectada.
- `@cron` excluido, mismo criterio que `mixed-service-auth` (nunca alcanzable vía HTTP). Un rpc que YA tiene `@requires` y además llama a `auth.currentRole()` no dispara -- esa llamada es a lo sumo redundante, nunca la única defensa.

**Verificado**: 7 tests unitarios + 1 contra el binario real. Suite completa (1410 tests) sin regresiones. Ver GRAMMAR.md §3.188.

## [1.143.1] - 2026-08-30

### 🐛 Arreglado
**CI en rojo tras v1.143.0, por una aserción de test incorrecta -- no un bug real.** El nuevo test de `jsonb` contra Postgres real (`pg_integration.rs`) esperaba texto EXACTO de vuelta (`{"button":"cta","n":2}`), pero `jsonb` (a diferencia de `json`) no preserva el texto de entrada tal cual -- Postgres lo reparsea a su árbol binario interno y lo reserializa al leer, reordenando claves y normalizando espacios (`{"n": 2, "button": "cta"}`, MISMO valor, texto distinto -- comportamiento real y documentado de Postgres). El fix en sí (`Cell::to_sql`/`PgJsonText`) ya funcionaba correctamente -- confirmado en CI: 72 de 73 tests pasaron, incluido el hermano de `json` (que sí preserva texto exacto). Corregidas las dos aserciones de texto exacto a comparación semántica (parsear y comparar el VALOR JSON, no los bytes) -- el contrato real de un roundtrip `jsonb` es equivalencia de valor, nunca byte a byte.

Suite completa (1403 tests) sin regresiones.

## [1.143.0] - 2026-08-30

### 🐛 Arreglado
**Escritura contra una columna `json`/`jsonb` NATIVA de Postgres fallaba SIEMPRE, con o sin valor.** Bug real de producción, severidad alta, reportado por skynet-43 (iaacademy): una columna `jsonb` adoptada (`properties`, una tabla de analíticas), mapeada a `String?` -- la forma que `linkc introspect` ya recomienda para JSON sin tipo propio -- daba `"error deserializing column N"` al escribir, la fila nunca se insertaba. `null` fallaba igual que un valor real. Impacto: ~2-3 min con un endpoint público de analíticas devolviendo 500 a todo visitante, antes de revertir a SQL crudo.

- Causa (confirmada leyendo el código fuente de `postgres-types`, no solo documentación): `String::accepts` no incluye `json`/`jsonb` -- el rechazo pasa por tipo de columna, antes de mirar el valor.
- Mismo patrón que `uuid`/`inet` (§3.177/§3.179): `json` es texto UTF-8 crudo; `jsonb` antepone un byte de versión (`0x01`). `PgJsonText` nueva (`runtime/store.rs`) resuelve lectura, un caso simétrico en `Cell::to_sql` resuelve escritura.
- Sin cambios a la advertencia de `linkc introspect` para `json`/`jsonb` -- sigue siendo consejo válido sobre MODELADO (¿`String` genérico o un `type` propio?), no sobre si `String` funciona.

**Verificado contra Postgres real**: el repro exacto reportado (columna `jsonb`, escritura con contenido y con `null`, confirmado con un operador `jsonb` real que solo funciona si se guardó como `jsonb` de verdad) + un segundo test para `json` (no `jsonb`, formato binario distinto) + 5 tests unitarios locales de la codificación/decodificación en sí. Suite completa (1403 tests) sin regresiones. Ver GRAMMAR.md §3.187.

## [1.142.0] - 2026-08-30

### 🔧 Interno
**`builtin_args!`: fast-path para curar un builtin nuevo más rápido** (PLAN.md §9.2 ítem 2, "Pilar 2" del roadmap de concurrencia) -- tooling del compilador, NO una feature del lenguaje: ningún `.link` cambia, cero sintaxis nueva, cero tipo nuevo, cero builtin nuevo expuesto.

- Investigación previa (2 forks) encontró que el pedido original -- FFI hacia `crates.io` entero -- no es viable con la arquitectura actual (`Value`/`Type` son enums cerrados matcheados exhaustivamente en checker/runtime/codegen, sin `libloading`/WASM-component en ningún lado) sin construir antes un sistema de macros/codegen completo, y choca con la política de "cero dependencias nuevas" ya sostenida. El usuario eligió explícitamente un fast-path para builtins curados en vez de FFI arbitrario.
- Cada builtin (`crypto`/`http`/`math`/etc., ~74 en total) se define hoy en dos lugares que pueden desincronizarse a mano: un arm en `checker.rs` (tipado) y uno espejo en `runtime/mod.rs` (lógica real). El lado checker es máximamente regular -- el macro `builtin_args!` lo colapsa de 5-7 líneas a 1, reusando el mismo patrón de destructuring que ya usaban esos arms. El lado runtime sigue 100% a mano, a propósito -- su lógica varía demasiado para generarse.
- Alcance v0: solo para builtins nuevos de acá en adelante. Retrofit de prueba en 2 arms existentes (`crypto.hashPassword`, `crypto.randomInt`) para confirmar equivalencia exacta -- encontrado en el camino: no existía cobertura de test para el mensaje de error de aridad de estos dos builtins, cerrado de paso con tests nuevos.

**Verificado**: 4 tests nuevos en `checker.rs` (camino feliz + mensaje de aridad exacto para cada builtin retrofiteado) + los tests de comportamiento ya existentes sin modificar. Suite completa (1396 tests) sin regresiones. Ver GRAMMAR.md §3.186.

## [1.141.0] - 2026-08-30

### ✨ Nuevo
**`linkc db export`/`linkc db import`** -- siguiente pieza de la suite de administración de datos (PLAN.md §9.7 ítem 2), después de `linkc db inspect` (§3.175). `export` vuelca cada colección declarada a un solo archivo JSON, byte-idéntico al wire real (mismo `value_to_json` que `db.<c>.all()` ya usa por HTTP); `import` lo lee de vuelta contra un target SQLite o PostgreSQL, PRESERVANDO el id original de cada fila. `seed` no necesitó su propia pieza -- importar contra un target vacío YA ES ese caso, mismo mecanismo. Solo `linkc db shell` (un REPL interactivo, mucho más difícil de verificar de forma no interactiva) queda pendiente.

- `export` nunca ejecuta DDL ni construye un `Db` completo (que siempre migra el esquema al conectar) -- lector propio, mismo espíritu que `db inspect`: una tabla faltante es "0 filas", nunca un error. Nunca filtra `@softDelete` -- mueve TODA fila física, mismo criterio que `db inspect`/`db.tableStats()`.
- `import` conecta con el camino NORMAL de conexión (`CREATE TABLE IF NOT EXISTS` idempotente) -- cubre "target vacío" (el caso `seed`) y "target ya servido antes" (cruce de entornos) con un solo código. Un mecanismo nuevo, solo Rust y nunca alcanzable desde `.link`, escribe cada fila con su id EXPLÍCITO preservado, y resincroniza la secuencia de autoincremento de cada backend después (SQLite: `sqlite_sequence`; Postgres: `setval`/`pg_get_serial_sequence`) para que un `insert()` normal posterior nunca choque con un id importado.
- `@validate`/`@check` de nivel tipo se saltean a propósito en `import` (las restricciones de base -- `CHECK`/`UNIQUE` -- siguen activas siempre): una restauración cruda de datos que ya eran válidos no debería bloquearse por un validador de flujo de trabajo específico de la app. Un choque de id cancela y revierte TODO el import, sin dejar nada a medias -- sin modo overwrite/skip en v0.

**Verificado**: 14 tests nuevos (6 de CLI contra el binario real para `export`, 5 para `import` -- incluye el caso seed con secuencia resincronizada confirmada vía un insert normal posterior, cruce de entornos idempotente, y choque de id revirtiendo todo -- más 3 contra Postgres real, incluido el resync de secuencia real). Suite completa (1392 tests) sin regresiones. Ver GRAMMAR.md §3.185.

## [1.140.2] - 2026-08-29

### 🐛 Arreglado
**CI en rojo tras v1.140.1, por segunda vez seguida -- pero esta vez el bug era del test, no del compilador.** El nuevo test de `sumBy` contra Postgres real (`pg_integration.rs`) afirmaba `"24.5100"` para el total de `WIDGET` -- ese número venía de copiar el escenario del test equivalente contra SQLite (`runtime/mod.rs`), que incluye un `reprice` de por medio; este test nuevo no lo tiene, así que el total real y correcto es `19.9900 + 0.0100 = "20.0000"`. Corregida la aserción, sin tocar el compilador -- el `sumBy` real ya sumaba bien.

## [1.140.1] - 2026-08-29

### 🐛 Arreglado
**CI en rojo tras v1.140.0**: `match`/narrowing de un `Optional`/`Union` (`match db.<c>.find(id) { fila: T => ..., null => ... }`) nunca matcheaba ningún arm cuando `T` tenía un campo `Decimal` -- `value_matches_type` (`runtime/mod.rs`) es un `match` sobre `Type` sin brazo para `Type::Decimal`, cayendo al fallback `_ => false`. Expuesto por el propio test nuevo contra Postgres real en CI (el test local equivalente contra SQLite usa un harness que se salta el checker, así que compiló pero nunca ejercitó este camino). Arreglado con un brazo explícito, mismo criterio que el resto de la ronda. Ver GRAMMAR.md §3.184.

**Verificado**: los 2 tests de `pg_integration.rs` de v1.140.0 ahora corren de punta a punta contra Postgres real en CI. Suite completa (1378 tests) sin regresiones.

## [1.140.0] - 2026-08-29

### ✨ Nuevo
**`Decimal`: tipo numérico de precisión exacta (punto fijo, 4 decimales).** PLAN.md §9.2 ítem 1 -- `Float` es una fuente de error de redondeo confirmada por adoptadores financieros reales en columnas de dinero (`19.99 * 3` da `59.96999999999999...` con `Float`, exacto `59.9700` con `Decimal`). Representación `i128` interna escalada ×10.000, decisión tomada explícitamente por el usuario tras ver el trade-off frente a precisión variable estilo `numeric` nativo.

- Sin sintaxis de literal nueva -- se construye vía `.toDecimal()` desde `Int` (exacto) o `Float` (redondea al 4to decimal), mismo patrón que `Int64`. `+`/`-` exactos; `*`/`/` con redondeo half-up (empate se aleja de cero); `%` rechazado a propósito con mensaje claro.
- Wire format: string JSON con exactamente 4 decimales siempre (`"1234.5600"`), nunca un `number` nativo -- evita pérdida de exactitud en cualquier cliente JS/TS.
- Almacenamiento: `INTEGER` escalado en SQLite (con chequeo de rango), `NUMERIC(38,4)` nativo en Postgres -- incluye lectura y escritura contra una columna `numeric`/`decimal` YA EXISTENTE (`--adopt-existing`, el caso real de MyFinance), con un codificador/decodificador binario nuevo que nunca toca `f64`.
- `sumBy`/`maxBy`/`minBy`/`maxRow`/`minRow` y `@check(min/max/range)` soportan `Decimal`. `avgBy` queda excluido a propósito en v0 -- asimetría real de almacenamiento entre backends, documentada como límite honesto, no atacada esta ronda.

**Verificado**: 29 tests nuevos (checker.rs, runtime/mod.rs -- incluye un CRUD completo contra SQLite real --, runtime/store.rs -- ida y vuelta del codec binario de Postgres -- y 2 contra Postgres real en pg_integration.rs, columna adoptada y generada). Suite completa (1066 tests de biblioteca + toda la matriz de integración) sin regresiones. Ver GRAMMAR.md §3.184.

## [1.139.0] - 2026-08-29

### ✨ Nuevo
**`link.lock` como pin real de dependencias git + locking entre procesos concurrentes** -- cierra los dos huecos reales que quedaban en el package manager `git+<url>#<rev>` (GRAMMAR.md §2.1), reforzando el modelo "git-as-registry" ya elegido en vez de construir un registro centralizado (decisión ya tomada y documentada en PLAN.md, no revertida acá).

- **Bug real encontrado antes de diseñar el fix, corriendo el código no leyéndolo**: una dependencia por RAMA quedaba congelada en el commit del primer clone para siempre -- `git fetch` nunca mueve una rama LOCAL, solo su ref de seguimiento remoto, y el checkout seguía resolviendo a la copia local vieja. Arreglado: un commit SHA completo o un tag ya conocido confían en el caché sin red; cualquier otra cosa siempre fetchea, y el checkout prefiere el ref de seguimiento remoto recién actualizado.
- `link.lock` ahora se LEE para decidir qué commit usar (antes solo se escribía, informativo) -- mismo contrato que `Cargo.lock`/`package-lock.json`. `linkc build --update-deps` es el único comando que ignora el pin y re-resuelve fresco, avanzándolo.
- Un lock advisory basado en archivo serializa dos `linkc build`/`serve` concurrentes que resuelven la misma dependencia -- se autorepara si un proceso muere a mitad de un clone.

**Verificado**: 9 tests nuevos (gitdep.rs + modules.rs) más una corrida manual de punta a punta contra un repo git local real (resuelve y pinnea, se queda pinneado con el remoto avanzado, `--update-deps` re-resuelve y el checker atrapa el tipo nuevo). Suite completa sin regresiones. Ver GRAMMAR.md §3.183.

## [1.138.0] - 2026-08-29

### 🐛 Arreglado
**Escritura de `Timestamp` corrompía en silencio una columna `date`/`timestamp`/`timestamptz` NATIVA de Postgres adoptada.** Bug real de producción, severidad alta, reportado por skynet-43 (iaacademy): `insert`/`applyPatch` contra una columna `created_at timestamp with time zone` guardaba una fecha completamente distinta a la enviada -- un salto de 26 años en el repro reportado (`2026-08-29T12:34:56.789Z` guardado como `2000-01-21 16:40:06.896896`), sin ningún error.

- Causa: `Cell::to_sql` nunca tuvo un caso para `TIMESTAMP`/`TIMESTAMPTZ` -- un `Cell::Int(millis)` caía al `i64::to_sql` genérico, que serializa 8 bytes crudos como `int8`. Postgres interpreta esos MISMOS 8 bytes, para una columna temporal, como microsegundos desde SU epoch (2000-01-01) -- mismo ancho binario, semántica distinta, así que el servidor los acepta sin protestar. Más peligroso que el mismatch ya documentado de `numeric` (§3.103, formato de ancho DISTINTO, falla ruidoso) precisamente porque acá el ancho coincide por casualidad.
- Arreglado con dos casos nuevos en `Cell::to_sql` (`TIMESTAMP`/`TIMESTAMPTZ` y `DATE`), simétricos a la lectura que §3.91 ya resolvía -- sin ninguna dependencia nueva.
- Límite honesto sin cambios: el mismatch simétrico de `Float`/`numeric` (§3.103) sigue sin arreglar en esta ronda -- solo se cerró `Timestamp`, el reportado como bug real y el más peligroso (falla en silencio, no ruidoso).

**Verificado contra Postgres real**: una tabla adoptada con columnas nativas, un `insert` real vía `--adopt-existing`, y la fila leída de vuelta con el cliente `postgres` CRUDO (no el decodificador propio de c-script) para confirmar el año real guardado. Más 5 tests unitarios sobre la aritmética de conversión. Suite completa (1346 tests) sin regresiones. Ver GRAMMAR.md §3.182.

## [1.137.0] - 2026-08-29

### ✨ Nuevo
**Camino de despliegue recomendado (git+CI)** -- último ítem de "mejoremos estos límites y las fricciones". Auditando qué faltaba apareció que `linkc docker`/`systemd`/`pm2-config`/`doctor`/`migrate --dry-run` ya existían todos, maduros -- el gap real era que nada los conectaba en un pipeline recomendado, y que `docs/multi-service-deployment.md` describía como "no existe todavía" tres cosas que ya habían enviado (`--host`, los generadores de systemd/pm2, `--restart-backoff`), más una premisa central (que no había modo de un solo proceso para varios `.link`) que tampoco era cierta desde que `linkc serve-all` se agregó.

- `linkc new <nombre>` ahora scaffoldea `.github/workflows/deploy.yml` en los tres templates. Job `test-and-build` en todo push, sin secrets; job `deploy` apagado por default (disparo manual) hasta configurar 5 secrets y cambiar una línea `if:` -- un proyecto recién creado no arranca con CI en rojo por un despliegue sin configurar.
- El job `deploy`, activo, encadena piezas que ya existían: `linkc doctor` (pre-flight de solo lectura) → copiar el `.link` + reiniciar el servicio → `linkc doctor --target-url` (confirma que el servidor en vivo corre la versión nueva).
- `docs/multi-service-deployment.md` corregido (no solo extendido): las tres afirmaciones desactualizadas arregladas, y reescrito para presentar `linkc serve-all` (un proceso para todos los servicios) como alternativa real, no solo "N procesos separados" como si fuera la única opción.
- `docs/deploying-from-git.md` (nuevo): qué hace cada paso del workflow scaffoldeado, cómo activar el deploy, tabla de los 5 secrets.

**Verificado**: 4 tests unitarios de scaffold (incluido uno nuevo que confirma los tres templates + que el job deploy queda apagado por default) más una corrida real de `linkc new` con el `deploy.yml` resultante validado contra un parser YAML real. Suite completa sin regresiones. Ver GRAMMAR.md §3.181.

## [1.136.1] - 2026-08-29

### 🔧 Proceso
**v1.136.0 quedó con CI en rojo** -- pero el bug era del PROCESO de release, no del código: al regenerar `examples/users.link.snap` localmente después de bumpear la versión, el comando corrió como `linkc test examples/users.link --update` -- sin el segundo argumento posicional (`examples/users.link.snap`) que `linkc test` en realidad requiere para saber a qué archivo escribir. El comando reportó "2 passed" y no tocó el snapshot en absoluto, así que el archivo committeado seguía con el stamp `v1.135.1` mientras el binario real ya decía `v1.136.0` -- CI (`linkc test examples/users.link examples/users.link.snap`, sin `--update`) lo detectó como deriva real en las dos plataformas (windows-latest y ubuntu-latest, mismo diff en ambas, confirmando que no era un problema de generación cross-platform sino simplemente el snapshot desactualizado).

**Arreglado regenerando con los tres argumentos posicionales correctos** (`linkc test examples/users.link examples/users.link.snap --update`) -- el mismo comando exacto que sugiere el propio mensaje de error de CI. De paso, los archivos generados de `examples/taskboard/frontend/src/gen/*` también se resincronizaron al mismo patrón (solo el stamp de versión cambia, sin deriva de contenido).

**Verificado en CI** -- el chequeo de deriva del contrato (GRAMMAR.md §3.29) que v1.136.0 no llegó a pasar en ninguna de las dos plataformas ahora sí. Suite completa sin regresiones.

## [1.136.0] - 2026-08-29

### ✨ Nuevo
**Compresión GZIP de la respuesta HTTP** -- segundo ítem de "mejoremos estos límites y las fricciones", junto al rate limiter distribuido (v1.134.0). Transparente: sin flag nuevo, sin anotación nueva. Si la request trae `Accept-Encoding: gzip`, la respuesta viaja comprimida con `Content-Encoding: gzip`; si no, byte a byte igual que antes de esta ronda.

- Solo GZIP, no brotli -- alcance v0 deliberado, `flate2` es la única dependencia nueva razonable acá (segunda excepción real a "cero dependencias nuevas" del proyecto, después de `regex`).
- Umbral mínimo de 1024 bytes (`GZIP_MIN_BODY_BYTES`, mismo orden de magnitud que el default de nginx) -- un body chico no se comprime aunque el cliente lo acepte, evita gastar CPU sin ahorro real.
- Un `stream` (SSE) queda excluido de forma estructural, no por un chequeo aparte -- ese camino escribe chunked transfer encoding a mano, nunca pasa por el único punto donde se decide comprimir (`cors_response_with_type`).
- `client.ts` no necesita ningún cambio -- `fetch` del browser/Node descomprime GZIP solo.

**Verificado contra un `linkc serve` real** (subprocess + `TcpStream`, mismo estilo que el resto de `server_http.rs`): un body grande con `Accept-Encoding: gzip` comprime y descomprime al JSON exacto esperado; el mismo body sin ese header no comprime; un body chico con el header tampoco comprime (umbral). Suite completa sin regresiones. Ver GRAMMAR.md §3.180.

## [1.135.1] - 2026-08-29

### 🔧 Proceso
**v1.135.0 quedó con CI en rojo** -- pero el bug era del TEST, no del fix real. `adopt_existing_reads_and_writes_a_native_inet_column_mapped_to_string` sembraba su fila inicial con el cliente `postgres` crudo, bindeando `&str` como parámetro contra la columna `inet` -- el MISMO problema que motivó todo este round (el driver no sabe bindear texto contra un tipo nativo sin decodificación binaria), solo que en el cliente de prueba, no en `Cell::to_sql` de c-script. Arreglado sembrando con el literal embebido en el SQL (seguro -- son constantes fijas del test) en vez de un parámetro bindeado.

**Verificado en CI contra Postgres real** -- el test que v1.135.0 no llegó a pasar ahora sí; confirma que el fix real (`postgres_string_cell`/`Cell::to_sql` para `uuid`/`inet`/`cidr`) funciona de punta a punta. Suite completa sin regresiones.

## [1.135.0] - 2026-08-29

### 🐛 Arreglado
**`String` (y campos `Uuid` fuera de la PK) contra columnas `uuid`/`inet`/`cidr` NATIVAS de Postgres.** Segundo reporte real de adopción de iaacademy (vía skynet-43), mismo día: `find`/`findWhere`/`all` rompían con `"error deserializing column N"` contra datos reales, aunque `linkc doctor`/`migrate --dry-run` pasaran limpios. Descartadas dos hipótesis en el camino (un hueco de `pg_attribute.attnum` tras un `DROP COLUMN` real, reproducido a mano sin éxito; el orden de campos del `.link`) antes de que skynet-43 aislara la causa real con el DDL exacto: una columna `source_ip inet` (mapeada a `String?`, como recomienda `linkc introspect`) y una columna `uuid` legada mapeada a `String` en vez de `Uuid`.

- Mismo problema que la PK `id: Uuid` (§3.177), generalizado: `uuid`/`inet`/`cidr` tienen formato binario propio, no texto UTF-8. `postgres_string_cell` prueba `String` primero, después `PgUuidText` (reusa el decodificador de la PK), y por último `PgInetText` (nuevo, usa `std::net::{Ipv4Addr,Ipv6Addr}` para el formateo de texto correcto). La escritura gana los mismos dos casos, simétricamente.
- `linkc introspect` sube `uuid` de `String` con advertencia a `Uuid` sin advertencia, e `inet`/`cidr` a `String` sin advertencia -- mapeos exactos ahora, mismo criterio que `date`/`timestamp` (§3.91).

**Verificado contra Postgres real**: una tabla adoptada con `source_ip inet` (lectura con valor y con NULL, escritura confirmada con SQL crudo `pg_typeof`), una tabla adoptada con una columna `uuid` nativa mapeada a `String`. Más 8 tests unitarios locales (sin Postgres) sobre la codificación/decodificación binaria en sí. Suite completa sin regresiones. Ver GRAMMAR.md §3.179.

## [1.134.1] - 2026-08-29

### 🔧 Proceso
**v1.134.0 quedó con CI en rojo** -- el UPSERT distribuido del rate limiter fallaba en cada request (`incorrect binary data format in bind parameter 2`), tragado en silencio por la degradación a memoria. Causa: `$2 - 1` con el literal entero `1` sin tipo hacía que Postgres infiriera `$2` como `integer`, no `double precision` -- la primera aparición de un parámetro fija su tipo para toda la sentencia. Arreglado con un cast explícito (`$2::double precision`/`$3::double precision`/`$4::bigint`) en cada aparición. De paso, la degradación silenciosa se volvió un `eprintln!` real -- útil para el diagnóstico esta vez, y para cualquier operador futuro que la vea en producción.

**Verificado en CI contra Postgres real** -- el test de dos instancias que v1.134.0 no llegó a pasar (exactamente 5 admitidas compartidas, no 10) ahora sí. Suite completa sin regresiones.

## [1.134.0] - 2026-08-29

### ✨ Nuevo
**`@rate_limit` distribuido vía Postgres** -- pedido explícito del usuario ("mejoremos estos límites y las fricciones") sobre el gap documentado de producción: N réplicas compartiendo una base diluían el límite real (hasta N × capacidad, no capacidad), con solo un contador en `/metrics` para notarlo, nunca una solución. Mismo `@rate_limit("N/ventana")` de siempre, sin sintaxis nueva -- lo que cambia es que, sobre Postgres, el bucket vive en una tabla interna (`_linkc_internal_rate_limits`, invisible para `db {}`/`introspect`/`migrate`) compartida de verdad por todas las instancias que apuntan a la misma base, en vez de un `HashMap` por proceso.

- Mismo algoritmo exacto que el limitador en memoria (token bucket, refill continuo). Un solo UPSERT atómico (`INSERT ... ON CONFLICT ... DO UPDATE ... WHERE`), mismo criterio que `increment()`: nunca leer-y-después-escribir en dos pasos que puedan carrerear entre procesos.
- Degrada, nunca rompe el arranque: sin la tabla (`--adopt-existing` sin crearla a mano, o un rol sin `CREATE TABLE`), esta instancia usa el limitador en memoria de siempre, comportamiento idéntico al de antes. SQLite no cambia -- un solo proceso ya tiene el estado exacto.
- De paso, corregidas dos afirmaciones desactualizadas en el README ("No CSP or HSTS" -- HSTS existe hace varias rondas, `--hsts`; "sin `X-Forwarded-For`" -- `--trust-proxy` también existe hace rondas) encontradas auditando esta misma sección.

**Verificado contra Postgres real**: DOS instancias `linkc serve` reales, procesos separados, apuntando a la misma base -- 16 requests concurrentes repartidas entre las dos contra `@rate_limit("5/2s")` admiten exactamente 5 en total, no 10 (5 por instancia). Refill real (agotar, esperar más que la ventana, volver a admitir). `--adopt-existing` sin la tabla: arranca y sigue limitando, por proceso. Suite completa sin regresiones. Ver GRAMMAR.md §3.178.

## [1.133.1] - 2026-08-29

### 🔧 Proceso
**v1.133.0 quedó con CI en rojo** -- el intento inicial de resolver el bind de un id `Uuid` contra una columna Postgres NATIVA `uuid` fue un cast SQL explícito (`$1::uuid` en vez de `$1`), asumiendo que forzaría al servidor a inferir el parámetro como texto. Verificado FALSO contra Postgres real en CI, no en teoría: el servidor sigue infiriendo el tipo desde la columna destino sin importar el cast, así que el mismatch de wire binario seguía pasando (`ERROR: incorrect binary data format in bind parameter 1`).

El arreglo real: `Cell::to_sql` (`runtime/store.rs`) ahora detecta cuándo el servidor pide de verdad el tipo `uuid` y decodifica la forma canónica de 36 caracteres a sus 16 bytes binarios crudos a mano (`uuid_string_to_binary`) en vez de mandar el texto tal cual -- sin sumar la dependencia opcional `with-uuid-1`. La lectura tiene el mismo problema simétrico, resuelto igual (`ColumnKind::Uuid` nuevo, `PgUuidText`/`postgres_cell`). El cast `::uuid` -- que no hacía nada -- se sacó del todo.

De paso, un test de `introspect` nuevo (v1.133.0) resultó frágil contra la base compartida de tests en paralelo -- corregido para chequear solo advertencias de SU propia tabla, no el stderr entero.

**Verificado en CI contra Postgres real** -- las 3 pruebas de `pg_integration.rs` que v1.133.0 no llegó a pasar (fresco, `--adopt-existing` contra una tabla armada a mano igual a la de iaacademy, `introspect`) ahora sí. Suite completa sin regresiones.

## [1.133.0] - 2026-08-29

### ✨ Nuevo
**`id: Uuid` como clave primaria alternativa a `id: Int`** -- cierra el bloqueo real de adopción de iaacademy que v1.132.0 (GRAMMAR.md §3.176) había dejado explícitamente pendiente: tablas de producción con `id uuid DEFAULT gen_random_uuid()` ahora se pueden modelar y adoptar de punta a punta, sin migrar ningún esquema.

```
type Lead = { id: Uuid, email: String }
type NewLead = { email: String }
db { leads: Lead[] }
service Leads {
  rpc create(email: String) -> Lead { db.leads.insert(NewLead { email: email }) }
  rpc get(id: Uuid) -> Lead? { db.leads.find(id) }
}
```

- La PK se genera SIEMPRE del lado de la aplicación (mismo generador que `crypto.uuid()`), nunca depende de `DEFAULT`/`RETURNING`/`last_insert_rowid()` -- es lo que hace posible adoptar una tabla existente sin tocarla.
- SQLite: `TEXT PRIMARY KEY NOT NULL`. Postgres: tipo nativo `UUID PRIMARY KEY` -- ver v1.133.1 para cómo el bind/lectura contra ese tipo nativo termina funcionando de verdad.
- `find`/`applyPatch`/`delete`/`increment`/`insert`/`upsert`/`page`/`maxRow`/`minRow` funcionan igual que con `id: Int`. `pageAfter` queda RECHAZADO a propósito sobre una PK Uuid -- su garantía de no saltear filas concurrentes depende de que el id crezca en el mismo orden que la inserción, falso para un UUID aleatorio.
- `linkc introspect` ahora emite `id: Uuid` directo (sin advertencia) para una PK `uuid` nativa; `linkc migrate --dry-run`/`--adopt-existing` la reconocen como compatible.

**Bug real encontrado en el camino** (atrapado por el test de `upsert`, vía su pushdown de predicado): `find_where_conjunction`/`select_rows_page`/`top_row` (`runtime/db.rs`) tenían el mismo `ColumnKind::Int` hardcodeado para decodificar la columna `"id"` que `select_rows` ya tenía -- sin el fix, `findWhere`/`page`/`maxRow`/`minRow` sobre una colección Uuid rompían con un error de decodificación.

**Verificado localmente**: 6 tests de checker + 5 contra SQLite real. Suite local sin regresiones (1316 tests, +14 sobre v1.132.0) -- pero CI contra Postgres real quedó en rojo, ver v1.133.1. Ver GRAMMAR.md §3.177.

## [1.132.0] - 2026-08-29

### ✨ Nuevo
Reporte de adopción real (proyecto nº5 del ecosistema, iaacademy, vía la sesión skynet-43) -- 3 tablas públicas con `id uuid` quedan bloqueadas por el requisito de PK entera (GRAMMAR.md §3.36/§3.59), rechazadas correctamente al conectar, pero dos gaps de HERRAMIENTAS sí eran nuevos:

- **`linkc introspect` ahora avisa cuando la PK `"id"` no es realmente un entero.** Antes emitía `id: Int` para CUALQUIER PK llamada `"id"` sin mirar su tipo real en PostgreSQL -- una PK `id uuid` generaba el mismo `.link` "limpio" que una PK `id BIGSERIAL`, y la única señal de que algo estaba mal aparecía recién en `linkc serve`/`migrate --dry-run`, después de ya escribir un programa entero alrededor de un placeholder que nunca fue real. Mismo canal de advertencia por stderr que el resto de `introspect`, nunca omite la columna.
- **`linkc doctor --target-url <url>` (o `LINK_DOCTOR_TARGET_URL`) detecta deriva de versión contra un `linkc serve` real ya corriendo.** Nuevo chequeo opt-in: compara `linkc::VERSION` local contra el `version` que `/health` ya devolvía -- `[OK]` si coinciden, `[INFO]` si difieren (no falla el chequeo, solo lo hace visible), `[ERROR]` si la URL no responde. Sin el flag, comportamiento idéntico a siempre.

**Deliberadamente NO resuelto** -- el bloqueo real de iaacademy: aceptar `id: Uuid` (o cualquier tipo no entero) como PK de una colección. Toca `insert`/`insert_returning_id`, el tipo de parámetro de `insert`, el emisor SQLite y el checker -- señal de madurez general del lenguaje, no un ticket puntual, queda para su propia ronda de diseño. Ver GRAMMAR.md §3.176.

**Verificado**: build release sin warnings; suite completa sin regresiones (1302 tests, +1 sobre v1.131.0 -- el nuevo test de CLI `introspect` contra Postgres real en `pg_integration.rs`); `linkc doctor --target-url` probado a mano contra un `linkc serve` real -- puerto cerrado (falla limpio, sin colgarse, sin panic) y versión igual (`[OK]`) contra un servidor real corriendo en este mismo round.

## [1.131.0] - 2026-08-28

### 🔧 Proceso
Sin cambios de código de producción sobre v1.130.0 -- ese tag quedó con CI en rojo por un bug real en el DISEÑO de un test, no en el binario. `db_inspect_reports_real_row_counts_against_postgres` (`pg_integration.rs`) declaraba dos colecciones en el MISMO `.link` que `Serve::start` corría, asumiendo que una colección que ningún `rpc` toca no tendría tabla física -- pero `linkc serve` crea la tabla de TODA colección declarada al conectar (`new_with_options`/`connect_postgres_with_options`, GRAMMAR.md §3.17), sin importar si algún `rpc` la usa. Fix: dos `.link` distintos contra la misma base -- uno más chico que `Serve::start` sirve de verdad, y uno más grande (con una colección de más) que `linkc db inspect` -- que nunca ejecuta DDL -- usa para leer. Confirmado en vivo contra Postgres real antes de este commit. v1.130.0 queda en el historial con CI rojo mencionado acá para que quede claro por qué; su código de producción es idéntico al de v1.131.0. Ver v1.130.0 para el changelog real de esta ronda (`linkc db inspect`).

## [1.130.0] - 2026-08-28

### ✨ Nuevo
**`linkc db inspect <archivo.link> [--db <url|archivo>]`** -- primera pieza de la suite de administración de datos (PLAN.md §9.7 ítem 2): un diagnóstico de SOLO LECTURA de qué colecciones declaradas existen físicamente y cuántas filas tienen, sin ejecutar ningún DDL.

```
$ linkc db inspect app.link --db app.db
linkc db inspect -- 'app.link' contra SQLite embebido en 'app.db'

  items       2 columna(s) declaradas  1 fila(s)

1 colección(es) declaradas, 0 sin crear todavía, 1 fila(s) en total
```

- Mismo espíritu de solo lectura que `linkc doctor`/`linkc migrate --dry-run` -- reusa el mismo `resolve_db_source` (`--db`/`LINK_DATABASE_URL`) que esos dos y `linkc serve`.
- `exists: false` implica `row_count: None`, nunca `Some(0)` -- "no existe todavía" nunca se confunde con "existe pero está vacía".
- El conteo es FÍSICO, sin filtrar `@softDelete` -- mismo criterio que `db.tableStats()`, a propósito distinto de `count()`.
- SQLite abre de solo lectura (`SQLITE_OPEN_READ_ONLY`); un archivo `.db` inexistente nunca es un error, es exactamente el caso "ninguna colección creada todavía".

Verificado con 5 tests de CLI contra el binario real (incluida una base REAL poblada por un `linkc serve` real) + 1 contra un Postgres real. Suite completa sin regresiones (1301 tests, +6 sobre v1.129.0). `db shell`/`export`/`import`/`seed` quedan para rondas futuras. Ver GRAMMAR.md §3.175, PLAN.md §9.7.

## [1.129.0] - 2026-08-28

### ✨ Nuevo
**`@unique(...) where <expr>`** -- cierra la mitad CONDICIONAL que §3.155 (v1.45.0) había dejado explícitamente afuera: el caso real citado ahí, el schema Drizzle de Glowapp (`UNIQUE(userId, appointmentDate, startTime) WHERE status != 'cancelled'`, permite reusar un horario una vez cancelado sin acumular filas basura).

```
@unique(userId, appointmentDate, startTime) where status != "cancelled"
type Appointment = { id: Int, userId: Int, appointmentDate: String, startTime: String, status: String }
```

- Reusa DIRECTO la infraestructura que `@check(<expr>)` de tipo (v1.128.0, esta misma sesión) acababa de construir -- misma validación de forma, mismo tipado contra `Bool`, misma traducción a SQL. Cero evaluador de aplicación nuevo: a diferencia de `@check`, `@unique` nunca tuvo enforcement de aplicación -- siempre fue puramente un constraint de base, y acá el índice simplemente se vuelve PARCIAL (`CREATE UNIQUE INDEX ... WHERE <condición>`, sintaxis idéntica en los dos backends).
- La condición puede referenciar cualquier campo del struct, no solo los del conjunto único.
- Bug encontrado y arreglado en el camino: el nombre determinístico del índice no podía concatenar la condición SQL tal cual (comillas/paréntesis rompían el identificador que lo envuelve) -- se hashea con el mismo SHA-256 que el sistema de módulos ya usaba para otra cosa, sin sumar una segunda implementación de hashing.
- El dedup de redundancia ahora es por `(campos, condición)`, no solo por campos -- dos `@unique` con los mismos campos pero condiciones distintas son dos constraints parciales legítimos.

Verificado con 7 tests de checker, 1 contra SQLite real (reproduce el caso exacto de Glowapp), 1 de DDL estático, 1 contra un Postgres real, y repetición en vivo contra SQLite (`.schema` + tres llamadas HTTP reales). Suite completa sin regresiones (1295 tests, +10 sobre v1.128.0). Ver GRAMMAR.md §3.174, PLAN.md §9.3.

## [1.128.0] - 2026-08-28

### ✨ Nuevo
**`@check(<expr>)` a nivel de `type`** -- cierra la mitad "expresión booleana arbitraria" que §3.96 (v1.60.0) había dejado pendiente: una comparación entre DOS campos del propio struct, no solo un rango numérico simple sobre un campo suelto. Complementa, sin reemplazar, el `@check(min/max/range/minLength/maxLength, ...)` de un solo campo ya existente.

```
@check(endDay > startDay)
type Booking = { id: Int, room: String, startDay: Int, endDay: Int }
```

- Acotado a lo que un `CHECK` de SQL puede expresar, no a "cualquier expresión de c-script": `ast::validate_check_expr_shape` rechaza en el checker -- antes de tipar nada -- cualquier forma que no sea identificador/literal/`!`/`-` unario/paréntesis/los operadores `==`/`!=`/`<`/`<=`/`>`/`>=`/`&&`/`||`/`+`/`-`/`*`/`/`/`%`. Ninguna llamada, acceso a `db`, closure, índice ni literal de struct/enum.
- Enforcement DOBLE, mismo criterio que el resto de `@check`: un `CHECK` real de TABLA (no de columna) en el `CREATE TABLE`, en los dos backends -- Y del lado de la aplicación, en los mismos dos puntos de entrada que el resto de las validaciones (wire y `StructLit` construido en el cuerpo de un rpc). El evaluador de aplicación es chico y autocontenido, pero reusa la misma aritmética/comparación (`checked_*`, NULL-segura) que el resto del intérprete.
- Un `applyPatch`/`Patch<T>` parcial saltea la expresión completa si le falta cualquier campo que referencia -- generaliza el mismo criterio de "ausente: nada que validar" que `@check` de un solo campo ya aplicaba.

Verificado con 9 tests de checker, 4 de runtime, 1 de DDL estático, 1 contra un Postgres real (acepta/rechaza con 400/rechaza un `INSERT` SQL crudo sin pasar por c-script), y repetición en vivo contra SQLite (`.schema` confirma el `CHECK` real, `sqlite3` rechaza un `INSERT` crudo). Suite completa sin regresiones (1285 tests, +15 sobre v1.127.0). Ver GRAMMAR.md §3.173, PLAN.md §9.3.

## [1.127.0] - 2026-08-27

### ✨ Nuevo
**Varios `db { ... }`, uno por módulo, se fusionan en un solo namespace de colecciones** -- cierra el último hueco genuinamente abierto del Pilar 3 (sistema de módulos) del roadmap de tres pilares que skynet-d3 relayó a nombre del usuario. Antes de esta ronda, un SEGUNDO `db { ... }` en el cierre transitivo de imports era un error duro sin importar sus nombres -- el único patrón que funcionaba era un `schema.link` central con el `db {}` que los módulos de servicio importaban.

```
// billing.link
db { invoices: Invoice[] }
service Billing { ... }

// crm.link
db { customers: Customer[] }
service Crm { ... }

// main.link
import "./billing.link";
import "./crm.link";
```

- Discovery primero: auditando quién consume un `Item::Db` apareció que, salvo el loop del checker que construye el mapa fusionado `db_collections`, TODO lo demás (codegen de Postgres, `linkc migrate`, el runtime) ya consumía exclusivamente ese mapa -- el cambio real quedó contenido en un solo lugar.
- Regla nueva: cualquier cantidad de `db {}` se fusiona; el único error duro que queda es un nombre de colección repetido (mismo criterio que `type`/`enum`/`fn`/`const` duplicados entre archivos), sin importar si las dos apariciones caen en el mismo bloque o en dos distintos. De paso se cerró un gap preexistente (un nombre repetido DENTRO de un solo bloque se perdía en silencio) y el gotcha de UX de la cascada de errores ya documentado.

Verificado con 3 tests nuevos en `checker.rs` + 1 en `modules.rs`, más repetición en vivo contra el binario real: `linkc build`/`linkc serve` con dos módulos reales generan el contrato y las dos tablas correctamente, cada `service` opera sobre la suya, y el caso de colisión da un solo error limpio sin cascada. Sigue abierto del Pilar 3: visibilidad `pub`/privado, sin evidencia de demanda propia. Ver GRAMMAR.md §3.172, PLAN.md §9.2.

## [1.126.0] - 2026-08-27

### ✨ Nuevo
**`countWhere`/`findWhere`/`deleteWhere` empujan comparaciones campo-vs-campo a SQL real** (`item.endDate > item.startDate`) -- cierra por completo el ítem 1 de PLAN.md §9.3 (el último hueco que §3.170/v1.125.0 había dejado explícito). Caso motivador: filtrar rangos de fecha inválidos sin traer la tabla entera a memoria.

- Acotado a propósito a los cuatro operadores relacionales (`<`/`<=`/`>`/`>=`) -- `==`/`!=` entre dos campos sigue sin pushear. El checker solo tipa la forma relacional cuando ambos lados son `Int`/`Int64`/`Float`/`Timestamp` sin `Optional`, y un campo no opcional siempre es columna `NOT NULL` -- así que esta forma nunca puede toparse con NULL para una tabla que c-script creó. `==`/`!=` sí permite comparar dos `T?`, donde `NULL = NULL` en SQL no es `true` como en el camino interpretado -- replicarlo habría necesitado `(a IS NULL AND b IS NULL) OR a = b` sin ningún caso real que lo pida, así que queda deliberadamente fuera (cae al camino interpretado, correcto siempre).
- `ast::PredicateOperand::Field`, `runtime::ConditionExpr::FieldPair` y `db.rs::field_pair_condition_sql` generan `"campoA" OP "campoB"` directo, sin ningún placeholder -- se integra al mismo recorrido recursivo de `&&`/`||` que cualquier otra hoja, sin caso especial adicional.

Verificado con los cuatro operadores, el caso mezclado con una hoja normal adentro de un `&&`, `deleteWhere` empujando la selección, y repetición en vivo contra un `linkc serve` real. Suite completa sin regresiones (1 fallo de test en la corrida completa fue el flake ambiental ya conocido de binding de puerto en Windows bajo paralelismo -- pasa limpio en aislamiento, no relacionado con este cambio). Ver GRAMMAR.md §3.171, PLAN.md §9.3.

## [1.125.0] - 2026-08-27

### ✨ Nuevo
**`countWhere`/`findWhere`/`deleteWhere`/`upsert` empujan `||` combinando condiciones a SQL real**, en cualquier profundidad de anidamiento con `&&` -- el hueco que §3.109 (v1.72.0) había dejado explícitamente documentado como pendiente. `a && b || c` respeta la precedencia real del lenguaje (`&&` liga más fuerte), reconociendo exactamente `(a && b) || c`.

- `ast::PredicateExpr` (árbol `Leaf`/`And`/`Or`) reemplaza la lista plana de hojas que solo sabía `&&` -- una cadena `a && b && c` sigue reconociéndose como un solo `And` de 3 hojas (sin paréntesis de más en el SQL generado), y un `||` en cualquier posición ahora también se reconoce, en vez de hacer fallar el reconocimiento entero y caer al camino interpretado (que sigue siendo correcto siempre, solo más lento).
- El `WHERE` generado parentiza cada hijo compuesto solo cuando es del tipo CONTRARIO al de su padre (`(b OR c)` adentro de un `AND`, o viceversa) -- nunca de más. El filtro de `@softDelete` se AND-ea correctamente incluso cuando el predicado de nivel superior es un `Or` (parentizando la disyunción entera primero).
- Mismo comportamiento NULL-seguro que la conjunción pura (`campo == variable` con `variable` resultando `null` en runtime se traduce a `IS NULL`, nunca a un `= ?` que en SQL nunca es cierto) ahora también dentro de una rama `||`.

Verificado con una disyunción pura, `&&` mezclado con `||` confirmando la precedencia exacta, una hoja NULL dentro de un `Or`, y repetición en vivo contra un `linkc serve` real (`countWhere`/`findWhere`/`deleteWhere` con predicados mixtos). Sigue sin cubrir: comparar dos campos del propio parámetro entre sí. Ver GRAMMAR.md §3.170, PLAN.md §9.3.

## [1.124.0] - 2026-08-27

### 🐛 Arreglado
Ronda 4 (última) de `AUDIT-FIX-PLAN-2026-08-27.md` -- cierra los 16 hallazgos de la tercera auditoría adversarial. 3 con código, 3 evaluados y documentados a propósito sin cambio de código:

- **`--jwt-secret ""` / `--service-api-key ""` (string vacío por flag) activaba la feature con secreto vacío.** El mismo filtro que ya aplicaba del lado de la env var no se aplicaba al valor de flag. Fix puntual en los dos `resolve_*`, sin tocar `read_flag_or_env` en sí (otros flags como `--host` tienen el contrato inverso deliberado: un valor vacío ahí es un error explícito).
- **Panics de tipo-incompatible decodificando filas de una tabla `--adopt-existing` con datos legado.** Tres sitios en `row_to_fields` asumían "esta fila la escribimos nosotros, con esta forma" -- un JSON guardado por una versión anterior del `.link` que ya no calza con el tipo actual, una columna JSON con una `Cell` física inesperada, o un tipo nativo declarado que no coincide con lo que la base tiene guardado (alcanzable de verdad: SQLite tiene afinidad de tipo, no enforcement). Los tres dan ahora el mismo `RuntimeError` limpio que el resto de la función.
- **`+`/`-`/`*` sobre `Int`/`Int64` (y el `-` unario, y `List<Int>.sum()`) seguían con aritmética cruda.** v1.119.0 solo había cerrado `/`/`%` -- en perfil `release` (los binarios publicados) un desborde de `+`/`-`/`*` wrappea EN SILENCIO, un bug de corrección, no solo de estabilidad. Generalizada la función que ya cubría `/`/`%` -- ya no queda ningún operador aritmético entero del lenguaje sin `checked_*`. Efecto colateral honesto: dos tests de rondas anteriores usaban desborde de `+` como disparador de un panic real para probar `catch_unwind` -- con `+` ahora protegido, ese disparador específico ya no panica, así que esos tests se actualizaron para reflejar la nueva realidad (siguen siendo regresiones válidas, ya no ejercitan el camino de panic).
- **`@cache` con la misma carrera que `@idempotent`**, **`@unique`/`@softDelete` sin índices parciales**, y **la composición check-then-act del lockout de login** se evaluaron y quedaron documentados como límites honestos en GRAMMAR.md, no atacados con apuro -- cada uno con su razonamiento explícito de por qué.

Con esto, `AUDIT-FIX-PLAN-2026-08-27.md` queda completo. Verificado con test unitario por hallazgo con código + repetición en vivo contra el binario real para los tres. Ver GRAMMAR.md §3.169.

## [1.123.0] - 2026-08-27

### 🐛 Arreglado
Ronda 3 de `AUDIT-FIX-PLAN-2026-08-27.md` (severidad media) -- los 6 hallazgos restantes de esa franja, en un solo paquete (mismo criterio que v1.119.0):

- **`insert()` panicaba en vez de dar `RuntimeError`** si la fila se borraba entre el INSERT y el SELECT de confirmación -- asimetría con `applyPatch`, que ya manejaba la carrera idéntica limpio. Mismo `.ok_or_else(...)`.
- **Agregaciones (`sumBy`/`countBy`/`avgBy`/`maxBy`/`minBy`) panicaban sobre una columna `NULL`** heredada de agregar un campo requerido a una colección Postgres con filas viejas (la migración nunca agrega `NOT NULL`, sin importar la opcionalidad declarada). Mismo `RuntimeError` limpio que la lectura normal ya usaba.
- **El checker aceptaba un rpc `@cron` como blanco de `@invalidates`** -- invalidación de caché muerta en silencio (`hooks.ts` con una llamada que ningún hook escribe jamás). Excluido explícitamente.
- **`linkc doc` no mostraba badges de auth/rate-limit/deprecated en un `stream`** -- documentación generada que desinformaba sobre qué está protegido. Unificado en una función compartida con el brazo `rpc`.
- **`GET /metrics` sostenía el candado de métricas mientras esperaba la conexión a la base** -- latencia/contención innecesaria bajo tráfico real combinado con transacciones largas. Reordenado.
- **`lint`: `mixed-service-auth` daba falso positivo** con un `@cron` al lado de rpcs protegidos, justo el patrón que `@cron` fue diseñado para soportar. Excluido del cálculo.

Un test unitario nuevo por hallazgo (6 en total) + repetición en vivo contra el binario real para los 3 que tienen repro directo (`@invalidates`+`@cron`, badges de `linkc doc`, falso positivo del lint). Suite completa sin regresiones (1257 tests, +6 sobre v1.122.0). Ver GRAMMAR.md §3.168, `AUDIT-FIX-PLAN-2026-08-27.md`.

## [1.122.0] - 2026-08-27

### 🔒 Seguridad / 🐛 Arreglado
Tercera auditoría adversarial de la sesión (5 agentes `Explore` read-only en paralelo, uno por capa: concurrencia/panics, consistencia de codegen, auth/secretos, superficie de `.unwrap()`/panic, capa SQL/DB). 16 hallazgos documentados en `AUDIT-2026-08-27.md`, priorizados en `AUDIT-FIX-PLAN-2026-08-27.md`. Rondas 1 y 2 del plan (severidad crítica y alta) se cierran acá:

- **`crypto.randomToken(length)` con `length` negativo o gigante mataba el proceso `linkc serve` ENTERO.** `*n as usize` sobre un negativo reinterpreta los bits como un `usize` gigante; sin ningún techo, el pedido de memoria resultante hacía que Rust llamara a `handle_alloc_error` → `std::process::abort()` -- ni siquiera `catch_unwind` puede evitarlo. Reproducido en vivo: una sola request `{"length": 9223372036854775807}` a un rpc que expone `crypto.randomToken(length)` tumbaba el proceso entero (todos los servicios coexistiendo bajo `serve-all` incluidos), sin necesitar autenticación si el rpc no la exigía. **Fix**: `length` se valida contra `1..=1024` antes de tocar memoria.
- **`@cache` + `@authenticated`/`@requires` filtraba datos de un usuario autenticado hacia otro.** La clave de caché (`(service, rpc, argumentos)`) nunca incluyó la sesión del caller. Reproducido en vivo: Bob, con su propio token de sesión válido, recibía el perfil completo de Alice (un `myProfile()` cacheado que lee `auth.currentUserId()`) en vez del suyo. **Fix**: rechazado en compilación -- combinar `@cache` con `@authenticated`/`@requires` en el mismo rpc es ahora un error del checker, hasta que exista un diseño real de scoping por sesión.
- **`Patch<T>`/`applyPatch` nunca aplicaba `@validate`/`@check`.** `json_to_typed_value` tiene dos caminos que construyen un struct desde el wire; solo el de un struct COMPLETO llamaba a `apply_field_validators`, el de `Patch<T>` (la forma canónica de actualización parcial) se lo saltaba entero -- y `@validate` no tiene ningún respaldo de DDL, así que era el único punto de enforcement en todo el sistema. Reproducido en vivo: `create` con un email inválido daba 400, `update` con el mismo valor daba 200 y lo persistía. **Fix**: `Type::PatchOf` ahora también corre `apply_field_validators` -- la función ya toleraba valores parciales, ningún cambio de semántica.
- **`@idempotent` tenía una carrera TOCTOU real -- doble ejecución con la misma `Idempotency-Key`.** `lookup`+`store` eran dos candados separados con el cuerpo del rpc corriendo sin ninguno sostenido entre medio. Reproducido en vivo: 30 requests concurrentes con la misma clave insertaron 2 filas para un solo cargo. **Fix**: `reserve` (revisar + marcar en vuelo, atómico bajo un único candado) reemplaza a `lookup` -- un segundo `reserve` sobre una clave todavía en vuelo da `409` sin correr el cuerpo, mismo criterio que la API real de Stripe. Una marca huérfana (el hilo que la reservó murió sin liberarla) se autolibera después de 120s.

Verificado con hilos de sistema operativo reales en los dos últimos casos (no solo tests unitarios) + repetición en vivo de los 4 repros exactos del audit contra un `linkc serve` real. Los 12 hallazgos restantes (media/baja severidad) quedan documentados y priorizados para rondas siguientes -- no todos entran en un solo paquete de bugfix. Ver GRAMMAR.md §3.165/§3.166/§3.167, PLAN.md §9.5, `AUDIT-FIX-PLAN-2026-08-27.md`.

## [1.121.0] - 2026-08-27

### 🔧 Proceso
Sin cambios de código sobre v1.120.0 -- ese tag quedó con CI en rojo por un error de proceso propio: el comando usado para regenerar `examples/users.link.snap` (`linkc test examples/users.link --update`) omitió el segundo argumento posicional del snapshot, así que no lo tocó de verdad -- el paso de CI que compara SIN `--update` ("Contrato del demo insignia sin deriva sin querer", GRAMMAR.md §3.29) lo agarró. El release/tag v1.120.0 ya estaba publicado con binarios reales (0 descargas) cuando se encontró -- borrarlo/re-taggearlo requería una acción bloqueada por el clasificador de auto mode por ser demasiado destructiva/visible sin confirmación explícita del usuario, que eligió avanzar en vez de forzarla. v1.120.0 queda en el historial con CI rojo mencionado acá para que quede claro por qué; su código es idéntico al de v1.121.0. Ver v1.120.0 para el changelog real de esta ronda (`catch_unwind` en `transaction { }` y `@cron`).

## [1.120.0] - 2026-08-27

### 🐛 Arreglado
Cierra los dos límites que la auditoría de v1.119.0 había dejado explícitamente abiertos (GRAMMAR.md §3.162): ningún panic real (no un `RuntimeError`) dentro de `transaction { }` ni dentro de una corrida de `@cron` tenía un camino de limpieza -- el fix de v1.119.0 solo tapaba el disparador de panic más alcanzable (división/resto entero por cero).

- **`catch_unwind` alrededor del cuerpo de `transaction { }`.** Antes: cualquier otro panic (un `.expect()` en `db.rs`/`store.rs`, un desborde de `+`/`-`/`*`, lo que sea) dejaba el hilo de la request morir en el unwind sin pasar por `rollback_transaction` -- el `BEGIN` se quedaba abierto sobre la conexión compartida para siempre, toda transacción futura del proceso fallaba con "ya hay una transacción abierta", y escrituras posteriores confirmadas al cliente se perdían en silencio al reiniciar (el mismo escenario de pérdida de datos de v1.119.0, con cualquier otro disparador). Ahora un panic atrapado se traduce a un `RuntimeError` normal y toma el mismo camino de `rollback_transaction()` que cualquier otro error del cuerpo.
- **`catch_unwind` alrededor de cada corrida de `@cron`.** El comentario del scheduler siempre prometió "una corrida fallida nunca apaga la tarea entera", pero eso era falso para un panic real: atraviesa el `match Ok/Err` sin tocarlo y se lleva puesto TODO el hilo del scheduler -- la tarea dejaba de correr para siempre, sin ninguna línea de log ni entrada de métrica, indistinguible desde afuera de "todavía no le tocaba el turno". Ahora un panic cuenta como falla en `/metrics` (`linkc_cron_failures_total`) igual que un `RuntimeError`, y el `loop` sigue durmiendo y reintentando en el próximo intervalo.

Ambos comparten el mismo helper para extraer un mensaje legible del payload del panic. Verificado con un desborde real de `+` sobre `i64` como disparador (código de producción sin arreglar a propósito, para probar el `catch_unwind` contra un panic genérico en vez de repetir el caso de división por cero ya cerrado) -- incluyendo, para `@cron`, contra un `linkc serve` real: `linkc_cron_failures_total` sigue creciendo con el tiempo en vez de quedarse clavado tras la primera corrida. Los dos tests nuevos están gateados con `#[cfg(debug_assertions)]`: el desborde solo panica con `overflow-checks` activo (perfil `dev`, lo que corre `cargo test`/CI); en `release` simplemente wrappea, sin nada que atrapar. Ver GRAMMAR.md §3.163/§3.164, PLAN.md §9.2.

## [1.119.0] - 2026-08-27

### 🐛 Arreglado
Segunda auditoría adversarial de la sesión, esta vez con un agente **read-only** (no puede editar código) sobre las cuatro versiones ya shippeadas (v1.114.0-v1.117.0) más el cambio en vuelo. Encontró 3 bugs REALES, los tres reproducidos a mano contra el binario real antes de tocar nada, y **dos de ellos introducidos por los propios fixes de v1.115.0/v1.116.0** -- el precio de haber tocado concurrencia:

- **Deadlock que dejaba el servidor vivo pero permanentemente colgado** (introducido en v1.115.0). Los dos fixes de esa versión, combinados, crearon un orden de candados cruzado: `subscribe()` pasó a sostener el candado de suscriptores mientras pide el de la conexión (vía `select_rows`), y `upsert` pasó a sostener el de la conexión mientras publica (que pide el de suscriptores). Reproducido con `upsert` y un `stream` concurrentes sobre la misma colección: `ping` seguía respondiendo 200 pero `health`, `/metrics` y toda escritura no volvían nunca; solo se recuperaba matando el proceso. **Fix**: `subscribe()` registra al suscriptor PRIMERO, suelta el candado, y recién después saca la foto -- nunca sostiene los dos. Preserva la garantía de no perder filas que v1.115.0 buscaba (un evento duplicado es inofensivo; una fila perdida no) y es más simple que lo que hacía antes. Verificado con el mismo martillo que lo colgaba: ahora todo responde 200.
- **`@cron` rompía el TypeScript generado** (introducido en v1.116.0). De los seis emisores de codegen, `emit_service_interface` era el único que se había quedado sin el filtro de `@cron` -- así que la interfaz declaraba un método que la clase que hace `implements` nunca define: **TS2420**, error de compilación en cualquier proyecto con una tarea `@cron`. Confirmado con el `tsc` real del propio repo. Exactamente la clase de bug que el proyecto existe para prevenir.
- **División entera por cero era un PANIC de Rust, no un error de runtime** (preexistente, pero mucho más grave desde v1.114.0). `a / 0` y `i64::MIN / -1` panicaban; el divisor casi siempre viene de datos del usuario. Con un hilo por request el panic ya no mata el proceso, pero mata el hilo SIN pasar por ningún camino de limpieza: adentro de un `transaction { }` dejaba la transacción SQL abierta para siempre. Reproducido de punta a punta **con pérdida silenciosa de datos**: tras el panic, toda transacción futura fallaba con "ya hay una transacción abierta", escrituras posteriores se confirmaban al cliente con 200, y al reiniciar el proceso el servidor pasaba de reportar 3 filas a tener 1 -- dos escrituras ya confirmadas, descartadas en silencio. **Fix**: `/` y `%` sobre enteros usan `checked_div`/`checked_rem` y devuelven un error de runtime limpio (500, el hilo sobrevive, el `transaction{}` rollbackea normal y la base queda usable). El camino de `Float` no cambia -- IEEE-754 ya define `/0` como infinito/NaN.

6 tests de regresión nuevos, todos con hilos de sistema operativo reales donde aplica. Ver GRAMMAR.md §3.162.

## [1.118.0] - 2026-08-27

### ✨ Nuevo
- **`import "./modulo.link";` — import "solo por efecto", sin llaves ni `from`.** Cierra el último hueco real para partir un programa en módulos: un módulo que solo aporta un `service` ahora se puede cargar directamente.
- **El discovery que motivó esto corrigió el propio PLAN.md.** El plan listaba "Pilar 3, sistema de módulos" como pendiente de discovery, con dos preguntas abiertas. Auditar el código antes de diseñar nada mostró que el sistema de módulos ya existía y era mucho más completo de lo que el plan reflejaba (imports multi-archivo, `link.json`, dependencias git reales, `link.lock`, ciclos, caso diamante) y que las dos preguntas ya tenían respuesta en el código.
- **El hueco real, medido y no supuesto**: `service` no es importable por nombre (a propósito) y no existía forma de import sin nombres, así que componer un programa a partir de módulos con servicios obligaba a declarar un tipo-fantasma en cada uno solo para tener algo que importar — y ese fantasma se filtraba al contrato público generado, como `export interface` en `contract.d.ts` Y como schema de Zod en `schemas.ts`. Confirmado inspeccionando el `gen/` real, antes y después.
- Puramente aditivo: la forma con llaves no cambia, las dos conviven en el mismo archivo (el parser decide por el token que sigue a `import`). Resolución de `from`, detección de ciclos, errores de sintaxis por archivo: todo idéntico.
- Límites honestos sin cambios: sigue habiendo **un solo `db {}` por programa** (varios módulos no pueden ser dueños de sus colecciones — decisión de diseño con peso propio, no atacada), sin `pub`/privado, sin re-exports.

Verificado con 4 tests unitarios nuevos + verificación manual contra el binario real (proyecto multi-módulo completo generando un contrato sin ningún tipo-fantasma, comparado lado a lado contra el workaround anterior) + 4 formas malformadas de import confirmadas como errores limpios. Ver GRAMMAR.md §2.1/§3.161, PLAN.md §9.2.

## [1.117.0] - 2026-08-27

### ✨ Nuevo
- **`http.postWithRetry(url, body, headers, maxAttempts)`: reintentos con backoff para webhooks salientes.** PLAN.md §9.4 ítem 2 -- firmar un webhook saliente ya funcionaba sin ningún primitivo nuevo (`crypto.hmacSha256` + `http.postWithHeaders`); el gap real (ya documentado como pendiente en GRAMMAR.md §3.86) era que ninguna llamada `http.*` reintentaba sola ante una falla transitoria.
- Nuevo método en el namespace `http` ya existente -- `maxAttempts: Int` es el único parámetro nuevo. Backoff exponencial FIJO (200ms doblando, techo de 5s, no configurable, mismo criterio que `MAX_WHILE_ITERATIONS`) -- mucho más corto que el techo de 30s de `--restart-backoff`, porque esto bloquea el hilo de UNA request, no un proceso servidor entero.
- Reintenta ante cualquier falla (red o status no-2xx, mismo criterio que `post`/`postWithHeaders`) -- sin distinguir 4xx de 5xx todavía, alcance v0. `maxAttempts <= 0` es un error de runtime limpio antes de mandar ninguna request real.

Verificado con 3 tests de integración contra un servidor de mentira real + 1 test unitario de la progresión del backoff. Suite completa sin regresiones. Ver GRAMMAR.md §3.160, PLAN.md §9.4.

## [1.116.0] - 2026-08-27

### ✨ Nuevo
- **`@cron("Ns"/"Nm"/"Nh"/"Nd")`: tareas recurrentes nativas dentro de `linkc serve`.** Reprorizado el 24/08/2026 por evidencia fuerte de Glowapp (10+ schedulers hand-rolled con `setInterval` más un `schedulerSupervisor.ts` completo), atacado recién ahora porque necesitaba la infraestructura de hilos reales de v1.114.0 (un hilo por request) -- antes de esa ronda, esto hubiera significado inventar concurrencia de un solo uso.
- Una anotación sobre un `rpc` normal (`@cron("5m")`), no una palabra reservada nueva. Tiene que ser la ÚNICA anotación del rpc (ninguna otra -- `@route`/`@authenticated`/`@rate_limit`/etc. -- tiene efecto sobre algo que nunca recibe una request HTTP real), sin parámetros, retorno `Void` obligatorio.
- Nunca alcanzable vía HTTP -- ni en su path por defecto (404 explícito, no solo ausencia de `@route`), ni en `client.ts`/`openapi.json`/`llms.txt`/hooks generados.
- Un hilo de sistema operativo dedicado por tarea, spawneado una vez al arrancar `serve()`, reusando `Arc<Db>`/`Arc<Program>`/`Arc<SessionStore>` -- sin scheduling nuevo. Duerme el intervalo completo antes de la primera corrida (mismo criterio que `setInterval` de JS). Un error del cuerpo se loguea y el loop sigue -- nunca apaga la tarea ni el servidor.
- Observabilidad: una línea de log por corrida (`log_cron_tick`) y dos contadores nuevos en `/metrics` (`linkc_cron_runs_total`/`linkc_cron_failures_total`, el segundo solo si hubo una falla real).
- Límites honestos documentados: sin coordinación entre instancias (N réplicas corren la tarea N veces), sin catch-up tras downtime, sin disparo manual, sin guard contra solapamiento entre corridas.

Verificado con 9 tests de checker + 2 de parseo + 2 de integración contra un `linkc serve` real (subproceso real, confirmando que corre sola y que da 404 en su path por defecto) + 1 de `/metrics` real. Suite completa: 1229 tests, 0 fallos. Ver GRAMMAR.md §3.159, PLAN.md §9.7.

## [1.115.0] - 2026-08-26

### 🐛 Arreglado
Auditoría propia de GRAMMAR.md tras shippear v1.114.0 (un hilo real por request): varias secciones documentaban invariantes de concurrencia que dependían de "el servidor procesa una request a la vez" -- releerlas con la premisa nueva hizo aparecer 3 bugs reales, ninguno reportado externamente, los 3 arreglados y verificados el mismo día:
- **`Db::subscribe` podía perder en silencio una fila insertada concurrentemente.** Sacaba la foto y RECIÉN DESPUÉS se registraba como suscriptor, dos pasos sin candado compartido con `publish`/`deliver_local` -- un `insert`/`applyPatch` de otro hilo podía commitear y publicar exactamente en esa ventana, sin quedar ni en la foto ni en el canal. Fix: registrar el sender y sacar la foto bajo el mismo candado que usa `deliver_local` para entregar.
- **Ese mismo fix casi introduce un deadlock**: si `commit_transaction` entregara sus eventos diferidos con el candado de la conexión todavía tomado (como hacía antes), un `transaction{}` confirmando y un `subscribe()` concurrente a la misma colección pedirían esos dos candados en órdenes opuestos. Fix: `commit_transaction` ahora devuelve la lista de eventos pendientes en vez de entregarlos, y `Expr::Transaction` los entrega después de soltar el candado de la conexión.
- **`upsert` podía duplicar una fila bajo el mismo `matchFn` concurrente.** Buscar la fila existente y decidir insert-o-patch eran dos pasos separados, sin candado compartido -- dos hilos podían ver "no hay match" a la vez y los dos insertar. Fix: `upsert` entero corre bajo `Db::with_exclusive_connection`, el mismo candado reentrante que ya usa `transaction{}`.

Los 3 bugs tienen test de regresión nuevo con hilos de sistema operativo reales (`std::thread::spawn`/`std::sync::Barrier` forzando la carrera) -- cada uno confirmado que reproduce el fallo original revirtiendo el fix a mano antes de restaurarlo, incluido un test que literalmente se cuelga (deadlock real) con el orden de entrega viejo. Suite completa: 1216 tests, 0 fallos. Ver GRAMMAR.md §3.16, §3.75, §3.154, §3.158.

## [1.114.0] - 2026-08-26

### ✨ Nuevo
- **`linkc serve` pasa de un solo hilo a un hilo real por request -- Pilar 1 de un roadmap de concurrencia mayor.** Propuesto por skynet-d3 a nombre del usuario, evaluado por escrito antes de escribir una línea de código (dos caminos comparados: reescritura completa a `tokio`/`async`, descartada por el "function coloring" que propagaría `async fn` por todo el intérprete; hilo-por-request con candado reentrante sobre la conexión, elegido por su riesgo mucho más acotado) y arrancado con autorización explícita del usuario sobre esa propuesta.
- Cada request ahora corre en su propio `std::thread::spawn`, con `Db`/`Program`/`SessionStore`/route table compartidos vía `Arc` y cada store mutable (rate limiter, idempotency, cache, métricas) vía `Arc<parking_lot::Mutex<...>>`.
- La conexión SQL usa `parking_lot::ReentrantMutex` a propósito -- `transaction{}` mantiene el candado tomado durante BEGIN+cuerpo+COMMIT/ROLLBACK, y el cuerpo vuelve a pedir el mismo candado en cada operación individual; un `Mutex` no reentrante se autobloquearía en el mismo hilo. Estado genuinamente por-request (contexto de la request actual, overrides de status/location de `response`) pasa a `thread_local!` en vez de forzarse a un `Mutex` compartido.
- `Checker::in_stream_body`/`in_transaction`/`hover_result` (embebidos en `Db` para resolución de tipos en runtime) se convirtieron con primitivos de `std::sync`, no `parking_lot` -- `checker.rs` sigue compilando a `wasm32-unknown-unknown` sin el feature `runtime`.
- Verificado con dos tests permanentes de concurrencia real (`std::thread::spawn`, estables en 5 corridas): 40 inserts concurrentes nunca pierden ni duplican una fila; 40 `transaction{}` concurrentes sobre la misma fila nunca pierden un update. Verificación manual contra `linkc serve` real (SQLite y Postgres) confirma la ganancia de paralelismo motivadora: 5 llamadas HTTP lentas de 2s, 11.3s en secuencial vs. 2.3s concurrentes.
- Pilares 2 (FFI tipado a crates de Rust) y 3 (sistema de módulos/paquetes) quedan explícitamente pendientes de su propio discovery. Ver GRAMMAR.md §3.158, PLAN.md §9.2.

## [1.113.0] - 2026-08-26

### 🐛 Arreglado
Con el pedido explícito del usuario, se corrió una auditoría multi-agente adversarial (modo "ultracode") sobre las 6 features shippeadas en esta sesión (v1.107.0-v1.112.0): 6 agentes en paralelo, uno por feature, más una fase de verificación independiente de cada hallazgo antes de reportarlo. 5 de las 6 features auditadas tenían un bug real confirmado -- ninguno reportado externamente, todos encontrados y arreglados en la misma ronda:
- **`upsert`/`findWhere`/`countWhere`/`deleteWhere` pusheados a SQL rompían la semántica de igualdad con NULL** -- `"campo" = ?` ligado a un parámetro NULL nunca es cierto en SQL, mientras el camino interpretado trata `Null == Null` como `true`. En `upsert` esto insertaba una fila DUPLICADA en vez de actualizar una existente con ese campo en NULL. Fix: `IS [NOT] NULL` para una hoja `==`/`!=` cuyo operando resultó NULL.
- **`transaction` anidada, alcanzada a través de una llamada a función auxiliar** (no anidamiento sintáctico directo), compilaba limpio y fallaba en runtime con el error crudo del backend en vez de un mensaje claro. Fix: chequeo explícito antes del `BEGIN` real.
- **Nombre de índice de `@unique` compuesto ambiguo**: dos constraints cuyos nombres de campo concatenados coincidían (`@unique(a_b, c)` vs `@unique(a, b_c)`) generaban el MISMO nombre de índice, y `CREATE UNIQUE INDEX IF NOT EXISTS` volvía el segundo un no-op silencioso -- su constraint nunca se enforcaba de verdad. Fix: codificación con prefijo de longitud, inyectiva por construcción.
- **Revivido de `Union` con un miembro `Int64` nunca revivía ese miembro** -- la disambiguación validaba contra el valor SIN revivir con un chequeo que ya asumía post-revivido, así que un `Int64 | String` quedaba como string para siempre, en silencio, con la validación pasando igual. Fix: revivir cada candidato primero (con `try/catch`), validar después.
- **Truncado de fecha en SQLite (`truncateToDay`/etc.) usaba división entera** en vez de real -- para un epoch pre-1970 con resto de milisegundos, redondeaba al día equivocado; Postgres ya hacía la división real y daba el resultado correcto, así que los dos backends discrepaban por un día entero en ese caso puntual. Fix: `/1000.0`.

Los 5 bugs tienen test de regresión nuevo (unitario y, donde aplica, contra Postgres real) y quedaron documentados en detalle en sus respectivas secciones de GRAMMAR.md (§3.75, §3.154, §3.155, §3.156, §3.157).

## [1.112.0] - 2026-08-26

### ✨ Nuevo
- **`.truncateToDay()`/`.truncateToMonth()`/`.truncateToYear()`: agregación agrupada por fecha.** Cuarto ítem de la auditoría propia de Glowapp: §3.65 dejaba documentado a propósito que agrupar por un `Timestamp` sin truncar produce un grupo por fila -- esta ronda agrega el método de truncado que faltaba, reconocido SOLO sintácticamente en el selector de clave de `sumBy`/`countBy`/`avgBy`/`maxBy`/`minBy` (la única posición de todo el lenguaje donde un método existe sobre `Timestamp`, nunca evaluado como llamada real). SQL específico por backend -- SQLite con `strftime`/`'start of day'` etc., Postgres con `date_trunc(unit, ts, 'UTC')` (el overload de 3 argumentos, para no depender en silencio del `TimeZone` de la sesión) -- ambos devolviendo milisegundos-desde-epoch planos. Bug real encontrado en la verificación manual: `scalar_cell_to_value` no tenía brazo para `Timestamp` (nunca hizo falta antes), así que la clave truncada viajaba como número en el JSON en vez de string ISO-8601 -- corregido con el mismo criterio que ya se usaba para `Int64`. Verificado con tests de checker, runtime contra SQLite real, integración contra Postgres real, y una verificación manual con fecha pre-1970 en los dos motores. Ver GRAMMAR.md §3.157.

## [1.111.0] - 2026-08-26

### ✨ Nuevo
- **`Int64` como `bigint` real en `client.ts`.** Tercer ítem de la auditoría propia de Glowapp: §3.30 (v1.35.0) dejaba `Int64` emitido como `string` en TypeScript a propósito ("cambiar a bigint sería arquitectura nueva"). Esta ronda construye esa arquitectura: `contract.d.ts`/`client.ts`/`hooks.ts` ahora declaran `bigint` de verdad, y `validators.ts` gana un segundo juego de funciones (`reviveX`, junto a los `isX` de siempre) que convierte cada `Int64` alcanzable (struct, `Optional`/`List`/`Tuple`/`MapOf`/`Union`/`Result`/`Patch`, `Generic`/`enum` expandidos de verdad) de string a `bigint` justo después de `res.json()`. El wire NO cambia -- sigue siendo string en las dos direcciones; del lado de ida, un replacer estructural de `JSON.stringify` (`__int64SafeStringify`) vuelve cualquier `bigint` saliente a texto sin ambigüedad. Ambos helpers se emiten solo si el programa realmente usa `Int64` -- cero costo para el caso común. Verificado con tests unitarios nuevos, `tsc --strict --noUnusedLocals` real sobre el código generado, y un `linkc serve` real ida y vuelta con `i64::MAX` exacto por HTTP. Ver GRAMMAR.md §3.156.

## [1.110.0] - 2026-08-26

### 🐛 Arreglado
- **`while` real dentro de un bloque `test { }` fallaba siempre en su primera vuelta.** `run_tests_core` (el runner de `linkc test`) inicializaba el contador compartido de iteraciones de `while` (`MAX_WHILE_ITERATIONS = 1_000_000`, GRAMMAR.md §3.15) directamente en el propio tope en vez de en cero -- el camino normal de `rpc` (`invoke_rpc_with_sessions`) sí lo inicializaba bien. Efecto: la primerísima vuelta de CUALQUIER `while` dentro de un `test` empujaba el contador por encima del tope y disparaba de inmediato "límite de 1000000 iteraciones excedido -- posible loop infinito", sin que el loop hubiera corrido de verdad -- el propio ejemplo canónico de §3.15 (sumar una lista con `while`) fallaba siempre invocado desde un test, funcionando perfecto desde `serve`. Encontrado verificando (no asumiendo) un reporte externo de un `while` "colgado" solo bajo el test runner. Fix de una línea; test de regresión nuevo en `tests/cli_test_runner.rs` que cubre tanto el caso corto (debe pasar) como un `while true` genuino (debe seguir cortando).

## [1.109.0] - 2026-08-26

### ✨ Nuevo
- **`@unique(campo1, campo2, ...)`: constraint UNIQUE compuesto a nivel de `type`.** Segundo ítem del pedido de discovery de migración (Glowapp, vía auditoría propia): `@unique`/`@index` de campo (v1.45.0) resuelven "este valor no se repite en toda la tabla", pero un caso real muy común -- "un slug único POR PERFIL, no globalmente" -- necesita un constraint sobre VARIOS campos a la vez.

  ```
  @unique(profileId, slug)
  type Product = { id: Int, profileId: Int, slug: String, name: String }
  ```

  `TypeDecl` gana `annotations: Vec<TypeAnnotation>` (enum propio, mismo criterio que `FieldAnnotation`/`Annotation`). Al menos 2 campos, cada uno tiene que existir de verdad, sin repetidos dentro del mismo `@unique`, sin declararse sobre un `type` que no sea struct, y sin dos `@unique` con el mismo conjunto de campos (redundante). DDL idéntico en los dos backends, emitido tanto por el runtime real como por `linkc build`/`linkc migrate --dry-run`.

### 🐛 Arreglado
- **Una violación de `@unique`/`@check` contra Postgres real daba 500, no el 400 documentado desde v1.45.0/v1.60.0.** Bug preexistente, encontrado verificando a mano el ítem de arriba: `postgres::Error::to_string()` para un error devuelto por el servidor es el literal fijo `"db error"`, sin el mensaje real -- el chequeo por substring de mensaje (`is_unique_violation`/`is_check_violation`) nunca matcheaba nada real contra ese backend, así que TODA violación contra Postgres caía como 500 genérico, en silencio, desde que esas dos anotaciones existen. Arreglado clasificando por **SQLSTATE** (`23505`/`23514`) en vez de por el mensaje humano -- el código nunca se traduce, a diferencia del mensaje, que SÍ está localizado según `lc_messages` del servidor (confirmado con un Postgres de prueba corriendo en español: "llave duplicada viola restricción de unicidad", no "duplicate key...").

1191 tests (12 nuevos): 8 de checker, 1 en `runtime/mod.rs` contra SQLite real, 1 en `codegen::postgres_emit` confirmando el DDL estático, y 2 contra un Postgres REAL (`pg_integration.rs`) -- uno del constraint compuesto de punta a punta, otro dedicado al fix del bug de status code confirmando el 400 real por HTTP. Detalle completo: GRAMMAR.md §3.155, PLAN.md §9.3.

## [1.108.0] - 2026-08-26

### ✨ Nuevo
- **`transaction { ... }`: transacciones SQL multi-escritura reales.** Pedido real de un adoptador en fase de discovery de migración (IgnisLove, coordinado vía otra sesión de Claude en su VPS de producción), confirmado como el ÚNICO bloqueo real -- no de conveniencia -- para migrar un flujo de checkout/pedidos completo: "crear pedido + descontar stock + cerrar carrito, con rollback si falla algo" no tenía forma segura de expresarse en un `.link`, porque cada escritura (`insert`/`applyPatch`/`delete`/`increment`) es autocommit individual.

  ```
  rpc checkout(productId: Int, qty: Int) -> Order {
    transaction {
      let matches = db.stock.findWhere(|s: Stock| { s.productId == productId });
      if matches.length() == 0 { panic("sin stock para ese producto"); } else { }
      let s = matches[0];
      if s.quantity < qty { panic("stock insuficiente"); } else { }
      db.stock.increment(s.id, |x: Stock| { x.quantity }, 0 - qty);
      db.orders.insert(Order { id: 0, productId: productId, qty: qty })
    }
  }
  ```

  `transaction { ... }` es una expresión de bloque, misma familia que `if`/`match` -- `BEGIN` real antes del cuerpo, `COMMIT` si termina de correr normal, `ROLLBACK` automático si cualquier error de runtime se propaga desde adentro (`panic`, una violación de `@check`/`@unique`, lo que sea). `panic(...)` (ya existente) es el mecanismo para abortar por una regla de negocio -- sin ningún `db.rollback()` nuevo, sin superficie de lenguaje extra. La publicación a `stream` de cada escritura se DIFIERE hasta el `COMMIT` -- el punto no negociable del diseño: un rollback nunca le miente a un suscriptor en vivo sobre una fila que la base terminó descartando. Sin anidamiento ni `return` en el cuerpo (v0, mismo criterio que `while`) -- los dos se rechazan en compilación. Los dos backends (SQLite/Postgres) comparten la misma implementación, sin código nuevo por motor.

  **`transaction` pasa a ser palabra reservada** (mismo criterio que `test`/`while`/`match`) -- sigue siendo válida como nombre de CAMPO (`type Log = { transaction: String }`), igual que otras palabras clave del lenguaje, pero ya no puede usarse como nombre de variable/función.

1179 tests (12 nuevos): 6 de checker (tipa contra el retorno del rpc, se checkea contra `Void` en posición de sentencia, anidamiento rechazado, `return` directo y anidado rechazados, síntesis sin contexto rechazada), 3 de runtime contra SQLite real (commit completo verificado por conteo real de filas, rollback completo -- ninguna escritura sobrevive un `panic` a mitad de camino --, la base sigue perfectamente usable después de un rollback), 2 de integración en `cli_transaction.rs` contra el binario real con un `stream` conectado por un socket de verdad (un rollback NUNCA genera un evento SSE, un commit genera exactamente uno), y 1 test contra un Postgres REAL confirmando que `BEGIN`/`COMMIT`/`ROLLBACK` funcionan igual en ese backend. Detalle completo: GRAMMAR.md §3.154, PLAN.md §9.3/§8.2.3.

## [1.107.0] - 2026-08-26

### ✨ Nuevo
- **`upsert` empuja `matchFn` a SQL cuando es pusheable.** Sexto y último hallazgo del barrido de "límites honestos" de GRAMMAR.md: `db.<c>.upsert(matchFn, insertValue, updateFn)` traía la colección ENTERA a memoria para evaluar `matchFn` sobre cada fila -- una colección que crecía de cientos a decenas de miles de filas hacía que un `upsert` antes instantáneo empezara a tardar segundos, sin ningún error ni aviso. Se notaba por quejas de latencia, nunca por el compilador.

  Mismo criterio que `findWhere`/`countWhere`/`deleteWhere` (§3.95/§3.108/§3.109/§3.145): si `matchFn` tiene la forma `|x| x.campo == valor` (o una conjunción `&&` de varias hojas así), la selección se empuja a `find_where_conjunction` real en vez de traer la tabla entera. Cualquier otra forma (`||`, comparar dos campos entre sí) sigue funcionando exactamente igual que antes, vía el camino interpretado -- sin ningún cambio de comportamiento observable, solo sin el atajo de SQL.

1167 tests (2 nuevos) en `runtime/mod.rs` (un `matchFn` no pusheable sigue funcionando idéntico al de siempre) y `pg_integration.rs` contra Postgres real (el camino pusheado genera SQL válido en ese backend, no solo SQLite). Detalle completo: GRAMMAR.md §3.75, PLAN.md §9.3.

Con esto quedan cerrados los seis landmines identificados en el barrido de "límites honestos" iniciado tras el incidente de puertos de IgnisLove (v1.102.0).

## [1.106.0] - 2026-08-26

### ✨ Nuevo
- **`linkc_rate_limit_rejections_total{method="..."}` en `/metrics`.** Quinto hallazgo del barrido de "límites honestos" de GRAMMAR.md: `@rate_limit` vive en memoria POR PROCESO (ya documentado como límite honesto desde v1.39.0) -- correr N réplicas detrás de un balanceador diluye el límite real sin ningún aviso, así que un endpoint caro (email, cobro) protegido "en el papel" puede estar recibiendo N veces más tráfico real del que su `.link` pidió, sin que nadie lo note hasta que ya duele.

  No arregla la dilución en sí -- eso necesitaría estado compartido entre procesos (Redis, o una tabla Postgres con incremento atómico), una pieza bastante más grande y sin evidencia real de demanda todavía. Lo que sí hace: cuenta cada `429` real por rpc y lo expone en `/metrics`, el mismo lugar que un operador ya mira -- agregable entre réplicas con una consulta Prometheus normal (`sum by (method) (linkc_rate_limit_rejections_total)`), convirtiendo un problema silencioso en una señal visible.

1165 tests (2 nuevos) en `metrics.rs` (se acumula por rpc, no aparece hasta el primer rechazo) y `cli_metrics.rs` contra el binario real: un rpc con `@rate_limit("1/1h")` golpeado tres veces confirma exactamente 2 rechazos reales en el contador. Detalle completo: GRAMMAR.md §3.39/§3.149, PLAN.md §9.8.

## [1.105.0] - 2026-08-26

### ✨ Nuevo
- **`linkc_notify_oversized_dropped_total` en `/metrics`.** Tercer hallazgo del barrido de "límites honestos" de GRAMMAR.md: un payload NOTIFY de más de 8000 bytes (el límite real de PostgreSQL) se descarta PARA SIEMPRE -- correcto, reintentarlo no lo arreglaría -- pero hasta esta ronda la única señal era un `eprintln!` por stderr, invisible corriendo desatendido bajo `pm2`/`systemd` sin revisar logs. Una colección con filas grandes (un catálogo de facets/búsqueda, por ejemplo) podía quedar desincronizada entre instancias durante meses sin que nadie lo notara -- descubierto por datos divergentes, nunca por un error visible.

  `Db` ahora cuenta estos drops por colección, expuesto en `/metrics` como counter (solo aparece la línea de una colección si tuvo al menos un drop, mismo criterio que `linkc_notify_latency_seconds_*`) -- en la instancia que ESCRIBE el cambio, no la que lo hubiera recibido.

1163 tests (2 nuevos) en `metrics.rs` (aparece solo cuando se provee, por colección) y `pg_integration.rs` contra Postgres real: un `insert` con un campo de 8200 caracteres confirma el contador en 1 sin afectar el insert local; un insert normal no lo incrementa. Detalle completo: GRAMMAR.md §3.150, PLAN.md §9.8.

## [1.104.0] - 2026-08-25

### ✨ Nuevo
- **`linkc serve-all --service-api-key-exempt <nombre1,nombre2,...>`.** Segundo hallazgo del barrido de "límites honestos" de GRAMMAR.md (después de v1.103.0): de todos los flags globales de `serve-all` (`--jwt-secret`/`--cors-origin`/`--session-ttl`/etc.), `--service-api-key` es el único que es una capa de SEGURIDAD real, no solo conveniencia -- el más caro de tener atascado como global sin excepción. Antes de este fix, un workspace con UN servicio que necesita quedar público (un healthcheck de terceros, un webhook entrante que no puede mandar el header) obligaba a sacar ese servicio de `serve-all` por completo y correrlo aparte con `linkc serve`.

  Un nombre en la lista (validado contra los `.link` reales descubiertos en el directorio -- un typo falla limpio antes de arrancar cualquier servicio, listando los nombres reales) recibe `None` en su propio hilo, sin tocar el chequeo del resto de los servicios. Requiere `--service-api-key`/`LINK_SERVICE_API_KEY` configurado (error de CLI limpio si no hay nada de qué eximir a nadie).

1161 tests (3 nuevos) en `cli_serve_all.rs`, contra el binario real: un servicio nombrado exento responde sin el header mientras el otro sigue exigiéndolo (401 sin clave, 200 con la clave correcta); un nombre exento inválido falla limpio antes de arrancar nada; usar el flag sin `--service-api-key` es un error de CLI limpio. Detalle completo: GRAMMAR.md §3.93, PLAN.md §9.5.

## [1.103.0] - 2026-08-25

### 🔧 Corregido
- **Aviso de colisión de tabla en Postgres (GRAMMAR.md §3.94): `createdAt`/`updatedAt`/`deletedAt` ya no cuentan solos como evidencia de que dos programas están relacionados.** Encontrado en un barrido propio de "límites honestos" pendientes en toda la documentación, motivado por el incidente real de puertos de IgnisLove (v1.102.0): un límite documentado con meses de anticipación recién se rompió cuando alguien lo tocó en producción -- ¿qué otros límites ya documentados tienen ese mismo perfil de riesgo? Auditando §3.94 con esa pregunta apareció que la convención de auditoría que el propio lenguaje promueve (`createdAt: Timestamp = now()`, `@autoUpdate`, `@softDelete`) hace casi seguro que dos programas SIN ninguna relación real compartan ese nombre de campo -- antes, un solo nombre en común alcanzaba para suprimir la advertencia de colisión de tabla, exactamente el escenario que existe para atrapar.

  Ahora esa terna se ignora como evidencia de relación; si el struct declarado no tiene NINGÚN campo fuera de ella (caso raro), cae de vuelta al comportamiento anterior. Sin cambio de comportamiento para el resto de los nombres de campo (dominio propio, como `sessionId`/`productId`) -- la heurística general sigue siendo la misma.

1158 tests (2 nuevos) en `pg_integration.rs` contra Postgres real: dos `.link` sin relación que SOLO comparten `createdAt` siguen disparando la advertencia (antes de este fix, no lo hacían); un struct compuesto únicamente por campos de auditoría cae de vuelta al comportamiento anterior sin regresión. Detalle completo: GRAMMAR.md §3.94, PLAN.md §9.3.

## [1.102.0] - 2026-08-25

### ✨ Nuevo
- **`linkc serve-all --port-registry <archivo.json>`: puerto estable por nombre de servicio** (GRAMMAR.md §3.153, PLAN.md §9.7). Diagnosticado por otra sesión de Claude ("skynet-d3") investigando en vivo el VPS de producción de un adoptador (IgnisLove): confirmó el mecanismo EXACTO del incidente de colisión de puerto ya conocido -- `serve-all` asigna por orden alfabético de archivo, así que con 17 servicios el puerto `8792` cayó en `bot_defense`, el mismo que otra app (`myfinance`) tenía hardcodeado -- y de paso descartó a `linkc` como sospechoso de ningún problema de RAM (12.8 MB / 0% CPU en un solo proceso de 103 hilos, el componente más barato de los cuatro de esa app).

  `--port-map-out` (§3.107, existente) ya hacía LEGIBLE la asignación pero era de solo escritura -- cada arranque la recalculaba entera. `--port-registry` LEE el archivo primero si ya existe (misma forma `{"nombre": puerto, ...}`): un nombre ya presente conserva su puerto de siempre sin importar qué otro `.link` se agregue/borre/renombre alrededor; un nombre nuevo recibe el próximo puerto libre desde `--port-base`. Un servicio borrado deja su entrada intacta en el registro A PROPÓSITO -- su puerto nunca se reasigna solo a un servicio distinto, para no reproducir el mismo incidente al revés (un gateway externo con ese puerto viejo todavía hardcodeado). JSON inválido en el archivo falla limpio antes de arrancar cualquier hilo, mismo criterio que un `.link` con error de tipos. Combina libremente con `--port-map-out`.

1156 tests (4 nuevos) en `cli_port_registry.rs`, contra el binario real bindeando puertos de verdad: sin historial previo la asignación es idéntica a la de siempre; agregar un `.link` que cae antes alfabéticamente no mueve el puerto de los ya registrados; borrar un `.link` y agregar uno nuevo confirma que el nuevo nunca hereda el puerto liberado por el viejo; un registro con JSON inválido falla limpio sin abrir ningún puerto. Detalle completo: GRAMMAR.md §3.153, PLAN.md §9.7.

## [1.101.0] - 2026-08-25

### ✨ Nuevo
- **Versión bundle: nueve ítems del backlog general (PLAN.md §9.3/§9.4/§9.5/§9.7/§9.8), cerrados juntos.** A pedido explícito del usuario ("seguí con los items y no pares hasta completar mínimo 10" -- nueve quedaron genuinamente listos con la misma vara de verificación de siempre; el detalle de por qué no se forzó un décimo va en el reporte de esta versión, no en este changelog):

  1. **`@cache("60s")`: cache de resultado del lado del servidor** (§3.144, PLAN.md §9.3 ítem 5). Sobre un `rpc` (rechazado sobre un `stream`), cachea in-memory el resultado de la primera ejecución EXITOSA por `(service, rpc, argumentos)` -- un reintento dentro del TTL repite la respuesta sin correr el cuerpo de nuevo. Ortogonal a `@cache_control` (cliente) y `@idempotent` (escrituras).
  2. **`deleteWhere` empuja la selección a SQL** (§3.145, PLAN.md §9.3 ítem 1, última parte). Un predicado pusheable ahora usa `find_where_conjunction` (la misma función que `findWhere`/`countWhere`) para encontrar las filas a borrar en vez de traer la colección entera a memoria -- el borrado en sí sigue fila por fila para no perder el aviso a `stream`.
  3. **`@check(minLength/maxLength, N)`: constraints de longitud sobre `String`** (§3.146, PLAN.md §9.3 ítem 3, mitad `String`). Mismo `FieldCheck`, mismos dos puntos de enforcement (aplicación + `CHECK` real en los dos backends) que la mitad numérica ya resuelta en v1.60.0. Cuenta caracteres Unicode, no bytes.
  4. **`@cors("...")`: override de CORS por ruta** (§3.147, PLAN.md §9.4 ítem 4). Reemplaza entero al CORS global para un `rpc`/`stream` puntual -- aplica tanto al preflight `OPTIONS` como a la respuesta real.
  5. **Log de auditoría de autorización estructurado** (§3.148, PLAN.md §9.5 ítem 2). `auth_role`/`auth_user_id`/`auth_allowed` como campos de PRIMER NIVEL en la línea de log de cada request que pasó por `@authenticated`/`@requires`.
  6. **`GET /metrics` en formato Prometheus** (§3.149, PLAN.md §9.8 ítems 1 y 2). `linkc_http_requests_total`/`linkc_http_request_duration_seconds_sum` por método, `linkc_stream_subscribers` por colección, `linkc_db_size_bytes` -- no exento de `--service-api-key`.
  7. **Latencia de propagación NOTIFY + cola de reintento acotada** (§3.150, PLAN.md §9.8 ítem 3, cierra la sección). Latencia real vía `sent_at_ms` en el payload, expuesta en `/metrics`; una falla TRANSITORIA de `NOTIFY` ahora se reintenta desde una cola FIFO acotada (`MAX_PENDING_NOTIFY_RETRIES = 50`) en vez de perderse para siempre.
  8. **`db.vacuum()`/`db.tableStats()`: RPCs de administración** (§3.151, PLAN.md §9.7 ítem 3). Dos builtins sobre `db` directo que quien escribe el `.link` expone en su propio service detrás de `@requires(Role.Admin)`, sin servicio `_admin` auto-inyectado.
  9. **Bloqueo de cuenta configurable** (§3.152, PLAN.md §9.5 ítem 1). `auth.recordFailedLogin`/`auth.failedLoginCount(identifier, windowSeconds)`/`auth.resetFailedLogins` sobre `SessionStore` -- umbral, ventana e `identifier` los elige el propio `.link`, sin mecanismo automático ni flag de servidor nuevo.

1152 tests (60 nuevos) repartidos entre `cache.rs` (5, nuevo módulo), `metrics.rs` (3, nuevo módulo), `checker.rs`/`parser.rs` (anotaciones y checks nuevos), `runtime/db.rs`/`runtime/mod.rs` (deleteWhere pusheado, minLength/maxLength, db.vacuum/tableStats con su test de regresión, NOTIFY con sent_at_ms), `session.rs` (bloqueo de cuenta), y seis archivos de integración contra un `linkc serve`/Postgres REALES: `server_http.rs` (cache), `cli_cors.rs` (override incluido el preflight), `cli_auth_audit_log.rs` (nuevo, stdout real), `cli_metrics.rs` (nuevo, incluido dos conexiones `stream` reales), `cli_auth_lockout.rs` (nuevo, login real con `Result<String, LoginError>`), `pg_integration.rs` (minLength con CHECK real, tamaño de base real, latencia NOTIFY real entre dos instancias, VACUUM real sin bloque de transacción). Detalle completo: GRAMMAR.md §3.144-§3.152, PLAN.md §9.3/§9.4/§9.5/§9.7/§9.8.

## [1.100.0] - 2026-08-25

### ✨ Nuevo
- **Versión bundle: cuatro ítems del backlog general (PLAN.md §9.3/§9.4/§9.6), cerrados juntos.** A pedido explícito del usuario ("seguí con el backlog general pero no subas nueva versión hasta que estén varios hechos, mínimo la mitad"):

  1. **`@idempotent`: idempotency keys nativas en rpcs de escritura** (§9.3). Sin argumentos sobre un `rpc` (rechazado sobre un `stream`) -- opt-in por REQUEST vía el header `Idempotency-Key` (mismo nombre que Stripe), nunca forzado. Con el header, un `idempotency::IdempotencyStore` en memoria (mismo modelo que `rate_limit::RateLimiter`) recuerda el resultado de la primera ejecución EXITOSA por `(service, rpc, clave)` -- un reintento con la misma clave y el mismo body (hasheado con SHA-256) repite la respuesta grabada sin correr el cuerpo de nuevo; la misma clave con un body distinto da 409. TTL de 24hs, no persiste entre reinicios.
  2. **`smtp.sendMessage`: cc/bcc y adjuntos reales** (§9.6). Variante "kitchen sink" aparte de `send`/`sendToMany`/`sendHtml` -- `{ to, cc?, bcc?, subject, body, html?, attachments? }`, con `attachments` viajando en `contentBase64` (decodificado directo a bytes, sin pasar por `base64.decode` que exige UTF-8). cc/bcc llegan al sobre SMTP real; `cc` aparece en el header `Cc:`, `bcc` nunca aparece en ningún header. Adjuntos como partes MIME reales (`multipart/mixed`).
  3. **`@rate_limit(..., key: <param>)`: una clave adicional a la IP** (§9.4). Segundo argumento opcional nombrando un parámetro `String`/`Int` del rpc -- la clave del bucket pasa de "solo IP" a "IP + valor del parámetro", cerrando el gap real de un middleware que rotaba de IP reusando el mismo email para seguir abusando.
  4. **`--hsts`: `Strict-Transport-Security` opt-in** (§9.4, cierra la sección). `linkc serve` nunca termina TLS por sí solo, así que el header solo se manda si el operador lo pide explícitamente vía `--hsts <valor>`/`LINK_HSTS` (texto literal, mismo criterio que `@cache_control`) -- para el caso real de un proxy de confianza terminando TLS delante.

1092 tests (31 nuevos) repartidos entre `idempotency.rs` (5), `checker.rs`/`parser.rs` (anotaciones nuevas), y tres archivos de integración contra un `linkc serve`/servidor SMTP de mentira REALES: `server_http.rs` (idempotencia, contando filas insertadas de verdad), `cli_smtp.rs` (cc/bcc en el sobre y el header, un adjunto real como parte MIME), `cli_rate_limit.rs` (clave combinada IP+valor), `cli_hsts.rs` (el header en `/health`, un rpc normal y un `stream`). Detalle completo: GRAMMAR.md §3.140-§3.143, PLAN.md §9.3/§9.4/§9.6.

## [1.99.0] - 2026-08-25

### ✨ Nuevo
- **`llms-full.txt`: la mitad expandida de la convención llmstxt.org.** A pedido explícito del usuario: cerrar todo lo relacionado con "SEO, meta datos, AEO, GEO, AIO, LLMO" antes de volver al backlog general de PLAN.md. Auditoría de PLAN.md §9.9 (SEO y descubribilidad para IA): los nueve ítems originales siguen resueltos, y AEO/GEO/AIO/LLMO son en sustancia la misma dimensión "descubribilidad para agentes de IA" con nombres de marketing más nuevos -- ninguno pidió una pieza técnica que no estuviera ya cubierta. La única brecha real: `llms.txt` (v1.82.0, GRAMMAR.md §3.118) implementa solo la mitad "índice" del spec de [llmstxt.org](https://llmstxt.org/) -- el spec define un `llms-full.txt` hermano con el contenido COMPLETO en vez de un resumen de una línea, para que un agente no tenga que invocar el rpc solo para ver el detalle.

  `codegen::llms_txt_emit::emit_llms_txt_full` recorre los mismos servicios/rpcs que `emit_llms_txt`, pero con un `### firma` (heading) por entrada, el docstring `///` COMPLETO (sin el recorte de "solo la primera línea" que el índice aplica a propósito), y el `@example(request: ..., response: ...)` (§3.119) del rpc, si lo declaró, como bloques ` ```json ` -- reusa `literal_expr_to_json` de `openapi_emit.rs` (ahora `pub(crate)`) en vez de duplicar la conversión. `linkc build` escribe `llms-full.txt` junto a `llms.txt` siempre, sin flag nuevo -- un adopter que ya consume `llms.txt` no ve ningún cambio.

1061 tests (5 nuevos) en `codegen::llms_txt_emit`: un `### firma` por rpc con el docstring entero, sin `@example` no hay ningún bloque JSON, `@example` con `request`+`response` se propaga como dos bloques separados byte a byte, un rpc sin docstring sigue apareciendo. Verificado a mano contra el binario real: `linkc build examples/users.link <tmp>` y `examples/taskboard` regenerados, `llms-full.txt` inspeccionado byte a byte junto a los demás archivos. Detalle completo: GRAMMAR.md §3.139, PLAN.md §9.9.

## [1.98.0] - 2026-08-25

### ✨ Nuevo
- **Versión bundle: los cuatro límites de "compatibilidad" documentados a lo largo de la sesión (§3.124, §3.129, §3.134), cerrados juntos.** A pedido explícito del usuario ("recopilá todo lo que haríamos en otras versiones relacionadas con TypeScript y la compatibilidad, y terminá todo eso en una sola versión"):

  1. **Cache de Query aislado por instancia de `client`** -- de `Map<string, ...>` a nivel de módulo a `WeakMap<client, Map<string, ...>>`: dos instancias de `client` distintas (multi-tenant, múltiples sesiones) nunca vuelven a compartir cache entre sí, mientras múltiples componentes con el MISMO client lo siguen compartiendo igual que antes.
  2. **`AbortSignal` real dentro de los hooks.** Query gana un `AbortController` reference-counted por entrada de cache -- cancela el fetch compartido SOLO cuando el ÚLTIMO componente que lo mira se desmonta (`entry.listeners.size === 0`), nunca mientras otra instancia siga esperando. Mutation e Infinite ganan `AbortSignal`/`AbortController` sin reference counting (su estado no es una entrada compartida entre "líneas de trabajo" concurrentes, cancelar siempre es seguro). De paso, `tsc` -- no un test -- atrapó una regresión real: el primer intento del `catch` de Query ante un abort hacía `return;` sin relanzar, y TypeScript infería `entry.promise` como `Promise<T | void>`, incompatible con `QueryCacheEntry<T>`; arreglado relanzando (`throw`) en los dos caminos del `catch`.
  3. **Mutaciones optimistas.** `mutate`/`mutateAsync` ganan `options?.optimisticData` -- se muestra en `data` INMEDIATAMENTE, antes de que la request salga, reemplazado por el valor real en éxito o revertido a `null` en fallo (rollback gateado por el mismo `requestIdRef` de siempre). Alcance deliberado: el optimismo es sobre el `data` PROPIO de la Mutation, no sobre el cache de una Query relacionada -- los targets de `@invalidates` pueden tener formas heterogéneas (`list` devuelve `Task[]`, `stats` devuelve `BoardStats`), un updater tipado de forma segura contra eso necesitaría un mapeo de tipos por target, una pieza de diseño más grande que esta ronda no amerita.
  4. **Cache de Infinite compartido entre instancias.** `use{Servicio}{Rpc}Infinite` pasa de `useState` local a la MISMA arquitectura de cache compartido que Query (`WeakMap`/`useSyncExternalStore`/dedupe real vía `entry.promise`/`AbortController` reference-counted) -- cierra el último "alcance v0" que quedaba documentado. Clave del cache: rpc + parámetros SIN `cursor` (progreso interno, no identidad de la lista paginada).

Demostrado en `examples/taskboard/frontend/src/App.tsx`: `createTask` ahora pasa `optimisticData`, con un indicador visible ("confirmando con el servidor...") mientras la mutación está en vuelo.

1057 tests (8 nuevos) en `codegen::ts_emit`: cache aislado por client (dos clients con la misma clave nunca comparten entrada), `AbortController` reference-counted de Query, `AbortSignal`/`optimisticData` de Mutation con rollback, cache compartido + abort de Infinite. Verificado también a mano contra un `linkc serve` real (Node, sin React, mismo criterio que las verificaciones anteriores de esta sesión): cache aislado por client confirmado con DOS instancias de `client` reales; `AbortController` reference-counted confirmado con dos "listeners" simulados (desmontar el primero no aborta, desmontar el último sí); rollback optimista confirmado con una mutación real EXITOSA (reemplaza el optimista por el dato real) y una FALLIDA (rollback a `null`); dedupe de Infinite confirmado con dos "instancias" simultáneas generando un solo fetch real. `examples/taskboard/frontend` regenerado tipando limpio contra React 18 real -- atrapando de paso la regresión de tipos del punto 2 antes de llegar a producción. Detalle completo: GRAMMAR.md §3.135-§3.138, PLAN.md §9.13.

## [1.97.0] - 2026-08-25

### ✨ Nuevo
- **`@infinite(cursor, limit)`: scroll infinito real.** Vuelta a mejoras de TypeScript/React (no bugs) tras cerrar el audit de Result/ADT/generics de v1.94.0-v1.96.0: de los tres tipos de hook generado, ninguno sabía manejar paginación -- un componente con scroll infinito tenía que gestionar el cursor a mano, llamando al rpc directo y concatenando páginas él mismo. `db.<c>.pageAfter(cursor: Int?, limit: Int)` (GRAMMAR.md §3.61) ya es el único mecanismo de paginación por cursor del lenguaje -- este ítem le da un hook dedicado, `use{Servicio}{Rpc}Infinite`, en vez de inventar un mecanismo genérico para cualquier forma de paginación imaginable.

Nueva anotación que nombra los DOS parámetros de un rpc con rol de cursor/límite -- el checker exige las MISMAS firmas que `pageAfter` ya tiene y que el retorno sea `T[]` con `T` teniendo un campo `id: Int`. Reemplaza el hook de Query normal para ese rpc (nunca coexisten). `data` viene ya aplanada (`pages.flat()`); `hasNextPage` se calcula por heurístico de largo de página (sin conteo total en la respuesta -- "si la última página trajo menos que `limit`, no hay más", mismo criterio que Relay); el próximo cursor es el `id` del último elemento, mismo criterio que `pageAfter` usa puertas adentro. `cursor` desaparece de la firma pública del hook (lo maneja internamente); `limit` sigue siendo un parámetro real que el caller elige. Mismas guardas que el resto de los hooks (`requestIdRef` contra respuesta fuera de orden, `startedRef` para no perder páginas ya cargadas si `enabled` alterna). Alcance v0 deliberado: sin cache compartido entre instancias (a diferencia de Query).

Demostrado en `examples/taskboard`: `listPaged(cursor, limit)` sobre `db.tasks.pageAfter` + una sección "Historial paginado" real en `App.tsx` con un botón "Cargar más".

1053 tests (11 nuevos): 2 de parser, 8 de checker (firma válida acepta; cursor no-`Int?`, limit no-`Int`, retorno sin `id: Int`, parámetro inexistente, mismo parámetro como cursor/limit, sobre un `stream`, declarado dos veces -- todos rechazados con su mensaje propio), 1 de `codegen::ts_emit` (firma pública correcta, no coexiste con Query, Mutation sigue igual). Verificado también a mano contra un `linkc serve` real (`examples/taskboard`, 7 tareas, `limit=3`): el mismo algoritmo que el hook implementa trajo exactamente 3 páginas (3+3+1=7), sin duplicados, en orden ascendente, `hasNextPage` apagándose en el momento correcto. Verificado end-to-end contra React real: `examples/taskboard/frontend` regenerado, `App.tsx` usando el hook de verdad, tipando limpio con `tsc --noEmit` en modo estricto. Detalle completo: GRAMMAR.md §3.134, PLAN.md §9.13.

## [1.96.0] - 2026-08-25

### 🐛 Arreglado
- **`openapi.json` tenía los mismos tres bugs que `isOk`/`isErr` y el schema Zod (v1.94.0/v1.95.0) -- esta vez en la especificación PÚBLICA de la API.** Continuación directa del mismo audit: `openapi_emit.rs` describía `Result<T,E>` como `{ ok: boolean, value, error }` (el mismo campo `ok` inexistente de `isOk`/`isErr`) y un enum ADT como `{"type":"string","enum":[...]}` (el mismo bug del schema Zod) -- `openapi.json` es lo que consume Swagger UI o un generador de SDK en otro lenguaje, así que describir el shape equivocado ahí no es cosmético. Arreglado a `oneOf`+`const` (el equivalente JSON Schema 2020-12 del `discriminatedUnion` que Zod ya usa) en ambos casos, con el mismo criterio `all_unit` para decidir entre enum simple y ADT.

- **Regresión propia, encontrada auditando el archivo antes de que llegara a producción: un `type`/`enum` GENÉRICO (`Box<T>`, o el `Result<T,E>` educativo de la documentación) rompía `linkc build` ENTERO**, tanto en `schemas.ts` (zod_emit.rs, `Item::Type` -- el mismo branch del fix de v1.95.0, ahí solo se había arreglado `Item::Enum`) como en `openapi.json` (`openapi_emit.rs`, `Item::Type` e `Item::Enum` ADT). El primer intento de cada fix resolvía campos con `resolve_type` a secas, que rechaza un parámetro de tipo sin instanciar (`T`) -- arreglado con `resolve_type_abstract` (mismo criterio que `resolve_field_ty` en ts_emit.rs ya usa desde antes), que cae al catch-all seguro ya existente (`z.unknown()` en Zod, `{"type":"object"}` en JSON Schema) en vez de fallar.

1042 tests (5 nuevos): 4 en `codegen::openapi_emit` (el `Result<T,E>` de un rpc real usa `oneOf`/`const`, nunca `{ok, ...}`; un ADT usa `oneOf`/`const` por variante; un enum sin datos sigue igual; un `type`/`enum` genérico no rompe el build) + 1 en `codegen::zod_emit` (un `type` genérico, complemento del test de enum genérico de v1.95.0). Verificado también a mano contra el binario real: `linkc build examples/users.link` regenerado, `openapi.json` inspeccionado byte a byte -- el `Result<Task, ValidationError>` de `create` y `ValidationError` en `components/schemas` usan la forma nueva en el archivo real. Detalle completo: GRAMMAR.md §3.133, PLAN.md §9.13.

## [1.95.0] - 2026-08-25

### 🐛 Arreglado
- **El schema Zod de un enum ADT usaba `z.enum([...])`, aceptando el string equivocado y rechazando el objeto real.** Octava ronda seguida, misma familia de bug que v1.94.0: `Item::Enum` en `zod_emit.rs` generaba `z.enum([...])` (unión de strings) para CUALQUIER enum, sin importar si sus variantes llevaban datos. `examples/users.link` declara exactamente ese caso (`ValidationError { InvalidEmail { field: String }, TooShort { field: String, min: Int } }`, el error de dominio real del `create` de ese ejemplo) -- el wire real de un ADT es un objeto con tag `type` más los campos de la variante, nunca un string pelado. El schema viejo aceptaba `"InvalidEmail"` (el string) y rechazaba `{ type: "InvalidEmail", field: "..." }` (el objeto real) -- exactamente al revés de lo que cualquier payload real necesita.

Mismo criterio `all_unit` que `emit_enum_decl` (ts_emit.rs) ya usa para decidir entre las dos formas: sin datos en ninguna variante, sigue `z.enum([...])` sin cambios (nunca tuvo el bug); con datos, ahora `z.discriminatedUnion("type", [z.object({ type: z.literal("Variante"), ...campos }), ...])`, reusando `render_zod_type_for_field` (con `.optional()`/validadores) para los campos -- el mismo camino que ya usan los campos de un `type` struct.

**Regresión real atrapada de paso por `docs_examples.rs`** (el suite que compila cada bloque marcado de la documentación con el binario real, antes de llegar a producción): un ADT GENÉRICO (`enum Result<T, E> { Ok { value: T }, ... }`, el ejemplo educativo de GRAMMAR.md/docs) rompía `linkc build` ENTERO -- el primer intento de este fix resolvía cada campo de variante con `resolve_type` a secas, que rechaza un parámetro de tipo sin instanciar (`T`) con "tipo desconocido". Arreglado con `resolve_type_abstract` (mismo criterio que `resolve_field_ty` en ts_emit.rs ya usa), que deja `T` como `Type::TypeParam` en vez de fallar -- cae al `z.unknown()` catch-all que `render_zod_type` ya tenía, sin romper el build.

1037 tests (3 nuevos) en `codegen::zod_emit`: un ADT de dos variantes con datos genera el `discriminatedUnion` esperado, un enum sin datos sigue generando `z.enum` sin cambios, una variante SIN datos mezclada dentro de un ADT lleva solo el discriminador, y un ADT genérico no rompe el build. Verificado también con Zod REAL en runtime, mismo criterio que v1.94.0: el schema arreglado acepta `{ type: "InvalidEmail", field: "email" }` (y la segunda variante) y RECHAZA explícitamente el string pelado `"InvalidEmail"` que la forma vieja aceptaba. Detalle completo: GRAMMAR.md §3.132, PLAN.md §9.13.

## [1.94.0] - 2026-08-25

### 🐛 Arreglado
- **`isOk`/`isErr` y el schema Zod de `Result<T,E>` chequeaban un campo que no existe.** Séptima ronda seguida sobre TypeScript/React, esta vez un bug real: `isOk`/`isErr` (las funciones exportadas por `client.ts` para narrowing de un `Result<T,E>`) estaban tipadas y implementadas contra `{ ok: true; value: T } | { ok: false; error: E }`. Ningún `Result<T,E>` real tiene un campo `ok` -- el wire, `contract.d.ts` (`{ type: "Ok"; value: T } | { type: "Err"; error: E }`) y `validators.ts` usan `type: "Ok"|"Err"` desde siempre. Pasarle un `Result<T,E>` real (literalmente `await client.create(...)`) a `isOk`/`isErr` ni siquiera TIPABA -- `tsc` real rechazaba la llamada. Mismo bug en `zod_emit.rs`: el schema de `Result<T,E>` discriminaba por `"ok"`, rechazando cualquier `Result` real con Zod.

**Alcance real, no sobre-vendido**: `validators.ts` -- la validación REAL de cada respuesta, la pieza de seguridad del contrato -- ya usaba `.type` correctamente desde siempre y nunca tuvo el bug; el impacto está acotado a dos exports auxiliares (`isOk`/`isErr`, y el schema Zod de `Result<T,E>` cuando aparece como campo de un tipo nombrado -- `emit_zod_schemas` no genera un schema por rpc, así que este camino no se ejercita para el uso más común de `Result` como retorno directo). `client.ts` ahora importa `Result` SIEMPRE desde `./contract`, sin importar si algún rpc del programa lo usa -- antes, `isOk`/`isErr` (emitidas incondicionalmente) podían terminar referenciando un nombre nunca importado.

1034 tests (3 nuevos): 2 en `codegen::ts_emit` (la firma real tipa contra `Result<T, E>` y narrowea con `.type`; `Result` se importa siempre, incluso sin ningún rpc que lo use) + 1 en `codegen::zod_emit` (el schema discrimina por `"type"`, no `"ok"`). Verificado a mano con `tsc` real, dos veces -- antes del fix (confirmando el error de tipo exacto) y después (confirmando que compila y narrowea) -- contra un `client.create(...)` genuino. El fix de Zod se verificó además con Zod REAL en runtime: el schema arreglado acepta un payload `{ type: "Ok"/"Err", ... }` genuino y RECHAZA explícitamente la forma vieja (`{ ok: true, ... }`). Detalle completo: GRAMMAR.md §3.131, PLAN.md §9.13.

## [1.93.0] - 2026-08-25

### ✨ Nuevo
- **`reconnect()` manual en el hook de `stream`.** Sexta ronda seguida sobre TypeScript/React: `use{Servicio}{Rpc}Query` tiene `refetch()`, `use{Servicio}{Rpc}Mutation` tiene `reset()`, pero el hook de `stream` no tenía NINGUNA forma de recuperarse de un fallo -- una conexión SSE cortada (blip de red, el servidor reiniciando) dejaba `isConnected: false`/`error` seteado PARA SIEMPRE, sin más recurso que desmontar y remontar el componente entero, perdiendo `data`/`latest` acumulados de paso. Un contador `reconnectAttempt` (`useState(0)`) como dependencia del `useEffect` -- incrementarlo re-ejecuta el efecto entero, re-suscribiéndose desde cero con una conexión SSE real. `reconnect()` es la función que lo incrementa. `data`/`latest` NO se limpian al reconectar (seguir la conexión viva, no empezar de cero); `error` sí, como cualquier reintento. Manual, no automático con backoff -- mismo criterio que `refetch()`/`reset()`/`mutate()`: quien consume el hook decide cuándo tiene sentido reconectar.

Demostrado en `examples/taskboard/frontend/src/App.tsx`: el indicador "Stream en Vivo" ahora muestra un botón "Reconectar" cuando `!isConnected`.

1031 tests (1 nuevo) en `codegen::ts_emit`: `SubscriptionState<T>` expone `reconnect: () => void`, `reconnectAttempt` es dependencia real del efecto, `reconnect` lo incrementa, el `return` del hook lo expone -- el test existente de generación de hooks sigue pasando sin cambios. Verificado también end-to-end contra React real: `examples/taskboard/frontend` regenerado, con `App.tsx` usando `reconnect()` de verdad, y tipando limpio con `tsc --noEmit` en modo estricto. Detalle completo: GRAMMAR.md §3.130, PLAN.md §9.13.

## [1.92.0] - 2026-08-25

### ✨ Nuevo
- **`options?: { signal?: AbortSignal }` en `client.ts`.** Quinta ronda seguida sobre TypeScript/React, esta vez fuera de `hooks.ts`: hasta esta ronda, ninguna request generada (`rpc` o `stream`) tenía forma de cancelarse -- un componente que se desmonta a mitad de un fetch, o un buscador que dispara una request nueva por cada letra tipeada, solo podía IGNORAR la respuesta vieja (mismo criterio que la guarda de `requestIdRef`, §3.123, y el cache de Query, §3.124), nunca cancelar el `fetch()` real -- que seguía corriendo en el servidor de todos modos. Nuevo último parámetro, siempre opcional, en CADA método generado (`rpc` y `stream` por igual, en la interfaz `contract.d.ts` Y la implementación `client.ts`) -- `push_fetch_call` (compartida entre ambos caminos) pasa `signal: options?.signal` al `fetch()` real, `undefined` cuando no se pasa `options`, mismo comportamiento que `fetch()` ya tiene sin `signal`. Ningún caller existente se rompe.

Alcance deliberado: solo `client.ts` -- `hooks.ts` no cambia en esta ronda. Integrar cancelación DENTRO de los hooks generados (ej. que `use{Servicio}{Rpc}Query` aborte automático al desmontar) es una decisión de diseño más grande: la entrada de cache de Query es COMPARTIDA entre instancias (§3.124), así que abortar por una instancia no debería cancelar la request que OTRA instancia montada sigue esperando -- queda para una ronda aparte con su propio diseño. Mientras tanto, cualquier componente puede usar `client.<rpc>(...)` directo con su propio `AbortController`, fuera de los hooks.

1030 tests (1 nuevo) en `codegen::ts_emit`: `options?: { signal?: AbortSignal }` presente en la interfaz y la implementación de un `rpc` sin parámetros y de un `stream`, siempre como último parámetro; `signal: options?.signal,` presente exactamente una vez por cada `fetch()` real. Todos los tests existentes que verificaban firmas exactas de métodos actualizados a la nueva firma. Verificado también a mano contra un `linkc serve` real (`examples/taskboard`, bundle de `client.ts` vía esbuild): abortar antes de que la respuesta llegue rechaza con `AbortError` real; abortar con un `setTimeout` de 1ms también; una llamada sin `options` sigue funcionando exactamente igual que antes -- las tres contra el servidor real. Detalle completo: GRAMMAR.md §3.129, PLAN.md §9.13.

## [1.91.0] - 2026-08-25

### ✨ Nuevo
- **`mutate` vs `mutateAsync` en `use{Servicio}{Rpc}Mutation`.** Cuarta ronda seguida sobre TypeScript/React, esta vez encontrando el gap directamente en la propia demostración del repo: `examples/taskboard/frontend/src/App.tsx`, `handleCreate`, hacía `await createTask(input)` (el `mutate` del hook) SIN try/catch -- el uso más natural. `mutate` SIEMPRE relanzaba, así que un fallo real producía una promesa rechazada sin manejar ("Uncaught (in promise)" en consola), pese a que `error` YA quedaba en el estado del hook. `mutateAsync` (mismo nombre que react-query usa para el mismo contrato) es ahora la función que relanza, para quien de verdad quiere `try`/`catch` a mano; `mutate` pasa a ser un wrapper que nunca relanza -- devuelve `null` en el fallo, mismo patrón que `refetch()` de Query ya usaba. `MutationState<T>` no cambia; el cambio vive en la intersección de tipos que cada hook devuelve. `App.tsx` actualizado al patrón nuevo.

### 🐛 Arreglado
- **`hooks.ts` generado no duplica `| null` en un retorno ya opcional.** De paso, escribiendo el test de `mutate`/`mutateAsync` sobre un rpc con retorno YA opcional (`T?`) apareció `Promise<Task | null | null>` -- real en el propio `taskboard.link` (`getById(id) -> Task?`). Compilaba igual en TS (las uniones se aplanan) pero el archivo generado quedaba con texto redundante en CUATRO lugares: `data`/`mutate`/`mutateAsync` de Mutation, `refetch()` de Query, y `latest` de un `stream` con item opcional -- los cuatro compartían el mismo bug (agregar `| null` a mano sin chequear si el tipo ya terminaba así). Unificado en una sola variable `nullable_ret_str`, calculada una vez por rpc/stream y reusada en los cuatro sitios.

1029 tests (2 nuevos) en `codegen::ts_emit`: la firma pública de Mutation expone las dos funciones con los tipos de retorno correctos, `mutateAsync` sigue relanzando sin cambios, `mutate` devuelve `null` en el `catch`; y ningún `| null | null` en todo el archivo generado, verificado sobre una Mutation, una Query y un `stream`, los tres con retorno/item opcional. Verificado también end-to-end contra React real: `examples/taskboard/frontend` regenerado y tipando limpio con `tsc --noEmit` en modo estricto, con `getById` confirmando que el texto redundante desapareció. Detalle completo: GRAMMAR.md §3.128, PLAN.md §9.13.

## [1.90.0] - 2026-08-25

### ✨ Nuevo
- **`loading` vs `isFetching` en `use{Servicio}{Rpc}Query`.** Tercera ronda seguida sobre el mismo pedido del usuario de seguir profundizando TypeScript/React ("sigue"): desde v1.87.0, `loading` era un único flag verdadero durante CUALQUIER fetch -- tanto el inicial (sin datos todavía) como un `refetch()` de FONDO sobre una entrada que YA tenía datos cacheados. Un componente escrito de la forma más natural (`if (loading) return <Spinner/>`) ocultaba una lista que ya estaba mostrando datos válidos cada vez que alguien la refrescaba -- el clásico problema que react-query resuelve distinguiendo `isLoading` de `isFetching`. Ahora `isFetching` es el flag real (renombre del que `loading` ocupaba en `QueryCacheState<T>`, verdadero durante cualquier fetch), y `loading` pasa a ser un valor DERIVADO (`data === null && isFetching`, "no hay nada que mostrar todavía") en vez de un flag propio -- imposible que queden desincronizados. Sin cambios en cuándo fetchea (dedupe vía `entry.promise`, auto-fetch del `useEffect`, invalidación vía `@invalidates`) -- puramente qué expone el hook. `Mutation` queda deliberadamente afuera: no tiene el concepto de "dato cacheado que sigue siendo válido mientras se recarga".

1027 tests (1 nuevo) en `codegen::ts_emit`: `QueryState<T>` expone `loading`+`isFetching`, `QueryCacheState<T>` interno usa `isFetching`, ningún `setQueryCacheState` en todo el archivo escribe un `loading: true`/`loading: false` -- el test existente de cache compartido actualizado a la nueva forma del `return`. Verificado también end-to-end contra React real: `examples/taskboard/frontend` regenerado y tipando limpio con `tsc --noEmit` en modo estricto. Detalle completo: GRAMMAR.md §3.127, PLAN.md §9.13.

## [1.89.0] - 2026-08-25

### 🐛 Arreglado
- **`LinkTransportError` ahora lleva `status: number` tipado, no solo interpolado en el mensaje.** Continuación del pedido de seguir profundizando TypeScript/React tras §3.123-§3.125: auditando `client.ts` apareció que el status HTTP de un fallo de transporte (401/404/500/...) solo viajaba dentro del string del mensaje (`` `HTTP ${res.status}` ``) -- un componente que necesitaba distinguir un 401 (redirigir a login) de un 500 (ofrecer reintentar) tenía que parsear ese mensaje a mano con una regex, exactamente el tipo de tipo poco ergonómico que este pedido venía a resolver. Ahora `status: number` es una propiedad real, poblada con el `res.status` genuino en los dos puntos donde `client.ts` la lanza (`!res.ok` y el caso borde de un stream con `res.body` nulo). Sin cambios del lado de los hooks -- `QueryState.error`/`MutationState.error` siguen `Error | null`; `error instanceof LinkTransportError && error.status === 401` es el patrón de narrowing que ahora queda disponible.

1026 tests (2 nuevos) en `codegen::ts_emit`: la clase emitida tiene la propiedad y el constructor la asigna; los dos call sites reales pasan `res.status`, no un valor inventado. Verificado también end-to-end contra React real: `examples/taskboard/frontend` regenerado y tipando limpio con `tsc --noEmit` en modo estricto. Detalle completo: GRAMMAR.md §3.126, PLAN.md §9.13.

## [1.88.0] - 2026-08-25

### ✨ Nuevo
- **`@invalidates(rpc1, rpc2, ...)`: invalidación automática de cache tras una Mutation.** Continuación explícita del usuario ("si, sigue con eso") sobre el límite documentado en v1.87.0: hasta esta ronda, `useUsersCreateMutation` no tenía forma de avisarle a `useUsersListQuery` que sus datos quedaron viejos -- cada componente era responsable de llamar a `refetch()` a mano tras una mutación exitosa. La nueva anotación se declara sobre la Mutation, no sobre cada Query que la consume: `@invalidates(list, stats)` sobre `create` limpia las entradas de cache de `list` y `stats` (reset a `data: null` + notificación a los listeners suscriptos vía `useSyncExternalStore`) tras un `mutate()` exitoso -- nunca en la rama de error. Deliberadamente NO dispara un fetch nuevo: reusa el `useEffect` de auto-fetch que el hook de Query ya tiene (v1.87.0), que re-dispara solo en cuanto ve `data === null`.

Checker con 4 reglas de validación, cada una con su propio mensaje: el target tiene que ser un rpc de la MISMA service, no puede ser un `stream`, tiene que tener forma de Query (mismo heurístico que decide qué hook genera cada rpc, extraído a un único método compartido `RpcDecl::looks_like_a_query()` entre checker y codegen para que nunca puedan divergir), y la anotación no puede repetirse. La coincidencia de cache es por PREFIJO de clave -- invalidar `search` limpia TODAS las variantes cacheadas de `search(...)`, sin importar con qué parámetros se llamó cada una, porque una Mutation no puede adivinar cuáles quedaron afectadas. Emisión condicional del helper `invalidateQueryCache`, mismo criterio que `useSyncExternalStore` en v1.87.0 -- un programa sin `@invalidates` no paga el costo de una función sin usar bajo `noUnusedLocals`. `examples/taskboard/backend/taskboard.link` demuestra el uso real: sus tres mutations (`create`/`update`/`remove`) invalidan `list`/`listByColumn`/`stats`.

1024 tests (10 nuevos): 2 en `parser.rs` (parsea la lista de nombres; `@invalidates()` vacío es error de parseo) + 6 en `checker.rs` (target válido, target inexistente, target de otra service, target que no es forma de Query, anotación sobre un `stream`, anotación repetida) + 2 en `codegen::ts_emit` (el helper se emite solo en la rama de éxito de la Mutation, nunca en el `catch`; sin `@invalidates` en el programa no se emite el helper). Verificado a mano contra el binario real: el camino feliz y los 5 caminos de error, cada uno con el mensaje exacto diseñado. Verificado también end-to-end contra React real: `examples/taskboard/frontend` regenerado (con `@invalidates` de verdad en sus tres mutations) y tipando limpio con `tsc --noEmit` en modo estricto -- primera vez que un `hooks.ts` con invalidación de cache se compila contra React 18 real. Detalle completo: GRAMMAR.md §3.125, PLAN.md §9.13.

## [1.87.0] - 2026-08-25

### ✨ Nuevo
- **Cache compartido entre instancias del hook de Query.** Continuación explícita del usuario ("avanza con el cache") sobre v1.86.0. Antes de esta ronda, dos componentes llamando al mismo `use{Servicio}{Rpc}Query` con los mismos parámetros disparaban dos fetches independientes, sin relación entre sí -- el problema clásico que react-query/SWR resuelven con un cache global, ahora resuelto DENTRO del propio `hooks.ts` generado, sin sumar ninguna librería nueva como dependencia. Un `Map<string, QueryCacheEntry<T>>` a nivel de módulo (singleton por archivo cargado), clave por rpc+parámetros (`JSON.stringify`), con `useSyncExternalStore` (la API que React 18 documenta para suscribirse a un store externo sin roturas de consistencia) reemplazando los `useState` locales del hook -- dos instancias con la misma clave comparten la misma entrada y se re-renderizan juntas cuando cualquiera actualiza el estado. Dedupe REAL vía `entry.promise`: si ya hay un fetch en vuelo para esa clave, un `refetch()` nuevo se une a la MISMA promesa en vez de disparar su propio request -- dos componentes montándose juntos generan una sola request HTTP, no dos. El `requestIdRef` que v1.86.0 le había agregado al hook de Query queda superado (el cache resuelve el mismo problema de respuestas fuera de orden por construcción); el de `Mutation` sigue exactamente igual, sin cambios -- una mutación no comparte cache.

Alcance documentado: cache por rpc+parámetros, NO por instancia de `client`; sin invalidación automática después de una `Mutation` relacionada (cada componente sigue responsable de llamar a `refetch()` a mano tras una mutación exitosa). Un programa sin ningún Query (todo mutations/streams) no emite `useSyncExternalStore` ni la infraestructura de cache, para no romper un build con `noUnusedLocals` por un import/const sin usar.

1014 tests (2 nuevos, netos: se removió el test de `requestIdRef` en Query -- ya no aplica -- y se sumaron dos): la clave de cache se arma correcta con params reales, la infraestructura compartida se emite UNA sola vez sin importar cuántos Query tenga el programa, la forma pública del hook no cambió; un programa sin Query no emite nada de la infraestructura de cache. Además, la lógica central del dedupe (sin React) verificada aparte con un script de Node standalone: dos "instancias" pidiendo la misma clave casi simultáneo comparten exactamente un fetch real, dos claves con parámetros distintos nunca se pisan, actualizar una entrada notifica a sus listeners. Verificado también end-to-end contra React real: `examples/taskboard/frontend` regenerado y tipando limpio con `tsc --noEmit` en modo estricto. Detalle completo: GRAMMAR.md §3.124, PLAN.md §9.13.

## [1.86.0] - 2026-08-25

### 🐛 Arreglado
- **Hooks de React generados: guarda contra respuestas fuera de orden.** Pedido explícito del usuario de mejorar la integración de `hooks.ts` con componentes reales. Auditando `codegen::ts_emit::emit_hooks` apareció algo más urgente que ergonomía: `use{Servicio}{Rpc}Query`/`use{Servicio}{Rpc}Mutation` no tenían ninguna protección contra una respuesta VIEJA resolviendo después de una más nueva (el caso real: un buscador llamando al hook por cada letra tipeada) -- sin guarda, la respuesta más lenta podía pisar `data` con un resultado desactualizado, en silencio, sin ningún error visible. El hook de `stream` ya se protegía de esto (`cancelled` en su `useEffect`); Query/Mutation, agregados en una ronda anterior, no. Ahora un `requestIdRef` (`useRef`, contador monotónico) por instancia del hook descarta cualquier respuesta que ya no sea la más reciente; el `useEffect` de Query invalida en su cleanup cualquier request en vuelo al desmontar/cambiar deps; `reset()` de Mutation invalida cualquier `mutate()` en vuelo.

**Gap adyacente cerrado de paso**: `hooks.ts` no tenía NINGUNA cobertura de type-check automatizada -- el frontend que corre en CI nunca lo importa, y `examples/taskboard/frontend` (el único ejemplo real que sí lo usa, con React 18 de verdad) no está conectado a ningún workflow. Verificado a mano regenerando ese ejemplo y corriendo `npx tsc --noEmit` -- pasó limpio (antes ni corría: le faltaba `zod` en el `package.json`, agregado de paso). `.gitignore` generalizado de dos entradas puntuales de `node_modules` a `**/node_modules/`.

1013 tests (2 nuevos) en `codegen::ts_emit` (Query tiene el `requestIdRef`/las guardas condicionales/el cleanup; Mutation tiene la misma guarda y `reset()` invalida en vuelo) -- el test ya existente de generación de hooks sigue pasando sin cambios (la forma pública no cambió, solo el cuerpo interno). Verificado también end-to-end contra React real. Detalle completo: GRAMMAR.md §3.123, PLAN.md §9.13.

## [1.85.0] - 2026-08-25

### ✨ Nuevo
- **`--log-format text|json` / `--log-level debug|info|warn|error`.** PLAN.md §9.8: `linkc serve` ya dejaba una línea `clave=valor` por request completada; faltaba una forma de indexarla como JSON sin parsear texto libre, y una forma de bajar el volumen en producción con tráfico real (hoy cada request exitosa deja una línea). `--log-format`/`LINK_LOG_FORMAT` (default `text`, sin cambios de comportamiento) y `--log-level`/`LINK_LOG_LEVEL` (default `info`, EXACTAMENTE el comportamiento de siempre -- las dos líneas por request, recibida y completada, se siguen imprimiendo SIEMPRE). Clasificación automática por `status` (`status_level`: 5xx=`Error`, 4xx=`Warn`, resto=`Info`), no una anotación manual por call-site -- a `--log-level warn` una request exitosa no deja ninguna línea, pero un 404/500 sigue apareciendo. `LogConfig` (`Copy`, `format`+`level`) cruza a los hilos de escritura de `stream` (`write_stream`/`write_live_stream`) igual que `max_body_bytes: u64` ya cruzaba. Alcance deliberado: solo las líneas POR REQUEST -- la línea de arranque y un error de `accept()` siguen como `println!`/`eprintln!` planos, no son la fuente de volumen que este ítem ataca; el campo libre `extra` en `LogFormat::Json` viaja tal cual dentro de un string, sin partirse en campos propios (límite documentado, no escondido).

1011 tests (6 nuevos): todos de CLI end-to-end contra el binario real en `cli_log_format.rs` (formato texto default sigue imprimiendo las dos líneas de siempre; `--log-format json` produce JSON parseable de verdad con los campos documentados; `--log-level warn` suprime una request exitosa pero sigue mostrando un 404; `--log-format`/`--log-level` inválidos rechazados con mensaje claro). Verificado también a mano contra el binario real (`curl` + lectura de stdout). Detalle completo: GRAMMAR.md §3.122, PLAN.md §9.8.

## [1.84.0] - 2026-08-25

### ✨ Nuevo
- **`linkc pm2-config <archivo.link> <puerto> [-o <archivo>]`.** PLAN.md §9.7: generador de configuración PM2, mismo criterio que `linkc docker`/`linkc systemd` (v1.83.0) -- PM2 ya aparecía citado como topología real de un adoptador (varios procesos `linkc serve-all`/pm2 compartiendo un único Postgres, GRAMMAR.md §3.105). A diferencia de esos dos comandos (directorio de salida con nombre fijo por archivo), acá el CALLER elige el archivo completo con `-o` (default `./ecosystem.json`). Genera un `ecosystem.json` en formato NATIVO de PM2 (`pm2 start ecosystem.json` lo entiende sin conversión) -- `"script": "linkc"` + `"interpreter": "none"` para ejecutar el binario directo, `"args"` como array evitando cualquier ambigüedad de quoting. `--restart-backoff 30s` va DENTRO de `args` (el backoff exponencial de conexión sigue siendo responsabilidad de `linkc serve`, GRAMMAR.md §3.92, no de PM2); `"autorestart": true` del lado de PM2 sigue siendo el reinicio de PROCESO ante un crash, complementario. Sin `LINK_DATABASE_URL` en el `env` generado -- a diferencia de la variable comentada que `linkc docker`/`linkc systemd` sí dejan como referencia, JSON no tiene comentarios, así que un placeholder ahí sería un valor REAL en vez de una pista inerte.

Nuevo módulo `pm2.rs`, mismo mecanismo que `docker::generate_docker_files`/`systemd::generate_systemd_unit`. 1005 tests (4 nuevos): 2 en `pm2.rs` (JSON válido -- parseado de verdad con `serde_json` -- con el puerto real y sin ninguna variable de conexión falsa; nombre de app del `file_stem`) + 2 de CLI end-to-end contra el binario real (`-o` explícito, y el default `./ecosystem.json` sin `-o`); `cli_help.rs` actualizado (`pm2-config` sumado a la lista de subcomandos verificados). Verificado también a mano contra el binario real, con y sin `-o`. Detalle completo: GRAMMAR.md §3.121, PLAN.md §9.7.

## [1.83.0] - 2026-08-25

### ✨ Nuevo
- **`linkc systemd <archivo.link> <puerto> [outdir]`.** PLAN.md §9.7 ítem 4: generador de unidad systemd, a la par de `linkc docker` que ya existía (`Dockerfile`/`docker-compose.yml`/`.dockerignore`) para quien despliega contra una VM/bare metal en vez de un contenedor -- armar la unidad a mano significa adivinar las opciones de hardening correctas sin ninguna guía. A diferencia de `linkc docker` (puerto siempre `3000` en la plantilla), acá el puerto es un argumento REQUERIDO -- `linkc serve` no tiene un puerto por default, mismo parseo y mismo mensaje de error (`"puerto inválido: '...'"`) que ese comando. Genera `<nombre>.service` (el `file_stem` del `.link`) con `ExecStart=/usr/local/bin/linkc serve <archivo> <puerto>`, `WorkingDirectory=/opt/<nombre>`, `Restart=on-failure`+`RestartSec=5` (reinicio de PROCESO ante un crash, complementario a `--restart-backoff` de `serve`/`serve-all`, que maneja un fallo de conexión a Postgres sin que el proceso muera), la variable `LINK_DATABASE_URL` comentada como referencia, y hardening mínimo (`NoNewPrivileges`, `ProtectSystem=strict`, `ReadWritePaths` acotado, `PrivateTmp`).

Nuevo módulo `systemd.rs`, mismo mecanismo que `docker::generate_docker_files`. 1001 tests (4 nuevos): 2 en `systemd.rs` (unidad bien formada con el puerto real, nombre de archivo del `file_stem`) + 2 de CLI end-to-end contra el binario real (`linkc systemd` genera lo esperado, puerto inválido rechazado); `cli_help.rs` actualizado sin sumar un test nuevo (`systemd` agregado a la lista que el test ya existente recorre, para que esa lista nunca se desactualice en silencio). Detalle completo: GRAMMAR.md §3.120, PLAN.md §9.7.

## [1.82.0] - 2026-08-25

### ✨ Nuevo
- **`@example(request: ..., response: ...)`: ejemplos tipados en `openapi.json`.** Último ítem de PLAN.md §9.9 (SEO y descubribilidad para IA) -- con este, la sección queda completamente resuelta. A diferencia de las demás anotaciones (`@route`, `@rate_limit`, `@deprecated`, `@cache_control`, ...), sus valores son EXPRESIONES de c-script (reusa `parse_expr`, acepta un `StructLit`/`ArrayLit` completo), no `String` crudo. Las dos mitades se TIPAN contra la forma real del rpc con el mismo mecanismo que `= default` de un campo/param (`check_expr` con `Env::new()` vacío): `request` contra un struct anónimo armado de los parámetros (un param con default es opcional ahí también), `response` contra el `return_type` resuelto. **Un ejemplo desincronizado del contrato es un error de compilación**, no un dato que puede mentir en silencio en `openapi.json`. Restringido a expresiones LITERALES (`is_literal_expr`, checker.rs) -- rechaza cualquier llamada (`crypto.uuid()`, `now()`), para que `openapi.json` no cambie en cada `linkc build` sin que el `.link` cambie, lo que rompería `--diff` (§3.79) en silencio. `@example` una sola vez por rpc, `request` solo si el rpc toma parámetros, rechazado sobre un `stream` (mismo motivo que `@cache_control` ahí). Propagado a `openapi.json` como `"example"` dentro del Media Type Object correspondiente (`requestBody`/`responses`, respetando `@content_type` si el rpc lo declaró) -- sin cambios en `contract.d.ts`/`client.ts`/`schemas.ts`, alcance atado exactamente a lo que pedía PLAN.md.

997 tests (14 nuevos): 4 en `parser.rs` (parsea expresiones de verdad, `@example()` vacío con mensaje propio, clave desconocida/repetida rechazadas) + 7 en `checker.rs` (tipa contra la forma real, rechaza type mismatch, respeta params con default como opcionales, rechaza `request` sin parámetros, rechaza una llamada no-literal, rechazado en `stream`, rechaza declararse dos veces) + 3 en `openapi_emit.rs` (propagación byte a byte, un ejemplo con solo `response` no toca `requestBody`, sin `@example` no aparece ninguna clave `"example"`). Verificado también a mano con `linkc build` real: caso feliz de punta a punta más los 7 casos de error. Detalle completo: GRAMMAR.md §3.119, PLAN.md §9.9 (sección completa).

## [1.81.0] - 2026-08-25

### ✨ Nuevo
- **`llms.txt` auto-generado por proyecto.** Tercer y último ítem resuelto de PLAN.md §9.9 (SEO y descubribilidad para IA). Convención [llmstxt.org](https://llmstxt.org/) -- no confundir con el `llms.txt` de ESTE repo, que documenta el compilador a mano. `linkc build` ahora emite un `llms.txt` junto a `contract.d.ts`/`client.ts`/`validators.ts`/`hooks.ts`/`schemas.ts`/`openapi.json`: un bullet por rpc/stream de cada `service` (`- [firma](/Servicio/rpc): nota`), con la firma completa resuelta por el checker y el docstring `///` (§3.72) como nota -- mismo dato que `openapi_emit` ya usa como `description`, cero gramática nueva. Un rpc sin docstring sigue apareciendo (solo sin nota, nunca se oculta una capacidad real de la API); un docstring de más de una línea aporta solo la primera. Nuevo módulo `codegen::llms_txt_emit`, mismo mecanismo que `openapi_emit` (`Checker::build_symbols` para resolver tipos sin repetir el chequeo completo).

983 tests (5 nuevos): título + una sección por `service` con un bullet por rpc, docstring como nota, docstring multi-línea aporta solo la primera línea, rpc sin docstring sigue apareciendo, `stream` etiquetado distinto de `rpc`. Verificado también a mano con `linkc build` real sobre un `.link` con dos servicios y un docstring, confirmando el archivo generado byte a byte. Detalle completo: GRAMMAR.md §3.118, PLAN.md §9.9 (queda un solo ítem abierto en la sección: ejemplos estructurados por rpc en `openapi.json`).

## [1.80.0] - 2026-08-25

### ✨ Nuevo
- **`metaTags`/`openGraphTags`/`canonicalLink`/`jsonLd`: metadata SEO clásica como helpers de `String`.** Segundo ítem resuelto de PLAN.md §9.9. `metaTags(tags: {name: String, content: String}[]) -> String` y `openGraphTags(tags: {property: String, content: String}[]) -> String` arman líneas `<meta>` bien formadas (atributo `name` para meta tags clásicos, `property` para Open Graph); `canonicalLink(url: String) -> String` arma un `<link rel="canonical" href="...">`; `jsonLd(data: Dynamic) -> String` arma un bloque `<script type="application/ld+json">` serializando `data` con el mismo mecanismo que `json.stringify`. Las cuatro son builtins sin receptor, mismo patrón de 5 puntos de enganche que `sitemapXml`/`robotsTxt` (v1.79.0): tipo estructural anónimo en `checker.rs` para las dos que reciben listas, dispatch en los 3 puntos de `runtime/mod.rs`. **Mitigación de XSS en `jsonLd`**: cada `<` del JSON serializado se reemplaza por su escape Unicode (técnica recomendada por OWASP) -- sin esto, un valor de usuario dentro de `data` que contuviera literalmente `</script><script>...` cerraría el bloque JSON-LD antes de tiempo y ejecutaría el resto como HTML/JS real.

978 tests (10 nuevos): 5 de tipos en `checker.rs` (acepta la forma correcta, rechaza `property` donde `metaTags` espera `name`, `canonicalLink`/`jsonLd` aceptan cualquier valor asignable) + 5 en `runtime/mod.rs` (contenido con comillas/`&` reales escapado, lista vacía sin nada inventado, `openGraphTags` con `property`, `canonicalLink` escapando `&` en la query string, `jsonLd` confirmando que el JSON serializado no contiene ningún `<` literal). Verificado también a mano contra un `linkc serve` real vía `curl`, incluida la mitigación de XSS. Detalle completo: GRAMMAR.md §3.117, PLAN.md §9.9.

## [1.79.0] - 2026-08-25

### ✨ Nuevo
- **`sitemapXml(urls: {loc: String, lastmod: Timestamp?}[]) -> String` / `robotsTxt(rules: {userAgent: String, disallow: String[]?, allow: String[]?}[], sitemapUrl: String?) -> String`.** Primer ítem resuelto de PLAN.md §9.9 (SEO y descubribilidad para IA), pedido explícito del usuario ("sigue con el seo y ia"). Hoy un `sitemap.xml`/`robots.txt` se escribe a mano concatenando `String` (ver el ejemplo de §3.35) -- ambos builtins arman el documento bien formado (XML válido según sitemaps.org, formato clásico de bloques `User-agent`/`Disallow`/`Allow` para `robots.txt`), el rpc sigue siendo responsable de la lista de datos (viene de la base, `@route` no puede inferir rutas dinámicas por sí solo). Mismo patrón de 5 puntos de enganche que `dateFromParts`: tipo estructural anónimo en `checker.rs` (`Type::Struct { name: None, .. }`, subtipado estructural -- cualquier `type` nominal con los campos correctos sirve, igual que `http.getWithHeaders`), y dispatch en los 3 puntos de `runtime/mod.rs` (Ident->FnRef, Call directo, `call_callable`). `sitemapXml` reusa `escape_html` (ya existente) para el `loc` -- las entidades HTML `&`/`<`/`>`/`"`/`'` son también válidas en XML -- y `timestamp::format_iso8601_millis` para `lastmod`. Decisión de alcance deliberada: NO se hardcodeó un preset de user-agents de IA conocidos (`GPTBot`/`ClaudeBot`/`PerplexityBot`/`Google-Extended`, el ítem 3 original de PLAN.md §9.9) -- una lista así se desactualiza en cuanto aparece un crawler nuevo y obligaría a un release del compilador para corregirla; `robotsTxt` genérico ya cubre el caso completo, un adoptador declara cualquier `userAgent` como una regla más, sin conocimiento especial del lenguaje.

968 tests (9 nuevos): 4 de tipos en `checker.rs` (acepta cualquier struct con la forma correcta, rechaza `loc` faltante, rechaza aridad incorrecta) + 5 en `runtime/mod.rs` (XML bien formado con y sin `lastmod`, escapado de caracteres especiales en `loc`, lista vacía sigue siendo un `urlset` válido, bloques `robots.txt` con `Disallow`/`Allow` en orden más `Sitemap:` final, ausencia de `sitemapUrl`/reglas vacías omite las líneas correspondientes). Detalle completo: GRAMMAR.md §3.116, PLAN.md §9.9.

## [1.78.0] - 2026-08-25

### 🐛 Arreglado
- **Lint `unused-var`: 14 falsos positivos dentro de closures y struct-literals.** Issue #11, reportado por IgnisLove con evidencia excepcional: 3 repros mínimos aislando la causa exacta más una tabla de 14 falsos positivos reales verificados a mano en 7 de sus 17 `.link` (`bandit_rewards`, `banners`, `catalog_facets`, `irene_chat`, `reviews`, `rfm_scorer`, `seo_engine`). `expr_count_ident` (`lint.rs`) tenía seis variantes de `Expr` sin manejar (`Closure`, `StructLit`, `Match`, `Index`, `TupleLit`, `TupleIndex`), cayendo al `_ => 0` genérico -- una variable usada SOLO adentro del `body` de una closure pasada a `.filter()`/`upsert`/`findWhere`, o SOLO como valor de un campo de un struct-literal de cola, se marcaba como no usada pese a estar bien. Confirmado desde que el check existe (v1.62.0), no una regresión puntual -- importa más que ruido: si `--fix` renombrara automáticamente a `_target`/`_reward`, o alguien lo hiciera a mano confiando en el aviso, rompería código que funciona.

959 tests (5 nuevos): los TRES repros del issue reproducidos literalmente + un caso de `Expr::Match` (mismo bug de fondo) + un test de no-regresión confirmando que una variable genuinamente sin usar se sigue marcando. Detalle completo: GRAMMAR.md §3.115.

## [1.77.0] - 2026-08-25

### 📝 Documentado (ya funcionaba, sin un ejemplo que lo dijera)
- **Flujo OAuth2 "client credentials" (servidor a servidor).** Auditoría de reducción de fricción con proveedores (Google APIs, Microsoft Graph, Salesforce, HubSpot y muchas otras APIs empresariales usan este flujo, distinto del "authorization code" con login de usuario, que sigue bloqueado -- necesita un proveedor de identidad real). CERO código nuevo del compilador: las tres piezas ya existían -- `http.postWithHeaders` para pedir el token, `json.parse(text) -> Dynamic` + acceso de campo sobre `Dynamic` (tipa devolviendo `Dynamic`, asignable donde se espera `String` sin cast) para extraer `access_token` sin declarar la forma completa de la respuesta, y `http.getWithHeaders` con `Authorization: Bearer <token>` para la llamada real.

954 tests (1 nuevo): verificado de punta a punta contra DOS servidores HTTP de mentira reales (uno de token, uno de API protegida) en `tests/cli_http.rs` -- confirma que el `client_id`/`client_secret` llegan tal cual al primero, y que el token que ESE servidor devuelve llega EXACTO como header `Authorization` al segundo. Detalle completo, más la auditoría de por qué Azure Blob SAS NO se implementó esta ronda (Microsoft no publica un vector de prueba reproducible con la clave incluida, a diferencia de AWS): GRAMMAR.md §3.114, PLAN.md §9.10.

## [1.76.0] - 2026-08-25

### ✨ Nuevo
- **`@cache_control("...")` por rpc.** Segundo ítem resuelto de PLAN.md §9.9 (SEO y descubribilidad para IA). Header `Cache-Control` declarativo -- dimensión ortogonal, se combina con `@route`/`@content_type`/auth/`@rate_limit` sin restricción. Solo en el camino de éxito (una respuesta de error nunca hereda la política de caché del éxito, mismo criterio que `@content_type`/`response.redirect`). Rechazado sobre un `stream` -- error de compilación, una conexión SSE nunca es cacheable de forma sensata. Mecanismo estático (`Annotation::CacheControl`, resuelto del AST vía `server.rs::declared_cache_control`), mismo patrón que `ContentType`/`RateLimit`/`Deprecated`.

953 tests (6 nuevos): 4 de tipos en `checker.rs` (combina con `@route`, vacío/duplicado rechazados, rechazado dentro de un `stream`) + 2 end-to-end en `cli_content_type.rs` contra un `linkc serve` real -- el header presente en éxito, ausente en un error forzado del mismo rpc, y el caso combinado real (`@route`+`@content_type`+`@cache_control` sobre un sitemap) con los tres headers correctos a la vez. Detalle completo: GRAMMAR.md §3.113.

## [1.75.0] - 2026-08-25

### 📝 Documentado (existía sin documentar ni probar)
- **`base64.encode(data: String) -> String` / `base64.decode(base64Str: String) -> String`.** Auditoría disparada por el pedido explícito del usuario de reducir fricción con terceros ("dar soporte a la mayor cantidad de proveedores posibles"): investigando Twilio (HTTP Basic Auth) apareció que estas dos funciones YA EXISTÍAN en el checker y el runtime -- pero en ningún lugar de GRAMMAR.md/README/`llms.txt`, y sin un solo test. Mismo patrón exacto que el incidente de la firma S3 falsa de MyFinance (v1.73.0): una capacidad real, invisible para quien la necesita. Esto solo ya destraba cualquier proveedor con HTTP Basic Auth combinado con `http.postWithHeaders`, sin escribir código nuevo del compilador.

947 tests (4 nuevos): 2 de tipos en `checker.rs` + 2 en `runtime/mod.rs` contra vectores conocidos (`"hello"` <-> `"aGVsbG8="`, confirmados con el `base64` del sistema) más los casos de error (base64 mal formado, bytes decodificados no-UTF8). Detalle completo, más una auditoría de fricción con Stripe/SendGrid/Twilio/Azure Blob/GCS/SQS/RabbitMQ (cuáles YA funcionan hoy, cuáles necesitan trabajo nuevo): GRAMMAR.md §3.112, PLAN.md §9.10.

## [1.74.0] - 2026-08-24

### ✨ Nuevo
- **`response.redirect(url, permanent: Bool) -> Void`.** Primer ítem resuelto de PLAN.md §9.9 (SEO y descubribilidad para IA, abierta a pedido explícito del usuario). Un redirect 301/302 real es SEO básico -- consolidar contenido duplicado, transferir el ranking de una URL vieja a la nueva -- y `response.setStatus` (§3.46) por sí solo no alcanzaba: fijar el status sin un header `Location` no es un redirect. Fija los dos a la vez (301 si `permanent`, 302 si no, más `Location: <url>`), mismo mecanismo interno que `setStatus` con un campo hermano nuevo para la URL. Rechaza `url` vacío o con salto de línea (inyección de headers HTTP) con un error de runtime limpio -- `url` es un `String` arbitrario que el propio rpc arma, no un header ya validado de una request entrante. Mismo límite que `setStatus` dentro de un `stream`: error de compilación, no no-op silencioso.

943 tests (5 nuevos): 3 de tipos en `checker.rs` + 1 en `runtime/mod.rs` (validación de `url`) + 1 end-to-end en `cli_content_type.rs` contra un `linkc serve` real, confirmando el status Y el header `Location` reales leídos del socket crudo. Detalle completo: GRAMMAR.md §3.111.

## [1.73.0] - 2026-08-24

### ✨ Nuevo
- **`crypto.awsS3PresignedUrl(accessKeyId, secretAccessKey, region, bucket, objectKey, expiresSeconds) -> String`.** Reportado por MyFinance: `DocumentStorageService` tenía una firma S3 FALSA (`?signature=hmac_verified`, un string literal) porque `crypto.hmacSha256` (String -> String) no alcanza para el encadenado de HMACs con BYTES CRUDOS que AWS Signature V4 exige -- confirmado como limitación real del primitivo existente, no como falta de documentación o de discoverabilidad. Arma la URL COMPLETA lista para usar (no solo la firma), resolviendo adentro del runtime todo el protocolo (canonical request, string-to-sign, derivación de clave de firma, URI-encoding exacto) para que ningún adoptador tenga que reimplementarlo a mano. Alcance acotado: solo `GET` (compartir/descargar), solo credenciales de larga duración, estilo virtual-hosted-style.

938 tests (8 nuevos): la derivación de clave + firma final reproduce BYTE A BYTE el vector de prueba OFICIAL que Amazon publica (`aws4_testsuite`, caso "get-vanilla") -- verificado sin necesitar ninguna cuenta de AWS real, mismo estándar que ya se usó para `crypto.hmacSha256`. Detalle completo: GRAMMAR.md §3.110.

### 📝 Nota de proceso
El primer intento de resolver este reporte fue recomendarle a MyFinance que armara la firma "a mano" con `crypto.hmacSha256` -- una recomendación que resultó ser INCORRECTA al verificarla (`hmacSha256` devuelve hex, no bytes crudos, y AWS Signature V4 necesita encadenar los bytes crudos de un HMAC como clave del siguiente). El error se corrigió antes de comunicarlo como solución final, pero es la razón por la que este ítem terminó siendo una función nueva del compilador en vez de quedar documentado como "ya se puede hacer con lo que existe".

## [1.72.0] - 2026-08-24

### ✨ Nuevo
- **`countWhere`/`findWhere` empujan una conjunción `&&` de varias hojas.** Pedido explícito del usuario tras revisar el estado de IgnisLove/MyFinance: resuelve los DOS casos reales de CRM que la ronda de un solo operador (v1.71.0) había dejado sin cubrir -- `notifications.link` (`n.userId == uid && !n.read`) e `inventory.link` (`p.stock <= 5 && p.stock > 0`, el MISMO campo dos veces). `ast::recognize_conjunction_predicate` reemplaza y generaliza `recognize_comparison_predicate`, recorriendo el árbol de `&&` recursivamente; también reconoce `!x.campo`/`x.campo` sueltos como hojas booleanas (`== false`/`== true`), sin ningún operador explícito. `runtime/db.rs::conjunction_condition` arma `"f1" op1 $1 AND "f2" op2 $2 AND ...` con un placeholder posicional por hoja. Alcance deliberado: solo `&&` -- `||` sigue sin pushear, sin evidencia real de demanda todavía.

930 tests (2 nuevos): 1 en `runtime/mod.rs` contra un SQLite en memoria real cubriendo los dos casos reales de CRM más las dos hojas booleanas sueltas + 1 en `pg_integration.rs` contra un PostgreSQL real confirmando que el `AND` con dos placeholders posicionales (`$1`/`$2`) bindea en el orden correcto. Un test existente que usaba un `&&` compuesto como ejemplo de predicado NO pusheable (agregado en v1.71.0) se corrigió otra vez, ahora a un `||`. Detalle completo: GRAMMAR.md §3.109. De paso se abrió PLAN.md §9.9, una nueva sección de backlog para SEO y descubribilidad de agentes de IA (sitemap/robots.txt declarativos, metadata SEO clásica, reglas para crawlers de IA, `llms.txt` auto-generado por proyecto, ejemplos estructurados en OpenAPI, redirects, `@cache_control`) -- todavía sin implementar, candidatos para próximas rondas.

## [1.71.0] - 2026-08-24

### ✨ Nuevo
- **`countWhere`/`findWhere` empujan a SQL `!=`/`<`/`<=`/`>`/`>=`, no solo `==`.** Reforzado por "CRM"/Nexus: tres casos reales de alta frecuencia (badge de notificaciones, alertas de stock, contador de chats sin leer) todavía traían la colección entera a memoria. `ast::recognize_comparison_predicate` generaliza el reconocimiento de shape (antes `recognize_equality_predicate`, solo `==`) a los cinco operadores relacionales restantes -- mismo criterio conservador de siempre, `|item: T| item.campo OP valor` en cualquier orden, con el operador "enderezado" cuando el campo aparece del lado derecho. Alcance deliberadamente acotado a UN SOLO operador por predicado -- `&&`/`||` compuesto sigue sin pushear (el ítem grande de verdad, PLAN.md §9.3.1 sigue abierto para eso): de los tres casos reales de CRM, solo `chat.link` (`c.unreadCount > 0`, un único operador) se beneficia de esta ronda.

928 tests (2 nuevos): 1 en `runtime/mod.rs` contra un SQLite en memoria real cubriendo los cinco operadores nuevos (incluido el caso del campo del lado derecho) + 1 en `pg_integration.rs` contra un PostgreSQL real con el caso exacto de `chat.link`. Un test existente que usaba `rating > 3` como ejemplo de predicado NO pusheable se corrigió a un `&&` compuesto, ya que ese operador solo ahora sí pushea. Detalle completo: GRAMMAR.md §3.108.

## [1.70.0] - 2026-08-24

### ✨ Nuevo
- **`linkc serve-all --port-map-out <archivo.json>`.** Gap encontrado analizando IgnisLove en profundidad: `serve-all` (v1.56.0) asigna puerto por orden ALFABÉTICO de los `.link` descubiertos, pero nada externo puede leer ese mapeo salvo replicándolo a mano -- el caso real: `server/cscript-gateway.ts` (gateway de producción, 13 servicios) hardcodea un mapa nombre→puerto con un comentario propio admitiendo el riesgo de desactualizarse si se agrega/quita/renombra un `.link`. Escribe `{"nombre_archivo": puerto, ...}` a un JSON ANTES de arrancar cualquier servicio -- si la escritura falla, no arranca nada (mejor que un gateway leyendo un mapeo viejo o inexistente). No cambia la regla de asignación en sí, solo la hace legible desde afuera.

926 tests (2 nuevos) en `cli_serve_all.rs` contra el binario real: el JSON escrito antes de servir tiene la asignación correcta; un destino sin permiso de escritura falla limpio y no arranca ningún servicio. Detalle completo: GRAMMAR.md §3.107.

## [1.69.0] - 2026-08-24

### ✨ Nuevo
- **Lint `delete-then-insert-same-id`.** Gap encontrado analizando IgnisLove en profundidad: varios `.link` del repo (`bandit_rewards`, `bot_defense`, `stock_cache`, `catalog_facets`, `seo_engine`, `rfm_scorer`) documentan en comentarios propios por qué migraron de "borrar e reinsertar" a `upsert`/`applyPatch` -- "delete+insert con autoincrement no reproduce el id" -- pero `banners.link` todavía no había migrado. El motivo real, no solo de estilo: `insert()` SIEMPRE asigna un id nuevo por autoincrement (GRAMMAR.md §3.17), nunca respeta el valor que un literal declara para `id` -- `db.<c>.delete(x.id); db.<c>.insert(T { id: x.id, ... })` NO preserva la fila aunque el código lo intente, y cualquier referencia externa al id viejo queda apuntando a una fila borrada. Shape detectado (mismo criterio "chico y ancho" del resto del linter): `delete(X)` seguido más adelante en el mismo bloque de `insert(Tipo { id: X, ... })` sobre la MISMA colección con la MISMA expresión `X` -- distinta colección (archivar) o distinto id (reemplazar por otra fila) no disparan. Puramente informativo, `linkc lint` sigue saliendo con código 0.

924 tests (4 nuevos) en `lint.rs`: el caso real de `banners.link` dispara; colección distinta, id distinto, e insert sin delete previo no disparan. Detalle completo: GRAMMAR.md §3.106.

## [1.68.0] - 2026-08-24

### ✨ Nuevo
- **`db.<c>.increment(id, selector, delta) -> T`.** Gap encontrado analizando IgnisLove en profundidad, con un riesgo de producción real como evidencia (lost-update estructuralmente posible, no ya materializado): tres `.link` (`bandit_rewards`, `bot_defense`, `banners`) hacían read-then-write manual con `upsert`/`updateFn` para contadores (`totalPulls`, `requestCount`, `impressionsCount`/`clicksCount`) -- en la topología real de este adoptador (varios procesos `linkc serve-all`/pm2 compartiendo un único Postgres), dos procesos pueden leer el mismo valor antes de que el otro escriba y perder un incremento. `increment` hace un `UPDATE "campo" = "campo" + ?` real, sin ninguna lectura previa -- la atomicidad la da el motor (row-level locking de la propia `UPDATE`), no ningún mecanismo de c-script. `delta` negativo decrementa. Alcance acotado a `Int` en esta ronda (Int64/Float deliberadamente afuera, sin evidencia real de demanda). Compone gratis con `@check` (la violación se sigue rechazando a 400) y con `@softDelete` (alcanzable por `id` directo, mismo criterio que `find`/`applyPatch`). `id` inexistente falla con un error claro, mismo criterio que `applyPatch`.

920 tests (8 nuevos): 5 en `checker.rs`, 2 en `runtime/mod.rs` contra un SQLite en memoria real, y **1 en `pg_integration.rs` que es la prueba real del punto entero de esta feature**: 20 hilos, cada uno con su propia conexión HTTP, incrementando la MISMA fila 25 veces cada uno (500 incrementos concurrentes) contra un Postgres real -- el conteo final da EXACTO, sin perder ni uno. Detalle completo: GRAMMAR.md §3.105.

## [1.67.0] - 2026-08-24

### 🐛 Arreglado
- **`Float` decodifica `numeric`/`decimal` nativo de Postgres.** Tercer reporte de MyFinance, verificando en su propio esquema real el fix de fechas de v1.55.0: "todos los importes monetarios del schema real son `numeric`, nunca `float` -- Float no los decodifica". `numeric` es un formato binario de precisión arbitraria que `postgres-types` no sabe leer como `f64` -- decodificado a mano (mismo espíritu que el fix de fechas, sin sumar `rust_decimal`). Solo lectura por ahora. GRAMMAR.md §3.103.
- **Escritura de `Int` contra una columna Postgres no-`BIGINT` (`SERIAL`/`SMALLINT`).** Encontrado auditando por qué CI llevaba varios pushes en rojo -- ver la nota de proceso abajo. `i64::to_sql` (crate `postgres-types`) ignora el ancho real de columna y siempre manda 8 bytes; contra una columna `int4`/`int2` (típico al adoptar una tabla `SERIAL`) eso corrompe el protocolo binario. `Cell::to_sql` ahora despacha por el `ty` real que pide el servidor. GRAMMAR.md §3.104.

### 📝 Nota de proceso (importante)
Auditando por qué `gh run list` mostraba **~10 pushes consecutivos en rojo** (desde v1.58.0), aparecieron dos causas separadas, ninguna relacionada con features nuevas de esta sesión:
1. **`examples/users.link.snap` sin regenerar desde v1.48.0** -- el snapshot embebe el número de versión exacto, así que cada bump posterior lo dejaba desincronizado. Regenerado (`linkc test ... --update`).
2. **`pg_integration.rs` nunca corrió contra un Postgres real localmente esta sesión** (no había uno disponible en el entorno) -- varios "Verificado" en GRAMMAR.md se apoyaban en lectura de código, no en ejecución real. Al conseguir acceso a un Postgres local se encontraron y arreglaron: el bug de escritura de arriba (real, de producto) y **4 bugs de TESTS** (no del compilador) que tampoco habían corrido nunca: un test de `Timestamp` con nombres de campo en camelCase contra columnas físicas en snake_case; un test de `@check` comparando `postgres::Error::Display` (que siempre es el literal `"db error"`, nunca el detalle real) en vez de `.as_db_error()`; un test de aviso de colisión con un campo requerido que quedaba `NULL` tras la migración de la segunda tabla, disparando un guard correcto pero ajeno a lo que ese test probaba; y un test de `linkc introspect` que reusaba el `db {...}` de la base ENTERA (introspect no filtra por tabla) en vez de acotarse a la tabla bajo prueba, sensible a qué otras tablas hubiera creado algún otro test corriendo en paralelo.

"Tests verdes localmente" y "CI verde" no son la misma promesa -- solo la segunda lo es de verdad. 911 tests, todos verificados contra Postgres real en esta ronda, no solo por lectura de código.

## [1.66.0] - 2026-08-24

### ✨ Nuevo
- **`db.<c>.maxRow(selector)` / `db.<c>.minRow(selector) -> T?`.** Gap nuevo encontrado analizando IgnisLove en profundidad, con un bug de producción real como evidencia: `bandit_rewards.link` (`getBestArm()`) hacía `db.arms.all()` y devolvía `allArms[0]` -- el orden de `all()` es por `"id"`, NUNCA por el campo de recompensa, así que ese rpc jamás devolvía el brazo con mejor `avgRewardTenths` pese a su nombre, un algoritmo de optimización silenciosamente roto. `maxBy`/`minBy` (§3.52) ya existían pero solo agregan un VALOR agrupado, nunca la fila completa que lo alcanza. Dos métodos nuevos (no uno con un parámetro `dir: "asc"|"desc"`, mismo criterio de "nombre por forma" que ya usa §3.52) empujan a `SELECT ... ORDER BY "<campo>" {DESC|ASC} LIMIT 1` real -- mismo shape reconocido y mismas restricciones de tipo (`Int`/`Int64`/`Float`, nunca opcional) que el campo de valor de `maxBy`/`minBy`, mismo respeto de `@softDelete`. `Value::Null` sobre una colección vacía, nunca un error.

911 tests (8 nuevos): 5 en `checker.rs`, 2 en `runtime/mod.rs` contra un SQLite en memoria real (reproduce el bug exacto de `getBestArm()`) y 1 en `pg_integration.rs` contra un PostgreSQL real. Detalle completo: GRAMMAR.md §3.102.

## [1.65.0] - 2026-08-24

### ✨ Nuevo
- **`List<Int>.sum() -> Int`.** Analizando en paralelo, por primera vez, tres adoptadores reales a la vez (IgnisLove, "CRM"/Nexus, Glowapp), este gap salió de "CRM" con un bug de producción real como evidencia: `accounting.link` (`getAccountingSummary`) necesitaba sumar montos ya filtrados en memoria y, al no existir forma de sumar sin un `while` manual, el código quedó con un placeholder que multiplica la CANTIDAD de transacciones por una tarifa plana inventada en vez de sumar los montos de verdad -- un reporte financiero con cifras fabricadas, no aproximadas. Alcance deliberadamente acotado a `List<Int>` -- `List<Int64>`/`List<Float>` quedan afuera a propósito: en runtime `Value::List` no lleva tag de tipo de elemento, así que una lista VACÍA de esos dos tipos no tendría de dónde sacar qué `Value` devolver sin construir infraestructura de recuperación de tipo estático que esta ronda no amerita para un solo método. El checker rechaza esos dos casos con un mensaje que nombra el motivo explícito, no un "método no encontrado" genérico.

903 tests (6 nuevos): 4 en `checker.rs` (tipa sobre `List<Int>`, rechaza `List<Int64>`/`List<Float>` con mensaje claro, sin argumentos) y 2 en `runtime/mod.rs` contra un servidor real vía `invoke_rpc` (suma real de una lista no vacía; lista vacía da `0` -- el caso que el placeholder de "cantidad × tarifa" jamás hubiera distinguido de "una transacción gratis"). Detalle completo: GRAMMAR.md §3.101.

### 📝 Nota
- **Primer análisis paralelo de tres adoptadores reales a la vez** (IgnisLove, "CRM"/Nexus -- primer análisis de este, 11 `.link` -- y Glowapp -- confirmado que NO usa c-script), solo lectura, sin modificar nada de sus repos. Además de `.sum()` (arriba), quedaron documentados en PLAN.md, priorizados para próximas rondas: `db.<c>.increment()` e `db.<c>.top()` (§9.3, ambos con bug de producción real confirmado en IgnisLove -- lost-update en contadores, y `getBestArm()` que nunca devuelve el mejor brazo), predicado pushdown más allá de `==` re-priorizado (§9.3.1, tres casos de alta frecuencia nuevos en CRM: badge de notificaciones, alertas de stock, contador de chats), `smtp` con adjuntos/cc/bcc re-priorizado (§9.6.1, CRM abandonó el módulo por completo), tareas programadas nativas re-priorizadas (§9.7.7, 10+ schedulers hand-rolled en Glowapp), `@rate_limit` con clave adicional a la IP (§9.4.6, caso de abuso real en Glowapp), `linkc serve-all --port-map-out` (§9.7.6, gateway de producción en IgnisLove hardcodea el mapeo de puertos a mano), índice único condicional (§9.3.2, junto al compuesto), lint para el antipatrón `delete()`+`insert()` (§9.3.9).

## [1.64.0] - 2026-08-24

### ✨ Nuevo
- **`linkc doctor <archivo.link> [--db <url|archivo>]`.** PLAN.md §9.7.1, en el backlog general desde antes de los reportes de adopción reciente: "diagnóstico de entorno (versión, PATH, permisos, conectividad a la DB configurada) antes de un despliegue". Elegido como siguiente ítem al cerrar la lista priorizada de IgnisLove/MyFinance por ser de bajo riesgo de regresión (lectura pura, sin tocar ningún camino de código ya modificado esta sesión). Cuatro chequeos independientes entre sí -- uno que falla no cancela los demás: (1) versión de `linkc`; (2) que el `.link` de entrada resuelva sus imports, parsee y tipe, con el mismo diagnóstico snippet+caret de `linkc <archivo.link>` si no; (3) permiso de escritura en el directorio del `.link` (crea y borra un archivo de prueba ahí); (4) conectividad de SOLO LECTURA (`SELECT 1`, reusando `connect_postgres_client` de `linkc migrate --dry-run`, nunca ningún DDL) a la base configurada vía `--db`/`LINK_DATABASE_URL` -- informativo si es SQLite embebido (default sin configurar nada). La credencial de una URL de Postgres se enmascara siempre en el reporte. "PATH" del ítem original reinterpretado a propósito: un binario estático sin ningún ejecutable de sistema del que depender no gana nada real inspeccionando esa variable de entorno -- el chequeo que sí importa antes de desplegar es que el programa de entrada compile. Código de salida `1` si algún chequeo real falló, pensado para un paso de CI.

897 tests (8 nuevos): 7 en `cli_doctor.rs` contra el binario real (éxito con SQLite default, archivo faltante, error de sintaxis, URL de Postgres inalcanzable sin colgarse ni panic, URL malformada sin panic, uso sin argumentos, `LINK_DATABASE_URL` igual que `--db`) y 1 en `pg_integration.rs` contra un PostgreSQL real, confirmando `[OK]` de conectividad Y que ninguna tabla se creó. Detalle completo: GRAMMAR.md §3.100.

## [1.63.0] - 2026-08-24

### ✨ Nuevo
- **`linkc test --db <url-postgres>`.** Segundo reporte de MyFinance, verificando el fix de decodificación de Postgres (v1.55.0, GRAMMAR.md §3.91) contra su propio esquema real: "`linkc test` corre contra SQLite embebido, que NO reproduce el bug original... sin esto, el fix está 'compilado y probado con datos falsas', no 'verificado'". El motor de `test "..." { ... }` siempre creaba SQLite `:memory:` nueva por cada test, sin ninguna forma de apuntar a Postgres -- los dos backends emiten SQL y decodifican el wire de forma distinta, así que pasar contra SQLite no prueba nada sobre cómo se comporta contra Postgres de verdad. `--db <url-postgres>`/`LINK_TEST_DB` (env var deliberadamente DISTINTA de `LINK_DATABASE_URL`, para que `linkc test` nunca use por accidente la URL de producción/desarrollo de `linkc serve`) corre TODOS los bloques `test` contra esa base real en vez de SQLite. Límite honesto, documentado explícitamente: sin el aislamiento por test que `:memory:` da gratis -- Postgres no tiene equivalente de "`:memory:`" (reconectar a la misma URL da el MISMO estado persistente, no uno fresco), así que en vez de fingir un reset automático (que sería una operación destructiva, evitada a propósito, mismo criterio que `linkc migrate --dry-run`), los tests comparten estado explícitamente: lo que uno inserta, el siguiente lo ve. Solo PostgreSQL; `--adopt-existing` funciona igual que en `linkc serve`.

889 tests (2 nuevos) en `pg_integration.rs` contra un PostgreSQL real: un `test` que inserta una fila la deja de verdad en Postgres (confirmado con una consulta directa, no solo "el test pasó"); dos `test` en el mismo archivo, el segundo ve el conteo que el primero dejó -- confirma el límite de "sin aislamiento" documentado, no lo esconde. Detalle completo: GRAMMAR.md §3.99.

## [1.62.0] - 2026-08-24

### ✨ Nuevo
- **Lint `hardcoded-secret-literal`.** PLAN.md §9.5.3: "que `linkc lint` avise si detecta una URL de conexión o API key literal en el código". Un `const NOMBRE: String = "..."` de nivel superior se marca en dos casos: el literal tiene forma de URL de conexión con credenciales embebidas (`postgres://usuario:contraseña@resto`, y equivalentes de `postgresql`/`mysql`/`mongodb`/`redis`/`amqp` -- una URL sin credenciales no dispara), o el NOMBRE del `const` sugiere un secreto (mismo heurístico laxo que `timing-unsafe-secret-comparison`, v1.53.0) con un valor no vacío. El mensaje recomienda `env.get("...")` -- pero como reemplazo del `const` en sí (no declararlo, leer el valor con `env.get` en el momento que se necesita, dentro del rpc/fn), nunca como el valor de un const: un `const` en c-script solo admite literales, `env.get(...)` ahí es un error de compilación aparte (`check_const`, checker.rs). Puramente informativo, como el resto del linter -- `linkc lint` sigue saliendo con código 0.

887 tests (6 nuevos) en `lint.rs`. Detalle completo: GRAMMAR.md §3.98.

## [1.61.0] - 2026-08-24

### ✨ Nuevo
- **`linkc migrate --dry-run`.** Octavo reporte de adopción real (IgnisLove): "antes de apuntar cualquier servicio a una tabla real con `--adopt-existing`, ver el DDL exacto que se ejecutaría sin aplicarlo todavía sería la verificación que le falta a ese paso". `linkc migrate <archivo.link> --db <url-postgres> --dry-run` (módulo nuevo, `src/migrate.rs`) conecta de solo LECTURA a la base y reporta el `CREATE TABLE`/`ALTER TABLE ADD COLUMN` exacto que `linkc serve`/`linkc serve-all` ejecutarían al conectar de verdad -- sin ejecutar nada. Reusa las MISMAS funciones puras de generación de DDL que ya usa el runtime real (`codegen::postgres_emit::create_postgres_table_sql`/`alter_table_add_column_postgres`, `runtime::db::create_index_statements`), nunca una copia propia que pudiera desincronizarse con el tiempo. También corre el chequeo de "¿esta tabla parece de otro programa?" (v1.58.0, GRAMMAR.md §3.94) y de tipo de `"id"` compatible ANTES de que alguien intente conectar en serio, no solo el diff de columnas. Solo PostgreSQL: SQLite ya falla fuerte con el diff exacto al conectar de verdad, antes de tocar nada. Sin `--allow-destructive` -- auditando la migración real de Postgres apareció que hoy no existe ningún camino destructivo que advertir (solo crea tablas nuevas y agrega columnas siempre nullable, nunca borra ni cambia tipos), así que el flag no tendría nada que hacer todavía. `linkc migrate` sin `--dry-run` se rechaza explícito: aplicar de verdad ya pasa automáticamente al conectar con `linkc serve`.

881 tests (2 nuevos) en `pg_integration.rs` contra un PostgreSQL real: una colección nueva muestra el `CREATE TABLE` exacto (con `@check` inline) y confirma que la tabla no se creó de verdad; una tabla existente con una columna faltante muestra el `ALTER TABLE ADD COLUMN` exacto y confirma que la columna no se agregó de verdad. Detalle completo: GRAMMAR.md §3.97.

### 📝 Nota
- **`--db-schema`/`--db-prefix` (namespacing para compartir una base Postgres entre varios `.link`), deliberadamente diferido tras auditarlo.** A diferencia del resto de ítems de esta ronda, necesitaría enhebrar el prefijo/schema por decenas de sitios en `runtime/db.rs`/`codegen/postgres_emit.rs`/`introspect.rs` que hoy arman SQL con el nombre de colección tal cual -- un refactor genuinamente grande, con más riesgo de regresión que cualquier feature de esta sesión. Documentado como pendiente explícito en PLAN.md §9.3, para una ronda propia.

## [1.60.0] - 2026-08-24

### ✨ Nuevo
- **`@check(min/max/range, ...)`: constraints numéricos de nivel de base.** Séptimo reporte de adopción real (IgnisLove), con el ejemplo exacto: "`reviews.link` solo evita un rating fuera de 1-5 porque `clampRating()` lo fuerza en el código; no hay ninguna barrera a nivel de base si algún día otro rpc inserta sin pasar por esa función". Tres formas -- `@check(min, N)`, `@check(max, N)`, `@check(range, N, M)`, mismo criterio "kind + argumento(s)" que `@validate(email)`/`@validate(regex, ...)` -- sobre un campo `Int`/`Int64`/`Float` (requerido u opcional). Enforcement DOBLE: del lado de la aplicación (`apply_field_validators`, mismo mecanismo y los mismos dos puntos de entrada que `@validate` -- wire y struct construido dentro de un rpc -- 400 nombrando el campo y el límite exacto) Y del lado de la BASE (`CHECK (...)` inline de verdad en el `CREATE TABLE`, en SQLite y en PostgreSQL, el mismo generador que usa `linkc build` para `schema.postgres.sql`) -- confirmado escribiendo SQL crudo, sin pasar por c-script en absoluto, y viendo el `INSERT` rechazado por el propio motor, en los DOS backends. `--adopt-existing` nunca ejecuta este DDL (mismo criterio que `@index`/`@unique`), pero la validación de aplicación sigue protegiendo `insert`/`applyPatch` sin importar el modo. Alcance deliberado de esta ronda: solo rangos numéricos simples, ninguna expresión booleana arbitraria ni constraint sobre `String`.

879 tests (11 nuevos): 5 en `checker.rs`, 1 en `codegen/postgres_emit.rs` (el DDL estático lleva el `CHECK` exacto), 4 en `runtime/mod.rs` contra un servidor real vía `invoke_rpc`, 1 en `runtime/db.rs` contra SQLite real (un `INSERT` crudo se rechaza a nivel de SQLite) y 1 en `pg_integration.rs` contra un PostgreSQL real (mismo criterio). Detalle completo: GRAMMAR.md §3.96.

## [1.59.0] - 2026-08-24

### ✨ Nuevo
- **`db.<c>.countWhere(predicate) -> Int` + `findWhere` empujado a SQL para `x.campo == valor`.** Sexto reporte de adopción real (IgnisLove): "agregué `@index` a `reviews.productId`/`telemetry.sessionId` y no aceleró nada -- cada `.filter()`/`findWhere` sigue trayendo la tabla entera a memoria". Cierto: `findWhere`/`deleteWhere` siempre evaluaron su predicado en el intérprete, trayendo la colección COMPLETA con `all()` primero, a diferencia de `sumBy`/`countBy`/etc. `countWhere` (builtin nuevo, mismo contrato de tipos que `findWhere`/`deleteWhere`) reconoce ESTÁTICAMENTE si el predicado tiene la forma exacta `|x| x.campo == valor` (un literal o una variable capturada del entorno externo, nunca otro campo de `x`) y, si la tiene, lo traduce a `SELECT COUNT(*) ... WHERE` real -- cero filas viajan del motor al proceso. `findWhere` gana el mismo atajo (mismo reconocimiento, trayendo las columnas reales en vez de `COUNT(*)`) sin cambiar su firma ni su comportamiento observable. Cualquier otro predicado (`>`/`<`/`!=`, `&&`/`||`, comparar dos campos entre sí, una columna JSON) sigue funcionando exactamente igual que antes por el camino interpretado -- nunca un error, solo sin el atajo. Respeta `@softDelete` incluso pusheado. `deleteWhere` NO gana este atajo todavía (sigue trayendo todo y borrando fila por fila) -- publicar cada fila borrada a los suscriptores de `stream` complica un `DELETE` de una sola sentencia, queda para una ronda aparte junto con operadores más allá de `==`.

867 tests (6 nuevos): 1 en `checker.rs` y 5 en `runtime/mod.rs` contra un servidor real vía `invoke_rpc` -- `countWhere`/`findWhere` correctos vía el atajo de SQL, correctos por fallback ante un predicado no pusheable, el caso especial `"id"`, y `@softDelete` respetado incluso pusheado. Detalle completo: GRAMMAR.md §3.95.

## [1.58.0] - 2026-08-24

### 🐛 Arreglado
- **Aviso de colisión de nombre de tabla en PostgreSQL.** Quinto reporte de adopción real (el propio caso del equipo de c-script): `telemetry.link` estuvo a punto de chocar contra una tabla `events` real de otro servicio -- evitado a mano, no porque el runtime lo hubiera señalado. `CREATE TABLE IF NOT EXISTS` es un no-op sobre una tabla que ya existe -- no miraba si sus columnas tenían algo que ver con lo declarado, así que la migración no destructiva de Postgres (`ADD COLUMN IF NOT EXISTS`) le agregaba, en silencio, TODAS las columnas del programa nuevo a una tabla ajena. Ahora, antes de migrar una tabla preexistente, si NINGUNA columna declarada (aparte de `id`) coincide por nombre con las que la tabla ya tiene, se imprime una advertencia por stderr nombrando ambos conjuntos de columnas. Deliberadamente solo advierte, nunca bloquea el arranque: dos `.link` distintos compartiendo una tabla con columnas disjuntas es un caso ya soportado y probado a propósito (GRAMMAR.md §3.17), indistinguible de una colisión accidental mirando solo "cero columnas en común" -- convertirlo en error habría roto ese caso legítimo. Solo Postgres: SQLite ya fallaba fuerte ante cualquier diferencia de schema real.

861 tests (2 nuevos) en `pg_integration.rs` contra un PostgreSQL real: dos `.link` con cero columnas en común sobre la misma tabla conectan y sirven normal, con la advertencia visible en stderr; una tabla evolucionando con al menos una columna compartida NO dispara ninguna advertencia; los tests preexistentes del caso legítimo de columnas disjuntas se re-confirmaron sin cambios. Detalle completo: GRAMMAR.md §3.94.

## [1.57.0] - 2026-08-24

### ✨ Nuevo
- **`--service-api-key <clave>`/`LINK_SERVICE_API_KEY`: autenticación servidor-a-servidor.** Cuarto reporte de adopción real (IgnisLove): un gateway Node.js hace `fetch` sin ninguna autenticación contra cada `linkc serve` que orquesta, confiando en que el puerto no sea alcanzable desde afuera -- `--host 127.0.0.1` (v1.46.0) ya cerraba la mitad EXTERNA de ese hueco, pero cualquier OTRO proceso en la misma máquina con acceso a loopback podía llamarlos exactamente igual que el gateway legítimo. `@requires`/JWT no resuelven esto: autentican a un USUARIO final, no a QUIÉN hace la llamada de red. Con el flag/env var puesto, toda request que no sea `/`/`/health`/`/status` necesita el header `X-Service-Api-Key` con el valor exacto -- comparado en tiempo constante (reusa `constant_time_eq`, la misma función de `crypto.timingSafeEqual`), verificado ANTES de leer el body. Capa DISTINTA y ANTERIOR a sesiones/JWT -- las dos conviven: una request típica lleva `X-Service-Api-Key` (prueba que viene del gateway) Y `Authorization: Bearer <token>` (prueba de qué usuario). Sin el flag: comportamiento idéntico al de siempre. Funciona igual bajo `linkc serve-all` (v1.56.0), como valor global compartido.

859 tests (7 nuevos) en `cli_service_api_key.rs` contra el binario real: sin el flag nadie lo necesita; sin el header, 401 antes de llegar al rpc; con la clave incorrecta, 401; con la correcta, la request se procesa normal; `/health`/`/`/`/status` siguen respondiendo 200 sin el header; `LINK_SERVICE_API_KEY` funciona igual que el flag. Detalle completo: GRAMMAR.md §3.93.

## [1.56.0] - 2026-08-24

### ✨ Nuevo
- **`linkc serve-all <directorio> --port-base N`: un proceso para varios servicios.** Reporte de adopción real (IgnisLove): 13-17 `.link` desplegados como 13-17 procesos `pm2` separados, cada uno con su propio puerto, su propio SQLite y su propia línea de deploy -- confirmado con un incidente puntual, 68 reinicios de un servicio (`telemetry`) en un arranque en frío donde varios procesos competían por bindear sus puertos casi al mismo tiempo. `serve-all` descubre cada `.link` de un directorio, los compila TODOS antes de arrancar cualquiera (un workspace a medio levantar es peor que ninguno), y levanta uno por hilo del sistema operativo dentro de un ÚNICO proceso -- puerto `N`+posición alfabética, impresa explícitamente en cada arranque. Aislamiento de datos preservado: cada servicio conserva su propio archivo SQLite, por eso `--db`/`LINK_DATABASE_URL` compartido se rechaza de entrada (mismo motivo que la falta de detección de colisión de nombre de tabla, todavía sin resolver).
- **`--restart-backoff <duración>`/`LINK_RESTART_BACKOFF`: backoff exponencial nativo ante un fallo de bind/conexión.** Funciona en `linkc serve` y en `linkc serve-all` -- reemplaza la mitigación externa (`pm2 --restart-delay`, una espera fija) con una que dobla en cada fallo consecutivo (techo 30s, reseteada a la base tras 60s de funcionamiento estable). Auditando `runtime::server::serve` para esto apareció que un fallo de conexión a Postgres usaba `std::process::exit(1)` -- inofensivo bajo un proceso por servicio (como hoy), pero bajo `serve-all` se habría llevado puesto TODO el workspace por un solo servicio caído; `serve` ahora devuelve `Result<(), String>` en vez de terminar el proceso (`linkc serve` preserva el comportamiento externo de siempre, código 1 en el primer fallo sin el flag).

852 tests (9 nuevos) en `cli_serve_all.rs` contra el binario real, bindeando puertos y hablando HTTP de verdad: arranca 2 servicios en un solo proceso con sus propios `.db` separados; rechaza `--db`/`LINK_DATABASE_URL` compartido; falla limpio sin `--port-base` o sin ningún `.link`; un error de tipos en un archivo aborta TODO antes de arrancar cualquier hilo; un bind ocupado en un servicio no tumba al otro; y con `--restart-backoff`, un servicio se recupera solo cuando su puerto se libera mientras el otro sigue sano -- el incidente real reproducido y confirmado resuelto. Detalle completo: GRAMMAR.md §3.92.

## [1.55.0] - 2026-08-24

### 🐛 Arreglado
- **`dateFromParts(year, month, day, hour, minute, second) -> Timestamp` -- construir un `Timestamp` arbitrario.** Encontrado por un segundo reporte de adopción real de MyFinance (backend de cálculo de Modelos tributarios 130/303/347): §3.31 documentaba a propósito que un `Timestamp` v0 solo podía llegar de un parámetro de rpc o de la base, nunca construirse arbitrariamente dentro del backend -- `now()` resolvía el instante ACTUAL, pero calcular el límite de un trimestre (`año`/`trimestre` → fecha inicio/fin), que es exactamente lo que Modelo 130/303 necesita, seguía siendo imposible de escribir enteramente en el servidor. `dateFromParts` es un builtin sin receptor, mismo mecanismo exacto que `now()`, que reusa el parser/validador de calendario que ya existía para un `Timestamp` que llega por el wire -- una fecha inválida (mes 13, 30 de febrero) es `bad_request` (400) nombrando el campo mal formado, nunca un panic ni un 500.
- **`Timestamp` decodifica `date`/`timestamp`/`timestamptz` nativos de Postgres.** La otra mitad del mismo reporte: una tabla YA EXISTENTE adoptada (`--adopt-existing`/`linkc introspect`) casi siempre tiene sus columnas de fecha en el tipo NATIVO de Postgres, no en el `BIGINT` de milisegundos que `linkc build` genera para un `Timestamp` propio. Auditando `runtime/store.rs` apareció que esto estaba roto en los DOS sentidos: declarado `String` (lo que `linkc introspect` recomendaba automáticamente) fallaba al leer la primera fila real porque el wire binario de un `timestamp`/`date` no es texto UTF-8; declarado `Timestamp` TAMBIÉN fallaba, porque el OID nativo de esos tipos no coincidía con ninguno de los anchos de entero (`BIGINT`/`INTEGER`/`SMALLINT`) que la decodificación ya probaba. `ColumnKind::Timestamp` (nuevo) prueba en orden `BIGINT` propio, después `timestamp`/`timestamptz` nativo, después `date` nativo -- decodificado a MANO contra el wire binario de Postgres (sin sumar la dependencia `chrono`, mismo espíritu que el algoritmo de calendario de Hinnant que ya vivía en `runtime/timestamp.rs`). `linkc introspect` ahora recomienda `Timestamp` sin advertencia para estas columnas -- la recomendación anterior (`String` con advertencia) estaba, en los hechos, igual de rota. Alcance de esta ronda: solo LECTURA, escribir contra una columna nativa adoptada sigue sin funcionar (no era parte del caso reportado).

843 tests (19 nuevos): 12 en `runtime/timestamp.rs` (`dateFromParts` coincide con el ISO-8601 equivalente, rechaza cada campo fuera de rango nombrándolo, el caso real de un límite de trimestre; los dos epochs de Postgres -- timestamp y date -- verificados contra un ancla pública conocida, precisión truncada a milisegundos, fechas negativas), 1 en `checker.rs`, 3 en `runtime/mod.rs` contra un servidor real (cálculo de trimestre end-to-end, valor de primera clase, fecha inválida como 400), 2 en `introspect.rs` (mapeo exacto sin advertencia, `time` sin fecha sigue advirtiendo) y 1 en `pg_integration.rs` contra un PostgreSQL real (tabla con columnas de fecha nativas, sembrada con SQL crudo, adoptada y leída correctamente vía un rpc real) -- más un test end-to-end existente extendido con una columna `date` real. Detalle completo: GRAMMAR.md §3.90 y §3.91.

## [1.54.0] - 2026-08-24

### ✨ Nuevo
- **`--trust-proxy`/`LINK_TRUST_PROXY`: `@rate_limit` detrás de un proxy real.** `@rate_limit` (GRAMMAR.md §3.39) siempre identificó al cliente por `remote_addr()` -- la conexión TCP real -- deliberadamente, nunca por `X-Forwarded-For`, que cualquier cliente puede mandar con el valor que quiera. Correcto contra un cliente directo, pero detrás de un proxy o balanceador de verdad (confirmado como bloqueo real en producción: la adopción de IgnisLove corre todo detrás de nginx) `remote_addr()` es siempre la IP del proxy, la misma para cada request -- el límite termina compartido por TODOS los usuarios reales a la vez, no por cada uno. `--trust-proxy`/`LINK_TRUST_PROXY` (apagado por default, mismo criterio de flag booleano que `--adopt-existing`) hace que `@rate_limit` use el PRIMER valor de `X-Forwarded-For` (`cliente, proxy1, proxy2, ...`) en su lugar -- sin el header presente, cae de vuelta a `remote_addr()` tal cual. Opt-in explícito a propósito: prenderlo sin tener de verdad un proxy de confianza delante deja que cualquier cliente directo evada el límite por completo, mandando un header distinto en cada request. v0 sin validar cuántos proxies hay en el medio ni de qué IP vienen -- confía en el header completo en cuanto el flag está prendido, sin un mecanismo más fino de "N saltos de confianza" o un rango CIDR.

824 tests (5 nuevos) en `cli_rate_limit.rs` contra el binario real: sin `--trust-proxy`, `X-Forwarded-For` con valores distintos NO separa el balde (todo cuenta contra el mismo límite); con `--trust-proxy`, cada IP reenviada distinta tiene su propio balde, y el PRIMER hop de una cadena se usa correctamente; con `--trust-proxy` pero sin el header, cae de vuelta a `remote_addr()` sin romper nada; y `LINK_TRUST_PROXY` funciona igual que el flag. Detalle completo: GRAMMAR.md §3.89.

## [1.53.0] - 2026-08-24

### 🔒 Seguridad
- **Lint: `timing-unsafe-secret-comparison`.** `crypto.timingSafeEqual` (v1.26.0, GRAMMAR.md §3.54) existe justamente porque un `==` de `String` corta en el primer byte distinto -- comparar un token, contraseña o API key con el operador de siempre filtra, por cuánto tarda la comparación, cuánto acertó quien prueba. La función existe desde esa ronda, pero nada avisaba si el código de alguien seguía usando `==` sobre algo que PARECE un secreto. El linter (`linkc lint`) ahora marca cualquier `==`/`!=` donde uno de los dos lados es un `Ident` o el campo final de un `FieldAccess` cuyo nombre contiene (sin distinguir mayúsculas) `secret`, `token`, `password`, `apikey` o `api_key` -- deliberadamente laxo, mejor un falso positivo ocasional que dejar pasar el caso real. Comparar contra `null` (chequeo de presencia, `token != null`) queda afuera a propósito -- no hay ningún byte de secreto involucrado ahí. Recorre TODO el cuerpo del rpc/fn/test, en cualquier nivel de anidamiento (`if`/`match`/`while`/closure) -- encontrado auditando la propia implementación: la primera versión duplicaba cada warning que cayera adentro de un `while`, corregido antes de este release. Puramente informativo, como el resto del linter: `linkc lint` sigue saliendo con código 0.

819 tests (8 nuevos): 7 en `lint.rs` (dispara con `==` y con `!=`, `null` y nombres comunes no disparan nada, encuentra el caso dentro de `if`/`while`/closure, y el test de regresión que confirma exactamente UNA vez adentro de un `while`, no duplicado) y 1 en `cli_fmt_lint.rs` contra el binario real. Detalle completo: GRAMMAR.md §3.88.

## [1.52.0] - 2026-08-24

### ✨ Nuevo
- **`/health` verifica conectividad real a la base.** Hasta esta ronda `/health` (`/`/`/status`, mismo handler) devolvía `200 {"status":"ok",...}` fijo sin tocar la base para nada -- inútil para cualquier orquestador (Kubernetes, un load balancer) que lo usa para decidir si reiniciar el proceso o sacarlo de rotación: el proceso podía estar vivo y sin embargo incapaz de servir ningún rpc real porque la base estaba caída, y `/health` igual reportaba todo bien. `Db::health_check()` (nuevo) ejecuta un `SELECT 1` real en CADA request a `/health`, sin caché -- `200` si respondió, `503 Service Unavailable` si no, con `"status":"error"` y el mensaje real en un nuevo campo `"database"` del body. Del lado Postgres, el chequeo pasa por el mismo `with_reconnect` que cualquier otra query real -- una caída transitoria se autorepara ahí mismo, así que `/health` no solo reporta el estado, también participa de la reconexión automática.

811 tests (3 nuevos): 2 en `cli_health.rs` contra un servidor SQLite real (forma exacta del JSON en el camino feliz, listando los servicios declarados de verdad; `/`, `/health`, `/status` devuelven exactamente lo mismo) y 1 en `pg_integration.rs` (reusando la técnica de `pg_terminate_backend` del test de reconexión existente): `/health` pasa de 200/"ok" a 503/"error" mientras la conexión está cortada, y vuelve solo a 200 sin reiniciar el proceso. Detalle completo: GRAMMAR.md §3.87.

## [1.51.0] - 2026-08-24

### 🔒 Seguridad
- **`--http-timeout <duración>`/`LINK_HTTP_TIMEOUT`: timeout de llamadas salientes `http.*`.** Auditando `runtime/mod.rs` apareció que `http.get`/`post`/`getWithHeaders`/`getWithStatus`/`postWithStatus`/`postWithHeaders` no fijaban ningún timeout de lectura/escritura propio -- `ureq` (la crate) trae 30s de timeout de CONEXIÓN por default, pero el de lectura/escritura, el que importa una vez que la conexión ya abrió, es "nunca" por default, documentado así por la propia crate. Para este intérprete de un solo hilo, eso significaba que una request saliente a un servidor lento o que acepta la conexión y nunca responde bloqueaba el proceso ENTERO para siempre -- ninguna otra request, de ningún cliente, se atendía mientras tanto, ni siquiera `/health`. `--http-timeout`/`LINK_HTTP_TIMEOUT` (mismo orden de precedencia y formato `Ns`/`Nm`/`Nh`/`Nd` que `--session-ttl`, default 30s -- el mismo número que `ureq` ya usaba para conexión) fija un timeout total por llamada, guardado en `Db` con el mismo mecanismo exacto que `argon2_params` (fijado una vez al arrancar, leído en cada llamada saliente, sin enhebrar un parámetro nuevo por todo el árbol de evaluación). Un timeout agotado se reporta como cualquier otro error de red -- 500 de runtime, nunca un panic ni un colgado.

808 tests (3 nuevos) en `cli_http.rs` contra el binario real: una request a un servidor que acepta la conexión pero nunca responde corta cerca del `--http-timeout` configurado (medido con un `Instant` real), la variable de entorno funciona igual, y una duración inválida es un error de uso limpio sin panic. Detalle completo: GRAMMAR.md §3.86.

## [1.50.0] - 2026-08-24

### 🔒 Seguridad
- **`--max-body-bytes <N>`/`LINK_MAX_BODY_BYTES`: límite configurable de tamaño de body.** Hasta esta ronda `linkc serve` leía el body de CUALQUIER request entero a memoria antes de tocarlo (`request.as_reader().read_to_string(&mut body)`, sin ningún límite) -- confirmado leyendo `runtime/server.rs`. Ni auth, ni rate limiting, ni la forma del JSON tenían oportunidad de rechazar nada antes de esa lectura completa: un solo body enorme (a propósito o no) era un vector real de agotamiento de memoria del proceso entero. `--max-body-bytes`/`LINK_MAX_BODY_BYTES` (mismo orden de precedencia que el resto de los flags de `serve`, default 10 MiB) acota la lectura con `Read::take(max_body_bytes + 1)` -- el `+1` distingue "mide EXACTO el límite" (permitido) de "sigue después" (rechazado), sin leer más de un byte de más nunca -- y responde `413 Payload Too Large` ANTES de cualquier otro chequeo, sin haber leído el body completo primero. Límite de proceso, no por rpc; no se drena el resto de un body rechazado (si el cliente reusa la conexión igual, el siguiente intento da 400 y cierra -- nunca un colgado ni una fuga de memoria).

805 tests (9 nuevos) en `cli_max_body.rs` contra el binario real: un body bajo el default se acepta, uno EXACTO al límite configurado se acepta, uno de un byte más se rechaza con 413 nombrando el límite, un body mucho más grande también se rechaza (probando que la lectura se corta temprano, no que se lee entero primero), flag y env var funcionan por separado con el flag ganando, valores inválidos son errores de uso limpios sin panic, y los headers de seguridad/CORS siguen en la respuesta 413. Detalle completo: GRAMMAR.md §3.85.

## [1.49.0] - 2026-08-24

### ✨ Nuevo
- **`auth.destroyAllSessions(userId: Int) -> Int`: revocar todas las sesiones de un usuario.** Hasta esta ronda la única forma de cerrar una sesión era `auth.destroySession()`, que opera sobre la sesión que ya autenticó la request actual, deliberadamente sin tomar ningún token como argumento -- si tomara un token, cualquiera podría destruir la sesión de otro con solo adivinarlo. Eso dejaba sin resolver el caso real de "cambió su contraseña, o un admin lo está baneando -- hay que cerrar TODAS sus sesiones, en todos los dispositivos, ahora mismo". `destroyAllSessions`, a diferencia de `destroySession`, SÍ toma un identificador explícito -- mismo criterio que `createSessionWithId`: un `userId` es una clave de aplicación, no un secreto adivinable como un token. Devuelve la cantidad de sesiones borradas (`0` si no había ninguna). Disponible desde cualquier cuerpo de rpc, como el resto de los builtins de `auth` -- gatearlo con `@requires(Role.Admin)` es una decisión de quien escribe el `.link`, no algo que el runtime imponga por sí solo. Solo alcanza sesiones propias (`createSession`/`createSessionWithId`); un JWT externo no pasa por el store en memoria, así que no hay nada que revocar de ese lado.

796 tests (6 nuevos): 3 en `session.rs` (borra todas las sesiones de un usuario y devuelve la cantidad exacta, deja intactas las de otro usuario, un usuario sin sesiones da 0), 1 en `checker.rs` (toma exactamente un Int, tipa Int), 1 en `runtime/mod.rs` (dos sesiones del mismo usuario se revocan, una tercera de otro usuario sobrevive) y 1 contra un servidor HTTP real (dos tokens del mismo usuario pasan a dar 401 tras revocar, el de otro usuario sigue funcionando). Detalle completo: GRAMMAR.md §3.84.

## [1.48.0] - 2026-08-24

### ✨ Nuevo
- **`linkc --version`/`-v`/`version`, y la versión estampada en cada archivo generado.** Hasta esta ronda `linkc` no tenía NINGUNA forma de reportar su propia versión, y ningún archivo que `linkc build` genera decía con qué versión del compilador se había generado -- un `gen/` desactualizado en un equipo donde conviven varias versiones del compilador no tenía cómo detectarse por sí solo. `linkc::VERSION` (`env!("CARGO_PKG_VERSION")`, tomada de `Cargo.toml` en tiempo de compilación, nunca un string hardcodeado aparte) alimenta las dos cosas a la vez, así que no pueden desincronizarse entre sí: `linkc --version` la imprime tal cual (`linkc 1.48.0`), y el header de `contract.d.ts`/`client.ts`/`hooks.ts`/`validators.ts`/`schemas.ts` queda estampado con ella (`// Generado automáticamente por linkc v1.48.0 — no editar a mano.`). `openapi.json`, que no admite comentarios `//`, lleva la misma información en `x-generated-by` -- una extensión de VENDOR estándar de OpenAPI (prefijo `x-`) -- deliberadamente NO en `info.version`, que es la versión del API que el propio `.link` documenta, un concepto distinto que no había que mezclar. Puramente informativo: nada compara la versión estampada en un `gen/` viejo contra el binario que lo está sirviendo o reconstruyendo -- sirve para que una persona lo note mirando el archivo.

790 tests (4 nuevos): `cli_help.rs` (`--version`/`-v`/`version` imprimen exactamente `linkc <versión>` a stdout, código 0, nada por stderr -- comparado contra `env!("CARGO_PKG_VERSION")` leído en el propio test) y 3 en `codegen/*.rs` (los cinco emisores TS empiezan con el header versionado; `openapi.json` lleva `x-generated-by` con la versión, e `info.version` sigue siendo la del API, no la del compilador). Detalle completo: GRAMMAR.md §3.83.

## [1.47.0] - 2026-08-24

### ✨ Nuevo
- **`linkc test <archivo> --filter <nombre>`.** Hasta esta ronda, `linkc test archivo.link` siempre corría TODOS los bloques `test "..." { ... }` del programa -- para un archivo con decenas de tests, iterar sobre uno solo (que está fallando, o que se está escribiendo recién) significaba esperar a que todos los demás corrieran también en cada vuelta. `--filter <nombre>` acota la corrida a los tests cuyo NOMBRE CONTIENE ese substring, sensible a mayúsculas -- mismo criterio que `cargo test <substring>`, no un nombre exacto ni una regex. Un filtro que no matchea ningún nombre corre cero tests y termina con éxito, no es un error. Solo aplica al test runner integrado, nunca al testing de contrato (`linkc test archivo.link archivo.snap`, que compara `contract.d.ts`/`client.ts`/`validators.ts` contra un snapshot y no tiene nombres que filtrar) -- combinar los dos es un error de uso claro en vez de un `--filter` ignorado en silencio.

786 tests (6 nuevos): 1 en `runtime/mod.rs` (`run_program_tests_filtered`: un filtro que matchea un subconjunto corre solo esos, uno que no matchea nada corre cero sin fallar, `None` corre TODOS -- idéntico a sin filtro) y 5 en `cli_test_runner.rs` con el binario real (filtra por substring, substring parcial también matchea, cero coincidencias termina limpio, `--filter` sin valor y combinado con un path de snapshot son errores de uso claros sin panic). Detalle completo: GRAMMAR.md §3.82.

## [1.46.0] - 2026-08-24

### ✨ Nuevo
- **`--host <dirección>`/`LINK_HOST` para `linkc serve`.** Hasta esta ronda el servidor siempre escuchaba en `0.0.0.0` (todas las interfaces de red de la máquina), sin ninguna alternativa -- confirmado leyendo `runtime/server.rs`, no existía ningún flag ni variable de entorno para acotarlo. Para un proceso que solo necesita aceptar conexiones locales (detrás de un proxy en el mismo host, o en una máquina de desarrollo con otras cosas corriendo), eso dejaba el firewall del sistema operativo como la ÚNICA capa de defensa -- un gap de seguridad real, no solo de conveniencia. Mismo orden de precedencia que el resto de los flags de `serve`: `--host` en la línea de comandos, si no la variable `LINK_HOST`, si no `"0.0.0.0"` de siempre (sigue siendo el valor correcto para el `ENTRYPOINT` que `linkc docker` genera, donde el proceso ya corre en su propio namespace de red de contenedor). El valor se pasa tal cual a `tiny_http::Server::http`, sin resolución ni validación propia más allá de rechazar `--host ""` vacío -- una dirección que no le pertenece a ninguna interfaz local hace fallar el bind al arrancar, con un mensaje que nombra la dirección exacta, nunca cae en silencio a `0.0.0.0`.

780 tests (7 nuevos) en `cli_host.rs` contra el binario real como subproceso: el default sigue aceptando una conexión por loopback, `--host 127.0.0.1`/`LINK_HOST=127.0.0.1` sirven igual por loopback, una dirección que no le pertenece a ninguna interfaz local (`192.0.2.1`, TEST-NET-1 de RFC 5737, para no depender de una segunda interfaz real en la máquina de test) hace fallar el arranque nombrando esa dirección -- probando que el valor de verdad se usa para bindear, no se ignora en silencio --, el flag le gana a la variable de entorno, y tanto `--host` sin valor como `--host ""` son errores de uso limpios sin panic. Detalle completo: GRAMMAR.md §3.81.

## [1.45.0] - 2026-08-24

### ✨ Nuevo
- **Índices declarativos de un solo campo: `@index`/`@unique`.** Hasta esta ronda la única columna indexada de cualquier tabla era la PK (`id`) -- cualquier otra búsqueda frecuente hacía un table scan completo, y no había forma de pedirle a la base una restricción de unicidad real: prevenir un email repetido solo se podía hacer a mano, con una lectura previa expuesta a una carrera entre dos requests concurrentes. `@index` (sin paréntesis, sobre cualquier campo) y `@unique` (índice + restricción de unicidad) son dos anotaciones de campo nuevas -- a lo sumo UNA de las dos por campo, rechazado en el parser si se repiten o se combinan (mismo criterio de forma que `@autoUpdate`/`@softDelete`). A diferencia de esas dos, ninguna exige un tipo de campo particular: un índice SQL tiene sentido sobre casi cualquier columna. El índice se crea de verdad al arrancar en LOS DOS backends (`CREATE [UNIQUE] INDEX IF NOT EXISTS "idx_<tabla>_<campo>" ...`, idempotente, mismo nombre determinístico en cada arranque), y `linkc build` emite la MISMA sentencia en el DDL estático que genera para Postgres. Una violación de `@unique` en `insert`/`applyPatch` (y por lo tanto en la rama de update de `upsert`) se traduce a 400, no a un 500 genérico -- detectando el mensaje específico que SQLite (`UNIQUE constraint failed`) y Postgres (`duplicate key value violates unique constraint`) devuelven para esta violación puntual; cualquier otra falla de SQL sigue siendo un 500. `--adopt-existing` nunca ejecuta este DDL, ni siquiera para un campo anotado -- mismo criterio ya establecido para el resto del schema en modo adopción.

  Límite deliberado de esta ronda: solo índices/constraints de UN campo. Un índice o `@unique` COMPUESTO (de varios campos a la vez) queda pendiente -- necesitaría una anotación a nivel de `type`, no de campo, que hoy no existe.

773 tests (9 nuevos): 4 en `checker.rs`/`parser.rs` (`@index`/`@unique` tipan limpio sobre cualquier tipo de campo, una segunda `@index` o `@unique` en el mismo campo es error de parser, combinar las dos en el mismo campo también), 4 en `runtime/db.rs` contra SQLite real (`@unique` crea un índice `UNIQUE` de verdad -- verificado leyendo `sqlite_master` -- y rechaza un segundo `insert`/`applyPatch` con el mismo valor devolviendo 400; `@index` sin `unique` no bloquea valores repetidos; `--adopt-existing` no crea el índice aunque el campo esté anotado) y 1 en `postgres_emit.rs` (el DDL estático de `linkc build` emite `CREATE UNIQUE INDEX`/`CREATE INDEX` con el mismo nombre determinístico que usa el runtime). Detalle completo: GRAMMAR.md §3.80.

## [1.44.0] - 2026-08-24

### ✨ Nuevo
- **`linkc build --diff <archivo-anterior>`.** Revisar un PR que toca un `.link` significa, en la práctica, revisar qué cambió en el CONTRATO público generado (`contract.d.ts`), no el `.link` mismo (eso ya lo muestra `git diff` normal) -- hasta esta ronda no había forma de pedirle eso al compilador, había que generar los dos contratos a mano y diffearlos con una herramienta aparte. `--diff <archivo>` compara el `contract.d.ts` recién generado contra el contenido de `<archivo>` (típicamente guardado con `git show <rev>:ruta > archivo` antes del build), línea por línea, reusando el mismo diff LCS (programación dinámica, sin dependencia nueva) que `linkc test` ya usaba para mostrar por qué un snapshot dejó de coincidir. Puramente informativo -- a diferencia de `linkc test`, nunca hace fallar el build: un archivo de comparación ilegible imprime una advertencia por stderr y el build sigue siendo exitoso igual. Solo compara `contract.d.ts` (no `client.ts`/`validators.ts`/`hooks.ts`/`schemas.ts`/`openapi.json`), y es un diff de texto plano, no semántico -- no distingue un campo nuevo (compatible) de un tipo cambiado (que rompe), eso lo decide quien lee el diff.

764 tests (4 nuevos) en `cli_build_diff.rs` contra el binario real como subproceso: agregar un campo muestra exactamente la línea `+` que corresponde, sin cambios reales muestra "no cambió", un archivo de comparación inexistente no hace fallar el build (solo avisa por stderr), y `linkc build` sin `--diff` sigue funcionando exactamente igual que antes. Detalle completo: GRAMMAR.md §3.79. Con esto, **PLAN.md §9.3 (Base de Datos y Consultas) tiene 9 ítems abiertos**: `count(predicate)` y el pushdown de `findWhere`/`deleteWhere` a SQL (bloqueados entre sí, ítem grande para una ronda dedicada), `@index`/`@check` declarativos, detección de colisión de tabla, `--db-schema`, `migrate --dry-run`, `@cache`, e idempotency keys.

## [1.43.0] - 2026-08-24

### ✨ Nuevo
- **Soft-delete nativo: `@softDelete`.** "Borrar" una fila casi nunca significa borrarla de verdad -- hasta esta ronda, `db.<c>.delete(id)` siempre era un `DELETE` de SQL real, sin ninguna forma declarativa de pedir "marcalo como borrado en vez de borrarlo". `@softDelete` sobre un campo `Timestamp?` (opcional -- `null` es "no borrado", cualquier otro valor es "borrado en este instante", así que TIENE que ser opcional) cambia el significado de `delete(id)`: deja de ser un `DELETE`, pasa a ser un `UPDATE` que fija ese campo a `now()`, con `AND "<campo>" IS NULL` en el propio `WHERE` para que sea IDEMPOTENTE (una segunda llamada sobre una fila ya borrada no re-toca el timestamp ni publica un evento de `stream` de nuevo, devuelve `false`). Toda lectura que devuelve lista o conteo filtra automáticamente: `all()`, `page()`, `pageAfter()`, `count()`, `sumBy`/`countBy`/`avgBy`/`maxBy`/`minBy` agregan la condición al SQL; `findWhere`/`deleteWhere` la heredan GRATIS, sin ningún código propio, porque reusan `all()` por dentro. A lo sumo un campo `@softDelete` por struct (dos sería ambiguo, rechazado en compilación nombrando los dos). `= null` (el mecanismo de default de campo, v1.39.0) es lo que evita tener que pasar el campo a mano en cada `insert`. Límite deliberado, no una omisión: `find(id)` (y la re-consulta interna que `insert`/`applyPatch` hacen después de escribir) NO filtra -- si filtrara, un `applyPatch` que tocara justo ese campo haría que su propia re-consulta no encontrara la fila que acaba de escribir, un panic en vez de un error limpio. Mismo criterio que Django/Rails adoptan para el mismo problema: listados filtran, lookup directo por id no.

760 tests (10 nuevos): 5 en `checker.rs` (`Timestamp?` tipa limpio, se rechaza sobre `Timestamp` requerido y sobre cualquier otro tipo, dos `@softDelete` en el mismo struct se rechaza, una segunda `@softDelete` en el mismo campo es error de parser) y 5 en `runtime/mod.rs` contra un servidor real vía `invoke_rpc` (`delete` fija el campo en vez de borrar, idempotente, `all()`/`count()` excluyen la fila borrada, `findWhere`/`deleteWhere` heredan el filtro, `page`/`pageAfter`/`sumBy` también filtran). Verificado también a mano contra un servidor HTTP real (`curl`): crear 2 filas, borrar una, `list`/`count` muestran solo 1, un segundo `delete` da `false`, `find` directo por id sigue encontrando la fila borrada. Detalle completo: GRAMMAR.md §3.78.

## [1.42.0] - 2026-08-24

### ✨ Nuevo
- **`createdAt`/`updatedAt` automáticos: `= now()` + `@autoUpdate`.** Fijar cuándo se creó una fila y cuándo se tocó por última vez es casi universal en cualquier tabla real -- hasta esta ronda, cada rpc de creación/edición lo asignaba a mano, con el riesgo real de olvidarse de tocar `updatedAt` en un `applyPatch` nuevo. Sin ninguna anotación mágica por nombre de campo (`createdAt`/`updatedAt` no son nombres reservados en ningún lado): "asignado una sola vez al crear" ya se resolvía SOLO componiendo dos primitivas ya existentes -- `now() -> Timestamp` (builtin sin receptor) más un valor por defecto de campo (`= now()`, v1.39.0) -- sin agregar nada nuevo. La única pieza genuinamente nueva es `@autoUpdate`, una anotación de campo (solo sobre `Timestamp`) que pisa ese campo a `now()` en CADA `applyPatch`/`upsert`-actualización, sin importar qué traiga el patch para ese campo -- interceptado en `runtime::call_method` (no en `db.rs::Db::call`, que no tiene acceso al checker) justo antes de aplicar el patch de verdad, mismo punto que ya usan `findWhere`/`deleteWhere`. `createdAt` (sin `@autoUpdate`) nunca se toca solo después del insert.

750 tests (6 nuevos): 4 en `checker.rs` (`@autoUpdate` sobre `Timestamp` tipa limpio, se rechaza sobre otro tipo, no exige un default a la vez, una segunda `@autoUpdate` en el mismo campo es error de parser) y 2 en `runtime/mod.rs` contra un servidor real vía `invoke_rpc` (un campo `Timestamp = now()` se completa solo al insertar, y `@autoUpdate` pisa el campo en un `applyPatch` real aunque el patch NO lo mencione, mientras `createdAt` -- sin la anotación -- se mantiene idéntico). Verificado también a mano contra un servidor HTTP real (`curl`, con un `sleep` real entre las dos llamadas): `createdAt` idéntico en las dos respuestas, `updatedAt` con timestamp distinto y posterior en la segunda. Detalle completo: GRAMMAR.md §3.77.

## [1.41.0] - 2026-08-24

### ✨ Nuevo
- **`db.<c>.insertMany(items: Omit<T,"id">[]) -> T[]`.** Un backfill que necesita crear N filas hacía N `insert` sueltos -- desde el cliente, N idas y vueltas HTTP secuenciales; desde un solo rpc con un loop, al menos una request pero sin ningún método dedicado para "estas son todas nuevas, insertalas". Cada elemento pasa por el `insert` de siempre (una sentencia SQL autocommit por fila, mismo criterio que el resto del lenguaje) en el orden dado -- lo que ahorra es la ida y vuelta HTTP N veces desde el cliente cuando N filas se crean juntas, no el costo de N inserts contra la base (sigue siendo N sentencias `INSERT`, no una sola sentencia batch). Sin transacción envolvente: si el ítem 3 de 5 falla, los 2 primeros quedan insertados igual, no hay rollback automático.

744 tests (4 nuevos): 3 en `checker.rs` (tipa limpio con una lista del shape insertable, rechaza tipo equivocado, rechaza 0 argumentos) y 1 en `runtime/mod.rs` contra un servidor real vía `invoke_rpc` (las filas se insertan con ids reales y distintos en el orden dado, y quedan persistidas de verdad, confirmado leyéndolas de vuelta con `all()` en una llamada aparte). Verificado también a mano contra un servidor HTTP real (`curl`): tres títulos en un solo `insertMany`, tres filas con id 1/2/3 en la respuesta. Detalle completo: GRAMMAR.md §3.76.

## [1.40.0] - 2026-08-24

### ✨ Nuevo
- **`db.<c>.upsert(matchFn, insertValue, updateFn)`.** El caso "si existe actualizá, si no insertá" era boilerplate repetido a mano (buscar con `findWhere`, borrar, reinsertar con el mismo id) -- y esa implementación manual arrastraba un bug de identidad real: borrar+reinsertar normalmente NO reproduce el mismo id autoincrement en SQLite/Postgres. `matchFn: (T) -> Bool` corre en el intérprete sobre toda la colección y se queda con la primera fila que matchea (mismo límite ya documentado para `findWhere`/`deleteWhere`: no empujado a SQL todavía). Sin match: inserta `insertValue: Omit<T,"id">`. Con match: llama `updateFn` con la fila EXISTENTE completa y aplica el resultado ENTERO sobre el MISMO id (vía el mismo mecanismo de `applyPatch`) -- nunca borra e inserta de nuevo. `updateFn` devuelve `Omit<T,"id">` completo, no `Patch<T>` parcial -- decisión deliberada, no un descuido: `Patch<T>` no tiene sintaxis de literal en el lenguaje (solo llega decodificado del wire como parámetro de rpc), así que una función que "devolviera un Patch<T>" sería, literalmente, imposible de escribir desde un cuerpo de función. Devolver el shape insertable completo sí es constructible con un literal común, y sigue permitiendo que la actualización dependa de los otros campos de la fila existente (`count + 1`, no un valor estático).

740 tests (5 nuevos): 3 en `checker.rs` (tipa limpio con las tres firmas correctas, rechaza un `updateFn` de shape equivocado, rechaza menos de 3 argumentos) y 2 en `runtime/mod.rs` contra un servidor real vía `invoke_rpc` (sin match inserta, con match actualiza la MISMA fila -- mismo id, campo incrementado -- y un match distinto sí inserta una fila nueva). Verificado también a mano contra un servidor HTTP real (`curl`): primer bump inserta id=1, segundo bump con el mismo nombre actualiza a id=1 con el contador incrementado, un nombre distinto inserta id=2. Detalle completo: GRAMMAR.md §3.75.

## [1.39.0] - 2026-08-24

### ✨ Nuevo
- **Valores por defecto en campos de `struct`.** Hasta esta ronda un default solo existía en un parámetro de función/rpc (`rpc list(limit: Int = 20)`) -- un campo de `struct` no tenía forma de decir "si no viene, usá este valor", así que cada `rpc` de creación lo repetía a mano (`status: "pending"` en cada `NewX { ... }` del proyecto). Misma sintaxis y mismo mecanismo que `Param::default`: `nombre: Tipo = expr`, mismo lugar del parser. Un campo CON default se puede omitir de un literal `Struct { ... }` igual que uno `?:`, pero el TIPO del campo no cambia a `Optional` -- sigue siendo el declarado adentro y afuera del literal. El checker exige que el default tipe contra el campo (`x: Int = "hola"` falla en `linkc build`) y que omitir un campo SIN default siga rechazándose. El intérprete completa el valor en `Expr::StructLit` -- el mismo punto que ya se tocó para `@validate` (v1.38.0), así que un `@validate` en el mismo campo también corre sobre el valor final. El default se evalúa DE NUEVO en cada construcción, no una sola vez: `token: Uuid = crypto.uuid()` da un UUID distinto por cada literal, verificado comparando dos construcciones separadas. Mismo entorno EXACTO que ya usa `Param::default` (`Env::new()` vacío) -- un default no ve otros campos del mismo literal ni el entorno que lo rodea. Propagado a los tres generados como "campo opcional", mismo criterio que un parámetro de rpc con default: `contract.d.ts`/`schemas.ts` (Zod) marcan `?`/`.optional()`; `openapi.json` lo saca de `required` y, cuando el default es un literal simple, lo suma como `"default"` (keyword estándar de JSON Schema) -- ausente para un default no literal como `crypto.uuid()`, que no tiene forma JSON fija sin evaluarla. Límites de esta ronda: sin acceso a otros campos del mismo literal, sin soporte en un `type` genérico, sin `DEFAULT` a nivel de columna SQL (se completa en el intérprete antes de llegar a la fila), `validators.ts` sin cambios (verifica forma de un valor YA EXISTENTE, un default es un concepto de construcción, no de validación externa).

735 tests (14 nuevos): 2 en `parser.rs`, 4 en `checker.rs` (tipa limpio, rechaza tipo equivocado, omitir un campo con default tipa pero sin default sigue fallando, funciona en variante de enum), 3 en `runtime/mod.rs` contra un servidor real vía `invoke_rpc` (se completa al construir, un valor explícito lo pisa, evaluación fresca por construcción verificada con `crypto.uuid()`), 2 en `ts_emit.rs`, 2 en `openapi_emit.rs` y 1 en `zod_emit.rs`. Detalle completo: GRAMMAR.md §3.74. Con esto, **PLAN.md §9.2 (Núcleo del Lenguaje) queda completo** salvo `Decimal`/`Money`, que necesita su propio diseño.

## [1.38.0] - 2026-08-24

### ✨ Nuevo
- **`@validate(email)` / `@validate(regex, "...")` sobre un campo `String`/`String?`.** Cierra a la vez PLAN.md §9.2 ("validadores declarativos por campo") y §9.4 ("validación de request body más allá del tipo") -- son la misma cosa. Enforcement real en CUATRO lugares, no solo documentación: (1) el servidor (`linkc serve`) rechaza con 400 un valor que no pase el validador, en DOS puntos -- `json_to_typed_value` (un rpc que recibe el struct completo como parámetro) Y `Expr::StructLit` en el intérprete (un rpc que arma el struct DENTRO del cuerpo a partir de parámetros sueltos -- el caso más común de los dos, y el que un `curl` real contra el servidor reveló que faltaba: sin este segundo punto, un email inválido pasaba con 200 pese a `@validate` estar declarado, porque el struct nunca se decodificaba del wire como tal); (2) `openapi.json` usa las keywords ESTÁNDAR de JSON Schema `"format": "email"` / `"pattern": "..."`; (3) `schemas.ts` (Zod) encadena `.email()` / `.regex(new RegExp("..."))` -- `new RegExp` de un string en vez de un literal `/.../`, para no tener que escapar `/` dentro del patrón del usuario, y ANTES de `.nullable()` en un campo opcional (`ZodNullable` no tiene esos métodos, el orden de encadenado no es cosmético); (4) `contract.d.ts` lleva un comentario JSDoc informativo. El patrón de `regex` se compila en `linkc build` (crate `regex`), nunca en el primer request real -- un patrón roto es un error de compilación con el mensaje real de la crate. Único límite genuino: `@validate` está atado a la declaración exacta donde se escribe -- el patrón "New\*" (`Omit<T,"id">`) que el resto del proyecto usa para `insert` es un tipo APARTE, así que hay que repetir la anotación ahí también, o construir el shape "completo" no queda protegido. Única excepción de esta sesión a "cero dependencias nuevas": a diferencia de UUID/SHA-256/ISO-8601 (formas FIJAS, hand-rolleables sin drama), un patrón de usuario es texto arbitrario -- soportar solo un subconjunto de regex a mano sería un espejismo de corrección. La crate `regex` es pura Rust (compila también a wasm32-unknown-unknown, no está detrás del feature `runtime`). Alcance de esta ronda: `validators.ts` (las funciones `isX()` hand-escritas) todavía no lo enforce -- trabajan sobre el tipo estructural, sin la anotación.

721 tests (19 nuevos): 8 en `checker.rs`/`parser.rs` (tipa limpio con `email`/`regex` sobre `String`/`String?`/campo de variante de enum; rechaza sobre `Int`, un patrón regex inválido, dos `@validate` en el mismo campo, una forma desconocida), 5 en `runtime/mod.rs` contra un servidor real vía `invoke_rpc` (email malformado rechazado en 6 formas, regex rechaza/acepta correctamente, opcional ausente no valida pero presente sí, el struct construido adentro del cuerpo del rpc -- el caso que reveló el gap de `Expr::StructLit` --, y el límite documentado de que un shape "New\*" sin la anotación repetida no valida nada), 2 en `ts_emit.rs`, 1 en `openapi_emit.rs` y 3 en `zod_emit.rs` (incluido el orden `.email()` antes de `.nullable()`). Detalle completo: GRAMMAR.md §3.73. Quedan 2 ítems abiertos en PLAN.md §9.2: `Decimal`/`Money` (necesita su propio diseño) y valores por defecto en campos de struct.

## [1.37.0] - 2026-08-24

### ✨ Nuevo
- **Docstrings `///` propagados a `openapi.json` y `contract.d.ts`.** Hasta esta ronda, la única documentación de un rpc en el spec generado era su propio nombre en `"summary"` -- cualquier comentario arriba se perdía al compilar, el lexer trataba `//`, `///` y `/* */` exactamente igual, como trivia a descartar. Nueva infraestructura de lexer: `///` (exactamente 3 slashes -- ni `//` ni `////`, que sigue siendo el separador visual común sin significado especial) se sigue saltando como trivia igual que siempre, pero además su texto queda capturado en `Token::leading_doc` y se pega al PRÓXIMO token real. Varias líneas `///` consecutivas se unen con `\n` en un solo docstring. El parser solo lee ese campo en el único lugar donde tiene sentido -- justo antes de un `rpc`/`stream`, incluso con una `@annotation` en el medio (`/// texto` → `@requires(...)` → `rpc` sigue atribuyéndose al rpc) -- así que es puramente aditivo: CERO riesgo de romper un programa existente, un `///` en cualquier otra posición simplemente no lo lee nadie, exactamente como antes. Se propaga como `description` del Operation Object en `openapi.json`, y como un bloque JSDoc multilínea `/** ... */` en `contract.d.ts`. Si el mismo rpc también lleva `@deprecated("...")` (v1.36.0), las dos cosas conviven en el mismo campo/bloque en vez de pisarse -- en OpenAPI el motivo se agrega al final de la descripción, en el `.d.ts` `@deprecated` aparece como su propia línea de tag dentro del mismo bloque.

702 tests (12 nuevos): 4 en `lexer.rs` (se saltea como trivia igual que `//` pero queda en `leading_doc`, varias líneas se unen con `\n`, `////` no cuenta, una línea vacía da `Some("")` no `None`), 3 en `parser.rs` (se atribuye al rpc de abajo, sigue atribuyéndose con una `@annotation` en el medio, sin docstring da `None`), 2 en `openapi_emit.rs` (se propaga como `description`, se combina con `@deprecated` sin pisarse) y 3 en `ts_emit.rs` (bloque JSDoc multilínea, combinado con `@deprecated` en un solo bloque, ausente cuando no hay ninguna de las dos cosas). Más el ejemplo nuevo de GRAMMAR.md §3.72, compilado y ejecutado por `docs_examples.rs`. Detalle completo: GRAMMAR.md §3.72.

## [1.36.0] - 2026-08-24

### ✨ Nuevo
- **`@deprecated("usa X en su lugar")` sobre un campo de struct o un rpc/stream.** Hasta esta ronda, marcar algo como "existe pero no lo uses" no tenía forma declarativa -- un comentario en el `.link` nunca llegaba al `.d.ts` generado. Sobre un `rpc`/`stream` reusa el mecanismo de anotaciones que ya existía (`@authenticated`/`@route`/`@rate_limit`/etc.), combinable libremente con cualquiera de esas -- es una dimensión ortogonal. Sobre un campo de struct es la ÚNICA anotación que un campo admite hoy: `Field` no tiene el `Vec<Annotation>` genérico de `RpcDecl`, así que el parser solo reconoce `@deprecated("...")` en esa posición y rechaza cualquier otro nombre ahí mismo con un error de sintaxis. Puramente informativo -- cero efecto en runtime ni en el checker: un rpc/campo deprecado sigue funcionando exactamente igual, y NO participa de la subtipificación estructural (dos structs idénticos salvo el `@deprecated` de un campo siguen siendo el mismo tipo). Propagado como comentario JSDoc `/** @deprecated <motivo> */` justo antes del campo/método en `contract.d.ts` (cualquier editor que entienda JSDoc lo tacha automáticamente), y como `deprecated: true` + `description` -- keywords nativas de Operation Object y JSON Schema 2020-12, sin extensión `x-*` propia -- en `openapi.json`.

690 tests (11 nuevos): 6 en `checker.rs`/parser (tipa limpio combinado con `@requires` en un rpc, rechaza dos `@deprecated` en el mismo rpc, rechaza motivo vacío en rpc y en campo, tipa limpio en un campo sin afectar subtipificación estructural, rechaza cualquier otra anotación sobre un campo) y 5 en `codegen` (3 en `ts_emit.rs`: el JSDoc aparece exactamente antes de lo marcado y en ningún otro lado en campo y en rpc, un motivo con `*/` literal no corta el comentario antes de tiempo; 2 en `openapi_emit.rs`: `deprecated`+`description` en la operación y en la propiedad del schema, ausentes en las que no llevan la anotación). Más el ejemplo nuevo de GRAMMAR.md §3.71, compilado y ejecutado por `docs_examples.rs` (no suma al conteo -- ya corre como parte de ese mismo test). Detalle completo: GRAMMAR.md §3.71.

## [1.35.0] - 2026-08-24

### ✨ Nuevo
- **Tipo nativo `Uuid`.** Hasta esta ronda un identificador con forma de UUID era `String` -- nada impedía basura, y validar el formato quedaba a mano en cada `rpc`. `Uuid` exige la forma canónica `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` (sin restringir el nibble de versión/variante -- cualquier RFC 4122 real vale) en los TRES bordes donde un valor puede cruzar: el runtime al decodificar JSON (`json_to_typed_value`, un escaneo manual de bytes, sin sumar la crate `regex`), `validators.ts`, y `schemas.ts`/Zod -- las tres regex son literalmente la misma, para que nunca puedan divergir. `openapi.json` usa el idiom estándar `"format": "uuid"`. Tipo aparte de `String`, sin mezcla implícita -- mismo criterio que `Int64` vs `Int`: `crypto.uuid()` ahora devuelve `Uuid`, no `String`; `"prefijo-" + unUuid` es un error de compilación; `.toString()` es la conversión explícita (después de eso, cualquier método de `String` funciona normal). Runtime: `Value::Uuid` como variante propia (no reusa `Value::Str`), mismo criterio que ya justificaba una variante propia para `Timestamp` -- el borde serializa igual, pero el runtime necesita distinguirlos para saber si `.toString()` tiene sentido. Storage: `TEXT` en los dos backends, nunca envuelto en JSON -- verificado con `sqlite3 ... ".schema"` mostrando la columna nativa, no un fallback JSON.

679 tests (8 nuevos): 5 en `checker.rs` (resuelve como tipo, `crypto.uuid()` tipa `Uuid`, sin mezcla implícita con `String` ni en asignación ni en `+`, `.toString()` funciona) y 3 en `runtime/mod.rs` contra un servidor real vía `invoke_rpc` (7 variantes de UUID malformado rechazadas con 400, un UUID válido -- incluido en mayúsculas -- viaja exacto por el wire, `crypto.uuid()` genera uno real que sobrevive un `insert`+`find` contra SQLite real). Verificado también a mano contra un servidor HTTP real (`curl`, malformado→400, válido→200) y contra el schema SQLite generado. Detalle completo: GRAMMAR.md §3.70.

## [1.34.0] - 2026-08-24

### ✨ Nuevo
- **Narrowing real de `T?`: `match`, `??`, `.isSome()`/`.isNone()`.** El gap más repetido y con más fricción de dos reportes de adopción real independientes -- hasta esta ronda no había NINGUNA forma de leer el valor interior de un `T?` dentro de un `rpc` (bloqueó lógica de negocio real, un caso confirmado: validar caducidad de un cupón tuvo que moverse fuera del servidor). `match x { v: T => ..., null => ... }` narrowea de verdad -- reusa el mismo mecanismo de patrones que ya narrowaba uniones (`Pattern::Type`, `check_exhaustive_union`), con un `check_exhaustive_optional` hermano y `null` como patrón literal nuevo. Exhaustivo de verdad: falta el caso `null` o el caso de valor, error de compilación. `a ?? b` (con encadenado real: `a ?? b ?? default`, cortocircuita como `&&`/`||`) cubre el caso "dame un default" sin la ceremonia de un match completo; `.isSome()`/`.isNone()` cubren "solo necesito saber si hay valor" -- con un caso adversarial real resuelto (un struct PLANO con un campo de verdad llamado `isSome` que guarda una closure sigue llamándose como ESE campo, nunca shadoweado por el atajo del opcional). `if x != null { x.campo }` sigue sin angostar, a propósito -- eso no cambió. El mensaje de error de acceso directo a un campo sobre `T?` ahora señala las tres alternativas reales en vez de solo decir "no se puede". Completion del LSP para `T?` ofrece `isSome()`/`isNone()`, nunca los campos de `T` (que siguen necesitando `match`).

671 tests (22 nuevos): 14 en `checker.rs` (exhaustividad completa en los dos sentidos, wildcard, patrón de tipo incompatible rechazado, `null` contra escrutinio no opcional rechazado, `??` sobre no-opcional rechazado, lado derecho de `??` debe ser `T` o `T?`, encadenado, `isSome`/`isNone` rechazados sobre no-opcional y sin argumentos), 7 en `runtime/mod.rs` contra un servidor real vía `invoke_rpc` (narrowing de struct y primitivo, `??` con cortocircuito real verificado, encadenado de 3 opcionales, caso adversarial del campo shadowing), 1 en `lsp.rs` (completion). Detalle completo: GRAMMAR.md §3.69.

## [1.33.0] - 2026-08-24

### 📝 Documentación
- **4 guías nuevas en `docs/`**, cierran PLAN.md §9.1 por completo:
  - [`docs/sqlite-vs-postgres.md`](docs/sqlite-vs-postgres.md): cómo decidir qué backend usar para un servicio nuevo.
  - [`docs/multi-service-deployment.md`](docs/multi-service-deployment.md): desplegar 10+ `.link` en un mismo host -- un puerto por servicio, proxy adelante, cuidado con colisiones de nombre de tabla si comparten base.
  - [`docs/incremental-adoption.md`](docs/incremental-adoption.md): migrar un backend existente (Express/Fastify/NestJS/lo que sea) servicio por servicio, con `linkc introspect` + `--adopt-existing` + el puente JWT como las tres piezas que lo hacen seguro.
  - [`docs/consuming-services.md`](docs/consuming-services.md): la versión de AGENTS.md para quien integra un servicio `.link` ya generado desde otra app, no para quien lo desarrolla -- forma exacta de los errores, del `/health`, y qué NO asumir (reintentos, timeouts, batching), todo verificado contra un servidor real, no de memoria.

Sin cambios de código -- 649 tests sin cambios. Detalle completo: PLAN.md §9.1.

## [1.32.0] - 2026-08-24

### 📝 Documentación
- **Gotcha real de `link.lock`/resolución de imports**: compilar un `.link` fuera de la carpeta que en verdad es la raíz de su proyecto hace que cualquier import bare-name o relativo falle con `no se pudo resolver '<ruta>'` -- confirmado con el mensaje real del compilador, no aproximado.
- **Comportamiento exacto ante `SIGTERM`/matar el proceso**: `linkc serve` no instala ningún manejador de señales (confirmado, no hay crate ni código de señales) -- terminación inmediata sin drenado gracioso, pero ninguna escritura ya confirmada al cliente puede perderse: cada `insert`/`applyPatch`/`delete` es una única sentencia SQL autocommit, nunca una transacción multi-sentencia abierta a mitad de camino.

Sin cambios de código en esta ronda -- ambos ítems ya se comportaban bien, solo faltaba confirmarlo y documentarlo. Detalle completo: GRAMMAR.md §2.1 y §3.17.

## [1.31.0] - 2026-08-24

### 🐛 Arreglado
- **`schema.postgres.sql` ya no pide `CREATE EXTENSION "pgcrypto"`.** Un reporte de adopción real preguntaba si esa línea (presente desde v1.0) necesitaba superusuario en un proveedor gestionado (Neon/RDS/Supabase) -- auditando para qué se usaba la respuesta fue "para nada": ninguna función de pgcrypto aparece en ningún SQL que el proyecto genera o ejecuta, `crypto.*` es Argon2id/HMAC/CSPRNG en Rust. Era peso muerto heredado que podía bloquear sin motivo a un rol sin permiso de crear extensiones. Se sacó por completo en vez de documentar un requisito que no existía.

649 tests (2 nuevos): uno confirma que el DDL generado nunca menciona pgcrypto/`CREATE EXTENSION`; el otro aplica el `schema.postgres.sql` completo con un rol Postgres real `NOSUPERUSER NOCREATEDB NOCREATEROLE` y confirma que aplica limpio. Detalle completo: GRAMMAR.md §3.36.

## [1.30.0] - 2026-08-24

### 📝 Documentación
- **Comportamiento de dos `.link` distintos declarando la misma colección contra la misma base** (GRAMMAR.md §3.36), pedido explícito en un reporte de adopción real ("no nos atrevimos a probarlo"). Verificado contra un PostgreSQL real, no solo razonado: columnas sin nombres en común conviven para lectura sin error, aunque un `INSERT` del segundo `.link` puede violar una constraint `NOT NULL` que dejó el primero sin que el segundo la conozca; un mismo nombre de columna con el mismo tipo convive sin problema; un mismo nombre con tipos DISTINTOS no se detecta al conectar (`ADD COLUMN IF NOT EXISTS` es no-op sobre una columna que ya existe) y falla recién en la primera lectura/escritura real, siempre con un error limpio del driver, nunca un panic.

647 tests (2 nuevos). Detalle completo: GRAMMAR.md §3.36.

## [1.29.0] - 2026-08-24

### 🐛 Arreglado
- **PostgreSQL: NULL en una columna requerida ya no se serializa como `null` en silencio.** Auditando la matriz de comportamiento de auto-migrate (pedida en dos reportes de adopción real) apareció un bug genuino: `connect_postgres` siempre agrega una columna nueva como `NULLABLE` sin importar si el campo es requerido en el `.link` -- una fila insertada antes de ese cambio queda con `NULL` ahí, y hasta esta ronda `row_to_fields` decodificaba eso en silencio como `Value::Null`, mandando `null` a un cliente TypeScript cuyo contrato declara ese campo `string` (no `string | null`). Ahora es un error de runtime limpio -- 5xx JSON normal, nunca un panic que tumbe el proceso entero -- que nombra la colección, el id de la fila y el campo. `row_to_fields` pasó de `Vec<(String, Value)>` a `Result<Vec<(String, Value)>, RuntimeError>`.

### 📝 Documentación
- **Matriz de comportamiento completa de auto-migrate** (GRAMMAR.md §3.17): columna nueva/eliminada/renombrada, cambio de tipo, y campo requerido↔opcional, para SQLite y PostgreSQL por separado -- el README solo documentaba antes el caso aditivo. Verificada con 5 tests nuevos contra SQLite real (los 5 casos que no son "agregar columna opcional nueva" fallan al conectar) y 1 test nuevo contra un PostgreSQL real para el bug de arriba.

645 tests (6 nuevos). Detalle completo: GRAMMAR.md §3.17 y §3.68.

## [1.28.0] - 2026-08-24

### ✨ Nuevo
- **`linkc serve --adopt-existing` (o `LINK_ADOPT_EXISTING`): adoptar tablas existentes sin auto-migrar.** Hasta esta ronda, `linkc serve` siempre intentaba `CREATE TABLE`/`ALTER TABLE ADD COLUMN` (no destructivo, pero DDL al fin) al abrir cada colección declarada -- dos bloqueos reales para adoptar un sistema existente: un rol de base sin permiso de DDL (común en producción), y una tabla SQLite con columnas físicas que el `.link` no modela (`check_schema_matches` exigía coincidencia EXACTA y hacía panic ante cualquier columna de más). Con la flag, `linkc serve` nunca ejecuta DDL -- ni siquiera el no destructivo de siempre -- y en su lugar valida con SELECTs de solo lectura que cada columna DECLARADA exista: en SQLite además compara el tipo SQL esperado (`PRAGMA table_info`), en PostgreSQL solo la existencia por nombre (`information_schema.columns`, mismo criterio que el chequeo de `"id"` que ya existía fuera de este modo). Una columna física no declarada se ignora sin queja -- justo el caso de adoptar una tabla legacy. Una tabla o columna declarada faltante falla al conectar, con un mensaje que dice qué falta, nunca con un `CREATE`/`ALTER` silencioso. Límite honesto: todo o nada por proceso (no colección por colección), y no valida `NOT NULL` en SQLite ni tipo columna por columna en PostgreSQL más allá de `"id"` -- eso se descubre en la primera lectura que lo toque, con el error normal de decode. Verificado contra SQLite real (dos corridas consecutivas de `linkc serve` sobre el mismo archivo: la primera crea una tabla con una columna que la segunda no declara, la segunda arranca en modo adopción y la ignora) y contra un PostgreSQL real (tabla creada a mano con una columna sin modelar; columna declarada faltante falla limpio, sin panic). 639 tests (9 nuevos). Detalle completo: GRAMMAR.md §3.67.

## [1.27.0] - 2026-08-24

### ✨ Nuevo
- **`linkc introspect <db-url>`: generar un `.link` desde una base PostgreSQL existente.** Reporte real de adopción (MyFinance): sin esto, adoptar Link sobre datos reales significaba escribir cada `type`/`db{...}` a mano, columna por columna. Lee `information_schema` del schema `public` y emite un `.link` de partida a stdout -- un `type` por tabla más el `db {...}` que las declara. `bigint`/`integer`/`smallint` -> `Int`, `boolean` -> `Bool`, `double precision`/`numeric` -> `Float`, `text`/`varchar` -> `String`, nullable -> `T?`, todos sin advertencia. `jsonb`/`json` (forma desconocida), `uuid`, y cualquier `timestamp`/`timestamptz`/`date` NATIVO (el `Timestamp` de c-script necesita milisegundos en `BIGINT`, no el tipo nativo de Postgres) salen igual como `String` -- nunca se omite una columna -- pero con una advertencia explícita en stderr. Los nombres de campo son los nombres REALES de columna SQL (`snake_case` incluido) a propósito: c-script no tiene alias campo↔columna, "prolijizar" a camelCase rompería la conexión con la tabla real. Alcance acotado: solo PostgreSQL, solo PK simple llamada `"id"`, sin FKs/índices/constraints, sin generar ningún `service`. Verificado contra un PostgreSQL real: una tabla creada A MANO (simulando un sistema ya adoptado) da un `.link` que, con un `service` mínimo agregado a mano, conecta de verdad y lee la fila sembrada antes de que `linkc` supiera que la tabla existía; más un test que confirma las advertencias de `jsonb`/`timestamptz`. 630 tests (6 nuevos). Detalle completo: GRAMMAR.md §3.66.

## [1.26.0] - 2026-08-24

### ✨ Nuevo
- **Agregación (`sumBy`/etc.): soporte de `Int64` como campo de agrupación y de valor.** Hasta esta ronda, `Int64` estaba rechazado en las dos posiciones -- un programa con IDs/montos declarados `Int64` no podía agregarlos en absoluto.

### 🐛 Arreglado
- **`scalar_cell_to_value` nunca distinguía `Int64` de `Int`.** `Int`/`Int64` comparten `ColumnKind::Int` (mismo `BIGINT` de storage) -- la función solo miraba la `Cell`, nunca el `Type` declarado, así que un resultado agregado `Int64` habría llegado etiquetado `Value::Int` y por lo tanto serializado como NÚMERO en el JSON, rompiendo la promesa de §3.30 (`Int64` siempre viaja como string, para no perder precisión arriba de 2^53). No era un bug ejercitable antes de esta ronda (el checker ya rechazaba `Int64` ahí), pero sí lo hubiera sido en cuanto se abriera la puerta -- corregido junto con la feature, no después.

Truncado de fechas para agregación sigue pendiente como ronda propia (PLAN.md §8.2.1) -- los dos backends divergen de verdad para truncar un `Timestamp` (milisegundos en `BIGINT`) a un día/mes/año real.

Verificado con un test de runtime contra SQLite que confirma el resultado es `Int64` de verdad (no solo que el número coincide), el mismo caso contra un PostgreSQL real confirmando que viaja como string en el JSON, y un test de compilación. 624 tests (3 nuevos). Detalle completo: GRAMMAR.md §3.65.

## [1.25.0] - 2026-08-24

### ✨ Nuevo
- **Auth externo: confiar en un JWT ya emitido, HS256.** Hasta esta ronda, Link solo emitía y validaba sus PROPIAS sesiones opacas -- bloqueaba CUALQUIER adopción dentro de una app con login preexistente sin correr dos sistemas de sesión en paralelo. `linkc serve --jwt-secret <secreto>` (o `LINK_JWT_SECRET`) verifica un JWT HS256 emitido por un backend existente -- junto con, nunca en vez de, las sesiones propias (`SessionStore::role_for`/`user_id_for` prueban la sesión propia primero, y solo intentan JWT si el token no está ahí y hay secreto configurado). `@requires`/`@authenticated`/`auth.currentRole()`/`auth.currentUserId()` funcionan igual sin importar cuál de los dos autenticó -- un sentinel (`enum_name` vacío) le dice a `check_auth_gate` que matchee por nombre de variante nada más, sin la comparación de identidad de enum que sí aplica a una sesión propia. `--jwt-role-claim`/`--jwt-user-id-claim` (default `role`/`sub`) eligen los claims; `sub` acepta número JSON o string de dígitos (convención real de OIDC). Sin dependencias nuevas -- `hmac`/`sha2`/`base64` ya estaban por `crypto.hmacSha256`/`base64.encode`. **Solo HS256, allowlist no blocklist**: `"alg":"none"` (la vulnerabilidad de JWT más común y documentada) y cualquier otro algoritmo se rechazan explícitamente antes de calcular una firma esperada; la comparación de firma reusa `constant_time_eq` (ya usado por `verifyPassword`). `exp` se respeta si está presente; sin `nbf`/`iss`/`aud` ni RS256/JWKS -- eso es un proveedor de identidad completo, ronda propia si hace falta. Verificado con 11 tests unitarios en `session.rs` (firma inválida, `alg:none`, `alg:RS256`, JWT vencido, JWT sin `exp`, entradas basura, precedencia de sesión propia, claims configurables) más 6 tests end-to-end contra un servidor real (`server_http.rs`). 621 tests (17 nuevos). Detalle completo: GRAMMAR.md §3.64.

## [1.24.0] - 2026-08-24

### ✨ Nuevo
- **`smtp.sendToMany()`/`smtp.sendHtml()`: varios destinatarios y cuerpo HTML.** `smtp.send` mandaba texto plano a UN destinatario -- mandar a varios significaba una llamada por destinatario (N conversaciones SMTP separadas), y no había forma de mandar HTML. Dos métodos nuevos, `send` sin cambios (mismo criterio que `getWithHeaders`/`getWithStatus`): `sendToMany(to: String[], subject, body)` manda UN mensaje con un `RCPT TO:` por destinatario; `sendHtml(to: String[], subject, html)` manda `Content-Type: text/html`, a uno o varios. Los dos siguen sacando conexión/remitente del entorno del proceso, nunca del rpc. Sigue sin adjuntos, cc/bcc, ni envío asíncrono -- los tres métodos son sincrónicos. Verificado contra un servidor SMTP real armado a mano en el test (`cli_smtp.rs`, habla EHLO/MAIL FROM/RCPT TO/DATA de verdad): dos destinatarios producen dos `RCPT TO` en la misma conversación, lista vacía falla limpio, el HTML llega con su Content-Type y sin escapar. 604 tests (3 nuevos). Detalle completo: GRAMMAR.md §3.63.

## [1.23.0] - 2026-08-23

### ✨ Nuevo
- **`@route` lee parámetros extra de la query string.** Hasta esta ronda, un rpc con `@route` tenía que tomar EXACTAMENTE los parámetros del path, ni de más -- cualquier filtro (`?estado=activo`) obligaba a duplicar el rpc completo. Ahora cualquier parámetro del rpc que no esté nombrado en el path se lee de la query string por nombre: `String`/`Int` obligatorio (400 si falta), `String?`/`Int?` opcional (`null` si no vino). `body` sigue sin leerse, a propósito -- la URL de `@route` es para que un crawler la abra con GET simple.

### 🐛 Arreglado
- **La query string se colaba entera dentro del último segmento de path capturado.** `/blog/hola-mundo?utm_source=twitter` -- una URL perfectamente normal, cualquier link compartido trae parámetros de tracking -- corrompía `:slug` con `"hola-mundo?utm_source=twitter"` completo, porque el path se partía en segmentos ANTES de separar la query string. Se arregló para toda ruta con `@route` (tenga o no parámetros de query declarados) y de paso también para el `/Service/rpc` normal, que tenía la misma vulnerabilidad latente sin ejercitar en la práctica.

Verificado contra un servidor real (`cli_route.rs`): query param obligatorio/opcional, 400 con el nombre del que falta, `Int` inválido, el bug de corrupción del slug fijado explícitamente, un query param desconocido sin efecto, decodificación `+`/`%XX`. 601 tests (4 nuevos). Detalle completo: GRAMMAR.md §3.62.

## [1.22.0] - 2026-08-23

### ✨ Nuevo
- **`db.<c>.pageAfter(cursor, limit)`: cursor de continuación.** Item de la tabla original del README sobre `page`: `page(limit, offset)` obliga a calcular el próximo `offset` a mano, y `OFFSET` cuenta filas desde el principio de la tabla EN CADA LLAMADA -- una fila insertada entre dos páginas puede hacer que la siguiente repita o se salte una fila. El cursor ES el `id` del último elemento visto (`null` para la primera página) -- no un token opaco codificado aparte, a propósito: el `id` ya es un campo público del struct, envolverlo no agrega ninguna garantía real. La propiedad real que esto resuelve es estabilidad bajo escritura concurrente, no "opacidad". `page` queda sin cambios, sigue siendo la opción correcta para saltar a una página arbitraria. Verificado con un test que inserta una fila nueva ENTRE dos llamadas a `pageAfter` y confirma que la página siguiente no se mueve -- contra SQLite (`db.rs`) y contra un PostgreSQL real (`pg_integration.rs`). 597 tests (2 nuevos). Detalle completo: GRAMMAR.md §3.61.

## [1.21.0] - 2026-08-23

### ✨ Nuevo
- **`http.getWithStatus`/`http.postWithStatus`: código de estado y headers de la respuesta.** Último item de la tabla "Does not work yet" original del README sobre HTTP saliente (PLAN.md §8.3.1): `http.get`/`http.post`(`WithHeaders`) solo devolvían el body -- un 4xx/5xx se volvía error de runtime genérico, sin forma de reintentar selectivamente (ej. solo en 429). Dos métodos NUEVOS, mismos argumentos que sus pares `WithHeaders`, sin tocar los cuatro existentes. Devuelven un struct estructural SIN nombre reservado -- mismo criterio que ya usa el tipo de `headers` (v1.11.0): `{status: Int, headers: {name: String, value: String}[], body: String}`, cualquier `type` declarado con esos campos sirve. Un 4xx/5xx deja de ser error en estos dos métodos -- `ureq::Error::Status` ya trae la `Response` completa, no solo el código; solo un error de RED de verdad sigue siendo `Err`. Verificado contra un servidor HTTP real armado a mano en el test (`cli_http.rs`): 2xx con headers de respuesta, 429 con `Retry-After` como dato (sin que el rpc falle), 201 de un POST. 595 tests (3 nuevos). Detalle completo: GRAMMAR.md §3.60.

## [1.20.0] - 2026-08-23

### ✨ Nuevo
- **Costo de Argon2id configurable (`--argon2-memory-kib`/`--argon2-iterations`) y `crypto.isLegacyHash()`.** Dos gaps de PLAN.md §8.4. El costo de `crypto.hashPassword` era fijo al default de la crate; ahora es configurable vía flag de servidor (mismo criterio que `--session-ttl`/`--cors-origin`, no un parámetro nuevo del lenguaje) sin tocar `verifyPassword` -- el formato PHC embebe sus propios parámetros en el hash. Mecanismo: `Db` gana un `RefCell<argon2::Params>` (mismo patrón que `current_request`/`response_status_override`, aditivo puro, sin enhebrar un parámetro nuevo por ~11 firmas). `crypto.isLegacyHash(hash) -> Bool` distingue el formato legado (`sha256$...`) del Argon2id real, para re-hashear proactivamente en el login. Verificado contra un servidor real (`cli_argon2.rs`): sin flags, default de la crate embebido en el hash; con flags, esos valores exactos; un valor no numérico falla ANTES de arrancar.

### 🐛 Arreglado
- **PostgreSQL: una tabla preexistente con "id" `SERIAL`/`IDENTITY` (32/16 bits) fallaba en el primer insert, pese a que `validate_existing_id_column` ya la aceptaba al conectar.** `insert_returning_id`/`postgres_cell` (`runtime/store.rs`) leían la columna con `try_get::<_, i64>`, que exige el OID exacto `int8` -- un desacuerdo real entre la capa de validación (que acepta `bigint`/`integer`/`smallint` desde que existe) y la capa de lectura (que solo toleraba la primera). El comentario junto al `try_get` afirmaba "esto nunca dispara" apoyándose en una validación que, leída con cuidado, ya aceptaba justo el caso que lo disparaba. `postgres_int_cell` ahora prueba `int8`→`int4`→`int2` en orden, y se generalizó a CUALQUIER columna `Int`, no solo `"id"`. **Sin verificar contra Postgres real en esta sesión** (sin Docker/Postgres disponible en el entorno) -- el test nuevo (`pg_integration.rs`) corre de verdad en CI.

592 tests (4 nuevos: 3 en `cli_argon2.rs`, 1 en `pg_integration.rs` que corre solo en CI). Detalle completo: GRAMMAR.md §3.58–§3.59.

## [1.19.0] - 2026-08-23

### ✨ Nuevo
- **`.toString()` sobre `Int`/`Int64`/`Float`/`Bool`, `response.setStatus` rechazado en compilación dentro de `stream`, y catch-all `:nombre*` en `@route`.** Tres gaps del roadmap PLAN.md §8.6/§8.7, cerrados en la misma ronda: (1) no existía NINGUNA conversión numérica/bool a `String` en el lenguaje -- ni para interpolar un contador en un mensaje de error; ahora cuatro métodos explícitos (`Bool.toString()` es el primer método que existe sobre `Bool`). (2) `response.setStatus` dentro de un `stream` documentaba ser un no-op desde su introducción (§3.46) pero tipaba sin quejarse -- ahora `Checker` gana un `Cell<bool>` (`in_stream_body`, mismo patrón de interior mutability que `hover_result`, §3.24) que lo rechaza en compilación, con el span exacto de la llamada. (3) `@route` solo capturaba un segmento por parámetro -- `:nombre*` como último segmento captura cero o más segmentos restantes unidos con `/`, siempre `String` (nunca `Int`, puede contener `/` y estar vacío). La detección de conflictos entre rutas (`route.rs::overlap_possible`) se generalizó para comparar solo el prefijo fijo compartido cuando hay catch-all de por medio, en vez de exigir igual longitud total -- conservador a propósito, prefiere un falso positivo a dejar pasar una ambigüedad real. `RoutePattern::matches` pasó de `Vec<&str>` a `Vec<String>` (un catch-all captura texto que no era un único slice del input). Verificado: 14 tests nuevos (conversiones, setStatus en stream vs rpc normal, parseo/matching/conflictos de catch-all, y 2 end-to-end contra un servidor real). 588 tests. Detalle completo: GRAMMAR.md §3.55–§3.57.

## [1.18.0] - 2026-08-23

### ✨ Nuevo
- **`crypto.randomInt(min, max)` y `crypto.timingSafeEqual(a, b)`: aleatoriedad numérica y comparación segura para código de usuario.** Reporte real de adopción de una app financiera existente ("MyFinance") -- `crypto.randomToken`/`uuid` (v1.0, endurecidos en la auditoría del 20/08) ya usaban el CSPRNG del sistema, pero ninguno sirve para un OTP numérico: `randomToken` da hex (`0-9a-f`), no dígitos en un rango exacto. `randomInt(min, max)` da un `Int` uniforme en `[min, max]` (ambos incluidos) con rechazo de muestreo contra el sesgo de módulo -- un `u64` que caería en el resto no divisible se descarta en vez de aplicarle `%` directo. `timingSafeEqual` expone `constant_time_eq` (`subtle::ConstantTimeEq`), ya usado internamente desde la auditoría de `crypto` para el camino de hashes legados, pero nunca alcanzable desde código de usuario -- comparar un secreto de webhook o una API key con `==` de `String` reabre el mismo canal lateral que ya se había cerrado para contraseñas. Verificado con tests de runtime: rango respetado en los extremos, rango degenerado (`min == max`), variabilidad entre llamadas consecutivas con un rango de OTP de 6 dígitos, comparación igual a `==` en el caso feliz y `false` (sin crash) ante largos distintos. 574 tests (2 nuevos). Detalle completo: GRAMMAR.md §3.54.

## [1.17.0] - 2026-08-22

### ✨ Nuevo
- **`auth.createSessionWithId(role, userId)` y `auth.currentUserId()`: asociar e inspeccionar el id del usuario en la sesión.** Cierra el gap de identidad de usuario que quedaba pendiente tras `auth.currentRole()` (v1.15.0) -- "la sesión solo guardaba el rol, impidiendo saber qué usuario específico autenticó la llamada sin pasar el id a mano como parámetro". `auth.createSessionWithId(role, userId)` permite emitir un token asociando tanto el rol como un `userId: Int` de forma segura en memoria. `auth.currentUserId()` devuelve `Int?` con el identificador del usuario autenticado en la petición actual. Mismo principio de indistinguibilidad: devuelve `null` si no hay sesión activa, si el token expiró bajo `--session-ttl` o si la sesión se creó sin id (`auth.createSession(role)`). Verificado contra un servidor HTTP real, tests de tipos en `checker.rs` y tests unitarios en `session.rs`. 573 tests (5 nuevos). Detalle completo: GRAMMAR.md §3.53.

## [1.16.0] - 2026-08-21

### ✨ Nuevo
- **`sumBy`/`countBy`/`avgBy`/`maxBy`/`minBy`: agregación con `GROUP BY` real, empujada a SQL.** El más grande de la serie de gaps que salió del mismo chequeo externo -- "KPIs de MRR por plan o cohorte mensual hay que calcularlos trayendo todas las filas a memoria y agregando a mano -- se degrada mal si la tabla crece". `findWhere`/`deleteWhere` (v1.0) ya traían todo a memoria para un predicado arbitrario; esto hace lo mismo para `GROUP BY`, pero de verdad empujado a la base -- el closure selector NUNCA se ejecuta, solo NOMBRA una columna: shape reconocido `|item: T| { item.campo }` (mismo patrón que `recognize_live_subscribe` de v1.0 para `stream`), cualquier otra forma (expresión derivada, método, campo anidado) se rechaza en compilación porque no hay forma real de traducirla a SQL. Cinco métodos con nombre explícito, no un query builder encadenado -- mismo criterio de "nombre por forma" que ya usa v1.11 para no inventar la primera aridad variable del lenguaje. Agrupar solo por `String`/`Int`/`Bool`/enum (Float excluido por el mismo motivo que `match` no tiene patrón `Float`; fechas truncadas quedan para otra ronda); agregar solo `Int`/`Float`; ningún campo opcional en ninguno de los dos roles. Agrupar por un campo enum devuelve el enum REAL como key, no un string degradado -- encontrado y corregido durante esta misma ronda (la primera versión sí degradaba, exactamente la clase de desacuerdo checker-vs-runtime que este proyecto viene evitando desde v1.0). `AVG` siempre da `Float`, `SUM`/`MAX`/`MIN` preservan el tipo de la columna. Portátil entre SQLite y Postgres sin ninguna rama por backend. Verificado contra los dos backends -- SQLite vía `test "..."` real (incluida una comparación de VALOR exacto sobre la key enum, no solo longitud) y Postgres en CI -- más 8 tests de compilación para cada camino de rechazo. 568 tests (11 nuevos). Detalle completo: GRAMMAR.md §3.52.

## [1.15.0] - 2026-08-21

### ✨ Nuevo
- **`auth.currentRole()`: leer el rol del caller dentro de un cuerpo.** Último gap de la misma serie de chequeos externos -- "no hay forma de leer el rol del caller dentro de un rpc... bloquea cualquier endpoint que hoy se comporte distinto según si eres agent o admin, no solo permitido/denegado". Con `@requires(Role.Admin | Role.Agent)` (v1.13) ya real, esto importaba de verdad: un endpoint compartido entre roles necesita a veces comportarse DISTINTO según cuál es, no solo decidir si entra. Devuelve `String?` (el nombre de la variante, ej. `"Admin"`), no el enum real -- evitar que el checker necesite saber con qué enum se autenticó CADA rpc en cualquier punto anidado de una expresión, mismo motivo por el que `response.setStatus` (v1.10) tampoco intentó saber si estaba dentro de un `stream`. Disponible SIEMPRE, sin requerir `@requires`/`@authenticated` en el rpc que lo llama -- mismo criterio que `request.rawBody()`/`request.header()`. `null` para "sin sesión" y "token inválido/vencido" por igual -- reusa `SessionStore::role_for` tal cual, así que hereda esa indistinguibilidad gratis, sin código nuevo. Cero cambios al modelo de sesión: ningún parámetro nuevo en `createSession`, el rol ya viajaba en la sesión desde v0, solo faltaba exponerlo. Límite honesto que sigue sin resolverse: solo el ROL, nunca la identidad completa del caller (`ctx.user`) -- la sesión nunca guardó una referencia al `User` real. Verificado contra un servidor real: un `sharedPanel` con `@requires(Role.Admin | Role.Agent)` respondiendo contenido DISTINTO según cuál rol autenticó, funcionando también en un rpc sin ninguna anotación de auth, y `null` tanto sin token como con uno inexistente. 557 tests (2 nuevos). Detalle completo: GRAMMAR.md §3.51.

## [1.14.0] - 2026-08-21

### ✨ Nuevo
- **`--session-ttl`: expiración real de sesión.** Último gap de la misma serie de chequeos externos -- "las sesiones no expiran solas... no hay forma de expresar 'sesión válida 7 días'". `linkc serve app.link 8787 --session-ttl 7d` (o `LINK_SESSION_TTL`, formato `Ns`/`Nm`/`Nh`/`Nd` -- mismo espíritu que `@rate_limit("20/1m")` pero CON días, porque la escala típica de una sesión los pide de verdad). Configuración de SERVIDOR, no del lenguaje -- `auth.createSession(role)` no ganó ningún parámetro nuevo, mismo criterio que `--cors-origin`/`--db`. Sin flag/variable, cero cambios de comportamiento: sigue viviendo hasta `destroySession()` o reiniciar el proceso, como siempre. Limpieza perezosa (una sesión vencida se borra recién en el próximo acceso a ese token, no por un barrido de fondo -- este intérprete no tiene ningún hilo de mantenimiento) -- costo documentado, no escondido. Token vencido y token que nunca existió dan el mismo 401, indistinguibles desde afuera a propósito, mismo criterio que ya regía para no revelar qué rol hacía falta en un 403. `Instant` (monotónico), no `SystemTime`, para medir el TTL -- inmune a que el reloj del sistema salte por NTP o cambio de horario. Verificado contra un servidor real: `--session-ttl 2s`, login real, acceso inmediato aceptado, el MISMO token rechazado 3 segundos después sin haber llamado `destroySession`. 555 tests (7 nuevos). Detalle completo: GRAMMAR.md §3.50.

## [1.13.0] - 2026-08-21

### ✨ Nuevo
- **`@requires(Role.Admin | Role.Agent)`: OR de roles.** Último gap real de la misma serie de chequeos externos -- "un solo rol por `@requires`" (v0, §3.14) hacía que un endpoint compartido entre dos roles (un dashboard que ven tanto Admin como Agent) no tuviera forma de expresarse sin duplicar el rpc completo. Reusa el `|` que ya existía para uniones de tipo (`A | B`), sin gramática nueva -- mismo token, significado análogo ("cualquiera de estos"). Todas las alternativas tienen que venir del MISMO enum -- una sesión tiene el rol de un solo enum a la vez, así que mezclar dos (`Role.Admin | Status.Active`) no tiene significado; se rechaza en el PARSER (puramente sintáctico, no hace falta tabla de símbolos) con el error apuntando exactamente al identificador que no matchea. Cada alternativa se sigue validando contra el enum declarado, igual que la v0 de un solo rol -- una variante inexistente sigue siendo un error de COMPILACIÓN, nunca un 403 imposible de satisfacer. Runtime: mismo mecanismo de siempre, un `.any()` más sobre la lista de variantes en vez de comparar contra una sola. Verificado contra un servidor real: dos logins con roles DISTINTOS, ambos aceptados por el mismo `@requires` compartido; un tercer rol rechazado; un `@requires` de un solo rol en el mismo programa sin ningún cambio de comportamiento. 551 tests (4 nuevos). Detalle completo: GRAMMAR.md §3.49.

## [1.12.0] - 2026-08-21

### ✨ Nuevo
- **`db.<coleccion>.page(limit, offset)`: paginación real, empujada a SQL.** Antes, acotar una colección grande significaba `.all().take(n)` -- pero `.take` (un método de `List<T>`) corre DESPUÉS de que `.all()` ya trajo la tabla ENTERA a memoria; pedir "la página 400" costaba lo mismo que traer la tabla completa. `page` pone `LIMIT`/`OFFSET` DENTRO del SQL -- portátil entre SQLite y Postgres sin ninguna rama por backend (`Backend::placeholder` ya resolvía esa diferencia). Mismo `ORDER BY "id"` que `.all()`, siempre, para que las páginas sean determinísticas (nunca se solapan ni se saltean una fila). `limit`/`offset` negativos son un error de runtime claro ANTES de tocar SQL -- Postgres y SQLite tratan un valor negativo de forma DISTINTA entre sí (la clase de divergencia entre capas que este proyecto viene evitando desde v1.0), así que dejarlo pasar tal cual hubiera hecho que el mismo programa se comportara diferente según el backend. Límite honesto: sin cursor, el caller arma el siguiente `offset` a mano. Verificado contra los DOS backends: test unitario con SQLite en memoria (páginas sin solapar, última página parcial, offset más allá del final como lista vacía, valores negativos rechazados) y el mismo caso repetido en `pg_integration.rs` contra un PostgreSQL real en CI. Detalle completo: GRAMMAR.md §3.48.

## [1.11.0] - 2026-08-21

### ✨ Nuevo
- **`http.getWithHeaders`/`http.postWithHeaders`: headers en llamadas salientes.** `http.get`/`http.post` (v1.0) ya existían pero sin ninguna forma de mandar un header -- así que aunque la llamada saliente funcionaba, autenticarse contra CUALQUIER API real de terceros (Stripe, GitHub, cualquiera que exija `Authorization`) era imposible. Era el lado saliente que quedaba pendiente, simétrico a `env.get`/`crypto.hmacSha256` (v1.3, el lado entrante de verificar webhooks). Dos métodos NUEVOS, no una sobrecarga de aridad variable sobre los existentes -- `http.get`/`http.post` quedan sin cambios. El tipo de cada header es `{name: String, value: String}[]`, estructural y SIN nombre reservado por el lenguaje (`Map<K,V>` se descartó: no tiene forma literal en c-script, ningún mecanismo para construir un valor desde cero) -- cualquier struct que el programa declare con esos dos campos funciona, gracias al subtipado estructural que `type` ya tiene (v1.0). Límite honesto: ni la versión con headers ni la original exponen el status code ni los headers de la RESPUESTA -- un 4xx/5xx se ve como un error de runtime genérico, no un valor que el programa pueda inspeccionar. Verificado contra un servidor HTTP real armado a mano en el test (no un mock interno): confirma que los headers declarados llegan tal cual, que el body de un POST viaja junto con ellos, y que un host inalcanzable falla limpio (no panic) -- de paso, primera cobertura de tests real para `http.get`/`http.post`, que no tenían ninguna hasta ahora. Detalle completo: GRAMMAR.md §3.47.

## [1.10.0] - 2026-08-21

### ✨ Nuevo
- **`response.setStatus(code)`: página 404 propia para un `@route`.** Último límite honesto real que quedaba de v1.1.0/v1.6.0 — un rpc `@route`+`@content_type("text/html")` (pensado para navegación directa del browser, no para el cliente generado) solo podía devolver 200; "no encontrado" no tenía forma de ser otra cosa que un `Err`/panic, y un error SIEMPRE sale como JSON, rompiendo justo la página HTML que se quería mostrar. La primera idea de diseño (dejar que `@content_type` acepte `Result<String, E>`) se descartó: rompe el contrato de `Result<T,E>` que el cliente generado ya asume, y de todos modos `E` es un `enum` de dominio, no HTML. La pieza que realmente faltaba era más chica: que un rpc elija SU status en el camino de ÉXITO. Mismo mecanismo que `request.rawBody()`/`request.header()` (v1.3.0) — un side-channel por request dentro de `Db`, no una nueva forma de `Value` que hubiera divergido entre checker y runtime. No está atado a HTML: cualquier rpc puede pedir un status de éxito distinto de 200 (`201` en un `create`, por ejemplo). Validado en runtime (100–599, el argumento puede ser cualquier expresión, no un literal); un `Err` posterior a llamarlo lo ignora por completo, el camino de error sigue siendo JSON siempre. Verificado contra un servidor real: una 404 con HTML propio, un 201 sobre un rpc JSON plano, y un código fuera de rango devolviendo el 500 esperado. Detalle completo: GRAMMAR.md §3.46.

## [1.9.0] - 2026-08-21

### ✨ Nuevo
- **`"...".escapeHtml()`: sanitizar datos antes de interpolarlos en una página.** `@content_type("text/html")` (v1.1.0) permitía devolver HTML de verdad, pero la respuesta se arma concatenando `String` sin ningún escapado — un nombre de usuario o un comentario con `<script>` podía terminar ejecutándose en el navegador de quien mira la página. Un método más sobre `String` (mismo lugar que `.trim()`/`.toUpper()`), no un tipo de string nuevo ni un sistema de templates con auto-escape implícito — deliberado, para no inventar una construcción de "template" encima de la nada. Escapa `& < > " '`, con `&` primero (si fuera al final, re-escaparía las entidades que las otras reemplazadas ya insertaron). No es automático: nada fuerza a usarlo, sigue siendo responsabilidad de quien escribe el rpc. Verificado con un payload de XSS de libro contra un servidor real, no solo el método en aislamiento. Detalle completo, incluidos los contextos que este escape NO cubre (dentro de `<script>`, atributos sin comillas): GRAMMAR.md §3.45.

## [1.8.0] - 2026-08-21

### ✨ Nuevo
- **PostgreSQL LISTEN/NOTIFY: `stream` entre varias instancias.** Último límite honesto que quedaba de v1.1.0 — dos instancias de `linkc serve` contra la misma base no se enteraban de las escrituras de la otra en un `stream`. Cambio real al modelo de concurrencia del intérprete: una conexión Postgres SEPARADA y dedicada solo a `LISTEN` (un único canal para todas las colecciones, el nombre va adentro del payload), corriendo en un hilo de fondo que se auto-repara igual que la conexión de queries (v1.4.0) si se corta. Cada `insert`/`applyPatch`/`delete` publica local primero y además hace `NOTIFY` vía `pg_notify()` (parámetro bindeado, no SQL armado a mano); cada instancia se reconoce a sí misma por un id de proceso en el payload, así que nunca duplica su propio eco. El loop principal del servidor pasó de bloquear en `incoming_requests()` a `recv_timeout` (200ms) cuando hay Postgres de por medio, para poder drenar el canal de cambios remotos sin dejar de atender requests HTTP; con SQLite el comportamiento no cambia. Límites honestos: el payload de NOTIFY tiene el tope de 8000 bytes de Postgres mismo (un cambio más grande no se propaga, pero sigue publicándose local donde se escribió), es best-effort sin cola de reintento, hasta 200ms de latencia en un servidor inactivo, una conexión Postgres más por instancia, y SQLite sigue sin participar. Verificado contra DOS procesos `linkc serve` reales apuntando a la misma base de Postgres: detalle completo en GRAMMAR.md §3.44.

## [1.7.0] - 2026-08-20

### ✨ Nuevo
- **`smtp.send(to, subject, body)`: mandar email.** Cierra el último gap de la misma lista de bloqueos de migración de esta serie de rondas — un backend real casi siempre necesita mandar mail (confirmar un registro, resetear una contraseña) y no había forma de hacerlo desde c-script. La conexión (`LINK_SMTP_URL`) y el remitente (`LINK_SMTP_FROM`) salen del entorno del proceso, nunca de argumentos del rpc — así un `.link` no puede hardcodear credenciales de un relay, y ningún caller puede spoofear el remitente con datos de la request. TLS vía `lettre` con el feature `rustls-tls` — mismo stack (`rustls` + `ring` + `webpki-roots`, sin OpenSSL) que ya usa el driver de PostgreSQL desde v1.4.0. Fallas (variable de entorno faltante, dirección inválida, relay inalcanzable) son errores de runtime normales, igual que `http.get`/`http.post` — nunca un panic. Límites honestos de esta ronda: un solo destinatario por llamada, solo texto plano (sin HTML ni adjuntos), sincrónico (bloquea el hilo único del servidor mientras dura el envío). Detalle completo, verificado contra un servidor SMTP real (escrito a mano en el propio test, sin dependencias externas): GRAMMAR.md §3.43.

## [1.6.0] - 2026-08-20

### ✨ Nuevo
- **`@route` con múltiples parámetros, en cualquier posición.** v0 (v1.2.0) solo permitía un segmento dinámico, y tenía que ser el último — `/blog/:categoria/:slug` se rechazaba. Ahora cualquier cantidad de segmentos `:nombre`, en cualquier posición, se bindean por NOMBRE (no por orden) contra los parámetros del rpc. La precedencia de siempre ("una ruta literal le gana a una dinámica que también matchearía") se generalizó a especificidad: gana la ruta con más segmentos literales fijos, determinísticamente. La detección de conflictos también se generalizó más allá de comparar formas exactas: dos rutas de forma DISTINTA que podrían igual matchear el mismo path real, empatadas en especificidad, se rechazan en compilación (`/blog/:categoria/latest` y `/blog/featured/:slug` matchean las dos `/blog/featured/latest`, y ninguna es más específica). Detalle completo, con el caso de conflicto cruzado explicado paso a paso: GRAMMAR.md §3.42.

## [1.5.0] - 2026-08-20

### ✨ Nuevo
- **CORS configurable y headers de seguridad fijos.** `linkc serve` mandaba `Access-Control-Allow-Origin: *` en toda respuesta, sin forma de acotarlo — sobre una API con auth Bearer, cualquier sitio podía leer una respuesta si el navegador de quien lo visita ya tenía el token guardado. `--cors-origin <origen>` (repetible) o `LINK_CORS_ORIGINS` (separados por coma) cambian el default a un allowlist real: el `Origin` de la request se compara exacto, se ecoa literal (nunca `*`) más `Vary: Origin` si matchea, y se omite el header por completo si no — la request se procesa igual, es el navegador quien bloquea la lectura. Sin configurar nada, el comportamiento no cambia. Además, toda respuesta —incluidos errores y un `stream` SSE, que arma su header a mano y antes no pasaba por el mismo camino— lleva ahora `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY` y `Referrer-Policy: no-referrer`. CSP y HSTS quedan afuera a propósito (CSP depende del contenido de cada página; HSTS le corresponde al reverse proxy que termina TLS, nunca a `linkc serve`). Detalle completo: GRAMMAR.md §3.41.

## [1.4.0] - 2026-08-20

### ✨ Nuevo
- **PostgreSQL: TLS oportunista y reconexión automática.** Los dos gaps reales que quedaban de una misma lista de bloqueos de migración ("Postgres sin pool/TLS/reconexión") — el tercero, pool de conexiones, no aplica: el intérprete es single-threaded, atiende una request a la vez, así que más de una conexión no compraría nada. TLS es `rustls` puro (crates `rustls` + `tokio-postgres-rustls`, backend `ring`), sin OpenSSL ni ninguna librería nativa del sistema, para que los 4 targets de release sigan compilando sin instalar nada — `sslmode` sale de la URL de conexión (`disable` = texto plano de siempre; sin especificar o `prefer` = intenta TLS y cae solo a texto plano si el servidor no lo ofrece, el nuevo default; `require` = TLS obligatorio). Antes de esta ronda, conectar a cualquier proveedor administrado que exige TLS (Supabase, Neon, RDS) era simplemente imposible. Reconexión: una conexión cortada ya no tira abajo el servidor hasta un reinicio manual — la request que la encuentra sigue fallando (nunca se reintenta a ciegas: podría duplicar un INSERT que el servidor ya había aplicado antes de cortarse), pero la conexión se reemplaza antes de devolver ese error, así que la request SIGUIENTE ya encuentra la base sana. Detalle completo, con los límites honestos que quedan (sin backoff más allá de un intento por request, sin fixture de CI con TLS real todavía, sigue sin `LISTEN`/`NOTIFY`): GRAMMAR.md §3.40.

## [1.3.0] - 2026-08-20

### ✨ Nuevo
- **`env.get`, `request.rawBody`/`request.header` y `crypto.hmacSha256`: verificar webhooks de terceros.** Disparado por un análisis de factibilidad de migración real que encontró un bloqueo concreto — sin leer una variable de entorno, ver el body crudo de una request ni calcular un HMAC, ningún rpc podía verificar la firma de un webhook entrante (Stripe, GitHub, o cualquiera que firme sus callbacks) y tenía que confiar en el body a ciegas. Las tres piezas juntas cierran eso. El contexto de la request (body + headers) vive en un `RefCell` sobre `Db` (ya threadeada en todo `runtime/mod.rs`), llenado por `runtime/server.rs` al principio de cada request — mismo criterio que ya usa `Db::subscribers`, en vez de sumar un parámetro más a las ~11 firmas que threadean `db`/`fns`/`checker`/`sessions`. Límite honesto: `rawBody()` requiere que el body sea JSON válido (aunque el rpc no use sus campos), porque el parseo de argumentos corre antes que cualquier rpc sin importar cuántos declare. Detalle completo, con el hallazgo de por qué CSRF NO aplica a este modelo de auth (Bearer-only, sin cookies) en vez de construir middleware para eso: GRAMMAR.md §3.38.
- **`@rate_limit("20/1m")`: límite de requests por cliente.** Como mucho N requests por ventana de tiempo, por `(ip del cliente, servicio, rpc)` — 429 al exceder, mismo shape de error que cualquier otro rechazo. Token bucket con refill continuo (no un contador de ventana fija, que deja pasar el doble en el borde de la ventana). La IP sale de la conexión TCP real (`Request::remote_addr`), nunca de `X-Forwarded-For` sin un proxy de confianza configurado (v0 no lo tiene). Combina con `@authenticated`/`@requires`/`@content_type`/`@route`; corre ANTES que el gate de auth, a propósito, para que un rpc protegido tampoco deje probar credenciales sin límite. Límite honesto: el estado vive en memoria de UN proceso, sin coordinación entre réplicas ni persistencia entre reinicios. Detalle completo: GRAMMAR.md §3.39.

## [1.2.0] - 2026-08-20

### ✨ Nuevo
- **`@route("/blog/:slug")`: URLs amigables para SEO.** `@content_type` (v1.1.0) ya permitía devolver HTML de verdad; el ruteo seguía siendo siempre `/Servicio/rpc`. Ahora un rpc puede declarar una URL alternativa, limpia y rastreable por GET, que convive con la dirección de siempre sin reemplazarla — el cliente TypeScript generado sigue llamando a `/Servicio/rpc`. Un segmento final `:nombre` se bindea a un parámetro `String` o `Int` del rpc con ese mismo nombre; sin parámetro, el rpc no puede pedir ninguno (v0 no lee query string ni body en una `@route`, a propósito, para que sirva tal cual a un crawler). Combina con `@authenticated`/`@requires`. Dos rutas con la misma forma se rechazan en compilación; una literal (`/blog/featured`) siempre gana sobre una dinámica que también matchearía (`/blog/:slug`) con el mismo criterio de precedencia que cualquier router HTTP común -- ese fue, de hecho, el primer bug real que este mismo feature encontró en su propio desarrollo (`cli_route.rs` lo fija como test explícito). Límites de esta ronda (un solo segmento dinámico y tiene que ser el último; no aparece en `openapi.json`; sin trailing slash): GRAMMAR.md §3.37.

## [1.1.1] - 2026-08-20

### 🔥 Arreglo crítico
- **Una tabla PostgreSQL preexistente con `id` no entero tiraba abajo el servidor completo.** Bug introducido en la propia v1.1.0, encontrado en un intento real de migración desde un backend que ya usaba UUID como clave primaria. `CREATE TABLE IF NOT EXISTS` es un no-op sobre una tabla que ya existía y nunca miraba sus columnas; el primer `insert` contra ella hacía panic (`Row::get` de `tokio-postgres` panickea si el valor no convierte al tipo pedido) en el hilo principal del servidor -- así que no fallaba esa request, fallaba el proceso entero, para todos los clientes conectados. Ahora `linkc serve` rechaza el arranque con un mensaje claro (qué tabla, qué tipo encontró) antes de aceptar una sola conexión, y ninguna lectura de PostgreSQL en `store.rs` puede panickear (defensa en profundidad). Detalle completo y verificación contra un PostgreSQL real: GRAMMAR.md §3.36.

## [1.1.0] - 2026-08-20

Versión menor y no de parche porque hay features nuevas (PostgreSQL en runtime,
`@content_type`), no solo correcciones. La única incompatibilidad práctica es
para quien use `linkc` como biblioteca de Rust: `runtime::server::serve` ahora
recibe un `DbSource` en vez de un `PathBuf`. Los programas `.link` no cambian.

### 🔐 Seguridad
- **`crypto.hashPassword` ahora es Argon2id** (RFC 9106) con sal aleatoria por contraseña y salida en formato PHC. Antes era un solo SHA-256 sobre la constante `"link_salt_2026"` — la misma sal para toda aplicación escrita en el lenguaje, sin iteraciones: dos usuarios con la misma contraseña compartían hash y una sola rainbow table las rompía todas.
- **`crypto.verifyPassword` compara en tiempo constante.** La comparación anterior (`==` de `String`) cortaba en el primer byte distinto y filtraba, por tiempo de respuesta, cuánto del hash había acertado quien probaba. Sigue aceptando los hashes del formato viejo para no dejar afuera a los usuarios ya registrados de una app en producción.
- **`crypto.randomToken` y `crypto.uuid` salen del CSPRNG del sistema.** Antes derivaban de `SystemTime::now().as_nanos()`: eran adivinables para quien pudiera acotar el instante de emisión, y dos llamadas dentro del mismo nanosegundo devolvían el mismo valor.
- **Los tokens de sesión piden entropía directo al SO** (`getrandom`), reemplazando el rodeo del hilo descartable sobre `RandomState` que documenta GRAMMAR.md §3.14.
- Detalle completo, con los límites que quedan (parámetros de Argon2id no configurables desde el lenguaje, sin señal de re-hash, el hashing bloquea el hilo del servidor ~15 ms): GRAMMAR.md §3.34.

### ✨ Nuevo
- **PostgreSQL como base de runtime, no solo como DDL generado.** `linkc serve app.link 8787 --db postgres://...` (o `LINK_DATABASE_URL`) corre el programa contra un PostgreSQL real, con el mismo `.link`, el mismo contrato generado y los mismos `test`. Hasta ahora `runtime/postgres.rs` solo emitía `schema.postgres.sql` y `linkc serve` usaba SQLite siempre, sin excepción — el "adaptador enterprise" del README no tenía el otro extremo. SQLite sigue siendo el default. Incluye auto-migración no destructiva (`ADD COLUMN IF NOT EXISTS`), enums simples legibles como texto y structs anidados en JSONB consultable. Límites (una sola conexión sin pool ni TLS, sin `LISTEN`/`NOTIFY`, la columna migrada queda nullable): GRAMMAR.md §3.36.
- Verificado contra un PostgreSQL de verdad en CI (`postgres:16`): CRUD completo por HTTP, persistencia entre reinicios, la tabla leída desde SQL plano, el esquema real comparado contra el que emite `linkc build`, y una migración que agrega un campo sin perder filas.

### ✨ Nuevo
- **`@content_type("...")`: respuestas que no son JSON.** Un rpc que devuelve `String` puede declarar el Content-Type de su respuesta, y entonces el cuerpo se escribe tal cual: HTML, sitemaps XML, CSV, texto plano. Antes el Content-Type estaba literal en el binario (`application/json` para rpcs, `text/event-stream` para streams) y un programa c-script no podía devolver una página, lo que dejaba fuera cualquier render en servidor y cualquier historia de SEO. Las tres capas cambiaron juntas: el servidor manda el header, el cliente TypeScript generado lee `res.text()` en vez de `res.json()`, y el spec OpenAPI declara el mismo tipo. Detalle y límites (sin rutas limpias, sin escapado de HTML, los errores siguen en JSON): GRAMMAR.md §3.35.
- **Las anotaciones de un rpc pasaron a ser una lista.** `@requires(Role.Admin) @content_type("text/html")` es válido — auth y Content-Type son dimensiones distintas, y un panel de administración necesita las dos. El checker sigue rechazando dos anotaciones de la misma dimensión.

### 🩺 Diagnósticos
- **Los tipos en los errores del compilador se escriben como en c-script.** Interpolaban el `Debug` de Rust, así que un error sobre un `T?` mostraba `Optional(Struct { name: Some("Todo"), fields: [FieldType { name: "id", optional: false, ty: Int }, ...] })`. Ahora dice `Todo?`.
- **El acceso a un campo sobre `T?` explica qué hacer**, no solo que no se puede: es el error más frecuente, porque en TypeScript `if (x != null)` sí angosta y en c-script no.

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
