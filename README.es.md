*[Read in English](README.md)*

<div align="center">
  <h1>⚡ Link (c-script)</h1>
  <p><strong>El lenguaje compilado de backend diseñado para garantizar Seguridad de Tipos Extremo a Extremo (End-to-End Type Safety) con TypeScript.</strong></p>
  
  <p>
    <a href="https://github.com/charlessonamericantrading/c-script-/actions/workflows/ci.yml"><img src="https://github.com/charlessonamericantrading/c-script-/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
    <a href="#-testing--quality-assurance"><img src="https://img.shields.io/badge/tests-1001-success.svg" alt="Tests" /></a>
    <a href="https://github.com/charlessonamericantrading/c-script-/releases"><img src="https://img.shields.io/badge/versión-1.83.0-blue.svg" alt="Versión" /></a>
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

## 📊 Estado — qué funciona y qué no

Esta sección es la verdad de fondo. Si cualquier otra parte de este README la contradice,
gana esta. Verificado el 24/08/2026 corriendo el compilador, no leyéndolo.

**Funciona hoy**, cubierto por 926 pruebas automáticas:

- `linkc build` / `serve` / `serve-all` / `migrate --dry-run` / `doctor` / `test` / `dev` / `lint` / `doc` / `docker` / `lsp` / `new`
- `linkc serve-all --port-map-out <archivo.json>`: escribe `{"nombre_archivo": puerto, ...}` antes de arrancar cualquier servicio, para que un gateway externo lea la asignación real en vez de replicar la regla de orden alfabético a mano. Falla limpio (no arranca ningún servicio) si la escritura en sí falla
- `linkc lint` marca `delete-then-insert-same-id`: `delete(x.id)` seguido de `insert(MismoTipo { id: x.id, ... })` sobre la misma colección -- `insert()` siempre asigna un id nuevo por autoincrement, nunca respeta un campo `id:` literal, así que esto nunca preserva la fila aunque el código parezca intentarlo. Recomienda `applyPatch`/`upsert` en su lugar
- `db.<c>.increment(id, selector, delta) -> T`: un `UPDATE "campo" = "campo" + ?` atómico, sin lectura previa -- arregla un riesgo real de lost-update (dos procesos leyendo el mismo valor antes de que ninguno escriba) que `upsert` con un `updateFn` de lectura-previa sí tiene bajo concurrencia real. `delta` negativo decrementa. Acotado a `Int` por ahora
- `db.<c>.maxRow(selector)` / `minRow(selector) -> T?`: la fila completa con el máximo/mínimo de un campo numérico, empujado a un `ORDER BY ... LIMIT 1` real -- a diferencia de `maxBy`/`minBy`, que solo agregan un valor, nunca la fila completa que lo alcanza
- `List<Int>.sum() -> Int`: suma todos los elementos con un loop real -- `List<Int64>`/`List<Float>` quedan deliberadamente afuera por ahora (una lista vacía de esos dos tipos no tiene de qué elemento leer el tag correcto de `Value` en runtime, y adivinar mal ahí sería un bug silencioso de formato en el wire, ya que `Int64` viaja como string y `Int` como número)
- `linkc doctor <archivo> [--db <url|archivo>]`: diagnóstico de entorno antes de un despliegue -- la versión de `linkc`, que el archivo de entrada resuelva sus imports/parsee/tipe, permiso de escritura en su directorio, y conectividad de solo lectura (`SELECT 1`, nunca DDL) a la base configurada. Imprime un checklist y sale con código `1` si algún chequeo real falló, pensado como paso de CI antes de `linkc serve`
- `linkc test <archivo> --db <url-postgres>` (o `LINK_TEST_DB`, deliberadamente separada de `LINK_DATABASE_URL`): corre cada bloque `test "..." { ... }` contra una base PostgreSQL real en vez de SQLite embebido -- necesario para reproducir de verdad un bug del wire de Postgres, ya que SQLite y Postgres emiten y decodifican SQL distinto para el mismo `.link`. Sin el aislamiento por test que SQLite `:memory:` da gratis -- Postgres no tiene equivalente, así que los tests comparten estado dentro de una corrida en vez de fingir un reset (que sería una operación destructiva que este proyecto evita a propósito); correr esto contra una base de test dedicada, nunca contra producción
- `linkc migrate <archivo> --db <url-postgres> --dry-run`: conecta de solo lectura y reporta el `CREATE TABLE`/`ALTER TABLE ADD COLUMN` exacto que `linkc serve` ejecutaría, sin ejecutar nada -- reusa las mismas funciones de generación de DDL que usa el runtime real, así que este reporte no puede desincronizarse de lo que pasa de verdad. También avisa una posible colisión de nombre de tabla o un tipo de `id` incompatible antes de que lo descubras conectando de verdad. Solo PostgreSQL -- SQLite ya falla fuerte con el diff exacto al conectar de verdad
- `@check(min, N)` / `@check(max, N)` / `@check(range, N, M)` sobre un campo `Int`/`Int64`/`Float`: una restricción de nivel de BASE, no solo código de aplicación -- se cumple TANTO en `insert`/`applyPatch` (400 que nombra el campo y el límite exacto) COMO en un `CHECK (...)` inline de verdad en el `CREATE TABLE` generado, en SQLite y en PostgreSQL. Confirmado escribiendo SQL crudo que evita a c-script por completo y viendo a la propia base rechazarlo, en los dos backends. `--adopt-existing` nunca ejecuta este DDL, pero la validación de aplicación sigue aplicando igual
- `db.<c>.countWhere(predicate) -> Int` cuenta filas que matchean con un `SELECT COUNT(*) ... WHERE` real cuando el predicado es una sola comparación `|x| x.campo OP valor` (`==`/`!=`/`<`/`<=`/`>`/`>=`) o una conjunción `&&` de varias hojas así (incluidas `!x.campo`/`x.campo` sueltas como hojas booleanas) -- cero filas viajan del motor al proceso. `findWhere` gana el mismo atajo (mismo reconocimiento, trayendo columnas reales en vez de `COUNT(*)`) sin cambiar su firma ni su comportamiento observable. Un predicado con `||`, o que compare dos campos del mismo parámetro entre sí, sigue funcionando igual que antes por el camino interpretado -- nunca un error, solo sin el atajo; `||` es el gap real que queda para una ronda dedicada. Respeta `@softDelete` incluso pusheado; `deleteWhere` todavía no gana este atajo
- PostgreSQL ahora avisa (nunca bloquea) al migrar una tabla preexistente cuyas columnas no tienen NADA en común con lo declarado -- el incidente real: un servicio estuvo a punto de fusionar en silencio su schema con una tabla ajena que casualmente compartía el nombre de colección. Deliberadamente una advertencia, no un fallo duro -- dos `.link` distintos compartiendo a propósito una tabla con columnas disjuntas es un patrón ya soportado que esta heurística no puede distinguir de una colisión accidental
- `--service-api-key <clave>`/`LINK_SERVICE_API_KEY` para `linkc serve`/`serve-all`: exige el header `X-Service-Api-Key` (comparado en tiempo constante) en toda request salvo `/health`/`/`/`/status`, verificado ANTES de leer el body -- cierra el hueco donde cualquier proceso en la misma máquina (no solo uno externo) podía llamar a un servicio igual que el gateway legítimo. Una capa distinta y anterior a `@requires`/JWT/sesiones (que autentican al USUARIO final, no a quién llama) -- las dos conviven en la misma request
- `linkc serve-all <dir> --port-base N` corre TODOS los `.link` de un directorio en un solo proceso (un hilo por servicio, con su propio puerto y su propio SQLite cada uno) en vez de un proceso por servicio -- el caso real que lo motivó: 13-17 procesos `pm2` separados en una adopción de producción, uno por `.link`. `--restart-backoff <duración>` (también usable con `linkc serve` solo) agrega backoff exponencial nativo ante un fallo de arranque recuperable (puerto ocupado, Postgres caído) -- un fallo de bind/conexión en un servicio ya no se lleva a los demás por delante
- `dateFromParts(year, month, day, hour, minute, second) -> Timestamp` construye un `Timestamp` arbitrario a partir de sus componentes de calendario -- `now()` solo daba el instante ACTUAL, así que calcular algo como el inicio de un trimestre enteramente adentro de un rpc era imposible antes de esto. Una fecha inválida (mes 13, 30 de febrero) es un 400 que nombra el campo mal formado, nunca un panic
- Un campo `Timestamp` ahora decodifica columnas `date`/`timestamp`/`timestamptz` NATIVAS de PostgreSQL, no solo la convención `BIGINT`-milisegundos que genera `linkc build` -- el caso común al adoptar una tabla ya existente, donde las columnas de fecha casi siempre son el tipo nativo de Postgres. Decodificado a mano contra el wire binario de Postgres (sin sumar la dependencia `chrono`); `linkc introspect` ahora recomienda `Timestamp` sin advertencia para estas columnas, en vez de un mapeo a `String` que en la práctica tampoco funcionaba. Solo lectura por ahora -- escribir a una columna nativa desde c-script todavía no funciona
- Un campo `Float` ahora decodifica columnas `numeric`/`decimal` NATIVAS de PostgreSQL también, no solo `float4`/`float8` -- el caso común para una columna de dinero en una tabla adoptada, ya que `numeric` es justo lo que evita el error de redondeo binario que `float8` sí tiene. Decodificado a mano contra el wire (sin dependencia nueva), mismo espíritu que el fix de `Timestamp` de arriba. Solo lectura por ahora. Aparte, escribir un `Int` contra una tabla adoptada cuyo `id` (o cualquier otro campo `Int`) es físicamente `SERIAL`/`SMALLINT` en vez de `BIGINT` también quedó arreglado -- el camino de escritura corrompía el protocolo binario en silencio, codificando siempre 8 bytes sin importar el ancho real de la columna
- `--trust-proxy`/`LINK_TRUST_PROXY` para `linkc serve`: hace que `@rate_limit` identifique al cliente por el primer valor de `X-Forwarded-For` en vez de `remote_addr()` -- apagado por default, porque `remote_addr()` es siempre la IP del proxy detrás de un reverse proxy/load balancer real (confirmado como bloqueo real en producción: la adopción de IgnisLove corre todo detrás de nginx), compartiendo el límite entre todos los usuarios reales a la vez. Opt-in explícito a propósito -- prenderlo sin un proxy de confianza real delante deja evadir el límite mandando un header distinto en cada request. v0 confía en el header completo una vez prendido, sin mecanismo de "N saltos de confianza" ni rango CIDR todavía
- `linkc lint` marca `==`/`!=` sobre algo nombrado como un secreto (`token`, `password`, `apiKey`, ...) con `timing-unsafe-secret-comparison`, recomendando `crypto.timingSafeEqual` en su lugar -- un `==` de `String` corta en el primer byte distinto, filtrando cuánto acertó quien lo adivina. Comparar contra `null` (chequeo de presencia) queda afuera a propósito. Recorre todo el cuerpo en cualquier nivel de anidamiento (`if`/`match`/`while`/closures); puramente informativo, `linkc lint` sigue saliendo con código 0
- `linkc lint` también marca un `const` de nivel superior cuyo valor literal tiene forma de URL de conexión con credenciales embebidas, o cuyo nombre sugiere un secreto con un valor literal no vacío -- `hardcoded-secret-literal`. El mensaje recomienda leer el valor con `env.get("...")` en el momento de uso en su lugar, ya que un `const` en c-script solo puede llevar un literal (una llamada como `env.get(...)` ahí es un error de compilación aparte, nunca un reemplazo válido del valor del const)
- `/health` (`/`, `/status`) verifica conectividad REAL a la base -- un `SELECT 1` en cada request, sin caché. Hasta ahora devolvía siempre `200` fijo, inútil para cualquier orquestador (Kubernetes, un load balancer) que lo usa para decidir si reiniciar el proceso: podía estar vivo y sin embargo incapaz de servir ningún rpc real porque la base estaba caída, y `/health` igual reportaba todo bien. Devuelve `503` con `"status":"error"` y la falla real en un nuevo campo `"database"` cuando el chequeo falla; del lado Postgres pasa por la misma auto-reparación de conexión que cualquier otra query, así que una caída transitoria se cura ahí mismo
- `--http-timeout <duración>`/`LINK_HTTP_TIMEOUT` para `linkc serve`: acota cuánto puede tardar cualquier llamada saliente `http.*` -- 30s por default. Hasta ahora `http.get`/`post`/`getWithHeaders`/etc. no tenían timeout de lectura/escritura (`ureq` solo trae 30s de timeout de CONEXIÓN por default); contra este intérprete de un solo hilo, un servidor remoto lento o colgado bloqueaba el proceso entero para siempre -- ni `/health` respondía mientras tanto. Mismo orden de precedencia y formato de duración (`Ns`/`Nm`/`Nh`/`Nd`) que `--session-ttl`; un timeout agotado se reporta como un error de runtime normal, nunca un panic ni un colgado
- `--max-body-bytes <N>`/`LINK_MAX_BODY_BYTES` para `linkc serve`: acota cuántos bytes de body puede mandar una request -- 10 MiB por default. Hasta ahora el servidor leía el body entero a memoria sin ningún límite, un vector real de agotamiento de memoria. La lectura se acota con `Read::take(max_body_bytes + 1)` y se rechaza con `413 Payload Too Large` ANTES de leerlo completo -- auth, rate limiting y el parseo del JSON nunca llegan a competir por memoria con un body ya sabido demasiado grande. Límite de proceso, no por rpc; no se drena el resto de un body rechazado (si el cliente reusa la misma conexión igual, el siguiente intento da un 400 limpio, nunca un colgado ni una fuga)
- `linkc --version`/`-v`/`version` imprime la versión exacta del compilador (`env!("CARGO_PKG_VERSION")`, tomada de `Cargo.toml` en tiempo de compilación) -- la misma constante estampa el header de cada archivo TypeScript generado (`contract.d.ts`/`client.ts`/`hooks.ts`/`validators.ts`/`schemas.ts`) y, como JSON no admite comentarios, una extensión de vendor `x-generated-by` en `openapi.json` (nunca `info.version`, que es la versión del API documentada, un concepto aparte). Puramente informativo -- nada compara la versión estampada en un `gen/` viejo contra el binario que lo sirve o reconstruye
- `linkc test <archivo> --filter <nombre>`: corre solo los bloques `test "..." { ... }` cuyo nombre CONTIENE ese substring (sensible a mayúsculas, mismo criterio que `cargo test <substring>`) -- un filtro que no matchea nada corre cero tests y termina con éxito igual. Solo aplica al test runner integrado, nunca al testing de contrato por snapshot (`linkc test <archivo> <snap>`), que no tiene nombres que filtrar -- combinar los dos es un error de uso claro, no un flag ignorado en silencio
- `--host <dirección>`/`LINK_HOST` para `linkc serve`: escucha en `0.0.0.0` (todas las interfaces) por default, igual que antes -- o en una dirección puntual (`127.0.0.1`, para un proceso que solo necesita conexiones locales) para que el firewall del sistema operativo no sea la única barrera contra el resto de la red. Se pasa tal cual al bind subyacente, sin resolución ni validación propia más allá de rechazar `--host ""` vacío -- una dirección que no le pertenece a ninguna interfaz local hace fallar el arranque nombrando esa dirección exacta, nunca cae en silencio a `0.0.0.0`
- Índices declarativos de un solo campo: `@index`/`@unique` sobre un campo de struct -- ninguna de las dos exige un tipo de campo particular. El índice se crea de verdad al arrancar en los dos backends (`CREATE [UNIQUE] INDEX IF NOT EXISTS`, idempotente, nombre determinístico), y `linkc build` emite la misma sentencia en el DDL estático de Postgres. Una violación de `@unique` en `insert`/`applyPatch` (y en la rama de update de `upsert`) se traduce a 400, no a un 500 genérico -- detectando el mensaje específico que SQLite/Postgres devuelven para esa violación puntual. `--adopt-existing` tampoco ejecuta este DDL, mismo criterio que el resto del schema. Índices/constraints COMPUESTOS (de varios campos) todavía no están soportados -- solo de un campo
- `linkc build --diff <archivo>`: compara el `contract.d.ts` recién generado contra una copia guardada aparte (típicamente `git show <rev>:ruta > archivo` antes del build) -- para revisar exactamente qué cambió en el contrato público de un PR. Reusa el mismo diff LCS que `linkc test` ya tenía para mostrar por qué un snapshot cambió. Puramente informativo, nunca hace fallar el build -- un archivo de comparación ilegible solo imprime una advertencia por stderr
- Soft-delete nativo: `@softDelete` sobre un campo `Timestamp?` convierte `delete(id)` en un `UPDATE` idempotente (fija el campo a `now()`, `AND "<campo>" IS NULL` en el WHERE para que una segunda llamada sea un no-op que devuelve `false`, nunca un `DELETE` real). Toda lectura que devuelve lista o conteo -- `all()`, `page()`, `pageAfter()`, `count()`, los agregados `*By`, y `findWhere`/`deleteWhere` (que reusan `all()` por dentro, sin código extra) -- lo filtra automáticamente. `find(id)` deliberadamente NO filtra -- una fila soft-deleteada sigue siendo encontrable por id directo, mismo criterio que Django/Rails, necesario para que la re-consulta de `insert`/`applyPatch` no explote si un patch toca justo ese campo
- `createdAt`/`updatedAt` automáticos: sin nombres de campo mágicos -- `createdAt: Timestamp = now()` (un default ya existente combinado con el builtin `now()` ya existente) ya cubre "asignado una sola vez al crear". `@autoUpdate` sobre un campo `Timestamp` (solo) es la única pieza nueva -- fuerza ese campo a `now()` en cada `applyPatch`/`upsert`-actualización, aunque el patch no lo mencione, mientras un campo sin la anotación nunca se toca solo
- `db.<c>.insertMany(items) -> T[]`: cada elemento pasa por el mismo `insert` real de siempre (una sentencia SQL autocommit por fila), en el orden dado -- ahorra las N idas y vueltas HTTP secuenciales del cliente para un backfill, no el costo de N inserts contra la base. Sin transacción envolvente: si el ítem 3 de 5 falla, los dos primeros quedan insertados igual
- `db.<c>.upsert(matchFn, insertValue, updateFn)`: actualizar-en-el-lugar-o-insertar sin reimplementar a mano buscar+borrar+reinsertar (que ni siquiera preserva el id de la fila con un autoincrement real). `matchFn` recorre toda la colección en el intérprete (mismo límite que `findWhere`/`deleteWhere` -- no empujado a SQL todavía); con match, `updateFn` recibe la fila existente completa y su resultado se aplica sobre ESE MISMO id. `updateFn` devuelve un valor `Omit<T,"id">` completo, no un `Patch<T>` parcial -- deliberado, ya que `Patch<T>` no tiene sintaxis de literal y no se podría construir adentro de un cuerpo de función
- Valores por defecto en campos de struct: `status: String = "pending"` -- misma sintaxis y mecanismo que un default de parámetro de función/rpc. Un campo con default se puede omitir de un literal `Struct { ... }` sin volverse `Optional` -- sigue siendo el mismo tipo declarado. Lo completa el intérprete al construir, evaluado de nuevo cada vez (`token: Uuid = crypto.uuid()` da un valor distinto por literal, no uno cacheado). Se propaga como campo opcional a `contract.d.ts`/`schemas.ts` (Zod), y fuera de `required` en `openapi.json` (más un valor `"default"` literal cuando es una constante simple). Sin acceso a otros campos del mismo literal, y sin soporte todavía en un `type` genérico
- `@validate(email)` / `@validate(regex, "...")` sobre un campo `String`/`String?`, con enforcement real en cuatro lugares -- el servidor real (400 ante un valor inválido, chequeado tanto cuando un rpc recibe el struct entero como parámetro como cuando lo arma adentro del cuerpo a partir de parámetros sueltos -- un `curl` real contra un servidor corriendo fue lo que reveló que ese segundo camino faltaba al principio), `openapi.json` (`format`/`pattern`, keywords estándar de JSON Schema), `schemas.ts`/Zod (`.email()` / `.regex(new RegExp(...))`, encadenado correctamente ANTES de `.nullable()` en un campo opcional) y un comentario JSDoc informativo en `contract.d.ts`. Un patrón regex mal formado es un error de compilación, nunca una sorpresa en el primer request. El límite real: `@validate` está atado a la declaración exacta donde se escribe -- el shape "New*" (`Omit<T,"id">`) que se usa en todos lados para `insert` es un tipo aparte, así que hay que repetir la anotación ahí también. Las funciones `isX()` hand-escritas de `validators.ts` todavía no lo enforce -- todo lo demás sí
- Docstrings `///` sobre un rpc/stream, propagados como `description` en el `openapi.json` generado y como bloque JSDoc multilínea en `contract.d.ts` -- puramente aditivo: `///` ya era válido en cualquier posición (misma trivia que `//`), así que ningún programa existente deja de compilar; el parser solo lee el texto capturado justo arriba de un `rpc`/`stream` (a través de una `@annotation` en el medio, si hay). Se combina con `@deprecated` sobre el mismo rpc en un solo campo/bloque en vez de que uno pise al otro
- `@deprecated("usa X en su lugar")` sobre un campo de struct o un rpc/stream -- puramente informativo, sin efecto en runtime ni en la subtipificación estructural (un struct sigue siendo el mismo tipo lleve o no un campo esta anotación). Se propaga como comentario JSDoc `/** @deprecated ... */` justo antes del campo/método en el `contract.d.ts` generado, y como `deprecated: true` + `description` nativos (keywords de Operation Object / JSON Schema 2020-12, sin extensión `x-*` propia) en `openapi.json`. Sobre un campo es la ÚNICA anotación que se acepta ahí -- cualquier otro nombre (`@authenticated`, etc.) es un error de sintaxis en esa posición, no algo que se ignore en silencio
- Narrowing real de `T?` dentro de un cuerpo de rpc: `match x { v: T => v.campo, null => ... }` liga `v` al `T` real (no `T?`) dentro de esa rama -- reusa la misma maquinaria de patrones exhaustivos que ya narrowaba uniones, así que faltar el caso `null` o el caso de valor es un error de compilación, no una sorpresa en runtime. `if x != null { x.campo }` sigue sin angostar -- eso queda deliberado -- pero `match` sí. `x ?? default` cubre el caso común "dame un default" (encadena de izquierda a derecha: `a ?? b ?? c`), y `x.isSome()`/`x.isNone()` cubren "solo necesito saber si hay valor", los dos sin necesitar un `match` completo
- Tipo nativo `Uuid`: valida la forma canónica `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` en cada borde que un valor puede cruzar -- decodificar una request entrante, `validators.ts`, y el schema Zod generado, los tres con exactamente el mismo chequeo para que nunca puedan discrepar. Tipo aparte de `String`, sin mezcla implícita (misma regla que `Int64` vs `Int`) -- `crypto.uuid()` devuelve `Uuid`, y `.toString()` es la bajada explícita a un string plano
- `linkc introspect <url-postgres>` genera un `.link` de partida (types + `db {...}`) desde el schema de una base PostgreSQL ya existente -- para adoptar un sistema con datos reales en vez de escribir cada campo a mano. Punto de partida para revisar, no listo para producción tal cual: cualquier columna que no pueda mapear con confianza (`jsonb`, `uuid`, un `timestamp`/`timestamptz` nativo) igual sale con un tipo válido (`String`) más una advertencia en stderr, nunca se omite en silencio. Solo PostgreSQL, sin generar ningún `service`
- SQLite embebido con persistencia real entre reinicios y auto-migraciones no destructivas
- Push en vivo sobre Server-Sent Events (`stream` + `db.<c>.subscribe()`)
- Auth declarativa: `@authenticated`, `@requires(Role.Admin)` (o `@requires(Role.Admin | Role.Agent)` para cualquiera de varios roles, todos del mismo enum), tokens de sesión desde el CSPRNG del sistema. `linkc serve --session-ttl 7d` (o `LINK_SESSION_TTL`) hace que las sesiones expiren solas -- sin configurar, siguen viviendo hasta `destroySession()` o un reinicio del proceso, como antes. `auth.currentRole() -> String?` lee qué rol autenticó la request actual desde adentro del cuerpo de un rpc -- deja que un endpoint `Role.Admin | Role.Agent` se comporte distinto según el rol, no solo permitir/denegar; funciona también sin ninguna anotación de auth, `null` si no hay sesión válida. `auth.createSessionWithId(role, userId)` asocia el id del usuario a la sesión y `auth.currentUserId() -> Int?` lo inspecciona desde el cuerpo de cualquier rpc (`null` si no hay sesión o se creó sin id). `auth.destroyAllSessions(userId: Int) -> Int` revoca de una vez todas las sesiones de un usuario (cambio de contraseña, un ban de admin) y devuelve cuántas se cerraron -- a diferencia de `destroySession()` (que solo opera sobre la sesión de la request ACTUAL, justamente para que nadie pueda revocar la de otro adivinando un token), éste sí toma un `userId` explícito, mismo criterio que `createSessionWithId`: un id de usuario es una clave de aplicación, no un secreto adivinable. Gatear quién puede llamarlo (típicamente `@requires(Role.Admin)`) es responsabilidad de quien escribe el `.link`
- Auth externo: `linkc serve --jwt-secret <secreto>` (o `LINK_JWT_SECRET`) verifica un JWT HS256 ya emitido por un backend existente -- junto con, nunca en vez de, las sesiones propias de Link. `@requires`/`@authenticated`/`auth.currentRole()`/`auth.currentUserId()` funcionan igual sin importar cuál de los dos autenticó la request. `--jwt-role-claim`/`--jwt-user-id-claim` (default `role`/`sub`) eligen qué claims traen el rol y el id de usuario; `sub` acepta un número JSON o un string de dígitos (convención real de OIDC). Solo HS256 -- cualquier otro `alg`, incluido `"none"`, se rechaza antes de siquiera revisar una firma
- PostgreSQL como base de runtime: `linkc serve app.link 8787 --db postgres://usuario:clave@host/base` (o `LINK_DATABASE_URL`), con auto-migración no destructiva (una columna nueva siempre queda nullable, incluso una requerida -- una fila preexistente con `NULL` ahí ahora falla ESA lectura con un error limpio que nombra la fila y el campo, nunca con un `null` silencioso mandado a un cliente tipado ni con el proceso cayéndose), TLS oportunista (rustls puro, sin OpenSSL -- conecta contra proveedores administrados como Supabase/Neon/RDS que lo exigen), reconexión automática tras una conexión cortada, y LISTEN/NOTIFY para que un `stream` conectado a una instancia de `linkc serve` vea una escritura que entró por otra instancia contra la misma base. El mismo programa, el mismo contrato generado — SQLite sigue siendo el default. El `schema.postgres.sql` generado nunca requiere `CREATE EXTENSION` para nada -- verificado aplicándolo con un rol de Postgres real sin privilegios de superusuario/createrole, el tipo que de verdad se tiene en un proveedor gestionado
- Adoptar una base existente sin tocarla: `linkc serve --adopt-existing` (o `LINK_ADOPT_EXISTING`) hace que cada colección declarada asuma que su tabla ya existe -- nunca ejecuta `CREATE TABLE` ni `ALTER TABLE`, ni siquiera el tipo no destructivo de siempre, solo chequeos de solo lectura de que cada columna declarada realmente esté ahí. Para un rol de base sin permiso de DDL (común en producción), o una tabla SQLite/Postgres que ya trae columnas que este programa no modela (que ahora simplemente ignora en vez de negarse a arrancar)
- Respuestas que no son JSON: `@content_type("text/html; charset=utf-8")` sobre un rpc que devuelve `String` manda ese cuerpo tal cual — páginas HTML, sitemaps XML, CSV — y se combina con `@requires(Role.Admin)` para páginas detrás de auth. `"...".escapeHtml()` sanitiza datos no confiables antes de meterlos en una página (no es automático -- se llama donde se interpola). `response.setStatus(code)` elige el status HTTP del camino de éxito (ej. una página 404 propia para un `@route` que no encontró nada, o 201 en un `create` JSON común) — los errores de transporte siguen saliendo siempre en JSON, sin cambios. `response.redirect(url, permanent: Bool)` manda un 301/302 real con header `Location` (301 si `permanent`) -- SEO básico, como consolidar una página movida sin perder su ranking. Rechaza un `url` vacío o con salto de línea (inyección de headers HTTP) con un error limpio; mismo tratamiento de error de compilación que `setStatus` dentro de un `stream`. `@cache_control("public, max-age=3600")` fija un header `Cache-Control` real -- se combina libremente con `@route`/`@content_type`/auth/`@rate_limit`, solo en el camino de éxito (una respuesta de error nunca lo hereda), rechazado dentro de un `stream` igual que `setStatus`/`redirect`
- URLs amigables: `@route("/blog/:slug")` le da a un rpc una URL limpia y rastreable por GET, además de (nunca en vez de) su dirección normal `/Servicio/rpc` — el cliente generado sigue usando esta última. Cualquier cantidad de segmentos `:parámetro`, en cualquier posición (`/blog/:categoria/:slug`), bindeados por nombre; una ruta más específica (más segmentos fijos) le gana determinísticamente a una totalmente dinámica que también matchearía. Un segmento catch-all final (`:nombre*`) captura el resto del path, unido con `/`. Cualquier parámetro del rpc que NO esté en el path se lee de la query string -- `String`/`Int` obligatorio, `String?`/`Int?` opcional (`null` si no vino) -- un filtro como `?page=2` ya no necesita un rpc aparte; `body` sigue sin leerse, a propósito, porque el punto es una URL que un crawler abre con un GET simple
- Verificar webhooks de terceros: `env.get(name)`, `request.rawBody()` / `request.header(name)` y `crypto.hmacSha256(secret, message)` le dan a un rpc todo lo necesario para chequear la firma de un callback de Stripe/GitHub/etc. antes de confiar en él
- URLs firmadas reales de AWS S3: `crypto.awsS3PresignedUrl(accessKeyId, secretAccessKey, region, bucket, objectKey, expiresSeconds) -> String` devuelve un link de descarga firmado, listo para usar -- `crypto.hmacSha256` solo no alcanza (AWS Signature V4 encadena los BYTES crudos de un HMAC como clave del siguiente, pero `hmacSha256` solo toma/devuelve `String` en hex), así que el protocolo entero corre adentro del runtime. Verificado byte a byte contra el vector de prueba oficial que publica AWS, sin necesitar ninguna cuenta de AWS real. Solo `GET` por ahora (compartir/descargar), no `PUT`
- `base64.encode(data: String) -> String` / `base64.decode(base64Str: String) -> String` (RFC 4648 estándar): junto con `http.postWithHeaders`, es todo lo que necesita un proveedor con HTTP Basic Auth (Twilio, y la mayoría de los que no usan Bearer token) -- `Authorization: "Basic " + base64.encode(usuario + ":" + clave)`. `decode` devuelve `String`, así que bytes decodificados que no son UTF-8 válido son un error de runtime limpio, no bytes crudos -- el lenguaje no tiene un tipo de datos binarios
- El flujo OAuth2 "client credentials" (servidor a servidor, sin login de usuario -- el que usan Google APIs/Microsoft Graph/Salesforce/HubSpot para auth de máquina) no necesita ningún builtin nuevo: `http.postWithHeaders` para pedir el token, `json.parse(text) -> Dynamic` con acceso de campo directo (`.access_token`, tipado `Dynamic`, asignable a `String` sin cast) para leer el token sin declarar la forma completa de la respuesta del proveedor, `http.getWithHeaders` con `"Bearer " + token` para la llamada real
- Llamar APIs de terceros: `http.get(url)` / `http.post(url, body)`, más `http.getWithHeaders(url, headers)` / `http.postWithHeaders(url, body, headers)` para llamadas que necesitan `Authorization` u otro header -- `headers` es cualquier `{name: String, value: String}[]` que declares vos, sin ningún tipo builtin de por medio. La respuesta es el body como `String`; un status que no sea 2xx se vuelve un error de runtime normal, no un panic. Cuando importa el status code o los headers de la respuesta (ej. reintentar solo en 429), `http.getWithStatus(url, headers)` / `http.postWithStatus(url, body, headers)` devuelven `{status: Int, headers: {name: String, value: String}[], body: String}` -- mismo principio de tipo estructural, un 4xx/5xx es un dato, no un error
- Paginación real: `db.<c>.page(limit, offset)` empuja `LIMIT`/`OFFSET` al SQL de verdad (SQLite y Postgres, los dos) en vez de traer la tabla entera y cortarla en memoria -- mismo orden que `.all()`, así que las páginas nunca se solapan ni se saltean una fila. `db.<c>.pageAfter(cursor, limit)` es una alternativa por cursor para scroll infinito/paginación secuencial -- el cursor es el último `id` visto (`null` para la primera página), estable ante inserciones concurrentes a diferencia de `OFFSET`, que cuenta filas desde el principio en cada llamada
- Agregación real: `db.<c>.sumBy(selectorDeGrupo, selectorDeValor)` / `countBy(selectorDeGrupo)` / `avgBy` / `maxBy` / `minBy` empujan un `GROUP BY` al SQL de verdad -- MRR por plan, conteos por estado -- en vez de traer cada fila a memoria. Los selectores tienen que ser un acceso de campo simple (`|o: Order| { o.planId }`); agrupar es por `String`/`Int`/`Int64`/`Bool`/`enum` (sin truncado de fechas todavía), el campo agregado tiene que ser `Int`/`Int64`/`Float` -- `Int64` sigue siendo `Int64` en el resultado, nunca se degrada a `Int` en silencio. Agrupar por un campo `enum` devuelve el enum real como key, no un string
- Límite de requests por cliente: `@rate_limit("20/1m")` acota un rpc a N requests por ventana de tiempo, por `(ip del cliente, servicio, rpc)`, con 429 al exceder — token bucket con refill continuo
- Mandar email: `smtp.send(to, subject, body)` — la conexión (`LINK_SMTP_URL`) y el remitente (`LINK_SMTP_FROM`) salen del entorno del proceso, nunca de argumentos del rpc. TLS con rustls puro, mismo stack que el driver de PostgreSQL. `smtp.sendToMany(to: String[], subject, body)` manda un solo mensaje con un `RCPT TO` por destinatario; `smtp.sendHtml(to: String[], subject, html)` manda un body HTML (`Content-Type: text/html`) a uno o varios destinatarios -- `send` en sí queda sin cambios
- CORS configurable y headers de seguridad fijos: `--cors-origin <origen>` (repetible, o `LINK_CORS_ORIGINS`) pasa del `*` abierto a un allowlist real (match exacto, ecoado literal + `Vary: Origin`); toda respuesta -- incluidos errores y un `stream` SSE -- lleva `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`
- `linkc fmt`, `linkc --help`, y el emisor de cliente TypeScript para archivos multi-service funcionan correctamente ahora
- Hashing de contraseñas real: `crypto.hashPassword` es Argon2id (RFC 9106) con sal aleatoria por contraseña, en formato PHC; `verifyPassword` compara en tiempo constante y sigue aceptando los hashes de la versión anterior para no dejar afuera a los usuarios ya registrados
- Aleatoriedad numérica y comparación en tiempo constante para código de usuario: `crypto.randomInt(min, max)` da un `Int` uniforme en ese rango inclusive desde el CSPRNG del sistema (con rechazo de muestreo contra el sesgo de módulo) — alcanza para un OTP de verdad, a diferencia del alfabeto hex de `randomToken`; `crypto.timingSafeEqual(a, b)` expone la misma comparación en tiempo constante que `verifyPassword` ya usaba internamente, para comparar un secreto de webhook o una API key sin filtrar nada por el tiempo de respuesta
- `.toString()` sobre `Int`/`Int64`/`Float`/`Bool` — conversión explícita, nunca automática (mismo principio que `toInt64()`); `Bool` no tenía ni un solo método antes de esto. `response.setStatus` dentro de un `stream` ahora es error de compilación en vez de no-op silencioso. `@route` soporta un segmento catch-all final (`:nombre*`) que captura cero o más segmentos restantes del path unidos por `/`, para rutas de profundidad variable (documentación, un CMS) — siempre `String`, nunca `Int`, y siempre el último segmento de la ruta
- Costo de hashing de contraseñas configurable: `linkc serve --argon2-memory-kib <N> --argon2-iterations <N>` (o `LINK_ARGON2_MEMORY_KIB`/`LINK_ARGON2_ITERATIONS`) sube el costo de Argon2id de `crypto.hashPassword` por encima del default de la crate; sin configurar, el comportamiento no cambia. `crypto.isLegacyHash(hash: String) -> Bool` le dice a quien llama si un hash guardado es el formato legado pre-Argon2id, para re-hashear de forma proactiva en el login en vez de mirar el prefijo a mano. Una tabla de PostgreSQL con un id autoincremental preexistente de 32 o 16 bits (`SERIAL`/`IDENTITY`, no solo `BIGSERIAL`) ya no falla en el primer insert — conectar ya lo aceptaba, leer la columna del id ahora también
- Contrato TypeScript, cliente tipado, validadores runtime, hooks de React, schemas Zod y OpenAPI 3.1 generados

