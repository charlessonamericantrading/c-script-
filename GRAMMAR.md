# Especificación Formal: Gramática y Sistema de Tipos de **c-script**

> Complementa a [`PLAN.md`](./PLAN.md). Aquí se define, con precisión de implementación: la gramática léxica y sintáctica (EBNF), las reglas del type checker (bidireccional), la tabla de mapeo exhaustiva c-script→TypeScript, y la semántica de nullability y errores.
>
> Notación EBNF (estilo ISO/Wirth): `,` secuencia · `|` alternativa · `[x]` opcional (0 o 1) · `{x}` repetición (0 o más) · `"texto"` terminal literal.

---

## Índice

> Este archivo es la especificación completa (~190 KB): la gramática EBNF, las reglas
> del checker bidireccional, el mapeo exhaustivo a TypeScript, y — sección por sección —
> el **límite honesto** de cada feature (qué quedó adentro, qué quedó afuera y por qué).
>
> Si lo que necesitás es escribir c-script y no entender cómo está construido, empezá por
> [`llms.txt`](llms.txt): la referencia condensada, con los errores de sintaxis que un LLM
> comete siempre. Volvé acá para el detalle de una feature puntual.

- [1. Gramática Léxica](#1-gramática-léxica)
- [2. Gramática Sintáctica](#2-gramática-sintáctica)
  - [2.1 Programa e ítems de nivel superior](#21-programa-e-ítems-de-nivel-superior)
  - [2.2 Expresiones de tipo — y la trampa del postfix](#22-expresiones-de-tipo--y-la-trampa-del-postfix)
  - [2.3 Expresiones, sentencias y patrones (cuerpo de un `rpc`)](#23-expresiones-sentencias-y-patrones-cuerpo-de-un-rpc)
- [3. Sistema de Tipos](#3-sistema-de-tipos)
  - [3.1 Juicios bidireccionales](#31-juicios-bidireccionales)
  - [3.2 Subtipado: estructural para `type`, nominal para `enum`](#32-subtipado-estructural-para-type-nominal-para-enum)
  - [3.3 Exhaustividad en `match` — RESUELTO (enum + literales, or-patterns, guardas)](#33-exhaustividad-en-match--resuelto-enum--literales-or-patterns-guardas)
  - [3.4 Nullability (`T?`) — RESUELTO (default aplicado)](#34-nullability-t--resuelto-default-aplicado)
  - [3.5 Manejo de errores en `rpc` — RESUELTO (default aplicado)](#35-manejo-de-errores-en-rpc--resuelto-default-aplicado)
  - [3.6 Genéricos definidos por el usuario — RESUELTO (monomorfización)](#36-genéricos-definidos-por-el-usuario--resuelto-monomorfización)
  - [3.7 Operadores e `if/else`](#37-operadores-e-ifelse)
  - [3.8 Métodos builtin](#38-métodos-builtin)
  - [3.9 Uniones de tipo (`A | B`) — RESUELTO (subtipado de flujo de valor Y narrowing)](#39-uniones-de-tipo-a--b--resuelto-subtipado-de-flujo-de-valor-y-narrowing)
  - [3.10 Funciones como valores — RESUELTO (referencias Y closures reales, `.map`/`.filter`)](#310-funciones-como-valores--resuelto-referencias-y-closures-reales-mapfilter)
  - [3.11 Validadores runtime (`validators.ts`) — RESUELTO](#311-validadores-runtime-validatorsts--resuelto)
  - [3.12 "DB tipada" v0 (`db { ... }`) — RESUELTO](#312-db-tipada-v0-db-----resuelto)
  - [3.13 Streaming real (SSE) para `stream` — RESUELTO, alcance `List<T>`](#313-streaming-real-sse-para-stream--resuelto-alcance-listt)
  - [3.14 Auth v0 (sesión opaca en memoria + roles) — RESUELTO](#314-auth-v0-sesión-opaca-en-memoria--roles--resuelto)
  - [3.15 Constructo de loop: `while` — RESUELTO, alcance acotado](#315-constructo-de-loop-while--resuelto-alcance-acotado)
  - [3.16 Push real: pub-sub sobre `db` para `stream` — RESUELTO, alcance acotado (shape fijo)](#316-push-real-pub-sub-sobre-db-para-stream--resuelto-alcance-acotado-shape-fijo)
  - [3.17 Persistencia real: `db` sobre SQLite — RESUELTO](#317-persistencia-real-db-sobre-sqlite--resuelto)
  - [3.18 CRUD real: `delete`/`deleteWhere`/`findWhere` sobre `db` — RESUELTO](#318-crud-real-deletedeletewherefindwhere-sobre-db--resuelto)
  - [3.19 Protocolo LSP real (`linkc lsp`) — RESUELTO, Nivel 1+2](#319-protocolo-lsp-real-linkc-lsp--resuelto-nivel-12)
  - [3.20 Codegen WASM nativo v0 (`linkc wasm`) — RESUELTO, alcance mínimo](#320-codegen-wasm-nativo-v0-linkc-wasm--resuelto-alcance-mínimo)
  - [3.21 LSP Nivel 3 (Ronda 1/3): goto-definición de un nombre de tipo en una firma — RESUELTO](#321-lsp-nivel-3-ronda-13-goto-definición-de-un-nombre-de-tipo-en-una-firma--resuelto)
  - [3.22 Identidad de archivo en `Span` — RESUELTO](#322-identidad-de-archivo-en-span--resuelto)
  - [3.23 `Field`/`Param` ganan `name_span` — RESUELTO](#323-fieldparam-ganan-name_span--resuelto)
  - [3.24 Hover de expresión arbitraria — RESUELTO, LSP Nivel 3 ronda 2/3](#324-hover-de-expresión-arbitraria--resuelto-lsp-nivel-3-ronda-23)
  - [3.25 Completion sensible al tipo del receptor — RESUELTO, LSP Nivel 3 ronda 3/3 (último ítem)](#325-completion-sensible-al-tipo-del-receptor--resuelto-lsp-nivel-3-ronda-33-último-ítem)
  - [3.26 Observabilidad: tracing estructurado por RPC — RESUELTO, v0](#326-observabilidad-tracing-estructurado-por-rpc--resuelto-v0)
  - [3.27 Hot reload real en `linkc dev` — RESUELTO, v0](#327-hot-reload-real-en-linkc-dev--resuelto-v0)
  - [3.28 Fase 3 (PLAN.md §4): política de estabilidad de sintaxis, y por qué source maps NO se persigue todavía](#328-fase-3-planmd-4-política-de-estabilidad-de-sintaxis-y-por-qué-source-maps-no-se-persigue-todavía)
  - [3.29 `linkc test`: contrato contra un snapshot commiteado (PLAN.md §5, "tests de contrato")](#329-linkc-test-contrato-contra-un-snapshot-commiteado-planmd-5-tests-de-contrato)
  - [3.30 `Int64` — RESUELTO, cierra la única fila "no" de tipos que quedaba en PLAN.md §2.3](#330-int64--resuelto-cierra-la-única-fila-no-de-tipos-que-quedaba-en-planmd-23)
  - [3.31 `Timestamp` — RESUELTO, alcance acotado a propósito](#331-timestamp--resuelto-alcance-acotado-a-propósito)
  - [3.32 Función builtin `now() -> Timestamp` — RESUELTO](#332-función-builtin-now---timestamp--resuelto)
  - [3.33 Test runner de comportamiento integrado (`test "nombre" { ... }`, `assert`, `panic`) — RESUELTO](#333-test-runner-de-comportamiento-integrado-test-nombre----assert-panic--resuelto)
  - [3.34 `crypto`: contraseñas y aleatoriedad — RESUELTO (Argon2id + CSPRNG del SO)](#334-crypto-contraseñas-y-aleatoriedad--resuelto-argon2id--csprng-del-so)
  - [3.35 `@content_type`: respuestas que no son JSON — RESUELTO (alcance acotado)](#335-content_type-respuestas-que-no-son-json--resuelto-alcance-acotado)
  - [3.36 PostgreSQL en runtime — RESUELTO (alcance acotado)](#336-postgresql-en-runtime--resuelto-alcance-acotado)
  - [3.37 `@route("/blog/:slug")`: URLs amigables para SEO — RESUELTO (alcance acotado)](#337-routeblogslug-urls-amigables-para-seo--resuelto-alcance-acotado)
  - [3.38 `env`, `request` y `crypto.hmacSha256`: verificar webhooks de terceros — RESUELTO (alcance acotado)](#338-env-request-y-cryptohmacsha256-verificar-webhooks-de-terceros--resuelto-alcance-acotado)
  - [3.39 `@rate_limit("20/1m")`: límite de requests por cliente — RESUELTO (alcance acotado)](#339-rate_limit201m-límite-de-requests-por-cliente--resuelto-alcance-acotado)
  - [3.40 PostgreSQL: TLS y reconexión automática — RESUELTO (alcance acotado)](#340-postgresql-tls-y-reconexión-automática--resuelto-alcance-acotado)
  - [3.41 CORS configurable y headers de seguridad — RESUELTO (alcance acotado)](#341-cors-configurable-y-headers-de-seguridad--resuelto-alcance-acotado)
  - [3.42 `@route` con múltiples parámetros — RESUELTO (alcance acotado)](#342-route-con-múltiples-parámetros--resuelto-alcance-acotado)
  - [3.43 `smtp.send`: mandar email — RESUELTO (alcance acotado)](#343-smtpsend-mandar-email--resuelto-alcance-acotado)
  - [3.44 PostgreSQL LISTEN/NOTIFY: `stream` entre varias instancias — RESUELTO (alcance acotado)](#344-postgresql-listennotify-stream-entre-varias-instancias--resuelto-alcance-acotado)
  - [3.45 `String.escapeHtml()`: sanitizar datos en una página — RESUELTO (alcance acotado)](#345-stringescapehtml-sanitizar-datos-en-una-página--resuelto-alcance-acotado)
  - [3.46 `response.setStatus(code)`: página 404 propia para un `@route` — RESUELTO](#346-responsesetstatuscode-página-404-propia-para-un-route--resuelto)
  - [3.47 `http.getWithHeaders`/`http.postWithHeaders`: headers en llamadas salientes — RESUELTO](#347-httpgetwithheadershttppostwithheaders-headers-en-llamadas-salientes--resuelto)
  - [3.48 `db.<coleccion>.page(limit, offset)`: paginación real, empujada a SQL — RESUELTO](#348-dbcoleccionpagelimit-offset-paginación-real-empujada-a-sql--resuelto)
  - [3.49 `@requires(Role.Admin | Role.Agent)`: OR de roles — RESUELTO](#349-requiresroleadmin--roleagent-or-de-roles--resuelto)
  - [3.50 `--session-ttl`: expiración real de sesión — RESUELTO](#350---session-ttl-expiración-real-de-sesión--resuelto)
  - [3.51 `auth.currentRole()`: leer el rol del caller dentro de un cuerpo — RESUELTO](#351-authcurrentrole-leer-el-rol-del-caller-dentro-de-un-cuerpo--resuelto)
  - [3.52 `sumBy`/`countBy`/`avgBy`/`maxBy`/`minBy`: agregación con `GROUP BY` real, empujada a SQL — RESUELTO](#352-sumbycountbyavgbymaxbyminby-agregación-con-group-by-real-empujada-a-sql--resuelto)
  - [3.53 `auth.createSessionWithId()` y `auth.currentUserId()`: asociar e inspeccionar el id del caller — RESUELTO](#353-authcreatesessionwithid-y-authcurrentuserid-asociar-e-inspeccionar-el-id-del-caller--resuelto)
  - [3.54 `crypto.randomInt()` y `crypto.timingSafeEqual()`: aleatoriedad numérica y comparación segura para código de usuario — RESUELTO](#354-cryptorandomint-y-cryptotimingsafeequal-aleatoriedad-numérica-y-comparación-segura-para-código-de-usuario--resuelto)
  - [3.55 `.toString()` sobre `Int`/`Int64`/`Float`/`Bool` — RESUELTO](#355-tostring-sobre-intint64floatbool--resuelto)
  - [3.56 `response.setStatus` dentro de un `stream` — RESUELTO (ahora error de compilación)](#356-responsesetstatus-dentro-de-un-stream--resuelto-ahora-error-de-compilación)
  - [3.57 `@route` con segmento catch-all (`:nombre*`) — RESUELTO](#357-route-con-segmento-catch-all-nombre--resuelto)
  - [3.58 `crypto`: costo de Argon2id configurable y señal de hash legado — RESUELTO](#358-crypto-costo-de-argon2id-configurable-y-señal-de-hash-legado--resuelto)
  - [3.59 PostgreSQL: acepta PK autoincremental de 32/16 bits, no solo `BIGSERIAL` — RESUELTO](#359-postgresql-acepta-pk-autoincremental-de-3216-bits-no-solo-bigserial--resuelto)
  - [3.60 `http.getWithStatus`/`http.postWithStatus`: código de estado y headers de la respuesta — RESUELTO](#360-httpgetwithstatushttppostwithstatus-código-de-estado-y-headers-de-la-respuesta--resuelto)
  - [3.61 `db.<c>.pageAfter(cursor, limit)`: cursor de continuación — RESUELTO](#361-dbcpageaftercursor-limit-cursor-de-continuación--resuelto)
  - [3.62 `@route` con parámetros de query string — RESUELTO](#362-route-con-parámetros-de-query-string--resuelto)
  - [3.63 `smtp.sendToMany()`/`smtp.sendHtml()`: varios destinatarios y cuerpo HTML — RESUELTO](#363-smtpsendtomanysmtpsendhtml-varios-destinatarios-y-cuerpo-html--resuelto)
  - [3.64 Auth externo: confiar en un JWT ya emitido — RESUELTO, alcance acotado (HS256)](#364-auth-externo-confiar-en-un-jwt-ya-emitido--resuelto-alcance-acotado-hs256)
  - [3.65 Agregación (`sumBy`/etc.): soporte de `Int64` — RESUELTO (fecha truncada sigue pendiente)](#365-agregación-sumbyetc-soporte-de-int64--resuelto-fecha-truncada-sigue-pendiente)
  - [3.66 `linkc introspect`: generar un `.link` desde una base PostgreSQL existente — RESUELTO, alcance acotado](#366-linkc-introspect-generar-un-link-desde-una-base-postgresql-existente--resuelto-alcance-acotado)
  - [3.67 `--adopt-existing`: adoptar tablas sin auto-migrar — RESUELTO](#367---adopt-existing-adoptar-tablas-sin-auto-migrar--resuelto)
  - [3.68 NULL en una columna requerida tras una migración de PostgreSQL: error limpio, no `null` silencioso — RESUELTO](#368-null-en-una-columna-requerida-tras-una-migración-de-postgresql-error-limpio-no-null-silencioso--resuelto)
  - [3.69 Narrowing real de `T?`: `match`, `??` y `.isSome()`/`.isNone()` — RESUELTO](#369-narrowing-real-de-t-match--y-issome-isnone--resuelto)
  - [3.70 Tipo nativo `Uuid` — RESUELTO](#370-tipo-nativo-uuid--resuelto)
- [4. Tabla de Mapeo c-script → TypeScript (exhaustiva)](#4-tabla-de-mapeo-c-script--typescript-exhaustiva)
  - [4.1 Qué puede aparecer en la firma de un `rpc`](#41-qué-puede-aparecer-en-la-firma-de-un-rpc)
  - [4.2 Validación en los dos extremos](#42-validación-en-los-dos-extremos)
- [5. Estado](#5-estado)

---

## 1. Gramática Léxica

```ebnf
digit        = "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" ;
letter       = "a".."z" | "A".."Z" | "_" ;
identifier   = letter , { letter | digit } ;

int_lit      = digit , { digit } ;
float_lit    = digit , { digit } , "." , digit , { digit } ;
string_lit   = '"' , { string_char } , '"' ;
string_char  = ? cualquier carácter excepto '"' o '\' ? | escape_seq ;
escape_seq   = "\" , ( "n" | "t" | "\" | '"' | "u" , hex4 ) ;
bool_lit     = "true" | "false" ;

line_comment  = "//" , { ? cualquier carácter excepto newline ? } ;
block_comment = "/*" , { ? cualquier carácter ? } , "*/" ;

keyword      = "type" | "enum" | "service" | "rpc" | "stream" | "match"
             | "import" | "from" | "pub" | "const" | "fn" | "let" | "mut"
             | "return" | "if" | "else" | "while" | "true" | "false" | "null"
             | "test" ;
```

**Reservado pero fuera del v0 de la gramática:** `async`, `await`, `trait`, `impl` — el modelo de concurrencia y de polimorfismo ad-hoc se diseña en una iteración posterior (ver PLAN.md §4, Fase 1). `for`, `in`, `break`, `continue` — v0 de loops (§3.15) es solo `while`; ninguno de estos cuatro es todavía una palabra reservada de verdad (no aparecen en `keyword_from_str`, `compiler/src/token.rs`), esto es prosa preparatoria, no una reserva real.

---

## 2. Gramática Sintáctica

### 2.1 Programa e ítems de nivel superior

```ebnf
program      = { item } ;
item         = import_decl | type_decl | enum_decl | service_decl | const_decl | fn_decl | db_decl | test_decl ;

import_decl  = "import" , "{" , ident_list , "}" , "from" , string_lit , ";" ;
ident_list   = identifier , { "," , identifier } ;

type_decl    = "type" , identifier , [ type_params ] , "=" , type_expr , [ ";" ] ;
type_params  = "<" , identifier , { "," , identifier } , ">" ;

enum_decl    = "enum" , identifier , [ type_params ] , "{" , variant_list , "}" ;
variant_list = variant , { "," , variant } , [ "," ] ;
variant      = identifier , [ "{" , field_list , "}" ] ;

field_list   = field , { "," , field } , [ "," ] ;
field        = identifier , [ "?" ] , ":" , type_expr ;

const_decl   = "const" , identifier , ":" , type_expr , "=" , expr , ";" ;

service_decl = "service" , identifier , "{" , { member_decl } , "}" ;
member_decl  = { annotation } , ( rpc_decl | stream_decl ) ;
(* Varias por rpc/stream se permiten -- lo que el checker rechaza es dos DE
   LA MISMA DIMENSIÓN (dos de auth, dos @content_type, dos @route, dos
   @rate_limit), no la combinación entre dimensiones distintas: §3.14
   (auth), §3.35 (@content_type), §3.37 (@route), §3.39 (@rate_limit). *)
annotation   = "@authenticated"
             | "@requires" , "(" , identifier , "." , identifier , ")"
             | "@content_type" , "(" , string_lit , ")"
             | "@route" , "(" , string_lit , ")"
             | "@rate_limit" , "(" , string_lit , ")" ;
rpc_decl     = "rpc" , identifier , "(" , [ param_list ] , ")" , "->" , type_expr , block ;
stream_decl  = "stream" , identifier , "(" , [ param_list ] , ")" , "->" , type_expr , block ;
param_list   = param , { "," , param } ;
param        = identifier , ":" , type_expr , [ "=" , expr ] ;

fn_decl      = "fn" , identifier , "(" , [ param_list ] , ")" , "->" , type_expr , block ;

test_decl    = "test" , ( string_lit | identifier ) , block ;

db_decl      = "db" , "{" , field_list , "}" ;   (* "db" NO es keyword -- ver §3.12 *)
```

**El `;` de `type_decl` es opcional.** Un `type X = { ... }` termina en `}`; exigir además un `;` es la misma incomodidad que Rust/Go evitan después de un `struct`. `const_decl`/`let_stmt`/`return_stmt` sí exigen `;` — su valor no siempre termina en `}` (`const MAX: Int = 100`), así que hace falta una marca explícita de fin de sentencia.

**`fn` — funciones libres, no expuestas como RPC.** A diferencia de `rpc`/`stream`, no vive dentro de un `service` y no entra al contrato `.d.ts` — es lógica interna del backend (p. ej. `validate` llamada desde un `rpc`). Misma forma que `rpc_decl` porque comparten `param_list`/`block`; la diferencia es de visibilidad, no de sintaxis.

**`import_decl` — RESUELTO (multi-archivo + package manager mínimo, `compiler/src/modules.rs`).** `import { X, Y } from "./otro.link";` ya resuelve de verdad: cada `.link` alcanzado se lexea/parsea, y sus ítems (menos los `Item::Import` ya resueltos) se funden en un solo `Program` antes de llegar al checker — que sigue viendo un único árbol, sin ningún concepto nuevo de "archivo".

```
type Point = { x: Int, y: Int }        // b.link

import { Point } from "./b.link";      // a.link
fn origin() -> Point { Point { x: 0, y: 0 } }
```

- **`from` relativo (`./`/`../`) vs. nombre pelado.** Un `from` que empieza con `./` o `../` es una ruta relativa al archivo que importa. Un nombre pelado (`import { X } from "shapes";`) se busca en `dependencies` de un `link.json` en el directorio del archivo de entrada — la raíz del proyecto, sin buscar hacia arriba en el árbol (eso es útil para monorepos, un caso más avanzado que v0 no necesita). **Gotcha real** (PLAN.md §9.1, confirmado en un reporte de adopción -- "tropezamos con este aviso sin entender qué lo causaba"): la raíz del proyecto SIEMPRE es el directorio del archivo que se le pasa a `linkc build`/`serve`/etc. en la línea de comandos, nunca configurable aparte. Copiar o apuntar a un `.link` FUERA de la carpeta real de su proyecto (ej. probar un archivo suelto en otro lado) hace que cualquier import bare-name o `./` relativo que dependía de encontrar `link.json`/otro archivo en esa carpeta falle con `error de módulos: no se pudo resolver '<ruta-absoluta>': ...` -- el mensaje nombra el archivo que no pudo encontrar, no dice explícitamente "estás compilando fuera de tu proyecto", así que si el error sorprende, la causa casi siempre es esta.
  ```json
  { "dependencies": {
    "shapes": "./libs/shapes.link",
    "auth-lib": "git+https://github.com/usuario/auth-link.git#v1.2.0"
  } }
  ```
  **Dependencias `git+<url>#<rev>` — RESUELTO (auditoría post-push, `compiler/src/gitdep.rs`).** El valor de una dependencia en `link.json` puede ser, además de una ruta local, una URL git real con el prefijo `git+` -- `resolve_import_target` (`modules.rs`) detecta el prefijo y delega en `gitdep::resolve`, que clona/actualiza un caché local (`<raíz-del-proyecto>/.linkc/cache/<hash-de-la-url>/`, `hash_source` reusado de `link.lock` en vez de sumar una segunda función de hashing) invocando el binario `git` real vía subproceso -- sin ningún cliente git en Rust, misma filosofía que `rusqlite` con SQLite (§3.17). El punto de entrada DENTRO del checkout es `main.link` en la raíz, por convención (el mismo nombre que `linkc new` ya scaffoldea) -- no configurable en esta v0.

  **`#<rev>` es OBLIGATORIO, a propósito.** Sin un registro que ordene versiones (no hay ninguno, PLAN.md §8.3 lo descarta a propósito), "la última versión" no tiene un significado bien definido -- resolver contra la rama default de cada remoto sería una fuente de builds NO reproducibles desde el día 1, exactamente el problema que un package manager existe para resolver, no para reintroducir. `<rev>` acepta un tag, una rama, o ya un commit SHA -- `git checkout --detach <rev>` los trata igual.

  **Resolución: siempre fresca, nunca cacheada más allá de lo que el clon local ya tiene.** Si el rev pedido ya resuelve contra el clon existente (un tag/commit ya conocido de una resolución anterior), no hay ningún acceso de red -- si no, un `git fetch --all --tags` sobre el clon ya cacheado alcanza (no un re-clone). Un rev que es una RAMA (no un tag/commit fijo) se re-resuelve contra su HEAD real en cada build -- si la rama avanzó, el build lo sigue; para un pin duro e inmutable, usar un tag o un commit SHA directamente en `link.json`.

  **`link.lock` graba el commit resuelto -- informativo en v0, no un pin que se imponga por sí solo.** Un nuevo campo, `git_dependencies` (`{"nombre":{"url":...,"rev":...,"resolved":"<sha-completo>"}}`), registra exactamente qué commit se usó la última vez que se corrió `linkc build` -- útil para auditar qué versión real terminó en un build dado, sobre todo cuando `rev` es un tag/rama que puede moverse. A diferencia de un `Cargo.lock`/`package-lock.json` real, esto NO se lee para decidir qué commit usar en el PRÓXIMO build (que siempre re-resuelve `rev` fresco, ver arriba) -- es un registro de auditoría, no una fuente de verdad que compita con `link.json`. Convertirlo en un pin real (leer `resolved` si está presente y `rev` no cambió, en vez de re-resolver) es la extensión natural, no incluida en esta ronda.

  **Sin locking entre procesos concurrentes** -- dos `linkc build` corriendo a la vez sobre el mismo proyecto podrían pisarse el mismo clon cacheado. Límite de v0 conocido, no manejado (`Cargo` tampoco lo manejó bien en sus primeras versiones).

  **`link.lock` para archivos LOCALES -- RESUELTO, pero sigue sin ser un lockfile de versiones.** Con una dependencia por RUTA local no hay versión ni conflicto que "lockear" en el sentido de Cargo/npm — ese razonamiento original sigue valiendo para ESE caso. Lo que se agregó primero (`compiler/src/lockfile.rs`) es más angosto: `linkc build` calcula un hash SHA-256 de cada archivo `.link` tocado (`touched`, el mismo `Vec<PathBuf>` que ya devuelve `load_program`) y lo escribe en `link.lock` (JSON, `{"version":1,"entries":{"ruta":{"path":...,"hash":...}},"git_dependencies":{...}}`); en el PRÓXIMO `build`, si ya existe un `link.lock`, se compara antes de sobreescribirlo y cualquier archivo cuyo hash no matchea imprime una advertencia — detección de deriva entre builds para archivos locales, resolución+registro real (no un pin, ver arriba) para dependencias git. Rutas siempre relativas a la raíz del proyecto (nunca el `\\?\C:\...` que `fs::canonicalize` da en Windows) para que el archivo sea legible y portable entre máquinas -- el mismo problema de prefijo apareció de nuevo al pasarle una ruta de caché a `git clone` como argumento (git no lo entiende como argumento de línea de comandos, "Invalid argument"; `display_path`, la función que ya pelaba esto para texto legible, resultó ser exactamente la función correcta acá también, por una razón distinta y más dura que la estética original).

  Verificado con subprocesos reales: `gitdep::resolve` contra un repo git LOCAL como "remoto" (clon inicial, reutilización de caché sin red, fetch de un tag agregado después del clon inicial, checkout de un commit SHA directo) y `linkc build` de punta a punta (clona, resuelve el import, tipa, genera el contrato, y graba el commit real en `link.lock`). 371 tests, todos pasando.

  **Cobertura agregada en un reparso posterior: los caminos de FALLA, no solo el feliz.** Hasta entonces, ni `gitdep.rs` ni el test a nivel CLI probaban qué pasa cuando algo sale mal -- un rev que no existe en el remoto, un remoto inalcanzable. Dos tests nuevos en `gitdep.rs` confirman que `resolve` falla ruidoso (`Err` con un mensaje real) en ambos casos, contra el mismo `FixtureRemote` local que ya usan los tests del camino feliz. Un tercer test, a nivel `linkc build` completo, cierra la capa que un test unitario de `gitdep.rs` solo no puede cubrir: el cableado real entre `resolve()` fallando y lo que el BINARIO hace con eso (`modules.rs` envolviendo el error, `main.rs` decidiendo el exit code y si escribe `link.lock`) -- confirma que un rev inexistente tumba `linkc build` con código de salida distinto de cero y sin dejar un `link.lock` a medio escribir, no algo que un test unitario aislado garantice por sí solo si esa cadena llegara a romperse. 380 tests, todos pasando.
- **Ciclos se rechazan con un error claro** (no un stack overflow silencioso ni un colgado): se detectan sobre la pila de imports que se está resolviendo en ese momento, no sobre "todo lo que ya se vio alguna vez" (eso rompería el caso diamante de abajo).
- **Sin re-exports, a propósito.** Un import se valida contra los ítems NATIVOS del archivo importado — nunca contra su cierre ya fusionado con SUS PROPIOS imports. Si A importa `X` de B, y B a su vez importa `X` de C (pero no declara `X` él mismo), el import de A **falla**: B nunca declaró `X` nativamente, así que no hay nada que A pueda "heredar" a través de B. Si hiciera falta lo contrario, hay que importar `X` directamente de C.
- **Namespaces cruzados.** `types`/`enums`/`fns`/`const`s son namespaces independientes (el checker los guarda en tablas separadas) — un import busca el nombre en los cuatro y alcanza con que matchee en uno; error solo si no matchea en ninguno. `service` queda afuera: no es algo que se referencie por nombre en ningún otro lado del lenguaje, así que "importar un service" no tiene un significado real todavía.
- **Sin visibilidad real (`pub`/privado).** El `Program` final que llega al checker es la unión plana de los ítems nativos de todo archivo alcanzado transitivamente — el import valida "¿existe ese nombre en ESE archivo puntual?" pero no oculta nada de los demás archivos del cierre entre sí (dos archivos no relacionados por ningún import, pero alcanzados por el mismo cierre transitivo, pueden verse los símbolos entre sí sin querer). Implementar visibilidad real necesitaría un scoping por archivo en el checker, que hoy no tiene ningún concepto de "de qué archivo vino este símbolo" — una extensión más grande, correctamente fuera de alcance acá.
- **Detección de colisiones, de paso.** Al construir esto se encontró que dos `type`/`enum`/`fn` con el mismo nombre en el mismo `Program` (antes, solo pasaba dentro de un único archivo; con imports, entre archivos) ganaban por orden de inserción, en silencio — `checker.rs::build_symbols` ahora rechaza el duplicado explícitamente.

### 2.2 Expresiones de tipo — y la trampa del postfix

```ebnf
type_expr     = union_type ;
union_type    = postfix_type , { "|" , postfix_type } ;
postfix_type  = primary_type , { type_postfix_op } ;
type_postfix_op = "?" | "[" "]" ;

primary_type  = identifier , [ type_args ]
              | "{" , field_list , "}"                     (* struct inline *)
              | "{" , type_expr , ":" , type_expr , "}"     (* map: {K: V} *)
              | "(" , type_expr , ")"                       (* agrupación *)
              | "(" , type_expr , "," , [ type_list ] , ")"  (* tupla, requiere ≥1 coma *)
              | "(" , [ type_list ] , ")" , "->" , type_expr (* tipo función *)
              ;
type_args     = "<" , type_expr , { "," , type_expr } , ">" ;
type_list     = type_expr , { "," , type_expr } ;
```

**El parser v0 no implementa la forma literal `{ type_expr : type_expr }` para mapas.** Es una ambigüedad real, no un detalle de implementación: `{ id: Int }` es sintácticamente idéntico tanto para "struct de un campo sin coma final" como para "map de `id` (tipo) a `Int`" — nada en la gramática los distingue sin recurrir a qué identificadores son "tipos conocidos", que no es información disponible en tiempo de parseo. Se resuelve con `Map<K, V>` (named type genérico ordinario, sin gramática especial) hasta que se justifique una sintaxis dedicada.

`★ Insight ─────────────────────────────────────`
Dos decisiones sutiles del diseño de gramática, no obvias hasta que las rompes:

1. **El orden de `?` y `[]` importa y por eso `postfix_type` es una lista, no dos campos fijos.** `T[]?` se parsea como `primary=T`, luego postfix `[]`, luego postfix `?` → `Optional(List(T))` ("array que puede ser null"). `T?[]` se parsea al revés → `List(Optional(T))` ("array de elementos que pueden ser null"). Son tipos completamente distintos y ambos son legítimos — la gramática tiene que permitir encadenarlos en cualquier orden, no fijar uno.
2. **`(A)` vs `(A, B)` es la misma ambigüedad clásica de Python con tuplas de un elemento.** Por eso exijo `,` obligatoria en `type_list` de la producción de tupla: `(A)` es agrupación pura, `(A,)` sería la tupla de un elemento (si algún día hace falta). Sin esta regla, `(Int)` sería ambiguo entre "el tipo Int entre paréntesis" y "una tupla de un Int".
`─────────────────────────────────────────────────`

### 2.3 Expresiones, sentencias y patrones (cuerpo de un `rpc`)

```ebnf
block        = "{" , { stmt } , [ expr ] , "}" ;
stmt         = let_stmt | assign_stmt | expr_stmt | return_stmt | while_stmt ;
let_stmt     = "let" , [ "mut" ] , identifier , [ ":" , type_expr ] , "=" , expr , ";" ;
assign_stmt  = identifier , "=" , expr , ";" ;
return_stmt  = "return" , [ expr ] , ";" ;
expr_stmt    = expr , ";" ;
while_stmt   = "while" , or_expr , block ;          (* nunca produce un valor -- ver §3.15 *)

expr         = match_expr | if_expr | or_expr ;

if_expr      = "if" , or_expr , block , "else" , ( if_expr | block ) ;

(* Precedence climbing estándar, de menor a mayor precedencia. Cada nivel
   solo delega al siguiente si no encuentra su propio operador — así `&&`
   liga más fuerte que `||`, `+` más fuerte que comparación, etc. *)
or_expr           = and_expr , { "||" , and_expr } ;
and_expr          = equality_expr , { "&&" , equality_expr } ;
equality_expr     = relational_expr , { ( "==" | "!=" ) , relational_expr } ;
relational_expr   = additive_expr , { ( "<" | "<=" | ">" | ">=" ) , additive_expr } ;
additive_expr     = multiplicative_expr , { ( "+" | "-" ) , multiplicative_expr } ;
multiplicative_expr = unary_expr , { ( "*" | "/" | "%" ) , unary_expr } ;
unary_expr        = ( "!" | "-" ) , unary_expr | postfix_expr ;

postfix_expr = primary_expr , { postfix_op } ;
postfix_op   = "." , identifier                   (* acceso a campo / método: db.users *)
             | "." , int_lit                       (* acceso posicional a tupla: t.0 *)
             | "(" , [ arg_list ] , ")"            (* llamada: f(x), o encadenada: db.users.find(id) *)
             | "[" , expr , "]" ;                  (* indexado: arr[i] *)
arg_list     = expr , { "," , expr } ;

primary_expr = struct_or_variant_lit
             | array_lit
             | tuple_lit
             | closure_lit
             | identifier
             | int_lit | float_lit | string_lit | bool_lit | "null"
             | "(" , expr , ")" ;

(* Closure (§3.10). El cuerpo es SIEMPRE un block -- no hay "block como
   expresión" en el lenguaje, así que esto lo reusa tal cual. Mínimo 1
   parámetro: "||" lexea como un solo token (or lógico), no como dos "|".
   El tipo de un parámetro se parsea como postfix_type, NO type_expr: un
   "|" de nivel superior pertenece al cierre del closure, así que un tipo
   unión necesita paréntesis (|x: (Int | String)| { ... }). *)
closure_lit       = "|" , closure_param , { "," , closure_param } , [ "," ] , "|" , block ;
closure_param     = identifier , [ ":" , postfix_type ] ;

struct_or_variant_lit = identifier , [ "." , identifier ] , "{" , [ field_init_list ] , "}" ;
field_init_list        = field_init , { "," , field_init } ;
field_init             = identifier , ":" , expr ;

array_lit = "[" , [ expr , { "," , expr } , [ "," ] ] , "]" ;

(* misma ambigüedad y misma solución que en tipos (§2.2): (a) es agrupación,
   (a,) tupla de 1, (a,b) tupla de 2+ -- requiere ≥1 coma para NO ser Paren. *)
tuple_lit = "(" , expr , "," , [ expr , { "," , expr } ] , ")" ;

match_expr   = "match" , expr , "{" , { match_arm } , "}" ;
(* La coma SEPARA un arm-expr del siguiente, así que es opcional en el
   último (justo antes del "}"), igual que en Rust. Un arm cuyo cuerpo es
   un block nunca la lleva. *)
match_arm    = pattern , [ "if" , expr ] , "=>" , ( expr , [ "," ] | block ) ;

pattern      = pattern_atom , { "|" , pattern_atom } ;        (* or-pattern *)
pattern_atom = identifier                                    (* binding, incl. "_" *)
             | identifier , "." , identifier ,
               [ "{" , field_pattern_list , "}" ]             (* Enum.Variant { .. } *)
             | type_pattern
             | literal_pattern ;
(* Narrowing de una unión a su miembro concreto (§3.9). El tipo se parsea
   como postfix_type, no type_expr: un "|" que siga pertenece al or-pattern
   que lo rodea (i: Int | s: String son DOS alternativas, no un tipo unión). *)
type_pattern = identifier , ":" , postfix_type ;
literal_pattern = int_lit | "-" , int_lit | str_lit | "true" | "false" ;
field_pattern_list = field_pattern , { "," , field_pattern } ;
field_pattern       = identifier , [ ":" , pattern ] ;        (* shorthand: `x` ≡ `x: x` *)
```

**`[]` vacío solo en modo chequeo.** Sin elementos no hay de dónde sintetizar el tipo — `[]` únicamente es válido donde el contexto ya da un tipo esperado `T[]` (ej. `let xs: Int[] = [];`), igual que `Result.Ok`/`Result.Err` (§3.5). Un array no vacío sí sintetiza: se infiere del primer elemento y se chequea que el resto coincida.

**Indexar fuera de rango es un error de runtime, no `null`.** `arr[i]` con `i` fuera de rango falla en tiempo de ejecución en vez de devolver un valor nulo silencioso — la alternativa (devolver `T?` siempre, incluso cuando `T` no es nullable) ensuciaría el tipo de CADA acceso a un array por un caso excepcional. Es la misma decisión que Rust (panic) y distinta de la de JS (`undefined`).

**`t.0.1` no encadena — limitación conocida del lexer, no un error silencioso.** El lexer decide si `0.1` es un solo `float_lit` o dos `int_lit` separados por un `.` mirando únicamente los caracteres, sin saber que venía de un acceso posicional a tupla — así que `t.0.1` se lexea como `Ident("t")`, `Dot`, `Float(0.1)`, no como dos accesos encadenados. Rust tiene el mismo problema de fondo y lo resuelve con una regla especial en su lexer; acá, mientras tanto, la forma de acceder a una tupla anidada es `let inner = t.0; inner.1;`.

**Nota de implementación — lookahead de `struct_or_variant_lit`:** distinguir `Result.Ok { value: u }` (literal de variante) de `db.users` (acceso encadenado, sin `{` después) requiere que el parser mire hasta 2 tokens adelante antes de decidir. No es una ambigüedad del lenguaje — es la misma clase de decisión que "no struct literals en la condición de un `if`" en Rust: una regla del parser, no del árbol de derivación.

**`if` siempre exige `else`.** Es una expresión total: si `if` pudiera faltar el `else`, ¿qué tipo tendría la rama ausente? Rust resuelve esto dándole tipo `()` al `if` sin `else` y exigiendo que solo se use donde `()` es válido; acá se simplifica exigiendo `else` siempre. Un condicional de solo-efecto se escribe `if cond { ... } else { }` explícito.

**Mutabilidad — por qué `let mut` no alcanza sin `assign_stmt`.** Antes de `assign_stmt`, `mut` era una palabra reservada sin ningún efecto: se podía escribir `let mut x = 1`, pero no había ninguna sentencia que permitiera cambiar `x` después. El checker exige que el nombre a la izquierda de un `assign_stmt` ya exista en el scope **y** haya sido declarado con `mut` — asignar a un binding inmutable, o a un nombre que no existe, es un error de tipos (checker.rs), no algo que el parser rechace. `assign_stmt` solo cubre variables simples (`x = ...`) — todavía no hay mutación de campos (`obj.campo = ...`) ni de posiciones de array (`arr[i] = ...`).

Or-patterns, patrones de literales y guardas ya están resueltos — ver §3.3 para el algoritmo de exhaustividad extendido y sus límites de alcance explícitos (en particular, ninguna alternativa de un `p1 | p2` puede introducir bindings).

---

## 3. Sistema de Tipos

### 3.1 Juicios bidireccionales

Dos juicios, como en Rust/TS/Swift modernos:

- `Γ ⊢ e ⇒ T` — **síntesis**: a partir de `e`, se infiere `T`.
- `Γ ⊢ e ⇐ T` — **chequeo**: se verifica que `e` es válido contra un `T` ya conocido.

La regla que conecta ambos mundos:

```
Γ ⊢ e ⇒ T'      T' <: T
─────────────────────────  (Subsunción)
Γ ⊢ e ⇐ T
```

Reglas clave:

```
─────────────────────────  (Lit-Int)
Γ ⊢ n ⇒ Int

─────────────────────────  (Lit-Str)
Γ ⊢ "s" ⇒ String

x : T ∈ Γ
─────────────────────────  (Var)
Γ ⊢ x ⇒ T

f : (T1, .., Tn) -> T ∈ Γ      Γ ⊢ eᵢ ⇐ Tᵢ  (para cada i)
────────────────────────────────────────────────────────  (Call)
Γ ⊢ f(e1, .., en) ⇒ T

Γ ⊢ e ⇐ T
─────────────────────────  (Struct-Lit, modo chequeo — necesita T objetivo
Γ ⊢ Nombre{...} ⇐ T          para saber qué campos son válidos)
```

**¿Por qué bidireccional y no solo inferencia (Hindley-Milner)?** Porque `rpc` declara su tipo de retorno explícitamente (`-> User`), lo cual da un "ancla" de tipo esperado en cada punto de entrada. Eso simplifica enormemente el checker: no hace falta unificación global, con propagar el tipo esperado hacia abajo (chequeo) y sintetizar hacia arriba en las hojas (literales, variables) alcanza. Es el mismo enfoque que TypeScript usa internamente para inferencia contextual.

### 3.2 Subtipado: estructural para `type`, nominal para `enum`

```
∀ (k: Tₖ) ∈ campos(T')  ∃ (k: Sₖ) ∈ campos(S)     Sₖ <: Tₖ
─────────────────────────────────────────────────────────  (Struct-Width-Depth)
S <: T'
```

- **`type` es estructural** (como TS): si `S` tiene al menos los campos de `T'` con tipos compatibles, `S <: T'`. Esto es necesario para que el mapeo a TS sea 1:1 — TS *solo* entiende structural typing para tipos objeto.
- **`enum` es nominal**: dos enums con las mismas variantes pero nombres distintos NO son intercambiables. Esto también refleja TS: una unión discriminada se distingue por el tipo declarado, no por su forma accidental.

### 3.3 Exhaustividad en `match` — RESUELTO (enum + literales, or-patterns, guardas)

Algoritmo base sobre un scrutinee **enum** (incluye `Result<T,E>` y enums genéricos instanciados):

```
cubierto := ∅
para cada arm SIN guard en match:      -- un arm CON guard nunca cuenta, ver más abajo
  si arm.pattern == "_" (o un bind con nombre):
    cubierto := todas_las_variantes(EnumType)
  si no:
    cubierto := cubierto ∪ variantes_de(arm.pattern)   -- un Or aporta la unión de sus alternativas

error si cubierto ≠ todas_las_variantes(EnumType)
```

Esto es lo que hace que el compilador de c-script, igual que Rust, **rechace un `match` que no cubre un nuevo variant** añadido a un enum. Es una propiedad valiosa por sí misma (no solo para el puente con TS): añadir un caso a `Result` rompe la compilación en *todos* los `match` que lo consumen, en el backend, no solo en el frontend.

**Extensión: `match` también acepta un scrutinee `Int`/`String`/`Bool`** (antes, `match` exigía un enum a secas — matchear un primitivo directamente no tenía ninguna forma de patrón que no fuera un bind, así que era, en los hechos, imposible de usar con más de un arm real). El algoritmo para este caso es distinto porque `Int`/`String` tienen un espacio de valores no enumerable:

```
error si NO hay un catch-all (bind sin guard) entre los arms
   Y  NO ( tipo == Bool  Y  'true' y 'false' están ambos cubiertos por un literal sin guard )
```

`Bool` es, en los hechos, un enum de dos variantes — es el único tipo no-enum donde un conjunto de literales, sin catch-all, alcanza para ser exhaustivo. `Int`/`String` **siempre** necesitan un arm final sin guard (`_ => ...` o un bind con nombre) — ningún conjunto finito de literales agota sus valores posibles.

```
fn describe(n: Int) -> String {
  match n {
    1 | 2 => "bajo",     // or-pattern: aporta {1, 2} a la cobertura
    -1    => "negativo", // literal negativo: un solo token de patrón, no unario aplicado a un patrón
    _     => "otro",     // catch-all obligatorio -- Int no es enumerable
  }
}
```

**Guardas (`pattern if cond => body`) nunca descartan exhaustividad por sí solas.** La condición podría ser `false` en runtime, así que un arm con guard —aunque su patrón sería, sin el guard, un catch-all o cubriría el último variant que faltaba— **no cuenta** para el algoritmo de arriba: sigue habiendo que cubrir ese caso con algún otro arm sin guard. En runtime, si el patrón matchea pero el guard da `false`, la búsqueda **continúa con el siguiente arm** (igual que Rust), no se trata como "sin match":

```
fn classify(n: Int) -> String {
  match n {
    x if x > 100 => "grande",
    x if x > 0   => "positivo chico",
    _            => "cero o negativo",
  }
}
```

El guard ve las variables que el propio patrón acaba de ligar — `Setting.Level { value } if value > 10 => ...` puede usar `value` en la condición — y debe sintetizar `Bool`, como cualquier condición (§3.7).

**Or-patterns (`p1 | p2 | ...`) — alcance v0: ninguna alternativa puede introducir bindings.** La regla completa de otros lenguajes (cada alternativa debe ligar exactamente las mismas variables, con el mismo tipo) es la parte cara de implementar or-patterns; acá se evita ese problema entero prohibiendo bindear del todo dentro de un `Or` — cubre el caso común (combinar variantes unitarias o literales que comparten cuerpo) sin esa complejidad:

```
enum Status { Active, Paused, Cancelled }
match s {
  Status.Active | Status.Paused => "en curso",   // ok: ninguna alternativa liga nada
  Status.Cancelled => "cancelado",
}

enum Shape { Circle { r: Int }, Square { r: Int } }
match sh {
  Shape.Circle { r } | Shape.Square { r } => r,  // ERROR: cada alternativa intenta ligar 'r'
}
```

**Deliberadamente fuera de alcance: literales `Float`, y matchear un `T?` directamente.** Sin patrón `Float`: comparar floats por igualdad exacta es una trampa conocida (`0.1 + 0.2 != 0.3`) — Rust llegó a la misma conclusión y terminó prohibiéndolo en sus propios patrones (antes era solo un warning). Sin `null` como patrón: eso requeriría que `match` acepte un scrutinee `Optional(T)`, una extensión relacionada pero distinta que queda para más adelante — hoy la forma de testear nullability sigue siendo `== null` / `!= null` dentro de un `if/else` (§3.7), que ya funciona porque `Null <: Optional(_)` (§3.4) hace que la comparación tipe.

### 3.4 Nullability (`T?`) — RESUELTO (default aplicado)

Regla de subtipado (se deriva de "opcional es más permisivo"):

```
S <: T
─────────────  (Optional-Widen)
S <: T?
```

**Decisión:** el default recomendado en `PLAN.md` §8.3, aplicado sin pasar por el TODO del usuario (ver `examples/decision-nullability.ts` para el resultado).

| Sintaxis c-script | Significado | TypeScript | Wire (JSON) |
|---|---|---|---|
| `x: T` | requerido, nunca ausente ni null | `x: T` | clave siempre presente |
| `x: T?` | la clave siempre está; el **valor** puede ser null | `x: T \| null` | clave presente, valor `null` |
| `x?: T` | la **clave** puede no existir | `x?: T` | clave omitida si ausente |
| `x?: T?` | ambos a la vez | `x?: T \| null` | combinación de las dos anteriores |

**PATCH parcial — `Patch<T>`:** utilitario análogo a `Partial<T>` de TS. Vuelve **todos** los campos de `T` del tipo `?:` (clave omitible), preservando si además eran `T?` (nullable). Esto resuelve exactamente la pregunta que planteaba `decision-nullability.ts` ("¿cómo distingo *no lo toques* de *bórralo*?"):

- Campo **no nullable** en la base (`x: T`) → en el patch, `x?: T`: omitido = no tocar, presente = fijar. No se puede limpiar (nunca fue nullable).
- Campo **nullable** en la base (`x: T?`) → en el patch, `x?: T | null`: omitido = no tocar, `null` = limpiar, valor = fijar.
- Campo **opcional-al-crear** (`x?: T`) → en el patch, sigue `x?: T`: omitido = no tocar, presente = fijar. Si además necesitás poder limpiarlo, la base tiene que declararse `T?`, no solo `?:` — la distinción tiene consecuencias reales, no es solo estilo.

```typescript
// rpc update(id: Int, patch: Patch<User>) -> User
type PatchUser = {
  name?: string;         // bio no nullable en la base -> solo fijar u omitir
  bio?: string | null;   // si bio fuera T? en la base -> se puede limpiar con null
};
declare function updateUser(id: number, patch: PatchUser): Promise<User>;

updateUser(42, { name: "Ada" }); // no toca bio ni deletedAt
```

Esta convención sigue el mismo principio que **JSON Merge Patch (RFC 7386)** y el patrón habitual de inputs nullable en GraphQL — no es una invención ad-hoc, es la solución estándar a este problema exacto.

### 3.5 Manejo de errores en `rpc` — RESUELTO (default aplicado)

**Decisión:** `Result<T, E>`, con `E` siempre un `enum` (así la exhaustividad de `match`, §3.3, aplica también a los errores). Razón: TypeScript no tipa lo que se lanza (`catch (e)` siempre es `unknown`), así que una excepción rompe la tesis central del proyecto justo en el peor lugar. `Result<T,E>` es coherente con el resto del lenguaje (ya hay `enum` + `match` exhaustivo — un error no es más que otro ADT) y es la única opción que preserva "rompe en compilación" para errores, no solo para el happy path (comparativa completa con la alternativa de excepciones tipadas: ver `examples/decision-errors.ts`).

```
enum ValidationError {
  InvalidEmail { field: String },
  TooShort     { field: String, min: Int },
}

enum ValidateResult {
  Ok  { value: NewUser },
  Err { error: ValidationError },
}

fn validate(input: NewUser) -> ValidateResult {
  ValidateResult.Ok { value: input }   // placeholder; reglas reales en checker/runtime
}

service Users {
  rpc create(input: NewUser) -> Result<User, ValidationError> {
    match validate(input) {
      ValidateResult.Ok  { value: v } => Result.Ok  { value: db.users.insert(v) },
      ValidateResult.Err { error: e } => Result.Err { error: e },
    }
  }
}
```

Nótese el patrón `ValidateResult.Ok { value: v }`, no `Ok(v)`: los variants de c-script se declaran con campos nombrados (§3.5 arriba), así que su patrón es struct-style (§2.3), no posicional al estilo Rust `Some(x)`. Es una consecuencia directa de la gramática de patrones, no una elección nueva.

Mapeo a TS (reusa la regla general de enum-con-datos, §4):

```typescript
type ValidationError =
  | { type: "InvalidEmail"; field: string }
  | { type: "TooShort"; field: string; min: number };

type Result_User_ValidationError =
  | { type: "Ok"; value: User }
  | { type: "Err"; error: ValidationError };

declare function create(input: NewUser): Promise<Result_User_ValidationError>;
```

```typescript
const result = await usersClient.create(input);
if (result.type === "Ok") {
  console.log(result.value.id);
} else {
  switch (result.error.type) {           // exhaustivo, TS avisa si falta un caso
    case "InvalidEmail": /* ... */ break;
    case "TooShort":     /* ... */ break;
  }
}
```

**Errores de transporte vs errores de dominio:** el cliente generado **nunca** lanza (`throw`) para un error que el `rpc` declaró en su `Result<T,E>` — esos siempre vuelven como valor. El cliente **sí** puede lanzar `LinkTransportError` para fallos fuera del contrato de dominio (red caída, 5xx, timeout) — son excepcionales por definición, no algo que el backend predijo. Es la misma línea divisoria que separa `Result`/`Option` de `panic!` en Rust.

### 3.6 Genéricos definidos por el usuario — RESUELTO (monomorfización)

`type Box<T> = { value: T }` / `enum Option<T> { Some{value:T}, None }` ya funcionan: se instancian (`Box<Int>`), se construyen, se accede a sus campos, y se hace `match` exhaustivo sobre enums genéricos.

**Cómo se resuelve una instanciación.** `resolve_type` arma un *subst* (`type_param -> tipo concreto`, ej. `{"T": Int}`) y lo aplica recursivamente al cuerpo de la declaración — es monomorfización real, no type erasure: `Box<Int>` y `Box<String>` son dos tipos concretos distintos para el checker, tal como se recomendaba acá antes de implementarlo. La instanciación queda **opaca** (`Type::Generic(nombre, args)`, sin expandir) hasta que hace falta la forma real — field access, construcción, match — el mismo patrón que ya usaban `Result<T,E>`/`Patch<T>`/`Map<K,V>`.

**Construcción: solo en modo chequeo, igual que `Result`.** `Box { value: 5 }` no trae los argumentos de tipo en su sintaxis (no hay `Box<Int> { value: 5 }`) — así que, igual que `Result.Ok`, necesita un tipo esperado ya instanciado viniendo del contexto (anotación de `let`, tipo de retorno declarado, etc.). Sintetizar `Box { value: 5 }` sin ese contexto es un error explícito: no hay de dónde sacar el argumento de tipo.

**Decisión: la comparación de un genérico ya instanciado es NOMINAL, no estructural.** `Box<Int>` y un struct suelto `{ value: Int }` con la misma forma **no** son intercambiables, aunque `type` sin genéricos sí es estructural (§3.2). Es una simplificación deliberada: sostener estructural-a-través-de-un-genérico exigiría que `is_subtype` pudiera "ver a través" de una instanciación opaca, lo cual necesita acceso a las tablas de símbolos que hoy no tiene (es una función libre, sin ese contexto) — y en la práctica varios lenguajes con tipado estructural (la propia TypeScript incluida, en varios casos con genéricos) tampoco garantizan esa equivalencia en general.

**La declaración se emite como genérico real de TypeScript, no monomorfizada.** A diferencia del checker (que sí monomorfiza internamente), el `.d.ts` emite `export interface Box<T> { value: T; }` **una sola vez** — TypeScript ya tiene genéricos nativos, así que no hace falta (ni conviene) generar una interface por cada instanciación usada. Una referencia a `Box<Int>` en una firma se emite como `Box<number>`, dejando que el propio `tsc` haga la instanciación.

### 3.7 Operadores e `if/else`

Sin coerción implícita — a diferencia de JS, `1 + "1"` es un error de tipos, no `"11"`. Cada operador exige que ambos operandos ya tengan el tipo correcto (vía la regla de Subsunción de §3.1); si hace falta convertir, es explícito (`.toFloat()`/`.toInt()`, §3.8), nunca automático.

| Operador | Regla | Resultado |
|---|---|---|
| `+` | ambos `Int`, ambos `Float`, o ambos `String` (concatenación) | mismo tipo que los operandos |
| `- * /` `%` | ambos operandos `Int`, o ambos `Float` (no mezclados) | mismo tipo que los operandos |
| `- ` unario | operando `Int` o `Float` | mismo tipo |
| `== !=` | operandos de tipos mutuamente compatibles (mismo primitivo, o mismo enum nominal) | `Bool` |
| `< <= > >=` | ambos operandos `Int`, o ambos `Float` | `Bool` |
| `&& \|\| !` | operando(s) `Bool` | `Bool` |

`if cond { A } else { B }` es de **modo chequeo**, igual que `match` (§3.1): no tiene un tipo propio que sintetizar, necesita el tipo esperado del contexto para verificar que `cond ⇐ Bool` y que tanto `A` como `B` chequean contra ese mismo tipo esperado. Es la misma familia de regla que `match` — control de flujo condicional siempre se chequea top-down, nunca se infiere bottom-up.

### 3.8 Métodos builtin

`x.metodo()` sobre un valor que no es un struct/enum declarado no es acceso a un campo real — es azúcar reconocida por nombre y tipo del receptor, resuelta ANTES de intentar el `FieldAccess` genérico (que fallaría: `Int`/`Float`/`String`/`List` no son `Struct` ni `Dynamic`). Es el mismo mecanismo que ya resolvía `db.users.find(...)` (`checker.rs`/`runtime/mod.rs`, `BoundMethod`), generalizado.

| Método | Receptor | Resultado | Nota |
|---|---|---|---|
| `.toFloat()` | `Int` | `Float` | conversión exacta |
| `.toInt()` | `Float` | `Int` | trunca hacia cero (`3.9`→`3`, `-3.9`→`-3`), igual que `as` en Rust — no redondea |
| `.length()` | `String` | `Int` | cantidad de caracteres |
| `.contains(s: String)` | `String` | `Bool` | substring, no regex |
| `.take(n: Int)` | `T[]` | `T[]` | los primeros `n`; si la lista tiene menos, la devuelve entera (no falla) |
| `.filter(p: (T) -> Bool)` | `T[]` | `T[]` | ver §3.10 |
| `.map(f: (T) -> U)` | `T[]` | `U[]` | ver §3.10 |
| `.length()` | `T[]` | `Int` | cantidad de elementos -- faltaba (solo existía para `String`) hasta que `login` (§3.14) necesitó "¿matcheó algún usuario?" |
| `.createSession(role: R)` | `auth` | `String` | ver §3.14 -- `R` debe ser un enum declarado |
| `.createSessionWithId(role: R, userId: Int)` | `auth` | `String` | ver §3.53 -- asocia el rol y el id numérico del usuario |
| `.destroySession()` | `auth` | `Void` | ver §3.14 -- sin argumentos, opera sobre la sesión de la request actual |
| `.currentRole()` | `auth` | `String?` | ver §3.51 -- devuelve el nombre de la variante del rol autenticado (`null` si no hay sesión) |
| `.currentUserId()` | `auth` | `Int?` | ver §3.53 -- devuelve el `userId` asociado a la sesión (`null` si no hay sesión o se creó sin id) |

No hay coerción implícita en ningún operador (§3.7) — `.toFloat()`/`.toInt()` son las únicas conversiones numéricas, y son siempre explícitas. `.length()`/`.contains()` son método, no propiedad (`x.length`, sin paréntesis) — consistencia con `.toFloat()`/`.toInt()` importó más acá que imitar la convención de propiedad de JS/TS.

### 3.9 Uniones de tipo (`A | B`) — RESUELTO (subtipado de flujo de valor Y narrowing)

`x: Int | String` ya se resuelve, se acepta como tipo de parámetro/campo, y se emite como la unión nativa de TypeScript (`number | string`). La gramática (§2.2) ya traía `union_type` desde el principio; lo que faltaba era que el checker supiera qué hacer con un `TypeExpr::Union` en vez de devolver un error fijo.

**Regla de subtipado — dos direcciones, no una:**

```
S <: Tᵢ   para algún i ∈ 1..n
──────────────────────────────  (Union-Intro, "a la derecha")
S <: (T₁ | ... | Tₙ)

∀i ∈ 1..n.  Sᵢ <: T
──────────────────────────────  (Union-Elim, "a la izquierda")
(S₁ | ... | Sₙ) <: T
```

La primera es la que cubre el caso real más común: un valor concreto (`Int`) fluye hacia un parámetro/campo tipado como unión con solo encajar en UNO de los miembros. La segunda es la que hace que una unión sea, a su vez, subtipo de otra unión más ancha (`Int | String <: Int | String | Bool`) — cada miembro de la izquierda tiene que encajar en algo de la derecha.

```
type Event = { payload: Int | String }

fn accept(x: Int | String) -> Void {}

fn f() -> Void {
  accept(1);        // Int <: Int | String -- ok (Union-Intro)
  accept("hola");   // String <: Int | String -- ok (Union-Intro)
}
```

Emitido:

```typescript
export interface Event {
  payload: number | string;
}
```

**Por qué a veces aparece un paréntesis (`(A | B)[]`):** igual que `Optional`/tipo función dentro de un `List` (§2.2), un miembro que en TS se renderiza con `|` o `=>` en su nivel superior necesita paréntesis explícitos al aparecer dentro de otra construcción — `number | string[]` en TS significa `number | (string[])`, no `(number | string)[]`. El emisor ya aplicaba esta regla para `Optional`/`Function` (`render_type_atom`); ahora también protege a `Union`, en ambas direcciones: como elemento de un `List`, y como miembro de otra unión.

#### Narrowing: `match` con patrones `nombre: Tipo`

```
type Query = Int | String

fn findByIdOrEmail(query: Int | String) -> User[] {
  match query {
    id: Int => db.users.all().filter(|u: User| { u.id == id }),
    email: String => db.users.all().filter(|u: User| { u.email == email }),
  }
}
```

`nombre: Tipo` reusa el `:` que ya significa "nombre tiene este tipo declarado" en todos lados (`let`, params, campos de struct) -- sin inventar puntuación nueva (`is`/`as`). Nuevo `Pattern::Type(String, TypeExpr)` en el AST, mismo orden nombre-primero que `Param`/`Field`/`FieldPattern`. El tipo se parsea con `parse_postfix_type` (NO `parse_type_expr`, que consumiría un `|` de nivel superior perteneciente al propio or-pattern que lo rodea) -- esa elección resuelve, sin lógica extra, tanto que un miembro `Optional<T>` sea narrowable (`u: User?`) como que `i: Int | s: String` funcione como or-pattern normal (el `|` queda para el loop de `parse_pattern`, no se lo come la anotación de tipo).

**Rechazo de ambigüedad en tiempo de COMPILACIÓN, no "primer match gana" en runtime.** Antes de mirar los arms siquiera, `check_exhaustive_union` rechaza una unión cuyos miembros no se puedan distinguir de forma demostrable -- es una propiedad de la unión en sí, no de cómo se la matchea. Un chequeo ingenuo de `is_subtype` mutuo entre cada par de miembros NO alcanza: `{x:Int,y:Int}` y `{x:Int,z:Int}` no son subtipo mutuo entre sí, pero un TERCER tipo más ancho (`{x:Int,y:Int,z:Int}`, construible por cualquier usuario vía subtipado estructural de ancho, GRAMMAR.md §3.2) satisface los campos requeridos de los DOS a la vez -- un valor de ese tercer tipo sería ambiguo para cualquier regla que solo mire nombres de campo. La condición real (`union_members_are_distinguishable`, checker.rs): dos miembros son distinguibles solo si existe al menos un campo REQUERIDO por ambos cuyos tipos declarados tengan discriminantes de `Value` mutuamente excluyentes (`Int` vs `String`, nunca los dos a la vez en el mismo valor real) -- si no comparten ningún campo así, incluyendo el caso de no compartir NINGÚN campo requerido, se rechazan como ambiguos (falla cerrado: si el análisis no puede probar que son distinguibles, es error, no "asumamos que está bien"). Siempre ambiguos, sin análisis fino: `Dynamic` emparejado con cualquier cosa; dos miembros `List(_)` (una lista vacía matchea cualquiera de los dos); dos miembros `Optional(_)` (`null` matchea ambos). Este chequeo corre SOLO dentro de un `match` -- el uso ya soportado de una unión como tipo de parámetro que solo acepta-y-pasa sin narrowear (`fn f(x: Int | String)`) sigue funcionando igual que siempre, sin verse afectado.

**El chequeo de runtime tiene que coincidir con el argumento de solidez, o el análisis de arriba no vale nada.** `value_matches_type` (runtime/mod.rs) no solo chequea que un campo requerido esté PRESENTE -- chequea recursivamente que el VALOR guardado ahí tenga el tipo declarado. Es la única forma de que "campo compartido con tipos en conflicto" sea una distinción confiable: dos valores `{x: 5}` y `{x: "hola"}` comparten el nombre de campo `x`, pero el runtime nunca los confunde porque mira el `Value` real (`Value::Int` vs `Value::Str`), no la forma estática de dónde vino ese valor. `try_match_pattern` necesitó, por primera vez en este módulo, resolver un `TypeExpr` a su forma real -- hasta esta ronda nada en runtime/mod.rs lo hacía, solo el checker.

**Corrección (encontrada en un reparso posterior, texto desactualizado desde entonces):** el párrafo de arriba, en su versión original, decía que esto se resolvía con una tabla `Symbols` propia del runtime, construida una sola vez en `invoke_rpc`. Esa tabla existió (commit `4513b96`) pero tenía un bug real: devolvía `Type::Dynamic` para `Generic`/`Tuple`/`Map`, así que una unión con un miembro `Box<Int>` tipaba en el checker pero JAMÁS podía matchear en runtime -- exactamente el tipo de inconsistencia checker-vs-runtime que esta sección entera existe para evitar. Un commit posterior (`49d227f`, "Auditoría: el borde de red ahora es tipado de verdad") la eliminó y reusa el `&Checker` real (`Checker::build_symbols`, construido una vez en `invoke_rpc_with_sessions`) en su lugar -- el resolvedor de tipos verdadero, no una segunda implementación ad-hoc que podía (y de hecho llegó a) divergir del primero. Ese commit nunca actualizó este párrafo; el comportamiento visible que describe (narrowing funciona, ambigüedad se rechaza) siguió siendo correcto todo este tiempo -- solo el detalle de implementación había quedado desactualizado, y en la dirección de "se reemplazó por algo mejor", no de una regresión.

**Fuera de alcance, a propósito:** narrowing fuera de `match` (sin operador `is`/`typeof` standalone, sin narrowing vía `if`). Una unión con miembros ambiguos según el análisis de arriba sigue sin poder matchearse -- error claro apuntando a la alternativa de siempre: modelar la alternancia como `enum` en vez de una unión estructural.

### 3.10 Funciones como valores — RESUELTO (referencias Y closures reales, `.map`/`.filter`)

Una `fn` de nivel superior, referenciada por su nombre sin llamarla ahí mismo, es un valor de primera clase: se puede pasar como argumento, guardar en una variable, o recibir a través de un parámetro tipado `(A) -> B`. `Expr::Ident` para un nombre que no resuelve a una variable local cae al conjunto de `fn`s declaradas y sintetiza `Type::Function(params, ret)` (checker.rs) / produce un `Value::FnRef(nombre)` en runtime (runtime/mod.rs) — nunca captura nada, porque una `fn` de nivel superior no tiene ningún scope léxico exterior que capturar.

```
fn add_one(x: Int) -> Int { x + 1 }
fn apply_twice(f: (Int) -> Int, x: Int) -> Int { f(f(x)) }

fn use_it() -> Int { apply_twice(add_one, 5) } // 7
```

**Subtipado de tipos función — contravariante en parámetros, covariante en el retorno** (regla estándar): una función que acepta MENOS de lo estrictamente necesario (parámetro declarado más angosto) o devuelve MÁS de lo prometido (retorno más ancho) sirve donde se espera la firma original.

```
S <: T          (para cada parámetro, EN SENTIDO INVERSO: T_param <: S_param)
S_ret <: T_ret  (el retorno, en el mismo sentido que todo lo demás)
──────────────────────────────────────────────────────────────────  (Function-Sub)
(S_params) -> S_ret  <:  (T_params) -> T_ret
```

Esa comparación de params vive en su propia función con nombre (`types::params_accept`), no repetida inline en cada lugar que la necesita -- ver más abajo por qué eso importó de verdad, no solo por prolijidad.

#### Closures: `|params| { block }`

```
list.filter(|u: User| { u.active })      // predicado -- siempre List<T>
list.map(|u: User| { u.name })           // transforma -- puede cambiar List<T> a List<U>

// captura 'total' del scope que lo rodea, y lo MUTA -- de ahí el `mut`
let mut total = 0;
let sumar = |x: Int| { total = total + x; x };
```

Estilo Rust, delimitado por `|`. El cuerpo es SIEMPRE un bloque con llaves -- nunca una expresión suelta (`|x| x + 1` no se soporta; hace falta `|x| { x + 1 }`) porque el lenguaje no tiene ningún concepto de "bloque como expresión general" y esto reutiliza `Block` tal cual en vez de inventarlo. Cada parámetro es `nombre (: tipo)?` -- la anotación es opcional cuando el closure se chequea (⇐) contra un `Type::Function` ya conocido (el callback de `.filter`/`.map`, o un `let` con el tipo declarado), y obligatoria cuando no hay ningún contexto del que inferirla (`synth_expr`, ej. `let f = |x| {...}` sin anotar el `let`).

**Dos límites de alcance reales, no arbitrarios:**
- **Closures de 0 parámetros no se soportan** (`||`). `||` lexea como un único token (`PipePipe`, distinto de `Pipe`), y ninguno de los dos consumidores nuevos (`.map`/`.filter`) necesita un closure sin parámetros -- no hay infraestructura sin un caso de uso real que la ejercite.
- **Un tipo unión en la anotación de un parámetro necesita paréntesis**: `|x: (Int | String)| { ... }`, no `|x: Int | String| { ... }`. La anotación se parsea con `parse_postfix_type` (no `parse_type_expr`, que consume `|` en loop para uniones y se comería el `|` de CIERRE del closure).

**Bug real de subtipado encontrado por un review de diseño antes de escribir código, no en producción:** al chequear un closure con un parámetro ANOTADO contra un `Type::Function` esperado, la dirección correcta es `is_subtype(esperado, anotación)` -- contravariante, igual que `Function-Sub` de arriba --, NUNCA `is_subtype(anotación, esperado)`. Al revés, un closure como `points.filter(|p: WidePoint| ...)` sobre una `List<NarrowPoint>` (donde `WidePoint` tiene MÁS campos que `NarrowPoint`) se aceptaría por error, y su cuerpo podría leer un campo que el dato real nunca tuvo -- crash en runtime, no error de compilación. La dirección correcta está aislada en `types::params_accept` (la misma función que usa `is_subtype`'s regla `Function`) precisamente para que no se pueda invertir por accidente una segunda vez.

**`.filter(pred)` y `.map(f)` -- por qué el checker los trata distinto.** `.filter` siempre devuelve `Bool`: el tipo esperado del callback (`(T) -> Bool`) se conoce ENTERO de entrada, así que se chequea (⇐) igual que cualquier otro argumento de tipo función. `.map` es distinto: el tipo de retorno del callback (`U`) es exactamente lo que no se sabe de entrada -- se SINTETIZA (`synth_callback_result`) en vez de chequearse contra algo fijo, ligando el parámetro del closure al tipo de elemento real de la lista y sintetizando el cuerpo. Ambos aceptan tanto un closure literal como una `fn` con nombre ya declarada (`xs.map(double)`) -- dos caminos de código distintos dentro de `synth_callback_result`, no un caso especial para cada forma.

**`return` dentro de un closure sin tipo de retorno conocido por contexto es un error, no una inconsistencia silenciosa.** `check_block` (la función que ya chequea cualquier bloque) usa el mismo `expected` tanto para la cola del bloque como para cualquier `Stmt::Return` anidado -- y hoy tiene un bug preexistente, real pero nunca ejercitado (`return` no se usaba en ningún `.link` ni test antes de esta ronda): un `if`/`match` en posición de sentencia (no cola) se chequea contra `Type::Void` sin importar el `expected` real del bloque que lo contiene, así que un `return` ahí adentro se compara contra `Void` en vez del retorno real. Ese bug queda **fuera de alcance de esta ronda** (es ortogonal, se documenta acá, no se arregla). Para no heredarlo de otra forma, la síntesis del cuerpo de un closure (`synth_block`, nueva) rechaza de entrada, con un error claro, cualquier `return` alcanzable desde el bloque que recorre -- incluso dentro de un `if`/`match` no-cola.

**Captura léxica real, no solo una referencia.** `Value::Closure` guarda el `Env` (`Rc<RefCell<Value>>` por variable) del momento en que se construyó -- clonar ese `Env` al llamar el closure clona los punteros `Rc`, no las celdas, así que una mutación posterior de una variable capturada (vía `Assign` en el scope exterior) SÍ es visible adentro del closure, y viceversa (mismo mecanismo que ya usan los bloques de `if` anidados).

**Hallazgo real, no pedido, encontrado por el mismo review: un closure recursivo arma un ciclo de `Rc`.** El patrón `let mut f: (Int)->Int = |x|{x}; f = |x|{ ... f(x-1) ... };` (necesario para escribir recursión desde un closure -- el lenguaje no tiene otra forma) tipa bien, y en runtime el segundo closure captura un `Env` que contiene la MISMA celda que `f` está a punto de sobreescribir: un ciclo real, no hipotético. Dos defensas, independientes y ambas baratas:
1. El checker rechaza `==`/`!=` cuando alguno de los dos operandos es (o contiene recursivamente, en un campo/elemento/miembro de unión) un tipo función -- comparar closures no tiene un significado útil de todos modos.
2. `Value` deja de derivar `PartialEq`/`Debug` -- se implementan a mano para que `Value::Closure` nunca recurse dentro de su `Env` capturado (nunca son iguales entre sí; su `Debug` solo imprime los nombres de parámetros). Defensa en profundidad para cualquier OTRO código (mensajes de error, tests) que compare/imprima un `Value` arbitrario sin saber que puede ser autorreferencial.

**Consecuencia real sobre el streaming (§3.13): `Value` dejó de ser `Send`.** `Value::Closure` guarda un `Env` con `Rc<RefCell<Value>>` -- ni `Rc` ni `RefCell` son `Send`/`Sync`, así que agregar este variant hizo que `Value` (y por lo tanto `Db`, que guarda `Vec<Value>`) dejaran de poder cruzar el borde de un hilo. El diseño original de streaming (Fase 2) corría `invoke_rpc` DENTRO del hilo spawneado para la conexión -- eso ya no compila. Arreglado moviendo `invoke_rpc` de vuelta al hilo PRINCIPAL (igual que cualquier `rpc` normal) y dejando que el hilo spawneado reciba solo el resultado YA CONVERTIDO a `serde_json::Value` (sin ningún `Rc` adentro, `Send` de sobra) -- el hilo aparte pasa a encargarse únicamente de la escritura de bytes SSE, que es lo único que de verdad necesitaba correr aparte (un cliente lento leyendo no debe bloquear al servidor de aceptar otras conexiones). Diseño más ajustado que el original, no solo un parche.

**Nada de esto cruza el wire.** Un valor de tipo función (`FnRef` o `Closure`) sigue siendo "solo campo de tipo local" en la tabla de mapeo (§4) -- `.map`/`.filter` y los closures solo existen DENTRO de cuerpos de `fn`/`rpc`, nunca como parte de un tipo declarado que el emisor tenga que traducir a TypeScript, así que `ts_emit.rs` no necesitó ningún cambio.

**Fuera de alcance, a propósito:** `.reduce()` y otros combinadores de orden superior, parámetros `mut` en un closure (ningún `fn`/`rpc` los tiene tampoco hoy), closures de 0 parámetros, y capturar por valor en vez de por referencia compartida.

### 3.11 Validadores runtime (`validators.ts`) — RESUELTO

Planeado desde el documento original (`PLAN.md` §3.1: *"[4b] Emisor de contrato → .d.ts + cliente TS + validadores"*, *"esto es lo que hace la seguridad real en el borde, no solo compile-time"*) pero nunca construido hasta ahora — `compiler/src/codegen/ts_emit.rs` solo emitía `.d.ts` y `client.ts`. `linkc build`/`linkc dev` ahora generan un tercer archivo, `validators.ts` (`compiler/src/codegen/validators_emit.rs`), y `client.ts` valida cada respuesta contra él antes de devolverla.

```typescript
async getById(id: number): Promise<User | null> {
  const res = await fetch(...);
  if (!res.ok) throw new LinkTransportError(`HTTP ${res.status}`);
  const json: unknown = await res.json();
  if (!(json === null || isUser(json))) throw new LinkValidationError("getById", json);
  return json as User | null;
}
```

**Generación por tipo concreto alcanzado, no por declaración con nombre.** Recorre las mismas firmas de rpc ya resueltas que usa `emit_client`, y genera una función `isX(x: unknown): x is X` por cada tipo con identidad propia (struct con nombre, enum, `Result<T,E>`, `Patch<T>`, instanciación de un genérico) que aparece en ellas — nunca para `Box<T>` abstracto (opaco, GRAMMAR.md §3.6), solo para instanciaciones concretas como `Box<Int>` que ya llegan resueltas. Un tipo estructural (Optional/List/Tuple/Union/Map/struct anónimo) no tiene función propia — se valida inline, igual que `render_type`/`render_type_atom` (ts_emit.rs) tratan esa misma división entre "tiene nombre" y "se renderiza en el lugar".

**Predicados a mano, no Zod/typia.** Consumir `validators.ts` no debería exigirle al usuario instalar nada — mismo criterio de cero dependencias nuevas que el resto del compilador (`tiny_http` + `serde_json` siguen siendo las únicas, y son del lado Rust, no del TS generado).

**`Patch<T>` tiene su propio validador, no delega en el de `T`.** Igual que `render_type` vuelve cada campo `?:` para `Patch<T>` (`Partial<T>`, §3.4), su validador vuelve cada campo `=== undefined || <chequeo>` — incluidos los que en `T` eran requeridos. Validar un patch contra el validador de `T` a secas rechazaría de forma incorrecta un patch parcial válido.

**Tercera categoría de error, `LinkValidationError`.** Ni un error de dominio declarado (`Result<T,E>`, siempre vuelve como valor) ni un fallo de transporte (`LinkTransportError`, red/5xx/timeout) — "el servidor respondió 200 pero el payload no matchea el contrato" es su propio modo de falla, con su propia clase, consistente con la línea divisoria que ya traza §3.5 entre las otras dos.

**Límite real: solo valida lo que efectivamente cruza el wire.** Un `type`/`enum` que ningún `rpc` usa como parámetro o retorno no genera validador — no hay ningún valor real en runtime que necesite chequear su forma. Si se agrega un `rpc` nuevo que lo referencia, el próximo `linkc build`/`linkc dev` lo agrega solo.

**Efecto secundario real: construir esto expuso un bug de serialización preexistente.** `Value::Variant` (runtime/mod.rs) siempre serializaba como `{ type: "..." }`, sin importar si el enum era simple (`Role`, todo unit) o un ADT (`ValidationError`) — nadie lo había notado porque nada construía un enum simple vía la sintaxis del lenguaje (`Role.Member {}`) antes de esta sesión; los datos sembrados a mano en `db.rs` usaban directamente un string de Rust, sin pasar por acá. `validators.ts` es justo lo bastante estricto como para haberlo atrapado apenas se ejercitó de punta a punta: `isRole` exige un string plano, no un objeto. Arreglado dándole a `Value::Variant` también el nombre del ENUM (no solo el de la variante), para que el runtime pueda replicar exactamente el mismo chequeo `all_unit` que ya usa `emit_enum_decl` (ts_emit.rs) al serializar — la variante ganadora no alcanza para decidirlo sola: un ADT puede tener una variante sin campos propios (ej. `enum Wrapped { Has{value:Int}, Empty }`) que igual debe serializar como `{type:"Empty"}`, no como un string suelto.

### 3.12 "DB tipada" v0 (`db { ... }`) — RESUELTO

`db` dejó de ser `Type::Dynamic` (cualquier `db.lo-que-sea.como-sea(...)` tipaba, y solo fallaba en runtime). Un nuevo ítem de nivel superior declara la forma real:

```
db {
  users: User[],
  posts: Post[],
}
```

**`db` no es palabra reservada.** Se reconoce por texto ("db" seguido de `{`) solo en posición de ítem de nivel superior — en cualquier otro lado (`let db = 5;`, un parámetro, un campo) sigue siendo un identificador común. De hecho, esto arregló un bug real: antes, el string mágico `"db"` se chequeaba ANTES del lookup de variables (tanto en el checker como en runtime/mod.rs), así que un `let db = ...` de un usuario quedaba sombreado en silencio por el builtin. Ahora el lookup de variables va primero.

**Cada colección necesita un campo `id: Int`.** No es un capricho — es lo que hace posible que `insert` pida `Omit<T, "id">` (los campos de T menos `id`, un utility type nativo de TS, sin sintaxis nueva) en vez de T completo. Sin esta regla, `insert` habría exigido el struct entero — y **habría roto el propio demo insignia**, donde la forma de creación (`NewUser`) es deliberadamente un subconjunto de `User` (sin `id`, `role`, `deletedAt`). El checker lo exige al procesar `db { ... }`, con un error claro si falta.

```
type User = { id: Int, name: String, email: String, role: Role, bio?: String, deletedAt: String? }
db { users: User[] }

// insert pide Omit<User, "id"> -- NO el User completo. Como el lenguaje no
// tiene sintaxis de struct literal anónimo (siempre hace falta un nombre
// declarado, ver struct_or_variant_lit §2.3), la forma completa de creación
// se modela con un `type` propio, estructuralmente idéntico a Omit<User,"id">:
type NewUserRecord = { name: String, email: String, role: Role, bio?: String, deletedAt: String? }
fn makeUser(input: NewUser) -> NewUserRecord {
  NewUserRecord { name: input.name, email: input.email, role: Role.Member {}, deletedAt: null }
}
// db.users.insert(makeUser(input)) -- NewUserRecord <: Omit<User,"id"> por subtipado estructural
```

**Métodos:** `all() -> T[]`, `find(id: Int) -> T?`, `insert(x: Omit<T,"id">) -> T`, `applyPatch(id: Int, p: Patch<T>) -> T` — resueltos contra el tipo de elemento de verdad (`Type::DbCollection`, checker.rs). Un nombre de colección o de método desconocido ya es un error del checker (`db.usres.fnid(1)`, con AMBOS typo'd, se rechaza en tiempo de chequeo), no algo que se descubre recién en runtime.

**Runtime: en memoria al principio, generalizado.** `runtime/db.rs`'s `Db` pasó de estar hardcodeado a una única colección `"users"` a un `HashMap` con una entrada por colección declarada. Se eliminó el hack que le ponía un default a `deletedAt` en `insert` — bajo la regla `Omit<T,"id">`, `deletedAt` (requerido, nullable) es un campo obligado del argumento; quien inserta pasa `deletedAt: null` explícito, consistente con "sin coerción implícita en ningún lado" (§3.7). **Actualización: RESUELTO.** El storage detrás ya no es en memoria -- ver §3.17: `Db` corre sobre SQLite real, con persistencia genuina entre reinicios de `linkc serve`.

### 3.13 Streaming real (SSE) para `stream` — RESUELTO, alcance `List<T>`

Antes, `Member::Rpc`/`Member::Stream` se colapsaban a lo mismo en todo el pipeline — pegarle a un `stream` por HTTP corría el cuerpo una vez y devolvía un solo JSON con 200, sin ningún indicio de que debía ser un stream. El stub del cliente generado ni siquiera lo intentaba (`throw new Error("streaming no implementado...")`).

**Alcance explícito, de entrada: repite una secuencia YA CALCULADA, no suscribe a eventos futuros.** El ejemplo de `PLAN.md` (`stream watch(id) -> User { db.users.subscribe(id) }`) implica suscribirse a cambios que todavía no pasaron — eso necesitaría una capa de pub-sub sobre `db` que no existe. Lo que sí es real y honesto: el cuerpo de un `stream` devuelve `List<T>` (una lista completa, ya en memoria) y el servidor la manda como eventos SSE genuinos en vez de un solo blob JSON — mejor time-to-first-byte del lado del cliente, y el wire protocol que `AsyncIterable<T>` promete de verdad. **Actualización: RESUELTO para un shape fijo.** El lenguaje ya tiene un constructo de loop (`while`, §3.15) y, sobre él, una capa real de pub-sub para `db` (§3.16) — un `stream` cuyo cuerpo es exactamente `while true { db.<coleccion>.subscribe() }` sí recibe eventos futuros de verdad, sin polling. Todo lo demás (un cuerpo con cualquier otra forma) sigue el camino `List<T>` descripto en esta sección, sin cambios.

```
// La firma declara el ELEMENTO (igual que un rpc normal) -- el cuerpo
// tiene que devolver la secuencia completa (List<User>, no User suelto).
stream watchAll() -> User {
  db.users.all()
}
```

**Checker: `check_rpc` chequea el cuerpo contra `List<T>`, no contra `T`, cuando `is_stream`.** La firma (`return_type`) sigue resolviendo a `T` sin ningún caso especial — eso es lo que ya usan `emit_service_interface` (`AsyncIterable<T>`) y el validador de cada evento (mismo `isX` que un rpc normal usa para su único valor de retorno). Solo el chequeo del CUERPO envuelve el tipo esperado en `Type::List` antes de llamar a `check_block`.

**Runtime: `invoke_rpc` no distingue Rpc/Stream al evaluar** (siempre hizo `Member::Rpc(r) | Member::Stream(r)` en el lookup) — el resultado ya es el `Vec<Value>`/array JSON completo que `server.rs` necesita. Lo único nuevo es `is_stream_member(program, service, rpc) -> bool`, una función APARTE (no un cambio a la firma de `invoke_rpc`) que le permite a `server.rs` decidir el framing ANTES de invocar, sin forzar a los ~30 call sites de test existentes (todos `.unwrap()` un solo `Value`) a desestructurar una tupla que no les interesa.

**`server.rs`: solo la ESCRITURA de eventos corre en un hilo aparte, no el cómputo.** `invoke_rpc` siempre corre en el loop principal (igual que cualquier `rpc` normal); el hilo spawneado recibe el resultado YA CONVERTIDO a `Vec<serde_json::Value>` y solo se encarga de mandar los bytes SSE al cliente, así una escritura lenta (cliente que lee despacio) no bloquea al servidor de aceptar el resto de las conexiones. **Revisado durante la ronda de closures (§3.10):** el diseño original hacía correr `invoke_rpc` DENTRO del hilo spawneado (con `Arc<Program>`/`Arc<Db>` compartidos) — eso dejó de compilar en cuanto `Value` ganó el variant `Closure` (guarda un `Env` con `Rc<RefCell<Value>>`, ni `Send` ni `Sync`), porque `Db` guarda `Vec<Value>` y ya no podía cruzar el borde del hilo. La corrección de arriba (cómputo en el hilo principal, solo escritura aparte) resultó, además, un diseño más ajustado que el original: lo único que de verdad necesitaba correr aparte era la escritura, no el cómputo.

**Hallazgo real, no anticipado por el plan: `tiny_http::Response` + `request.respond()` NO sirve para streaming.** Confirmado con un spike aislado antes de tocar código de producción (no solo lectura de fuente): `request.rs::respond_impl` solo llama `writer.flush()` UNA vez, al final, sobre un `BufWriter::with_capacity(1024, ...)` (`client.rs`) que envuelve el socket real. Un `Read` que produce datos de a poco con sleeps en el medio NO llega incrementalmente al cliente por ese camino — todo el body sale junto, recién al cerrar la respuesta. La solución: `request.into_writer()` (acceso directo al mismo `BufWriter`, pero bajo control manual) + un `flush()` explícito después de cada evento — `BufWriter::flush()` ignora su capacity interno y fuerza lo acumulado al socket en el momento.

**Segundo hallazgo real, encontrado recién al probar con el `client.ts` GENERADO (no con un cliente crudo): `Connection: close` sin `Content-Length` ni `Transfer-Encoding` no alcanza.** Es válido por RFC 7230 §3.3.3 regla 7 ("el body termina cuando se cierra la conexión"), y un cliente TCP crudo lo respeta bien — pero `fetch()` nativo de Node (sobre `undici`) no lo trata como señal confiable de fin de body bajo HTTP/1.1: el stream llegaba completo pero el `for await` nunca veía `done: true`, colgado esperando más datos indefinidamente. Fix: `Transfer-Encoding: chunked` real, con el framing (`{tamaño-hex}\r\n{datos}\r\n`, terminado en `0\r\n\r\n`) escrito a mano en `server.rs` — bypasseando también `chunked_transfer::Encoder` (vive adentro de `Response::raw_print`, el mismo camino que ya se bypasseaba por el hallazgo anterior). Es la señal que todo cliente HTTP/1.1 sabe reconocer sin ambigüedad, a diferencia de depender del cierre de conexión.

**Desconexión de cliente a mitad de stream: no cuelga el hilo.** Confirmado con el mismo spike: el próximo `write()` después de que el cliente cierra la conexión falla de inmediato con `BrokenPipe`/`ConnectionAborted`/`ConnectionReset` (según la plataforma) — nunca se queda esperando. `write_stream` corta el loop apenas ve ese error, sin nada más que limpiar (la lista ya estaba completa en memoria de entrada).

**Cliente generado: `fetch()` + parseo manual del framing SSE, no `EventSource`.** `EventSource` es GET-only y sin body — pero el resto del contrato ya asume POST+JSON body para argumentos (igual que cualquier otro rpc), y un `stream` puede tener parámetros. En cambio, `async *m(): AsyncIterable<T>` lee `res.body` (un `ReadableStream` nativo de `fetch`) a mano: acumula en un buffer, corta en `\n\n`, valida cada `data: ...` con el mismo `isX` que un rpc normal, y hace `yield` recién si pasa. Cero dependencias nuevas (`TextDecoder`/`ReadableStream` son nativos de Node y del browser).

**De paso: un log mínimo de request-id.** Un `AtomicU64` incremental (`server.rs`) y dos líneas por request (inicio + status/resultado) — lo mínimo que el cambio a multi-hilo hace necesario para poder correlacionar logs concurrentes, no una iniciativa de observabilidad aparte.

### 3.14 Auth v0 (sesión opaca en memoria + roles) — RESUELTO

Hasta acá no existía NINGÚN mecanismo de guard/autorización en el lenguaje — cualquiera podía invocar cualquier `rpc`. Alcance elegido para v0, explícitamente: sesión opaca en memoria + roles, **sin JWT y sin ninguna dependencia nueva** (el proyecto sigue dependiendo solo de `tiny_http` + `serde_json`). Verificar contraseña/hash de credenciales queda **fuera de alcance a propósito** — es su propio problema de seguridad, no algo para meter de paso acá.

```
service Users {
  @authenticated
  rpc me() -> User { ... }

  @requires(Role.Admin)
  rpc update(id: Int, patch: Patch<User>) -> User { db.users.applyPatch(id, patch) }

  rpc list() -> User[] { ... }   // sin anotación = sin restricción, como siempre
}
```

`@authenticated` exige una sesión válida, cualquier rol. `@requires(Enum.Variante)` exige además que el rol de esa sesión sea exactamente esa variante. **A lo sumo una anotación DE AUTH por rpc/stream** (`@requires` ya implica autenticado; el checker rechaza dos) — límite deliberado de v0, sigue vigente. **"Un solo rol por `@requires`" era otro límite de v0 -- resuelto en §3.49**: `@requires(Role.Admin | Role.Agent)` acepta cualquiera de varios roles, todos del mismo enum. Desde §3.35 la lista sí admite varias anotaciones (`RpcDecl.annotations: Vec<Annotation>`), porque `@content_type` es una dimensión distinta de la auth: una página de panel de administración es HTML *y* está protegida.

**`@requires(Role.Admin)` reusa el mecanismo de `Enum.Variante` que YA existía para nombrar una variante en un patrón de `match`** (`parse_pattern_atom`, `ident "." ident`, SIN llaves) — no se inventó una tercera sintaxis. Esto es a propósito ASIMÉTRICO con `Role.Admin {}` (que sí hace falta para *construir* un valor real, ej. al llamar `auth.createSession(Role.Admin {})`): una anotación nombra un TAG a comparar, una expresión construye un VALOR — dos reglas correctas por separado, pero que un usuario puede confundir la primera vez que las ve una al lado de la otra.

**El enum de `@requires`/`createSession` NO necesita ser "simple" (todas las variantes unitarias).** La comparación en runtime es solo por tag (`enum_name` + nombre de variante), nunca mira campos — así que `enum Role { Admin, Member, ServiceAccount { scopes: String[] } }` puede usar `@requires(Role.Admin)` sin problema, aunque `ServiceAccount` (una variante HERMANA) sí tenga datos.

**Dos builtins nuevos sobre el identificador `auth`** (mismo mecanismo que `db`: `Type::Auth`/`Value::Auth`, identificador especial resuelto en `synth_expr`/`eval_expr` DESPUÉS del lookup de variables locales — ver el hallazgo de abajo sobre por qué ese orden importa):
- `auth.createSession(role: R) -> String` — `R` debe sintetizar a un enum declarado; devuelve un token opaco.
- `auth.destroySession() -> Void` — **CERO argumentos**, a propósito (ver "hallazgo de seguridad" más abajo).

```
rpc login(email: String) -> String? {
  let matches = db.users.all().filter(|u: User| { u.email == email });
  if matches.length() > 0 { auth.createSession(matches[0].role) } else { null }
}

@authenticated
rpc logout() -> Void { auth.destroySession() }
```

**La decisión de autorización (401/403) vive en `server.rs`, no en el intérprete.** `runtime/mod.rs` solo recibe `sessions: &SessionStore` (para que los dos builtins de arriba funcionen) y `current_token: Option<&str>` (para que `destroySession()` sepa cuál es "la propia" sesión) — ninguno de los dos es una decisión, son datos ya resueltos por el caller. El gate real (`server.rs::check_auth_gate`) corre ANTES de `parse_args`/`json_to_typed_value`, usando solo `program` (para mirar la anotación vía `required_auth`, hermana de `is_stream_member`) + `sessions` (para resolver el token a un rol) — nunca construye ningún `Value` del intérprete. Corre para `rpc` Y `stream` por igual (ambos pasan por el mismo punto en `serve()`). `invoke_rpc` (la firma pública de siempre, ~70 call sites — tests + `wasm_demo.rs`) queda intacta como wrapper de una línea sobre `invoke_rpc_with_sessions`, que es la que de verdad recibe `sessions`/`current_token`.

**401 vs. 403, y qué NO se revela.** Sin token, o token que no resuelve a ninguna sesión → 401 genérico ("se requiere autenticación"), sin distinguir los dos casos (no ayuda a ningún cliente legítimo, y sí le da a un atacante una forma barata de validar el formato de un guess). Sesión válida pero rol incorrecto → 403, con un mensaje genérico que **no nombra el rol exigido** — a diferencia del nombre del rpc (ya público vía `client.ts`/`contract.d.ts`), qué rol hace falta para cada operación es política interna del servidor; regalarla le daría a cualquiera con un token de bajo privilegio un mapeo completo endpoint→rol gratis.

**Hallazgo de seguridad central de esta ronda: el generador de tokens original estaba roto, no solo "no revisado".** La primera versión generaba el token con `RandomState::new().build_hasher().finish()`, llamado dos veces, asumiendo ~128 bits frescos por token. Dos revisores adversariales en paralelo llegaron, cada uno por su cuenta, a la misma causa raíz: `std` cachea las keys `(k0,k1)` de `RandomState` **por hilo** — la primera vez que se pide en un hilo dado, lee del SO; cada llamada SUBSIGUIENTE en ESE MISMO hilo solo incrementa `k0` en 1, `k1` nunca cambia. Como el intérprete corre siempre en el hilo principal (single-threaded por diseño, §3.13), esto no daba "un secreto nuevo por token" sino **un único secreto de 128 bits fijado una vez al arrancar el proceso**, reusado con un contador chico encima — insuficiente para lo único que hace segura a una sesión bearer ("poseer el string ES la sesión"). Fix real, sin agregar ninguna dependencia: un hilo RECIÉN CREADO nunca inicializó ese cache thread-local, así que su PRIMER `RandomState::new()` sí pega contra el RNG real del SO (`BCryptGenRandom`/`ProcessPrng` en Windows). `SessionStore::fresh_128_bits` (`runtime/session.rs`) spawnea un hilo descartable y, DENTRO de él, deriva 2 hashes de 64 bits de la MISMA `RandomState` (sin volver a llamar `::new()`, que reincidiría en el problema). **Esto sigue sin ser un CSPRNG auditado** — alcanza para v0/demo; una implementación real necesitaría el crate `rand`/`getrandom`.

**Segundo hallazgo real: `destroySession(token)` como parámetro ordinario es una vulnerabilidad, no un detalle de API.** La propuesta original tomaba el token a destruir como argumento, simétrico a `createSession`. Un revisor adversarial lo marcó como el hallazgo más concreto de su ronda: **cualquiera que conozca o adivine el token de otra sesión podría destruirla sin poseerla ni haber pasado ningún chequeo de `@requires`** — un primitivo de "logout ajeno"/DoS dirigido, sin ningún segundo factor (a diferencia de RFC 7009, revocación OAuth, que sí exige credenciales del client que revoca). Fix: `destroySession()` sin argumentos, operando implícitamente sobre `current_token` — la sesión que ya autenticó la request actual. Por eso `logout` necesita `@authenticated`: sin sesión válida no hay nada que destruir, y sin la anotación el intérprete no sabría cuál token es "el propio".

**Bug preexistente encontrado de paso, no introducido por esta ronda: `eval_expr` no respetaba el orden de shadowing que `synth_expr` (checker) ya respetaba.** Al agregar el identificador especial `"auth"` a `eval_expr::Ident`, se encontró que esa función chequeaba `if name == "db"` **ANTES** de `env.get(name)` — al revés que el checker, que hace `env` primero desde que se corrigió el mismo bug para "DB tipada" (con un comentario explícito documentándolo). Consecuencia real, sin tocar nada de esta ronda: `fn f(db: Int) -> Int { db + 1 }` tipaba perfecto y **crasheaba en runtime**, porque `eval_expr` devolvía `Value::Db` ignorando el parámetro real. El único test relacionado solo verificaba que tipara, nunca lo ejecutaba. Corregido en el mismo lugar que hacía falta tocar para `auth`, con un test de runtime nuevo (antes no existía ninguno que ejecutara este caso).

**Otro hallazgo de paso: `const` no estaba restringido a literales fuera de `linkc build`.** `check_const` aceptaba cualquier expresión que tipara — la restricción real de forma-literal vivía solo en `ts_emit.rs::render_const_value`, o sea que `linkc serve` (que nunca llama a los emisores) nunca la exigía. Ya era una rareza inocua con `db` (`const X: User[] = db.users.all();` "funciona" en `serve`, releyendo la colección en cada uso). Con `auth.createSession(...)` deja de ser inocuo: un `const` así crearía una sesión Admin nueva cada vez que se lo referencia (los `const` no se memoizan en runtime), sin que nadie la pidiera ni forma de limpiarla. `check_const` ahora exige la misma forma-literal en `check_program` (por lo tanto en `serve` también), cerrando el agujero para los dos casos con una sola regla.

**CORS: `Access-Control-Allow-Headers` no dejaba pasar `Authorization`.** Confirmado por los dos reviews como necesario para que la feature sea alcanzable en absoluto: sin agregarlo, el preflight `OPTIONS` de cualquier browser real rechaza la request ANTES de que salga — ni siquiera es que el servidor la rechace, el browser no la intenta. Un solo cambio (`"Content-Type, Authorization"`) cubre `rpc` y `stream` por igual. `Access-Control-Allow-Origin: *` + un header `Authorization` manual no es el caso que la spec de CORS prohíbe combinar con `*` (eso aplica a `credentials: 'include'`/cookies, que este cliente nunca usa).

**Cliente generado: `token` es estado MUTABLE de instancia, no un parámetro por-llamada.** `{Service}ClientImpl` gana `private token: string | null` + `setToken(token)`, parte de la interfaz pública (`{Service}Client`) para que algo tipado como tal también pueda llamarlo. `push_fetch_call` adjunta `Authorization: Bearer ${token}` en TODO rpc si hay token seteado (el servidor decide caso por caso si lo exige). Correcto para "una instancia de cliente = un usuario/sesión activa" (mismo patrón que la mayoría de SDKs generados reales) — pero una instancia COMPARTIDA entre requests concurrentes de usuarios DISTINTOS (ej. un backend-for-frontend Node reusando un cliente módulo-level) puede pisarse el token entre requests. Documentado como límite v0 explícito; la alternativa (token por-llamada) cambiaría la forma pública de TODOS los métodos generados, no solo los protegidos.

**Fuera de alcance, a propósito:** verificación de contraseña/credenciales; un CSPRNG auditado (ver el hallazgo de arriba). Cuatro límites de esta lista original **ya se resolvieron**: expiración de sesión ("vive hasta `destroySession()` o hasta reiniciar el proceso" — resuelto en §3.50, `--session-ttl`), múltiples roles por `@requires` (resuelto en §3.49, `Role.Admin | Role.Agent`), leer el ROL del caller dentro de un cuerpo (resuelto en §3.51, `auth.currentRole()`), y persistir/leer el ID del caller (resuelto en §3.53, `auth.createSessionWithId(role, userId)` y `auth.currentUserId()`) — múltiples anotaciones DE AUTH por rpc (dos `@requires` distintos en el mismo rpc) sigue sin tener sentido y el checker lo sigue rechazando, eso no cambió. Cargar la entidad completa `User` en memoria (`ctx.user`) sigue haciéndose de forma explícita mediante `db.users.find(uid)` a partir del `userId` obtenido.

---

### 3.15 Constructo de loop: `while` — RESUELTO, alcance acotado

Hasta acá el lenguaje no tenía NINGÚN constructo de loop — la única forma de repetir algo era recursión (una `fn` con nombre llamándose a sí misma, o un closure reasignado vía `mut` que se referencia a sí mismo, que además arma un ciclo real de `Rc`, ver §3.10). Elegido para v0, explícitamente: **`while` únicamente, `Stmt` (nunca `Expr`), sin `break`/`continue`, con una cota dura de iteraciones.**

```
fn sum(xs: Int[]) -> Int {
  let mut total = 0;
  let mut i = 0;
  while i < xs.length() {
    total = total + xs[i];
    i = i + 1;
  }
  total
}
```

**`while` NUNCA es una expresión.** `if`/`match` sí lo son porque necesitan unificar un valor entre ramas — eso exigiría diseñar `break <valor>`, un tipo para "el loop que nunca hace `break`" (el lenguaje no tiene ningún tipo `Never`/bottom) y unificación de tipos entre N sitios de `break`. Nada de eso hace falta para agregar sin recursión: el patrón es mutar un `let mut` declarado ANTES del loop, y usar un valor de cola DESPUÉS de él — el `while` en sí corre por puro efecto, se chequea contra `Type::Void` (mismo tratamiento que un `if`/`match` en posición de sentencia).

**Sin `for`, a propósito.** No existe ningún concepto de rango/iterador en el lenguaje (`.take`/`.filter`/`.map`/`.length` siguen siendo los únicos métodos de `List`, sin `.reduce()`/`.forEach()`); todo lo que `for` daría ya es expresable con `while` + indexado manual (`arr[i]`, que ya existía). Agregarlo antes de que `while` se haya usado en programas reales sería azúcar prematuro — mismo criterio que ya dejó afuera closures de 0 parámetros y roles múltiples en `@requires`.

**Sin `break`/`continue`, a propósito.** Implementarlos bien primero necesita resolver el hallazgo de abajo (un `break` anidado dentro de un `if`/`match` fallaría en silencio por la misma razón estructural que `return` ya falla) — deferido a una ronda futura si el uso real lo pide; la recursión sigue disponible mientras tanto para loops con salida temprana.

**`return` dentro de un cuerpo de `while` se RECHAZA explícitamente en el checker — no es una limitación caprichosa, evita heredar un bug real y ya existente.** Encontrado leyendo el código vecino al diseñar esto, no introducido por esta ronda: un `return` anidado dentro de un `if`/`match` usado COMO SENTENCIA (no cola) no solo tipa mal hoy (se chequea contra `Void` en vez del tipo real de retorno, por cómo `check_stmt` trata `if`/`match`-como-sentencia) sino que en RUNTIME es un no-op silencioso — `eval_block` descarta el valor que produce ese `if`/`match` (incluido cualquier `return` de adentro, que solo corta el `eval_block` INTERNO de esa rama, no el que la contiene) y sigue con la sentencia siguiente como si nada. Ya es explotable hoy con un `return;` desnudo en una función `Void`. En vez de reescribir el mecanismo de señalización de control de flujo entero (un cambio mucho más grande y riesgoso que agregar un loop), `while` simplemente no deja usar `return` en su cuerpo — sacá el valor final con una variable `mut` declarada antes del loop y un tail después, como en el ejemplo de arriba. El bug preexistente en `if`/`match`-como-sentencia queda documentado pero sin arreglar, fuera de alcance de esta ronda.

**Cota dura de iteraciones (`MAX_WHILE_ITERATIONS = 1_000_000`, `runtime/mod.rs`) — no opcional, agregada en la MISMA ronda que el loop.** El servidor (`server.rs::serve`) es un loop estrictamente single-threaded sin timeout ni scheduling cooperativo: un `while true { }` (o cualquier condición que el programa nunca vuelve falsa) congelaría PARA SIEMPRE el único hilo que atiende TODAS las requests, no solo la que lo disparó. Esto no es un límite v0 "honesto" en el mismo espíritu que otros (ej. "sin CSPRNG auditado") — es un footgun nuevo que la propia feature introduce, y este proyecto ya encontró y arregló footguns reales de ese calibre por review adversarial (el generador de tokens y `destroySession`, §3.14). La cota es deliberadamente generosa y NO configurable: un backstop contra el bug/loop-infinito más común, no un sistema fino de cuotas de recursos. Se cuenta una vez por invocación de rpc/fn (un `Cell<u64>` enhebrado por todo el árbol de evaluación, incluidos loops anidados y loops dentro de una fn/closure llamada desde el cuerpo), así que partir un loop grande en muchos chicos no lo esquiva.

**Fuera de alcance, a propósito:** `for`, `break`/`continue`, `while` como expresión con `break <valor>`; el bug preexistente de `return` dentro de `if`/`match`-como-sentencia (documentado arriba, no arreglado); límite de profundidad de recursión (preexistente, no empeorado por esta ronda — barato de cerrar reusando el mismo `Cell<u64>` si hace falta más adelante).

### 3.16 Push real: pub-sub sobre `db` para `stream` — RESUELTO, alcance acotado (shape fijo)

Con `while` ya resuelto (§3.15), el segundo bloqueo que §3.13 dejaba pendiente para push real era la falta total de una capa de pub-sub sobre `db`. Elegido para v0, explícitamente (vía pregunta directa, no un default silencioso): en vez de un mecanismo general de corutinas/`yield` para lógica arbitraria por evento, el diseño reconoce en tiempo de compilación UN ÚNICO shape sintáctico fijo como cuerpo de un `stream` "en vivo":

```
stream watchItems() -> Item {
  while true {
    db.items.subscribe()
  }
}
```

Cualquier otra forma (otro método, argumentos, sentencias de más, otra condición) NO dispara push real — cae al camino `List<T>` de §3.13, o directamente no tipa, nunca a una ejecución silenciosamente distinta de lo que el código sugiere.

**Por qué un shape fijo alcanza, en vez de corutinas de verdad.** El caso de uso real (anunciar mutaciones de `db` para siempre) no tiene ningún estado que se acarree entre iteraciones — cada vuelta hace exactamente lo mismo, "¿cuál es la próxima fila?". Bajo esa condición, "suspender el intérprete a mitad del loop y reanudarlo después" y "no dejar que el intérprete corra el loop en absoluto, y resolver todo con un registro de suscriptores en Rust puro" son observacionalmente idénticos — no hay nada que una corutina real preservaría que este atajo no dé gratis. Por eso `server.rs` intercepta el shape reconocido ANTES de invocar `invoke_rpc_with_sessions`: el cuerpo de un `stream` "en vivo" nunca llega a `eval_block`.

**El reconocedor vive en `ast.rs`, no en el checker ni en el runtime.** `recognize_live_subscribe(body: &Block) -> Option<&str>` es sintáctico puro (sin tipos): devuelve el nombre de la colección si el cuerpo es exactamente ese `while true { db.<col>.subscribe() }`, o `None` para cualquier otra cosa. Vivir en `ast.rs` es lo que le permite tanto a `checker.rs` (`check_rpc`, para tipar) como a `runtime/mod.rs`/`server.rs` (`live_subscribe_collection`, para interceptar en tiempo de request) llamarlo sin que ninguno de los dos dependa del otro.

**Hueco de TOCTOU cerrado a propósito, no dejado abierto.** Si `check_db_method` le diera a `"subscribe"` una firma normal y libremente componible (como `all`/`find`), entonces `rpc getOne() -> User { db.users.subscribe() }` -- fuera del shape reconocido -- tipiaría bien sin tener ningún comportamiento sensato en runtime. Fix: el brazo `"subscribe"` de `check_db_method` SIEMPRE falla, con un mensaje que apunta al shape exacto que sí funciona. La única forma de que `subscribe()` tipe en todo el programa es a través de `check_rpc` reconociendo el shape completo primero -- nunca a través del camino genérico de métodos de `db`.

**`Db` gana un registro de suscriptores; `subscribe()` hace snapshot+registro en una sola llamada sincrónica.** `Db::subscribe(collection)` devuelve `(snapshot, Receiver)`: `snapshot` es el estado actual de la colección ya serializado a JSON (mismo `value_to_json` que cualquier respuesta normal), y `Receiver` es el lado de lectura de un `mpsc::sync_channel(1024)` recién registrado. Las dos partes (sacar la foto, registrarse) son las dos líneas de UNA sola llamada, sin ningún punto de suspensión entre ellas -- y la única otra cosa que podría "colarse" (una mutación, vía `insert`/`applyPatch`) solo pasa dentro de `Db::call`, en el mismo único hilo del servidor. Como el servidor entero procesa una request a la vez, no hay forma de que una mutación se intercale entre esas dos líneas: el single-threading del servidor ES el lock del pub-sub, no algo aparte que hubo que agregar. (Si `Db` alguna vez dejara de ser single-threaded, este argumento hay que revisarlo primero -- probablemente invirtiendo el orden a "registrarse, después sacar la foto, después descartar duplicados".)

**`publish()` nunca bloquea, y un suscriptor lento o muerto no puede tirar abajo al servidor.** Cada `insert`/`applyPatch` exitoso llama `publish(collection, &row)` justo antes de devolver -- convierte la fila a JSON una vez y hace `try_send` (nunca bloqueante) a cada suscriptor de esa colección, podando (`retain`) cualquiera que devuelva `Full` (buffer de 1024 lleno, cliente no lee lo bastante rápido) o `Disconnected` (el hilo que escribía ya terminó). Un canal ilimitado hubiera sido un vector real de agotamiento de memoria; la política elegida es simple y explícita: mejor perder eventos para un suscriptor atascado que crecer sin límite.

**La limpieza de un suscriptor desconectado es LAZY, a propósito -- no eager.** Nada en el servidor nota activamente que un socket se cerró; lo que pasa es que el hilo escritor de ESE stream (`write_live_stream`, spawneado por `server.rs`, nunca el hilo principal) intenta escribir el próximo evento que le llega por su `Receiver`, ese `write()` falla con `BrokenPipe`/`ConnectionReset` igual que en §3.13, el hilo loguea `"cliente desconectado de un stream en vivo tras N eventos"` y termina -- recién en la SIGUIENTE mutación a esa colección, `publish()` encuentra el `SyncSender` ya cerrado (`Disconnected`) y lo poda del registro con `retain`. Entre la desconexión real y esa próxima mutación, el suscriptor muerto sigue ocupando una entrada -- aceptado a propósito: una limpieza eager reabriría la misma pregunta de `Send`/`Sync` que todo este diseño evita (ver §3.10 sobre por qué `Value`, y por lo tanto `Db`, están confinados a un hilo).

**Suscripción a la colección ENTERA, no por fila.** `subscribe(id: Int)` (recibir solo los cambios de una fila puntual) queda deliberadamente afuera de v0 -- whole-collection es un superset estrictamente más simple de reconocer (el shape fijo no necesita validar ningún argumento) y el cliente ya puede filtrar por `id` del lado TS sin ningún cambio de protocolo, gratis.

**Verificado end-to-end con el `client.ts` generado de verdad, no con una llamada cruda.** Se ejecutó el flujo completo con el cliente TAL COMO lo genera `linkc build` (ningún cambio de codegen hizo falta -- confirma la premisa del diseño en §3.13, "el cliente ya lee de forma indefinida"): insertar una fila ANTES de abrir el stream y confirmar que el primer evento recibido es esa foto inicial; insertar una SEGUNDA fila mediante una request separada mientras el stream seguía abierto y confirmar que llega como evento nuevo por la MISMA conexión, sin que se corte; terminar el proceso cliente abruptamente (sin cerrar el stream de forma prolija) e insertar una tercera fila, confirmando que el hilo principal sigue respondiendo de inmediato (el stream muerto se poda recién ahí, con el log esperado) -- nada se cuelga ni crashea del lado del servidor.

**Fuera de alcance de esta ronda, a propósito:**
- Filtrado/transformación por evento DENTRO del cuerpo de un stream -- exigiría reentrada real del intérprete (insegura sin corutinas) o cómputo en el momento del `publish`; ninguna de las dos entra en el shape fijo de esta ronda.
- Suscripción por fila (`subscribe(id)`) -- ver arriba.
- `delete` sobre `db` (no existe hoy) y qué significaría publicar una fila "eliminada".
- Re-autorización de una conexión en vivo de larga duración si la sesión que la abrió se revoca después -- `@authenticated`/`@requires` se valida una sola vez, al abrir: una conexión de horas de duración amplía ese hueco respecto de un rpc normal de vida corta.
- Limpieza EAGER de suscriptores desconectados (ver arriba) -- lazy es la política elegida para no reabrir la pregunta de `Send`/`Sync`.

### 3.17 Persistencia real: `db` sobre SQLite — RESUELTO

Con auth v0, los 3 prerrequisitos de LSP, y push real + loop (§3.15/§3.16) ya cerrados, el pendiente elegido fue "DB real con SQL": `db { ... }` (§3.12) era real a nivel de TIPOS desde esa ronda, pero el storage detrás seguía siendo un `HashMap<String, Mutex<Vec<Value>>>` puramente en memoria -- cada reinicio de `linkc serve` empezaba con todo vacío. Esta ronda le da persistencia genuina, manteniendo el mismo contrato público (`all/find/insert/applyPatch`, más `subscribe` del §3.16) sin ningún cambio en checker.rs ni en los ~50 call sites de test existentes.

**`rusqlite` (SQLite embebido, feature `bundled`), no Postgres.** El servidor es deliberadamente single-threaded y sin ningún runtime async (`Value::Closure` guarda un `Env` con `Rc<RefCell<Value>>>`, ni `Send` ni `Sync` -- confirmado desde la ronda de closures, §3.10) -- un driver async (`sqlx`, `tokio-postgres`) exigiría traer `tokio` entero, un cambio de arquitectura mucho más grande que esta ronda. `rusqlite` es sync-only por diseño, embebido (sin proceso de servidor externo corriendo aparte), y `bundled` compila su propio SQLite sin necesitar uno instalado en el sistema -- coherente con que `linkc serve` siga arrancando solo, mismo espíritu que ya tiene `tiny_http`. Postgres se descartó explícitamente por necesitar un servidor externo corriendo, rompiendo ese mismo espíritu.

**El schema SQL se DERIVA de `db { ... }`, nunca se escribe a mano** -- mismo principio que ya rige contract.d.ts/client.ts/validators.ts (todos generados de la misma fuente de verdad). Por cada colección, `Db::new` corre `CREATE TABLE IF NOT EXISTS` con un mapeo fijo:

| Campo c-script | Columna SQLite | Round-trip |
|---|---|---|
| `id: Int` | `INTEGER PRIMARY KEY AUTOINCREMENT` | ver justificación abajo -- y §3.18 para por qué pasó a llevar `AUTOINCREMENT` |
| `x: Int/Float/String/Bool` (requerido) | `INTEGER`/`REAL`/`TEXT`/`INTEGER` `NOT NULL` | directo |
| `x: EnumSimple` (requerido) | `TEXT NOT NULL` | nombre de variante en texto plano (`"Admin"`), no envuelto en JSON |
| `x: T?` (nullable, la clave SIEMPRE está) | columna nullable de `T` | SQL `NULL` ⇄ `Value::Null` |
| `x?: T` (opcional-por-clave, `T` no opcional) | columna nullable de `T` | SQL `NULL` ⇄ clave AUSENTE del `Value::Struct` |
| `x?: T?` (ambos a la vez, §3.4) | `TEXT`, siempre | único caso con 3 estados reales -- ver abajo |
| Struct / enum ADT / List / Tuple / Map / Generic / Union / Result / Patch | `TEXT` | `value_to_json`/`json_to_typed_value` reusados tal cual, cero formato nuevo |

**`id` como `INTEGER PRIMARY KEY AUTOINCREMENT` (revisado en §3.18).** En SQLite, `INTEGER PRIMARY KEY` es alias del rowid: insertar sin especificarlo autoasigna `max(rowid)+1`. En la ronda original de esta sección, `AUTOINCREMENT` se dejó afuera a propósito porque su única garantía adicional ("nunca reusar un id después de un borrado") era irrelevante -- no existía ningún método `delete` en todo el lenguaje, así que un id reusado no podía pasar por construcción. §3.18 agregó `delete`, lo que vuelve real esa situación (insertar tras borrar el último row reusaría su id sin `AUTOINCREMENT`) -- el mapeo de la tabla de arriba ya refleja el fix.

**`x?: T?` necesita 3 estados; una columna SQL solo tiene un bit de NULL.** Este es el único caso que se fuerza a `TEXT` (envuelto en JSON) aunque `T` sea nativo, específicamente para ganar un tercer estado: SQL `NULL` = clave ausente; el texto `"null"` (el JSON de `Value::Null`) = clave presente con valor null; cualquier otro texto = clave presente con un valor real. Sale gratis de `value_to_json`/`json_to_typed_value` sin ningún código especial -- `value_to_json(Value::Null)` YA serializa a `"null"`.

**Schema incompatible entre corridas: falla fuerte, nunca migra -- con una única excepción aditiva.** Al abrir, después del `CREATE TABLE IF NOT EXISTS`, se compara vía `PRAGMA table_info` el schema real de la tabla contra el que el programa actual declara (como conjunto, no por posición). Cualquier diferencia hace panic ANTES de aceptar ninguna request, nombrando el archivo, la colección, y el diff exacto (esperado vs. encontrado), terminando en "borrá el archivo y volvé a intentar" -- salvo el único caso donde SÍ auto-migra sin destruir nada: una columna nueva y OPCIONAL (`x?: T`/`x: T?`, nunca requerida) que el `.link` actual declara pero la tabla física todavía no tiene se agrega con `ALTER TABLE ADD COLUMN`, sin tocar filas existentes. Detalle real: `id INTEGER PRIMARY KEY` reporta `notnull=0` en `PRAGMA table_info` aunque nunca pueda ser NULL de verdad -- la comparación trata esto como un caso especial, o cualquier reinicio detectaría un mismatch falso desde el primer arranque.

**Matriz de comportamiento completa** (PLAN.md §9.1.1, pedida explícitamente en dos reportes de adopción real -- el README solo documentaba antes el caso aditivo):

| Cambio en el `.link` | SQLite (`linkc serve`) | PostgreSQL (`linkc serve --db postgres://...`) |
|---|---|---|
| Columna nueva, opcional | Se auto-agrega (`ALTER TABLE ADD COLUMN`), sin pérdida de datos | Se auto-agrega, siempre nullable |
| Columna nueva, **requerida** | Falla al conectar (no hay forma de rellenar filas viejas) | Se auto-agrega igual, pero SIEMPRE nullable -- una fila vieja queda con `NULL` ahí; leerla ahora da un error de runtime limpio, no un `null` silencioso (§3.68) |
| Columna eliminada del `.link` | Falla al conectar (columna física de más) | Se ignora en silencio -- queda huérfana, nunca se borra ni se lee |
| Columna renombrada | Falla al conectar (la vieja queda de más, la nueva falta) | La vieja queda huérfana; la nueva se auto-agrega nullable -- los datos NO se migran de una a otra |
| Cambio de tipo de una columna existente | Falla al conectar | No se detecta al conectar (`ADD COLUMN IF NOT EXISTS` es no-op sobre una columna que ya existe); el tipo real sigue siendo el viejo, una lectura/escritura con el tipo nuevo puede fallar según la conversión SQL |
| Campo requerido → opcional | Falla al conectar (la columna física sigue `NOT NULL`, inofensivo pero el schema no calza) | Sin problema -- una columna `NOT NULL` siempre satisface un campo que ahora es opcional |
| Campo opcional → requerido | Falla al conectar (la columna física sigue nullable) | No se detecta al conectar; una fila vieja con `NULL` ahí da el mismo error de runtime limpio que la fila arriba (§3.68) |

`--adopt-existing` (§3.67) no cambia ninguna fila de esta tabla -- ese modo se salta el auto-migrate por completo, así que un desacuerdo de columna nueva/requerida se descubre siempre al conectar (nunca al leer), con su propio mensaje.

**El argumento de concurrencia de §3.16 se mantiene sin cambios.** Un `SELECT`/`INSERT` de `rusqlite` es una llamada de Rust sincrónica normal, sin `.await`, que corre entera en el hilo que la llama -- ni distinto de clonar un `Vec` en ese sentido. El single-threading del servidor sigue siendo el lock de `Db::subscribe`; lo único que cambia es cuánto tarda cada llamada (I/O real de disco), no si algo puede colarse en el medio. Se activa `PRAGMA busy_timeout` y, si el archivo lo permite (no aplica a `:memory:`), `journal_mode=WAL` -- higiene operativa que permite inspeccionar el archivo con `sqlite3` mientras el servidor sigue corriendo, sin cambiar ningún argumento de corrección.

**Verificado con un spike real ANTES de escribir el resto del código, no asumido de la documentación de `rusqlite`.** El riesgo real de esta ronda era `wasm32-wasip1` (el target del demo de `bin/wasm_demo.rs`): compilar el C de SQLite bundleado para WASI necesita un compilador C que apunte ahí (`wasi-sdk`), y no hay ninguna garantía de que "simplemente funcione". Investigación previa (release notes reales de `rusqlite`, no solo la crate en general) encontró evidencia concreta de soporte activo para `wasm32-wasip1 + bundled` desde la v0.33 -- confirmado corriendo el spike de verdad: **un único backend `rusqlite` sirve tanto para `linkc serve` (nativo) como para el demo wasm**, sin ningún fork `#[cfg(target_arch = "wasm32")]` en el código de la aplicación. `Db::seeded()`/tests usan `Connection::open(":memory:")` -- mismo código que un archivo real, SQLite trata ese string como su propio caso especial. Hallazgo real del spike, no anticipado: las variables de entorno `CC_wasm32_wasip1`/`AR_wasm32_wasip1` tienen que usar rutas con formato Windows (`C:/...`), NO el formato POSIX de Git Bash (`/c/...`) -- este segundo formato "parece" funcionar en una invocación directa de `clang` desde Bash (la conversión automática de rutas de MSYS lo arregla en ese caso puntual) pero falla en silencio cuando cargo lee la variable de entorno y spawnea `clang` él mismo (ese camino nunca pasa por la conversión de MSYS), con un error de `stdio.h no encontrado` que no tiene nada que ver con la causa real.

**Costo real, a propósito no escondido: rompe la política de "cero dependencias nuevas" documentada en 3 lugares del proyecto** (`session.rs`, `diagnostics.rs`, `codegen/validators_emit.rs`). Elegir esta feature de la lista de pendientes ya implicaba aceptar infraestructura real de DB (§3.12 ya lo enmarcaba así). Efecto colateral nuevo para cualquiera que compile el binario nativo: `rusqlite` con `bundled` necesita un compilador C disponible (ya presente en este entorno vía el toolchain GNU activo; una instalación MSVC sin Build Tools lo necesitaría de cero) -- mismo tipo de requisito que crates como `openssl-sys`/`ring` ya piden en el ecosistema Rust en general.

**Verificado de punta a punta contra el binario real, no solo con tests unitarios:** insertar un usuario por HTTP real, matar el proceso de `linkc serve`, volver a levantarlo con el mismo comando, y confirmar que el usuario sigue ahí sin haberlo vuelto a insertar (con el efecto colateral esperado y correcto: el segundo usuario insertado después del reinicio ya NO se vuelve `Admin`, porque `examples/users.link`'s regla de bootstrap mira si la colección está vacía, y ahora "vacía" significa de verdad "nunca tuvo datos", no "desde el último reinicio"). Por separado, cambiar el schema de una colección y apuntar al mismo archivo confirma el panic esperado, con el mensaje y el diff exactos, antes de aceptar ninguna conexión.

**Comportamiento exacto ante `SIGTERM`/matar el proceso** (PLAN.md §9.1, pedido explícito -- "necesitamos saber si un `pm2 restart` puede cortar una escritura a medias"). Confirmado leyendo el código, no adivinado: `linkc serve` no instala NINGÚN manejador de señales -- ni `SIGTERM` ni ningún otro (no hay ningún crate de señales entre las dependencias, ni código propio que las toque). Eso significa:
- **Sin drenado gracioso.** Un `SIGTERM` (o `kill`/`taskkill`/un `pm2 restart`) termina el proceso con el comportamiento DEFAULT del sistema operativo -- inmediato, sin esperar a que ninguna request en curso termine. Una request que estaba a mitad de procesarse cuando llega la señal simplemente se corta -- el cliente ve la conexión cerrada, no una respuesta completa.
- **Sin flush explícito, y no hace falta.** No hay ningún código que haga `PRAGMA wal_checkpoint` ni nada parecido al recibir la señal -- pero tampoco hace falta: cada escritura (`insert`/`applyPatch`/`delete`) es una única sentencia SQL en modo autocommit (nunca hay un `BEGIN`/`COMMIT` multi-sentencia abierto -- transacciones reales sobre varias escrituras `db.<c>` todavía no existen como feature del lenguaje, PLAN.md §8.2.3), así que para el momento en que un rpc de escritura devuelve 200 al cliente, esa fila YA es durable -- el WAL de SQLite (activado en `Db::new`, arriba) es precisamente lo que garantiza que un corte abrupto del proceso no corrompe ni pierde una escritura ya confirmada. Lo único que un `SIGTERM` puede cortar es una request que TODAVÍA no había terminado de escribir su respuesta -- nunca una que ya el cliente vio como exitosa.
- **Ningún timeout configurable de apagado**, porque no hay ningún periodo de gracia que configurar -- la terminación es inmediata por diseño (o más precisamente, por AUSENCIA de diseño: nadie intercepta la señal).

**Fuera de alcance, a propósito:**
- `delete`/`deleteWhere`/`findWhere` -- no existían al escribir esta sección; agregados en §3.18, que también corrige el mapeo de `id` de arriba.
- Migraciones reales tipo `ALTER TABLE` más allá de agregar una columna opcional nueva (drop/rename/retype/cambiar nullability) -- ver la matriz de arriba, fallan fuerte en vez de auto-migrar.
- Índices más allá de `id` -- no hace falta ninguno hoy porque el lenguaje no tiene ningún mecanismo de query además de `find(id)`/`all()`/`findWhere` + `.filter()` del lado interpretado.
- Acceso concurrente desde múltiples procesos `linkc serve` al mismo archivo -- `busy_timeout`/WAL mitigan, no se verifica exhaustivamente.
- Cualquier motor que no sea SQLite (Postgres/MySQL) -- decisión explícita arriba, no una limitación técnica de último momento.

### 3.18 CRUD real: `delete`/`deleteWhere`/`findWhere` sobre `db` — RESUELTO

§3.17 persistió el CRUD que ya existía (`all/find/insert/applyPatch`) pero no agregaba superficie nueva. Esta ronda sí: `delete(id: Int) -> Bool` (borra por id, `false` si no existía), `deleteWhere(fn(T) -> Bool) -> Int` (borra cada fila que matchea, devuelve cuántas) y `findWhere(fn(T) -> Bool) -> T[]` (mismo predicado, sin borrar) -- mismo espíritu que `.filter()` de `List` (§3.10), ahora también sobre una colección de `db`.

**Dónde vive de verdad la evaluación del predicado -- y por qué no puede vivir en `Db::call`.** `deleteWhere`/`findWhere` reciben un closure de usuario (`fn(T) -> Bool`) que hay que invocar una vez por fila. `Db::call` (en `runtime/db.rs`) es la capa que sabe hablar SQL, pero no tiene acceso a `call_callable` ni al `Env`/`fns`/sesiones que evaluar un closure necesita -- esa información vive en el intérprete (`runtime/mod.rs`), no en la capa de storage. Por eso la implementación real intercepta ambos métodos en `call_method` (mismo punto que ya redirigía `List::filter`/`List::map` a su propia lógica) *antes* de que la llamada llegue a `Db::call`: trae todas las filas con `all`, evalúa el predicado real fila por fila con `call_callable`, y para `deleteWhere` borra cada fila que matcheó a través del `delete` ya persistente (así que también publica, ver abajo). `Db::call` conserva sus propios brazos `"deleteWhere"`/`"findWhere"`, pero ahora devuelven un error explícito en vez de intentar algo -- son inalcanzables desde el intérprete normal (que siempre pasa por `call_method` primero), pero como `Db::call` es `pub fn`, quedan invocables directo (tests, LSP, código futuro); antes de esta ronda, esos dos brazos existían con la implementación INGENUA e incorrecta (`deleteWhere` ignoraba el predicado y borraba TODAS las filas; `findWhere` ignoraba el suyo y devolvía TODAS) -- exactamente el tipo de resultado que parece válido y no lo es, ahora reemplazado por un error claro que nombra el problema.

**`delete` ahora publica a los suscriptores.** Antes de esta ronda, `delete` quitaba la fila de SQLite pero nunca llamaba a `Db::publish` -- un `stream` con `while true { db.<col>.subscribe() }` (§3.16) nunca se enteraba de un borrado, solo de inserts. Ahora publica la fila borrada igual que `insert`/`applyPatch` ya hacían, así que un suscriptor ve el borrado como un evento más sobre el mismo wire SSE, sin ningún cambio de protocolo.

**`id` gana `AUTOINCREMENT`** (tabla de columnas en §3.17) -- con `delete` real, insertar después de borrar la última fila reusaría su id bajo el `INTEGER PRIMARY KEY` liso de antes; `AUTOINCREMENT` cierra esa ventana.

**Fuera de alcance, a propósito:**
- `deleteWhere`/`findWhere` traen SIEMPRE la colección entera a memoria antes de filtrar (vía `all`) -- correcto para el volumen de datos de v0, no pensado para una tabla grande; no hay traducción de predicado a `WHERE` de SQL.
- Sin transacción envolvente en `deleteWhere`: cada borrado es su propio `DELETE` -- una falla a mitad de camino deja borrado un prefijo, no ninguna o todas.

### 3.19 Protocolo LSP real (`linkc lsp`) — RESUELTO, Nivel 1+2

Los 3 prerrequisitos (spans+columna real, recuperación de errores del parser, spans en todo el AST/checker -- ver los tres "Done" de LSP en README.md) dejaban listo el terreno; esta ronda escribe el servidor en sí. `linkc lsp` habla JSON-RPC 2.0 sobre stdio con framing `Content-Length` estándar, y responde `initialize` anunciando `textDocumentSync: Full`, `hoverProvider`, `completionProvider` y `definitionProvider`.

**Diagnósticos con imports resueltos de verdad, no un buffer aislado.** `didOpen`/`didChange`/`didSave` arman un overlay en memoria (`HashMap<PathBuf, String>`, ruta canonicalizada como clave) con TODOS los documentos actualmente abiertos -- no solo el que cambió, porque un archivo importado puede estar abierto en otra pestaña -- y re-chequean a través de `modules::load_program_with_overlay` (el `Program` fusionado, siguiendo `import` de verdad) más `checker::Checker::check_program_full`. Antes de esta conexión, cada request re-tokenizaba/re-parseaba el buffer aislado con `lexer::tokenize`+`parser::parse` directo, así que cualquier archivo con `import` daba "no declarado" en falso -- el símbolo importado nunca se resolvía. Cuando `uri` no corresponde a un archivo real en disco (un buffer `untitled:` nunca guardado, fuera de alcance en v0), cae de vuelta al chequeo aislado de antes en vez de no publicar nada.

**Atribución multi-archivo: igual de honesta que la CLI, mejor en un caso.** Un `LoadError::Syntax{path, errors}` ya tiene identidad de archivo real incluso cruzando imports (se captura antes del merge) -- si `path` no es el documento abierto, el mensaje lo nombra explícitamente en vez de fingir que el error está en el buffer actual. Un `CheckError`, en cambio, no tiene identidad de archivo tras el merge -- mismo gate que `main.rs::report_check_errors` ya usaba (`touched.len() == 1`): con un solo archivo en el cierre transitivo, rango preciso; con más de uno, todos los mensajes se publican igual (nunca esconder que algo está mal) anclados en una posición degradada que aclara la imprecisión, en vez de arriesgar una heurística que adivine mal el archivo.

**`span_to_range`: multi-línea y UTF-16 de verdad, no una suposición heredada.** `diagnostics.rs` (el renderer de la CLI) asume que un span nunca cruza una línea porque ahí solo hay UNA línea ya extraída para trabajar -- una suposición razonable ahí, pero el LSP tiene el documento COMPLETO disponible y puede hacerlo bien: cuenta saltos de línea reales entre `span.start` y `span.end` para la línea de fin, y sobre cada char usa `char::len_utf16()` para la columna en unidades UTF-16 (lo que el wire de LSP pide, no un conteo crudo de chars) -- necesario porque los spans de declaración (`TypeDecl`/`FnDecl`/`ServiceDecl`, los que hover usa) son rutinariamente multi-línea en código real.

**Hover/completion/goto-def -- Nivel 2: a nivel de declaración, no sensible a posición.** Hover reconoce palabras clave/tipos builtin y, si el cursor cae sobre un nombre declarado (`type`/`enum`/`service`/`rpc`/`fn`/`const`/colección de `db`), muestra un resumen de ESA declaración -- ahora resuelta contra el `Program` fusionado, así que también funciona sobre un símbolo usado pero declarado en otro archivo. Completion da una lista plana (palabras clave + nombres de nivel superior, más el listado de colecciones tras `db.`) igual en cualquier posición del cursor -- deliberadamente no sensible a posición. Goto-definition busca una referencia en posición de valor por nombre sobre el mismo `Program` fusionado. Explícitamente fuera de alcance, documentado, no escondido: completion sensible a posición después de `x.` (necesitaría reconstruir el `Env` de tipos en el punto exacto del cursor, una tercera función de recorrido paralela a `check_expr`/`synth_expr`, Nivel 3, ronda futura), hover de una expresión arbitraria en medio de un body, goto-def de un nombre de TIPO escrito en una firma (`TypeExpr` no tiene span propio), documentos `untitled:`, sync incremental, multi-root workspaces, `$/cancelRequest`.

**Transporte hand-rolled, no `lsp-server`/`lsp-types`.** La investigación previa a esta ronda había elegido esos dos crates (mantenidos por rust-analyzer) para el framing/dispatch -- en la implementación real terminó siendo un loop propio, chico, sobre `io::stdin()`/`io::stdout()` (parseo de `Content-Length`, un `match` sobre `method`), sin ninguna dependencia nueva. Funciona y está cubierto por tests reales; los dos crates se sacaron de `Cargo.toml` por quedar sin ningún consumidor, en vez de dejarlos declarados sin usar. Documentado como divergencia consciente del plan original, no como algo pendiente de corregir -- si en el futuro hace falta algo que el loop propio no cubre bien (p. ej. `$/cancelRequest` real), migrar a `lsp-server` sigue siendo una opción.

**Aislamiento de errores: `catch_unwind` alrededor de cada re-chequeo.** `linkc lsp` es un proceso de LARGA VIDA de un solo hilo -- un panic sin capturar dentro de `load_program_with_overlay`/`check_program_full` (hoy sin ningún caso conocido alcanzable desde texto inválido, pero un checker que sigue creciendo puede introducir uno) terminaría el proceso entero, tirando abajo el servidor para TODOS los documentos abiertos por un solo archivo problemático. `compute_diagnostics_for`/`full_program_for` envuelven su lógica en `std::panic::catch_unwind` -- un panic capturado se loggea a stderr (el canal de Output de un cliente LSP real, VS Code incluido) y degrada a un único diagnóstico (o a `None`, cayendo al chequeo aislado del buffer) en vez de propagar. `&LspServer` no tiene mutabilidad interior, así que es `UnwindSafe` sin necesitar `AssertUnwindSafe` -- verificado con un test que fuerza un panic sintético con el mismo patrón exacto de captura, dado que no existe hoy un input real que dispare uno.

**Bug real, encontrado en un reparso general (no en uso real): un framing corrupto dejaba el server colgado en silencio, para siempre.** `run_stdio` parsea los headers línea por línea buscando `Content-Length`; si faltaba o no era numérico, el código original hacía `continue` de vuelta al tope del loop -- pero los bytes del BODY de ese mensaje mal formado nunca se leían, así que quedaban sin consumir en el stream. La próxima vuelta del loop de headers los interpretaba como si fueran líneas de header (nunca lo son, así que nunca encuentra un `Content-Length` válido tampoco) -- un desync PERMANENTE: el server dejaba de responder a TODO lo que viniera después de ese único mensaje roto, sin ningún error, indistinguible de un proceso colgado desde el lado del editor. No hay forma confiable de "resincronizar" sin saber cuántos bytes saltar -- ese largo es exactamente el dato que falta o es inválido -- así que el fix trata esto como lo que es: un error fatal de conexión, no una condición recuperable. `run_stdio` ahora devuelve `Err` ahí mismo (`cmd_lsp`, `main.rs`, ya traducía cualquier `Err` de `run_stdio` a un mensaje en stderr + código de salida distinto de cero -- no hizo falta tocar esa parte). Verificado con un test que manda un `Content-Length` no numérico a mano contra el binario real y espera la salida del proceso con un timeout propio (`compiler/tests/lsp_stdio.rs`) -- necesario porque un `child.wait()` sin cota hubiera colgado el TEST también si el bug hubiera seguido ahí.

**Gap de conformidad más chico, mismo reparso: un request roto se perdía en silencio, sin tumbar la conexión.** Distinto del bug de framing de arriba -- acá el mensaje SÍ estaba bien delimitado (`Content-Length` correcto, JSON válido), pero le faltaba `"method"` (o no era un string). `handle_message` hacía `req.get("method")?.as_str()?`, devolviendo `None` sin mirar si el mensaje tenía `"id"` -- indistinguible, para el código, de una notificación rota (que correctamente no espera respuesta) de un REQUEST roto (que un cliente real sí está esperando responder). Menos grave que el bug de framing -- la conexión sigue sana, el próximo mensaje bien formado responde normal -- pero silencio ahí sigue dejando a un cliente esperando para siempre una respuesta a ESE id puntual. Ahora, si el mensaje tiene `"id"`, se responde con un error JSON-RPC 2.0 explícito (`code: -32600`, "Invalid Request") en vez de nada; sin `"id"` (notificación), sigue sin responder, que es lo correcto. Deliberadamente acotado a este único punto de entrada -- CADA rama de método de más abajo (`didOpen`, `hover`, etc.) tiene sus propios `?` internos sobre campos de `params` específicos, y llevar los mismos a un error explícito por-método es una extensión de conformidad más grande, no incluida en esta ronda; ver los dos tests nuevos en `lsp.rs` (`test_a_request_with_an_id_but_no_method_gets_an_explicit_error_not_silence` / `..._is_silently_ignored`) para el comportamiento exacto de este único punto que sí se cerró.

**Verificado en dos capas, no solo in-process.** Los tests unitarios de `lsp.rs` llaman `handle_message` directo, sin ningún proceso de por medio. `compiler/tests/lsp_stdio.rs` agrega una segunda capa que sí importa: spawnea el binario `linkc` compilado de verdad (`env!("CARGO_BIN_EXE_linkc")`) con el arg `lsp`, escribe bytes con framing `Content-Length` real a su stdin, y lee la respuesta de vuelta de su stdout -- cubriendo el buffering real de pipes de sistema operativo (particularmente en Windows) que una llamada a función in-process no puede. Incluye el mismo caso de import válido entre dos archivos reales que `lsp.rs` ya prueba in-process, ahora también contra el binario.

Cliente de referencia real en `editors/vscode/` (extensión mínima que spawnea `linkc lsp` y conecta un `LanguageClient` para archivos `.link`).

### 3.20 Codegen WASM nativo v0 (`linkc wasm`) — RESUELTO, alcance mínimo

Distinto del target WASM que ya existía (`compiler/src/bin/wasm_demo.rs`, que recompila el intérprete ENTERO a `wasm32-wasip1` -- sigue siendo el camino real/de producción): `linkc wasm <archivo.link> <salida.wasm>` (y, como efecto colateral best-effort de `linkc build`, un `main.wasm` junto a `contract.d.ts`/`client.ts`/`validators.ts`) genera bytecode WASM DIRECTO por función vía `wasm-encoder`, sin pasar por el intérprete en absoluto -- el experimento de codegen nativo que la fila de Fase 1 en PLAN.md §4 nombraba como evolución futura.

**Alcance real: aritmética/comparación entera sobre `Int`/`Bool`, una sola expresión final, nada más.** Todo parámetro y tipo de retorno tiene que ser `Int` o `Bool` (ambos representados como `i64`, `Bool` como 0/1) -- cualquier otro tipo (`String`, `Float`, un struct, un enum, `T?`, `T[]`, `Map<K,V>`, ...) no tiene representación en este esquema. El cuerpo de la función tiene que ser exactamente una expresión final (`Int`/`Bool` literal, un identificador que sea un parámetro, `+ - * / % == != < > <= >=` sobre esas, y paréntesis) -- nunca una sentencia (`let`/asignación/`if` como sentencia/`while`).

**Fuera de ese subconjunto, siempre falla explícito -- nunca antes.** La primera versión de este codegen (de una sesión externa al repo) reemplazaba silenciosamente CUALQUIER construcción no soportada por `I64Const(0)`, e ignoraba por completo las sentencias de un bloque (`emit_block` solo miraba la cola) -- `linkc wasm`/`linkc build` reportaban éxito mientras el `.wasm` generado calculaba otra cosa. Ahora `emit_expr`/`emit_block` devuelven `Result`, y cualquier construcción fuera del subconjunto de arriba (un parámetro `String`, un operador lógico `&&`/`||`, una sentencia `let` en el cuerpo, una llamada, un `match`, ...) hace fallar la emisión con un mensaje que nombra la función y el problema exacto. En `linkc build`, esto es una ADVERTENCIA, no un fallo del build entero: `contract.d.ts`/`client.ts`/`validators.ts` son las salidas de las que el resto del proyecto depende; `main.wasm` es un artefacto secundario best-effort, y casi ningún programa real (empezando por `examples/users.link`, que usa `String`/structs/`db`) cae dentro del subconjunto soportado hoy -- el mensaje de éxito de `linkc build` solo nombra `main.wasm` cuando de verdad se escribió.

**Fuera de alcance, a propósito -- v0 mínimo, no un backend de codegen general:** cualquier sentencia dentro de un cuerpo (locals, control de flujo compilado a bloques/loops/branches WASM); `String`/`Float`/structs/enums/`Optional`/`List`/`Map`/`Union`/`Result`/`Patch`; llamadas entre funciones dentro del módulo emitido; `db`/sesiones/streaming (no tienen sentido fuera del intérprete). Cerrar esta brecha de verdad (soportar un programa real como `users.link`) es una ronda propia, del tamaño aproximado de esta, no una extensión incremental.

**Decisión de roadmap (auditoría post-push): congelado a propósito, no una brecha a cerrar.** `wasm32-wasip1` (recompilar el intérprete entero, `compiler/src/bin/wasm_demo.rs`) es y sigue siendo el ÚNICO camino real de producción -- ya corre un programa REAL (`Users.getById` de punta a punta dentro de `wasmtime`, PLAN.md §2.4), mientras que cerrar la brecha de `linkc wasm` hasta soportar algo comparable (statements, `String`/structs/`db`, llamadas entre funciones) es, en la práctica, escribir un backend de codegen nativo completo desde cero -- meses de trabajo, no una ronda más. `linkc wasm` se queda tal como está: un experimento honesto, correctamente acotado y con tests, sin plan de extenderlo -- no se retira (el código y sus tests siguen siendo correctos para lo que documentan soportar) pero tampoco se lo trata como el Fase 1 "codegen directo vía `wasm-encoder`" pendiente de crecer que PLAN.md §4 todavía sugería. Si en el futuro hace falta codegen nativo de verdad (no vía intérprete), la recomendación sigue siendo `cranelift-jit`/`cranelift-object` (PLAN.md §2.4) sobre expandir esto -- son herramientas hechas para ESO, `wasm-encoder` es autoría de bajo nivel pensada para emitir bytecode ya decidido, no un framework de compilación con manejo de locals/control de flujo/calling conventions.

---

### 3.21 LSP Nivel 3 (Ronda 1/3): goto-definición de un nombre de tipo en una firma — RESUELTO

§3.19 dejó 3 gaps documentados como "Nivel 3, fuera de alcance a propósito": completion sensible al tipo real del receptor tras `x.`, hover de una expresión arbitraria en medio de un body, y goto-def de un nombre de TIPO escrito en una firma (ej. `Point` en `fn origin() -> Point`) -- bloqueado porque `TypeExpr`/`Param`/`Field` no tenían span propio. Esta ronda resuelve el tercero. Investigado con 2 agentes de Plan en paralelo (Nivel 3 mínimo vs. completo) que coincidieron en que los 3 ítems NO son una sola ronda: comparten una utilidad de conversión de posición y un principio (reusar el checker/AST existente en vez de duplicar lógica), pero el ítem de goto-def vive enteramente en la capa sintáctica (`ast.rs`+`parser.rs`, sin `Checker`) mientras los otros dos necesitan reconstruir el `Env` de tipos del checker -- una traversal genuinamente más cara, y su propia ronda futura (orden recomendado: hover antes que completion, porque completion termina reusando la misma máquina que hover construye).

**`TypeExpr::Named` gana un tercer campo, `Span` -- las otras 7 variantes no.** Verificado con grep en todo el crate: `TypeExpr::Named` se construye en 2 sitios de producción (`parser.rs::parse_primary_type`, y una construcción sintética en `checker.rs::synth_struct_lit` sin texto fuente real) y se destructura en 2 (`checker.rs::resolve_named_type_subst`, `codegen/wasm_emit.rs::wasm_scalar_type`) -- nada parecido a los ~155 sitios que la migración a `Spanned<Expr>`/`Spanned<Stmt>` tocó en su momento (Ronda A). La razón de fondo: de las 8 variantes de `TypeExpr`, solo `Named` corresponde a un identificador ESCRITO al que alguien pediría saltar -- `Struct`/`Map`/`Tuple`/`Function`/`Optional`/`List`/`Union` son combinadores sintácticos sin nombre propio (el `Int` dentro de `Int[]` ya es su propio `Named` anidado), así que no se repitió el patrón `Spanned<T>` para todo el enum. `TypeExpr` sacó `PartialEq` del derive y lo reimplementa a mano ignorando el span en el brazo de `Named` (mismo criterio que `Spanned<T>` ya resolvía) -- si no, dos `Named("Int", vec![])` en offsets distintos hubieran dejado de ser `==`, rompiendo en silencio los tests existentes que comparan `TypeExpr` por igualdad.

**La búsqueda es puramente sintáctica -- sin `Checker`, sin `Env`.** `find_named_type_in_program` (`compiler/src/lsp.rs`) recorre, para cada ítem del programa, exactamente los mismos lugares donde un `Field`/`Param`/`return_type` puede aparecer -- `type`/`db`/variantes de `enum` (sus `Field`s), y `Param`s + `return_type` de `fn`/`rpc`/`stream` -- exactamente los spans de FIRMA que `FnDecl.span`/`RpcDecl.span` ya cubrían desde el prerrequisito 3/3 original del LSP (firma completa, nunca el body), así que esta búsqueda nunca se solapa con hover/completion de Nivel 2 ni con lo que Nivel 3 (ítems 1/2) construirá más adelante dentro de un body. `find_named_type_at` es exhaustiva sobre las 8 variantes de `TypeExpr` -- sin brazo `_` a propósito, para que agregar una variante nueva rompa la compilación acá en vez de que la búsqueda la ignore en silencio -- y prioriza los `args` de un genérico antes que el propio nombre, para que el cursor en `Line` dentro de `Box<Line>` resuelva a `Line`, no al `Box` que lo envuelve.

**Integración en `get_definition`: autoritativa cuando dispara, nunca cuando no.** Si el offset del cursor cae dentro de un `TypeExpr::Named`, la respuesta viene de esta búsqueda -- la declaración `type`/`enum` encontrada, o `None` si el nombre es un builtin (`Int`/`String`/...) o un parámetro de tipo genérico sin declaración propia -- y NUNCA cae al loop viejo de coincidencia-por-palabra de Nivel 2. Esto evita un falso positivo real y concreto: un tipo builtin usado en una firma (`fn f() -> Int`), si además existe un `const`/`fn`/`service` con el mismo nombre en otro namespace (`const Int: Bool = true;`), el loop viejo saltaría (mal) a ESE otro ítem por pura coincidencia de texto -- la búsqueda nueva, al confirmar que el offset SÍ es un uso de tipo, responde `None` ella misma en vez de dejar que el loop viejo adivine.

**Límite honesto, encontrado escribiendo los tests de esta misma ronda, no anticipado en el diseño original: esto NO protege contra un `Field`/`Param` cuyo NOMBRE (no su tipo) coincide textualmente con una declaración `type`/`enum` existente** (ej. `type Point = {...}; type Shape = { Point: Int }` -- pedir goto-def sobre el nombre de CAMPO `Point` sigue cayendo al loop viejo, que salta al `type Point`, porque el cursor ahí no está dentro de ningún `TypeExpr::Named` en absoluto). La causa es la misma que ya limita a Nivel 2: `Field`/`Param` no tienen span propio, así que no hay forma de que la búsqueda nueva sepa "este offset es un NOMBRE de campo, no un tipo" sin agregarles uno -- fuera de alcance de esta ronda a propósito (agregar el span de `Named` alcanzaba para el objetivo principal; agregar spans a `Field`/`Param` sería una extensión aparte, del mismo tamaño que esta, no una consecuencia gratis).

**Bug real encontrado en la investigación, corregido como corequisito (no un cuarto ítem de Nivel 3, un defecto de Nivel 2 que esta ronda expuso al agregar el primer test cross-file de `get_definition`):** `full_program_for` fusiona todo el cierre transitivo de imports en un solo `Program`, descartando `touched` -- así que un símbolo resuelto vía un archivo IMPORTADO tenía un `Span` con offsets de ESE otro archivo, pero `get_definition` devolvía `"uri": uri` (el documento ABIERTO) con el rango calculado sobre `source` (el texto del documento abierto). Resultado: el editor no navegaba a ningún lado útil. Mismo gate que `compute_diagnostics_for_inner` ya usa para diagnósticos (`touched.len() <= 1`): con más de un archivo tocado, `get_definition` devuelve `None` para CUALQUIERA de las dos búsquedas (nunca arriesga una posición que puede estar en el archivo equivocado) en vez de intentar el caso general, que necesitaría que `modules.rs` etiquete cada `Item` con su archivo de origen -- cambio genuinamente más grande, no pedido acá.

**Fuera de alcance, a propósito:**
- Anotaciones de tipo DENTRO de un cuerpo (`let x: Point`, tipo de un parámetro de closure) -- mismo tipo de búsqueda aplicada desde otro punto de partida sería la extensión natural, no incluida acá.
- El límite de `Field`/`Param` sin span descrito arriba.
- Los ítems 1 (completion sensible a `x.`) y 2 (hover de expresión arbitraria) de Nivel 3 -- rondas futuras separadas; el orden recomendado es hover antes que completion (completion termina siendo un superconjunto de la misma máquina de "reconstruir `Env` + ubicar nodo" que hover necesita construir primero) y ninguno de los dos depende de esta ronda.

---

### 3.22 Identidad de archivo en `Span` — RESUELTO

§3.21 documentó como bug real (no un ítem de Nivel 3 en sí) que `get_definition` cruzando archivos se negaba en bloque (`touched.len() <= 1`) porque un `Span` del `Program` fusionado no decía de qué archivo real venía -- podía ser de un `import`. Esta ronda cierra ese gap de fondo, con el mismo mecanismo sirviendo a LSP y CLI a la vez.

**`item_files: Vec<PathBuf>`, un archivo por ÍTEM, no por span individual.** `modules::load_program_with_overlay` ahora devuelve una tercera pieza junto a `Program`/`touched`: un `Vec<PathBuf>` del mismo largo y orden que `Program.items`, poblado en `Loader::visit` en el mismo `push` que ya llenaba `merged`. La idea central: un `Item` (`type`/`enum`/`fn`/`service`/`const`) nunca se parte entre dos archivos, así que CUALQUIER `Span` anidado a cualquier profundidad dentro de ese ítem -- firma, body, una sub-expresión -- pertenece al mismo archivo que el ítem completo. No hace falta razonar sobre RANGOS de offsets (ambiguos entre archivos: el offset 200 puede existir válidamente en dos archivos de 500 bytes cada uno) ni requiere ningún cambio en `ast.rs`/`parser.rs` -- alcanza con trackear el archivo por ítem, en el único lugar (`modules.rs`) que ya sabía cuál era.

**El checker estampa el archivo en el mismo punto donde ya estampaba el span.** `checker::CheckError` gana `file: Option<PathBuf>` (mismo patrón que `span: Option<Span>`, pero sin la semántica "primer stamp gana" de `with_span` -- el archivo es constante para todo el subárbol de un ítem, nunca "más específico" según la profundidad). `check_program_full` gana un segundo parámetro, `item_files: &[PathBuf]`, y en cada uno de sus 5 puntos de entrada (`Item::Fn`, los 3 chequeos por `rpc`/`stream` de `Item::Service`, `Item::Const`) ahora itera con `.enumerate()` y estampa `item_files[index]` junto al span. `check_program` (pública, sin `Checker` ni archivos) pasa `&[]` -- `item_files.get(i)` da `None` para cualquier índice, así que los ~113 call sites de error existentes y los tests que arman un `Program` a mano quedan bit-a-bit iguales. `check_program_with_files` es la nueva fachada pública para callers que SÍ tienen `item_files` (no pueden llamar a `check_program_full` directo: es `pub(crate)` de la librería, invisible desde el crate binario aunque compartan paquete Cargo).

**LSP: goto-definición cruza archivos de verdad.** `get_definition`/`get_definition_inner` ganan `item_files: &[PathBuf]` y `overlay: &HashMap<PathBuf, String>`. Un `respond(index, span)` interno resuelve, para el ítem encontrado, si su archivo real coincide con el documento abierto (`uri`, camino rápido de siempre) o es OTRO archivo -- en ese caso arma la respuesta con el `uri` y el rango calculados sobre el archivo REAL (leído del overlay del editor o de disco), en vez de negarse. `item_files` vacío (el buffer aislado de un test o un documento sin resolver vía `modules.rs`) preserva el comportamiento exacto de antes: todo pertenece a `uri`/`source`. El bug concreto que esto arregla, confirmado con un test de subproceso real contra el binario: goto-def sobre un tipo importado ahora devuelve `{uri: "file:///.../b.link", range: {line: 0, ...}}` en vez de `null`.

**LSP: diagnósticos de tipos ya no degradan el programa ENTERO por tocar más de un archivo.** Antes, `touched.len() > 1` hacía caer TODOS los errores de un programa a un único diagnóstico en posición (0,0) con un mensaje genérico ("podría estar en uno de los N archivos importados") -- aunque el 100% de los errores estuviera en el documento abierto. Ahora cada `CheckError.file` se compara contra el documento que disparó el chequeo: si coincide, snippet con rango real de siempre; si no (vino de un `import`), el protocolo LSP no da forma de apuntar una posición de OTRO archivo dentro de la respuesta de `publishDiagnostics` para `uri` -- así que se nombra el archivo real en el mensaje (mismo criterio que ya usan los errores de sintaxis de `LoadError::Syntax`) en vez de esconder cuál de los N archivos era. Publicar diagnósticos para MÚLTIPLES uris en una sola notificación (el arreglo completo) queda fuera de esta ronda -- ver "Fuera de alcance" abajo.

**CLI: el mismo mecanismo, sin la limitación de "una sola uri".** `main.rs::report_check_errors` no tiene el problema de protocolo del LSP (escribe a stderr, no responde a un `uri` puntual), así que acá el arreglo es completo: CUALQUIER error, en CUALQUIER archivo tocado, ahora sale con su snippet real -- confirmado con `linkc <archivo>` de verdad, tanto para un error en el archivo de entrada como en uno importado (`compiler/tests/cli_multifile_diagnostics.rs`, subproceso real). `report_check_errors` cachea las lecturas de disco por archivo (`source_cache`) en vez de releer el mismo archivo por cada error que le pertenece.

**Fuera de alcance, a propósito:**
- Publicar `publishDiagnostics` para múltiples URIs desde un solo re-chequeo (lo que daría rango real también para un error en un archivo importado, no solo el mensaje nombrándolo) -- requiere trackear y limpiar diagnósticos de archivos que dejan de estar en el cierre transitivo entre un chequeo y el siguiente; una ronda propia, más grande que "adjuntar identidad de archivo".
- Los ítems 1 y 2 de Nivel 3 (§3.21) siguen sin empezar -- no dependen de esta ronda.
- El límite de `Field`/`Param` sin span (§3.21) sigue igual -- resuelto en la siguiente ronda (§3.23), no acá.

---

### 3.23 `Field`/`Param` ganan `name_span` — RESUELTO

§3.21 dejó documentado, como límite honesto encontrado escribiendo sus propios tests: un campo o parámetro cuyo NOMBRE coincide textualmente con una declaración `type`/`enum` existente (`type Point = {...}; type Shape = { Point: Int }`) seguía cayendo al loop viejo de coincidencia-por-palabra al pedir goto-def sobre el nombre de CAMPO `Point` -- saltaba (mal) a `type Point`, porque el cursor ahí no caía dentro de ningún `TypeExpr::Named`. Esta ronda lo cierra.

**Un solo campo nuevo por struct, verificado con grep: exactamente 2 sitios de producción.** `Field` y `Param` ganan `name_span: Span`, cubriendo SOLO el identificador del nombre (mismo criterio que `TypeExpr::Named`, §3.21). A diferencia de la migración `Spanned<Expr>`/`Spanned<Stmt>` (~155 sitios) o incluso `TypeExpr::Named` (~4 sitios), acá hubo literalmente UN sitio de producción real por tipo (`parser.rs::parse_field`, `parser.rs::parse_param`) -- `ClosureParam` (parámetros de un closure `|params| {...}`) queda deliberadamente afuera, es un tercer tipo de parámetro distinto y el bug reportado nunca lo mencionaba. Capturar el span es `let name_span = self.span();` inmediatamente ANTES de `self.eat_ident()` -- mismo patrón que `parse_primary_type` ya usaba para `TypeExpr::Named`, válido porque `eat_ident` no saltea nada antes del identificador.

**`PartialEq` manual, mismo motivo que `TypeExpr` ya tenía.** `Field`/`Param` sacaron `PartialEq` del derive automático (que hubiera empezado a comparar `name_span`, rompiendo en silencio cualquier comparación estructural existente -- ej. `TypeExpr::Struct`'s propio `PartialEq` compara `Vec<Field>` elemento a elemento, y `FnDecl`/`RpcDecl` hacen lo mismo con `Vec<Param>`) y lo reimplementan a mano ignorando `name_span`, exactamente el mismo patrón que `TypeExpr::Named` ya resolvía para su propio span. Cero tests rotos por esto -- confirmado corriendo la suite completa antes y después: los tests existentes ya construían programas parseando texto fuente real, no armando `Field`/`Param` a mano, así que el nuevo campo no tocó ningún sitio de construcción fuera de los 2 de arriba.

**LSP: `is_field_or_param_name_at` + `field_name_at_in_type`, mismo criterio de exhaustividad que `find_named_type_at`.** Dos funciones nuevas en `lsp.rs`, sin brazo `_` en ninguna (agregar una variante de `TypeExpr` rompe la compilación acá, no se ignora en silencio): `field_name_at_in_type` recorre las 8 variantes buscando un `TypeExpr::Struct` en cualquier profundidad (un genérico puede envolver un struct inline, `Box<{ n: Int }>`) y chequea el `name_span` de sus campos; `is_field_or_param_name_at` aplica eso sobre exactamente los mismos lugares que `find_named_type_in_program` ya recorre (`Field` de `type`/`db`/variantes de `enum`, `Param` de `fn`/`rpc`/`stream`). En `get_definition_inner`, corre como un SEGUNDO gate autoritativo, inmediatamente después del de `TypeExpr::Named` (§3.21) y antes del loop viejo: si el offset cae sobre el nombre de un campo/parámetro, responde `None` directamente -- un nombre de campo no es una referencia a otro símbolo (a diferencia de su TIPO, que el primer gate ya resuelve), así que no hay ninguna declaración a la que saltar.

**Verificado que el gate nuevo no es sobre-amplio.** Además del caso que arregla (`test_goto_def_on_a_field_name_that_collides_with_an_existing_type_name_does_not_jump`, `test_goto_def_on_a_param_name_that_collides_with_an_existing_type_name_does_not_jump`), un test cubre la contraparte exacta con el MISMO código: pedir goto-def sobre el TIPO de un campo cuyo nombre coincide con ese mismo tipo (`type Marker = {...}; type Shape = { Marker: Marker }`, cursor sobre el segundo `Marker`) sigue resolviendo a `type Marker` como siempre -- el gate nuevo distingue nombre de campo vs. uso de tipo en vez de tragarse ambos. 345 tests, todos pasando.

**Fuera de alcance, a propósito:**
- `ClosureParam` (parámetros de closure) no ganó `name_span` -- el bug reportado en §3.21 solo mencionaba `Field`/`Param`; si aparece el mismo problema ahí, es una extensión de tamaño similar, no una consecuencia gratis de esta ronda.
- Los ítems 1 y 2 de Nivel 3 (§3.21: hover de expresión arbitraria, completion sensible a `x.`) siguen sin empezar -- resueltos/en curso en §3.24 y §3.25 respectivamente.
- Publicar `publishDiagnostics` multi-URI (§3.22) sigue sin empezar.

---

### 3.24 Hover de expresión arbitraria — RESUELTO, LSP Nivel 3 ronda 2/3

§3.21 dejó esta ronda como "la más cara" del Nivel 3 -- reconstruir el `Env` del checker en vez de una búsqueda puramente sintáctica como las rondas anteriores (§3.21, §3.23). Se investigó primero un diseño alternativo (reimplementar en `lsp.rs` el recorrido de scoping -- params, `let`, bloques de `if`/`match`/closures -- para reconstruir el `Env` activo en un offset "desde afuera") y se descartó: hubiera duplicado ~150-300 líneas de reglas que YA viven en `check_stmt`/`check_block`/`bind_pattern`, con el riesgo real de que diverjan con el tiempo (dos fuentes de verdad para las mismas reglas de scoping). El diseño elegido reusa el checker de verdad, sin reimplementar nada de scoping.

**El "probe" vive en los DOS puntos de entrada unificados de expresión, no en cada `synth_*`/`check_*` interno.** Absolutamente toda expresión del programa pasa por `synth_expr` (modo síntesis, ⇒) o por `check_expr` (modo chequeo, ⇐) en algún momento -- son los dos wrappers públicos que `synth_expr_inner`/`check_expr_inner` (y los ~15 `synth_*`/`check_*` especializados que delegan en ellos) nunca bypasean. Instrumentar esos DOS puntos (no los ~15 internos) alcanza para cubrir el árbol completo: `Checker` gana `hover_target: Option<usize>` (el offset a buscar, `None` en cualquier chequeo normal) y `hover_result: RefCell<Option<(ancho_del_span, Type)>>` (interior mutability porque el checker entero opera con `&self`, nunca `&mut self` -- agregarlo hubiera tocado los ~40 call sites de `check_expr`/`synth_expr`). `synth_expr` guarda el tipo SINTETIZADO cuando su span contiene el offset; `check_expr` guarda `expected` (no hay tipo sintetizado propio en modo chequeo -- pero si el chequeo tuvo éxito, `expected` es por construcción un tipo válido para esa expresión, ej. un `if`/`match`/closure).

**Bug real evitado ANTES de implementarlo, analizando el orden de recursión:** la primera versión de este diseño consideraba "última escritura gana" (sobreescribir sin guardas) -- INCORRECTO. Un nodo padre (ej. `x > 5`) siempre tiene un span que CONTIENE al de sus hijos (`x`, `5`), y el padre termina de procesarse DESPUÉS de que sus hijos ya retornaron (la recursión entra a los hijos antes de que el padre calcule su propio resultado) -- así que "última escritura" se hubiera quedado con el nodo MÁS EXTERNO que contiene el offset, no el más específico: hoverear sobre `x` en `x > 5` hubiera mostrado `Bool` (el tipo de toda la comparación), no `Int` (el tipo real de `x`). `probe_hover` en cambio compara ANCHOS de span -- solo reemplaza el resultado guardado si el nuevo span es más angosto que el mejor visto hasta ahora, sin importar el orden cronológico. Fijado con un test que prueba exactamente este caso (`checker::tests::hover_on_a_param_reference_inside_a_comparison_gives_the_param_type_not_the_comparisons_bool`) antes de dar la ronda por terminada.

**`hover_type_at(program, offset) -> Option<Type>`, el único punto de entrada nuevo (`pub(crate)`).** Encuentra qué `fn`/`rpc`/`stream` tiene un `body.span` (`Block.span`, prerrequisito 3/3 del LSP -- ninguna esta ronda necesitó agregar ningún span nuevo) que contiene `offset`, y llama a `check_fn`/`check_rpc` TAL CUAL sobre ese ítem -- ni siquiera necesita saber cómo se arman los bindings de parámetros, esas funciones ya lo hacen. El resultado real (`Ok`/`Err`) se descarta a propósito: lo único que importa es el efecto colateral sobre `hover_result` vía las llamadas a `synth_expr`/`check_expr` que ese chequeo dispara por su cuenta.

**`lsp::get_hover` reestructurado para no depender de estar sobre un identificador.** Antes, la función entera arrancaba con `let word = get_word_at_pos(...)?;` -- un `?` que cortaba TODO el hover (palabras clave, nombres de declaración, y ahora expresiones) apenas el cursor caía sobre un operador, un literal, o cualquier posición sin una palabra reconocible. La lógica de palabras clave/declaración (Nivel 1/2, sin cambios de comportamiento) se extrajo a `get_hover_for_word`: si no da resultado (incluyendo el caso "no hay ninguna palabra ahí"), `get_hover` sigue con el hover de expresión, que solo necesita un OFFSET, no una palabra -- así que hoverear sobre `>` en `x > 5`, o sobre un literal `5`, ahora también puede resolver, no solo sobre identificadores.

**El tipo se renderiza con `ts_emit::render_type`, el MISMO renderer que emite el `.d.ts` real** (no un volcado de `Debug` de Rust) -- mismo criterio en los dos lugares para lo que un tipo "se ve": `Int` se muestra `number`, un struct declarado muestra su nombre real (`Point`, no una forma anónima), coherente con "el contrato es el código" (PLAN.md §2.1).

**Límite honesto, documentado en el propio código de `hover_type_at`:** `check_fn`/`check_rpc` paran en el PRIMER error dentro de un body -- el checker no tiene recuperación de errores a nivel de SENTENCIA (el parser sí tiene, pero a nivel de ÍTEM completo, prerrequisito 2/3). Si el body tiene un error de tipos ANTES de la expresión que se está hovereando, esa expresión nunca se llega a chequear y esto devuelve `None` -- ausente, no una respuesta incorrecta, pero sí un hueco real. Cerrarlo necesitaría recuperación de errores a nivel de sentencia en el checker, una extensión propia y más grande que esta ronda (test que fija este límite: `checker::tests::hover_stops_at_an_earlier_error_in_the_same_body`).

Verificado con 6 tests directos sobre `hover_type_at` en `checker.rs` (incluyendo el caso decisivo de más arriba), 3 tests sobre `get_hover` en `lsp.rs`, y un test de subproceso real contra el binario (`compiler/tests/lsp_stdio.rs`). 355 tests, todos pasando.

**Fuera de alcance, a propósito:**
- Statement-level error recovery en el checker (el límite documentado arriba) -- una extensión propia, más grande que esta ronda.
- Hover sobre el NOMBRE de un parámetro en la FIRMA (antes del body) -- sigue sin cambios (Nivel 2, coincidencia por palabra, no llega a activar `hover_type_at` porque esa posición está fuera de `body.span`).
- El ítem 3 de Nivel 3 (completion sensible a `x.`, §3.21/§3.25) -- ronda separada, reutiliza esta misma máquina.

---

### 3.25 Completion sensible al tipo del receptor — RESUELTO, LSP Nivel 3 ronda 3/3 (último ítem)

Cierra el Nivel 3 del LSP completo (§3.19 → §3.21 → §3.24 → acá). Antes de esta ronda, `x.` (cualquier receptor) ofrecía SIEMPRE la misma lista fija de los ~15 métodos builtin posibles (de colección, de lista, de string, conversión numérica), todos mezclados, sin mirar el tipo real de `x` -- Nivel 2, no Nivel 3 (§3.19 ya lo documentaba así). Esta ronda reusa `hover_type_at` (§3.24) tal cual: el "tipo del receptor" es exactamente lo mismo que "el tipo de la expresión bajo el cursor" que el hover ya sabía calcular.

**El problema específico de completion (no de hover): el buffer con un `.` colgante casi nunca parsea.** Mientras se escribe "x.", el resto del archivo puede estar perfecto, pero un `.` sin nada después (o un identificador incompleto) es un error de sintaxis real -- y como el parser no tiene recuperación a nivel de sentencia (§3.19), el `fn`/`rpc` que se está editando se cae ENTERO del `Program`, justo el que hace falta para tipar el receptor. Se resuelve con un parche quirúrgico: `receiver_type_before_dot` reemplaza el rango `[offset_del_punto, offset_del_cursor)` por espacios (mismo largo exacto, nunca toca un `\n`) y re-parsea esa COPIA de forma aislada -- todo lo anterior al punto y todo lo posterior al cursor queda byte a byte idéntico al original, así que el receptor y el resto del archivo parsean normal. `char_offset_from_char_position` (nueva, hermana de `char_offset_from_utf16_position` pero contando CARACTERES en vez de unidades UTF-16, la convención que `get_word_at_pos`/`get_line_prefix_at_pos` ya usaban) convierte la longitud del prefijo recortado de vuelta a un offset absoluto sin mezclar las dos convenciones.

**Bug real encontrado escribiendo los tests de esta ronda, corregido en el mecanismo de §3.24 (no algo nuevo de completion en sí):** el tail de un body se chequea en modo ⇐ (`check_expr`) contra el tipo de retorno declarado -- si NO matchea (ej. una función que declara `-> Int` pero su body es en realidad una `List<Int>`), el chequeo falla, pero la SÍNTESIS de esa misma expresión sí había tenido éxito (literalmente lo que el mensaje de error reporta: "se esperaba Int, se encontró List(Int)"). Antes, el probe de `check_expr` solo grababa un tipo en el caso ÉXITO (`expected`, ver §3.24) -- así que un chequeo fallido perdía el tipo real sintetizado por completo, aunque `synth_expr_inner` lo hubiera calculado correctamente unas líneas antes. Arreglado: si el chequeo falla, `check_expr` reintenta una síntesis best-effort del MISMO nodo antes de rendirse -- gateado por `hover_target` (no corre nunca en un chequeo normal) y correcto incluso si es redundante con la síntesis que ya corrió adentro (re-probar sub-expresiones ya grabadas con el mismo ancho nunca las pisa, ver `probe_hover` §3.24). Sin este fix, completion sobre un receptor cuya firma envolvente tuviera CUALQUIER inconsistencia de tipos (algo que pasa todo el tiempo mientras se escribe código a medias) hubiera vuelto a caer en la lista genérica -- justo el escenario más común en la práctica.

**`completions_for_receiver_type`, un match directo sobre `Type`:** `DbCollection` (all/find/insert/applyPatch/delete/deleteWhere/findWhere/subscribe), `List` (length/take/map/filter), `String` (length/contains), `Int`/`Float` (conversión al otro), `Auth` (createSession/destroySession, con las firmas reales de `check_auth_method`) -- y, capacidad NUEVA que ningún tipo de receptor tenía antes, `Struct { fields, .. }` ofrece los NOMBRES DE CAMPO reales como completion (`p.` sobre `p: Point` ahora sugiere `x`/`y`, no solo métodos builtin genéricos). `Type::Db` (el identificador `db` a secas) devuelve `None` a propósito -- ya tenía su propio manejo por texto (`prefix.ends_with("db.")`, listar nombres de colección), que necesita el `Program` completo, no solo el `Type` aislado; no se tocó para no arriesgar esa lógica ya proband. Cualquier tipo no cubierto, o cualquier fallo en la cadena completa (parche → re-parse → `hover_type_at`), cae a la lista genérica de siempre -- esta ronda solo AGREGA precisión, nunca resta lo que ya había.

Verificado con 5 tests directos sobre `get_completions` en `lsp.rs` (lista/string/struct/colección específica/fallback), 1 test directo sobre el fix del checker en `checker.rs`, y 1 test de subproceso real contra el binario (`compiler/tests/lsp_stdio.rs`, campos de un struct real). 362 tests, todos pasando.

**Fuera de alcance, a propósito:**
- El parche-y-reparse es AISLADO (`parser::parse` directo, no `modules::load_program_with_overlay`) -- si el tipo del receptor depende de un `type`/`enum` de un archivo IMPORTADO, cae al fallback genérico en vez de resolverlo. Reconstruir el overlay completo del `LspServer` acá necesitaría que esta función deje de ser libre y pase a ser un método de instancia -- no pedido en esta ronda.
- Completion de un campo ESPECÍFICO dentro de un `Type::Generic` instanciado (`Box<Point>.`) -- `completions_for_receiver_type` no tiene un brazo para `Generic`, cae al fallback genérico (necesitaría `expand_generic_struct`, ya `pub(crate)`, pero no conectado acá).
- Nada de esto reemplaza el filtrado del lado del CLIENTE (VS Code, etc.) sobre lo que el usuario ya tipeó después del `.` -- sigue asumiendo el trigger character estándar (un `.` recién tipeado, sin texto parcial todavía), mismo alcance que la lista genérica ya tenía antes de esta ronda.

Con esto, el Nivel 3 del LSP completo queda resuelto: los 3 ítems que §3.19/§3.21 dejaban pendientes (goto-def de tipo en firma, hover de expresión arbitraria, completion sensible a tipo) están hechos.

---

### 3.26 Observabilidad: tracing estructurado por RPC — RESUELTO, v0

PLAN.md §4 (Fase 2) la nombraba junto al package manager como pendiente. `runtime/server.rs` ya tenía un `req_id` incremental (agregado como prerrequisito parcial para poder correlacionar líneas de log entre el hilo principal y los hilos de escritura de `stream`) -- esta ronda es lo que faltaba encima de eso.

**Una línea por request COMPLETADA, formato `clave=valor` -- greppable sin parsear JSON.** `log_done(req_id, method, status, start, extra)` es el único punto de emisión: `[req {id}] method={service}.{rpc} status={code} duration_ms={ms}` (+ `{extra}` si no está vacío). Mismo espíritu que el formato de texto de `tracing`/los logs de Heroku -- no se suma la dependencia `tracing` para esto, `println!` con un formato consistente ya alcanza para un v0 (agregar salida JSON estructurada, o niveles de log configurables, sería la extensión natural si hiciera falta después).

**Tres piezas nuevas sobre el log de "request recibida" que ya existía:**
- **Duración real** (`duration_ms`), un `Instant::now()` capturado al entrar y restado en cada punto de salida -- incluyendo los hilos de escritura de `stream`/`stream` en vivo (`start` se les pasa junto con `req_id`), así que la duración de un stream cubre el envío completo, no solo el cómputo inicial en el hilo principal.
- **El método real** (`method=Users.create`, no la ruta cruda) -- ya se conocía en cada rama existente (`service_name`/`rpc_name` de `parse_path`), esta ronda solo lo agrega al log de salida. `None` (`method=-`) para los pocos casos que nunca llegan a resolverlo (un 404 por URL mal formada).
- **El mensaje de error en la propia línea de log**, no solo el código de status. Antes, un 401/400/500 solo mostraba el número -- para saber QUÉ pasó había que inspeccionar la respuesta por otro lado (un `curl -v`, ver el cliente generado fallar). Ahora `error="..."` va en la misma línea. Para el camino de `handle_rpc` (la mayoría de los rpc), el body de error es `{"error": "<mensaje>"}` -- se extrae el mensaje real en vez de loguear el JSON completo escapado adentro de otro string (`error="{\"error\":\"...\"}"`, técnicamente correcto pero feo de leer); si el body no tiene esa forma exacta por algún motivo, cae al body crudo en vez de esconder la falla.

**Los casos de desconexión de un `stream` (antes texto libre) ahora usan el mismo formato**, con campos propios (`client_disconnected=true stage=snapshot sent=N`, etc.) en vez de una oración armada a mano -- consistente con el resto, aunque conceptualmente no sean "un error" (la respuesta 200 ya se había mandado; es el cliente el que se fue).

Verificado con un servidor real: los 4 casos (éxito, 404 por ruta desconocida, 401 por auth, 500 por servicio desconocido) dan líneas limpias y completas -- confirmado leyendo el stdout real del proceso, no solo por inspección de código. El demo insignia completo (`frontend/src/main.ts` contra un servidor real) también se corrió de punta a punta para confirmar que el refactor de logging no cambió ningún comportamiento funcional. 371 tests, todos pasando (esta ronda no agregó tests nuevos -- el logging en sí no es una superficie que este proyecto testee con asserts, mismo criterio que ya regía para el `req_id`/formato de log anteriores; se verificó leyendo stdout real, el mismo método que ya usaba la auditoría original de este mismo módulo).

**Fuera de alcance, a propósito:**
- Salida estructurada en JSON (para ingestión por un colector de logs real) -- el formato `clave=valor` alcanza para un v0 de un solo proceso sin infraestructura de observabilidad detrás.
- Niveles de log configurables (`--verbose`/`RUST_LOG`) -- hoy todo sale siempre, sin flag para silenciar ni para pedir más detalle.
- Métricas agregadas (percentiles de latencia, tasa de error) -- esto es tracing por request individual, no una capa de métricas encima.

---

### 3.27 Hot reload real en `linkc dev` — RESUELTO, v0

PLAN.md §4 (Fase 2) lo nombraba junto a `LSP completo`/`package manager`/`observabilidad`. Antes, `linkc dev <archivo> <outdir>` observaba y reconstruía el contrato (`contract.d.ts`/`client.ts`/`validators.ts`/`link.lock`) pero nunca tocaba un servidor -- correr el backend en paralelo seguía siendo `linkc serve` aparte, sin ninguna conexión entre los dos.

**`linkc dev <archivo> <outdir> [puerto]` -- el `[puerto]` es opcional y retrocompatible.** Sin él, comportamiento IDÉNTICO a antes de esta ronda. Con él, cada rebuild EXITOSO reinicia un `linkc serve` HIJO real con el programa actualizado.

**Restart de proceso, no hot-swap en memoria -- decisión deliberada.** `spawn_serve_child` reinvoca el propio binario (`env::current_exe()`) con `serve <archivo> <puerto>`, reusando `cmd_serve`/`runtime::server::serve` TAL CUAL, sin ningún cambio. La alternativa (mutar el `Program` de un servidor YA CORRIENDO) hubiera necesitado tocar el modelo de threading que `runtime/server.rs` ya documenta con cuidado (`Value::Closure`/`Rc` no cruzan un borde de hilo, GRAMMAR.md §3.13) -- un restart de proceso es más simple de razonar y más robusto, al costo de perder las conexiones `stream` abiertas en cada reload (aceptable en modo desarrollo, no sería aceptable en producción, pero esto es explícitamente `linkc dev`, nunca `linkc serve` en frío).

**Un rebuild FALLIDO nunca tira abajo el servidor.** Si el rebuild que sigue a un cambio de archivo falla (error de sintaxis/tipos mientras se edita), el hijo de la ÚLTIMA versión válida sigue sirviendo sin tocarse -- mismo criterio que un dev server de frontend real (Vite/webpack) que sigue sirviendo el último build bueno en vez de caerse por un typo a medio escribir. Solo un rebuild EXITOSO mata al hijo viejo (`kill_serve_child`, por su PID exacto -- nunca un kill por nombre de imagen, para no afectar otro `linkc serve` que el usuario tenga corriendo aparte) y levanta uno nuevo.

**Persistencia de datos entre reloads: gratis, por diseño ya existente.** `db_path` se deriva de `<archivo>.db` (GRAMMAR.md §3.17) -- el mismo archivo en cada restart, así que los datos de la sesión de desarrollo sobreviven un hot reload sin ningún código nuevo. Si el reload además cambia la FORMA de `db { ... }`, el hijo nuevo falla fuerte con el mismo diff-y-remedio de siempre (§3.17) al reabrir un schema incompatible -- comportamiento heredado, no una brecha nueva de esta ronda.

**Límite honesto sobre limpieza al salir.** Sin manejo de señales explícito: `Command::spawn()` sin `CREATE_NEW_PROCESS_GROUP` deja al hijo en el mismo grupo de proceso/consola que el padre en ambas plataformas, así que un Ctrl+C real en una terminal interactiva le llega TAMBIÉN al hijo -- el camino verificado manualmente. Un kill programático dirigido SOLO al PID del proceso padre (no un Ctrl+C real desde una terminal) es el caso que sí puede dejar al hijo huérfano sirviendo el puerto -- límite de v0 conocido, no manejado, mismo tipo de limitación que `gitdep::resolve` ya documenta para el locking entre procesos (§2.1).

**Verificado manualmente de punta a punta contra el binario real** (no un test automatizado -- ver por qué abajo): `linkc dev` con un archivo mínimo (`service Ping { rpc version() -> Int { 1 } }`) y puerto, confirmando en cada paso contra el servidor real vía `curl`: (1) arranque inicial sirviendo `1`; (2) al editar el archivo a `2`, detección del cambio, rebuild, kill del PID viejo, spawn de un PID nuevo, y `curl` devolviendo `2`; (3) al introducir un error de tipos, el rebuild falla con el snippet real de siempre y el servidor del PID anterior sigue sirviendo `2` sin interrupción. 371 tests automatizados (sin cambios de este archivo -- ver la nota de alcance de tests abajo), todos pasando.

**Por qué sin test automatizado:** `cmd_dev` es un loop infinito e interactivo (nunca antes tuvo cobertura automatizada, ni siquiera para su comportamiento de observar-y-reconstruir previo a esta ronda) que ahora además administra un PROCESO HIJO -- un test de subproceso real necesitaría descubrir el PID del hijo (parseando stdout) para limpiarlo aparte, porque matar solo al PID del padre programáticamente (a diferencia de un Ctrl+C real) no arrastra al hijo (ver el límite de arriba). Se prefirió verificación manual real y exhaustiva (3 escenarios, contra el binario real) antes que forzar un harness de test alrededor de una herramienta pensada para uso interactivo en primer plano.

**Fuera de alcance, a propósito:**
- Preservar conexiones `stream` abiertas a través de un reload -- se cortan y el cliente debe reconectar, igual que cualquier restart de servidor.
- Limpieza del hijo ante un kill programático del padre (el límite de arriba).
- Debounce de múltiples cambios de archivo muy seguidos -- cada mtime distinto dispara su propio rebuild+restart, como ya hacía el `linkc dev` sin servidor.

---

### 3.28 Fase 3 (PLAN.md §4): política de estabilidad de sintaxis, y por qué source maps NO se persigue todavía

Última pieza del backlog de la auditoría post-push que arrancó con la Ronda 0. PLAN.md §4 nombra, para Fase 3 (1.0, producción, "+6–12 meses, 4–6+ personas"), dos entregables puntuales: "estabilidad de sintaxis" y "debugging con source maps". Ninguno de los dos es una feature que se pueda "implementar" como las anteriores -- son, respectivamente, una DECISIÓN de política y una decisión de NO-hacer, y esta sección las deja explícitas en vez de dejarlas flotando sin resolución.

**Congelar la sintaxis ahora sería prematuro -- decisión consciente, no un olvido.** El propio [README](README.md) sigue abriendo con "This repo is the **Phase 0 MVP** ... It is not a production-ready language". Comprometerse a una sintaxis inmutable ANTES de que exista un solo usuario externo real usándola sería fijar en piedra decisiones (§2.3 nullability, §3.5 manejo de errores, y cada `RESUELTO` de la sección 3 de arriba) que todavía no pasaron la prueba de fuego de un caso de uso ajeno -- exactamente el tipo de compromiso prematuro que PLAN.md §7 ya identifica como uno de los riesgos principales de un lenguaje nuevo.

**Política aplicada en su lugar, efectiva desde esta ronda:** mientras la versión declarada en `compiler/Cargo.toml` sea `0.x` (hoy `0.1.0`), un cambio de sintaxis que rompa un `.link` existente se documenta en el `CHANGELOG` de su propio commit (mismo criterio que esta auditoría entera ya viene aplicando: cada ronda que cambió comportamiento lo dice explícitamente en README/GRAMMAR, nunca en silencio) pero NO requiere ningún proceso de deprecación ni compatibilidad hacia atrás. Recién en `1.0.0` esta libertad se cierra: un cambio incompatible pasa a requerir una migración documentada (o un nuevo mecanismo de edición/versión de lenguaje, al estilo `edition` de Rust, si para entonces hay motivo real de necesitarlo -- decisión que le corresponde a esa ronda futura, no a esta). Esto no es una promesa nueva inventada acá: es simplemente hacer explícito lo que SemVer ya dice sobre una versión `0.x`, para que quede escrito una vez en vez de asumido.

**Source maps: valor genuinamente incierto con la arquitectura actual, no simplemente "no hubo tiempo".** La razón habitual para pedir source maps es mapear código GENERADO (JS transpilado, minificado) de vuelta al fuente original durante una sesión de debugging. Acá:
- La lógica de negocio real (el cuerpo de cada `rpc`/`fn`) corre en el INTÉRPRETE de Rust (`runtime/mod.rs`), nunca se transpila a JS/TS -- no hay ningún paso de compilación de ESE código para el que un source map tenga sentido. Un error de runtime ahí ya sale con la posición real en el `.link` fuente (`diagnostics.rs`, GRAMMAR.md prerrequisitos 1-3 del LSP), sin necesitar ningún mapeo.
- Lo único que SÍ se genera hacia TS (`contract.d.ts`/`client.ts`/`validators.ts`, `ts_emit.rs`/`validators_emit.rs`) es deliberadamente FINO -- interfaces y un cliente RPC que arma un `fetch()`, sin lógica propia que alguien necesite pisar con un breakpoint y "step into" hacia el `.link` original. Pisar un breakpoint DENTRO de `client.ts` ya te deja en TypeScript legible, generado pero no ofuscado ni minificado -- el caso de uso que un source map resuelve (código irreconocible) no se da acá.
- El único lugar donde HOY se emite bytecode de verdad no legible por un humano es `linkc wasm` (§3.20) -- explícitamente congelado esta misma auditoría, alcance mínimo, no el camino de producción.

Dado esto, la recomendación es NO perseguir source maps como una ronda propia hasta que la arquitectura cambie de forma que los vuelva necesarios (ej. si algún día existe un compilador real hacia JS del CUERPO de un `rpc`, no solo del cliente) -- perseguirlos ahora sería construir infraestructura para un problema que este diseño concreto no tiene todavía. Si aparece un caso real y concreto de "no puedo debuggear X" que un source map resolvería, esa necesidad puntual es la que debería disparar la ronda, no esta lista de tareas.

**Con esto, el backlog completo de la auditoría post-push queda resuelto o explícitamente decidido: nada se dejó flotando sin una razón escrita.** Ver la sección "Estado" del [README](README.md) para el resumen de qué se hizo en cada ronda.

### 3.29 `linkc test`: contrato contra un snapshot commiteado (PLAN.md §5, "tests de contrato")

PLAN.md §5 nombra, en la lista de herramientas de ecosistema, "Testing: runner integrado + tests de contrato (que el `.d.ts` generado no rompa sin querer)" -- el único ítem de esa lista que seguía sin una v0 real (CLI, LSP, package manager, debugging/observabilidad y las integraciones vía interop nativa ya estaban resueltos, ver "Estado" del [README](README.md)).

**Qué hace.** `linkc test <archivo.link> <archivo.snap> [--update]` genera el mismo trío que `linkc build` (`contract.d.ts`, `client.ts`, `validators.ts`, vía los mismos emisores) y lo compara contra un snapshot de texto plano. Sin snapshot previo, lo crea y avisa que hay que commitearlo -- esa corrida establece la base. Con un snapshot que matchea, sale OK sin tocar nada. Con un snapshot que difiere, falla (`ExitCode::FAILURE`) y muestra el diff línea a línea; `--update` acepta el contrato nuevo como la base siguiente.

**Por qué un archivo de texto commiteado, y no algo dentro de `outdir`.** `outdir` (`gen/` en este repo) está en `.gitignore` -- se regenera en cada build, nunca sobrevive entre commits. Un snapshot necesita sobrevivir precisamente para servir de algo: comparar el contrato de HOY contra el de la ÚLTIMA VEZ QUE ALGUIEN LO REVISÓ, no contra el de hace un segundo. Por eso el snapshot vive en una ruta separada que el usuario elige (en este repo, `examples/users.link.snap`, sibling del `.link`, fuera de `gen/`) y se commitea a git como cualquier otro archivo fuente.

**Por qué falla en vez de sobreescribir.** Que el contrato haya cambiado puede ser una ronda legítima (agregar un campo, un rpc nuevo) o el bug exacto que esta feature existe para atrapar (un rename accidental, un tipo que cambió de forma sin que nadie se diera cuenta). Un comando que sobreescribe solo no distingue los dos casos -- por eso `--update` es un paso explícito, no el default, y CI (`.github/workflows/ci.yml`) corre `linkc test` SIN `--update`: un PR que cambia el contrato del demo insignia sin commitear el `.snap` actualizado falla la build, con el diff real en el log.

**El diff es un LCS real (programación dinámica), no una comparación posición-a-posición.** Una comparación ingenua línea-por-línea (línea 5 vieja vs línea 5 nueva, línea 6 vs línea 6, ...) marca como "distinta" cada línea después de una sola inserción o borrado, aunque el resto del archivo sea idéntico -- inútil para revisar un cambio real. El algoritmo hand-rolled (mismo espíritu que el SHA-256 de `lockfile.rs`: chico, estable, autocontenido, sin depender de un crate nuevo para algo bien entendido) encuentra la subsecuencia común más larga entre las líneas viejas y nuevas y reporta inserciones/borrados reales. Guarda de tamaño: la tabla LCS es O(n×m) en memoria -- trivial para un contrato de cientos de líneas, así que por encima de ~2000×2000 líneas el comando se rehúsa a construir la tabla completa y devuelve un mensaje en vez de arriesgar un uso de memoria sin cota -- no una ruta esperada hoy, pero tampoco silenciosa si algún día pasa.

**Límites honestos de v0.** Es "tests de contrato", no el "runner integrado" completo que PLAN.md §5 nombra en la misma línea -- no hay forma de escribir assertions sobre el COMPORTAMIENTO de un `rpc` (ej. "`Users.create` con este input devuelve este output") dentro de un `.link`, solo sobre la FORMA del contrato que genera. Escribir un framework de tests real embebido en el lenguaje (sintaxis `test { }`, aserciones, un runner que invoque rpcs contra una `db` de prueba) es una feature de lenguaje nueva -- semanas, no una ronda -- y queda fuera de esta ronda a propósito, mismo criterio que ya se aplicó al no perseguir source maps sin un caso concreto (§3.28). Si aparece la necesidad real de testear comportamiento (no solo forma), esa necesidad puntual debería disparar esa ronda, no esta lista.

Verificado con tests de integración reales contra el binario compilado (`compiler/tests/cli_test_snapshot.rs`): primera corrida crea el snapshot, corrida sin cambios matchea, un cambio real (rename de campo) falla mostrando el campo nuevo en el diff, y `--update` acepta el cambio y vuelve a matchear después. Dogfooded sobre el propio demo insignia: `examples/users.link.snap` está commiteado y CI lo verifica en cada push/PR (ver el paso nuevo en `.github/workflows/ci.yml`).

**Bug real, encontrado por CI en el primer push, no en revisión local: falso positivo de "cambió" en `windows-latest`.** El primer commit de esta ronda pasó local y en `ubuntu-latest`, pero falló en `windows-latest` -- `linkc test` reportaba que el contrato del demo insignia había cambiado, con un diff VACÍO (contradictorio: "cambió" pero sin mostrar qué). Causa real: este repo tiene `core.autocrlf=true`, así que el checkout en un runner Windows convierte `examples/users.link.snap` (commiteado en LF -- `linkc` nunca escribe `\r\n`) a CRLF en el disco del runner; la comparación `previous == current` es sobre los bytes crudos, así que un `\r` de más alcanza para que nunca matcheen, en TODA corrida sobre ese checkout. El diff vacío fue una segunda capa del mismo bug: `diff_lines` opera sobre `str::lines()`, que sí ignora `\r\n` vs `\n` al partir líneas -- así que el diff, corriendo sobre las mismas dos cadenas, no encontraba ninguna línea distinta, aunque la comparación de arriba ya hubiera decidido que sí. Fix real en `cmd_test` (`main.rs`): normalizar `\r\n` → `\n` tanto en el snapshot leído como en el contrato recién generado ANTES de cualquier comparación -- así la corrección del comando no depende de `core.autocrlf` de la máquina que lo corre, ni de que `.gitattributes` esté bien configurado (que también se agregó, fijando `*.snap` como LF, pero como higiene del diff commiteado, no como el fix en sí). Test de regresión que reproduce el bug a mano (reescribe el snapshot con CRLF sin depender de ninguna configuración real de git, para ser determinista en cualquier máquina) en `cli_test_snapshot.rs`. 376 tests, todos pasando -- verificado de nuevo en CI real en ambos sistemas operativos después del fix, no solo local, ya que el bug NUNCA se reprodujo localmente en primer lugar.

### 3.30 `Int64` — RESUELTO, cierra la única fila "no" de tipos que quedaba en PLAN.md §2.3

PLAN.md §2.3 (la propuesta original, no el estado real) siempre tuvo una fila `Int64`/`BigInt` marcada explícitamente "no -- nunca se implementó". La razón de por qué importa: `Int` ya es `i64` en el intérprete (`Value::Int(i64)`, `runtime/mod.rs`), pero se emite como `number` de TypeScript -- cualquier valor arriba de `2^53` pierde precisión en silencio en cuanto el cliente hace `JSON.parse` (IEEE-754 `f64` no puede representar todo el rango de un i64). No es una feature que faltaba por gusto: es un gap de corrección latente para cualquier campo `Int` que en la práctica necesite el rango completo (ids tipo snowflake, contadores grandes).

**No es un bignum de precisión arbitraria.** `Int64` es exactamente el mismo rango que `Int` (`i64`) -- una variante propia, `Value::Int64(i64)`/`Type::Int64`, no una reinterpretación de `Value::Int`. Necesita ser su propia variante porque `value_to_json` (`runtime/mod.rs`) no recibe ningún contexto de `Type`, solo hace match estructural sobre `Value` -- sin una variante propia no hay forma de saber, en ese punto, si un entero debe serializarse como número nativo o como string.

**Wire format: string en ambas direcciones, y el tipo TS emitido es `string`, no `bigint`.** El wire ya estaba decidido en PLAN.md (string, "para no perder precisión") -- lo que se resolvió en esta ronda fue el lado TS. `push_fetch_call`/`emit_client` (`ts_emit.rs`) no tienen ningún punto de conversión dirigido por tipo hoy: el request hace `JSON.stringify({args})` sin replacer, la respuesta hace `res.json()` sin reviver. Emitir `bigint` correctamente necesitaría un reviver real que distinga "este string es semánticamente un Int64" de "este campo `String` que por casualidad parece un número" -- eso es un walker recursivo dirigido por tipo nuevo, arquitectura nueva, no una extensión del patrón que `Int`/`Bool`/`String` ya siguen. Emitiendo `string`, el wire format y el tipo TS coinciden exactamente y el cliente generado no necesita ningún cambio de codegen -- quien necesite aritmética real hace `BigInt(x)` a mano, consistente con que el lenguaje ya no hace ninguna coerción implícita en ningún otro punto (§3.7). La opción `bigint` queda como una ronda futura separada, con su propio walker de (de)serialización, si alguna vez hay un caso concreto que la pida.

**`.toInt64()`/`.toInt()` son la ÚNICA forma de obtener un `Int64` desde código fuente en v0 -- no un nice-to-have.** Un literal entero (`Expr::Int`) siempre sintetiza `Type::Int`, nunca `Type::Int64` directamente, e `is_subtype(Int, Int64)` es `false` (tipos distintos, sin coerción implícita) -- así que `let x: Int64 = 5;` no compila. Ambas conversiones son exactas, nunca lossy (mismo rango `i64` en los dos lados), a diferencia de `toFloat`/`toInt` entre `Int` y `Float`. Consecuencia honesta de lo mismo: `is_const_literal_shape` no permite llamadas a método, así que **`const X: Int64 = ...` no tiene ninguna forma válida de escribirse en v0**; tampoco puede ser el `id` de una colección `db` (`validate_db_element_type` exige `Type::Int` exacto) -- ambos límites de diseño, no bugs.

**Aritmética/comparaciones: mismos operadores que `Int`, sin mezcla implícita.** `Int64 + Int64`, `-`, `*`, `/`, `%`, `<`/`<=`/`>`/`>=`/`==`/`!=` y `-` unario funcionan igual que sobre `Int` -- pero `Int64 + Int` es un error de tipos, mismo criterio que ya separa `Int`/`Float` (§3.7). Es un scrutinee de `match` válido con patrones de literal (`LiteralPattern::Int` sirve para ambos, no hay sintaxis de literal `Int64` propia), con la misma semántica de igualdad exacta que `Int` -- a diferencia de `Float`, que se excluye a propósito.

**Persistencia: mapea a `INTEGER` en SQLite, sin columna especial.** `rusqlite`/SQLite ya son 64-bit nativos para `Int` (`SqlValue::Integer` es un `i64`), así que `Int64` reusa exactamente la misma columna -- ningún cambio de esquema, solo una nueva rama de conversión en `native_sql_type`/lectura de fila/`write_param` (`runtime/db.rs`).

**Dos riesgos reales que el compilador NO fuerza a cerrar, cerrados a mano.** `impl PartialEq for Value` y `value_matches_type` (`runtime/mod.rs`) están escritos a mano y terminan en `_ => false` -- sin un brazo explícito para `Int64`, `==`/`!=` entre dos valores iguales devolvería `false` en silencio, y el narrowing de `match` sobre una unión con un miembro `Int64` nunca dispararía ese arm, también en silencio. A diferencia de `render_type`/`type_key`/`render_check`/`Debug for Value` (exhaustivos, el compilador obliga a tocarlos en cuanto se agrega la variante), estos dos no avisan -- quedaron como checklist explícito durante el diseño de esta ronda, no descubiertos por un test que fallara.

Verificado con tests unitarios nuevos (checker: conversión, no-mezcla, aritmética/comparaciones, match; `runtime/mod.rs`: round-trip exacto en `i64::MIN`/`i64::MAX` a través de `invoke_rpc`, rechazo de un número JSON nativo y de strings malformados/fuera de rango; `runtime/db.rs`: insert+find exacto en los extremos de i64; `ts_emit.rs`/`validators_emit.rs`: forma emitida real; `lsp.rs`: hover y completion) y, además, con el binario real: un programa `.link` con un campo `Int64` compilado de punta a punta (`linkc build`, contrato y validador inspeccionados a mano), y un servidor real (`linkc serve`) golpeado con `curl` -- `i64::MAX` como string viaja, se persiste en SQLite, y vuelve exacto (`"9223372036854775807"`, no `9223372036854775807` truncado a `f64`); un número JSON nativo para el mismo campo se rechaza con un 400 claro (`"se esperaba Int64, se recibió un número"`), en vez de aceptarse y perder precisión en silencio -- exactamente el bug que esta ronda existe para cerrar.

### 3.31 `Timestamp` — RESUELTO, alcance acotado a propósito

La otra fila "no -- nunca se implementó" que quedaba en la tabla de tipos original de PLAN.md §2.3 (§3.30 cerró la de `Int64`). No había ningún tipo de fecha nativo -- quien necesitaba una fecha usaba `String`/`Int` a mano, sin validación ni semántica propia.

**Representación: milisegundos desde epoch UTC internamente (`Value::Timestamp(i64)`), string ISO-8601 de forma FIJA en el wire y en TS.** La forma exacta es `YYYY-MM-DDTHH:mm:ss.sssZ` -- UTC, milisegundos siempre presentes, `Z` obligatorio; cualquier otra variante (offset de timezone, precisión distinta, separador distinto) se rechaza, no se acepta a medias. Milisegundos-desde-epoch fue la elección deliberada frente a guardar el string ISO-8601 tal cual: mismas unidades que `Date.now()`/`new Date(ms)` de JS (cero sorpresa de unidades del lado cliente), orden/comparación triviales (comparar dos `i64` en vez de comparar dos strings, que solo ordena bien por lexicografía si TODOS los strings comparados están en el mismo formato exacto -- una invariante que dependería de convención, no de construcción), y una columna SQLite `INTEGER` nativa con range queries indexadas correctas (`WHERE createdAt > ?`), en vez de una columna `TEXT` comparada por lexicografía.

**TS emitido: `string` plano, no branded.** Mismo criterio minimalista que el resto del proyecto -- un tipo branded (`type Timestamp = string & { __brand: "Timestamp" }`) da una distinción nominal real pero suma fricción de conversión sin que hoy haya un caso concreto que la necesite. Revisar si aparece un caso real, mismo patrón que la decisión de no perseguir source maps sin uno (§3.28).

**Sin dependencia nueva -- algoritmo de calendario civil adaptado, no una librería.** El cálculo año/mes/día ↔ días-desde-epoch (`compiler/src/runtime/timestamp.rs`) es un puerto directo del algoritmo público de Howard Hinnant (`civil_from_days`/`days_from_civil`, dominio público/CC0, el mismo que usa `libc++` para `std::chrono::year_month_day`) -- aritmética entera exacta, sin tabla de lookup, correcto en años bisiestos y en fronteras de siglo (1900 no es bisiesto, 2000 sí) por construcción, no por casos especiales a mano. Mismo espíritu que el SHA-256 de `lockfile.rs` o el diff LCS de `linkc test`: un algoritmo chico y bien definido no amerita sumar una crate de calendario completa. El parseo (`parse_iso8601_millis`) reusa el propio algoritmo como su validador: una fecha que no existe (30 de febrero) hace que el cálculo "se derrame" hacia el mes siguiente, así que convertir el resultado de vuelta y compararlo contra los campos originales detecta el derrame sin duplicar ninguna tabla de "días por mes" a mano.

**Solo comparable -- sin aritmética, sin ser scrutinee de `match`, sin métodos.** `<`/`<=`/`>`/`>=`/`==`/`!=` funcionan entre dos `Timestamp` (mismo mecanismo que ordena `Int`); `+`/`-`/`*`/`/`/`%` y `-` unario se rechazan -- no existe un tipo `Duration`, así que "sumar" a un `Timestamp` no tiene un significado definido todavía, diseño futuro aparte. Excluido como scrutinee de `match`, mismo criterio que ya excluye `Float` a propósito. Sin ningún método propio (`.algo()` sobre un `Timestamp` es siempre un error del checker) -- la completion del LSP para un receptor `Timestamp` devuelve una lista vacía explícita, no el fallback genérico con métodos de otros tipos que acá no aplican.

**Sin construcción desde código fuente en v0 -- límite real, documentado, no un olvido.** No hay `now()`: el lenguaje no tiene NINGÚN mecanismo de función builtin sin receptor (`Expr::Call` solo reconoce una `fn` de usuario por nombre, o un método vía `receptor.metodo(...)`) -- agregar uno sería territorio arquitectónico nuevo, no parte de "agregar un tipo". Tampoco hay auto-stamping de una columna tipo `createdAt` al hacer `insert` en `db` -- una decisión de diseño aparte, con sus propios trade-offs (¿es magia sorprendente o la conveniencia esperada?), que amerita su propia ronda si aparece la necesidad real. Un valor `Timestamp` en v0 solo puede: llegar como parámetro de un `rpc` desde el cliente, o ya estar guardado en `db`. Ninguno de los dos queda descartado para siempre, solo fuera de esta ronda.

### 3.32 Función builtin `now() -> Timestamp` — RESUELTO

Cierra el límite honesto documentado en §3.31 ("sin construcción desde código fuente en v0"). `now()` es una función builtin de primer nivel sin receptor que retorna un `Timestamp` con la fecha y hora actual en milisegundos desde el epoch UTC (formateado en ISO-8601 en el wire).

- **Sintaxis y tipado:** `now()` sintetiza `Type::Function([], Type::Timestamp)`. No toma argumentos.
- **Runtime:** Invoca el reloj del sistema (`SystemTime::now().duration_since(UNIX_EPOCH)`) devolviendo `Value::Timestamp(millis)`.
- **LSP:** Incluido en la lista de autocompletado y hover de funciones built-in.

### 3.33 Test runner de comportamiento integrado (`test "nombre" { ... }`, `assert`, `panic`) — RESUELTO

Completa el objetivo de PLAN.md §5 ("Testing: runner integrado"). Permite escribir tests de integración y comportamiento directamente en archivos `.link`.

<!-- linkc:check -->
```link
type User = { id: Int, name: String }
db { users: User[] }

service Users {
  rpc create(name: String) -> User {
    db.users.insert(User { id: 0, name: name })
  }
}

test "crear usuario incrementa id y persiste" {
  let user = Users.create("Ada");
  assert(user.name == "Ada", "el nombre debe coincidir");
  assert(user.id > 0);
}
```

- **Sintaxis:** `test <string_lit | identifier> { ... }`.
- **Aislamiento total:** Cada bloque `test` corre con una base de datos SQLite en memoria (`:memory:`) fresca y aislada y un nuevo `SessionStore`, garantizando que las mutaciones de un test no contaminen a los demás.
- **Llamada a servicios:** Dentro de los tests (y en el lenguaje en general), los servicios y sus RPCs pueden ser invocados directamente como `Service.rpc(args...)`.
- **Builtins:** `assert(cond: Bool, [msg: String])` verifica condiciones y falla con el mensaje especificado; `panic(msg: String)` aborta la ejecución con un error explícito.
- **CLI:** `linkc test <archivo.link>` ejecuta todos los tests de comportamiento y reporta el conteo de pasados/fallidos con exit code 0 o 1. Si se pasa un segundo argumento `.snap`, continúa ejecutando el test de snapshot de contratos.

---

### 3.34 `crypto`: contraseñas y aleatoriedad — RESUELTO (Argon2id + CSPRNG del SO)

Auditoría del 20/08/2026, disparada por un intento real de migrar un panel de
administración a c-script: el módulo `crypto` tenía el nombre de la seguridad
pero no su comportamiento. Las cuatro cosas que estaban mal, todas en
`runtime/mod.rs`:

1. **`hashPassword` era un solo SHA-256 con una sal constante** —
   `"link_salt_2026"`, escrita en el compilador. No una sal por usuario: la
   MISMA sal para toda aplicación escrita en este lenguaje, en cualquier parte
   del mundo. Dos usuarios con la misma contraseña producían el mismo hash, y
   una única rainbow table calculada una vez servía contra todas. Sin
   iteraciones ni costo de memoria, además: exactamente el escenario para el
   que existe un KDF.
2. **`verifyPassword` comparaba con `==` de `String`**, que corta en el primer
   byte distinto. El tiempo de respuesta filtra cuántos caracteres del hash
   acertó quien está probando.
3. **`randomToken(n)` era SHA-256 del reloj** (`SystemTime::now().as_nanos()`
   más el largo pedido). Un token es adivinable para quien pueda acotar el
   instante en que se emitió, y dos llamadas dentro del mismo nanosegundo
   devuelven el mismo token. Cero bits de entropía real.
4. **`uuid()` era lo mismo**, formateado con los bits de versión de un v4 para
   que pareciera aleatorio. Dos identificadores "únicos" generados en el mismo
   nanosegundo eran idénticos.

Lo notable del hallazgo es dónde NO estaba el bug: `runtime/session.rs` ya
había pasado por una auditoría propia sobre exactamente este tema (el problema
de `RandomState`, documentado en §3.14) y sus tokens de sesión sí tenían
entropía del sistema. La corrección se había aplicado a una capa y nunca se
propagó a la API que ve el usuario del lenguaje — el mismo patrón
"dos capas que discrepan" que este documento viene registrando desde §3.9.

**Lo que hay ahora:**

| Función | Implementación |
|---|---|
| `crypto.hashPassword(pwd)` | Argon2id (RFC 9106, parámetros por defecto de la crate `argon2`: m=19 MiB, t=2, p=1) con sal aleatoria de 16 bytes **por contraseña**, salida en formato PHC `$argon2id$v=19$m=...,t=...,p=...$sal$hash` |
| `crypto.verifyPassword(pwd, hash)` | Verificación de la crate, que compara en tiempo constante; el camino legado usa `subtle::ConstantTimeEq` |
| `crypto.randomToken(n)` | `getrandom` (CSPRNG del SO: `BCryptGenRandom` en Windows, `getrandom(2)` en Linux, `random_get` en WASI) |
| `crypto.uuid()` | UUIDv4 real, 122 bits del mismo CSPRNG |
| `crypto.hashSha256(s)` | Sin cambios: es un digest y se documenta como tal, no como algo para contraseñas |

**Migración, y por qué se aceptan los hashes viejos.** `verifyPassword`
reconoce el formato anterior (`sha256$<sal>$<hex>`) y lo verifica en tiempo
constante. Rechazarlo habría sido más "limpio", pero significaría que
actualizar el compilador deja afuera a todos los usuarios ya registrados de
cualquier app en producción. La próxima vez que esa contraseña se guarde,
`hashPassword` la escribe ya en Argon2id.

**Límites honestos de esta ronda:**

- **Los parámetros de Argon2id no se pueden configurar desde el lenguaje.** Son
  los del default de la crate. Un servicio que necesite subir el costo de
  memoria hoy no tiene cómo pedirlo.
- **No hay señal de "este hash es viejo, re-hashealo".** `verifyPassword`
  devuelve `Bool`; quien quiera migrar de forma proactiva tiene que mirar el
  prefijo del hash guardado desde su propio código.
- **Hashear bloquea el hilo del servidor.** El intérprete es single-threaded
  por diseño (§3.13), y un `hashPassword` cuesta ~15 ms en la máquina donde se
  midió esto. Es el precio correcto para un login, pero es tiempo de servidor
  serializado: N logins simultáneos se atienden uno detrás del otro.
- **Sin rotación ni expiración de sesiones** — eso sigue como estaba en §3.14.

**Tests que fijan estas propiedades** (`runtime/mod.rs`, módulo de tests): que
dos hashes de la misma contraseña difieran, que ambos verifiquen igual, que el
hash declare `$argon2id$`, que un hash legado válido siga verificando y uno que
no corresponde no, y que dos `randomToken`/`uuid` consecutivos sean distintos.
Cada uno de esos asserts falla con la implementación anterior — que es la
prueba de que testean la propiedad y no la firma.

---

### 3.35 `@content_type`: respuestas que no son JSON — RESUELTO (alcance acotado)

El `Content-Type` de la respuesta estaba literal en el binario: `application/json`
para todo rpc, `text/event-stream` para todo stream. No había una tercera vía, y
eso no es un detalle de implementación — significa que un programa c-script **no
puede devolver una página**. Sin HTML no hay render en servidor, y sin render en
servidor no hay SEO: el hallazgo salió de un intento real de migrar un sitio con
178 páginas públicas, que se frenó exactamente acá.

**Lo que hay ahora:** un rpc que devuelve `String` puede declarar el
Content-Type de su respuesta, y entonces el cuerpo es ese `String` **tal cual**,
sin las comillas de JSON alrededor. Sirve igual para un sitemap XML, un CSV, un
`robots.txt` o texto plano:

<!-- linkc:check -->
```link
type Article = { id: Int, slug: String, title: String }
type NewArticle = { slug: String, title: String }

enum Role { Admin, Member }

db { articles: Article[], }

service Site {
  @content_type("text/html; charset=utf-8")
  rpc home() -> String {
    "<!doctype html><html><head><title>Mi sitio</title></head><body><h1>Hola</h1></body></html>"
  }

  @content_type("application/xml; charset=utf-8")
  rpc sitemap() -> String {
    "<?xml version=\"1.0\"?><urlset><url><loc>https://ejemplo.com/</loc></url></urlset>"
  }

  // Una página protegida: auth y Content-Type son dimensiones distintas y se
  // combinan. Sin token, la respuesta es un 401 en JSON, no una página.
  @requires(Role.Admin)
  @content_type("text/html; charset=utf-8")
  rpc panel() -> String {
    "<h1>Panel</h1>"
  }

  // Un rpc sin la anotación sigue respondiendo JSON, igual que siempre.
  rpc list() -> Article[] {
    db.articles.all()
  }
}

test "la pagina se arma como String" {
  assert(Site.home().contains("<h1>Hola</h1>"), "el html sale entero");
  assert(Site.list().length() == 0, "y los rpc normales siguen devolviendo datos");
}
```

**Las tres piezas tienen que coincidir, y por eso las tres cambiaron:**

- `runtime/server.rs` manda el header declarado y escribe el String crudo.
- `codegen/ts_emit.rs` genera `await res.text()` para ese rpc en vez de
  `res.json()`. La primera versión no lo hacía, y el cliente generado moría con
  un `SyntaxError` sobre el primer `<` del HTML — el mismo patrón de
  "dos capas que discrepan" de §3.9: el servidor tenía razón y el cliente no se
  había enterado.
- `codegen/openapi_emit.rs` declara ese Content-Type en la respuesta 200. Si
  siguiera diciendo `application/json`, cualquier cliente generado a partir del
  spec parsearía mal.

**Reglas que impone el checker** (con su error, en tiempo de compilación):

| Caso | Por qué se rechaza |
|---|---|
| El rpc no devuelve `String` | El cuerpo se escribe tal cual; una lista de structs no es texto |
| Sobre un `stream` | SSE define su propio Content-Type por protocolo (§3.13) |
| `@content_type` dos veces | Una respuesta tiene un solo Content-Type |
| Valor vacío | No es un tipo MIME |
| Dos anotaciones de auth | `@requires` ya implica autenticado (§3.14) |

**Las anotaciones pasaron de `Option<Annotation>` a `Vec<Annotation>`.** El
modelo anterior ("a lo sumo una") hacía inexpresable justo el caso que motivó
todo esto en su forma más útil: un panel de administración es HTML **y** está
detrás de `@requires(Role.Admin)`. Combinar auth con `@content_type` ahora se
puede; combinar dos anotaciones de la misma dimensión, no.

**Límites honestos de esta ronda:**

- **Los errores de TRANSPORTE siguen saliendo en JSON**, aunque el rpc declare
  HTML -- deliberado, sigue vigente: el cliente generado espera `{"error": ...}`
  para cualquier status ≥ 400 causado por un `Err`/panic, y una página de error
  ahí rompería ese contrato justo cuando algo ya salió mal. **Resuelto para el
  caso que sí importa (§3.46):** un rpc puede pedir su PROPIO status (`response.
  setStatus`, ej. 404) en el camino de ÉXITO -- una página HTML "no encontrado"
  no es un error de transporte, es una respuesta válida con otro status.
- **El ruteo no cambió**: la URL sigue siendo `/Servicio/rpc`. Para SEO de
  verdad hacen falta rutas limpias (`/blog/mi-articulo`) y parámetros en el
  path, que es una ronda aparte — hoy se resuelve con un proxy adelante.
- **No hay helpers de plantillas.** El HTML se arma concatenando `String`, sin
  escapado AUTOMÁTICO: quien interpole datos de la base tiene que escaparlos
  a mano. **`.escapeHtml()` (§3.45) da la herramienta para eso** -- lo que
  sigue sin haber es algo que lo fuerce por default, así que sigue siendo
  responsabilidad de quien escribe el rpc acordarse de usarlo.
- **Sin `Cache-Control`, `ETag` ni compresión** — nada de la capa de caching
  HTTP es configurable todavía.

**Verificado** en `compiler/tests/cli_content_type.rs` contra el binario real:
un servidor de verdad devolviendo HTML y XML con su header, un rpc normal
respondiendo JSON igual que siempre, el cliente generado leyendo texto, el
spec OpenAPI declarando lo mismo que manda el servidor, las cuatro
combinaciones que el checker rechaza, y una página HTML detrás de
`@requires(Role.Admin)` que sin token devuelve un 401 en JSON.

---

### 3.36 PostgreSQL en runtime — RESUELTO (alcance acotado)

`runtime/postgres.rs` existía desde v1.0 y generaba DDL: `linkc build` emitía un
`schema.postgres.sql` correcto, con BIGINT/JSONB/DOUBLE PRECISION y todo. Lo que
no existía era el otro extremo: **`linkc serve` usaba SQLite siempre, sin
excepción**. No había forma de conectar un programa c-script a la base que un
equipo ya administra, y el propio README hablaba de un "adaptador PostgreSQL
enterprise" que en realidad era un generador de texto. Ningún test lo detectaba
porque los tests de esa capa comparaban strings de SQL contra strings esperados
-- nunca tocaron una base.

**Lo que hay ahora:**

```bash
linkc serve app.link 8787 --db postgres://usuario:clave@host/base
LINK_DATABASE_URL=postgres://... linkc serve app.link 8787
```

Sin `--db` ni variable de entorno, el default no cambió: `app.link` → `app.db`,
SQLite al lado del fuente (§3.17). Un valor que empieza con `postgres://` o
`postgresql://` es PostgreSQL; cualquier otro es la ruta de un archivo SQLite.

**Lo que NO cambia según el backend:** nada del lenguaje. El mismo `.link`, los
mismos `rpc`, el mismo contrato TypeScript generado, los mismos `test`. Un
programa no se entera de qué motor tiene atrás.

**Cómo está partido el código.** `runtime/store.rs` es la única capa que sabe de
motores, y lo que contiene es exactamente lo que difiere entre los dos:

| | SQLite | PostgreSQL |
|---|---|---|
| Placeholders | `?` | `$1`, `$2`, … |
| Id recién insertado | `last_insert_rowid()` | `RETURNING "id"` |
| Booleano | INTEGER 0/1 | BOOLEAN |
| Compuestos | TEXT con JSON adentro | JSONB nativo |
| Clave primaria | `INTEGER PRIMARY KEY AUTOINCREMENT` | `BIGSERIAL` |
| Deriva de esquema | falla fuerte (§3.17) | `ALTER TABLE … ADD COLUMN IF NOT EXISTS` |

Todo lo demás -- y es donde está lo difícil -- sigue siendo un solo código para
los dos: `ColumnPlan` decide qué campo va a columna nativa y cuál a JSON, con el
caso `campo?: T?` que necesita tres estados (ausente / null / valor) donde una
columna SQL tiene un solo bit de NULL. Esa regla es del LENGUAJE, no del motor,
y duplicarla por backend habría sido la forma más rápida de que los dos se
separaran con el tiempo.

Por el mismo motivo, el DDL que crea el runtime sale del MISMO
`create_postgres_table_sql` que usa `linkc build` para emitir
`schema.postgres.sql`. Si el runtime armara las tablas por su cuenta, el
esquema que el proyecto documenta y el que la base realmente tiene podrían
divergir, que es la familia de bugs que §3.9 viene registrando.

**Lo legible desde SQL.** Un enum simple se guarda como el nombre de su variante
en texto plano (`'Admin'`), no como un número; un struct anidado es JSONB de
verdad, consultable con `->>` e indexable. La promesa de "es tu Postgres de
siempre" solo vale si se puede abrir con `psql` y entender lo que hay, así que
hay un test que consulta la tabla por fuera de c-script para fijarlo.

**Migración no destructiva.** Al conectarse, el runtime hace `CREATE TABLE IF
NOT EXISTS` y después un `ADD COLUMN IF NOT EXISTS` por campo. Una tabla escrita
por una versión anterior del programa gana las columnas nuevas sin perder filas.
Es distinto a propósito de lo que hace SQLite (§3.17), que ante cualquier deriva
de esquema falla fuerte: PostgreSQL es el backend donde hay datos de producción
y volver a crear la tabla no es una opción.

**Límites honestos de esta ronda:**

- **La columna migrada siempre queda nullable**, aunque el campo sea requerido:
  `ADD COLUMN … NOT NULL` sobre una tabla con filas falla, porque no hay valor
  que poner en las que ya están. No hay forma de dar un default todavía.
- **Ninguna otra migración es automática**: renombrar un campo, cambiarle el
  tipo o borrarlo no se detecta ni se aplica. La columna vieja queda ahí.
- **Una sola conexión, sin pool -- deliberado, no pendiente.** El intérprete es
  single-threaded por diseño (§3.13) y atiende una request a la vez: nunca hay
  dos queries en vuelo al mismo tiempo, así que un pool de más de una conexión
  no compraría nada (esperarían su turno igual, solo que en una cola distinta).
  **TLS y reconexión automática sí eran gaps reales de esta ronda -- resueltos
  en §3.40.**
- **Sin transacciones expuestas.** Cada operación va sola, igual que en SQLite;
  el lenguaje no tiene todavía forma de agrupar varias en una transacción.
- **`LISTEN`/`NOTIFY` no se usa.** Los `stream` (§3.16) siguen notificando desde
  el proceso, así que dos instancias de `linkc serve` contra la misma base no se
  enteran de las escrituras de la otra. Con SQLite pasaba lo mismo; en
  PostgreSQL duele más, porque compartir la base entre instancias es
  justamente para lo que uno la elige. **Resuelto en §3.44.**

**Verificado** en `compiler/tests/pg_integration.rs` contra un PostgreSQL real
(el job `postgres` de CI levanta un `postgres:16`): el CRUD completo por HTTP
contra un servidor real, las filas sobreviviendo al reinicio del proceso, la
tabla leída desde SQL plano (incluido un `WHERE meta->>'source'`), el esquema
real comparado columna por columna contra el `schema.postgres.sql` que emite
`linkc build`, una migración que agrega un campo sin perder filas, y una URL
inválida que falla con un mensaje en vez de un panic. Si la variable de entorno
con la URL falta, el test **falla** en vez de saltearse: un test que se saltea en
silencio pasa en verde sin haber probado nada.

**Corrección post-release (20/08/2026, v1.1.1): una tabla preexistente con `id`
no entero tiraba abajo el servidor entero.** `CREATE TABLE IF NOT EXISTS` es un
no-op sobre una tabla que ya existía -- nunca mira sus columnas. Encontrado en
un intento real de migración desde un backend que ya usaba UUID como clave
primaria: apuntar `linkc serve` a esa tabla conectaba sin ninguna queja, y
recién en el primer `insert` -- `RETURNING "id"` leído como `i64` contra una
columna `uuid` vía `Row::get` (que en `tokio-postgres` panickea si el valor no
convierte al tipo pedido, a diferencia de todo lo demás en `store.rs`, que usa
`try_get`) -- el proceso moría. Y como `handle_rpc` corre sincrónico en el hilo
principal del accept-loop (`server.rs`), ese panic no tiraba abajo solo esa
request: tiraba abajo el servidor completo, para cualquier cliente conectado,
en cualquier colección.

Dos capas de arreglo:

1. `store.rs::insert_returning_id` pasa a `try_get` -- defensa en profundidad,
   ninguna lectura de PostgreSQL en el archivo debería poder panickear.
2. `Db::connect_postgres` valida el tipo de `"id"` de cualquier tabla
   preexistente ANTES de aceptar la primera request (`validate_existing_id_column`
   en `runtime/db.rs`) -- mismo momento y mismo criterio que `check_schema_matches`
   ya aplica para SQLite (§3.17), adaptado a que Postgres no recrea tablas: si
   `"id"` no es `bigint`/`integer`/`smallint`, el arranque falla con un mensaje
   que nombra la tabla y el tipo real encontrado, en vez de conectar igual y
   fallar recién en el primer insert.

Verificado en `pg_integration.rs`: una tabla creada a mano con
`id UUID PRIMARY KEY DEFAULT gen_random_uuid()`, apuntando `linkc serve` a
ella -- el arranque falla, el mensaje nombra `uuid` y la tabla, nunca aparece
`panicked at`, y el servidor nunca llega a imprimir que está escuchando.

**Qué pasa cuando dos `.link` distintos declaran la MISMA colección contra la misma base** (PLAN.md §9.1, pedido explícito en un reporte de adopción real -- "no nos atrevimos a probarlo en real"). No hay ninguna coordinación entre procesos `linkc serve` distintos -- cada uno valida y migra solo lo que SU PROPIO programa declara, así que el resultado depende de qué tan parecidos sean:

- **Columnas sin nombres en común** (`a.link` declara `name`, `b.link` declara `price`): conviven sin error. Cada `ADD COLUMN IF NOT EXISTS` agrega la columna que falta, nullable; una lectura de `b.link` sobre una fila que `a.link` insertó ve su propia columna en `null` (nunca falta la fila); `a.link` nunca ve `price` en absoluto, porque sus `SELECT` solo nombran sus propias columnas declaradas (§3.36, tabla de arriba). El riesgo real no es de lectura, es de escritura: si `a.link` dejó alguna columna `NOT NULL` (como `name` acá), un `INSERT` de `b.link` que nunca la menciona viola esa constraint -- un error limpio de Postgres, propagado como error de runtime normal, nunca un panic.
- **Un mismo nombre de columna con el MISMO tipo**: conviven sin error, ambos leen/escriben la misma columna física con la misma semántica -- es, en efecto, la forma de facto de "compartir" un campo entre dos `.link`.
- **Un mismo nombre de columna con tipos DISTINTOS**: el peligroso. `ADD COLUMN IF NOT EXISTS` es un no-op sobre una columna que ya existe -- el segundo `.link` en conectar nunca se entera de que su tipo declarado no coincide con el real (a diferencia de `"id"`, que sí se valida explícitamente arriba). El desacuerdo se descubre recién en el primer `INSERT`/`SELECT` real contra esa columna, como un error de tipo del driver de Postgres -- limpio, nunca un panic que tumbe el proceso, pero tampoco detectado al conectar.

No hay hoy ningún mecanismo de namespacing (`--db-schema`/`--db-prefix`) para evitar esto -- queda para una ronda propia (PLAN.md §9.3.10). **Recomendación mientras tanto**: si dos `.link` comparten una base, que declaren la misma colección con EXACTAMENTE los mismos campos y tipos (tratándola como una interfaz compartida), o que usen nombres de colección distintos.

Verificado en `pg_integration.rs` contra una PostgreSQL real: dos `.link` con columnas disjuntas conviven para lectura, y el `INSERT` del segundo falla limpio (sin panic) cuando pisa una columna `NOT NULL` que no conoce; dos `.link` con el mismo nombre de campo y tipos distintos (`Int` vs `String`) fallan limpio en la primera lectura real, nunca al conectar.

**`schema.postgres.sql` NUNCA pide `CREATE EXTENSION`, ninguna.** Hasta esta ronda, `generate_postgres_ddl` (`codegen/postgres_emit.rs`) emitía `CREATE EXTENSION IF NOT EXISTS "pgcrypto";` al principio de cada `schema.postgres.sql` generado -- una pregunta real de un reporte de adopción ("¿requiere superusuario en Neon/RDS/Supabase?") llevó a auditar para qué se usaba, y la respuesta fue: para nada. Ninguna función de pgcrypto (`crypt()`, `gen_random_uuid()`, `digest()`, etc.) aparece en ningún SQL que este proyecto genera o ejecuta -- `crypto.hashPassword`/`hmacSha256`/`randomToken`/etc. (§3.34/§3.38/§3.54/§3.55) son Argon2id/HMAC/CSPRNG implementados en Rust, nunca en SQL. La línea era peso muerto heredado que podía bloquear sin motivo a alguien conectado con un rol sin permiso de `CREATE EXTENSION` -- se sacó por completo, en vez de solo documentar el requisito que en realidad no existía. Verificado de la forma más directa posible: un rol Postgres real creado con `NOSUPERUSER NOCREATEDB NOCREATEROLE` y solo `GRANT CREATE, USAGE ON SCHEMA public` aplica el `schema.postgres.sql` completo sin ningún error.

---

### 3.37 `@route("/blog/:slug")`: URLs amigables para SEO — RESUELTO (alcance acotado)

§3.35 (`@content_type`) resolvió la mitad del problema de SEO: un rpc ya podía
devolver HTML de verdad. La otra mitad seguía intacta -- el ruteo es siempre
`/Servicio/rpc`, sin parámetros en el path ni rutas propias. Una página de blog
servida en `/Blog/page` (con el slug adentro de un body JSON) no es una URL que
un buscador indexe de forma útil ni que un humano pueda compartir. `@route`
cierra esa segunda mitad.

**Lo que hay ahora:**

```
@content_type("text/html; charset=utf-8")
@route("/blog/:slug")
rpc page(slug: String) -> String { ... }
```

`GET /blog/mi-primer-post` invoca `Blog.page("mi-primer-post")` -- sin body,
como manda cualquier crawler. La dirección de siempre, `/Blog/page` por POST
con `{"slug": "..."}`, **sigue funcionando exactamente igual**: `@route` es un
alias que se SUMA, nunca reemplaza nada -- el cliente TypeScript generado
sigue llamando a `/Servicio/rpc` como siempre (con un comentario nuevo que
menciona el alias, nada más).

**Reglas de forma, todas verificadas en `check_route_annotation`
(checker.rs):**

- El patrón tiene que empezar con `/`, sin segmentos vacíos.
- Un segmento `:nombre` es un parámetro. **A lo sumo el ÚLTIMO segmento puede
  serlo** -- `/blog/:slug` sí, `/:categoria/:slug` no (ver "límites" abajo).
- Sin parámetro (`@route("/sitemap.xml")`): el rpc no puede pedir NINGÚN
  parámetro -- v0 no lee query string ni body en un rpc con `@route`, a
  propósito, para que la URL sirva tal cual para un crawler sin depender de
  un POST con JSON.
- Con parámetro (`@route("/blog/:slug")`): el rpc tiene que tomar
  EXACTAMENTE ese parámetro, con ESE nombre, de tipo `String` o `Int` -- los
  únicos dos que salen de un segmento de URL sin ambigüedad. `Int` se
  obtiene parseando el segmento; si no parsea, 400 con un mensaje que nombra
  el parámetro y lo que llegó, nunca un 500 ni mucho menos un panic.
- `@route` sobre un `stream`: rechazado -- un stream no tiene una
  request/response HTTP normal a la que pegarle una URL alternativa.

**Conflictos entre rutas, detectados en compilación
(`check_route_conflicts`).** Dos `@route` con la MISMA FORMA (mismos
segmentos literales, y las dos terminan en parámetro o las dos no) son
indistinguibles al despachar una request real -- se rechaza al compilar, sin
importar el nombre del parámetro de cada una. Lo que SÍ convive sin
conflicto: una ruta literal y una con parámetro que comparten prefijo
(`/blog/featured` y `/blog/:slug`) -- la literal gana siempre que matchee
exacto, mismo criterio de precedencia que cualquier router HTTP común.

Ese criterio de precedencia fue, de hecho, el primer bug real que este mismo
feature encontró en su propio desarrollo: la primera versión de
`resolve_route` (`runtime/server.rs`) devolvía la primera entrada de la tabla
de rutas que matcheara, en orden de DECLARACIÓN -- así que `/blog/featured`
resolvía al rpc de `/blog/:slug` (declarado primero en el programa de
prueba) en vez de al rpc literal. Se encontró corriendo el servidor real y
pidiendo esa URL exacta, no leyendo el código -- exactamente el motivo por
el que este proyecto prueba contra el binario. `cli_route.rs` fija esa
precedencia como test explícito.

**Dónde vive el parser de patrones.** `compiler/src/route.rs`, un módulo sin
dependencias de `Program`/`Checker`/`Db` -- usado TAL CUAL tanto por el
checker (validar en compilación) como por `runtime/server.rs` (armar la
tabla de rutas y despachar en runtime). Una sola fuente de verdad de qué es
un patrón válido y qué significa que matchee, para no repetir la clase de
bug que este documento viene registrando desde §3.9 (dos capas que
implementan la misma regla por separado, y divergen).

**Límites honestos de esta ronda, a propósito:**

- **Un solo segmento dinámico por ruta, y tiene que ser el último.**
  `/blog/:categoria/:slug` (dos parámetros) no está soportado -- resolver
  ambigüedad entre patrones con MÁS de un segmento dinámico es una extensión
  real, no una línea de más. El caso que motivó esto (una URL humana por
  página de contenido SEO) no lo necesita. **Resuelto en §3.42**: cualquier
  cantidad de parámetros, en cualquier posición, con detección de conflictos
  generalizada.
- **Sin query string ni body en un rpc con `@route`.** Todos sus parámetros
  tienen que salir del path. Si hace falta más información, se resuelve con
  otro rpc aparte (sin `@route`), llamado desde el cliente normal.
- **No aparece en `openapi.json`.** El spec generado sigue documentando
  únicamente `/Servicio/rpc` -- que es el contrato que consume el cliente
  tipado. `@route` es la cara humana/crawler, no parte del contrato
  programático; sumarla al spec (con sus propios parámetros `in: path`) queda
  para una ronda futura si hace falta.
- **Sin trailing slash ni normalización.** `/blog/mi-post/` (con barra
  final) NO matchea `/blog/:slug` -- es un segmento vacío, rechazado como
  cualquier otro.
- **Los errores DE TRANSPORTE de un rpc con `@route` siguen siendo JSON**,
  nunca una página de error en HTML -- mismo criterio que ya vale para
  `@content_type` (§3.35): el cliente generado espera `{"error": ...}` para
  cualquier status ≥ 400 causado por un `Err`/panic. Una 404 "no encontrado"
  propia, en cambio, ya no necesita eso -- `response.setStatus` (§3.46)
  resuelve ese caso desde el camino de éxito.

Para cualquiera de estos límites -- rutas de dos parámetros, servir
estáticos de verdad -- [`docs/routing.md`](../docs/routing.md) tiene el
patrón de proxy (nginx/Caddy) que los resuelve sin tocar `linkc`.

**Verificado** en `compiler/tests/cli_route.rs` contra un servidor real,
hablando HTTP de verdad: un `GET` sin body a una ruta con parámetro String,
la precedencia literal-antes-que-dinámico, una ruta puramente literal sin
parámetros, un parámetro `Int` inválido devolviendo 400 con mensaje claro (no
un 500 ni un panic), la dirección `/Servicio/rpc` de siempre funcionando en
paralelo a la ruta linda, `@route` combinado con `@requires` devolviendo 401
en JSON sin token, un path sin ninguna ruta cayendo al 404 de siempre, y las
cuatro combinaciones que el checker rechaza (forma de dos rutas en
conflicto, parámetro que no es el último segmento, tipo no permitido,
`@route` sobre un `stream`).

---

### 3.38 `env`, `request` y `crypto.hmacSha256`: verificar webhooks de terceros — RESUELTO (alcance acotado)

Auditoría del 20/08/2026, disparada por un análisis de factibilidad de
migración real (un e-commerce evaluando mover su backend a c-script), que
encontró un bloqueo concreto: no había forma de leer un secreto de
configuración desde el lenguaje, ni de ver el body crudo de una request, ni
de calcular un HMAC — las tres cosas que hacen falta para verificar la firma
de un webhook entrante (Stripe, GitHub, o cualquier proveedor que firme sus
callbacks). Sin esto, cualquier `rpc` expuesto para recibir un webhook tenía
que confiar en el body sin verificar quién lo mandó.

**Lo que hay ahora, las tres piezas:**

| Función | Firma | Notas |
|---|---|---|
| `env.get(name)` | `(String) -> String?` | Lee `std::env::var` del proceso servidor. `None` si la variable no está seteada o no es UTF-8 válido — nunca un error. |
| `request.rawBody()` | `() -> String` | El body EXACTO de la request HTTP que invocó este rpc, antes de cualquier parseo. String vacío fuera de un servidor real (ej. `linkc test`). |
| `request.header(name)` | `(String) -> String?` | Un header de esa misma request, sin distinguir mayúsculas/minúsculas (como manda el estándar HTTP). `None` si no vino. |
| `crypto.hmacSha256(secret, message)` | `(String, String) -> String` | HMAC-SHA256 (crate `hmac` + `sha2`), hex en minúsculas — el primitivo que hace falta para verificar la firma de CUALQUIER proveedor, no solo Stripe. |

Combinadas, verifican un webhook así:

```
service Webhooks {
  rpc stripeEvent() -> String {
    let body = request.rawBody();
    let signature = request.header("Stripe-Signature");
    let secret = env.get("STRIPE_WEBHOOK_SECRET");
    // comparar `crypto.hmacSha256(secret, body)` contra `signature`
    // (con la forma exacta que documente el proveedor -- Stripe, por
    // ejemplo, firma "timestamp.body", no el body solo) antes de confiar
    // en nada de lo que sigue.
    body
  }
}
```

**De dónde sale el contexto de la request.** `Db` (ya threadeada por todo
`runtime/mod.rs` — `db: &Db` en ~11 firmas, ver §3.9) gana un
`current_request: RefCell<Option<RequestContext>>` con el body y los headers
crudos. `runtime/server.rs` lo llena al principio de CADA request, antes de
cualquier dispatch (`/Servicio/rpc` o `@route`), y la request SIGUIENTE lo
pisa antes de que su propio dispatch corra — nunca hace falta limpiarlo a
mano entre medio (hay un `clear_request_context()` al final del loop
igual, defensa en profundidad, no carga estructural). Se optó por esto en
vez de sumar un parámetro más a las ~11 firmas que ya threadean `db`/`fns`/
`checker`/`sessions`/`current_token`/`step_budget`: mismo criterio que ya
usa `Db::subscribers` (push real, §3.16) — piggybackear sobre una struct que
YA está en todos lados, en vez de tocar cada call site.

**Por qué no CSRF, en vez de construirlo.** El mismo análisis de
factibilidad marcó "sin protección CSRF" como un gap. No aplica: CSRF ataca
credenciales que el NAVEGADOR adjunta solo (cookies, autoridad ambiente) —
un sitio malicioso hace que el navegador de la víctima mande una request a
otro dominio, y esa request sale con la cookie de sesión puesta
automáticamente. La auth de c-script (§3.14) es exclusivamente
`Authorization: Bearer <token>` — un header que el navegador NUNCA adjunta
solo, tiene que escribirlo explícitamente el código JavaScript que hace el
fetch. `grep -rn "Set-Cookie\|cookie"` sobre todo `runtime/` no encuentra
ningún uso: no hay ninguna cookie que un atacante pueda hacer viajar sin
querer. Construir middleware CSRF acá sería copiar un patrón de Express que
no resuelve nada en este modelo de auth — documentado en vez de
implementado.

**Límites honestos de esta ronda:**

- **`request.rawBody()` requiere que el body sea JSON válido**, aunque el
  rpc no use ninguno de sus campos. `resolve_route` (runtime/server.rs)
  parsea el body como JSON ANTES de invocar cualquier rpc, sin importar
  cuántos parámetros declare — un body con forma de webhook real (JSON con
  más campos de los que el rpc usa, como manda cualquier proveedor) pasa
  sin problema; un body que no sea JSON en absoluto (form-encoded, XML)
  nunca llega a ejecutar el rpc. Resolver esto del todo — una anotación
  para saltear el parseo — es una extensión real, no una línea de más; el
  caso que motivó esta ronda (verificar un webhook JSON) no la necesita.
- **`env.get` lee el entorno del PROCESO servidor**, no un `.env` por
  request ni scoping por servicio — el mismo modelo que cualquier backend
  que lee `process.env`/`os.environ`.
- **Sin helper de "verificar firma" integrado.** El lenguaje da los tres
  primitivos; comparar el HMAC calculado contra el header recibido, y en
  tiempo constante, es responsabilidad de quien escribe el rpc (`==` de
  `String` en c-script no es de tiempo constante — mismo caveat que
  `verifyPassword` documentó en §3.34 antes de corregirse ahí; acá no hay
  wrapper que lo corrija por vos).

**Verificado** en `compiler/tests/cli_env_request.rs` contra un servidor
real: `env.get` con la variable seteada en el PROCESO hijo (`Command::env`,
nunca `std::env::set_var` sobre el proceso de test — mutaría estado
compartido entre tests que corren en paralelo) y sin setear (da `null`);
`request.rawBody()` devolviendo el body de un webhook realista byte a byte;
`request.header()` con y sin el header, y sin distinguir mayúsculas de
minúsculas; y que dos requests consecutivas nunca se mezclan (cada una ve
SU PROPIO body). `crypto.hmacSha256` fijado en `runtime/mod.rs` contra un
vector de referencia calculado con Python (`hmac.new(...).hexdigest()`), no
inventado a mano.

---

### 3.39 `@rate_limit("20/1m")`: límite de requests por cliente — RESUELTO (alcance acotado)

Segunda pieza del mismo análisis de factibilidad que motivó §3.38: sin forma
de limitar cuántas veces un cliente puede llamar a un rpc, cualquier
operación cara (enviar un email, cobrar una tarjeta, disparar un webhook
saliente) queda expuesta a que alguien la golpee sin límite — por accidente
(un bug en un cliente que reintenta en loop) o a propósito.

**Lo que hay ahora:**

```
@rate_limit("20/1m")
rpc sendPasswordReset(email: String) -> Void { ... }
```

Como mucho 20 requests por minuto para ESTE rpc, contadas por
`(ip_del_cliente, servicio, rpc)` — otro cliente, u otro rpc del mismo
servicio, tiene su propio cupo, sin compartirlo. Al excederlo, **429** con
`{"error": "..."}`, mismo shape de error que cualquier otro rechazo del
servidor.

**Formato del límite:** `"N/ventana"`, donde la ventana es un número
opcional seguido de `s`/`m`/`h` (`"5/s"` = 5 por segundo, `"20/1m"` = 20 por
minuto, `"100/2h"` = 100 cada dos horas). Validado en compilación
(`check_rate_limit_annotation`, checker.rs) contra el mismo parser que usa
el servidor en runtime (`compiler/src/rate_limit.rs`, `RateLimitSpec::parse`)
— un solo lugar que decide qué es un límite válido, mismo motivo que
`route.rs` (§3.37) para existir aparte: dos capas que implementan la misma
regla por separado terminan divergiendo (§3.9).

**Algoritmo: token bucket con refill continuo**, no un contador de ventana
fija. Un bucket de capacidad N se rellena a razón de N/ventana por segundo,
así que una ráfaga corta al principio de la ventana no deja "muerto" el
resto de la ventana como pasaría con un contador que resetea de golpe en el
límite exacto (el problema clásico de "doble ráfaga en el borde de la
ventana" de un contador de ventana fija).

**De dónde sale la IP del cliente — y de dónde NO.** `Request::remote_addr()`
de `tiny_http`: la IP de la conexión TCP real. **Nunca** un header como
`X-Forwarded-For`, que cualquier cliente puede mandar con el valor que
quiera — confiar en él sin un mecanismo de "proxy de confianza" configurado
(que v0 no tiene) dejaría a cualquiera evadir el límite mandando un header
distinto en cada request. Consecuencia honesta: detrás de un proxy o
balanceador, esto limita por la IP del proxy, no la del usuario final — la
misma limitación que documenta cualquier rate limiter que no conoce su
topología de red por adelantado.

**Combina con cualquier otra anotación** (`@authenticated`, `@requires`,
`@content_type`, `@route`) — es una dimensión ortogonal, igual que
`@content_type`/`@route` lo son entre sí desde §3.35. Corre **antes** que el
gate de auth (`check_auth_gate`, runtime/server.rs), a propósito: si
corriera después, un rpc protegido dejaría probar credenciales sin límite
alguno (un 401 no cuesta nada de recursos reales, así que no frenarlo ahí
sería inútil contra fuerza bruta).

**Límites honestos de esta ronda:**

- **El estado vive en memoria del proceso**, un solo `RateLimiter` para todo
  el servidor (mismo modelo de concurrencia que `route_table`: se arma y
  muta en el hilo principal, nunca cruza a los hilos de escritura de
  stream). Un reinicio del proceso resetea todos los buckets — no hay
  persistencia entre despliegues, ni coordinación entre réplicas si el
  mismo `.link` corre en más de un proceso a la vez.
  - Reinicios frecuentes en un ambiente de despliegue agresivo (o correr N
    réplicas detrás de un balanceador) diluyen el límite real -- es un
    límite POR PROCESO, no global de la aplicación.
- **Barrido de buckets inactivos, no eviction fina.** Cada 1000 checks se
  descartan los buckets sin actividad hace más de una hora, así un proceso
  de larga vida con muchos clientes distintos no crece sin límite en
  memoria — pero no hay un tope duro de cuántos buckets simultáneos puede
  haber entre barridos.

**Verificado** en `compiler/tests/cli_rate_limit.rs` contra un servidor
real: las primeras N requests pasan y la N+1 da 429, un rpc SIN
`@rate_limit` no se ve afectado por haber agotado el bucket de otro, la
combinación con `@requires` (el límite corre antes que el 401, así que se
agota aunque ninguna request traiga token válido), y los formatos que el
checker rechaza (sin conteo, conteo/ventana en cero, unidad de ventana
desconocida, declarado dos veces). Unit tests del parser y del token bucket
en `compiler/src/rate_limit.rs`.

---

### 3.40 PostgreSQL: TLS y reconexión automática — RESUELTO (alcance acotado)

Auditoría del 20/08/2026, misma fuente que §3.38/§3.39 (un análisis de
factibilidad de migración real) y misma frase señalada como bloqueo: "Postgres
sin pool, TLS ni reconexión". §3.36 ya documentaba las tres como límites
honestos. De las tres, **pool no era un gap real** (ver el bullet actualizado
en §3.36: con el intérprete atendiendo una request a la vez, una segunda
conexión ociosa no compra nada) -- las otras dos sí, y son las que cierra esta
ronda.

**TLS, vía `rustls` puro -- sin OpenSSL ni ninguna librería nativa del
sistema.** La razón de elegir `rustls` (crates `rustls` + `tokio-postgres-rustls`,
backend criptográfico `ring`) en vez de `native-tls`/`postgres-openssl`: los 4
targets de release (Linux, Windows, macOS x86_64 y ARM, GRAMMAR.md — ver
`.github/workflows/release.yml`) tienen que seguir compilando sin instalar
nada del sistema operativo -- `rustls` es Rust puro, sin ese riesgo.

`sslmode` sale de la URL de conexión, estándar libpq (`postgres://.../db?sslmode=...`),
`postgres::Config` ya lo parsea:

| `sslmode` | Comportamiento |
|---|---|
| `disable` | Texto plano, sin intentar TLS -- igual que ANTES de esta ronda. |
| *(sin especificar)* | **Cambia de default**: antes era texto plano siempre; ahora intenta TLS primero y, si el servidor no lo ofrece, sigue en texto plano solo (`tokio-postgres` maneja ese fallback, sin código extra acá) -- mismo comportamiento que `sslmode=prefer` explícito. |
| `require` | TLS obligatorio; si el servidor no lo ofrece, la conexión falla con un mensaje claro. |

El cambio de default es intencional: cualquier servicio administrado real
(Supabase, Neon, RDS, Railway, etc.) exige TLS hoy, y antes de esta ronda
`linkc serve` era literalmente incapaz de conectarse a uno de ellos --
`postgres::NoTls` no negocia, punto. El efecto práctico para quien apunta a un
Postgres local sin TLS configurado (el caso más común en desarrollo, y el que
usa la CI de este mismo repo) es ninguno: el fallback a texto plano es
transparente.

**Reconexión automática, con una elección deliberada sobre qué SÍ
reintentar.** `Backend::Postgres` guarda ahora la URL de conexión junto al
`postgres::Client`, y las cuatro operaciones que tocan la base
(`execute`/`execute_ddl`/`query`/`insert_returning_id`, `runtime/store.rs`)
pasan por `with_reconnect`: si el error es de conexión cerrada
(`Error::is_closed()`, de `tokio-postgres`), reemplaza el cliente por uno
nuevo (reconectado con el MISMO criterio de TLS que el arranque -- una sola
función, `connect_postgres_client` en `runtime/db.rs`, para las dos cosas, por
el mismo motivo de siempre: dos lugares abriendo la conexión con criterios
distintos es la clase de divergencia que GRAMMAR.md §3.9 viene registrando).

Lo deliberado: **`with_reconnect` NUNCA reintenta la operación que encontró la
conexión cortada.** La request que la encuentra sigue devolviendo su error tal
cual (un 500 con el mensaje real) -- lo que cambia es que la conexión queda
sana ANTES de que ese error vuelva, así que la PRÓXIMA request (otro intento
del cliente, u otro rpc cualquiera) ya encuentra la base reconectada, en vez de
que el proceso entero quede sirviendo error tras error hasta un reinicio
manual, que era el comportamiento de antes de esta ronda.

La razón de NO reintentar automáticamente la operación en sí: una conexión
puede cortarse en cualquier punto de una request -- incluido DESPUÉS de que el
servidor ya aplicó un `INSERT`, pero ANTES de que la respuesta (`RETURNING
"id"`) llegara al cliente. Reintentar a ciegas en ese caso insertaría una fila
DUPLICADA, sin que quien llamó al rpc se entere. "Falla una vez, se cura sola
para la próxima" es la garantía correcta acá -- transparencia total (reintentar
también la escritura) habría sido más cómoda pero incorrecta.

**Límites honestos de esta ronda:**

- **La request que encuentra la conexión cortada falla igual.** No hay
  reintento transparente de la operación en sí, a propósito (ver arriba) --
  quien llama sigue viendo ese error puntual; solo la request SIGUIENTE se
  beneficia de la reconexión.
- **Un solo intento de reconexión por request que falla.** Si ese intento
  también falla (la base sigue caída, no solo esa conexión), el cliente viejo
  queda como estaba y la request siguiente vuelve a intentarlo sola -- no hay
  backoff exponencial ni un límite de reintentos distinto de "una vez por
  request que topa con el problema".
- **Un handshake TLS de verdad completado contra un servidor que sí ofrece
  SSL no tiene un fixture de CI propio en este repo todavía** -- levantar un
  `postgres:16` con certificados en GitHub Actions es una pieza aparte de
  infraestructura, no prototipeada localmente antes de este cambio (sin
  Docker ni un Postgres local a mano en esta ronda). Lo que SÍ está verificado
  en CI es la integración completa contra el Postgres real de siempre
  (`sslmode` sin especificar, negociando y cayendo a texto plano porque ese
  servidor no tiene TLS) y el camino explícito `sslmode=disable`. La conexión
  usa la integración pública estándar de `tokio-postgres` + `rustls`
  (`tokio-postgres-rustls`), no código de negociación TLS propio, así que el
  riesgo residual es bajo -- pero no está clavado con un test end-to-end
  contra un servidor que de verdad exija TLS.
- **Sigue sin haber `LISTEN`/`NOTIFY`.** Dos instancias de `linkc serve` contra
  la misma base todavía no se enteran de las escrituras de la otra en un
  `stream` -- eso necesitaría un hilo de escucha aparte, y el modelo de
  concurrencia actual (todo en el hilo principal, `RefCell` en vez de
  `Mutex`, ver §3.13) no lo soporta sin un rediseño real. Sigue siendo el
  límite más grande de correr más de una instancia contra la misma base.

**Verificado** en `compiler/tests/pg_integration.rs` contra un PostgreSQL
real: `sslmode=disable` conectando en texto plano tal cual antes;
`sslmode` sin especificar conectando igual contra un servidor sin TLS
configurado (prueba el fallback de `Prefer`); y un test que corta la
conexión de verdad -- `pg_terminate_backend` desde una conexión
administrativa aparte, identificando la del servidor por
`application_name` -- y reintenta hasta que el MISMO proceso (nunca
reiniciado) vuelve a servir, confirmando además que la fila insertada
ANTES del corte sigue ahí y una fila nueva después del corte se puede
crear. Deliberadamente no afirma sobre si la PRIMERA request después del
corte falla o no -- es una carrera contra cuándo el cliente interno nota la
conexión cerrada, y fijar ese detalle habría sido un test frágil por
diseño; lo que se prueba es la propiedad real: que se recupera solo, en un
plazo razonable, sin reiniciar el proceso.

---

### 3.41 CORS configurable y headers de seguridad — RESUELTO (alcance acotado)

Última pieza de la misma auditoría que §3.38/§3.39/§3.40. Antes de esta
ronda, `linkc serve` mandaba `Access-Control-Allow-Origin: *` en TODA
respuesta, sin forma de acotarlo -- una API con auth Bearer (§3.14) servida
así deja que CUALQUIER página, de cualquier origen, lea la respuesta de un
rpc si el navegador de quien la visita ya tiene un token guardado (en
`localStorage`, por ejemplo) y ese sitio hace el fetch con el header
`Authorization` puesto. `*` no protege nada ahí: es la sesión del usuario la
que importa, no la del sitio que hace la request. Tampoco había ningún
header de seguridad más allá de CORS.

**CORS configurable, opt-in:**

```bash
linkc serve app.link 8787 --cors-origin https://app.midominio.com --cors-origin https://admin.midominio.com
LINK_CORS_ORIGINS=https://app.midominio.com,https://admin.midominio.com linkc serve app.link 8787
```

Sin ninguno de los dos, el default NO cambia: `*`, igual que siempre --
ningún despliegue existente se rompe por actualizar. Con al menos un origen
configurado, el servidor pasa a un **allowlist real**: el `Origin` de la
request entrante se compara EXACTO contra la lista, y

- si matchea, `Access-Control-Allow-Origin` se manda con ESE valor exacto
  (nunca `*`) más `Vary: Origin` (la respuesta depende de qué origen pidió,
  así que un cache intermedio no puede servir la respuesta pensada para un
  origen a otro distinto);
- si no matchea (o no vino `Origin` en absoluto -- el caso normal de un
  server-to-server o `curl`), el header se OMITE por completo. La request
  se procesa igual (200, con la respuesta real) -- CORS lo hace cumplir el
  NAVEGADOR sobre la respuesta, nunca el servidor rechazando la request; sin
  el header, el navegador de quien sí lo respeta bloquea que el JavaScript
  de ese origen lea el body.

`--cors-origin` gana sobre `LINK_CORS_ORIGINS` (mismo criterio de
precedencia que `--db`/`LINK_DATABASE_URL`, §3.36).

**Headers de seguridad fijos, siempre, en TODA respuesta** (incluidas las de
error -- un 401/404/429 los necesita igual, y un `stream` SSE también, ver
abajo):

| Header | Valor | Para qué |
|---|---|---|
| `X-Content-Type-Options` | `nosniff` | El navegador no "adivina" el tipo real de un body y lo ejecuta como algo distinto del `Content-Type` declarado. |
| `X-Frame-Options` | `DENY` | Ninguna respuesta de este servidor se puede embeber en un `<iframe>` de otro sitio -- protección contra clickjacking. |
| `Referrer-Policy` | `no-referrer` | La URL completa de una request a este servidor (que puede llevar datos en el path o la query) nunca sale en el header `Referer` de un link que salga desde una página servida por acá. |

**Deliberadamente AFUERA de esta ronda, y por qué:**

- **CSP (`Content-Security-Policy`)**: depende del CONTENIDO de cada página
  -- qué scripts/estilos carga, desde dónde. Un default fijo o rompe páginas
  legítimas (`@content_type("text/html")`, §3.35) que cargan algo externo, o
  no protege nada por ser demasiado laxo. Sin una forma de que el programa
  declare su propia política, un CSP del lado del servidor sería adivinar.
- **HSTS (`Strict-Transport-Security`)**: solo tiene sentido sobre una
  conexión que YA es HTTPS -- mandarlo sobre HTTP (que es todo lo que
  `linkc serve` habla, ver el modelo de despliegue en
  [`docs/routing.md`](../docs/routing.md)) sería una promesa falsa. Le
  corresponde a quien SÍ termina TLS -- el reverse proxy (nginx/Caddy) del
  deploy real, no al proceso c-script.
- **`Access-Control-Allow-Credentials`**: solo importa para requests que
  llevan cookies o `credentials: "include"`. La auth de c-script es
  exclusivamente `Authorization: Bearer` (§3.14, sin `Set-Cookie` en ningún
  lado del runtime) -- no hay credential ambiente que este header necesite
  habilitar.
- **Ningún origen con wildcard parcial** (`https://*.midominio.com`): cada
  entrada de la allowlist es un match EXACTO, sin patrones. Un subdominio
  nuevo necesita agregarse a la lista explícitamente.

**Dónde vive el código.** `runtime/server.rs`: `CorsConfig` (la política,
armada una vez al arrancar) y `CorsHeaders` (ya resuelta para una request
puntual, contra su `Origin`) son los mismos dos tipos que usan TANTO
`cors_response_with_type` (la respuesta normal de un rpc, vía el builder de
`tiny_http`) COMO `sse_preamble` (el header de un `stream`, armado a mano
byte a byte -- ver §3.13 sobre por qué un `stream` no puede pasar por el
builder normal). Una sola función resuelve la política por request
(`CorsConfig::headers_for`); las dos rutas de escritura la consumen igual,
así que no pueden divergir en qué mandan -- el motivo de siempre (§3.9).

**Verificado** en `compiler/tests/cli_cors.rs` contra un servidor real: el
default `*` sin romper nada, un origen permitido ecoado exacto con
`Vary: Origin`, uno no permitido sin el header (pero la request igual
procesada, 200 con el body real), el preflight `OPTIONS` con el mismo
criterio, la precedencia `--cors-origin` sobre `LINK_CORS_ORIGINS`, los tres
headers de seguridad presentes en una respuesta de ERROR (500 por un rpc
inexistente), y -- el caso que más importaba fijar -- que un `stream` SSE
respeta la MISMA allowlist que un rpc normal, no la política vieja
hardcodeada que tenía antes.

---

### 3.42 `@route` con múltiples parámetros — RESUELTO (alcance acotado)

§3.37 dejó un límite explícito, documentado como "extensión real, no una
línea de más": como mucho UN segmento dinámico por ruta, y tenía que ser el
ÚLTIMO -- `/blog/:categoria/:slug` (dos parámetros) no estaba soportado.
Auditoría del 20/08/2026: esa extensión.

**Lo que hay ahora:**

```
@route("/blog/:categoria/:slug")
rpc page(slug: String, categoria: String) -> String { ... }
```

Cualquier cantidad de segmentos `:nombre`, en CUALQUIER posición -- no
tienen que ser los últimos, ni ir todos juntos. El binding entre la ruta y
los parámetros del rpc es por NOMBRE, no por posición: `page` arriba declara
`slug` antes que `categoria` en su firma, en el orden contrario al de la
ruta, y funciona igual -- lo único que importa es que el CONJUNTO de nombres
coincida exacto (ni de más, ni de menos), mismo criterio que ya valía para
el caso de un solo parámetro.

**Precedencia, generalizada.** El criterio de siempre ("una ruta literal le
gana a una dinámica que también matchearía", §3.37) se generaliza a
**especificidad**: cuenta cuántos segmentos de cada ruta son literales
fijos, y la que tiene MÁS gana, determinísticamente, cuando las dos podrían
matchear el mismo path real. `/blog/featured/:slug` (1 literal) le gana a
`/blog/:categoria/:slug` (0 literales) para el path `/blog/featured/algo`.

**Conflictos, generalizados -- el caso que de verdad ameritaba pensarlo con
cuidado.** No alcanza con comparar la FORMA exacta de dos rutas (lo que
hacía §3.37: mismos segmentos literales, y las dos terminan en parámetro o
las dos no). Con múltiples parámetros en distintas posiciones, dos rutas de
forma DISTINTA pueden igual matchear el mismo path real sin que ninguna sea
más específica:

```
@route("/blog/:categoria/latest")   // 1 literal (posición 1)
@route("/blog/featured/:slug")      // 1 literal (posición 0)
```

Ninguna tiene la misma forma que la otra (en la posición 0 una es parámetro
y la otra literal, y viceversa en la 1), pero las dos matchean
`/blog/featured/latest` -- y las dos tienen exactamente UN segmento
literal, así que ninguna es más específica. Eso es un conflicto real, y el
checker lo rechaza, aunque las dos formas sean técnicamente distintas.

La regla exacta (`RoutePattern::conflicts_with`, `compiler/src/route.rs`):
dos rutas del mismo largo conflictúan si (a) podrían matchear el mismo path
-- no hay ninguna posición donde las dos sean literales con texto DISTINTO,
lo único que prueba que nunca se cruzan -- Y (b) tienen la misma
especificidad (mismo número de segmentos literales). Si difieren en
especificidad, no es un conflicto: la más específica gana sola, sin
ambigüedad.

**Nombres de parámetro repetidos dentro de la MISMA ruta** (`/:slug/comentarios/:slug`)
se rechazan al parsear: un valor capturado no puede bindear a dos lugares
distintos del rpc a la vez sin que uno de los dos gane arbitrariamente.

**Dónde vive el código.** Todo en `compiler/src/route.rs` -- el mismo módulo
compartido de siempre entre checker (`check_route_conflicts`,
`check_route_annotation`) y runtime (`build_route_table`/`resolve_route` en
`runtime/server.rs`). La tabla de rutas se ordena UNA vez, al arrancar, por
especificidad descendente; `resolve_route` hace una sola pasada y se queda
con el primer match -- que por construcción es el más específico, porque el
checker ya garantizó que nunca hay dos entradas empatadas que puedan
matchear el mismo path real.

**Límites honestos de esta ronda:**

- **Sigue sin query string ni body en un rpc con `@route`.** Todos sus
  parámetros tienen que salir del path -- mismo límite que §3.37, sin
  cambios.
- **Sin segmentos "catch-all"** (`/docs/:resto*`, capturando el resto del
  path como un solo valor). Cada `:nombre` es exactamente un segmento.
- **La detección de conflictos es conservadora, no exhaustiva sobre
  TODAS las combinaciones posibles de valores.** Rechaza cualquier PAR de
  rutas que estructuralmente podrían cruzarse (aunque en la práctica el
  programa nunca reciba ese path exacto) -- puede rechazar una combinación
  que en los hechos nunca iba a pasar, a cambio de la garantía más fuerte
  de nunca dejar pasar una ambigüedad real sin avisar.

**Verificado** en `compiler/src/route.rs` (unit tests del parser, de
`conflicts_with` con el caso de conflicto cruzado de arriba, y del binding
de múltiples valores en el orden correcto) y en `compiler/tests/cli_route.rs`
contra un servidor real: una ruta de dos parámetros bindeando por nombre
en un orden distinto al de la firma del rpc, y una ruta con un segmento
literal ganándole a una totalmente dinámica del mismo largo.

---

### 3.43 `smtp.send`: mandar email — RESUELTO (alcance acotado)

Última pieza de la misma auditoría que §3.38-§3.42. Un backend real casi
siempre necesita mandar mail -- confirmar un registro, resetear una
contraseña, notificar un evento -- y no había ninguna forma de hacerlo desde
c-script.

**Lo que hay ahora:**

```
smtp.send(to: String, subject: String, body: String) -> Void
```

```
rpc register(email: String) -> Void {
  // ... crear el usuario ...
  smtp.send(email, "Bienvenido", "Gracias por registrarte.");
}
```

**De dónde salen la conexión y el remitente -- del ENTORNO, nunca de
argumentos del rpc.** Dos variables de entorno, mismo criterio que
`LINK_DATABASE_URL` (§3.36):

| Variable | Contenido |
|---|---|
| `LINK_SMTP_URL` | Connection string de `lettre` (la librería detrás): `smtps://usuario:clave@host` (TLS implícito, puerto 465 por default -- la opción recomendada), `smtp://usuario:clave@host?tls=required` (STARTTLS obligatorio), o `smtp://host` sin más (sin cifrar, para un relay local de desarrollo). |
| `LINK_SMTP_FROM` | La dirección remitente, opcionalmente con nombre (`"Mi App <no-reply@midominio.com>"`). |

La razón de que estas dos NO sean parámetros del rpc: un `.link` no debería
poder hardcodear ni filtrar credenciales de un relay SMTP en su código
fuente, y dejar que cualquier caller elija el remitente abriría la puerta a
que datos de la request terminen falsificando el `From:` de un email real
(el mismo tipo de riesgo que `@rate_limit`/CORS vienen tratando con
cuidado en esta misma serie de rondas).

**TLS, mismo stack que Postgres (§3.40).** `lettre` con el feature
`rustls-tls` -- `rustls` + `ring` + `webpki-roots`, sin OpenSSL ni ninguna
dependencia nativa del sistema, para que los 4 targets de release sigan
compilando sin instalar nada.

**Fallas, todas como error de runtime normal (igual que `http.get`/`http.post`,
no como un `Result<T,E>` del lenguaje):** variable de entorno faltante,
dirección inválida (remitente o destinatario), o el relay inalcanzable
vuelven un mensaje claro que nombra qué falló -- nunca un panic, nunca tira
abajo el servidor.

**Límites honestos de esta ronda:**

- **Un solo destinatario por llamada.** Sin `cc`/`bcc`, sin lista de
  destinatarios. Mandar a varios es una llamada a `smtp.send` por cada uno.
- **Solo texto plano.** Sin HTML, sin adjuntos. El body es el `String` tal
  cual, sin ningún tipo de contenido MIME alternativo.
- **Sin plantillas.** Armar el `subject`/`body` (con interpolación de
  strings, `+`) es responsabilidad de quien escribe el rpc.
- **Sin cola ni reintento.** `smtp.send` es sincrónico: bloquea el hilo del
  servidor (single-threaded, §3.13) hasta que el relay responde o falla. Un
  relay lento hace lento a TODO el servidor mientras dura ese envío -- igual
  trade-off que ya vale para `crypto.hashPassword` (§3.34) y cualquier otra
  operación bloqueante del intérprete.

**Verificado** en `compiler/tests/cli_smtp.rs` contra un servidor SMTP real
-- escrito a mano en el propio archivo de test (sin ninguna dependencia
externa al binario, para que corra igual en CI que en cualquier máquina):
habla lo mínimo del protocolo (`EHLO`/`MAIL FROM`/`RCPT TO`/`DATA`) para que
`smtp.send` complete un envío de punta a punta de verdad, y el test confirma
que el remitente, destinatario, asunto y cuerpo que el servidor de mentira
RECIBIÓ son exactamente los que se mandaron -- más los cuatro caminos de
error (falta `LINK_SMTP_URL`, falta `LINK_SMTP_FROM`, dirección inválida,
relay inalcanzable), todos devolviendo un 500 con mensaje claro, nunca un
panic.

---

### 3.44 PostgreSQL LISTEN/NOTIFY: `stream` entre varias instancias — RESUELTO (alcance acotado)

Último límite honesto que quedaba de §3.36: dos instancias de `linkc serve`
contra la misma base de Postgres no se enteraban de las escrituras de la
otra en un `stream` -- cada una solo publicaba a sus PROPIOS suscriptores
(`Db::subscribers`, en memoria de ESE proceso). Para SQLite esto es
inherente (un archivo no tiene ningún mecanismo de notificación
cross-proceso); para Postgres dolía más, porque compartir la base entre
instancias es justamente para lo que uno la elige -- correr más de un
`linkc serve` detrás de un balanceador, contra la misma base, es un patrón
de despliegue real.

**Lo que hay ahora:** con Postgres, cada `insert`/`applyPatch`/`delete`
que ya se publicó LOCAL además manda `NOTIFY` -- cualquier otra instancia
de `linkc serve` contra la misma base lo recibe y lo vuelve a publicar
LOCAL en su propio proceso. Un suscriptor conectado a la instancia B ve una
escritura que entró por la instancia A, sin ninguna diferencia respecto de
si hubiera entrado por B misma. Sin configuración: si el backend es
Postgres, esto está andando solo.

**Cómo está armado, y por qué:**

- **Una conexión SEPARADA, dedicada, solo para `LISTEN`.** La conexión de
  queries normales (`Backend::Postgres`, §3.40) no puede a la vez bloquear
  esperando notificaciones Y ejecutar SELECT/INSERT/UPDATE sincrónicos --
  son dos usos que no comparten una sola conexión de la crate `postgres`.
  Un hilo de fondo dedicado la abre al arrancar y hace `LISTEN
  link_stream_changes` -- UN SOLO canal para TODAS las colecciones del
  programa (el nombre de la colección va adentro del payload JSON, no en
  el nombre del canal), así que hace falta un único `LISTEN` sin importar
  cuántas colecciones declare `db { ... }`.
- **El hilo de LISTEN se auto-repara**, mismo espíritu que la reconexión de
  la conexión de queries (§3.40): si se corta, lo nota (`Ok(None)` del
  iterador de notificaciones, o un error) y la reabre sola cada 5 segundos
  -- un problema de red no deja la propagación cross-instancia rota para
  siempre sin un reinicio manual.
- **`NOTIFY` vía `pg_notify()` (forma de función), no la sentencia `NOTIFY
  canal, 'texto'`.** El payload es un parámetro bindeado, no un literal SQL
  armado a mano -- ni escapado manual, ni riesgo de inyección con datos que
  vienen de una fila real.
- **Cada instancia se reconoce a sí misma, para no duplicar su propio
  evento.** El payload lleva un `instance` -- un id aleatorio del CSPRNG,
  generado una vez por proceso al conectar. Cuando el hilo de LISTEN de una
  instancia recibe un NOTIFY con SU PROPIO `instance`, lo descarta: ese
  cambio ya se entregó local, en el momento de escribir (`Db::publish`), y
  reinyectarlo de nuevo lo entregaría dos veces a los mismos suscriptores.
  Un cambio de OTRA instancia sí se reinyecta local (`Db::publish_remote`)
  -- y esa función nunca vuelve a hacer `NOTIFY`: si lo hiciera, cada
  instancia reenviaría el cambio de las demás sin parar nunca.
- **El loop principal del servidor pasó de bloquear en
  `incoming_requests()` a `recv_timeout` con un intervalo corto (200 ms).**
  Solo cuando hay un canal de cambios remotos que atender (Postgres); con
  SQLite el loop bloqueante de siempre sigue igual, sin el overhead de
  este polling. En cada vuelta, antes de esperar la próxima request, drena
  lo que haya llegado por el canal y lo publica local -- así un cambio
  remoto no queda esperando indefinidamente si el servidor está inactivo
  (sin ninguna request HTTP nueva que "despierte" el loop por su cuenta).

**Límites honestos de esta ronda:**

- **El payload de `NOTIFY` tiene el límite de 8000 bytes que impone
  Postgres mismo.** Un cambio cuyo JSON completo (con el envoltorio
  `instance`/`collection`) supere ese tamaño simplemente no se propaga a
  otras instancias -- se loguea un aviso, no se parte ni se comprime (eso
  abriría su propia complejidad para un caso de borde). El cambio sigue
  publicándose local en la instancia donde se escribió, como siempre.
- **`NOTIFY` es best-effort, sin cola ni reintento.** Si falla (la conexión
  de queries también está cortada en ese instante, por ejemplo), el
  `insert`/`applyPatch`/`delete` en sí IGUAL tuvo éxito y se publicó local
  -- solo la propagación cross-instancia de ESE cambio puntual no llegó.
  No hay un mecanismo de "reintentar más tarde" ni de "avisar qué se
  perdió".
- **Latencia de hasta 200 ms cuando el servidor está inactivo.** Con
  requests HTTP llegando seguido, el canal se drena en cada vuelta del
  loop, así que la propagación es casi inmediata; sin ninguna request
  nueva, el intervalo de `recv_timeout` es lo que determina cuánto puede
  tardar en notarse un cambio remoto.
- **Una conexión Postgres MÁS por instancia** (dos en total: queries +
  LISTEN, en vez de una sola como hasta §3.40).
- **Solo Postgres.** SQLite sigue sin ningún mecanismo de notificación
  cross-proceso -- dos instancias de `linkc serve` sobre el MISMO archivo
  SQLite (patrón de despliegue que este proyecto no recomienda de todos
  modos, ver §3.17) siguen sin verse entre sí.

**Verificado** en `compiler/tests/pg_integration.rs` contra un PostgreSQL
real: DOS procesos `linkc serve` DISTINTOS (subprocesos reales, no dos
hilos del mismo proceso) apuntando a la misma base, un `stream` conectado
por un `TcpStream` crudo a la instancia A (leyendo los eventos SSE tal
cual llegan, sin ningún cliente de por medio), un `insert` que entra por
la instancia B vía su `/Service/rpc` normal, y la instancia A recibiendo
ese cambio -- confirmando tanto el id como el resto de los campos --
dentro de un plazo razonable, sin compartir memoria ni proceso con B. Unit
tests del parseo del payload de NOTIFY (`parse_remote_notification`,
`runtime/db.rs`): un payload bien formado de otra instancia se acepta, el
propio eco se descarta, y un payload mal formado se ignora en vez de
panickear.

---

### 3.45 `String.escapeHtml()`: sanitizar datos en una página — RESUELTO (alcance acotado)

§3.35 (`@content_type`) permite devolver HTML de verdad desde un rpc, pero
la respuesta se arma concatenando `String` -- nada escapaba por vos los
datos que interpolabas. Un nombre de usuario, un comentario, cualquier
texto que no controla el propio programa, podía llevar `<script>` o un
atributo `onerror=` y terminar ejecutándose en el navegador de quien mira
la página: el mismo problema que resuelve el auto-escape de cualquier motor
de templates real (Django, Rails ERB, JSX).

**Lo que hay ahora:**

```
rpc page(comentario: String) -> String {
  "<p>" + comentario.escapeHtml() + "</p>"
}
```

Un método más sobre `String` (mismo lugar que `.trim()`/`.toUpper()`/
`.startsWith()`), no un tipo de string nuevo ni un sistema de templates con
auto-escape implícito -- deliberado: c-script no tiene una construcción de
"template", así que auto-escapar por default habría significado inventar
una encima de la nada (un tipo `HtmlString` que distinga "ya seguro" de "sin
escapar", con su propia complejidad de cuándo se permite mezclar uno con
otro). Un método explícito da la herramienta sin esa complejidad: quien
arma HTML decide qué interpolar tal cual (el markup propio) y qué pasar por
`.escapeHtml()` (los datos que no controla).

Escapa los 5 caracteres que HTML interpreta como marcado en vez de texto --
mismo set que cualquier escapador estándar (`html.escape` de Python, las
guías de OWASP):

| Caracter | Se convierte en |
|---|---|
| `&` | `&amp;` |
| `<` | `&lt;` |
| `>` | `&gt;` |
| `"` | `&quot;` |
| `'` | `&#39;` |

`&` se escapa PRIMERO, a propósito: si se escapara después de los demás, el
`&` que esas mismas entidades acaban de insertar (`&lt;`, `&quot;`, ...) se
escaparía de nuevo, dejando `&amp;lt;` en vez de `&lt;`.

**Límites honestos de esta ronda:**

- **No es automático.** Nada fuerza a escapar -- un rpc que concatena datos
  sin pasarlos por `.escapeHtml()` sigue compilando y sirviendo esa página
  tal cual, vulnerable. La herramienta existe; usarla en el lugar correcto
  sigue siendo responsabilidad de quien escribe el rpc.
- **Solo texto de nodo y atributo ENTRE COMILLAS -- dobles o simples, las
  dos (corrección del 23/08/2026: una versión anterior de este párrafo
  decía "solo comillas dobles", pero `'` también se escapa a `&#39;`, así
  que un atributo `'...'` es igual de seguro) --, no todos los contextos
  HTML.** NO cubre interpolar directo dentro de un bloque `<script>`/
  `<style>` (ahí hace falta escape de JS/CSS, no de HTML -- son reglas
  completamente distintas, ningún escapador de HTML las resuelve), ni un
  atributo sin comillas (`onerror=comentario`, sin `"` ni `'`): la mitigación
  correcta para ese último caso es no escribir atributos sin comillas, no
  un escape más agresivo.
- **Sin sanitización de HTML "permitido a medias"** (dejar pasar `<b>` pero
  no `<script>`, el caso de un editor de texto enriquecido). Esto es
  escape TOTAL -- todo interpolado se vuelve texto plano visible, nunca
  markup. Para HTML parcialmente confiable hace falta una librería de
  sanitización de verdad, que este método no reemplaza.

**Verificado** en `compiler/tests/cli_content_type.rs` contra un servidor
real: un payload de XSS de libro (`<img src=x onerror=alert(1)>`) mandado
como parámetro de un rpc con `@content_type("text/html")`, confirmando que
la respuesta HTTP real lo devuelve escapado -- `<img` nunca aparece tal
cual en el body -- no solo que el método produce el string esperado en
aislamiento. Unit tests adicionales (`runtime/mod.rs`) fijan los 5
caracteres y el orden de escape (`&` primero).

### 3.46 `response.setStatus(code)`: página 404 propia para un `@route` — RESUELTO

Último límite honesto real que quedaba de §3.35/§3.37: un rpc `@route` +
`@content_type("text/html")` -- pensado para que alguien navegue a esa URL
directo desde el navegador, no para el cliente generado -- solo podía
devolver 200. "No encontrado" no tenía forma de ser otra cosa que un
`Err`/panic, y un error SIEMPRE sale como JSON (§3.35), rompiendo justo la
página HTML que se quería mostrar en el peor momento.

**La pregunta de diseño real no era sobre `Result<T,E>`.** La primera idea
-- dejar que `@content_type` acepte `Result<String, E>`, con `Err`
mapeando a un status fijo -- rompe el contrato que el cliente generado ya
asume (`Result<T,E>` viaja siempre como `{type:"Ok"|"Err", ...}` en un 200,
GRAMMAR.md §3.5) y además no resuelve nada: `E` es un `enum` de dominio,
no HTML, así que igual haría falta una segunda pieza para convertirlo en
markup. Reformulado, el problema es más chico de lo que parecía: la única
pieza que faltaba es que un rpc pueda elegir SU status en el camino de
ÉXITO -- "no encontrado" no es un error de transporte, es una respuesta
válida con otro status y un body distinto.

```
@route("/users/:id")
@content_type("text/html")
rpc userPage(id: Int) -> String {
  let found = db.users.find(id);
  if found == null {
    response.setStatus(404);
    "<h1>404: usuario no encontrado</h1>"
  } else {
    "<h1>encontrado</h1>"
  }
}
```

**Diseño: side-channel por request, mismo mecanismo que `request.rawBody()`/
`request.header()` (§3.38), no una nueva forma de valor.** `response` es un
módulo builtin más (mismo lugar que `db`/`http`/`env`); `setStatus(code)`
NO devuelve el status en el valor de retorno del rpc (eso hubiera exigido
inventar una representación de `Value` que "es un `String`" para el checker
pero carga algo más en runtime -- exactamente la clase de divergencia
checker-vs-runtime que este proyecto viene evitando desde §3.9). En cambio,
guarda el código en un `Cell` por request dentro de `Db` -- el mismo lugar
donde ya vive el contexto de la request entrante -- y `server.rs` lo
consume UNA vez, después de que `invoke_rpc` vuelve con éxito, para elegir
el status de la respuesta en vez de 200 fijo. Si el rpc no lo llama nunca,
el comportamiento no cambia: 200, como siempre.

**No está atado a `@content_type`/HTML.** Cualquier rpc puede pedir un
status de éxito distinto de 200 -- un `create` que quiere devolver 201, un
`delete` que quiere 204. El caso motivador es la página HTML, pero la
herramienta es más general a propósito: no hay ninguna razón real para
restringirla a un solo tipo de contenido.

**Validado en runtime, no en compilación.** El argumento puede ser
cualquier expresión (`response.setStatus(a_veces_calculado)`), no solo un
literal, así que no hay forma de chequear el valor exacto en el checker --
`setStatus` exige `Int` como tipo, y en runtime rechaza cualquier código
fuera de 100–599 con un error claro, en vez de escribir un status HTTP
inválido tal cual al socket.

**Camino de error, sin cambios: sigue siendo JSON siempre.** Si el rpc
termina en `Err`/panic DESPUÉS de haber llamado `response.setStatus`, ese
override nunca se usa -- `handle_rpc` solo lo consume en la rama `Ok`,
antes de armar la respuesta; el status de un error sigue saliendo de
`status_for(&RuntimeError)` como siempre (§3.35 sigue vigente ahí). El
`Cell` se limpia igual al final de la request (mismo `clear_request_context`
que ya limpiaba `request.rawBody()`) para que un override nunca sobreviva
a la request que lo pidió.

**Límite honesto:** dentro de un `stream`, llamarlo es un no-op silencioso
-- el status de una conexión SSE está fijado en 200 para toda la conexión
(§3.13), no por evento, así que no hay ningún status que cambiar ahí. No
hay un error de compilación para este caso porque detectarlo exigiría que
el checker sepa, en cada punto de una expresión arbitrariamente anidada,
si está dentro del cuerpo de un `stream` o de un `rpc` normal -- una pieza
de contexto que hoy no se enhebra por `check_expr` y que agregar solo para
esto sería una complicación real a cambio de rechazar un caso de uso que,
en la práctica, no tiene ningún motivo para aparecer (nadie escribe
`response.setStatus` dentro de un `stream` esperando que haga algo).

**Verificado** en `compiler/tests/cli_content_type.rs` contra un servidor
real: una 404 con el HTML propio del rpc (no el `{"error": ...}` de
siempre) para un `@route` sin resultado, un 201 sobre un rpc JSON plano
(confirmando que no está atado a HTML), y un código fuera de rango (`50`)
devolviendo el 500 con el mensaje de validación esperado -- no un status
HTTP roto llegando al cliente.

### 3.47 `http.getWithHeaders`/`http.postWithHeaders`: headers en llamadas salientes — RESUELTO

`http.get(url)`/`http.post(url, body)` (§3.5) ya existían, pero sin ninguna
forma de mandar un header -- así que aunque la llamada saliente en sí
funcionaba, autenticarse contra CUALQUIER API real de terceros era
imposible: Stripe, GitHub, o cualquier servicio que exija `Authorization`
(o cualquier otro header propio) rechazaba la request antes de mirarla
siquiera. `env.get`/`crypto.hmacSha256` (§3.38) ya resolvían el lado
ENTRANTE (verificar la firma de un webhook); este era el lado SALIENTE
simétrico que quedaba pendiente.

```
type Header = { name: String, value: String }

rpc createCharge(amountCents: Int) -> String {
  http.postWithHeaders(
    "https://api.stripe.com/v1/charges",
    "amount=" + amountCents.toString() + "&currency=usd",
    [
      Header { name: "Authorization", value: "Bearer " + env.get("STRIPE_SECRET_KEY") },
      Header { name: "Content-Type", value: "application/x-www-form-urlencoded" },
    ]
  )
}
```

**Dos métodos NUEVOS, no una sobrecarga de los existentes.** `http.get`/
`http.post` quedan exactamente como estaban -- ningún programa existente
cambia de comportamiento. `getWithHeaders`/`postWithHeaders` son builtins
aparte (mismo criterio que ya separa `Int`/`Int64`, o `check_program`/
`check_program_with_files`, GRAMMAR.md en general): un nombre explícito por
forma, en vez de una aridad variable sobre el mismo nombre, que hubiera
sido la primera vez que un builtin de este lenguaje se comporta distinto
según CUÁNTOS argumentos recibe.

**El tipo de cada header es estructural, sin nombre.** El checker espera
`{name: String, value: String}[]` -- un tipo ANÓNIMO (`Type::Struct{name:
None, ...}`), no un `Header` inventado por el lenguaje. Como `type` en
c-script ya es estructural (§3.2, `is_subtype` ignora el nombre), cualquier
struct que el programa declare con esos dos campos sirve tal cual --
`Header` en el ejemplo de arriba es una elección del programa, no una
palabra reservada. Alternativa descartada: `Map<K,V>` (§4) parecía la
opción obvia, pero NO tiene forma literal en c-script (`{K: V}` no se
parsea -- ambigüedad real con un struct de un campo, §2.2) y ningún
mecanismo para construir un valor desde cero -- solo existe como tipo de
ANOTACIÓN. Reusar `List<T>` + struct, ambos con literal real ya
existente, no necesitó ninguna sintaxis nueva.

**Runtime: cada `(name, value)` se aplica con `Request::set` de `ureq`
antes de mandar la request.** El checker ya garantiza la forma exacta
(subtipado estructural), así que el error que puede tirar
`http_headers_from_value` en runtime es defensivo -- el mismo criterio que
ya vale para el `unwrap_or_else` de `@content_type` en `server.rs` -- no
un caso que un programa bien tipado pueda alcanzar en la práctica.

**Límite honesto: la respuesta sigue siendo solo el body, como texto.**
Ni `http.get`/`http.post` ni las versiones con headers exponen el status
code ni los headers de la RESPUESTA -- un 4xx/5xx de la API llamada se ve
como un `Err`/`RuntimeError` genérico (el mensaje incluye el error de
`ureq`, pero no como un valor que el programa pueda inspeccionar por
campo). Suficiente para el caso que motivó esta ronda (crear un recurso y
seguir, o fallar) pero no para lógica que necesite ramificar según el
status exacto de la respuesta (ej. reintentar solo en 429, no en 402) --
eso queda para una ronda aparte si hace falta.

**Verificado** en `compiler/tests/cli_http.rs` contra un servidor HTTP real
armado a mano en el propio test (no un mock interno): confirma que
`Authorization`/headers custom llegan tal cual en un GET y un POST reales,
que el body de un POST sigue viajando junto con los headers, y que un host
inalcanzable falla con un error de runtime normal (500, sin panic) --
mismo criterio de robustez que ya prueba `cli_smtp.rs` para el llamado
saliente equivalente por SMTP. De paso, esta ronda le dio a `http.get`/
`http.post` su primera cobertura de tests real (no tenían ninguna hasta
ahora).

### 3.48 `db.<coleccion>.page(limit, offset)`: paginación real, empujada a SQL — RESUELTO

Antes de esta ronda, la única forma de acotar cuántas filas trae una
colección era `.all().take(n)` -- pero `.take` (§3.10, un método de
`List<T>` genérico, no de una colección de `db`) corre DESPUÉS de que
`.all()` ya trajo la tabla ENTERA a memoria. Para una tabla chica no se
nota; para una con miles de filas, pedir "la página 400" seguía costando
exactamente lo mismo que traer la tabla completa. `page` resuelve esto de
la única forma real posible: `LIMIT`/`OFFSET` viajan DENTRO del SQL.

```
rpc listUsers(limit: Int, offset: Int) -> User[] {
  db.users.page(limit, offset)
}
```

**Un método nuevo, no una segunda forma de `.take`.** `db.<c>.take` (el
método de `List<T>`) sigue significando "de lo que ya tengo en memoria,
los primeros N" -- eso no cambia, y sigue siendo la herramienta correcta
para acotar el resultado de `.findWhere(pred)` (que de por sí ya trae todo
a memoria para poder evaluar el predicado -- SQL pushdown de un predicado
arbitrario es un problema aparte, no resuelto acá). `page` es la
herramienta nueva y distinta: dos argumentos siempre, sin default (mismo
criterio de "nombre explícito por forma" que ya usa §3.47 para no
convertir esto en la primera aridad variable del lenguaje sobre un mismo
nombre).

**Mismo orden que `.all()`, siempre.** `ORDER BY "id"` en ambos casos --
paginar con un orden distinto en cada query dejaría que una fila
aparezca en dos páginas, o en ninguna, según cuándo se ejecute cada
consulta. Página determinística, no "lo que el motor devuelva esta vez".

**Portátil entre SQLite y Postgres sin ninguna rama por backend.**
`LIMIT $1 OFFSET $2` (Postgres) / `LIMIT ? OFFSET ?` (SQLite) es idéntico
en ambos dialectos -- mismo criterio que el resto de `db.rs`, que ya
resuelve el placeholder correcto por backend (`Backend::placeholder`) sin
que el código que arma la consulta necesite saber cuál es. `limit`/
`offset` viajan como parámetros bindeados, nunca interpolados en el
string SQL, aunque ya sean `Int` validados por el checker -- mismo
criterio defensivo de siempre.

**Validado en runtime, ambos backends.** `limit < 0` o `offset < 0` es un
error claro ANTES de tocar el SQL -- Postgres rechaza un `OFFSET`
negativo con su propio error (menos legible), y SQLite lo interpreta con
una semántica propia distinta (trata un `LIMIT` negativo como "sin
límite") que hubiera hecho que el mismo programa se comportara distinto
según el backend -- exactamente la clase de divergencia entre capas que
este proyecto viene evitando desde §3.9. Un `offset` más allá del final
de la tabla no es un error: lista vacía, igual que pedir una página que
no existe en cualquier API paginada real.

**Límite honesto: sin cursor, sin "próxima página" implícito.** El
caller arma el siguiente `offset` a mano (`offset + limit`) -- no hay un
token de continuación opaco (paginación por cursor, mejor para tablas que
cambian mientras se pagina) ni un total de páginas calculado por el
lenguaje. Para eso, `count()` (ya existente) sigue siendo la forma de
saber cuántas hay en total.

**Verificado** contra los dos backends: `compiler/src/runtime/db.rs`
(test unitario, SQLite en memoria) confirma páginas sin solapar, la
última página parcial, un offset más allá del final (lista vacía, no
error), y limit/offset negativos rechazados -- `compiler/tests/
pg_integration.rs` repite exactamente el mismo caso contra un PostgreSQL
real en CI, confirmando que el mismo programa se comporta igual en los
dos backends.

### 3.49 `@requires(Role.Admin | Role.Agent)`: OR de roles — RESUELTO

Límite de v0 (§3.14): `@requires` solo podía nombrar UN rol. Un endpoint
que dos roles distintos necesitan ver (un dashboard que comparten Admin y
Agent, por ejemplo) no tenía forma de expresarse sin duplicar el rpc
entero, uno por rol, o aflojar a `@authenticated` (cualquier rol, sin
restricción real).

```
@requires(Role.Admin | Role.Agent)
rpc sharedPanel() -> String { "panel compartido" }
```

**Reusa el `|` que ya existía para uniones de tipo, sin gramática
nueva.** `A | B` como TIPO (§2.2) y `Role.Admin | Role.Agent` como lista
de alternativas dentro de `@requires` son dos contextos distintos, pero
el mismo token con un significado análogo ("cualquiera de estos") --
más fácil de aprender que inventar un separador propio para esto.

**Todas las alternativas tienen que venir del MISMO enum -- rechazado en
el PARSER, no en el checker.** `@requires(Role.Admin | Status.Active)` no
tiene significado: una sesión tiene el rol de UN enum a la vez
(`auth.createSession(role)` toma un solo valor), así que "el rol es
`Role.Admin` O `Status.Active`" no es una pregunta que tenga sentido
hacerle a una sesión real. Se rechaza en el parser porque es puramente
sintáctico -- comparar el identificador antes de cada `.` contra el
primero no necesita tabla de símbolos, y el error sale en el token exacto
que no matchea, antes de que el checker llegue a mirar nada semántico.

**Cada alternativa se sigue validando contra el enum declarado**, igual
que la v0 de un solo rol (§3.14) -- `@requires(Role.Admin | Role.Typo)`
es un error de COMPILACIÓN, nunca un 403 imposible de satisfacer
descubierto en producción.

**Runtime: sin cambios de forma, un `.any()` más.** `check_auth_gate`
(`runtime/server.rs`) seguía comparando el rol de la sesión contra UNA
tupla `(enum, variante)`; ahora compara contra una lista de variantes del
mismo enum -- el mensaje de error (403 genérico, sin nombrar qué rol
hacía falta) no cambió: seguir sin filtrar qué roles protegen un
endpoint importa igual de acá que de v0 (hallazgo del review adversarial
original, GRAMMAR.md §3.14).

**Verificado** contra un servidor real (`compiler/tests/server_http.rs`):
dos logins reales con roles DISTINTOS, ambos aceptados por el mismo
`@requires` compartido; un tercer rol rechazado (403); sin token,
rechazado antes de siquiera mirar el rol (401); y un `@requires` de un
solo rol en el MISMO programa sin ningún cambio de comportamiento. Más
tests de compilación (`checker.rs`) para el OR válido, la variante
desconocida, y el rechazo en el parser de mezclar dos enums.

### 3.50 `--session-ttl`: expiración real de sesión — RESUELTO

Último límite honesto real de auth v0 (§3.14) que quedaba: una sesión
vivía hasta `destroySession()` o hasta reiniciar el proceso -- sin forma
de expresar "sesión válida 7 días", el patrón más común de cualquier
sistema de auth real (cookies de sesión, tokens de acceso con expiración
fija).

```
linkc serve app.link 8787 --session-ttl 7d
# o, para un contenedor (mismo criterio que --db/--cors-origin):
LINK_SESSION_TTL=7d linkc serve app.link 8787
```

**Configuración de servidor, no del lenguaje -- ninguna sintaxis nueva en
`.link`.** `auth.createSession(role)` no ganó un parámetro de TTL: el
tiempo de vida es una decisión operativa (¿cuánto dura una sesión en
ESTE deploy?), no algo que cada `rpc` deba decidir caso por caso, mismo
criterio que ya vale para `--cors-origin`/`--db` (§3.36, §3.41). Formato
`"Ns"`/`"Nm"`/`"Nh"`/`"Nd"` -- mismo espíritu que `@rate_limit("20/1m")`
(§3.39) pero CON días: la escala típica de una sesión (horas a semanas)
los necesita de verdad, a diferencia de una ventana de rate limit, donde
"N por día" es un caso raro.

**Sin flag/variable, cero cambios de comportamiento.** El default sigue
siendo "sin expirar sola" -- exactamente v0, para no romper a nadie que
no pida esto explícitamente (mismo criterio que TODA config opcional de
`linkc serve` hasta ahora).

**Limpieza perezosa, no un barrido de fondo.** Una sesión vencida se
borra recién en el PRÓXIMO acceso a ese token (`SessionStore::role_for`),
no por un timer que la busque proactivamente -- este intérprete no tiene
ningún hilo de mantenimiento (single-threaded por diseño, §3.13), así que
inventar uno solo para esto hubiera sido la primera excepción a ese
modelo. Costo real: una sesión creada y nunca vuelta a usar después de
expirar queda en memoria hasta que alguien intente usarla (o el proceso
reinicie) -- aceptable para v0, documentado en vez de escondido.

**Token vencido y token que nunca existió son INDISTINGUIBLES desde
afuera, a propósito.** `role_for` devuelve `None` para los dos casos por
igual, y `check_auth_gate` (`server.rs`) da el mismo 401 "se requiere
autenticación" -- mismo principio que ya regía para "no revelar qué rol
hacía falta" en un 403 (hallazgo del review adversarial original de auth
v0): un atacante que prueba tokens no debería poder distinguir "este
token existió alguna vez y venció" de "este token es pura invención".

**`Instant`, no `SystemTime`, para el reloj interno.** El TTL se mide con
`std::time::Instant` (monotónico, inmune a que el reloj del sistema
salte hacia atrás/adelante por NTP o cambio de horario) -- a diferencia
de `now() -> Timestamp` (§3.32), que SÍ necesita `SystemTime` porque
tiene que representar una fecha de calendario real que el programa
compara/muestra. Acá no hace falta ninguna fecha visible, solo "cuánto
pasó desde que se creó" -- `Instant` es la herramienta más correcta para
eso, no una casualidad de implementación.

**Verificado** contra un servidor real (`compiler/tests/server_http.rs`):
`--session-ttl 2s`, un login real, acceso inmediato aceptado, y el MISMO
token rechazado (401) tres segundos después, sin haber llamado
`destroySession` en ningún momento. Tests unitarios (`session.rs`)
confirman además que un store sin TTL configurado (`new()`, el default)
nunca expira nada, y que un token vencido y uno inexistente dan
exactamente la misma respuesta.

### 3.51 `auth.currentRole()`: leer el rol del caller dentro de un cuerpo — RESUELTO

Último límite real de esta misma serie: `@requires`/`@authenticated`
(§3.14) eran una puerta de sí/no, pero el CUERPO de un rpc no tenía
forma de saber qué rol autenticó la request. Con `@requires(Role.Admin |
Role.Agent)` (§3.49) ya real, esto importaba de verdad: un endpoint
compartido entre dos roles a menudo necesita comportarse DISTINTO según
cuál de los dos es, no solo decidir "entra o no entra".

```
@requires(Role.Admin | Role.Agent)
rpc sharedPanel() -> String {
  if auth.currentRole() == "Admin" {
    "panel de administrador"
  } else {
    "panel de agente"
  }
}
```

**Devuelve `String?`, no el enum real.** La alternativa -- que
`currentRole()` devuelva un valor del enum REAL declarado en el
`@requires` del propio rpc (`Role?` en vez de `String?`) -- se descartó:
exigiría que el checker sepa, en cualquier punto arbitrariamente anidado
de una expresión, con qué enum se autenticó ESTE rpc en particular, una
pieza de contexto que hoy no viaja por `check_expr` (mismo motivo, ya
razonado en §3.46, por el que `response.setStatus` tampoco intentó saber
si estaba dentro de un `stream`). `String?` es más chico, no necesita
ningún contexto nuevo, y sigue siendo suficiente para lo que el caso real
pedía: ramificar lógica por rol, no reconstruir un valor tipado del enum
completo.

**Disponible SIEMPRE, no solo bajo `@requires`/`@authenticated`.** Mismo
criterio que `request.rawBody()`/`request.header()` (§3.38): un rpc SIN
ninguna anotación de auth puede llamar `auth.currentRole()` igual --
`null` si no hay sesión válida, el nombre de la variante si la hay. Útil
para un endpoint público que se comporta distinto si el caller resulta
estar logueado, sin por eso EXIGIR que lo esté.

**`null` para "sin sesión" y "token inválido" son indistinguibles, a
propósito.** Mismo principio que ya regía para el 401 genérico (§3.14) y
para expiración (§3.50, `role_for`): esto reusa exactamente
`SessionStore::role_for`, así que hereda esa propiedad gratis, sin
código nuevo -- un token vencido bajo `--session-ttl` también da `null`
acá, consistente con que ya no cuenta como sesión válida en ningún otro
lado.

**Cero cambios al modelo de sesión.** No hay ningún parámetro nuevo en
`auth.createSession(role)` ni un nuevo campo en `SessionStore` -- el rol
ya viajaba en la sesión desde v0 (§3.14); lo único que faltaba era
EXPONERLO al cuerpo del rpc. `current_token: Option<&str>` ya llegaba a
`call_method` desde antes (lo usa `destroySession`), así que esto es
aditivo puro sobre plumbing que ya existía.

**Límite honesto, sigue sin resolverse:** solo el ROL, nunca la
identidad completa del caller. `auth.currentRole()` no reemplaza un
`ctx.user`/similar -- la sesión sigue sin guardar ninguna referencia al
`User` real que inició sesión (§3.14 nunca lo guardó, ver el hallazgo de
diseño original). Si un rpc necesita saber QUIÉN es el caller, no solo
QUÉ rol tiene, sigue sin haber forma de resolverlo desde adentro del
lenguaje.

**Verificado** contra un servidor real (`compiler/tests/server_http.rs`):
un `sharedPanel` con `@requires(Role.Admin | Role.Agent)` respondiendo
CONTENIDO DISTINTO según cuál de los dos roles autenticó -- no solo
permitido/denegado; `auth.currentRole()` funcionando en un rpc SIN
ninguna anotación de auth; `null` tanto sin token como con un token que
nunca existió. Más un test de compilación (`checker.rs`) para el tipo
`String?` y el rechazo de argumentos.

### 3.52 `sumBy`/`countBy`/`avgBy`/`maxBy`/`minBy`: agregación con `GROUP BY` real, empujada a SQL — RESUELTO

Último gap real de esta serie: "KPIs de `/admin/revenue` (MRR por plan) o
`/admin/analytics` (top por vistas) hay que calcularlos trayendo todas
las filas a memoria y agregando a mano en el propio lenguaje -- funciona
en tablas chicas, se degrada mal si crecen". `findWhere`/`deleteWhere`
(§2.1) ya traían todo a memoria para evaluar un predicado arbitrario;
esto es lo mismo pero para `GROUP BY` -- y acá SÍ hay una forma real de
empujarlo a SQL, porque el shape que hace falta reconocer es mucho más
chico que "un predicado cualquiera".

```
type RevenueByPlan = { key: String, value: Int }

rpc revenueByPlan() -> RevenueByPlan[] {
  db.orders.sumBy(|o: Order| { o.planId }, |o: Order| { o.amountCents })
}
```

**Cinco métodos, no un query builder.** `sumBy`/`avgBy`/`maxBy`/`minBy`
toman DOS selectores (agrupar por, agregar); `countBy` toma uno solo
(`COUNT(*)` por grupo, no cuenta un campo). Se descartó una API tipo
`db.orders.groupBy(...).sum(...)` encadenada -- eso necesitaría un tipo
intermedio nuevo ("query en construcción") en el sistema de tipos, mucho
más superficie que cinco métodos con nombre explícito por combinación
(mismo criterio de "nombre por forma" que ya usa §3.47 para no inventar
la primera aridad variable del lenguaje sobre un mismo nombre).

**El closure NUNCA se ejecuta -- solo nombra una columna.** A diferencia
de `findWhere` (el predicado SÍ corre, una vez por fila, en el
intérprete), acá el shape reconocido (`ast::recognize_field_selector`,
mismo patrón que `recognize_live_subscribe` de §3.16: una función
standalone que checker.rs Y runtime llaman cada uno por su lado) es
EXACTAMENTE `|item: T| { item.campo }` -- un acceso de campo simple,
nada más. Cualquier otra forma (`item.campo + 1`, un método, un campo
anidado) se rechaza en compilación con un mensaje claro: no hay forma
real de traducir una expresión c-script arbitraria a SQL, así que ni se
intenta -- se reconoce un shape chico y ancho (cualquier campo real
sirve) en vez de un intérprete de expresiones parcial.

**Restricciones de tipo, deliberadas y angostas:**
- **Agrupar** solo por `String`, `Int`, `Bool` o un `enum` -- ni `Float`
  (igualdad exacta de floats es una trampa conocida, mismo motivo que
  §3.3 no tiene patrón `Float` en `match`), ni `Timestamp`/`Int64`
  todavía. Agrupar por fecha con truncado (`GROUP BY` mes/día) queda
  fuera de esta ronda -- necesitaría una segunda pieza (algo como
  `.truncateToMonth()`) que reconocer, no solo el campo desnudo.
- **Agregar** (`sumBy`/`avgBy`/`maxBy`/`minBy`) solo `Int`/`Float` --
  `Int64` tampoco todavía.
- **Ninguno de los dos acepta un campo opcional** -- ni por clave
  (`campo?: T`) ni nullable (`campo: T?`). Un grupo o un valor "ausente"
  no tiene una fila SQL real que lo represente sin inventar más
  semántica (¿el grupo `null` cuenta aparte? ¿se descarta?) -- se deja
  afuera en vez de adivinar.

**Agrupar por un campo `enum` devuelve el enum REAL como `key`, no un
`String`.** La columna de storage detrás sigue siendo `TEXT` (mismo
mapeo que cualquier enum simple, GRAMMAR.md tabla §4), pero el checker
ya le promete al programa el tipo declarado del campo tal cual
(`field_selector` no lo degrada) -- así que el runtime tiene que
CUMPLIRLO: `scalar_cell_to_value` (`runtime/db.rs`) reconstruye
`Value::Variant`, no `Value::Str`, para ese caso -- mismo camino que ya
usa `row_to_fields` para una columna enum normal. Se encontró y arregló
DURANTE esta ronda (verificado contra un servidor real antes de escribir
el test permanente): la primera versión degradaba a `String` en runtime
mientras el checker prometía el enum, exactamente la clase de
desacuerdo entre capas que GRAMMAR.md §3.9 existe para evitar.

**`AVG` siempre da `Float`, aunque la columna de origen sea `Int`.**
`SUM`/`MAX`/`MIN` preservan el tipo de la columna (sumar `Int` sigue
dando `Int`) -- pero un promedio es fraccionario por naturaleza en SQL,
sea cual sea el tipo de entrada, así que el tipo de retorno de `avgBy`
es `Float` siempre, sin excepción.

**Portátil entre SQLite y Postgres sin ninguna rama por backend.**
`SELECT "campo" AS "key", SUM("otro") AS "value" FROM "c" GROUP BY
"campo"` es SQL estándar en los dos motores -- mismo criterio que
`page` (§3.48): nombres de columna vienen del propio programa compilado
(nunca de input del caller), así que interpolarlos directo en el string
SQL es seguro, mismo patrón que ya usa el resto de `db.rs`.

**Verificado** contra los dos backends: `runtime/mod.rs` (test de
integración vía `test "..."` real, SQLite en memoria) corre los cinco
métodos contra datos reales y confirma además, con un assert de VALOR
exacto (no solo de longitud), que agrupar por un campo enum devuelve la
variante real comparable con `==` -- `compiler/tests/pg_integration.rs`
repite `sumBy`/`countBy` contra un PostgreSQL real en CI. Más 8 tests de
compilación (`checker.rs`) para cada camino de rechazo: selector
derivado, tipo de agrupación inválido, tipo de valor inválido, campo
opcional (las dos formas), aridad de argumentos, y que agrupar por un
enum tipa con el enum real como key.

### 3.53 `auth.createSessionWithId()` y `auth.currentUserId()`: asociar e inspeccionar el id del caller — RESUELTO

Con `auth.currentRole()` (§3.51) era posible saber el rol con el que se autenticó la petición, pero no la identidad numérica del usuario (`userId: Int`). Un sistema real donde cada usuario es dueño de sus propios recursos (e.g. `db.notes.findWhere(|n: Note| { n.authorId == uid })`) obligaba a pasar el `userId` como parámetro explícito en cada llamada RPC, perdiendo la seguridad que otorga la sesión en el servidor.

<!-- linkc:fragment -->
```rust
service Auth {
  rpc login(email: String) -> String {
    let user = db.users.findWhere(|u: User| { u.email == email })[0];
    auth.createSessionWithId(user.role, user.id)
  }
}

service Notes {
  @authenticated
  rpc myNotes() -> Note[] {
    let uid = auth.currentUserId();
    db.notes.findWhere(|n: Note| { n.authorId == uid })
  }
}
```

**Dos métodos dedicados y explícitos:**
- `auth.createSessionWithId(role: R, userId: Int) -> String`: toma el rol (un enum declarado) y el identificador de usuario (`Int`). Emite un token de sesión seguro de 128 bits asociando ambos datos en memoria. `auth.createSession(role)` de siempre sigue existiendo sin cambios (fija `userId` en `None`).
- `auth.currentUserId() -> Int?`: devuelve el `userId` asociado a la sesión de la request actual (`Int?`).

**`null` para "sin sesión", "sin id asociado" y "token inválido/expirado":**
Mismo principio de indistinguibilidad de siempre (§3.14, §3.50, §3.51) — un endpoint público o autenticado obtiene `null` si no hay sesión activa, si el token expiró bajo `--session-ttl`, o si la sesión se creó mediante `createSession(role)` sin id.

**Verificado** contra un servidor real (`compiler/tests/server_http.rs`): login con `createSessionWithId`, recuperación exitosa de `currentUserId()` (`42`) y `currentRole()` (`"Member"`), bifurcación de lógica con `@authenticated`, y retorno de `null` en peticiones sin sesión o con sesiones creadas sin id. Más 3 tests de compilación en `checker.rs` (tipado de argumentos y retorno `Int?`) y tests unitarios en `session.rs`.

---

### 3.54 `crypto.randomInt()` y `crypto.timingSafeEqual()`: aleatoriedad numérica y comparación segura para código de usuario — RESUELTO

Reporte real del 23/08/2026 (adopción de una app financiera existente, `MyFinance`): el módulo `crypto` ya generaba secretos con el CSPRNG del sistema (`randomToken`, `uuid` — §3.34) y comparaba en tiempo constante internamente (`verifyPassword` contra hashes legados), pero ninguna de las dos cosas estaba expuesta en la forma que un `rpc` de usuario necesita:

1. **Sin generador numérico.** `crypto.randomToken(n)` devuelve hex (`0-9a-f`) — sirve para un token de sesión, no para un OTP de 6 dígitos, donde el alfabeto tiene que ser `0-9` y el rango exacto (`100000..999999`). Construir eso a mano desde `randomToken` significa parsear hex a entero e introducir sesgo a mano, exactamente el tipo de criptografía-artesanal que el resto del módulo evita.
2. **`constant_time_eq` (`subtle::ConstantTimeEq`) era una función privada de `runtime/mod.rs`**, usada solo dentro de `verifyPassword` para comparar contra el hash legado. Comparar un secreto de webhook (`crypto.hmacSha256(secret, body) == signature`, patrón de §3.38) o una API key con `==` de `String` corta en el primer byte distinto — el mismo canal lateral que ya se había cerrado para contraseñas en §3.34, reabierto para cualquier otro secreto que el código de usuario compare.

<!-- linkc:fragment -->
```rust
service Auth {
  rpc requestOtp(userId: Int) -> Int {
    let code = crypto.randomInt(100000, 999999);
    // ... guardar `code` asociado a `userId` con expiracion ...
    code
  }
}

service Webhooks {
  rpc receive(payload: String, signature: String) -> Bool {
    let expected = crypto.hmacSha256(env.get("WEBHOOK_SECRET"), payload);
    crypto.timingSafeEqual(expected, signature)
  }
}
```

**Lo que hay ahora:**

| Función | Firma | Implementación |
|---|---|---|
| `crypto.randomInt(min, max)` | `(Int, Int) -> Int` | Entero uniforme en `[min, max]` (ambos incluidos) del CSPRNG del sistema, con rechazo de muestreo (`rejection sampling`) contra el sesgo de módulo: un `u64` que caería en el resto no divisible se descarta y se pide otro, en vez de aplicar `%` directo, que haría a los primeros valores del rango levemente más probables que los últimos. |
| `crypto.timingSafeEqual(a, b)` | `(String, String) -> Bool` | Expone `constant_time_eq` (ya usado internamente desde §3.34) sobre los bytes UTF-8 de ambos strings — largos distintos devuelven `false` sin comparar contenido. |

**Límites honestos de esta ronda:**
- `randomInt` no genera `Float` ni `Int64` — solo `Int` (`i64`), que alcanza para OTPs, sorteos y muestreo; un rango que exceda `2^64` valores (prácticamente inalcanzable con `Int`) cae a un solo `u64` sin rechazo en vez de fallar, porque el sesgo es inmedible frente a un rango tan grande.
- `timingSafeEqual` compara bytes, no números ni structs — comparar dos `Int` en tiempo constante no tiene el mismo problema (una CPU ya compara enteros de ancho fijo en tiempo constante) así que no hace falta una sobrecarga para ese caso.
- No hay conversión `Int -> String` en el lenguaje todavía (nada de esta ronda la agrega), así que un OTP que necesite viajar como texto con ceros a la izquierda (`"042857"`) queda fuera de alcance por esa razón, no por `randomInt` en sí — el `rpc` de ejemplo arriba devuelve el código como `Int`.

**Tests que fijan estas propiedades** (`runtime/mod.rs`, módulo de tests): `randomInt` cae siempre dentro de `[min, max]`, un rango de un solo valor siempre lo devuelve, tres llamadas seguidas con un rango de OTP no dan siempre el mismo valor; `timingSafeEqual` compara igual que `==` en el caso feliz y devuelve `false` (sin crashear) ante strings de largo distinto.

---

### 3.55 `.toString()` sobre `Int`/`Int64`/`Float`/`Bool` — RESUELTO

Auditoría del roadmap del 23/08/2026 (PLAN.md §8.6): hasta esta ronda no existía NINGUNA forma de convertir un número o un `Bool` a `String` en todo el lenguaje. No es un detalle de un caso de uso puntual -- bloquea algo tan básico como interpolar un contador en un mensaje de error, porque `'+'` exige `String + String` sin coerción implícita (§3.7): `"código: " + n` nunca compiló, para ningún `n` que no fuera ya un `String`.

<!-- linkc:fragment -->
```rust
rpc describe(count: Int, active: Bool) -> String {
  "hay " + count.toString() + " activo: " + active.toString()
}
```

**Cuatro métodos nuevos, mismo criterio que `toInt64()`/`toIsoString()` (§3.30/§3.34): conversión EXPLÍCITA, nunca automática.**

| Método | Resultado |
|---|---|
| `Int.toString()` | Igual que el `Display` estándar de Rust (`i64::to_string`) -- sin separador de miles, `-` para negativos. |
| `Int64.toString()` | Idéntico, sobre el `i64` interno de `Int64`. |
| `Float.toString()` | `f64::to_string` de Rust -- notación decimal para el rango normal, sin redondeo ni formato de moneda/precisión configurable. |
| `Bool.toString()` | `"true"` / `"false"` literales. Primer método que existe sobre `Bool` en todo el lenguaje -- hasta esta ronda `Bool` no tenía NINGÚN método, ni siquiera este. |

**Límite honesto:** ningún método de formato (separador de miles, notación científica, precisión decimal fija, padding de ceros a la izquierda) -- es el `Display` de Rust tal cual, sin capa de formato encima. Si hace falta, se construye a mano en el propio `.link` con las funciones de `String` que ya existen.

**Tests que fijan esto** (`runtime/mod.rs`): las cuatro conversiones, incluyendo un negativo (`Int`) y que el resultado compone de verdad con `'+'` de `String` -- la propiedad que estaba bloqueada antes de esta ronda.

---

### 3.56 `response.setStatus` dentro de un `stream` — RESUELTO (ahora error de compilación)

Mismo audit del 23/08/2026 (PLAN.md §8.6): `response.setStatus(code)` (§3.46) documentaba desde su propia introducción que es un no-op dentro de un `stream` -- el status de una conexión SSE es fijo para toda su duración, se decide una sola vez al abrir la respuesta. Lo que NO hacía hasta ahora es rechazarlo: tipaba sin ninguna queja, y el no-op solo se notaba en producción, cuando alguien esperaba que un `stream` respondiera 201 y seguía viendo 200.

<!-- linkc:fragment -->
```rust
service Items {
  stream watchAll() -> Item {
    response.setStatus(201); // ahora: error de compilación
    db.items.subscribe()
  }
}
```

**Mecanismo:** `Checker` gana un `Cell<bool>` (`in_stream_body`, interior mutability -- mismo motivo que `hover_result`, §3.24: el resto de `Checker` chequea con `&self` de punta a punta) que `check_rpc` prende mientras chequea el cuerpo de un `stream` -- nunca el de un `rpc` normal. El match arm de `(Type::Response, "setStatus")` lo consulta antes de tipar el argumento: si está prendido, el error sale ahí mismo, con el span de la llamada real (mismo mecanismo de `.with_span` que ya estampa cualquier otro error del checker).

**Verificado:** dos tests en `checker.rs` -- el mismo cuerpo con `setStatus(201)` rechazado dentro de un `stream` y aceptado sin cambios dentro de un `rpc` normal (para probar que el chequeo es específico de `stream`, no una regresión sobre el caso de siempre).

---

### 3.57 `@route` con segmento catch-all (`:nombre*`) — RESUELTO

Tercer gap del mismo audit (PLAN.md §8.7): `@route` (§3.37, generalizado a múltiples parámetros en §3.42) solo podía capturar UN segmento de path por parámetro. Cualquier ruta de profundidad variable -- documentación, un CMS, un proxy de archivos estáticos -- necesitaba un `rpc` por cada nivel posible, o quedaba fuera de `@route` por completo.

<!-- linkc:fragment -->
```rust
service Docs {
  @route("/docs/:rest*")
  rpc page(rest: String) -> String {
    // "/docs/api/v2/users" -> rest == "api/v2/users"
    // "/docs"              -> rest == ""
    renderDoc(rest)
  }
}
```

**Sintaxis:** `:nombre*` -- el nombre sigue las mismas reglas que un parámetro normal (identificador válido, sin repetirse dentro de la misma ruta), el `*` lo marca como catch-all. Solo puede ser el ÚLTIMO segmento de la ruta -- cualquier cosa después sería inalcanzable siempre, así que se rechaza en el parser (`route.rs::parse_route_pattern`), no en runtime.

**Captura cero o más segmentos**, unidos con `"/"` en una sola `String` -- nunca `Int` (el texto puede contener `"/"` y estar vacío, ninguna de las dos cosas es un entero válido; el checker lo rechaza explícitamente si el parámetro del rpc no es `String`). `/docs` matchea con `rest == ""`, exactamente igual que `/docs/x/y/z` matchea con `rest == "x/y/z"`.

**Precedencia con otras rutas -- se extiende, no se reemplaza, el criterio de especificidad de §3.42:** un catch-all cuenta como CERO segmentos literales fijos (igual que un `:param` normal), así que cualquier ruta con más literales gana determinísticamente. `/docs/changelog` (2 literales) le gana a `/docs/:rest*` (1 literal) para ese path exacto, aunque las dos podrían matchearlo.

**Detección de conflictos, extendida:** dos patrones ya no necesitan la MISMA longitud total para competir -- un catch-all se puede estirar para cubrir cualquier cantidad de segmentos, así que `overlap_possible` ahora compara solo el prefijo fijo compartido (hasta el más corto de los dos) cuando cualquiera de los dos tiene catch-all. Deliberadamente conservador: prefiere marcar un conflicto que en la práctica nunca chocaría, antes que dejar pasar una ambigüedad real -- mismo criterio que el resto de `route.rs` desde que existe.

**Cambio de tipo interno:** `RoutePattern::matches` pasó de devolver `Vec<&str>` (segmentos prestados) a `Vec<String>` -- un catch-all captura VARIOS segmentos originales unidos por algo que no estaba en el string de entrada, así que un `&str` prestado ya no alcanza para representar el resultado. Costo real: una asignación por parámetro capturado en cada request con `@route`, en un intérprete de un solo hilo -- no es el camino caliente que este proyecto optimiza.

**Verificado:** 9 tests unitarios en `route.rs` (parseo, rechazo fuera de posición, matching de 0/1/muchos segmentos, conflicto entre dos catch-all, no-conflicto con prefijo literal distinto) más 2 tests end-to-end en `cli_route.rs` contra un servidor real (captura multi-segmento y precedencia del literal sobre el catch-all que también matchea).

---

### 3.58 `crypto`: costo de Argon2id configurable y señal de hash legado — RESUELTO

Dos gaps de PLAN.md §8.4, cerrados en la misma ronda -- ambos documentados como límite honesto desde la auditoría original de `crypto` (§3.34):

**1. Costo de Argon2id configurable.** Antes de esta ronda, `crypto.hashPassword` siempre corría con el default de la crate (`m=19456` KiB, `t=2`) sin ninguna forma de subirlo. Como el costo es una decisión de POSTURA DE SEGURIDAD del despliegue -- no algo que varíe llamada por llamada dentro de un mismo programa -- se resolvió como flag de servidor, mismo criterio que `--session-ttl`/`--cors-origin`, no como parámetro nuevo de `hashPassword`:

```
linkc serve app.link 8787 --argon2-memory-kib 65536 --argon2-iterations 3
```

(o `LINK_ARGON2_MEMORY_KIB`/`LINK_ARGON2_ITERATIONS`). Sin ninguno de los dos, el comportamiento es idéntico al de siempre.

**Mecanismo:** `Db` gana un `RefCell<argon2::Params>` -- mismo criterio que `current_request`/`response_status_override` (§3.38/§3.46): vive en `Db` en vez de enhebrarse como parámetro nuevo por las ~11 firmas que ya cargan `db`/`sessions`/`current_token` a través de todo el árbol de evaluación (`eval_expr`, `call_method`, ...), porque `db: &Db` ya está disponible en cualquier punto de ese árbol. `server.rs` lo fija UNA sola vez al arrancar, antes de aceptar la primera request; `crypto.hashPassword` lo lee en cada llamada. `verifyPassword` NO lo necesita -- el formato PHC (`$argon2id$v=19$m=...,t=...,p=...$`) embebe sus propios parámetros en el hash guardado, así que verificar sigue funcionando sin importar con qué costo se hasheó.

**2. `crypto.isLegacyHash(hash: String) -> Bool`.** `verifyPassword` sigue aceptando el formato legado (`sha256$<sal>$<hex>`) por compatibilidad, pero no había forma de preguntarle a un hash guardado si es de ese formato sin mirar el prefijo a mano. Ahora, el patrón de re-hasheo proactivo es directo:

<!-- linkc:fragment -->
```rust
rpc login(email: String, password: String) -> String {
  let user = db.users.findWhere(|u: User| { u.email == email })[0];
  if (!crypto.verifyPassword(password, user.passwordHash)) {
    panic("credenciales inválidas");
  }
  if (crypto.isLegacyHash(user.passwordHash)) {
    // ... db.users.update(user.id, Patch { passwordHash: crypto.hashPassword(password) }) ...
  }
  auth.createSessionWithId(user.role, user.id)
}
```

**Verificado:** el costo configurable, contra un servidor real (`cli_argon2.rs`) -- sin flags el hash embebe el default de la crate (`m=19456,t=2`), con `--argon2-memory-kib 8192 --argon2-iterations 3` el hash embebe exactamente esos valores, y un valor no numérico falla ANTES de arrancar (nunca llega a escuchar el puerto). `isLegacyHash` se agregó al mismo test de propiedades de `crypto` que ya fija el resto de esta familia (`runtime/mod.rs`): distingue un hash legado real de un Argon2id real.

---

### 3.59 PostgreSQL: acepta PK autoincremental de 32/16 bits, no solo `BIGSERIAL` — RESUELTO

Bug real encontrado auditando PLAN.md §8.5 (reporte de adopción de una app financiera sobre una base Postgres ya existente): `validate_existing_id_column` (`runtime/db.rs`, agregada para el caso de `id UUID`) ya aceptaba `bigint`, `integer` Y `smallint` como tipos válidos de "id" para una tabla preexistente -- pero `insert_returning_id`/`postgres_cell` (`runtime/store.rs`) leían esa columna con `try_get::<_, i64>`, que exige que el OID de la columna sea EXACTAMENTE `int8`. Una tabla real con `id SERIAL` (`int4`, típico al migrar desde un backend que no usaba `BIGSERIAL`) pasaba la conexión sin ninguna queja -- y fallaba en el primer `insert`, con un error de tipo que ninguna de las dos capas documentaba de este lado. El comentario que quedó al lado de ese `try_get` incluso afirmaba "esto nunca dispara" apoyándose en una validación que, leída con cuidado, ya aceptaba justamente el caso que lo disparaba -- el mismo patrón de "dos capas que discrepan" que este documento viene registrando desde §3.9.

**La corrección** generaliza `postgres_cell` (no solo el camino de `insert_returning_id`, que fue donde se encontró el bug) con un helper que prueba `int8` → `int4` → `int2` en orden, aceptando cualquiera de los tres anchos que `validate_existing_id_column` ya reconocía como válidos -- y que además importa para CUALQUIER columna `Int` de una tabla adoptada, no solo `"id"`: un campo `Int` normal guardado como `INTEGER` en vez de `BIGINT` tenía exactamente el mismo problema.

**Límite que sigue en pie:** las tablas que `linkc` GENERA siguen usando `BIGSERIAL` siempre (`postgres_emit.rs`) -- esto es solo sobre LEER una tabla que ya existía con otro ancho, nunca sobre crear una nueva con un ancho distinto.

**Verificado:** un nuevo test en `pg_integration.rs` (`a_preexisting_table_with_a_32_bit_serial_id_accepts_inserts_and_reads`) crea una tabla a mano con `id SERIAL PRIMARY KEY` y confirma `insert`/`get`/`list` de punta a punta contra un Postgres real. **Sin verificar en esta sesión**: no había Postgres disponible en el entorno de desarrollo (ni Docker para levantar uno) -- el test corre de verdad recién en CI, que sí tiene la base levantada (`.github/workflows/ci.yml`, job `postgres`). El razonamiento sobre `try_get`/OIDs está confirmado por lectura cuidadosa del código y la documentación de `postgres`/`tokio-postgres`, no por ejecución real todavía.

---

### 3.60 `http.getWithStatus`/`http.postWithStatus`: código de estado y headers de la respuesta — RESUELTO

Item de la tabla "Does not work yet" del README desde v1.11.0 (`http.getWithHeaders`/`postWithHeaders`, §3.47), reflejado también en PLAN.md §8.3.1: `http.get`/`http.post` (con o sin headers salientes) solo devolvían el body como `String` -- un 4xx/5xx de la API llamada se volvía un error de runtime genérico, sin forma de que el programa lo inspeccionara (por ejemplo, para reintentar solo ante un 429).

<!-- linkc:fragment -->
```rust
type Header = { name: String, value: String }
type ApiResponse = { status: Int, headers: Header[], body: String }

rpc charge(amount: Int, apiKey: String) -> Bool {
  let resp = http.postWithStatus("https://api.example.com/charges", "amount=" + amount.toString(), [
    Header { name: "Authorization", value: "Bearer " + apiKey },
  ]);
  match (resp.status) {
    200 => true,
    429 => panic("rate limited, reintentar más tarde"),
    _ => false,
  }
}
```

**Dos métodos nuevos**, no una sobrecarga de los cuatro existentes -- `http.get`/`http.post`/`http.getWithHeaders`/`http.postWithHeaders` quedan sin cambios, siguen devolviendo `String` y siguen convirtiendo un 4xx/5xx en error de runtime (quien no necesita inspeccionar el status sigue con la forma simple). `getWithStatus`/`postWithStatus` toman los mismos argumentos que sus pares `WithHeaders` (`headers: []` si no hace falta mandar ninguno) y devuelven un struct estructural, SIN nombre reservado por el lenguaje -- mismo criterio exacto que ya usa el tipo de `headers` (§3.47): cualquier `type` que el programa declare con los campos `status: Int`, `headers: {name: String, value: String}[]`, `body: String` sirve como destino.

**Un 4xx/5xx deja de ser un error de runtime en estos dos métodos** -- es justo el dato que existen para exponer. `ureq::Error::Status` (la librería HTTP que usa el intérprete) trae la `Response` completa, no solo el código, así que se decodifica igual que el camino 2xx. Solo un error de RED de verdad (DNS, conexión rechazada, timeout) sigue siendo un error de runtime -- eso nunca fue algo que un programa pudiera "manejar" de forma significativa de todos modos.

**Verificado** contra un servidor HTTP real armado a mano en el test (`cli_http.rs`, no un mock interno): un 2xx expone status/headers/body correctos; un 429 con header `Retry-After` llega completo como dato, sin que el rpc que lo llama falle; un 201 de un POST también. Nombres de header comparados sin distinguir mayúsculas -- HTTP no las distingue, y `ureq` normaliza a minúsculas al parsear.

---

### 3.61 `db.<c>.pageAfter(cursor, limit)`: cursor de continuación — RESUELTO

Item de la tabla "Does not work yet" del README desde que `page` existe (§3.48): `db.<c>.page(limit, offset)` obliga al caller a calcular el próximo `offset` a mano (`offset + limit`), y un `OFFSET` cuenta filas desde el principio de la tabla EN CADA LLAMADA -- una fila insertada entre dos páginas puede hacer que la siguiente repita o se salte una fila. `page` queda exactamente igual (sigue siendo la opción correcta cuando hace falta saltar a una página arbitraria, ej. "página 40"); `pageAfter` es una forma nueva, para el caso de scroll infinito/paginación secuencial, donde esa estabilidad importa más que poder saltar.

<!-- linkc:fragment -->
```rust
rpc feed(cursor: Int?, limit: Int) -> Item[] {
  db.items.pageAfter(cursor, limit)
}
```

**El cursor ES el `id` del último elemento visto** (`null` para la primera página) -- no un token opaco codificado aparte, a propósito: el `id` ya es un campo público del struct que el cliente ya recibió, así que envolverlo en una capa de codificación no agrega ninguna garantía real, solo ceremonia. Lo que hace a esto un cursor DE VERDAD no es que esté "ofuscado" -- es que `WHERE "id" > cursor ORDER BY "id" LIMIT n` no cuenta filas desde el principio como `OFFSET`, así que es estable bajo escritura concurrente: pasar el `id` del último elemento visto siempre da la página que sigue, sin importar cuántas filas se insertaron mientras tanto.

**Límite honesto:** solo hacia adelante -- no hay forma de pedir "la página anterior" a partir de un cursor (para eso, `page(limit, offset)` sigue estando disponible), ni de saltar a una posición arbitraria sin recorrer.

**Verificado:** un test unitario contra SQLite (`db.rs`) que además prueba explícitamente la propiedad de estabilidad -- inserta una fila nueva ENTRE dos llamadas a `pageAfter` y confirma que la segunda página no cambia -- más el mismo caso contra un PostgreSQL real en `pg_integration.rs`.

---

### 3.62 `@route` con parámetros de query string — RESUELTO

Hasta esta ronda, `@route` (§3.37/§3.42) exigía que el rpc tuviera EXACTAMENTE los parámetros que la ruta declara -- ni de más ni de menos. Eso significaba que cualquier endpoint que además necesitara un filtro (`?estado=activo`, `?page=2`) tenía que duplicar el rpc completo solo para agregar ese parámetro, porque `@route` no tenía forma de leer nada fuera del path.

<!-- linkc:fragment -->
```rust
type SearchResult = { q: String, page: Int? }

service Search {
  @route("/search")
  rpc search(q: String, page: Int?) -> SearchResult {
    // GET /search?q=rust&page=2  ->  q="rust", page=2
    // GET /search?q=rust         ->  q="rust", page=null (opcional, no 400)
    // GET /search                ->  400: falta 'q' (obligatorio)
    SearchResult { q: q, page: page }
  }
}
```

**La regla es simple: cualquier parámetro del rpc que NO esté nombrado en el path se lee de la query string, por nombre.** `String`/`Int` obligatorio (400 si falta), o `String?`/`Int?` si puede estar ausente sin que eso sea un error (`null` en ese caso). Los parámetros de PATH siguen siendo exactamente como antes -- esto solo agrega los que sobran. **`body` sigue sin leerse, a propósito**, no por falta de tiempo: la URL de `@route` existe para que un crawler (o cualquier link compartido) la abra con un GET normal, y un GET nunca trae body -- soportarlo ahí no tendría con qué activarse nunca.

**Un bug real encontrado escribiendo esta ronda, no solo la ausencia de la feature:** antes de esto, el path completo (incluyendo un eventual `?...`) se partía en segmentos directamente. Una URL tan común como `/blog/hola-mundo?utm_source=twitter` -- cualquier link real recibe parámetros de tracking tarde o temprano -- capturaba `"hola-mundo?utm_source=twitter"` ENTERO como valor de `:slug`, corrompiendo el parámetro. Ahora la query string se separa ANTES de partir en segmentos, así que esto se arregló para TODA ruta con `@route`, tenga o no parámetros de query declarados -- y de paso también para el `/Service/rpc` normal, que tenía la misma vulnerabilidad latente (nunca ejercitada en la práctica, porque el cliente TypeScript generado nunca agrega query string a un POST).

**Decodificación:** `+` significa espacio en un valor de query string (`application/x-www-form-urlencoded`) -- a diferencia de un segmento de path, donde `+` es un caracter literal. `%XX` se decodifica igual en los dos casos. Un query param no declarado por el rpc (como `utm_source` en el ejemplo de arriba) se ignora sin error -- ni bloquea, ni se cuela en ningún lado.

**Verificado** contra un servidor real (`cli_route.rs`): query param obligatorio y opcional leídos por nombre; falta el obligatorio -> 400 nombrando cuál; un `Int` inválido -> 400; la query string ya NO corrompe el segmento de path capturado (el test que fija el bug de arriba); un query param desconocido no pisa uno real; `+`/`%XX` decodificados correctamente.

---

### 3.63 `smtp.sendToMany()`/`smtp.sendHtml()`: varios destinatarios y cuerpo HTML — RESUELTO

`smtp.send` (§3.43) mandaba texto plano a UN destinatario -- mandar a varios significaba una llamada por destinatario (N conversaciones SMTP separadas, no un solo mensaje con varios `RCPT TO`), y no había forma de mandar HTML, algo que cualquier notificación transaccional real (confirmación de compra, bienvenida) necesita.

<!-- linkc:fragment -->
```rust
rpc notifyTeam(emails: String[], subject: String, html: String) -> Void {
  smtp.sendHtml(emails, subject, html)
}
```

**Dos métodos nuevos, `send` sin cambios** -- mismo criterio que `getWithHeaders`/`getWithStatus` (§3.47/§3.60): agregar, no sobrecargar.

| Método | Firma | Qué hace |
|---|---|---|
| `smtp.sendToMany(to, subject, body)` | `(String[], String, String) -> Void` | Texto plano, UN mensaje con un `RCPT TO:` por destinatario -- no N conversaciones SMTP separadas. |
| `smtp.sendHtml(to, subject, html)` | `(String[], String, String) -> Void` | Cuerpo HTML (`Content-Type: text/html`), a uno o varios destinatarios -- `to` es lista también acá, un solo elemento cubre el caso de un destinatario. |

Los dos comparten la conexión/remitente desde el ENTORNO del proceso (`LINK_SMTP_URL`/`LINK_SMTP_FROM`), mismo criterio de siempre (§3.43) -- ninguno de los dos agrega una forma de que el rpc elija el remitente. `to` vacío es un error de runtime claro, no un mensaje mandado a nadie.

**Límites honestos que siguen en pie:** sin adjuntos, sin `cc`/`bcc`, sin envío asíncrono -- los tres son sincrónicos, un relay lento sigue haciendo lento al servidor entero (de un solo hilo) mientras dura esa request. Nada de esto cambió respecto a `send`.

**Verificado** contra un servidor SMTP real armado a mano en el test (`cli_smtp.rs`, habla el protocolo real: EHLO/MAIL FROM/RCPT TO/DATA), no un mock interno: `sendToMany` con dos destinatarios produce dos `RCPT TO:` en la MISMA conversación; una lista vacía falla limpio; `sendHtml` produce un mensaje con `Content-Type: text/html` y el markup sin escapar en el body.

---

### 3.64 Auth externo: confiar en un JWT ya emitido — RESUELTO, alcance acotado (HS256)

Hasta esta ronda, Link solo emitía y validaba sus PROPIAS sesiones opacas (`auth.createSession(WithId)`, §3.14) -- no había forma de decirle "confiá en este JWT que mi backend Express/Node/lo-que-sea ya emitió". Eso bloqueaba CUALQUIER adopción de Link dentro de una app con login preexistente: la única salida era correr dos sistemas de sesión en paralelo, uno para los endpoints viejos y otro para los nuevos escritos en c-script.

<!-- linkc:fragment -->
```rust
enum Role { Admin, Member }

service Orders {
  // El caller manda Authorization: Bearer <jwt-emitido-por-otro-backend>.
  // Sin llamar auth.createSession en ningún lado -- el JWT YA autentica.
  @requires(Role.Admin)
  rpc cancel(id: Int) -> Void {
    db.orders.delete(id);
  }

  rpc whoAmI() -> String? {
    auth.currentRole() // lee el claim "role" del JWT, igual que de una sesión propia
  }
}
```

```
linkc serve app.link 8787 --jwt-secret "$JWT_SIGNING_SECRET"
```

**Un flag de servidor, no una anotación del lenguaje** -- mismo criterio que `--session-ttl`/`--argon2-memory-kib`: el secreto de firma es una decisión de DESPLIEGUE (compartida con el backend que ya emite los JWTs), nunca algo que un `.link` deba poder hardcodear. Sin `--jwt-secret`/`LINK_JWT_SECRET`, el comportamiento es IDÉNTICO al de antes de esta ronda -- cero JWT se intenta verificar nunca.

| Flag / env var | Default | Qué hace |
|---|---|---|
| `--jwt-secret` / `LINK_JWT_SECRET` | (ninguno -- feature apagada) | Secreto HMAC para verificar la firma. |
| `--jwt-role-claim` / `LINK_JWT_ROLE_CLAIM` | `"role"` | Qué claim trae el nombre del rol (`"Admin"`, matcheado por NOMBRE contra el `enum` que pida `@requires`). |
| `--jwt-user-id-claim` / `LINK_JWT_USER_ID_CLAIM` | `"sub"` | Qué claim trae el id de usuario -- acepta número JSON o string de dígitos (`"sub": "42"`, la convención real de OIDC). |

**Convive con las sesiones propias, nunca las reemplaza.** `SessionStore::role_for`/`user_id_for` prueban primero la sesión creada por este mismo programa (`auth.createSessionWithId`); si el token no está ahí Y hay `--jwt-secret` configurado, lo intentan como JWT externo. `auth.createSession(WithId)` sigue funcionando exactamente igual para cualquier endpoint nuevo escrito directamente en c-script -- una migración real no reemplaza su login existente de un día para el otro.

**Solo HS256 -- allowlist, no blocklist.** Cualquier otro valor de `alg` en el header del JWT (`"none"`, `"RS256"`, lo que sea) se rechaza explícitamente, ANTES de siquiera calcular una firma esperada. `"alg":"none"` es la vulnerabilidad de JWT más común y documentada que existe (un verificador que confía en lo que el propio token dice ser su algoritmo); aceptar RS256 verificado con una clave pensada para HMAC sería la misma clase de error de confusión de algoritmo. La firma se compara en tiempo constante (`constant_time_eq`, ya usado por `verifyPassword` desde §3.34) -- reusa la primitiva, no reinventa la comparación.

**Sin ningún enum de c-script asociado a un token externo.** `role_for` devuelve `("", variante)` para una sesión JWT -- el `""` es un sentinel: `check_auth_gate` matchea `@requires(Role.Admin)` por NOMBRE de variante nada más, sin la comparación de identidad de `enum` que sí aplica a una sesión creada por este mismo programa (donde SÍ hay un `enum` real detrás). En la práctica esto solo importa si un programa declarara dos `enum` de rol distintos con una variante de nombre idéntico -- un caso patológico, documentado acá en vez de ignorado.

**Límites honestos de esta ronda:**
- **Solo HS256 (HMAC con secreto compartido).** Sin RS256/ES256 (clave pública/privada) ni JWKS (rotación de claves vía endpoint `.well-known`) -- eso es un proveedor de identidad completo (Auth0, Clerk, Cognito), una ronda mucho más grande. HS256 cubre el caso más común de una migración real: el MISMO backend que emite los JWTs es el que configura `--jwt-secret`, así que un secreto compartido no es una limitación operativa, es exactamente el modelo de confianza que ya existe.
- **`exp` se respeta si está presente; sin `nbf`, `iss` ni `aud`.** Un JWT vencido (`exp` en el pasado) se rechaza; ninguno de los otros claims estándar de validación (`nbf`: "no válido antes de", `iss`: emisor esperado, `aud`: audiencia esperada) se chequea todavía.
- **Verificación no cacheada.** Cada llamada a `role_for`/`user_id_for` recalcula el HMAC del JWT -- una request que llama a las dos (típico: `check_auth_gate` + `auth.currentUserId()` dentro del cuerpo) lo verifica dos veces. Barato (un HMAC-SHA256), pero real.

**Verificado:** 11 tests unitarios en `session.rs` (JWT válido resuelve rol/id; `sub` como string de dígitos parsea a `Int`; nombres de claim configurables; firma con secreto equivocado rechazada; `alg:"none"` rechazado incluso con firma técnicamente válida; `alg:"RS256"` rechazado; JWT vencido rechazado; JWT sin `exp` nunca vence; entradas basura no paniquean; sin `--jwt-secret` un token con forma de JWT es simplemente desconocido; una sesión propia tiene precedencia) más 6 tests end-to-end contra un servidor real (`server_http.rs`): rol correcto satisface `@requires`, rol incorrecto da 403, cualquier rol satisface `@authenticated`, `auth.currentRole()`/`currentUserId()` leen los claims del JWT, firma inválida da 401, y sin `--jwt-secret` configurado un JWT sigue sin autenticar nada.

---

### 3.65 Agregación (`sumBy`/etc.): soporte de `Int64` — RESUELTO (fecha truncada sigue pendiente)

Hasta esta ronda, `sumBy`/`countBy`/`avgBy`/`maxBy`/`minBy` (§3.52) rechazaban `Int64` tanto como campo de agrupación como campo de valor -- un programa con IDs o montos declarados `Int64` (el tipo correcto para cualquier valor que pueda superar 2^53, GRAMMAR.md §3.30) no podía usar agregación real sobre ellos en absoluto.

<!-- linkc:fragment -->
```rust
type Sale = { id: Int, region: Int64, amount: Int64 }
type RegionTotal = { key: Int64, value: Int64 }

rpc totalByRegion() -> RegionTotal[] {
  db.sales.sumBy(|s: Sale| { s.region }, |s: Sale| { s.amount })
}
```

**Un bug real encontrado auditando el gap, no solo la ausencia de la feature:** `scalar_cell_to_value` (`runtime/db.rs`, la función que reconstruye un `Value` a partir de una fila de SQL) nunca distinguía `Int64` de `Int` -- ambos comparten `ColumnKind::Int` (mismo `BIGINT` de storage), así que la única forma de saber cuál armar es mirar el `Type` declarado, no la celda SQL, que es idéntica para los dos. Si `Int64` hubiera colado como key o value ANTES de esta ronda (no colaba, el checker ya lo rechazaba), el resultado habría llegado etiquetado `Value::Int` -- y por lo tanto serializado como NÚMERO en el JSON, rompiendo la promesa de §3.30 de que `Int64` siempre viaja como STRING para no perder precisión. La ronda cierra las dos cosas juntas: el checker ahora acepta `Int64`, y el runtime lo etiqueta bien.

**Límite que sigue en pie: sin truncado de fechas.** Agrupar por un `Timestamp` sigue sin aceptarse -- un `Timestamp` se guarda como milisegundos exactos (`BIGINT`, §3.31), así que agruparlo tal cual produciría un grupo por fila, nunca cohortes reales. Lo que hace falta es un método de truncado (`.truncateToMonth()`, por ejemplo) reconocido en la MISMA posición de selector, empujado a `DATE_TRUNC`/`strftime` según el backend -- una ronda aparte a propósito: los dos backends divergen de verdad acá (Postgres necesita convertir el `BIGINT` a un `timestamp` nativo con `to_timestamp`/`EXTRACT(EPOCH ...)` antes de truncar; SQLite trunca con `strftime` y devuelve texto, no milisegundos), y ese tipo de divergencia entre backends es exactamente la clase de bug que este proyecto viene encontrando y documentando desde §3.9 -- mejor una ronda propia con tests dedicados en los dos motores que apurarla acá.

**Verificado:** un test de runtime contra SQLite (`runtime/mod.rs`) que agrupa y suma por un campo `Int64`, confirmando que el resultado es `Int64` de verdad (no solo que el valor numérico coincide) comparándolo contra `1200.toInt64()`; el mismo caso contra un PostgreSQL real (`pg_integration.rs`), donde además confirma que `key` y `value` viajan como STRING en el JSON, no como número -- la parte que un bug de etiquetado hubiera roto en silencio; y un test de compilación que confirma que el tipo del resultado tipa como `Int64`, no `Int`.

---

### 3.66 `linkc introspect`: generar un `.link` desde una base PostgreSQL existente — RESUELTO, alcance acotado

Reporte real de adopción (app financiera "MyFinance" sobre una base Postgres ya existente): sin esto, adoptar Link dentro de un sistema con datos reales significaba escribir cada `type`/`db {...}` a mano, columna por columna, mirando el schema en otra ventana.

```
linkc introspect postgres://usuario:clave@host/base > main.link
```

Lee `information_schema` del schema `public` (tablas base + columnas + clave primaria) y emite un `.link` de partida: un `type` por tabla, más el bloque `db {...}` que las declara todas como colecciones. **Es un punto de partida para revisar a mano, no un `.link` listo para producción sin mirarlo** -- el archivo generado empieza con un comentario que lo dice, y cualquier columna que este comando no pueda mapear con confianza sale igual (nunca se omite una columna en silencio) como `String`, con una advertencia en STDERR y del lado del código explicando qué revisar.

**Qué mapea con confianza (sin advertencia):** `bigint`/`integer`/`smallint` -> `Int` (los tres decodifican igual desde §3.59); `boolean` -> `Bool`; `double precision`/`real`/`numeric` -> `Float`; `text`/`character varying`/`character` -> `String`. Una columna `NULL`-able sale como `T?`.

**Qué mapea con advertencia (sigue emitiendo un tipo válido, `String`, pero avisa):**
- `jsonb`/`json`: la FORMA real del JSON no se puede inferir de `information_schema` -- hace falta declarar un `type` propio a mano si se necesita.
- `uuid`: se mapea al texto tal cual, no hay un tipo UUID dedicado.
- `timestamp`/`timestamptz`/`date`: el `Timestamp` de c-script necesita milisegundos en `BIGINT` (§3.31), no el tipo de fecha/hora NATIVO de Postgres -- una columna así no es directamente compatible, hace falta migrarla o convertir a mano.
- Cualquier otro tipo sin mapeo conocido (`inet`, `cidr`, arrays, etc.).

**Los nombres de campo son los nombres REALES de columna SQL, `snake_case` incluido -- a propósito.** c-script no tiene ningún mecanismo de alias campo↔columna (`insert`/`find`/etc. usan el nombre del campo COMO nombre de columna, `runtime/db.rs`), así que "prolijizar" a `camelCase` acá rompería la conexión real con la tabla. Queda como ejercicio manual para quien lo quiera (y también renombrar la columna real).

**Límites honestos de esta ronda:**
- **Solo PostgreSQL, nunca SQLite.** El caso real que motiva esto -- adoptar un sistema existente -- casi siempre es sobre una base de producción ya corriendo, y eso es Postgres.
- **Solo tablas con una PK simple llamada `"id"`.** Sin PK, o una PK compuesta, o una PK que no se llama `"id"`: igual emite el `type` (con un campo `id: Int` placeholder y un comentario `// TODO`), pero avisa que hace falta revisar a mano -- c-script requiere exactamente una columna `"id"` entera autoincremental por colección.
- **Sin foreign keys, índices, constraints de `CHECK`, ni valores default.** Solo columnas y su nullability.
- **No genera ningún `service`.** El `.link` resultante tiene los `type`/`db` para conectar, pero cero `rpc` -- escribir el servicio sigue siendo trabajo del desarrollador, a propósito: no hay forma de adivinar qué operaciones necesita la app real.

**Verificado** contra un PostgreSQL real (`pg_integration.rs`): una tabla creada A MANO (simulando un sistema ya existente, no generada por `linkc`) con columnas `NOT NULL`/nullable de varios tipos -- el `.link` generado no solo "parece" correcto, se guarda, se le agrega un `service` mínimo a mano, y `linkc serve` conecta de verdad contra la MISMA tabla y lee la fila sembrada antes de que `linkc` supiera que esa tabla existía. Un segundo test confirma que columnas `jsonb`/`timestamptz` generan la advertencia esperada en stderr sin romper la compilación del archivo.

---

### 3.67 `--adopt-existing`: adoptar tablas sin auto-migrar — RESUELTO

Hasta esta ronda, `linkc serve` SIEMPRE intentaba `CREATE TABLE IF NOT EXISTS` (SQLite y PostgreSQL) y auto-migrar con `ALTER TABLE ADD COLUMN` (columnas opcionales faltantes en SQLite; todas las columnas, siempre, en PostgreSQL) al abrir cada colección declarada. Dos bloqueos reales para adoptar un sistema existente que eso dejaba sin resolver:

1. **Un rol de base sin permiso de DDL.** Una restricción común en producción: la cuenta que usa la app tiene `SELECT`/`INSERT`/`UPDATE`/`DELETE`, pero no `CREATE`/`ALTER` -- y `linkc serve` ni siquiera arrancaba, aunque el schema ya matcheara exacto y no hiciera falta migrar nada de verdad.
2. **Una tabla SQLite con columnas físicas que el `.link` no modela.** `check_schema_matches` exige coincidencia EXACTA entre lo declarado y lo que ya existe -- una tabla legacy con una columna de más (que el programa nunca va a leer) hacía panic al arrancar.

```
linkc serve app.link 8787 --db postgres://usuario:clave@host/base --adopt-existing
```

`--adopt-existing` (o `LINK_ADOPT_EXISTING` con cualquier valor no vacío) le dice a `linkc serve` que asuma que TODAS las tablas ya existen: nunca ejecuta `CREATE TABLE` ni `ALTER TABLE`, ni siquiera uno no destructivo. En su lugar valida, con SELECTs de solo lectura, que cada columna DECLARADA en el `.link` exista en la tabla física:

| Backend | Qué valida en modo adopción | Qué ignora |
|---|---|---|
| SQLite | Cada columna declarada existe con el tipo SQL esperado (`PRAGMA table_info`) | Cualquier columna física no declarada en el `.link` |
| PostgreSQL | Cada columna declarada existe por nombre (`information_schema.columns`), más el chequeo de siempre de que `"id"` sea un entero (§3.59) | Tipo columna por columna (más allá de `"id"`) -- mismo criterio que `validate_existing_id_column` ya aplicaba fuera de este modo |

Si falta una tabla entera, o falta una columna declarada, `linkc serve` no arranca -- con un mensaje que dice exactamente qué falta, nunca con el `CREATE TABLE`/`ALTER TABLE` silencioso que el modo normal haría.

**Límites honestos:**
- **Todo o nada por proceso.** La flag aplica a TODAS las colecciones que el programa declara, no colección por colección -- si algunas tablas son nuevas (y sí necesitan crearse) y otras son legacy (y no), hoy hace falta separarlas en dos programas/procesos.
- **No valida `NOT NULL` en SQLite, ni tipo columna por columna en PostgreSQL más allá de `"id"`.** Una fila vieja con `NULL` en un campo que el `.link` declara requerido, o un tipo de columna incompatible, recién falla en la primera lectura que la toque -- con el error normal de decode, no al conectar. Mismo criterio que `validate_existing_id_column` ya documentaba para PostgreSQL fuera de este modo.
- **Una columna declarada opcional pero ausente en la tabla física también falla.** A propósito: el punto entero de este modo es no tocar DDL, así que ni siquiera el `ALTER TABLE ADD COLUMN` no destructivo que el modo normal haría para un campo opcional corre acá.

**Verificado**: `runtime/db.rs` (SQLite, contra un archivo real -- tabla con una columna extra se adopta igual, tabla faltante y columna declarada faltante fallan con el mensaje esperado), `cli_adopt_existing.rs` (dos corridas reales y consecutivas de `linkc serve` sobre el MISMO archivo SQLite: la primera crea la tabla normalmente con una columna que la segunda no declara, la segunda arranca en modo adopción y la ignora; más el flag y la env var probados por separado) y `pg_integration.rs` contra un PostgreSQL real (tabla creada a mano con una columna sin modelar, `linkc serve --adopt-existing` arranca y la ignora; una columna declarada faltante falla limpio, sin panic).

---

### 3.68 NULL en una columna requerida tras una migración de PostgreSQL: error limpio, no `null` silencioso — RESUELTO

Auditando el comportamiento real de auto-migrate para PLAN.md §9.1.1 (matriz de comportamiento pedida en dos reportes de adopción reales) apareció un bug genuino, no solo un gap de documentación: `connect_postgres` (GRAMMAR.md §3.36) agrega SIEMPRE una columna nueva como `NULLABLE` -- nunca puede saber qué backfillear en filas ya existentes, sin importar si el campo es requerido en el `.link` actual. Una fila insertada ANTES de declarar ese campo requerido queda con `NULL` en esa columna. Hasta esta ronda, `row_to_fields` (`runtime/db.rs`) decodificaba ese `NULL` en silencio como `Value::Null` -- el cliente TypeScript recibía `null` en un campo que su propio contrato generado declara `string` (no `string | null`), sin ningún error en ningún lado. Exactamente la clase de "los dos extremos no están de acuerdo" que este proyecto viene evitando desde §3.9.

<!-- linkc:check -->
```
type Item = { id: Int, name: String, note: String }
db { items: Item[] }

service Items {
  rpc list() -> Item[] { db.items.all() }
}
```

Si la tabla física de `items` tiene una fila con `NULL` en `note` (típico tras un `note: String?` -> `note: String` sobre datos ya existentes), `Items/list` ahora devuelve un error de runtime normal -- un 5xx JSON como cualquier otro, nunca un `null` silencioso ni un panic que tumbe el proceso -- que nombra la colección, el `id` de la fila y el campo.

| Caso | Antes de esta ronda | Ahora |
|---|---|---|
| Campo nativo (`Int`/`String`/etc.) requerido, columna física `NULL` | `Value::Null` silencioso, `"note": null` en el JSON de un campo `string` | `RuntimeError` limpio, 5xx, nombra colección/id/campo |
| Campo requerido cuyo tipo es un struct/enum-con-datos/lista/etc. (columna JSON), columna física `NULL` | Mismo `Value::Null` silencioso | Mismo `RuntimeError` limpio |
| Campo `T?` o `x?: T` (nullable/opcional de verdad) | `Value::Null` o clave ausente, correcto | Sin cambios -- sigue siendo el comportamiento correcto |

**Por qué un error de runtime normal y no un panic.** `handle_rpc` corre sincrónico en el hilo principal del accept-loop (`server.rs`) -- un panic ahí no tira abajo solo esa request, tira abajo el PROCESO ENTERO, el mismo motivo por el que el bug de `id UUID` (§3.36) y el de `id SERIAL` de 32 bits (§3.59) se corrigieron como errores limpios en vez de panics. `row_to_fields` pasó de devolver `Vec<(String, Value)>` a `Result<Vec<(String, Value)>, RuntimeError>`; solo tiene 3 call sites (`select_rows`/`select_rows_page`/`select_rows_after`), los tres ya propagaban `RuntimeError` con `?` para errores de SQL, así que el cambio fue aditivo en la práctica.

**Por qué esto casi nunca pasa en SQLite.** El `CREATE TABLE`/`check_schema_matches` de SQLite (§3.17, con la matriz de comportamiento completa) exige coincidencia EXACTA de schema en cada arranque -- un campo que pasa de opcional a requerido ya falla AL CONECTAR, antes de que cualquier request pueda leer una fila con este problema. El camino solo es alcanzable de verdad en PostgreSQL, donde el auto-migrate SÍ deja pasar el connect.

**Límites honestos:**
- No hay backfill automático -- el error dice qué fila y qué campo, corregirlo (a mano, o con un `UPDATE` propio) sigue siendo responsabilidad del operador.
- `--adopt-existing` (§3.67) no cambia nada acá: sigue validando solo que la columna EXISTA, nunca sus valores reales.

**Verificado**: 5 tests nuevos en `runtime/mod.rs` confirman la matriz completa de auto-migrate contra SQLite real (columna eliminada, renombrada, tipo cambiado, requerido→opcional, opcional→requerido -- las 5 fallan al conectar, con el mismo mensaje ya documentado en §3.17); 1 test nuevo en `pg_integration.rs` contra un PostgreSQL real siembra una fila con `NULL` a mano (simulando datos de antes de la migración), confirma que `list()` falla con un error que nombra el campo -- nunca un panic -- y que el servidor sigue respondiendo 200 normal a la request SIGUIENTE contra una fila sin el problema.

---

### 3.69 Narrowing real de `T?`: `match`, `??` y `.isSome()`/`.isNone()` — RESUELTO

El gap más repetido y con más fricción de dos reportes de adopción real independientes (MyFinance, IgnisLove): hasta esta ronda no había NINGUNA forma de leer el valor interior de un `T?` dentro de un `rpc` -- `if x != null { x.campo }` tipaba el `if` pero el acceso a `campo` adentro siempre fallaba, a propósito (§3.4), porque c-script no tenía narrowing vía `if`. La salida documentada era "devolvé el `T?` tal cual y desarmalo del lado de TypeScript" -- una respuesta real, pero que en la práctica bloqueó lógica de negocio genuina (un caso real: validar la caducidad de un cupón tuvo que moverse FUERA del servidor, al cliente, porque no había forma de comparar `cupon?.expiresAt` contra `now()` dentro del `rpc`).

**La pieza que ya existía y hacía esto tratable en una sola ronda**: el narrowing de uniones vía `match` (§3.9) -- `Pattern::Type(nombre, Tipo)` para ligar una variable al tipo real, más el algoritmo de exhaustividad que ya usa `check_exhaustive_union`. Un `T?` no es más que "T o ausente" -- dos casos, no una lista de miembros -- así que se resuelve con el MISMO mecanismo de patrones, una función de exhaustividad hermana (`check_exhaustive_optional`, dos `bool` en vez de un `Vec<bool>` por miembro), y un patrón nuevo: `null` como literal de patrón (antes explícitamente prohibido, `LiteralPattern` no tenía esa variante).

<!-- linkc:check -->
```rust
type Coupon = { id: Int, code: String, expiresAt: Timestamp }

service Coupons {
  // 'match' desarma el T? de verdad: 'cc' queda ligado a Coupon (no
  // Coupon?) dentro de esa rama -- ahí sí se puede leer cc.code/cc.expiresAt.
  rpc describe(c: Coupon?) -> String {
    match c {
      cc: Coupon => "activo: " + cc.code,
      null => "sin coupon",
    }
  }

  // '??': el caso común, "dame un default" -- azúcar sobre el mismo match,
  // sin la ceremonia de escribirlo entero para solo eso.
  rpc nameOrDefault(name: String?) -> String {
    name ?? "anonimo"
  }

  // '.isSome()'/'.isNone()': cuando el cuerpo solo necesita SABER si hay
  // valor, no leerlo -- la rama sin la ceremonia de un match completo.
  rpc hasCoupon(c: Coupon?) -> Bool {
    c.isSome()
  }
}

test "narrowing real sobre T? funciona en los tres casos" {
  let present = Coupon { id: 1, code: "AHORRO10", expiresAt: now() };
  assert(Coupons.describe(present) == "activo: AHORRO10");
  assert(Coupons.describe(null) == "sin coupon");
  assert(Coupons.nameOrDefault("Ada") == "Ada");
  assert(Coupons.nameOrDefault(null) == "anonimo");
  assert(Coupons.hasCoupon(present));
  assert(!Coupons.hasCoupon(null));
}
```

**Las tres formas, y cuándo usar cada una:**

| Forma | Cuándo | Lo que da |
|---|---|---|
| `match x { v: T => ..., null => ... }` | Necesitás leer campos/métodos de `T`, o las dos ramas hacen algo distinto de verdad | Narrowing completo -- `v` es `T`, no `T?`, dentro de esa rama |
| `x ?? default` | Solo necesitás "el valor, o un default si no hay" | El `T` desenvuelto directo, sin match -- encadenable (`a ?? b ?? c`, asocia a izquierda) |
| `x.isSome()` / `x.isNone()` | Solo necesitás SABER si hay valor, no leerlo | `Bool` -- la rama sigue sin poder leer el valor sin `match`/`??` |

**`match` sobre `T?` es exhaustivo de verdad, igual que sobre una unión.** Falta el caso `null`, o falta el caso `v: T` (o ambos): error de compilación (`match no exhaustivo sobre {T}?: falta cubrir [...]`), nunca un caso sin cubrir que explota en runtime. Un `Pattern::Bind` sin guard (`_ => ...` o `cualquiera => ...`) cubre los dos casos a la vez, mismo criterio que el resto de los escrutinios de este lenguaje (§3.3).

**`a ?? b` acepta DOS formas para `b`, no solo una.** Si `b` es el `T` desenvuelto (el caso común), el resultado de `a ?? b` es `T`, definitivo -- ya no opcional. Si `b` es TAMBIÉN `T?` (para encadenar `a ?? b ?? default`), el resultado sigue siendo `T?` hasta que algún eslabón de la cadena sea definitivo. `a ?? b ?? c` no necesita ningún caso especial para "la cadena completa" -- asocia a izquierda como cualquier otro binario, así que cada `??` sólo mira sus dos operandos inmediatos. Cortocircuita: `b` nunca se evalúa si `a` ya tiene valor, mismo criterio que `&&`/`||`.

**`.isSome()`/`.isNone()` funcionan sobre CUALQUIER `T?`, incluido uno struct-shaped -- con un caso adversarial real resuelto.** Un opcional no tiene ningún envoltorio en runtime: su forma "presente" es el valor de `T` tal cual (`Value::Struct` si `T` es un struct), y su forma "ausente" es `Value::Null`. Eso significa que `x.isSome()` sobre un `x: Item?` presente evalúa `x` a un `Value::Struct` normal -- y el camino genérico de `FieldAccess` sobre un struct busca un CAMPO real llamado `isSome`, no un método, así que un struct que declara honestamente un campo `isSome: (Int) -> Bool` (closures como campos, §3.10) y lo llama con esa misma sintaxis tiene que seguir funcionando como ESE campo. Se resuelve mirando, en runtime, si el valor real tiene un campo con ese nombre ANTES de asumir que es el método del opcional -- verificado con un test que arma ese struct adversarial a propósito.

**Límites honestos:**
- **Narrowing solo vía `match`, nunca vía `if`.** `if x != null { x.campo }` sigue sin angostar -- decisión deliberada de §3.4/§3.9, no algo que esta ronda haya revisado. Quien quiera narrowing con sintaxis de `if` tiene que usar `match` con un patrón de tipo, no hay un atajo `if let` separado.
- **`match` sobre `T?` exige EXACTAMENTE el tipo interno, no un supertipo/subtipo arbitrario** -- mismo criterio de subtipado mutuo que ya usa `check_exhaustive_union` (dos structs estructuralmente idénticos SÍ se aceptan entre sí, por diseño de subtipado estructural -- no es una laxitud nueva de esta ronda).
- **`??` no valida en runtime que el default sea "seguro"** -- si el default es una llamada costosa (una lectura a `db`, por ejemplo), cortocircuita igual que `&&`/`||`, pero seguirá pagando ese costo cada vez que el lado izquierdo SÍ sea `null`.
- **Nada de esto cruza a TypeScript.** `match`/`??`/`.isSome()`/`.isNone()` son construcciones de BACKEND -- el contrato generado sigue emitiendo `T | null` para un campo `T?` tal cual siempre lo hizo (§3.4); el cliente sigue angostando con las herramientas propias de TypeScript, sin ningún cambio.

**Verificado**: 14 tests nuevos en `checker.rs` (exhaustividad completa -- falta `null`, falta el caso de valor, wildcard cubre los dos, un patrón de tipo incompatible se rechaza, `null` contra un escrutinio no opcional se rechaza, `??` sobre algo no opcional se rechaza, el lado derecho de `??` tiene que ser `T` o `T?`, encadenar typechecked, `.isSome()`/`.isNone()` se rechazan sobre un tipo no opcional y no aceptan argumentos) y 7 en `runtime/mod.rs` contra un servidor real vía `invoke_rpc` (narrowing de struct y de primitivo en los dos sentidos, `??` con valor presente/default/cortocircuito real, encadenado de 3 opcionales, `isSome`/`isNone` sobre un struct-shaped `T?`, y el caso adversarial del campo `isSome` real que no se deja shadowear); 1 test más en `lsp.rs` confirma que el completion sobre un `T?` ofrece solo `isSome()`/`isNone()`, nunca los campos de `T`.

---

### 3.70 Tipo nativo `Uuid` — RESUELTO

Hasta esta ronda, un identificador con forma de UUID era `String` -- nada impedía que `"hola"` llegara donde el programa esperaba un identificador real, y validar el formato quedaba en manos de cada `rpc`, a mano, cada vez.

<!-- linkc:check -->
```rust
type Session = { id: Int, token: Uuid }
type NewSession = { token: Uuid }

db { sessions: Session[] }

service Sessions {
  rpc create() -> Session {
    db.sessions.insert(NewSession { token: crypto.uuid() })
  }
  rpc get(id: Int) -> Session? {
    db.sessions.find(id)
  }
}

test "un uuid generado se guarda y se lee de vuelta identico" {
  let s = Sessions.create();
  assert(s.token.toString().length() == 36);
  match Sessions.get(s.id) {
    v: Session => assert(v.token == s.token, "el mismo uuid vuelve identico"),
    null => panic("se esperaba encontrar la sesion"),
  }
}
```

**Forma validada, no solo un alias de `String`.** `Uuid` exige la forma canónica `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` (36 caracteres, hex en las 32 posiciones que no son guión) -- sin restringir el nibble de versión/variante, así que cualquier UUID RFC 4122 real (v1/v4/v7/...) es válido, pero basura con la forma equivocada no. La validación pasa en los TRES lugares que un valor puede cruzar una frontera de tipo: el runtime al decodificar JSON entrante (`json_to_typed_value`, un escaneo manual de bytes, sin sumar la crate `regex` solo para esto), `validators.ts` (la misma forma como regex de JS), y `schemas.ts`/Zod (`z.string().regex(...)`) -- las tres regex son literalmente la misma, para que las tres capas nunca puedan divergir sobre qué es válido. `openapi.json` usa el idiom estándar `"format": "uuid"` en vez de un pattern propio.

**Tipo aparte de `String`, sin mezcla implícita -- mismo criterio que `Int64` vs `Int`.** `crypto.uuid()` devuelve `Uuid`, no `String`; `"prefijo-" + unUuid` es un error de compilación, igual que comparar un `Uuid` con un literal `String` sin desenvolverlo primero. `.toString()` es la conversión explícita (mismo patrón ya establecido para `Int`/`Int64`/`Float`/`Bool`, §3.55) -- después de eso, cualquier método de `String` (`.length()`, `.contains()`, etc.) funciona normal.

**Runtime: variante propia (`Value::Uuid`), no reusa `Value::Str`.** La razón real, no solo prolijidad: una vez que un valor cruza al runtime, la información de tipo ESTÁTICO ya no está disponible -- `call_method` no podría distinguir "esto es un `Uuid`, `.toString()` tiene sentido" de "esto ya es un `String` plano" si los dos compartieran la misma representación. Mismo criterio exacto que ya justificaba una variante propia para `Type::Timestamp`/`Value::Timestamp` (§3.31): el borde serializa igual (ambos son un string plano en el wire), pero el runtime necesita distinguirlos.

**Storage: `TEXT` en los dos backends, nunca envuelto en JSON.** Mismo criterio de "sin rama por backend" que el resto del lenguaje -- SQLite no tiene un tipo `UUID` nativo, así que Postgres tampoco lo usa, aunque podría. La validación de forma ya pasó en el borde JSON antes de que un `Value::Uuid` pueda siquiera llegar a la capa de storage, así que la columna física no necesita ningún constraint propio -- verificado con `sqlite3 archivo.db ".schema"` mostrando `"token" TEXT NOT NULL`, no una columna JSON.

**Límites honestos:**
- **Sin sintaxis de literal `Uuid`.** No hay forma de escribir un UUID directamente en el código fuente (`"..." as Uuid` no existe) -- un `Value::Uuid` solo nace de `crypto.uuid()` o de un wire decode que ya validó el formato. Para un valor fijo en un `test`, hay que recibirlo como parámetro o generarlo con `crypto.uuid()`.
- **No valida versión/variante RFC 4122.** Un UUID "nil" (`00000000-0000-0000-0000-000000000000`) o cualquier otro con nibbles de versión/variante inválidos pasa la validación -- solo se exige la forma general 8-4-4-4-12 en hex, no conformidad estricta con el RFC.
- **Sin tipo `Uuid` dedicado en WASM.** El codegen wasm nativo sigue siendo solo escalares (`Int`/`Int64`/`Bool`/`Float`) -- una función con un parámetro o retorno `Uuid` no compila a WASM, mismo límite que ya aplica a `String`.

**Verificado**: 5 tests nuevos en `checker.rs` (resuelve como nombre de tipo en campos de struct y firmas de rpc, `crypto.uuid()` tipa `Uuid` no `String`, sin mezcla implícita con `String` ni en asignación ni en `+`, `.toString()` funciona) y 3 en `runtime/mod.rs` contra un servidor real vía `invoke_rpc` (7 variantes de UUID malformado rechazadas con 400 -- forma corta, forma larga, sin guiones, caracteres no-hex, un número JSON, `null` -- todas nombrando el campo; un UUID válido, incluido en mayúsculas, viaja por el wire exacto; `crypto.uuid()` genera un UUID real que sobrevive un `insert`+`find` contra SQLite real, idéntico byte a byte). Verificado a mano contra un servidor HTTP real (`curl`) y contra el archivo SQLite generado (`sqlite3 ... ".schema"`) además de los tests automatizados.

---

## 4. Tabla de Mapeo c-script → TypeScript (exhaustiva)

| Construcción c-script | TypeScript emitido | Forma JSON en el cable | Nota |
|---|---|---|---|
| `Int`, `Float` | `number` | número | — |
| `Int64` | `string` | string (decimal, ej. `"9223372036854775807"`) | mismo rango `i64` que `Int`, serializado como string para no perder precisión arriba de `2^53` -- ver §3.30. `.toInt64()`/`.toInt()` para convertir; sin mezcla implícita con `Int` |
| `Timestamp` | `string` | string ISO-8601 de forma fija, ej. `"2026-08-08T14:30:00.000Z"` | milisegundos desde epoch UTC internamente -- ver §3.31. Obtenible con `now() -> Timestamp` (§3.32). Solo comparable (`< <= > >= == !=`); sin aritmética |
| `now()` | `now(): Timestamp` | `"2026-08-15T12:00:00.000Z"` | función builtin de fecha y hora actual en UTC (§3.32) |
| `assert`, `panic` | — | — | funciones builtin de aserción y control de tests en backend (§3.33) |
| `test "nombre" { }` | — | — | bloques de test de comportamiento (§3.33), no cruzan a TS |
| `String` | `string` | string | — |
| `Uuid` | `string` | string, forma canónica validada | tipo aparte de `String`, sin mezcla implícita -- `.toString()` para convertir (§3.70). Construible con `crypto.uuid()` |
| `Bool` | `boolean` | bool | — |
| `Void` | `void` | `null` en el cuerpo | Solo válido como retorno COMPLETO de un `rpc` -- como campo o parámetro es un error del checker (§4.1) |
| `T[]` | `T[]` | array | — |
| `Map<K, V>` | `Record<K, V>` | objeto | `K` limitado a `String`/`Int` (claves JSON); `{K: V}` como literal de tipo NO se parsea, ver §2.2 |
| `(A, B)` | `[A, B]` | array de longitud fija | tupla, ver §2.2 sobre ambigüedad de paréntesis |
| `(A) -> B` | `(arg0: A) => B` | — | solo dentro del backend; usarlo en la firma de un `rpc` (o en un tipo que esa firma alcance) es un error del checker (§4.1) |
| `A \| B` | `A \| B` | valor tal cual, con la forma de cualquiera de los miembros | subtipado de flujo de valor Y narrowing vía `match` — resuelto en §3.9 |
| `type X = {...}` | `interface X {...}` (structural) | objeto | subtipado estructural, §3.2 |
| `type X<T> = {...}` | `interface X<T> {...}` | objeto | monomorfizado en el backend, genérico en TS, §3.6 |
| `enum E { A, B }` | `type E = "A" \| "B"` | string | enum simple = unión de literales |
| `enum` con datos (ADT) | unión discriminada con tag fijo `type` (no configurable en v0) | objeto con campo `type` | ver ejemplo `Result` en PLAN.md §2.2 |
| `x: T?` (campo) | `x: T \| null` | clave presente, valor `null` | resuelto en §3.4 |
| `x?: T` (campo) | `x?: T` (clave ausente = `undefined`) | clave omitida | resuelto en §3.4 |
| `Patch<T>` | todos los campos `?:`, preserva nullability de cada uno | — | utilitario análogo a `Partial<T>`, resuelto en §3.4 |
| `rpc f(x: T = v)` | parámetro con default → opcional en la firma TS del cliente | — | `f(x?: T)` en el cliente si se omite |
| `rpc f(...) -> Result<T, E>` | `{type:"Ok",value:T} \| {type:"Err",error:E}` | objeto con tag `type` | resuelto en §3.5 — nunca lanza para errores declarados |
| `stream f(...) -> T` | `AsyncIterable<T>` | eventos SSE reales (`data: ...\n\n`), uno por `T` serializado, sobre chunked transfer | §3.13: cuerpo genérico, repite una lista ya calculada. §3.16: cuerpo `while true { db.<col>.subscribe() }`, push real de eventos futuros |
| `service S { ... }` | `interface SClient { ... }` + instancia concreta generada | — | el cliente real es un thin wrapper sobre `fetch`/WS |
| `const X: T = v` | `export const X: T = v` **en `client.ts`**, no en `contract.d.ts` | — | un `.d.ts` es ambiental y TS rechaza inicializadores ahí (TS1039); un `const` es un valor, así que vive en el módulo real |

### 4.1 Qué puede aparecer en la firma de un `rpc`

Todo lo que aparece en la firma de un `rpc`/`stream` viaja de verdad por la red, así que tiene que ser expresable como JSON. Dos tipos de la tabla de arriba NO lo son, y el checker los rechaza en esa posición:

- **Tipos función** (`(A) -> B`) en cualquier lado de la firma, incluso anidados dentro de un struct que la firma alcance. Dentro del backend siguen siendo válidos (pasar una `fn` a otra, §3.10) -- lo que no puede es cruzar.
- **`Void`** en cualquier posición que no sea el retorno COMPLETO de un `rpc`. Como campo de struct o parámetro no significa nada.

Esta regla existía como afirmación en la tabla desde el principio, pero nada la hacía cumplir: hasta la auditoría, un `type T = { h: (Int) -> String }` usado como retorno tipaba, emitía `h: (arg0: number) => string` al contrato, y generaba un validador con `typeof x.h === "function"` -- una condición que ningún payload JSON puede satisfacer, así que el cliente rechazaba SIEMPRE la respuesta. Un error de compilación claro es mejor que un contrato imposible de cumplir.

### 4.2 Validación en los dos extremos

El contrato no es solo una promesa de tipos en tiempo de compilación: los dos extremos lo verifican en runtime, con errores de categorías distintas.

| Dirección | Quién valida | Qué pasa si no matchea |
|---|---|---|
| Respuesta (servidor → cliente) | `validators.ts`, llamado desde `client.ts` (§3.11) | `LinkValidationError` en el cliente |
| Petición (cliente → servidor) | el servidor, contra el tipo declarado de cada parámetro | HTTP **400** con la ruta exacta del campo que falló |

La segunda mitad faltaba por completo hasta la auditoría: el servidor convertía el JSON entrante con una función puramente sintáctica, sin mirar ningún tipo. Las consecuencias reales están documentadas en el commit que lo arregló; la más visible era que un enum recibido por el wire nunca llegaba a ser un enum de verdad adentro, así que `match` sobre cualquier parámetro de tipo enum fallaba siempre. Un campo de más en la petición se acepta (subtipado de ancho, §3.2) pero se descarta: el valor que entra al backend tiene EXACTAMENTE la forma declarada.

---

## 5. Estado

`T?` (§3.4) y el manejo de errores (§3.5) quedaron resueltos con los defaults recomendados en `PLAN.md` §8.3 — ver `examples/decision-nullability.ts` y `examples/decision-errors.ts` para el resultado aplicado. Son reemplazables: si el criterio real termina siendo otro, es un cambio acotado a esas dos secciones y al emisor, no un rediseño del lenguaje.

El compilador está construido y vive en `compiler/` (Rust; dependencias en `Cargo.toml` — `tiny_http`/`serde_json`/`serde` para el runtime del demo, más `wasm-encoder` §3.20 y `rusqlite` §3.17, agregadas a propósito y documentadas donde se justifican, no un descuido). Para el estado real y actualizado de qué está hecho y qué no, ver la sección "Estado" del [README](README.md) — este documento describe el LENGUAJE, no el avance del proyecto. Cada gap de diseño que se fue cerrando tiene su propia sección `§3.X — RESUELTO` acá arriba, incluyendo lo que quedó deliberadamente afuera y por qué.