**Todavía no funciona** — no planifiques sobre esto:

| Límite | Detalle |
|---|---|
| `@rate_limit` es por proceso, en memoria | Sin persistencia entre reinicios, sin coordinación entre réplicas si el mismo `.link` corre en más de un proceso; la IP del cliente sale de la conexión TCP real, nunca de `X-Forwarded-For` (sin config de proxy de confianza todavía, así que detrás de un proxy esto limita por la IP del proxy). |
| `request.rawBody()` necesita un body JSON | El parseo de argumentos corre antes que cualquier rpc, sin importar cuántos parámetros declare, así que un body que no sea JSON (form-encoded, XML) nunca llega al rpc — un payload de webhook en JSON con campos de más funciona bien. |
| Sin CSP ni HSTS | CSP depende del contenido real de cada página (sin eso, no hay default seguro posible); HSTS solo tiene sentido sobre una conexión que YA es HTTPS, y `linkc serve` nunca lo es por sí mismo -- las dos le corresponden al reverse proxy que termina TLS delante. Las entradas del allowlist de CORS son match exacto únicamente, sin wildcards de subdominio. |
| `smtp` sin adjuntos, cc/bcc, ni envío asíncrono | `smtp.send`/`sendToMany`/`sendHtml` cubren texto plano y HTML a uno o varios destinatarios (desde esta ronda), pero ninguno de los tres acepta un adjunto ni una lista cc/bcc, y los tres son sincrónicos -- un relay lento hace lento a TODO el servidor (single-threaded) mientras dura esa request. |
| `--session-ttl` limpia de forma perezosa | Una sesión vencida se borra de memoria recién la próxima vez que se usa su token -- una creada y nunca vuelta a usar queda en memoria hasta que el proceso reinicia. |
| La estructura completa de usuario no se auto-carga en sesión | `auth.currentRole()` y `auth.currentUserId()` exponen el rol autenticado y el id numérico del usuario, pero cargar el struct `User` completo en memoria se hace explícitamente vía `db.users.find(uid)`. |
| La agregación (`sumBy`/etc.) no tiene truncado de fechas | No se puede agrupar por una fecha truncada (cohortes mensuales, por ejemplo) -- agrupar por un campo `Timestamp` desnudo no se acepta, y no hay ningún método de truncado para acotarlo antes. Soporte de `Int64` ya está -- ver Funciona hoy. |
| El push de `stream` entre instancias (LISTEN/NOTIFY) tiene límites reales | Una fila cambiada de más de 8000 bytes (el límite de payload de NOTIFY que impone el propio Postgres) no se propaga a otras instancias -- sigue publicándose local donde se escribió. NOTIFY es best-effort, sin cola de reintento; un servidor inactivo puede tardar hasta 200ms en notar un cambio remoto; cada instancia abre una conexión extra a Postgres solo para LISTEN; SQLite no participa en absoluto. |
| Sin paquete npm | `link-lang` todavía no está en el registro de npm. Los releases de GitHub sí funcionan — ver Instalación más abajo. |
| `linkc wasm` | Congelado a propósito en funciones escalares de enteros/booleanos; el camino de producción es `wasm32-wasip1`. |
| El playground web compila un solo archivo | Corre el lexer/parser/checker/generadores reales (compilados a `wasm32-unknown-unknown`), pero sin pasar por el cargador de módulos: sin `import` entre archivos, y sin ejecutar `test` (eso necesita el intérprete nativo). |

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

> **Todavía no está publicado.** `link-lang` no está en el registro de npm. Usá alguno de
> los instaladores de arriba (bajan el binario real precompilado desde
> [GitHub Releases](https://github.com/charlessonamericantrading/c-script-/releases)), o
> compilá desde el código:

```bash
git clone https://github.com/charlessonamericantrading/c-script-.git
cd c-script-/compiler
cargo build --release        # target/release/linkc
```

---

## 🤖 Diseñado para Cursor y Agentes de IA (Grok, Claude, GPT)

Link trae las reglas del lenguaje en el formato que lee cada herramienta, y **cada ejemplo
de esas reglas lo compila el binario real en cada corrida de CI** (`compiler/tests/docs_examples.rs`)
— que es lo que de verdad baja las alucinaciones: que lo que el agente lee sea lo que el
compilador acepta, no una promesa.

- **[`AGENTS.md`](AGENTS.md)**: lo que leen primero Claude Code y Codex — mapa del repo, comandos reales, convenciones del proyecto, y la lista de lo que está roto a sabiendas para que un agente no lo reporte como hallazgo nuevo.
- **[`llms.txt`](llms.txt) y [`llms-full.txt`](llms-full.txt)**: la referencia condensada del lenguaje, con los errores de sintaxis que comete todo LLM (las variantes de enum necesitan llaves como valor, los closures no llevan tipo de retorno, un `T?` no se puede desreferenciar).
- **`CLAUDE.md`, `.cursorrules`, `.cursor/rules/c-script.mdc`, `.windsurfrules`, `.github/copilot-instructions.md`**: las mismas reglas en el formato de cada herramienta — `CLAUDE.md` es lo que Claude Code carga solo al abrir el repo; es un puntero chico a `AGENTS.md`, no un duplicado.
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

[`playground/index.html`](playground/index.html) corre el lexer, parser, checker de tipos y
generadores REALES de `linkc` en tu navegador -- compilados a `wasm32-unknown-unknown` vía el
crate [`playground-wasm`](compiler/playground-wasm), no una demo enlatada. Compila un solo
archivo (sin `import` entre archivos) y no ejecuta `test` (eso necesita el intérprete nativo --
para eso, `linkc test` local). Para probarlo:

```bash
cd playground && python3 -m http.server 8000   # cualquier servidor estático sirve
# abrí http://localhost:8000/ -- abrir index.html directo por file:// NO funciona,
# los navegadores bloquean fetch() de archivos locales, y el módulo wasm carga vía fetch()
```

Para regenerar `playground/pkg/` después de tocar el compilador:

```bash
cd compiler/playground-wasm
cargo build --target wasm32-unknown-unknown --release
wasm-bindgen --target web --out-dir ../../playground/pkg --out-name playground_wasm \
  target/wasm32-unknown-unknown/release/playground_wasm.wasm
```

---

## 🧪 Pruebas y Control de Calidad

El compilador y el runtime de Link están verificados por **861 pruebas automáticas** unitarias,
de integración y de CLI, incluidas pruebas que levantan el binario real como subproceso, manejan
un servidor HTTP real, y compilan cada ejemplo de c-script publicado en la documentación de este repo:

```bash
cd compiler
cargo test
```

---

## 📄 Licencia

Licencia MIT — Copyright (c) 2026 Charlesson UK Consulting Group LTD. Ver [LICENSE](LICENSE).
