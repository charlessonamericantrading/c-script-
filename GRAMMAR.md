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
  - [3.71 `@deprecated("motivo")` en un campo o un rpc — RESUELTO](#371-deprecatedmotivo-en-un-campo-o-un-rpc--resuelto)
  - [3.72 Docstrings `///` propagados a OpenAPI y al `.d.ts` — RESUELTO](#372-docstrings--propagados-a-openapi-y-al-dts--resuelto)
  - [3.73 `@validate(email)` / `@validate(regex, "...")` sobre un campo — RESUELTO](#373-validateemail--validateregex--sobre-un-campo--resuelto)
  - [3.74 Valores por defecto en campos de `struct` — RESUELTO](#374-valores-por-defecto-en-campos-de-struct--resuelto)
  - [3.75 `db.<c>.upsert(matchFn, insertValue, updateFn)` — RESUELTO](#375-dbcupsertmatchfn-insertvalue-updatefn--resuelto)
  - [3.76 `db.<c>.insertMany(items)` — RESUELTO](#376-dbcinsertmanyitems--resuelto)
  - [3.77 `createdAt`/`updatedAt` automáticos: `= now()` + `@autoUpdate` — RESUELTO](#377-createdatupdatedat-automáticos--now--autoupdate--resuelto)
  - [3.78 Soft-delete nativo: `@softDelete` — RESUELTO](#378-soft-delete-nativo-softdelete--resuelto)
  - [3.79 `linkc build --diff <archivo-anterior>` — RESUELTO](#379-linkc-build---diff-archivo-anterior--resuelto)
  - [3.80 Índices declarativos: `@index`/`@unique` — RESUELTO](#380-índices-declarativos-indexunique--resuelto)
  - [3.81 `--host <dirección>`: en qué interfaz escucha `linkc serve` — RESUELTO](#381---host-dirección-en-qué-interfaz-escucha-linkc-serve--resuelto)
  - [3.82 `linkc test --filter <nombre>` — RESUELTO](#382-linkc-test---filter-nombre--resuelto)
  - [3.83 `linkc --version` y versión estampada en cada archivo generado — RESUELTO](#383-linkc---version-y-versión-estampada-en-cada-archivo-generado--resuelto)
  - [3.84 `auth.destroyAllSessions(userId)`: revocar todas las sesiones de un usuario — RESUELTO](#384-authdestroyallsessionsuserid-revocar-todas-las-sesiones-de-un-usuario--resuelto)
  - [3.85 `--max-body-bytes <N>`: límite de tamaño del body de una request — RESUELTO](#385---max-body-bytes-n-límite-de-tamaño-del-body-de-una-request--resuelto)
  - [3.86 `--http-timeout <duración>`: timeout de llamadas salientes `http.*` — RESUELTO](#386---http-timeout-duración-timeout-de-llamadas-salientes-http--resuelto)
  - [3.87 `/health` verifica conectividad real a la base — RESUELTO](#387-health-verifica-conectividad-real-a-la-base--resuelto)
  - [3.88 Lint: comparación insegura de un secreto con `==` — RESUELTO](#388-lint-comparación-insegura-de-un-secreto-con----resuelto)
  - [3.89 `--trust-proxy`: `@rate_limit` detrás de un proxy real — RESUELTO](#389---trust-proxy-rate_limit-detrás-de-un-proxy-real--resuelto)
  - [3.90 `dateFromParts(...)`: construir un `Timestamp` arbitrario — RESUELTO](#390-datefromparts-construir-un-timestamp-arbitrario--resuelto)
  - [3.91 `Timestamp` decodifica `date`/`timestamp`/`timestamptz` nativos de Postgres — RESUELTO](#391-timestamp-decodifica-datetimestamptimestamptz-nativos-de-postgres--resuelto)
  - [3.92 `linkc serve-all` + `--restart-backoff`: un proceso para varios servicios — RESUELTO](#392-linkc-serve-all----restart-backoff-un-proceso-para-varios-servicios--resuelto)
  - [3.93 `--service-api-key`: autenticación servidor-a-servidor — RESUELTO](#393---service-api-key-autenticación-servidor-a-servidor--resuelto)
  - [3.94 Aviso de colisión de nombre de tabla en PostgreSQL — RESUELTO](#394-aviso-de-colisión-de-nombre-de-tabla-en-postgresql--resuelto)
  - [3.95 `countWhere` + `findWhere` empujados a SQL para `x.campo == valor` — RESUELTO](#395-countwhere--findwhere-empujados-a-sql-para-xcampo--valor--resuelto)
  - [3.96 `@check(...)`: constraints numéricos de nivel de base — RESUELTO](#396-check-constraints-numéricos-de-nivel-de-base--resuelto)
  - [3.97 `linkc migrate --dry-run` — RESUELTO](#397-linkc-migrate---dry-run--resuelto)
  - [3.98 Lint `hardcoded-secret-literal` — RESUELTO](#398-lint-hardcoded-secret-literal--resuelto)
  - [3.99 `linkc test --db <url-postgres>` — RESUELTO](#399-linkc-test---db-url-postgres--resuelto)
  - [3.100 `linkc doctor`: diagnóstico de entorno antes de un despliegue — RESUELTO](#3100-linkc-doctor-diagnóstico-de-entorno-antes-de-un-despliegue--resuelto)
  - [3.101 `List<Int>.sum() -> Int` — RESUELTO, alcance acotado](#3101-listintsum---int--resuelto-alcance-acotado)
  - [3.102 `db.<c>.maxRow(selector)`/`minRow(selector) -> T?` — RESUELTO](#3102-dbcmaxrowselectorminrowselector---t--resuelto)
  - [3.103 `Float` decodifica `numeric`/`decimal` nativo de Postgres — RESUELTO](#3103-float-decodifica-numericdecimal-nativo-de-postgres--resuelto)
  - [3.104 Escribir un `Int` contra una columna Postgres no-`BIGINT` (`SERIAL`/`SMALLINT`) — RESUELTO](#3104-escribir-un-int-contra-una-columna-postgres-no-bigint-serialsmallint--resuelto)
  - [3.105 `db.<c>.increment(id, selector, delta) -> T` — RESUELTO, alcance acotado](#3105-dbcincrementid-selector-delta---t--resuelto-alcance-acotado)
  - [3.106 Lint `delete-then-insert-same-id` — RESUELTO](#3106-lint-delete-then-insert-same-id--resuelto)
  - [3.107 `linkc serve-all --port-map-out <archivo.json>` — RESUELTO](#3107-linkc-serve-all---port-map-out-archivojson--resuelto)
  - [3.108 `countWhere`/`findWhere` empujan a SQL `!=`/`<`/`<=`/`>`/`>=` — RESUELTO, alcance acotado](#3108-countwherefindwhere-empujan-a-sql--------resuelto-alcance-acotado)
  - [3.109 `countWhere`/`findWhere` empujan una conjunción `&&` de varias hojas — RESUELTO, alcance acotado](#3109-countwherefindwhere-empujan-una-conjunción--de-varias-hojas--resuelto-alcance-acotado)
  - [3.110 `crypto.awsS3PresignedUrl(...)`: URLs firmadas reales para Amazon S3 — RESUELTO, alcance acotado](#3110-cryptoawss3presignedurl-urls-firmadas-reales-para-amazon-s3--resuelto-alcance-acotado)
  - [3.111 `response.redirect(url, permanent)`: redirects HTTP reales — RESUELTO](#3111-responseredirecturl-permanent-redirects-http-reales--resuelto)
  - [3.112 `base64.encode`/`base64.decode` — YA EXISTÍA, sin documentar ni probar hasta ahora](#3112-base64encodebase64decode--ya-existía-sin-documentar-ni-probar-hasta-ahora)
  - [3.113 `@cache_control("...")` por rpc — RESUELTO](#3113-cache_control-por-rpc--resuelto)
  - [3.114 Flujo OAuth2 "client credentials" (servidor a servidor) — YA FUNCIONABA, sin un ejemplo que lo dijera](#3114-flujo-oauth2-client-credentials-servidor-a-servidor--ya-funcionaba-sin-un-ejemplo-que-lo-dijera)
  - [3.115 Lint `unused-var`: 14 falsos positivos dentro de closures y struct-literals — RESUELTO](#3115-lint-unused-var-14-falsos-positivos-dentro-de-closures-y-struct-literals--resuelto)
  - [3.116 `sitemapXml`/`robotsTxt`: builtins declarativos para SEO — RESUELTO](#3116-sitemapxmlrobotstxt-builtins-declarativos-para-seo--resuelto)
  - [3.117 `metaTags`/`openGraphTags`/`canonicalLink`/`jsonLd`: metadata SEO clásica como helpers de `String` — RESUELTO](#3117-metatagsopengraphtagscanonicallinkjsonld-metadata-seo-clásica-como-helpers-de-string--resuelto)
  - [3.118 `llms.txt` auto-generado por proyecto — RESUELTO](#3118-llmstxt-auto-generado-por-proyecto--resuelto)
  - [3.119 `@example(request: ..., response: ...)`: ejemplos tipados en `openapi.json` — RESUELTO](#3119-examplerequest-response-ejemplos-tipados-en-openapijson--resuelto)
  - [3.120 `linkc systemd`: generador de unidad systemd — RESUELTO](#3120-linkc-systemd-generador-de-unidad-systemd--resuelto)
  - [3.121 `linkc pm2-config`: generador de configuración PM2 — RESUELTO](#3121-linkc-pm2-config-generador-de-configuración-pm2--resuelto)
  - [3.122 `--log-format`/`--log-level`: logging estructurado JSON y nivel configurable — RESUELTO](#3122---log-format---log-level-logging-estructurado-json-y-nivel-configurable--resuelto)
  - [3.123 Hooks de React generados: guarda contra respuestas fuera de orden — RESUELTO](#3123-hooks-de-react-generados-guarda-contra-respuestas-fuera-de-orden--resuelto)
  - [3.124 Hooks de React generados: cache compartido entre instancias — RESUELTO](#3124-hooks-de-react-generados-cache-compartido-entre-instancias--resuelto)
  - [3.125 Hooks de React generados: invalidación de cache tras una Mutation — RESUELTO](#3125-hooks-de-react-generados-invalidación-de-cache-tras-una-mutation--resuelto)
  - [3.126 `LinkTransportError`: el status HTTP viaja tipado, no solo en el mensaje — RESUELTO](#3126-linktransporterror-el-status-http-viaja-tipado-no-solo-en-el-mensaje--resuelto)
  - [3.127 Hooks de React generados: `loading` vs `isFetching` — RESUELTO](#3127-hooks-de-react-generados-loading-vs-isfetching--resuelto)
  - [3.128 Hooks de React generados: `mutate` vs `mutateAsync` — RESUELTO](#3128-hooks-de-react-generados-mutate-vs-mutateasync--resuelto)
  - [3.129 `client.ts`: cancelar una request con `AbortSignal` — RESUELTO](#3129-clientts-cancelar-una-request-con-abortsignal--resuelto)
  - [3.130 Hook de `stream`: `reconnect()` manual — RESUELTO](#3130-hook-de-stream-reconnect-manual--resuelto)
  - [3.131 `isOk`/`isErr` y el schema Zod de `Result<T,E>` chequeaban un campo que no existe — RESUELTO (bug real)](#3131-isokiserr-y-el-schema-zod-de-resultte-chequeaban-un-campo-que-no-existe--resuelto-bug-real)
  - [3.132 Schema Zod de un enum ADT: `z.enum([...])` no alcanzaba — RESUELTO (bug real)](#3132-schema-zod-de-un-enum-adt-zenum-no-alcanzaba--resuelto-bug-real)
  - [3.133 `openapi.json`: mismos tres bugs que `isOk`/`isErr` y el schema Zod, esta vez en la especificación pública de la API — RESUELTO (bug real)](#3133-openapijson-mismos-tres-bugs-que-isokiserr-y-el-schema-zod-esta-vez-en-la-especificación-pública-de-la-api--resuelto-bug-real)
  - [3.134 `@infinite(cursor, limit)`: scroll infinito real — RESUELTO](#3134-infinitecursor-limit-scroll-infinito-real--resuelto)
  - [3.135 Cache de Query: aislado por instancia de `client`, no solo por rpc+parámetros — RESUELTO](#3135-cache-de-query-aislado-por-instancia-de-client-no-solo-por-rpcparámetros--resuelto)
  - [3.136 `AbortSignal` real dentro de los hooks (Query reference-counted, Mutation explícito) — RESUELTO](#3136-abortsignal-real-dentro-de-los-hooks-query-reference-counted-mutation-explícito--resuelto)
  - [3.137 Mutaciones optimistas: `optimisticData` con rollback automático — RESUELTO](#3137-mutaciones-optimistas-optimisticdata-con-rollback-automático--resuelto)
  - [3.138 Cache de Infinite compartido entre instancias — RESUELTO](#3138-cache-de-infinite-compartido-entre-instancias--resuelto)
  - [3.139 `llms-full.txt`: la mitad expandida de la convención llmstxt.org — RESUELTO](#3139-llms-fulltxt-la-mitad-expandida-de-la-convención-llmstxtorg--resuelto)
  - [3.140 `@idempotent`: idempotency keys nativas en rpcs de escritura — RESUELTO](#3140-idempotent-idempotency-keys-nativas-en-rpcs-de-escritura--resuelto)
  - [3.141 `smtp.sendMessage`: cc/bcc y adjuntos reales — RESUELTO](#3141-smtpsendmessage-ccbcc-y-adjuntos-reales--resuelto)
  - [3.142 `@rate_limit(..., key: <param>)`: una clave adicional a la IP — RESUELTO](#3142-rate_limit-key-param-una-clave-adicional-a-la-ip--resuelto)
  - [3.143 `--hsts`: `Strict-Transport-Security` opt-in — RESUELTO](#3143---hsts-strict-transport-security-opt-in--resuelto)
  - [3.144 `@cache("60s")`: cache de resultado del lado del servidor — RESUELTO](#3144-cache60s-cache-de-resultado-del-lado-del-servidor--resuelto)
  - [3.145 `deleteWhere` empuja la SELECCIÓN a SQL — RESUELTO](#3145-deletewhere-empuja-la-selección-a-sql--resuelto)
  - [3.146 `@check(minLength/maxLength, N)`: constraints de longitud sobre `String` — RESUELTO](#3146-checkminlengthmaxlength-n-constraints-de-longitud-sobre-string--resuelto)
  - [3.147 `@cors("...")`: override de CORS por ruta — RESUELTO](#3147-cors-override-de-cors-por-ruta--resuelto)
  - [3.148 Log de auditoría de autorización estructurado — RESUELTO](#3148-log-de-auditoría-de-autorización-estructurado--resuelto)
  - [3.149 `GET /metrics` en formato Prometheus — RESUELTO](#3149-get-metrics-en-formato-prometheus--resuelto)
  - [3.150 Latencia de propagación NOTIFY + cola de reintento acotada — RESUELTO](#3150-latencia-de-propagación-notify--cola-de-reintento-acotada--resuelto)
  - [3.151 `db.vacuum()`/`db.tableStats()`: RPCs de administración — RESUELTO](#3151-dbvacuumdbtablestats-rpcs-de-administración--resuelto)
  - [3.152 Bloqueo de cuenta configurable — RESUELTO](#3152-bloqueo-de-cuenta-configurable--resuelto)
  - [3.153 `linkc serve-all --port-registry <archivo.json>`: puerto estable por nombre de servicio — RESUELTO](#3153-linkc-serve-all---port-registry-archivojson-puerto-estable-por-nombre-de-servicio--resuelto)
  - [3.154 `transaction { ... }`: transacciones SQL multi-escritura — RESUELTO, alcance acotado](#3154-transaction--transacciones-sql-multi-escritura--resuelto-alcance-acotado)
  - [3.155 `@unique(campo1, campo2, ...)`: constraint UNIQUE compuesto a nivel de `type` — RESUELTO](#3155-uniquecampo1-campo2--constraint-unique-compuesto-a-nivel-de-type--resuelto)
  - [3.156 `Int64` como `bigint` real en `client.ts` — RESUELTO, cierra el límite que dejaba abierto §3.30](#3156-int64-como-bigint-real-en-clientts--resuelto-cierra-el-límite-que-dejaba-abierto-330)
  - [3.157 `.truncateToDay()`/`.truncateToMonth()`/`.truncateToYear()`: agregación agrupada por fecha — RESUELTO, cierra el límite que dejaba abierto §3.65](#3157-truncatetodaytruncatetomonthtruncatetoyear-agregación-agrupada-por-fecha--resuelto-cierra-el-límite-que-dejaba-abierto-365)
  - [3.158 `linkc serve`: un hilo por request — RESUELTO, Etapa 1 de un roadmap de concurrencia mayor](#3158-linkc-serve-un-hilo-por-request--resuelto-etapa-1-de-un-roadmap-de-concurrencia-mayor)
  - [3.159 `@cron("Ns"/"Nm"/"Nh"/"Nd")`: tareas recurrentes nativas dentro de `linkc serve` — RESUELTO](#3159-cronnsnmnhnd-tareas-recurrentes-nativas-dentro-de-linkc-serve--resuelto)
  - [3.160 `http.postWithRetry(url, body, headers, maxAttempts)`: reintentos con backoff para webhooks salientes — RESUELTO](#3160-httppostwithretryurl-body-headers-maxattempts-reintentos-con-backoff-para-webhooks-salientes--resuelto)
  - [3.161 `import "./modulo.link";`: import "solo por efecto" — RESUELTO, cierra el último hueco real para partir un programa en módulos](#3161-importmodulolink-import-solo-por-efecto--resuelto-cierra-el-último-hueco-real-para-partir-un-programa-en-módulos)
  - [3.162 Segunda auditoría adversarial: 3 bugs reales, dos de ellos creados por los fixes de la ronda anterior — RESUELTOS](#3162-segunda-auditoría-adversarial-3-bugs-reales-dos-de-ellos-creados-por-los-fixes-de-la-ronda-anterior--resueltos)
  - [3.163 `catch_unwind` alrededor del cuerpo de `transaction { }` — RESUELTO, cierra el primer límite honesto de §3.162](#3163-catch_unwind-alrededor-del-cuerpo-de-transaction----resuelto-cierra-el-primer-límite-honesto-de-§3162)
  - [3.164 `catch_unwind` alrededor de cada corrida de `@cron` — RESUELTO, cierra el segundo límite honesto de §3.162](#3164-catch_unwind-alrededor-de-cada-corrida-de-cron--resuelto-cierra-el-segundo-límite-honesto-de-§3162)
  - [3.165 Tercera auditoría adversarial (27/08/2026): 2 bugs críticos — RESUELTOS](#3165-tercera-auditoría-adversarial-27082026-2-bugs-críticos--resueltos)
  - [3.166 `Patch<T>`/`applyPatch` ahora corre `@validate`/`@check` — RESUELTO, cierra el hallazgo #3 de AUDIT-2026-08-27.md](#3166-patchtapplypatch-ahora-corre-validatecheck--resuelto-cierra-el-hallazgo-3-de-audit-2026-08-27md)
  - [3.167 `@idempotent`: la carrera de doble ejecución concurrente — RESUELTO, cierra el hallazgo #4 de AUDIT-2026-08-27.md](#3167-idempotent-la-carrera-de-doble-ejecución-concurrente--resuelto-cierra-el-hallazgo-4-de-audit-2026-08-27md)
  - [3.168 Ronda 3 de AUDIT-FIX-PLAN-2026-08-27.md: 6 bugs de severidad media — RESUELTOS](#3168-ronda-3-de-audit-fix-plan-2026-08-27md-6-bugs-de-severidad-media--resueltos)
  - [3.169 Ronda 4 de AUDIT-FIX-PLAN-2026-08-27.md: los 6 hallazgos restantes — CERRADA (3 código, 3 documentación deliberada)](#3169-ronda-4-de-audit-fix-plan-2026-08-27md-los-6-hallazgos-restantes--cerrada-3-código-3-documentación-deliberada)
  - [3.170 `countWhere`/`findWhere`/`deleteWhere`/`upsert` empujan `||` combinando condiciones — RESUELTO, cierra PLAN.md §9.3 ítem 1](#3170-countwherefindwheredeletewhereupsert-empujan--combinando-condiciones--resuelto-cierra-planmd-93-ítem-1)
  - [3.171 `countWhere`/`findWhere`/`deleteWhere` empujan comparaciones campo-vs-campo (`item.endDate > item.startDate`) — RESUELTO, cierra el resto de PLAN.md §9.3 ítem 1](#3171-countwherefindwheredeletewhere-empujan-comparaciones-campo-vs-campo-itemenddate--itemstartdate--resuelto-cierra-el-resto-de-planmd-93-ítem-1)
  - [3.172 Varios `db { ... }`, uno por módulo, se fusionan en un solo namespace de colecciones — RESUELTO, cierra el último hueco de §3.161 (Pilar 3 del roadmap de skynet-d3)](#3172-varios-db--uno-por-módulo-se-fusionan-en-un-solo-namespace-de-colecciones--resuelto-cierra-el-último-hueco-de-3161-pilar-3-del-roadmap-de-skynet-d3)
  - [3.173 `@check(<expr>)` a nivel de `type` — RESUELTO, cierra la mitad "expresión booleana arbitraria" que §3.96 había dejado pendiente](#3173-checkexpr-a-nivel-de-type--resuelto-cierra-la-mitad-expresión-booleana-arbitraria-que-396-había-dejado-pendiente)
  - [3.174 `@unique(...) where <expr>`: la mitad CONDICIONAL de §3.155 — RESUELTO](#3174-uniquewhereexpr-la-mitad-condicional-de-3155--resuelto)
  - [3.175 `linkc db inspect`: primera pieza de la suite de administración de datos — RESUELTO PARCIAL](#3175-linkc-db-inspect-primera-pieza-de-la-suite-de-administración-de-datos--resuelto-parcial)
  - [3.176 Reporte de adopción de iaacademy: `linkc introspect` avisa sobre una PK `id` no entera, `linkc doctor --target-url` detecta deriva de versión — RESUELTO PARCIAL](#3176-reporte-de-adopción-de-iaacademy-linkc-introspect-avisa-sobre-una-pk-id-no-entera-linkc-doctor---target-url-detecta-deriva-de-versión--resuelto-parcial)
  - [3.177 `id: Uuid` como PK alternativa — RESUELTO, cierra el bloqueo real de iaacademy que §3.176 dejó pendiente](#3177-id-uuid-como-pk-alternativa--resuelto-cierra-el-bloqueo-real-de-iaacademy-que-3176-dejó-pendiente)
  - [3.178 `@rate_limit` distribuido vía Postgres — RESUELTO](#3178-rate_limit-distribuido-vía-postgres--resuelto)
  - [3.179 `String` contra `uuid`/`inet`/`cidr` NATIVOS de Postgres — RESUELTO](#3179-string-contra-uuidinetcidr-nativos-de-postgres--resuelto)
  - [3.180 Compresión GZIP de la respuesta HTTP — RESUELTO](#3180-compresión-gzip-de-la-respuesta-http--resuelto)
  - [3.181 Camino de despliegue recomendado (git+CI) — RESUELTO, alcance acotado](#3181-camino-de-despliegue-recomendado-gitci--resuelto-alcance-acotado)
  - [3.182 Escritura de `Timestamp` contra `date`/`timestamp`/`timestamptz` NATIVOS de Postgres — RESUELTO](#3182-escritura-de-timestamp-contra-datetimestamptimestamptz-nativos-de-postgres--resuelto)
  - [3.183 `link.lock` como pin real de dependencias git + locking entre procesos — RESUELTO](#3183-linklock-como-pin-real-de-dependencias-git--locking-entre-procesos--resuelto)
  - [3.184 `Decimal`: tipo numérico de precisión exacta (punto fijo, 4 decimales) — RESUELTO](#3184-decimal-tipo-numérico-de-precisión-exacta-punto-fijo-4-decimales--resuelto)
  - [3.185 `linkc db export`/`linkc db import` — RESUELTO PARCIAL](#3185-linkc-db-exportlinkc-db-import--resuelto-parcial)
  - [3.186 `builtin_args!`: fast-path para curar un builtin nuevo — RESUELTO (tooling interno, no una feature del lenguaje)](#3186-builtin_args-fast-path-para-curar-un-builtin-nuevo--resuelto-tooling-interno-no-una-feature-del-lenguaje)
  - [3.187 `String` contra `json`/`jsonb` NATIVOS de Postgres — RESUELTO](#3187-string-contra-jsonjsonb-nativos-de-postgres--resuelto)
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
             | "test" | "transaction" ;
```

**Reservado pero fuera del v0 de la gramática:** `async`, `await`, `trait`, `impl` — el modelo de concurrencia y de polimorfismo ad-hoc se diseña en una iteración posterior (ver PLAN.md §4, Fase 1). `for`, `in`, `break`, `continue` — v0 de loops (§3.15) es solo `while`; ninguno de estos cuatro es todavía una palabra reservada de verdad (no aparecen en `keyword_from_str`, `compiler/src/token.rs`), esto es prosa preparatoria, no una reserva real.

---

## 2. Gramática Sintáctica

### 2.1 Programa e ítems de nivel superior

```ebnf
program      = { item } ;
item         = import_decl | type_decl | enum_decl | service_decl | const_decl | fn_decl | db_decl | test_decl ;

import_decl  = "import" , ( "{" , ident_list , "}" , "from" , string_lit
                          | string_lit )                        (* §3.161 *)
             , ";" ;
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

  **Resolución fresca la primera vez, o cuando `link.json` cambia el `url`/`rev` de una dependencia -- pin real (`link.lock`) en cualquier otro caso.** Ver GRAMMAR.md §3.183 para el detalle completo: `link.lock` pasó de ser puramente informativo a un pin de verdad, con `--update-deps` como el único camino que lo avanza a propósito.

  **`link.lock` graba el commit resuelto Y actúa como pin real (§3.183).** Un campo, `git_dependencies` (`{"nombre":{"url":...,"rev":...,"resolved":"<sha-completo>"}}`), registra exactamente qué commit se usó -- y, a diferencia del comportamiento original de esta sección, SÍ se lee para decidir qué commit usar en el PRÓXIMO build (mientras `link.json` no le haya cambiado el `url`/`rev` a esa dependencia): el mismo contrato de reproducibilidad que `Cargo.lock`/`package-lock.json`.

  **Locking entre procesos concurrentes -- RESUELTO (§3.183).** Dos `linkc build`/`serve` corriendo a la vez sobre el mismo proyecto ya no pueden pisarse el mismo clon cacheado -- un lock advisory basado en archivo (`CacheLock`, `gitdep.rs`) serializa el acceso al directorio de caché de cada dependencia.

  **`link.lock` para archivos LOCALES -- RESUELTO, pero sigue sin ser un lockfile de versiones.** Con una dependencia por RUTA local no hay versión ni conflicto que "lockear" en el sentido de Cargo/npm — ese razonamiento original sigue valiendo para ESE caso. Lo que se agregó primero (`compiler/src/lockfile.rs`) es más angosto: `linkc build` calcula un hash SHA-256 de cada archivo `.link` tocado (`touched`, el mismo `Vec<PathBuf>` que ya devuelve `load_program`) y lo escribe en `link.lock` (JSON, `{"version":1,"entries":{"ruta":{"path":...,"hash":...}},"git_dependencies":{...}}`); en el PRÓXIMO `build`, si ya existe un `link.lock`, se compara antes de sobreescribirlo y cualquier archivo cuyo hash no matchea imprime una advertencia — detección de deriva entre builds para archivos locales, pin real (§3.183) para dependencias git. Rutas siempre relativas a la raíz del proyecto (nunca el `\\?\C:\...` que `fs::canonicalize` da en Windows) para que el archivo sea legible y portable entre máquinas -- el mismo problema de prefijo apareció de nuevo al pasarle una ruta de caché a `git clone` como argumento (git no lo entiende como argumento de línea de comandos, "Invalid argument"; `display_path`, la función que ya pelaba esto para texto legible, resultó ser exactamente la función correcta acá también, por una razón distinta y más dura que la estética original).

  Verificado con subprocesos reales: `gitdep::resolve` contra un repo git LOCAL como "remoto" (clon inicial, reutilización de caché sin red, fetch de un tag agregado después del clon inicial, checkout de un commit SHA directo) y `linkc build` de punta a punta (clona, resuelve el import, tipa, genera el contrato, y graba el commit real en `link.lock`). 371 tests, todos pasando.

  **Cobertura agregada en un reparso posterior: los caminos de FALLA, no solo el feliz.** Hasta entonces, ni `gitdep.rs` ni el test a nivel CLI probaban qué pasa cuando algo sale mal -- un rev que no existe en el remoto, un remoto inalcanzable. Dos tests nuevos en `gitdep.rs` confirman que `resolve` falla ruidoso (`Err` con un mensaje real) en ambos casos, contra el mismo `FixtureRemote` local que ya usan los tests del camino feliz. Un tercer test, a nivel `linkc build` completo, cierra la capa que un test unitario de `gitdep.rs` solo no puede cubrir: el cableado real entre `resolve()` fallando y lo que el BINARIO hace con eso (`modules.rs` envolviendo el error, `main.rs` decidiendo el exit code y si escribe `link.lock`) -- confirma que un rev inexistente tumba `linkc build` con código de salida distinto de cero y sin dejar un `link.lock` a medio escribir, no algo que un test unitario aislado garantice por sí solo si esa cadena llegara a romperse. 380 tests, todos pasando.
- **Ciclos se rechazan con un error claro** (no un stack overflow silencioso ni un colgado): se detectan sobre la pila de imports que se está resolviendo en ese momento, no sobre "todo lo que ya se vio alguna vez" (eso rompería el caso diamante de abajo).
- **Sin re-exports, a propósito.** Un import se valida contra los ítems NATIVOS del archivo importado — nunca contra su cierre ya fusionado con SUS PROPIOS imports. Si A importa `X` de B, y B a su vez importa `X` de C (pero no declara `X` él mismo), el import de A **falla**: B nunca declaró `X` nativamente, así que no hay nada que A pueda "heredar" a través de B. Si hiciera falta lo contrario, hay que importar `X` directamente de C.
- **Namespaces cruzados.** `types`/`enums`/`fns`/`const`s son namespaces independientes (el checker los guarda en tablas separadas) — un import busca el nombre en los cuatro y alcanza con que matchee en uno; error solo si no matchea en ninguno. `service` queda afuera: no es algo que se referencie por nombre en ningún otro lado del lenguaje, así que "importar un service" por nombre no tiene un significado real. **Para cargar un módulo POR su `service` existe la forma "solo por efecto" (`import "./billing.link";`, §3.161)** — el mecanismo que cierra ese hueco sin inventarle al `service` una semántica de nombre que no tiene.
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

expr         = match_expr | if_expr | transaction_expr | or_expr ;

if_expr      = "if" , or_expr , block , "else" , ( if_expr | block ) ;

transaction_expr = "transaction" , block ;   (* §3.154 -- de modo chequeo nada más, igual que if_expr/match_expr *)

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
  if (!res.ok) throw new LinkTransportError(`HTTP ${res.status}`, res.status);
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

**Métodos:** `all() -> T[]`, `find(id: Int) -> T?`, `insert(x: Omit<T,"id">) -> T`, `insertMany(items: Omit<T,"id">[]) -> T[]` (§3.76), `applyPatch(id: Int, p: Patch<T>) -> T`, `findWhere(f: (T) -> Bool) -> T[]`, `deleteWhere(f: (T) -> Bool) -> Int`, `count() -> Int`, `page(limit: Int, offset: Int) -> T[]`, `pageAfter(cursor: Int?, limit: Int) -> T[]`, `upsert(matchFn: (T) -> Bool, insertValue: Omit<T,"id">, updateFn: (T) -> Omit<T,"id">) -> T` (§3.75), `sumBy`/`countBy`/`avgBy`/`maxBy`/`minBy`, `maxRow`/`minRow` (§3.102), `increment` (§3.105) — resueltos contra el tipo de elemento de verdad (`Type::DbCollection`, checker.rs). Un nombre de colección o de método desconocido ya es un error del checker (`db.usres.fnid(1)`, con AMBOS typo'd, se rechaza en tiempo de chequeo), no algo que se descubre recién en runtime.

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

**Sin `for`, a propósito.** No existe ningún concepto de rango/iterador en el lenguaje (`.take`/`.filter`/`.map`/`.length`/`.join`/`.reverse`, más `.sum()` sobre `List<Int>` desde §3.101, siguen siendo los únicos métodos de `List`, sin `.reduce()`/`.forEach()` genéricos); todo lo que `for` daría ya es expresable con `while` + indexado manual (`arr[i]`, que ya existía). Agregarlo antes de que `while` se haya usado en programas reales sería azúcar prematuro — mismo criterio que ya dejó afuera closures de 0 parámetros y roles múltiples en `@requires`.

**Sin `break`/`continue`, a propósito.** Implementarlos bien primero necesita resolver el hallazgo de abajo (un `break` anidado dentro de un `if`/`match` fallaría en silencio por la misma razón estructural que `return` ya falla) — deferido a una ronda futura si el uso real lo pide; la recursión sigue disponible mientras tanto para loops con salida temprana.

**`return` dentro de un cuerpo de `while` se RECHAZA explícitamente en el checker — no es una limitación caprichosa, evita heredar un bug real y ya existente.** Encontrado leyendo el código vecino al diseñar esto, no introducido por esta ronda: un `return` anidado dentro de un `if`/`match` usado COMO SENTENCIA (no cola) no solo tipa mal hoy (se chequea contra `Void` en vez del tipo real de retorno, por cómo `check_stmt` trata `if`/`match`-como-sentencia) sino que en RUNTIME es un no-op silencioso — `eval_block` descarta el valor que produce ese `if`/`match` (incluido cualquier `return` de adentro, que solo corta el `eval_block` INTERNO de esa rama, no el que la contiene) y sigue con la sentencia siguiente como si nada. Ya es explotable hoy con un `return;` desnudo en una función `Void`. En vez de reescribir el mecanismo de señalización de control de flujo entero (un cambio mucho más grande y riesgoso que agregar un loop), `while` simplemente no deja usar `return` en su cuerpo — sacá el valor final con una variable `mut` declarada antes del loop y un tail después, como en el ejemplo de arriba. El bug preexistente en `if`/`match`-como-sentencia queda documentado pero sin arreglar, fuera de alcance de esta ronda.

**Cota dura de iteraciones (`MAX_WHILE_ITERATIONS = 1_000_000`, `runtime/mod.rs`) — no opcional, agregada en la MISMA ronda que el loop.** El servidor (`server.rs::serve`) era, al momento de agregar esta cota, un loop estrictamente single-threaded sin timeout ni scheduling cooperativo: un `while true { }` (o cualquier condición que el programa nunca vuelve falsa) congelaría PARA SIEMPRE el único hilo que atiende TODAS las requests, no solo la que lo disparó. Esto no era un límite v0 "honesto" en el mismo espíritu que otros (ej. "sin CSPRNG auditado") — era un footgun nuevo que la propia feature introducía, y este proyecto ya encontró y arregló footguns reales de ese calibre por review adversarial (el generador de tokens y `destroySession`, §3.14). **Actualizado (26/08/2026, GRAMMAR.md §3.158): desde que `linkc serve` corre un hilo real por request, un `while true { }` sin esta cota solo colgaría el hilo de ESA request, no el proceso entero -- pero la cota sigue existiendo y sigue sin ser opcional**, porque un hilo colgado para siempre todavía es un leak real (nunca termina, ocupa memoria/stack indefinidamente, y bajo carga sostenida agotaría los recursos del proceso igual, solo que más despacio que "todo el servidor congelado de una". La cota es deliberadamente generosa y NO configurable: un backstop contra el bug/loop-infinito más común, no un sistema fino de cuotas de recursos. Se cuenta una vez por invocación de rpc/fn (un `Cell<u64>` enhebrado por todo el árbol de evaluación, incluidos loops anidados y loops dentro de una fn/closure llamada desde el cuerpo), así que partir un loop grande en muchos chicos no lo esquiva.

**Bug real, encontrado y arreglado el 26/08/2026: TODO `while` que corriera al menos una vez dentro de un `test { }` fallaba de entrada.** `run_tests_core` (el runner de `linkc test`) inicializaba ese mismo `Cell<u64>` compartido en `Cell::new(1_000_000)` -- el propio valor de `MAX_WHILE_ITERATIONS` -- en vez de `Cell::new(0)`, que es como lo inicializa correctamente el camino normal de `rpc` (`invoke_rpc_with_sessions`). Efecto: la primerísima vuelta de CUALQUIER `while` dentro de un test ya empujaba el contador a `1_000_001`, disparando el mensaje "límite de 1000000 iteraciones excedido -- posible loop infinito" de inmediato, sin que el loop hubiera dado ni una vuelta real -- el propio ejemplo canónico de esta sección (`sum(xs)`) fallaba siempre que se lo llamaba desde un `test`, funcionando perfecto desde `serve`. Encontrado verificando un reporte externo de un `while` "colgado" solo bajo `linkc test`; reproducido con el ejemplo mínimo de arriba antes de tocar una línea de código, siguiendo la misma disciplina de este documento (nunca asumir sin correr el binario real). Fix de una línea (`Cell::new(0u64)`); test de regresión en `tests/cli_test_runner.rs` que cubre tanto el caso corto (pasa) como un `while true` genuino (sigue cortando).

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

**`Db` gana un registro de suscriptores; `subscribe()` hace snapshot+registro bajo un candado compartido con la entrega.** `Db::subscribe(collection)` devuelve `(snapshot, Receiver)`: `snapshot` es el estado actual de la colección ya serializado a JSON (mismo `value_to_json` que cualquier respuesta normal), y `Receiver` es el lado de lectura de un `mpsc::sync_channel(1024)` recién registrado.

**Actualizado (26/08/2026, GRAMMAR.md §3.158): el argumento original de esta sección ("el single-threading del servidor ES el lock del pub-sub, no algo aparte que hubo que agregar") dejó de ser cierto el día que `linkc serve` pasó a un hilo real por request -- y el propio texto de esta sección, en su momento, ya avisaba que había que revisarlo primero.** Bug real, encontrado auditando esta sección (no reportado externamente): con el orden original ("sacar la foto, DESPUÉS registrarse", dos pasos sin candado compartido con `publish`/`deliver_local`), un `insert`/`applyPatch` de OTRO hilo podía commitear y publicar EXACTAMENTE en la ventana entre esas dos líneas -- la fila nueva no quedaba en la foto (ya tomada) ni llegaba por el canal (todavía sin registrar): **una fila perdida en silencio**, sin ningún error visible para el suscriptor ni para quien escribió el `.link`. Fix: `subscribe()` ahora registra el sender Y saca la foto bajo el MISMO candado (`Db::subscribers`) que `deliver_local` usa para entregar -- exactamente la idea que el texto original anticipaba ("invirtiendo el orden a 'registrarse, después sacar la foto'"), más el candado compartido que hacía falta para que la inversión de orden realmente sirviera de algo. Costo aceptado: un duplicado OCASIONAL (la fila llega en la foto Y como evento, en la ventana angosta donde las dos operaciones coinciden) en vez de una fila perdida -- un consumidor de `stream` ya trata cada evento como el estado ACTUAL de esa fila, no como un delta, así que un duplicado es inofensivo; una fila perdida no lo es.

**Detalle no obvio, la razón real por la que este fix no fue mecánico:** sostener el candado de `subscribers` durante `select_rows` (una llamada a la base) invierte el orden de candados respecto al camino de `transaction{}` (§3.154), que sostiene el candado de la CONEXIÓN durante todo `BEGIN`+cuerpo+`COMMIT` -- si `commit_transaction` entregara sus eventos diferidos ahí adentro (como hacía antes de este mismo fix), un `transaction{}` confirmando y un `subscribe()` concurrente a la misma colección pedirían esos dos candados en órdenes opuestos, deadlock clásico. Se resolvió moviendo la entrega de eventos diferidos de `transaction{}` a DESPUÉS de soltar el candado de la conexión (`Expr::Transaction`, `runtime/mod.rs`) -- `commit_transaction` ahora DEVUELVE la lista de eventos pendientes en vez de entregarlos él mismo. Verificado con dos tests de hilos reales nuevos: `subscribing_concurrently_with_a_real_insert_never_loses_the_new_row` (300 vueltas con `std::sync::Barrier` forzando la carrera; falla de forma reproducible con el orden viejo, confirmado revirtiendo el fix temporalmente antes de restaurarlo) y `a_transaction_committing_concurrently_with_a_subscribe_on_the_same_collection_never_deadlocks` (100 vueltas; con el orden viejo de entrega el test literalmente SE CUELGA -- confirmado de la misma forma, matando el proceso colgado a mano tras 30s).

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

**`rusqlite` (SQLite embebido, feature `bundled`), no Postgres.** El intérprete corre sin ningún runtime async (`Value::Closure` guarda un `Env` con `Rc<RefCell<Value>>>`, ni `Send` ni `Sync` -- confirmado desde la ronda de closures, §3.10, y todavía cierto tras GRAMMAR.md §3.158/v1.114.0: cada request tiene su PROPIO hilo, pero DENTRO de ese hilo la evaluación sigue siendo estrictamente sincrónica) -- un driver async (`sqlx`, `tokio-postgres`) exigiría traer `tokio` entero, un cambio de arquitectura mucho más grande que esta ronda. `rusqlite` es sync-only por diseño, embebido (sin proceso de servidor externo corriendo aparte), y `bundled` compila su propio SQLite sin necesitar uno instalado en el sistema -- coherente con que `linkc serve` siga arrancando solo, mismo espíritu que ya tiene `tiny_http`. Postgres se descartó explícitamente por necesitar un servidor externo corriendo, rompiendo ese mismo espíritu.

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

**Límite honesto sobre limpieza al salir.** Sin manejo de señales explícito: `Command::spawn()` sin `CREATE_NEW_PROCESS_GROUP` deja al hijo en el mismo grupo de proceso/consola que el padre en ambas plataformas, así que un Ctrl+C real en una terminal interactiva le llega TAMBIÉN al hijo -- el camino verificado manualmente. Un kill programático dirigido SOLO al PID del proceso padre (no un Ctrl+C real desde una terminal) es el caso que sí puede dejar al hijo huérfano sirviendo el puerto -- límite de v0 conocido, no manejado.

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

**Wire format: string en ambas direcciones -- el tipo TS emitido, en cambio, ES `bigint` desde §3.156, no `string`. [ACTUALIZADO -- ver §3.156].** El wire siguió decidido en PLAN.md (string, "para no perder precisión") y NO cambió. Lo que sí cambió fue el lado TS: al momento de escribir este párrafo, `push_fetch_call`/`emit_client` (`ts_emit.rs`) no tenían ningún punto de conversión dirigido por tipo, así que emitir `bigint` de verdad hubiera necesitado "un walker recursivo dirigido por tipo nuevo, arquitectura nueva" -- exactamente lo que §3.156 terminó construyendo (`validators_emit.rs`, revividores `reviveX`), cuando apareció un caso concreto real (Glowapp) que lo pedía. Este párrafo queda como registro histórico de la decisión ORIGINAL y su motivo -- ver §3.156 para el diseño final.

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

**Sin construcción desde código fuente en v0 -- límite real, documentado, no un olvido. [ACTUALIZADO -- ver §3.32 y §3.90].** Al momento de escribir este párrafo, no había `now()`: el lenguaje no tenía NINGÚN mecanismo de función builtin sin receptor (`Expr::Call` solo reconocía una `fn` de usuario por nombre, o un método vía `receptor.metodo(...)`) -- agregar uno era territorio arquitectónico nuevo, no parte de "agregar un tipo". Ese límite quedó resuelto en dos pasos posteriores: §3.32 agregó `now()` (el instante actual), y §3.90 agregó `dateFromParts(...)` (una fecha/hora arbitraria construida a mano, para cálculos como "el primer día del trimestre fiscal"). Tampoco hay auto-stamping de una columna tipo `createdAt` al hacer `insert` en `db` -- ver `@autoUpdate` (GRAMMAR.md, sección de columnas automáticas) para ese caso, que sí quedó resuelto por separado. Un valor `Timestamp` ya no está limitado a llegar como parámetro de un `rpc` o a estar guardado en `db`: también se puede construir en código con `now()` o `dateFromParts(...)`.

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
- **Hashear ocupa CPU real por el tiempo que tarda.** Un `hashPassword` cuesta
  ~15 ms en la máquina donde se midió esto -- el precio correcto para un
  login. **Actualizado (26/08/2026, GRAMMAR.md §3.158): antes de esa ronda,
  `linkc serve` era estrictamente single-threaded y esto SÍ serializaba N
  logins simultáneos, uno detrás del otro, bloqueando el proceso entero
  mientras duraba. Con un hilo real por request, N logins concurrentes
  corren en paralelo de verdad (limitado por núcleos de CPU reales, no por
  el diseño del intérprete)** -- sigue siendo trabajo de CPU real, así que
  muchísimos logins a la vez en una máquina chica de todas formas compiten
  por el mismo recurso finito, pero ya no es una cola estrictamente
  serial de un solo servidor.
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
- **Una sola conexión, sin pool.** Cuando esto se escribió era deliberado, no
  pendiente: el servidor era estrictamente single-threaded y atendía una
  request a la vez, así que nunca había dos queries en vuelo al mismo tiempo
  -- un pool de más de una conexión no compraba nada (esperarían su turno
  igual, solo que en una cola distinta). **Actualizado (26/08/2026, GRAMMAR.md
  §3.158): con un hilo real por request, esa premisa ya no es cierta -- ahora
  SÍ puede haber varias queries queriendo correr a la vez, y todas se
  serializan igual sobre la MISMA conexión física (`Backend::execute`/`query`,
  candado breve por operación). Sigue siendo correcto -- no hay corrupción
  posible -- pero ya no es "sin costo": un pool de conexiones real dejaría de
  ser una optimización sin sentido para pasar a ser una ganancia de
  throughput genuina bajo carga concurrente. Sigue sin implementarse, ahora sí
  como límite honesto pendiente en vez de decisión cerrada -- ver "Límites
  honestos" de §3.158.** TLS y reconexión automática sí eran gaps reales de
  esta ronda -- resueltos en §3.40.
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
  - **La dilución en sí es silenciosa -- pero el rechazo real ya no lo es
    (26/08/2026).** `linkc_rate_limit_rejections_total{method="..."}` en
    `/metrics` (§3.149) cuenta cada `429` real por rpc -- no arregla la
    dilución entre réplicas (necesitaría estado compartido, ej. Redis o
    una tabla Postgres con incremento atómico, fuera de alcance de esta
    ronda), pero agregado entre réplicas en Prometheus (`sum by (method)
    (linkc_rate_limit_rejections_total)`) le da a quien opera la señal
    real de cuánto está rechazando el sistema en conjunto -- antes de
    esto, la única forma de notar que un `@rate_limit` dejó de proteger
    algo era ver el efecto (un endpoint caro golpeado sin control), nunca
    la causa.
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
no por un timer que la busque proactivamente -- cuando esto se escribió,
el intérprete no tenía ningún hilo de mantenimiento (single-threaded por
diseño, §3.13), así que inventar uno solo para esto hubiera sido la
primera excepción a ese modelo. Con un hilo real por request (§3.158) ya
no sería técnicamente "la primera excepción" -- pero la decisión de
diseño sigue siendo la misma: sin evidencia real de que la limpieza
perezosa haya sido un problema, agregar un hilo de barrido dedicado sería
complejidad nueva sin demanda que la pida. Costo real: una sesión creada y
nunca vuelta a usar después de expirar queda en memoria hasta que alguien
intente usarla (o el proceso reinicie) -- aceptable para v0, documentado
en vez de escondido.

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

**Límite honesto, documentado en el barrido de "límites honestos" (26/08/2026), REDUCIDO (no eliminado) por GRAMMAR.md §3.158 (v1.114.0, el mismo día): `crypto.hashPassword`/`verifyPassword` siguen corriendo SINCRÓNICAMENTE, ahora sobre el hilo de SU PROPIA request, no sobre "el único hilo del servidor".** El costo real de Argon2id (deliberado -- es justamente lo que lo hace resistente a fuerza bruta, GRAMMAR.md §3.34) es del orden de decenas de milisegundos con parámetros razonables. **Antes de §3.158**, `linkc serve` procesaba una request a la vez, así que un pico REAL de tráfico de `login`/`register` encolaba cada hash detrás del anterior, bloqueando el proceso ENTERO mientras tanto -- no solo `login`, CUALQUIER otro rpc, incluido `/health`. **Con un hilo real por request, eso ya no pasa**: N logins concurrentes hashean EN PARALELO, cada uno en su propio hilo -- otros rpcs (`/health` incluido) siguen respondiendo con normalidad durante un pico de logins. Lo que SIGUE siendo cierto, y por eso este límite queda "reducido" y no "cerrado": Argon2id es trabajo de CPU real, y un hilo por request no crea núcleos de CPU de la nada -- muchísimos logins concurrentes en una máquina con pocos núcleos siguen compitiendo por el mismo recurso finito, degradando la latencia de TODO lo demás que también necesita CPU (aunque sin el bloqueo total de antes), y siguen siendo, como vector de ataque DELIBERADO, tráfico barato de generar para forzar ese gasto de CPU real sin necesitar credenciales válidas ni explotar ningún bug -- el costo lo sigue pagando el servidor, por diseño de Argon2id.

**La mitigación sigue siendo la misma, y sigue recomendada: `@rate_limit("N/window", key: <param>)` (GRAMMAR.md §3.39/§3.142) sobre el `rpc` que llama a `hashPassword`/`verifyPassword`.** Acotar cuántos logins/registros por ventana de tiempo acepta el servidor acota, de forma directa, cuánta CPU puede pedir un atacante a la vez -- la defensa correcta contra el vector de ataque de arriba, independientemente del modelo de hilos por debajo. `--argon2-memory-kib`/`--argon2-iterations` más bajos (arriba) bajan el costo por hash a cambio de menos margen de seguridad -- un lever de operación, no un reemplazo de `@rate_limit`. Correr varios `linkc serve`/`serve-all` detrás de un balanceador (GRAMMAR.md §3.92) sigue acotando el radio de impacto por proceso -- ahora complementario a, no sustituto de, el paralelismo real que cada proceso individual ya tiene. Un pool de hilos DEDICADO solo a hashing (separado de los hilos de request, con su propio tope de concurrencia) seguiría siendo una mejora real por encima de esto -- sin evidencia real de que la degradación de CPU bajo carga haya sido un problema concreto todavía, así que se documenta el límite reducido y la mitigación disponible en vez de forzar un diseño nuevo sin demanda confirmada.

---

### 3.59 PostgreSQL: acepta PK autoincremental de 32/16 bits, no solo `BIGSERIAL` — RESUELTO

Bug real encontrado auditando PLAN.md §8.5 (reporte de adopción de una app financiera sobre una base Postgres ya existente): `validate_existing_id_column` (`runtime/db.rs`, agregada para el caso de `id UUID`) ya aceptaba `bigint`, `integer` Y `smallint` como tipos válidos de "id" para una tabla preexistente -- pero `insert_returning_id`/`postgres_cell` (`runtime/store.rs`) leían esa columna con `try_get::<_, i64>`, que exige que el OID de la columna sea EXACTAMENTE `int8`. Una tabla real con `id SERIAL` (`int4`, típico al migrar desde un backend que no usaba `BIGSERIAL`) pasaba la conexión sin ninguna queja -- y fallaba en el primer `insert`, con un error de tipo que ninguna de las dos capas documentaba de este lado. El comentario que quedó al lado de ese `try_get` incluso afirmaba "esto nunca dispara" apoyándose en una validación que, leída con cuidado, ya aceptaba justamente el caso que lo disparaba -- el mismo patrón de "dos capas que discrepan" que este documento viene registrando desde §3.9.

**La corrección** generaliza `postgres_cell` (no solo el camino de `insert_returning_id`, que fue donde se encontró el bug) con un helper que prueba `int8` → `int4` → `int2` en orden, aceptando cualquiera de los tres anchos que `validate_existing_id_column` ya reconocía como válidos -- y que además importa para CUALQUIER columna `Int` de una tabla adoptada, no solo `"id"`: un campo `Int` normal guardado como `INTEGER` en vez de `BIGINT` tenía exactamente el mismo problema.

**Límite que sigue en pie:** las tablas que `linkc` GENERA siguen usando `BIGSERIAL` siempre (`postgres_emit.rs`) -- esto es solo sobre LEER una tabla que ya existía con otro ancho, nunca sobre crear una nueva con un ancho distinto.

**Verificado (24/08/2026, contra un Postgres local real -- ver §3.104 para cómo se destapó que esto NO estaba tan resuelto como decía esta nota):** un test en `pg_integration.rs` (`a_preexisting_table_with_a_32_bit_serial_id_accepts_inserts_and_reads`) crea una tabla a mano con `id SERIAL PRIMARY KEY` y confirma `insert`/`get`/`list` de punta a punta. Esta nota decía antes "sin verificar en esta sesión -- el test corre de verdad recién en CI" porque no había Postgres disponible localmente cuando se escribió -- una auditoría posterior de por qué CI llevaba varios pushes en rojo (§3.104) encontró que ESE supuesto no se había confirmado nunca: el LADO DE LECTURA que esta sección arregla (`postgres_int_cell` probando `int8`→`int4`→`int2`) era correcto, pero el test seguía fallando por un bug DISTINTO en el lado de ESCRITURA (bindear un parámetro `Int` contra una columna no-`int8`) que nadie había encontrado porque nadie había corrido este test contra un Postgres real todavía. Ver §3.104 para ese fix.

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

**Límites honestos que siguen en pie:** sin adjuntos, sin `cc`/`bcc`, sin envío asíncrono -- los tres son sincrónicos, un relay lento sigue haciendo lenta a ESA request mientras dura (desde GRAMMAR.md §3.158/v1.114.0, ya no "al servidor entero" -- ese era el comportamiento antes de un hilo real por request). Nada de esto cambió respecto a `send`.

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
- **Visible por herramientas de diagnóstico del gestor de procesos, sea flag o env var.** Ni `linkc` ni ningún lenguaje de aplicación puede evitar esto: `pm2 describe`, `systemctl show`, y `/proc/<pid>/environ`/`/proc/<pid>/cmdline` en Linux muestran el entorno y los argumentos REALES con los que el proceso arrancó -- exactamente lo que ese gestor necesita para poder reiniciarlo. Un incidente real (25/08/2026): diagnosticar un despliegue con `pm2 describe` filtró `LINK_JWT_SECRET`/`LINK_SERVICE_API_KEY` en texto plano al transcript de la sesión que lo corrió. Tratar la salida de estas herramientas como tan sensible como el propio secreto -- nunca pegarla completa en un chat/ticket/log sin redactar los valores -- es responsabilidad de quien opera el despliegue, no algo que `--jwt-secret`/`--service-api-key` (§3.93) puedan mitigar por sí solos.

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

**Límite que sigue en pie: sin truncado de fechas. [ACTUALIZADO -- ver §3.157].** Agrupar por un `Timestamp` sigue sin aceptarse -- un `Timestamp` se guarda como milisegundos exactos (`BIGINT`, §3.31), así que agruparlo tal cual produciría un grupo por fila, nunca cohortes reales. Lo que hace falta es un método de truncado (`.truncateToMonth()`, por ejemplo) reconocido en la MISMA posición de selector, empujado a `DATE_TRUNC`/`strftime` según el backend -- una ronda aparte a propósito: los dos backends divergen de verdad acá (Postgres necesita convertir el `BIGINT` a un `timestamp` nativo con `to_timestamp`/`EXTRACT(EPOCH ...)` antes de truncar; SQLite trunca con `strftime` y devuelve texto, no milisegundos), y ese tipo de divergencia entre backends es exactamente la clase de bug que este proyecto viene encontrando y documentando desde §3.9 -- mejor una ronda propia con tests dedicados en los dos motores que apurarla acá. §3.157 hace exactamente eso.

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

### 3.71 `@deprecated("motivo")` en un campo o un rpc — RESUELTO

Marcar una parte del contrato como "existe pero no la uses para código nuevo" no tenía forma declarativa -- la única opción era un comentario en el `.link` que nunca llegaba al `.d.ts` generado, así que quien integraba el cliente TypeScript no se enteraba hasta que alguien le avisara a mano.

<!-- linkc:check -->
```rust
type Contact = {
  id: Int,
  @deprecated("usa email en su lugar, se elimina en la próxima versión mayor")
  legacyPhone: String?,
  email: String,
}
type NewContact = { legacyPhone: String?, email: String }

db { contacts: Contact[] }

service Contacts {
  @deprecated("usa createV2, que valida el formato de email")
  rpc create(email: String) -> Contact {
    db.contacts.insert(NewContact { legacyPhone: null, email: email })
  }
  rpc createV2(email: String) -> Contact {
    db.contacts.insert(NewContact { legacyPhone: null, email: email })
  }
}

test "un campo o rpc deprecado sigue funcionando igual -- es puramente informativo" {
  let c = Contacts.create("a@b.com");
  assert(c.email == "a@b.com", "el rpc deprecado sigue creando el registro normalmente");
}
```

**Dos puntos de anclaje, cada uno con su propia gramática.** Sobre un `rpc`/`stream` reusa el mecanismo de anotaciones que ya existía (`RpcDecl.annotations`, junto a `@authenticated`/`@route`/`@rate_limit`/etc.) -- se puede combinar libremente con cualquiera de esas, es una dimensión ortogonal. Sobre un campo de `struct` es **la única anotación que un campo admite hoy**: `Field` no tiene un `Vec<Annotation>` genérico como `RpcDecl` -- el parser solo sabe reconocer `@deprecated("...")` en esa posición, y cualquier otro nombre (`@authenticated`, por ejemplo) se rechaza ahí mismo con un error de sintaxis, no silenciosamente.

**Puramente informativo -- cero efecto en runtime o en el checker de tipos.** Un `rpc`/campo deprecado se sigue pudiendo llamar/leer/escribir exactamente igual; `@deprecated` no bloquea nada en compilación (a diferencia de, por ejemplo, `@content_type`, que si se usa mal es un error). Tampoco participa de la subtipificación estructural: dos `struct` idénticos salvo que uno marca un campo con `@deprecated` y el otro no siguen siendo el MISMO tipo a los ojos del checker -- la anotación es metadata de documentación, no de forma.

**Propagado al `.d.ts` como JSDoc, y a `openapi.json` con la keyword nativa.** En `contract.d.ts`, el motivo aparece como `/** @deprecated <motivo> */` justo antes del campo en la `interface`, o antes de la firma del método en `{Service}Client` -- cualquier editor que entienda JSDoc (VS Code incluido) tacha automáticamente esa línea en el código de quien lo consume. En `openapi.json` se usa `"deprecated": true` (keyword estándar tanto de Operation Object como de JSON Schema 2020-12, la base de OpenAPI 3.1), más `"description"` con el motivo -- sin inventar ninguna extensión `x-*` propia.

**Límites honestos:**
- **Sin `@deprecated` sobre un `type`/`enum` completo, ni sobre un parámetro de rpc.** Solo campos de struct y rpc/stream completos -- deprecar un tipo entero o un parámetro individual no está cubierto; el workaround es deprecar cada campo del tipo, o el rpc entero si el parámetro es lo único que cambió.
- **Sin fecha de remoción ni versión estructurada.** El motivo es texto libre (`"usa X en su lugar"`) -- no hay un campo separado tipo `removedIn: "2.0.0"` que una herramienta pudiera leer programáticamente; si hace falta esa fecha, va como parte del texto.
- **La clase `{Service}ClientImpl` (la implementación concreta, no la interfaz `{Service}Client`) no repite el JSDoc en sus propios métodos.** `create{Service}Client(...)` devuelve el tipo `{Service}Client` (la interfaz), así que cualquier editor que tipe la variable contra ese tipo -- el caso normal -- sí ve el aviso; solo alguien que importe y tipe explícitamente contra la clase concreta se lo perdería.

**Verificado**: 6 tests en `checker.rs` (tipa limpio combinado con `@requires` en un rpc, rechaza dos `@deprecated` en el mismo rpc, rechaza motivo vacío en un rpc, tipa limpio en un campo sin afectar subtipificación estructural -- un struct con un campo deprecado sigue aceptándose donde se espera el struct equivalente sin la anotación --, rechaza motivo vacío en un campo, rechaza cualquier otra anotación sobre un campo) y 4 en `codegen` (2 en `ts_emit.rs`: el JSDoc aparece exactamente antes del campo/método marcado y en ningún otro, un motivo con `*/` literal no corta el comentario antes de tiempo; 2 en `openapi_emit.rs`: `deprecated: true` + `description` en la operación y en la propiedad del schema, ausentes en las que no llevan la anotación).

---

### 3.72 Docstrings `///` propagados a OpenAPI y al `.d.ts` — RESUELTO

Hasta esta ronda, la única documentación de un `rpc` en `openapi.json` era su propio nombre en `"summary"` -- cualquier comentario `//` o `/* */` que alguien escribiera arriba se perdía por completo al compilar: el lexer los trataba todos igual, como trivia a descartar.

<!-- linkc:check -->
```rust
type Task = { id: Int, title: String }
type NewTask = { title: String }

db { tasks: Task[] }

service Tasks {
  /// Crea una tarea nueva.
  /// El titulo no puede estar vacio -- lo valida el checker, no este comentario.
  rpc create(title: String) -> Task {
    db.tasks.insert(NewTask { title: title })
  }
}

test "un rpc documentado con /// sigue funcionando exactamente igual" {
  let t = Tasks.create("comprar leche");
  assert(t.title == "comprar leche", "el docstring es puramente informativo, no cambia nada en runtime");
}
```

**`///` (exactamente 3 slashes) es la única forma que se captura -- `//` y `/* */` se siguen descartando igual que siempre.** `////` (4 o más) tampoco cuenta como docstring, a propósito: sigue siendo el separador visual común (`//// Sección ////`) sin ganar un significado nuevo por accidente. Varias líneas `///` consecutivas (sin nada más que espacio en blanco entre ellas) se unen con `\n` en un solo texto -- así es como se escribe un docstring de más de una línea.

**Cero riesgo de romper un programa existente.** Un `///` en CUALQUIER posición sigue siendo válido -- el lexer lo saltea como trivia exactamente igual que antes, esté o no justo arriba de un `rpc`/`stream`. La única diferencia es que, ADEMÁS de saltearlo, el texto queda capturado y se lo pega al siguiente token real (`Token::leading_doc`, ver lexer.rs) -- el parser solo lo lee en el único lugar donde tiene sentido (justo antes de un `rpc`/`stream`, incluso si hay una `@annotation` en el medio: `/// texto` seguido de `@requires(...)` seguido de `rpc` sigue atribuyéndose al rpc). En cualquier otra posición, el texto capturado simplemente no lo lee nadie -- ningún programa que compilaba antes deja de compilar.

**Propagado a `openapi.json` como `description` del Operation Object, y a `contract.d.ts` como un bloque JSDoc multilínea.** Si el mismo rpc también lleva `@deprecated("...")` (§3.71), las dos cosas conviven en el mismo campo en vez de que una pise a la otra: en OpenAPI, el motivo se agrega al final de la descripción (`"{docstring}\n\nDeprecated: {motivo}"`); en el `.d.ts`, `@deprecated` aparece como su propia línea de tag DENTRO del mismo bloque `/** ... */` -- nunca dos comentarios separados.

**Límites honestos:**
- **Solo sobre `rpc`/`stream` -- no sobre `type`/`enum`/campo de struct.** A diferencia de `@deprecated` (que sí llega a un campo), un docstring sobre un `type` completo o sobre un campo individual no se captura ni se propaga a ningún lado todavía -- el ítem original (PLAN.md §9.2) pedía específicamente documentación de rpc, que es donde hoy `openapi.json` tiene el hueco más visible.
- **Un `///` "huérfano" (sin ningún `rpc`/`stream` inmediatamente después, ni siquiera separado por una línea en blanco) se pierde en silencio, sin error.** El lexer no distingue "esto está pegado a una declaración real" de "esto quedó suelto al final de un archivo" -- ambos casos simplemente no producen ningún `RpcDecl.doc`. Mismo comportamiento que cualquier comentario hoy: nada avisa que "sobra".
- **Una línea en blanco entre el `///` y el `rpc` NO rompe la asociación.** El texto capturado se pega al PRÓXIMO token real sin importar cuántas líneas en blanco (o comentarios `//`/`/* */` intercalados) haya en el medio -- así que un docstring separado por accidente de su rpc por una línea vacía igual se atribuye, cosa que puede sorprender si la intención era documentar otra cosa más abajo.

**Verificado**: 4 tests en `lexer.rs` (`///` se saltea como trivia igual que `//` pero además queda en `leading_doc` del próximo token real, varias líneas consecutivas se unen con `\n`, `////` NO cuenta como docstring, una línea `///` vacía produce `Some("")` no `None`), 3 en `parser.rs` (se atribuye al rpc directamente arriba, sigue atribuyéndose cuando hay una `@annotation` en el medio, un rpc sin docstring tiene `doc: None`), 2 en `openapi_emit.rs` (se propaga como `description`, y junto con `@deprecated` las dos se combinan en un solo texto en vez de pisarse) y 3 en `ts_emit.rs` (bloque JSDoc multilínea antes del método, docstring + `@deprecated` en un solo bloque con `@deprecated` como tag final, un rpc sin ninguna de las dos cosas no gana ningún comentario).

---

### 3.73 `@validate(email)` / `@validate(regex, "...")` sobre un campo — RESUELTO

Hasta esta ronda, "validación de más allá del tipo" (¿este `String` tiene forma de email? ¿matchea un patrón?) era responsabilidad de cada `rpc`, a mano, con el mismo riesgo de cada adoptante reimplementando (o directamente no implementando) el mismo chequeo. Cierra a la vez el ítem de "validadores declarativos por campo" de PLAN.md §9.2 y la petición de "validación de request body más allá del tipo" del lado HTTP (§9.4): son la misma cosa vista desde dos ángulos.

<!-- linkc:check -->
```rust
type Signup = {
  id: Int,
  @validate(email) email: String,
  @validate(regex, "^[A-Z]{3}-[0-9]{4}$") invoiceCode: String,
}
// La anotación va en LA DECLARACIÓN que de verdad se construye adentro del
// rpc -- acá NewSignup, no solo Signup -- ver "Límites honestos" más abajo.
type NewSignup = {
  @validate(email) email: String,
  @validate(regex, "^[A-Z]{3}-[0-9]{4}$") invoiceCode: String,
}

db { signups: Signup[] }

service Signups {
  rpc create(email: String, invoiceCode: String) -> Signup {
    db.signups.insert(NewSignup { email: email, invoiceCode: invoiceCode })
  }
}

test "un email y un codigo validos se aceptan; uno invalido se hubiera rechazado antes de llegar aca" {
  let s = Signups.create("persona@ejemplo.com", "ABC-1234");
  assert(s.email == "persona@ejemplo.com", "email valido pasa el validador");
  assert(s.invoiceCode == "ABC-1234", "regex valido pasa el validador");
}
```

**Dos formas, ambas sobre `String`/`String?` únicamente.** `@validate(email)` exige una forma general de dirección de email (ver "Límites honestos"); `@validate(regex, "patrón")` compila el patrón con la crate `regex` y exige que el valor completo matchee. A lo sumo UNA `@validate` por campo -- el parser la rechaza si aparece dos veces, mismo criterio que `@deprecated`. Sobre cualquier tipo que no sea `String`/`String?` (`Int`, `Bool`, un struct anidado) es un error de compilación, no un validador que simplemente nunca corre.

**El patrón de `regex` se compila EN `linkc build`, no en el primer request real.** Un patrón roto (`"[unclosed"`) es un error de compilación citando el mensaje real de la crate `regex` -- nunca un 500 la primera vez que alguien manda datos.

**Enforcement real en CUATRO lugares, no solo documentación.** (1) El servidor (`linkc serve`) rechaza con 400 cualquier valor que no pase el validador, en DOS puntos -- `json_to_typed_value` (un `rpc` que recibe el struct COMPLETO como parámetro, ej. `rpc update(s: Signup)`) y `Expr::StructLit` en el intérprete (un `rpc` que arma el struct DENTRO del cuerpo a partir de parámetros sueltos, ej. `rpc register(email: String) { ... NewSignup { email: email } ... }` -- el caso más común, y el que motivó agregar el segundo punto: probando contra un servidor real con `curl`, un email inválido pasaba de largo con 200 porque solo el primer punto existía todavía). Los dos resuelven la lista de `ast::Field` (con sus `@validate`) contra la declaración ORIGINAL por nombre -- `field_annotations_for` en runtime/mod.rs. (2) `openapi.json` usa las keywords ESTÁNDAR de JSON Schema `"format": "email"` / `"pattern": "..."`, sin extensión propia. (3) `schemas.ts` (Zod) encadena `.email()` / `.regex(new RegExp("..."))` -- `new RegExp(json_string)` en vez de un literal `/.../`, para no tener que escapar `/` dentro del patrón del usuario. (4) `contract.d.ts` lleva un comentario JSDoc informativo (`Formato: email` / `Formato: coincide con /patrón/`) -- sin tag JSDoc estándar propio para esto, así que texto libre en vez de inventar uno.

**Única excepción de esta sesión a "cero dependencias nuevas".** UUID/SHA-256/ISO-8601 son formas FIJAS y acotadas (36 caracteres en posiciones fijas, por ejemplo) -- hand-rollables sin drama. Un patrón de `@validate(regex, "...")` es texto arbitrario del usuario: soportar solo un subconjunto de sintaxis regex a mano sería un espejismo de corrección (funciona hasta que alguien usa un lookahead o una clase de caracteres que el subconjunto no cubre, y ahí falla de forma confusa). La crate `regex` es puro Rust (compila también a `wasm32-unknown-unknown`, por eso NO está detrás del feature `runtime` como `rusqlite`/`postgres`), del mismo ecosistema que ya se confía para el resto del compilador.

**Límites honestos:**
- **`@validate` está atado a LA DECLARACIÓN donde se escribe, nunca a la forma estructural del campo.** El gotcha real, encontrado probando (no leyendo el código): el patrón "New\*" que el resto de este documento usa en TODOS LADOS para `insert` (`Omit<T, "id">`, ver §2.1) es un tipo APARTE de `Signup`, no un alias -- si `@validate(email)` está solo en `Signup.email` y `NewSignup.email` no lo repite, construir `NewSignup { email: "basura" }` no valida NADA, aunque `Signup.email` sí esté anotado. No es un bug: es la misma regla que ya rige `@deprecated` (§3.71) y toda anotación de campo -- atada a la declaración, nunca "hereda" entre dos tipos structuralmente parecidos. Pero es fácil pisarlo sin darse cuenta con este patrón específico, así que el ejemplo de arriba anota LAS DOS declaraciones a propósito.
- **`@validate(email)` no es RFC 5322 completo.** Exige exactamente un `@`, local-part no vacío sin espacios, dominio con al menos un `.` y ningún segmento vacío -- rechaza formas exóticas pero técnicamente válidas (local-part entre comillas, IP literal entre corchetes) que casi ningún email real usa.
- **`validators.ts` (las funciones `isX(x): x is X` hand-escritas) NO enforce `@validate` todavía.** Siguen verificando forma/tipo (incluida la regex fija de `Uuid`, GRAMMAR.md §3.70) pero no un `@validate` de usuario -- ese emisor trabaja sobre `types::FieldType` (estructural, sin anotaciones), no sobre la declaración `ast::Field` original, y conectar los dos ahí es más superficie de la que este ítem pedía. `openapi.json`, `schemas.ts` y el servidor real sí enforce completo.
- **Una fila YA guardada en `db` antes de agregar `@validate` no se re-valida al LEERLA.** El enforcement es en CONSTRUCCIÓN (`Expr::StructLit`) y en el DECODE del wire -- una fila leída de SQLite/Postgres (`db.<c>.find`/`all`/etc.) nunca pasa por ninguno de los dos puntos, así que un valor viejo que ya no cumpliría el validador sigue leyéndose sin error. Mismo criterio que el resto del lenguaje: la validación de forma es de ENTRADA, no una invariante re-chequeada en cada lectura.
- **La sintaxis de regex es la de la crate `regex` de Rust del lado servidor, y la nativa de JS del lado `schemas.ts`.** Las dos son PCRE-como pero no idénticas -- ninguna soporta backreferences; lookaround (`(?=...)`/`(?!...)`) existe en JS pero no en `regex` de Rust. Un patrón que use lookaround compila en `schemas.ts` pero falla en `linkc build` (la crate `regex` lo rechaza) -- se entera en compilación, no en producción, pero el mensaje de error no explica esta asimetría entre motores.
- **Sin más formas de validador** (`minLength`, `min`/`max` numérico, `oneOf`) -- solo `email`/`regex`, ampliable a futuro sin romper la forma (`@validate(nombre, ...)` ya es el patrón).

**Verificado**: 8 tests en `checker.rs`/`parser.rs` (tipa limpio con `email`/`regex`, sobre `String?`, sobre un campo de variante de enum; rechaza sobre `Int`, un patrón regex inválido, dos `@validate` en el mismo campo, y una forma desconocida como `@validate(minLength, 3)`), 5 en `runtime/mod.rs` contra un servidor real vía `invoke_rpc` (email malformado rechazado con 400 en 6 formas distintas y uno válido aceptado, regex rechaza lo que no matchea y acepta lo que sí, un campo opcional ausente no dispara validación pero uno presente sí, **el caso del struct construido adentro del cuerpo del rpc a partir de parámetros sueltos -- el que reveló el gap de `Expr::StructLit` --**, y el límite documentado arriba de que un shape "New\*" sin la anotación repetida no valida nada), 2 en `ts_emit.rs` (comentario JSDoc informativo, combinado con `@deprecated` en un solo bloque), 1 en `openapi_emit.rs` (`format`/`pattern` estándar) y 3 en `zod_emit.rs` (`.email()`, `.regex(new RegExp(...))`, orden correcto ANTES de `.nullable()` sobre un campo opcional). Verificado también a mano contra un servidor HTTP real (`curl`): el bug de `Expr::StructLit` (200 en vez de 400 para un email inválido armado adentro del rpc) se reprodujo primero así, antes de escribir el test que lo fija.

---

### 3.74 Valores por defecto en campos de `struct` — RESUELTO

Hasta esta ronda, un default solo existía en un parámetro de función/rpc (`rpc list(limit: Int = 20)`) -- un campo de `struct` no tenía forma de decir "si no viene, usá este valor", así que cada `rpc` de creación tenía que rellenarlo a mano (`status: "pending"` repetido en cada `NewX { ... }` del proyecto).

<!-- linkc:check -->
```rust
type Task = {
  id: Int,
  title: String,
  status: String = "pending",
}
type NewTask = { title: String, status: String = "pending" }

db { tasks: Task[] }

service Tasks {
  rpc create(title: String) -> Task {
    db.tasks.insert(NewTask { title: title })
  }
}

test "un campo con default se completa solo cuando el literal no lo menciona" {
  let t = Tasks.create("comprar leche");
  assert(t.status == "pending", "el default se aplico sin que el rpc lo pasara");
}
```

**Misma sintaxis y mismo mecanismo que `Param::default`, no una `@annotation`.** `nombre: Tipo = expr`, exactamente como un parámetro de función (§2.2) -- `Field` gana un `default: Option<Spanned<Expr>>` propio, en el mismo lugar del parser que ya sabía leer el default de un `Param`. Un campo CON default puede omitirse de un literal `Struct { ... }` igual que uno `?:` -- pero a diferencia de `?:`, el TIPO del campo no cambia a `Optional`: `status` sigue siendo `String`, nunca `String?`, adentro y afuera del literal.

**El default se evalúa DE NUEVO en cada construcción, no una sola vez.** `token: Uuid = crypto.uuid()` genera un UUID distinto por cada `NewSession { }` -- verificado comparando dos construcciones consecutivas. Mismo entorno de evaluación EXACTO que ya usa `Param::default` (`Env::new()` vacío): un default no ve otros campos del mismo literal ni el entorno que lo rodea, es una expresión autocontenida.

**Enforcement en dos capas, igual que `@validate`.** El CHECKER exige que el default tipe contra el tipo declarado del campo (`x: Int = "hola"` falla en `linkc build`, no en el primer request) Y que un literal que omite un campo SIN default (ni `?:`) siga rechazándose -- el cambio solo relaja la regla para campos que de verdad tienen un default. El INTÉRPRETE completa el valor en `Expr::StructLit`, el mismo punto que ya se tocó para `@validate` (§3.73) -- después de completar los defaults, cualquier `@validate` del mismo campo también corre sobre el valor final, así que un default roto (si el autor se equivoca) igual se detecta.

**Propagado a los tres generados como "campo opcional" -- mismo criterio que un parámetro de rpc con default.** `contract.d.ts` y `schemas.ts` (Zod) marcan el campo `?`/`.optional()`, así que quien construye el objeto del lado TS también puede omitirlo. `openapi.json` lo saca de `required` y, cuando el default es un literal simple (no una llamada como `crypto.uuid()`, que no tiene forma JSON fija sin evaluarla), lo suma como `"default"` -- keyword estándar de JSON Schema.

**Límites honestos:**
- **Un default no puede referenciar otros campos del mismo literal.** `{ a: Int, b: Int = a + 1 }` no es soportado -- el default se evalúa en un `Env::new()` vacío, no ve `a` (mismo límite que ya tenía `Param::default` para otros parámetros del mismo rpc).
- **Sin soporte en un `type` genérico.** `type Box<T> = { value: T, tag: String = "x" }` no aplica el default al construir `Box<Int> { value: 1 }` -- esa vía (`check_generic_struct_lit`/`expand_generic_struct`) trabaja con `types::FieldType` ya resuelto, que no conserva `default`. Alcance de esta ronda.
- **`validators.ts` no se toca.** Las funciones `isX(x): x is X` verifican forma de un valor YA EXISTENTE (típicamente de una respuesta HTTP) -- un default es un concepto de CONSTRUCCIÓN, no de validación de algo externo, así que no hay nada que cambiar ahí (a diferencia de `@validate`, donde sí había un gap real).
- **Sin `DEFAULT` a nivel de columna SQL.** El valor se completa en el intérprete, ANTES de que la fila llegue a SQLite/Postgres -- la columna generada sigue sin ningún `DEFAULT` propio. Un `INSERT` que bypasee el runtime de Link (una migración manual, por ejemplo) no se beneficia de este default.

**Verificado**: 2 tests en `parser.rs` (`= expr` parsea después del tipo, ausente da `None`), 4 en `checker.rs` (tipa limpio, rechaza un default de tipo equivocado, omitir un campo CON default tipa pero omitir uno SIN default sigue fallando, funciona sobre un campo de variante de enum), 3 en `runtime/mod.rs` contra un servidor real vía `invoke_rpc` (se completa al construir, un valor explícito lo pisa, `crypto.uuid()` como default dio dos valores DISTINTOS en dos construcciones separadas -- confirma evaluación fresca, no una sola vez), 2 en `ts_emit.rs` (campo con default sale opcional en la interfaz, uno sin default sigue requerido), 2 en `openapi_emit.rs` (`"default"` para un literal simple, ausente pero igual fuera de `required` para `crypto.uuid()`) y 1 en `zod_emit.rs` (`.optional()`). Verificado también a mano contra un servidor HTTP real (`curl`): crear sin mandar el campo devuelve el default, mandándolo explícito lo pisa.

---

### 3.75 `db.<c>.upsert(matchFn, insertValue, updateFn)` — RESUELTO

El caso "si existe actualizá, si no insertá" no tenía método propio -- confirmado como patrón reimplementado a mano (buscar con `findWhere`, borrar, reinsertar con el mismo id) en varios servicios reales. Reinsertar con el mismo id es además un problema real por sí mismo: en SQLite/Postgres con autoincrement, un borrado+inserción normalmente NO reproduce el mismo id (el contador sigue subiendo), así que esa implementación a mano ya arrastraba un bug de identidad estable, no solo boilerplate.

<!-- linkc:check -->
```rust
type Counter = { id: Int, name: String, count: Int }
type NewCounter = { name: String, count: Int }

db { counters: Counter[] }

service Counters {
  rpc bump(name: String) -> Counter {
    db.counters.upsert(
      |c: Counter| { c.name == name },
      NewCounter { name: name, count: 1 },
      |c: Counter| { NewCounter { name: c.name, count: c.count + 1 } }
    )
  }
}

test "la primera vez inserta, la segunda actualiza la MISMA fila" {
  let a = Counters.bump("clics");
  assert(a.count == 1, "primera vez: cuenta en 1");
  let b = Counters.bump("clics");
  assert(b.id == a.id, "misma fila, no una nueva");
  assert(b.count == 2, "updateFn incrementa sobre la fila existente");
}
```

**Tres argumentos: `matchFn: (T) -> Bool`, `insertValue: Omit<T,"id">`, `updateFn: (T) -> Omit<T,"id">`.** Si `matchFn` tiene la forma pusheable reconocida (`|x| x.campo == valor`, o una conjunción `&&` de varias hojas así -- la MISMA que `findWhere`/`countWhere`/`deleteWhere` ya usan, §3.95/§3.108/§3.109/§3.145), la SELECCIÓN se empuja a SQL (26/08/2026, ver "Límites honestos" abajo); cualquier otra forma (`||`, comparar dos campos entre sí) sigue trayendo la tabla ENTERA a memoria, como siempre. En los dos casos se queda con la PRIMERA fila que matchea. Sin match: se inserta `insertValue` (una fila nueva, id autoincrement normal). Con match: se llama `updateFn` con la fila EXISTENTE completa, y el valor que devuelve se aplica ENTERO sobre el MISMO id (vía el mismo mecanismo que `applyPatch`) -- nunca borra e inserta de nuevo, así que el id de la fila actualizada NO cambia, a diferencia del workaround manual que reemplazaba.

**`updateFn` devuelve `Omit<T,"id">` completo, no `Patch<T>` parcial -- decisión deliberada, no una limitación por descuido.** `Patch<T>` no tiene sintaxis de literal en el lenguaje (GRAMMAR.md §3.4): solo llega ya decodificado del wire como parámetro de un `rpc`, nunca se puede CONSTRUIR desde adentro de un cuerpo de función. Un `updateFn: (T) -> Patch<T>` sería, literalmente, imposible de escribir -- no hay ninguna expresión que produzca ese tipo. Devolver el shape insertable completo (`NewCounter { ... }`, un literal común y corriente) sí es constructible, y sigue permitiendo que la actualización DEPENDA de los otros campos de la fila existente (`c.count + 1`, no un valor estático) -- que es la ventaja real de pedir una función en vez de un valor fijo.

**Límites honestos:**
- **`matchFn` no pusheable trae la tabla ENTERA a memoria.** `||` o comparar dos campos entre sí (mismo límite ya documentado para `findWhere`/`deleteWhere`, §3.95/§3.145) siguen sin bajar a SQL -- landmine real cerrado parcialmente (26/08/2026): el caso más común (`|x| x.campo == valor`, una igualdad o conjunción de igualdades) ya no se degrada con el crecimiento de la colección, pero el caso general sigue abierto.
- **Sin control sobre CUÁL fila gana si `matchFn` matchea más de una.** Pusheado o no, se queda con la primera fila que matchea en el orden que la fuente devuelve (SQL sin `ORDER BY` explícito, o `all()` con su `ORDER BY "id"` de siempre en el camino interpretado) -- si el predicado no es lo bastante específico para matchear a lo sumo una fila, cuál se actualiza es determinístico pero no necesariamente el que el autor esperaba.
- **No atómico frente a una escritura concurrente entre el `matchFn` y el `applyPatch`/`insert` -- pero solo ENTRE INSTANCIAS distintas de `linkc serve`, no dentro de una.** Cuando esto se escribió, `linkc serve` era single-threaded (una request en vuelo a la vez), así que dentro de UN proceso no había carrera real -- la carrera solo era posible con más de una instancia contra la misma base (§3.44). **Bug real, encontrado y arreglado el 26/08/2026 (GRAMMAR.md §3.158): con un hilo real por request, la MISMA carrera se volvió posible DENTRO de un solo proceso** -- dos hilos podían buscar la fila existente a la vez, los dos ver "no hay match", y los dos insertar, duplicando exactamente la fila que `upsert` promete no duplicar nunca. Fix: `upsert` entero (buscar + decidir + escribir) corre bajo `Db::with_exclusive_connection`, el mismo candado reentrante que ya usa `transaction{}` -- `matchFn`/`updateFn` pueden seguir llamando a `db.<c>.*` sin deadlock porque el candado es reentrante para el mismo hilo. Verificado con 20 hilos reales corriendo `upsert` a la vez sobre el mismo `matchFn`: una sola fila, contador incrementado exactamente 20 veces (confirmado que el test detecta la carrera revirtiendo el fix a mano: sin el candado, el mismo escenario deja la fila con un contador muy por debajo de 20). **La carrera ENTRE procesos distintos (§3.44) sigue sin resolver** -- un candado en memoria de un proceso no protege contra otro proceso separado; eso necesitaría un constraint real de la base (`@unique` sobre el campo de `matchFn`, si el shape lo permite) o coordinación externa, no algo que este fix pueda cerrar.

**Bug real, encontrado por una auditoría multi-agente adversarial (26/08/2026), no por un reporte externo: el pushdown rompía la semántica de NULL sobre un campo opcional.** `conjunction_condition` (`runtime/db.rs`, compartida por `upsert`/`findWhere`/`countWhere`/`deleteWhere` -- §3.95/§3.108/§3.109/§3.145) generaba `"campo" = ?` ligado a un parámetro NULL cuando una hoja `c.opcional == variable` capturaba una `variable` que resultó `null` en runtime. En SQL, `x = NULL` nunca es cierto (NULL no es igual a nada, ni a sí mismo) -- pero el camino INTERPRETADO de siempre trata `Value::Null == Value::Null` como `true` (mismo criterio que `==` de c-script en cualquier otro lado). Efecto concreto en `upsert`: una fila existente con ese campo en `null` nunca se encontraba por el camino pusheado, así que se insertaba una fila DUPLICADA en vez de actualizar la existente -- silencioso, sin ningún error, divergiendo del comportamiento del camino interpretado (y del comportamiento de antes de que existiera el pushdown). Arreglado generando `"campo" IS NULL`/`IS NOT NULL` (sin ningún parámetro ligado) para una hoja `==`/`!=` cuyo operando resultó NULL -- los cuatro operadores relacionales (`<`/`<=`/`>`/`>=`) no tienen una forma NULL seguro razonable de todos modos y siguen cayendo al camino interpretado si el operando es NULL, mismo criterio que cualquier otra forma no pusheable.

**Verificado**: 3 tests en `checker.rs` (tipa limpio con las tres firmas correctas, rechaza un `updateFn` que devuelve un tipo que no es el shape insertable, rechaza menos de 3 argumentos) y 3 en `runtime/mod.rs` contra un servidor real vía `invoke_rpc` (sin match inserta con `count: 1`, con match pusheable actualiza la MISMA fila -- mismo id, `count` incrementado vía `updateFn` -- y un `matchFn` distinto sí inserta una fila nueva con id distinto; más un `matchFn` NO pusheable, `||`, confirmando que el camino interpretado de siempre sigue funcionando sin cambios de comportamiento). Verificado también a mano contra un servidor HTTP real (`curl`): primer `bump` inserta id=1 count=1, segundo `bump` con el mismo nombre actualiza a id=1 count=2, un nombre distinto inserta id=2. Más 1 test contra un Postgres REAL (`pg_integration.rs`, 26/08/2026) confirmando que el camino pusheado genera SQL válido y correcto contra ese backend, no solo SQLite. Sobre el bug de NULL: 1 test en `runtime/mod.rs` (un segundo `upsert` con el mismo campo `null` actualiza la MISMA fila, no inserta una duplicada; un `note` real y distinto de null sigue insertando/actualizando como siempre) + 1 contra Postgres real.

---

### 3.76 `db.<c>.insertMany(items)` — RESUELTO

Un backfill que necesita crear N filas hacía N llamadas a `insert` -- si venían del CLIENTE, N idas y vueltas HTTP secuenciales; si venían de un solo `rpc` con un loop adentro, N sentencias `insert` de todos modos, pero al menos una sola request. Ninguna de las dos formas tenía un método dedicado para "estas son todas nuevas, insertalas".

<!-- linkc:check -->
```rust
type Task = { id: Int, title: String }
type NewTask = { title: String }

db { tasks: Task[] }

service Tasks {
  rpc seed() -> Task[] {
    db.tasks.insertMany([
      NewTask { title: "primera" },
      NewTask { title: "segunda" },
      NewTask { title: "tercera" },
    ])
  }
}

test "insertMany inserta cada item y devuelve las filas con id real asignado" {
  let rows = Tasks.seed();
  assert(rows.length() == 3, "las tres filas se insertaron");
  assert(rows[0].title == "primera", "orden preservado");
  assert(rows[0].id != rows[1].id, "cada fila tiene su propio id real, no uno compartido");
}
```

**`db.<c>.insertMany(items: Omit<T,"id">[]) -> T[]`.** Mismo shape insertable que `insert` (`Omit<T,"id">`), pero como lista -- cada elemento se inserta con el `insert` de siempre (una sentencia SQL autocommit por fila, mismo criterio que el resto del lenguaje), en el orden dado. Lo que ahorra es la ida y vuelta HTTP N veces desde el cliente cuando N filas se crean juntas, no el costo de N inserts contra la base -- sigue siendo N sentencias SQL, no una sola sentencia batch.

**Límites honestos:**
- **Sin transacción envolvente.** Cada `insert` es autocommit por su cuenta (mismo criterio ya documentado para el resto del lenguaje, GRAMMAR.md §2.1/§3.17) -- si el ítem 3 de 5 falla (por ejemplo, un `@validate` que rechaza uno de los valores), los 2 primeros quedan insertados igual, no hay rollback automático de lo que ya se aplicó.
- **No es una sentencia SQL batch real.** `insertMany([a, b, c])` ejecuta 3 `INSERT` separados, no un `INSERT ... VALUES (...), (...), (...)` de una sola sentencia -- el ahorro es de round-trips HTTP del cliente, no de round-trips a la base de datos.

**Verificado**: 3 tests en `checker.rs` (tipa limpio con una lista del shape insertable, rechaza una lista de tipo equivocado, rechaza 0 argumentos) y 1 en `runtime/mod.rs` contra un servidor real vía `invoke_rpc` (las 3 filas se insertan con ids reales y distintos, en el orden dado, y quedan persistidas de verdad -- confirmado leyéndolas de vuelta con `all()` en una llamada aparte). Verificado también a mano contra un servidor HTTP real (`curl`): tres títulos mandados en un solo `insertMany`, tres filas con id 1/2/3 en la respuesta.

---

### 3.77 `createdAt`/`updatedAt` automáticos: `= now()` + `@autoUpdate` — RESUELTO

Fijar cuándo se creó una fila y cuándo se tocó por última vez es casi universal en cualquier tabla real -- hasta esta ronda, cada `rpc` de creación/edición tenía que asignar esos dos campos a mano, con el riesgo real de que alguien se olvide de tocar `updatedAt` en un `applyPatch` nuevo. La solución NO es una anotación mágica de campo por nombre (`createdAt`/`updatedAt` no son nombres reservados en ningún lado) -- es la COMPOSICIÓN de dos primitivas ya existentes, más una anotación chica y explícita para la única parte que de verdad faltaba.

<!-- linkc:check -->
```rust
type Task = {
  id: Int,
  title: String,
  createdAt: Timestamp = now(),
  @autoUpdate updatedAt: Timestamp = now(),
}
type NewTask = {
  title: String,
  createdAt: Timestamp = now(),
  @autoUpdate updatedAt: Timestamp = now(),
}

db { tasks: Task[] }

service Tasks {
  rpc create(title: String) -> Task {
    db.tasks.insert(NewTask { title: title })
  }
  rpc rename(id: Int, patch: Patch<Task>) -> Task {
    db.tasks.applyPatch(id, patch)
  }
}
```

**`createdAt` no necesita nada nuevo -- ya funcionaba con lo que esta sesión ya había agregado.** `now() -> Timestamp` (builtin sin receptor) más un valor por defecto de campo (`= now()`, GRAMMAR.md §3.74) alcanzan solos: un `NewTask { title: title }` que omite `createdAt` ya lo completa al construirse, sin ninguna anotación de por medio. Esto es a propósito -- componer primitivas chicas ya existentes en vez de agregar una anotación redundante que hiciera lo mismo.

**`@autoUpdate` es la única pieza genuinamente nueva, y solo hace falta para "tocar en CADA actualización".** Un default (`= now()`) se completa una sola vez, al CONSTRUIR el literal -- no vuelve a correr en un `applyPatch` posterior, porque `applyPatch` nunca construye un literal nuevo, solo aplica un `Patch<T>` ya decodificado. `@autoUpdate` sobre un campo `Timestamp` (nada más -- `Int`/`Bool`/etc. es un error de compilación) hace que ESE campo se pise a `now()` en CADA `applyPatch` -- y en el paso de actualización de `upsert` (§3.75), que internamente usa el mismo mecanismo -- sin importar qué traiga el patch para ese campo, incluso si el patch ni lo menciona. Interceptado en `runtime::call_method` (no en `db.rs::Db::call`, que no tiene acceso al checker) justo antes de aplicar el patch de verdad.

**`createdAt` nunca se toca después del insert -- ni con `@autoUpdate`, ni sin él.** Ninguna de las dos primitivas usadas para `createdAt` (el default, la ausencia de `@autoUpdate`) hace que se reescriba en un `applyPatch` -- si el patch trae un valor para `createdAt`, ese valor se aplica tal cual (mismo comportamiento que cualquier otro campo escribible), pero nada lo fuerza a cambiar solo. Si además se quisiera que `createdAt` fuera literalmente INMUTABLE (rechazar incluso un intento explícito de cambiarlo), eso queda para un ítem aparte (constraints declarativos, PLAN.md §9.3).

**Límites honestos:**
- **Sin nombres de campo mágicos.** `@autoUpdate` funciona sobre CUALQUIER campo `Timestamp`, no solo uno llamado `updatedAt` -- y a la inversa, un campo llamado `updatedAt` sin la anotación NO se comporta especial. La automatización es siempre explícita.
- **`createdAt`/`updatedAt` (o los nombres que sean) siguen siendo columnas SQL normales, sin `DEFAULT`/trigger propio.** Mismo límite ya documentado para los defaults de campo en general (§3.74) -- todo pasa por el intérprete, nunca por la base.
- **`@autoUpdate` no distingue "el patch no traía nada más" de "el patch traía otros campos" -- siempre pisa, sin excepción.** Un `applyPatch(id, {})` (patch vacío, sin campos) IGUAL toca `updatedAt` -- no hay forma de "aplicar un patch sin que cuente como una actualización".

**Verificado**: 4 tests en `checker.rs` (`@autoUpdate` sobre `Timestamp` tipa limpio, se rechaza sobre otro tipo, no exige un default a la vez, una segunda `@autoUpdate` en el mismo campo es error de parser) y 2 en `runtime/mod.rs` contra un servidor real vía `invoke_rpc` (un campo `Timestamp = now()` se completa solo al insertar, y `@autoUpdate` pisa el campo en un `applyPatch` real aunque el patch mandado NO lo mencione, mientras `createdAt` -- sin la anotación -- se mantiene idéntico antes y después). Verificado también a mano contra un servidor HTTP real (`curl`, con un `sleep` real entre las dos llamadas): `createdAt` idéntico en las dos respuestas, `updatedAt` con un timestamp distinto y posterior en la segunda.

---

### 3.78 Soft-delete nativo: `@softDelete` — RESUELTO

"Borrar" una fila casi nunca significa borrarla de verdad -- la mayoría de los sistemas reales necesitan poder auditar o recuperar algo que un usuario marcó como eliminado. Hasta esta ronda, `db.<c>.delete(id)` siempre era un `DELETE` de SQL real, sin ninguna forma declarativa de pedir "marcalo como borrado en vez de borrarlo".

<!-- linkc:check -->
```rust
type Task = {
  id: Int,
  title: String,
  @softDelete deletedAt: Timestamp? = null,
}
type NewTask = { title: String, deletedAt: Timestamp? = null }

db { tasks: Task[] }

service Tasks {
  rpc create(title: String) -> Task {
    db.tasks.insert(NewTask { title: title })
  }
  rpc list() -> Task[] { db.tasks.all() }
  rpc remove(id: Int) -> Bool { db.tasks.delete(id) }
}

test "delete() no borra la fila -- la marca, y all() la deja de traer" {
  let t = Tasks.create("comprar leche");
  assert(Tasks.list().length() == 1, "arranca visible");
  assert(Tasks.remove(t.id) == true, "delete() devuelve true la primera vez");
  assert(Tasks.list().length() == 0, "all() ya no la trae");
  assert(Tasks.remove(t.id) == false, "una segunda vez sobre la misma fila no hace nada -- idempotente");
}
```

**`@softDelete` solo sobre `Timestamp?` (opcional) -- nunca `Timestamp` requerido.** `null` es el estado "no borrado", cualquier otro valor es "borrado en este instante" -- por eso el campo TIENE que ser opcional, no hay otra forma de representar los dos estados. A lo sumo UN campo `@softDelete` por struct (dos sería ambiguo: `delete()` no sabría cuál de los dos fijar) -- rechazado en compilación, nombrando los dos campos. `= null` (el mismo mecanismo de default de campo, §3.74) es lo que hace que `NewTask { title: title }` no necesite mencionar `deletedAt` -- sin el default, cada `insert` tendría que pasarlo a mano.

**`delete(id)` deja de ser un `DELETE` SQL -- pasa a ser un `UPDATE` que fija el campo a `now()`.** `AND "<campo>" IS NULL` en el propio `WHERE` hace la operación IDEMPOTENTE: una segunda llamada sobre una fila ya borrada no re-toca el timestamp (no publica un evento de `stream` de nuevo tampoco), devuelve `false` -- igual que `delete` sobre un `id` que nunca existió.

**Toda lectura que devuelve una LISTA o un CONTEO filtra automáticamente.** `all()`, `page()`, `pageAfter()`, `count()`, `sumBy`/`countBy`/`avgBy`/`maxBy`/`minBy` agregan `WHERE "<campo>" IS NULL` (o lo combinan con `AND` cuando ya había otra condición, como el cursor de `pageAfter`) -- ninguna fila soft-deleteada aparece en ninguno de estos. `findWhere`/`deleteWhere` heredan el filtro GRATIS, sin ningún caso especial propio: los dos reusan `all()` por dentro (mismo mecanismo ya documentado en §3.9/§8), así que si `all()` ya no trae la fila, el predicado de `findWhere`/`deleteWhere` nunca llega a evaluarla.

**Límites honestos:**
- **`find(id)` (y la re-consulta interna que hacen `insert`/`applyPatch` después de escribir) NO filtra -- una fila soft-deleteada sigue siendo encontrable por id directo.** Deliberado, no una omisión: `insert`/`applyPatch` re-consultan la fila que ELLOS MISMOS acaban de escribir por el mismo camino que usa `find` -- si un `applyPatch` tocara justo el campo de soft-delete (nada lo impide, es un campo escribible como cualquier otro), filtrar ahí haría que la re-consulta no encontrara la fila recién escrita, un panic en vez de un error limpio. La distinción real termina siendo "listados filtran, lookup directo por id no" -- mismo criterio que varios frameworks reales (Django, Rails) ya adoptan para el mismo problema.
- **Sin forma de pedir "traeme TODO, incluidas las borradas" desde `all()`/`page()`/etc.** No hay un parámetro `includeDeleted` ni una variante -- quien necesite ver filas borradas hoy solo puede usar `find(id)` una por una (ver el límite de arriba).
- **`applyPatch` puede seguir escribiendo el campo `@softDelete` como cualquier otro campo del `Patch<T>`.** No hay ninguna protección que lo vuelva de solo-lectura para `applyPatch` -- un patch que trae `{"deletedAt": "..."}` lo aplica tal cual, sin pasar por la lógica de idempotencia de `delete()`. "Restaurar" una fila (poner `deletedAt` de vuelta en `null` a mano vía `applyPatch`) funciona, pero no es una operación con nombre propio todavía.
- **Sin `DEFAULT`/índice parcial a nivel de columna SQL.** El filtro se agrega en cada consulta desde el intérprete -- no hay un índice `WHERE deletedAt IS NULL` creado automáticamente que acelere esas consultas sobre una tabla grande.

**Verificado**: 5 tests en `checker.rs` (`Timestamp?` tipa limpio, se rechaza sobre `Timestamp` requerido y sobre cualquier otro tipo, dos `@softDelete` en el mismo struct se rechaza, una segunda `@softDelete` en el mismo campo es error de parser) y 5 en `runtime/mod.rs` contra un servidor real vía `invoke_rpc` (`delete` fija el campo en vez de borrar la fila, una segunda llamada es idempotente y devuelve `false`, `all()`/`count()` excluyen la fila borrada, `findWhere`/`deleteWhere` heredan el filtro sin código propio, y `page`/`pageAfter`/`sumBy` también filtran). Verificado también a mano contra un servidor HTTP real (`curl`): crear 2 filas, borrar una, `list`/`count` muestran solo 1, un segundo `delete` sobre la misma da `false`, `find` directo por id SIGUE encontrando la fila borrada con su `deletedAt` ya fijado.

---

### 3.79 `linkc build --diff <archivo-anterior>` — RESUELTO

Revisar un PR que toca un `.link` significa, en la práctica, revisar qué cambió en el CONTRATO público que consume el frontend -- no el `.link` mismo (eso ya lo muestra `git diff` normal), sino el `contract.d.ts` que `linkc build` termina generando. Hasta esta ronda no había forma de pedirle eso al compilador directamente; había que generar los dos contratos a mano y diffearlos con una herramienta aparte.

```bash
# guardar el contrato de la rama base ANTES de aplicar los cambios del PR
git show origin/main:gen/contract.d.ts > /tmp/contract-base.d.ts

# build normal de la rama del PR, comparando contra esa base
linkc build app.link gen --diff /tmp/contract-base.d.ts
```

`--diff <archivo>` compara el `contract.d.ts` RECIÉN generado (el de la corrida actual de `linkc build`) contra el contenido de `<archivo>`, línea por línea, e imprime el resultado en la salida estándar -- reusa el mismo diff LCS (programación dinámica, O(n·m), sin ninguna dependencia nueva) que `linkc test` ya usaba para mostrar por qué un snapshot dejó de coincidir (GRAMMAR.md §5). Sin cambios: `el contrato no cambió respecto a '<archivo>'`. Con cambios: una línea por cambio, `- ...` para lo que desapareció, `+ ...` para lo que apareció, en el mismo orden relativo que ya tenían (no reordenado alfabéticamente ni agrupado por tipo).

**Puramente informativo -- nunca hace fallar el build.** A diferencia de `linkc test` (que si el snapshot no coincide devuelve código de salida distinto de cero, porque ahí "cambió sin querer" es justo lo que se busca atrapar), acá el build ya tuvo éxito antes de llegar a la comparación -- `--diff` solo agrega texto a la salida para que una persona lo lea, nunca cambia si el comando termina bien o mal. Un `<archivo>` que no se puede leer (no existe, sin permisos) imprime una advertencia por stderr y el build sigue siendo exitoso igual -- el archivo de comparación es responsabilidad de quien arma el pipeline de CI/revisión, no algo que el compilador pueda validar de antemano.

**Límites honestos:**
- **`<archivo>` es un `contract.d.ts` guardado aparte, no un ref de git ni un commit.** No hay integración con git -- guardar la versión "anterior" (`git show <rev>:<path> > archivo`, como en el ejemplo de arriba) es responsabilidad de quien arma el pipeline.
- **Solo compara `contract.d.ts`.** `client.ts`/`validators.ts`/`hooks.ts`/`schemas.ts`/`openapi.json` no entran en el diff -- son derivados del mismo contrato, así que casi todo cambio real ya se ve reflejado ahí, pero un cambio que SOLO tocara, por ejemplo, la forma exacta de un validador sin cambiar ningún tipo público no aparecería.
- **Diff de texto plano, no un diff semántico de tipos.** No distingue "se agregó un campo opcional" (cambio compatible hacia atrás) de "se cambió el tipo de un campo existente" (cambio que rompe) -- ambos aparecen igual, como líneas `-`/`+`; es una persona la que decide qué tan grave es cada cambio, mirando el diff.

**Verificado**: `cli_build_diff.rs` con el binario real como subproceso (agregar un campo muestra exactamente la línea `+` que corresponde, sin ningún cambio real muestra "no cambió", un archivo de comparación inexistente no hace fallar el build -- solo avisa por stderr, y `linkc build` sin `--diff` sigue funcionando exactamente igual que antes de esta ronda).

---

### 3.80 Índices declarativos: `@index`/`@unique` — RESUELTO

Hasta esta ronda, la única columna indexada de cualquier tabla era la PK (`id`) -- cualquier otra búsqueda frecuente (`findWhere(|u: User| { u.email == e })` sobre una tabla grande) hacía un table scan completo, y no había forma de pedirle a la base una restricción de unicidad real: un email repetido solo se podía prevenir a mano, con una lectura previa que además queda expuesta a una carrera entre dos requests concurrentes.

<!-- linkc:check -->
```rust
type User = {
  id: Int,
  @unique email: String,
  @index country: String,
  name: String,
}
type NewUser = { email: String, country: String, name: String }

db { users: User[] }

service Users {
  rpc create(email: String, country: String, name: String) -> User {
    db.users.insert(NewUser { email: email, country: country, name: name })
  }
}

test "un email nuevo se acepta -- el indice unico no molesta al camino feliz" {
  let u = Users.create("ada@ejemplo.com", "AR", "Ada");
  assert(u.email == "ada@ejemplo.com", "el insert normal sigue funcionando igual");
}
```

**Dos anotaciones de campo, sin paréntesis -- `@index` (no exige unicidad) y `@unique` (índice + restricción de unicidad).** A lo sumo UNA de las dos por campo -- combinarlas sería redundante (`@unique` ya implica un índice), rechazado en el PARSER, no en el checker (mismo criterio de forma que `@autoUpdate`/`@softDelete`, §3.77/§3.78). A diferencia de esas dos, ninguna exige un tipo de campo particular -- un índice SQL tiene sentido sobre casi cualquier columna, así que `@index`/`@unique` tipan limpio sobre `Int`, `String?`, un enum simple, lo que sea.

**El índice se crea de verdad al arrancar, en LOS DOS backends.** `Db::new`/`Db::new_with_options` (SQLite) y el lado Postgres del mismo constructor ejecutan `CREATE [UNIQUE] INDEX IF NOT EXISTS "idx_<tabla>_<campo>" ON "<tabla>"("<campo>")` por cada campo anotado, una vez por arranque -- `IF NOT EXISTS` lo hace idempotente, así que correr el servidor de nuevo no falla ni duplica nada. `linkc build` emite la MISMA sentencia (mismo nombre determinístico de índice) en el DDL estático que genera para Postgres, para que aplicar ese DDL a mano deje la base en el mismo estado que `linkc serve` hubiera creado sola.

**Una violación de `@unique` es un 400, no un 500.** `insert`/`applyPatch` (y por lo tanto `upsert` en su rama de update) atrapan el mensaje de error específico que SQLite (`UNIQUE constraint failed`) y Postgres (`duplicate key value violates unique constraint`) devuelven para este caso puntual y lo traducen a `RuntimeError::bad_request` -- un email repetido es un error del CLIENTE (mandó un valor que ya existe), nunca "el servidor se rompió". Cualquier otra falla de SQL (columna inexistente, base caída) sigue siendo un 500 genuino, sin cambios.

**`--adopt-existing` nunca ejecuta este DDL, ni siquiera para un campo anotado.** Mismo criterio ya establecido para el resto del schema (§3.67): en modo adopción NINGÚN DDL corre, así que un índice declarado sobre una colección adoptada simplemente no se crea -- si la tabla real ya lo necesita, es responsabilidad de quien administra esa base agregarlo por fuera.

**Límites honestos:**
- **Solo índices de UN campo con ESTA anotación.** Un `@unique` COMPUESTO (varios campos a la vez, `@unique(profileId, slug)`) ya está resuelto -- a nivel de `type`, no de campo, ver §3.155.
- **Sin `@check` declarativo.** Una restricción de forma más general que unicidad (`price > 0`, por ejemplo) no es parte de esta ronda -- ver PLAN.md §9.3.
- **El nombre del índice es siempre `idx_<tabla>_<campo>`, no configurable.** Dos colecciones con el mismo nombre de campo anotado (`users.email` y `admins.email`, por ejemplo) generan nombres de índice distintos porque el nombre de TABLA ya entra en la fórmula -- no hay colisión real en la práctica, pero tampoco forma de elegir un nombre propio.
- **Índice sobre una columna JSON-serializada (struct/lista/map/genérico) es válido pero de utilidad dudosa.** El campo se guarda igual como TEXT con el JSON serializado (`ColumnPlan::for_field`) -- SQLite/Postgres indexan esa columna sin problema (y `serde_json` sin la feature `preserve_order` serializa las claves de un objeto siempre en el mismo orden, así que `@unique` sobre un `Map<K,V>` es correcto), pero comparar/ordenar por el texto de un JSON casi nunca es lo que alguien quiere de un índice.
- **`@unique`/`@index` nunca son índices PARCIALES respecto a `@softDelete` (AUDIT-2026-08-27.md #12) -- una fila soft-deleted sigue ocupando su slot único en el índice físico.** `create_index_statements`/`create_composite_unique_statements` (`runtime/db.rs`) y sus equivalentes de DDL estático en `codegen/postgres_emit.rs` nunca agregan `WHERE "<campoSoftDelete>" IS NULL` al `CREATE [UNIQUE] INDEX`, aunque los DOS backends soportan índices parciales. Consecuencia real: con `type User = { id: Int, @unique email: String, deletedAt: Timestamp? }`, borrar (soft-delete) a un usuario y después intentar CREAR uno nuevo con el MISMO email falla con 400 de violación `UNIQUE` -- aunque todo camino de LECTURA (§3.78) ya trata a ese usuario como inexistente, "borrar y volver a crear con la misma clave" no funciona como la mayoría esperaría. No es una respuesta incorrecta silenciosa (el 400 es honesto), es una limitación de diseño no atacada esta ronda -- arreglarla de verdad necesita índices parciales en los DOS backends (`CREATE UNIQUE INDEX ... WHERE "deletedAt" IS NULL`) y, más delicado, una migración segura para una base YA desplegada con el índice viejo (no-parcial) -- `CREATE ... IF NOT EXISTS` no reemplaza un índice existente por una versión parcial, haría falta `DROP`+`CREATE` explícito, una operación con riesgo real sobre una tabla en producción que este proyecto no toma a la ligera (mismo criterio de cautela que ya aplica en toda esta sección de migración no-destructiva). Discovery hecho, diseño e implementación quedan pendientes de una ronda propia.

**Verificado**: 4 tests en `checker.rs`/`parser.rs` (`@index`/`@unique` tipan limpio sobre cualquier tipo de campo, una segunda `@index` o `@unique` en el mismo campo es error de parser, combinar las dos en el mismo campo también), 4 en `runtime/db.rs` contra SQLite real (`@unique` crea un índice `UNIQUE` de verdad -- verificado leyendo `sqlite_master` -- y rechaza un segundo `insert`/`applyPatch` con el mismo valor devolviendo 400; `@index` sin `unique` no bloquea valores repetidos; `--adopt-existing` no crea el índice aunque el campo esté anotado) y 1 en `postgres_emit.rs` (el DDL estático de `linkc build` emite `CREATE UNIQUE INDEX`/`CREATE INDEX` con el mismo nombre determinístico que usa el runtime).

---

### 3.81 `--host <dirección>`: en qué interfaz escucha `linkc serve` — RESUELTO

Hasta esta ronda, `linkc serve` siempre escuchaba en `0.0.0.0` (todas las interfaces de red de la máquina) sin ninguna alternativa -- confirmado leyendo `runtime/server.rs`, no había ningún flag ni variable de entorno para acotarlo. Para un proceso que solo necesita aceptar conexiones locales (detrás de un proxy en el mismo host, por ejemplo, o en una máquina de desarrollo con otras cosas corriendo), eso deja el firewall del sistema operativo como la ÚNICA capa de defensa contra que otra máquina en la misma red le hable directamente -- un gap de seguridad real, no solo de conveniencia.

```bash
# Solo local -- ninguna otra máquina en la red puede conectarse directo.
linkc serve app.link 8787 --host 127.0.0.1

# Equivalente vía variable de entorno (para un contenedor/orquestador que
# no siempre controla el comando exacto).
LINK_HOST=127.0.0.1 linkc serve app.link 8787
```

**`--host`/`LINK_HOST`, mismo orden de precedencia que el resto de los flags de `serve` (flag primero, después la env var, después el default).** Sin ninguno de los dos: `"0.0.0.0"`, el comportamiento de siempre -- no rompe a nadie que no pida esto explícitamente, y sigue siendo el valor correcto para el `ENTRYPOINT` que `linkc docker` genera, donde el proceso corre en su propio namespace de red de contenedor y `0.0.0.0` ahí adentro es exactamente lo que hace falta para que el mapeo de puertos del host funcione.

**El valor se pasa tal cual a `tiny_http::Server::http((host, puerto))` -- sin resolución ni validación propia.** Cualquier forma que esa llamada acepte (una IP, `"localhost"`, un hostname que resuelva) funciona; una dirección que no le pertenece a ninguna interfaz de la máquina hace que el bind falle al arrancar, con un mensaje que nombra la dirección y el puerto exactos -- nunca cae en silencio a `0.0.0.0`. La única validación propia de `linkc` es rechazar `--host ""` (vacío) antes de intentar el bind, con un mensaje de uso claro en vez de un error de bind confuso.

**Límites honestos:**
- **Todo o nada por proceso, igual que el puerto.** No hay forma de escuchar en más de una interfaz puntual a la vez (ej. loopback + una IP interna, pero no `0.0.0.0`) -- son las opciones que `tiny_http`/el sistema operativo ya exponen: una dirección concreta, o todas.
- **`linkc dev` no expone este flag todavía.** Reinvoca `linkc serve <archivo> <puerto>` como proceso hijo sin pasar ningún flag adicional (tampoco `--db`/`--cors-origin`/etc. hoy) -- mismo alcance que el resto de esos flags en modo desarrollo.

**Verificado**: `cli_host.rs` con el binario real como subproceso (7 tests) -- el default sigue aceptando una conexión por loopback, `--host 127.0.0.1` y `LINK_HOST=127.0.0.1` sirven igual por loopback, una dirección que no le pertenece a ninguna interfaz local (`192.0.2.1`, TEST-NET-1 de RFC 5737, para no depender de que la máquina de test tenga una segunda interfaz real) hace fallar el arranque nombrando esa dirección en el mensaje -- probando así que el valor de verdad se usa para bindear y no se ignora en silencio --, el flag le gana a la variable de entorno, y tanto `--host` sin valor como `--host ""` son errores de uso limpios, sin panic.

---

### 3.82 `linkc test --filter <nombre>` — RESUELTO

Hasta esta ronda, `linkc test archivo.link` siempre corría TODOS los bloques `test "..." { ... }` del programa -- confirmado leyendo `cmd_test`, no había ningún flag para acotar la corrida a uno solo. Para un archivo con decenas de tests, iterar sobre UNO que está fallando (o que se está escribiendo recién) significaba esperar a que todos los demás corrieran también en cada vuelta.

<!-- linkc:check -->
```rust
type User = { id: Int, name: String }
db { users: User[] }

service Users {
  rpc create(name: String) -> User {
    db.users.insert(User { id: 0, name: name })
  }
}

test "crear usuario exitoso" {
  let u = Users.create("Ada");
  assert(u.name == "Ada");
}

test "actualizar usuario exitoso" {
  assert(true);
}

test "borrar item" {
  assert(true);
}
```

```bash
linkc test app.link --filter usuario
# running 2 tests (filtro: 'usuario')
# test result: ok. 2 passed; 0 failed
#
# "borrar item" ni siquiera se menciona -- no matchea el filtro, así que
# no corre.
```

**`--filter <nombre>`: substring sobre el NOMBRE del test, sensible a mayúsculas -- mismo criterio que `cargo test <substring>`, no un nombre exacto ni una regex.** Cualquier test cuyo nombre CONTENGA ese texto corre; el resto ni se ejecuta ni se menciona en la salida. Un filtro que no matchea ningún nombre corre cero tests -- termina con éxito (`test result: ok. 0 tests run`), no es un error: mismo criterio que un filtro de `cargo test` que no matchea nada.

**Solo aplica al test runner INTEGRADO (`test "..." { ... }`), nunca al testing de contrato (`linkc test archivo.link archivo.snap`).** Ese segundo modo compara el `contract.d.ts`/`client.ts`/`validators.ts` completos contra un snapshot -- no tiene nombres de test que filtrar, así que combinar `--filter` con un path de snapshot es un uso confuso, rechazado con un mensaje claro en vez de ignorado en silencio.

**Verificado**: 1 test en `runtime/mod.rs` (`run_program_tests_filtered`: un filtro que matchea un subconjunto corre solo esos, uno que no matchea nada corre cero sin fallar, `None` corre TODOS -- idéntico a `run_program_tests` sin filtro) y 5 en `cli_test_runner.rs` con el binario real (filtra por substring, substring parcial también matchea, cero coincidencias termina limpio, `--filter` sin valor y `--filter` combinado con un path de snapshot son errores de uso claros, sin panic).

---

### 3.83 `linkc --version` y versión estampada en cada archivo generado — RESUELTO

Hasta esta ronda, `linkc` no tenía NINGUNA forma de reportar su propia versión -- confirmado leyendo `main.rs`, ni `--version` ni `-v` ni `version` estaban despachados en ningún lado, y ningún archivo que `linkc build` genera decía con qué versión del compilador se había generado. Para un equipo donde conviven varias versiones del compilador en el tiempo (una máquina de CI recién actualizada, un desarrollador con un binario viejo en el `PATH`), un `gen/` desactualizado no tenía ninguna forma de detectarse por sí solo -- había que confiar en que quien lo generó se acordara de anotarlo en algún lado aparte.

```bash
linkc --version
# linkc 1.48.0
linkc -v        # equivalente
linkc version   # equivalente
```

**`linkc::VERSION` es `env!("CARGO_PKG_VERSION")` -- tomada de `Cargo.toml` en tiempo de COMPILACIÓN, nunca un string hardcodeado aparte que alguien podría olvidarse de actualizar en un release.** La misma constante alimenta `linkc --version` Y el header de cada archivo que `linkc build` genera, así que las dos lecturas nunca pueden desincronizarse entre sí -- si difieren, es porque se está comparando la salida de DOS binarios distintos, que es justamente lo que este ítem existe para poder detectar.

**Cada archivo TypeScript generado (`contract.d.ts`/`client.ts`/`hooks.ts`/`validators.ts`/`schemas.ts`) queda con la versión en su primera línea:**

```typescript
// Generado automáticamente por linkc v1.48.0 — no editar a mano.
```

**`openapi.json` -- que no admite comentarios `//` -- lleva la misma información en `x-generated-by`, una extensión de VENDOR estándar de OpenAPI (prefijo `x-`, cualquier herramienta que no la reconozca la ignora sin romper la validación del documento).** Deliberadamente NO se reusa `info.version` para esto -- ese campo es la versión del API que el propio `.link` documenta (algo que decide quien lo escribe, sin relación con qué versión del compilador lo generó), reusarlo mezclaría dos conceptos distintos bajo el mismo campo.

**Límites honestos:**
- **Es un COMENTARIO/campo informativo, no una restricción que `linkc build`/`linkc serve` verifiquen.** Nada compara la versión estampada en un `gen/` viejo contra la versión del binario que lo está sirviendo o reconstruyendo -- sirve para que una PERSONA lo note al mirar el archivo, no para que el compilador rechace un `gen/` desactualizado.
- **`link.lock` tiene su propio `version` -- un número de FORMATO del lockfile (hoy `1`), no la versión del compilador.** Las dos cosas conviven sin relación: el lockfile versiona su propio schema, este ítem versiona el ARTEFACTO generado.

**Verificado**: `cli_help.rs` (`--version`/`-v`/`version` imprimen exactamente `linkc <versión>` a stdout, código de salida 0, nada por stderr -- comparado contra `env!("CARGO_PKG_VERSION")` leído en el propio test, así que una desincronización real haría fallar el test) y 4 tests en `codegen/*.rs` (`contract.d.ts`/`client.ts`/`hooks.ts`, `validators.ts` y `schemas.ts` empiezan con el header versionado; `openapi.json` lleva `x-generated-by` con la versión, y `info.version` sigue siendo la del API, no la del compilador -- las dos NO deben coincidir).

---

### 3.84 `auth.destroyAllSessions(userId)`: revocar todas las sesiones de un usuario — RESUELTO

Hasta esta ronda, la única forma de cerrar una sesión era `auth.destroySession()` -- que opera sobre la sesión que ya autenticó la request ACTUAL, deliberadamente sin tomar ningún token como argumento (§3.14: si tomara un token, cualquiera podría destruir la sesión de otro con solo adivinar/conocer ese string). Eso deja sin resolver el caso real de "un usuario cambió su contraseña, o un admin lo está baneando -- hay que cerrar TODAS sus sesiones abiertas, en todos los dispositivos, ahora mismo", que no tiene forma de expresarse con un método que solo conoce "la sesión actual".

<!-- linkc:fragment -->
```rust
enum Role { Admin, Member }

service Admin {
  @requires(Role.Admin)
  rpc banUser(userId: Int) -> Int {
    auth.destroyAllSessions(userId)
  }
}
```

**`auth.destroyAllSessions(userId: Int) -> Int`: a diferencia de `destroySession`, SÍ toma un identificador explícito.** Mismo criterio que `createSessionWithId` (§3.53): un `userId` es una clave de aplicación, no un secreto adivinable como un token de sesión -- no hay el mismo riesgo que motivó a `destroySession` a no tomar ningún argumento. Devuelve la CANTIDAD de sesiones borradas (`0` si el usuario no tenía ninguna sesión abierta, nunca un error).

**Quién puede LLAMAR a esto es responsabilidad de quien escribe el `.link`, el método en sí no impone ninguna política.** Como cualquier otro builtin de `auth` (`createSession`/`createSessionWithId`), está disponible desde CUALQUIER cuerpo de rpc -- gatearlo con `@requires(Role.Admin)` (como en el ejemplo de arriba) es una decisión del autor del programa, no algo que el runtime fuerce por sí solo.

**Solo alcanza sesiones creadas por ESTE `SessionStore` (`createSession`/`createSessionWithId`) -- un JWT externo (§3.64) no pasa por acá.** Un JWT válido nunca se guarda en el store en memoria (se verifica al vuelo en cada request), así que no hay nada que "borrar" del lado de `linkc` -- revocar un JWT externo sigue siendo responsabilidad del sistema que lo emitió (rotar el secreto, o su propia lista de revocación).

**Verificado**: 3 tests en `session.rs` (borra todas las sesiones de un usuario y devuelve la cantidad exacta, deja intactas las de otro usuario, un usuario sin sesiones da `0` sin tocar nada, una sesión creada sin `userId` nunca matchea), 1 en `checker.rs` (toma exactamente un `Int`, tipa `Int`) y 1 en `runtime/mod.rs` contra `invoke_rpc_with_sessions` (dos sesiones del mismo usuario se revocan, una tercera de otro usuario sobrevive). Verificado también contra un servidor HTTP real (`server_http.rs`): dos tokens del mismo usuario dejan de autenticar (401, mismo código que cualquier token inexistente o vencido) después de `destroyAllSessions`, mientras el token de otro usuario sigue funcionando sin cambios.

---

### 3.85 `--max-body-bytes <N>`: límite de tamaño del body de una request — RESUELTO

Hasta esta ronda, `linkc serve` leía el body de CUALQUIER request entero a memoria antes de tocarlo -- `request.as_reader().read_to_string(&mut body)`, sin ningún límite -- confirmado leyendo `runtime/server.rs`. Ni auth, ni rate limiting, ni la forma del JSON tenían oportunidad de rechazar nada ANTES de esa lectura completa: un solo body enorme (a propósito o no) era un vector real de agotamiento de memoria del proceso entero, sin ninguna forma declarativa de acotarlo.

```bash
linkc serve app.link 8787 --max-body-bytes 1000000   # 1 MB
LINK_MAX_BODY_BYTES=1000000 linkc serve app.link 8787  # equivalente
```

**`--max-body-bytes`/`LINK_MAX_BODY_BYTES`, mismo orden de precedencia que el resto de los flags de `serve` -- default 10 MiB (`10 * 1024 * 1024` bytes) sin ninguno de los dos.** Un entero PLANO de bytes, sin sufijos de unidad (`"1000000"`, no `"1mb"`) -- mismo criterio que `--argon2-memory-kib`. El default es un número razonable, no exhaustivamente investigado: generoso para un body JSON real (incluido uno con algún campo `String` grande en base64), acotado para no dejar el proceso expuesto a un body sin límite.

**Un body que supera el límite se rechaza con `413 Payload Too Large`, sin leerlo completo primero.** La lectura usa `Read::take(max_body_bytes + 1)` -- el `+1` es lo que distingue "el body mide EXACTO el límite" (permitido) de "sigue después del límite" (rechazado), sin necesitar leer más de un byte de más en ningún caso. El rechazo ocurre ANTES de cualquier otro chequeo (auth, rate limit, parseo del JSON) -- el punto entero es no dejar que ninguno de esos pasos compita por memoria con un body que ya se sabe demasiado grande.

**Límites honestos:**
- **No se drena el resto de un body rechazado.** Tras responder `413`, los bytes del body que todavía no se leyeron quedan sin consumir en el socket -- si el cliente intenta REUSAR la misma conexión (`keep-alive`) para otra request, el servidor intentará parsear esos bytes viejos como una línea de request nueva, fallará (`400`), y cerrará la conexión. Nunca un colgado ni una fuga de memoria -- en el peor caso, un `400` de más que el cliente no esperaba. Un cliente HTTP real que recibe un `413` normalmente no sigue mandando por la misma conexión sin haber leído la respuesta primero, así que este caso es infrecuente en la práctica.
- **Es un límite de PROCESO, no por ruta ni por rpc.** No hay forma de darle a un rpc puntual (uno que de verdad necesita bodies grandes, ej. subir un archivo codificado en base64) un límite distinto al resto.
- **`linkc dev` no expone este flag todavía**, mismo alcance que `--host`/el resto de los flags de `serve` en modo desarrollo.

**Verificado**: `cli_max_body.rs` con el binario real como subproceso (9 tests) -- un body bajo el default se acepta, un body EXACTO al límite configurado se acepta, uno de un byte más se rechaza con `413` nombrando el límite en el mensaje, un body mucho más grande (2 MiB contra un límite de 1000 bytes) también se rechaza -- probando que la lectura se corta temprano, no que se lee entero y se rechaza después --, el flag y la variable de entorno funcionan por separado y el flag le gana a la env var, `--max-body-bytes` con un valor no numérico o sin valor son errores de uso limpios sin panic, y los headers de seguridad/CORS siguen presentes en la respuesta `413`.

---

### 3.86 `--http-timeout <duración>`: timeout de llamadas salientes `http.*` — RESUELTO

Auditando `runtime/mod.rs` apareció que `http.get`/`post`/`getWithHeaders`/`getWithStatus`/`postWithStatus`/`postWithHeaders` (GRAMMAR.md §3.47/§3.60) llamaban a `ureq::get`/`ureq::post` sin fijar NINGÚN timeout propio. `ureq` (la crate) sí trae un timeout de CONEXIÓN por default (30s) -- pero el de LECTURA/ESCRITURA, el que importa una vez que la conexión ya abrió, es "nunca" por default, documentado así por la propia crate. Cuando esto se escribió, el intérprete era de un solo hilo (GRAMMAR.md §3.13), así que eso significaba que una request saliente a un servidor lento, o que acepta la conexión y después simplemente no responde nunca, bloqueaba el proceso ENTERO para siempre: ninguna otra request, de ningún cliente, se atendía mientras tanto. Ni siquiera `/health`. **Actualizado (26/08/2026, GRAMMAR.md §3.158): con un hilo real por request, ese `http.*` colgado solo bloquea el hilo de ESA request -- salvo dentro de un `transaction{}` (o, desde el mismo día, un `upsert`), donde sigue sosteniendo el candado exclusivo de la conexión y por lo tanto sigue bloqueando a las demás.** El timeout configurable sigue siendo la defensa real de todas formas -- un hilo colgado para siempre, aunque ya no tumbe TODO el servidor, sigue siendo un leak de recursos que ningún proceso de larga vida debería aceptar sin límite.

```bash
linkc serve app.link 8787 --http-timeout 10s
LINK_HTTP_TIMEOUT=10s linkc serve app.link 8787   # equivalente
```

**`--http-timeout`/`LINK_HTTP_TIMEOUT`, mismo orden de precedencia que el resto de los flags de `serve` -- mismo formato `Ns`/`Nm`/`Nh`/`Nd` que `--session-ttl` (`parse_duration`), default 30 segundos sin ninguno de los dos.** El default es el mismo número que `ureq` ya usaba para conexión -- no un valor nuevo inventado, solo aplicado también a lectura/escritura, que es la parte que faltaba. Vive en `Db` (`http_timeout: RefCell<Duration>`), fijado UNA vez al arrancar -- mismo mecanismo exacto que `argon2_params` (GRAMMAR.md §3.58): `db: &Db` ya está disponible en cualquier punto del árbol de evaluación, así que no hace falta enhebrar un parámetro nuevo por `call_method`/`eval_expr`/etc.

**Un timeout agotado se reporta como cualquier otro error de red -- 500 de runtime, nunca un panic ni un colgado.** `ureq::Error` ya distinguía un timeout de una conexión rechazada o un DNS que no resuelve; el mensaje de error que llega al cuerpo del rpc simplemente nombra la causa real que `ureq` reporta.

**Límites honestos:**
- **Un solo timeout total, no timeouts separados de conexión/lectura/escritura configurables por separado.** `ureq::Request::timeout(...)` fija el límite TOTAL de la operación -- suficiente para el problema real (que el intérprete nunca se bloquee indefinidamente), pero no permite, por ejemplo, un timeout de conexión más corto que el de lectura.
- **Sin reintentos en `get`/`post`/`getWithHeaders`/`postWithHeaders`/`getWithStatus`/`postWithStatus`.** Un timeout (o cualquier otro error de red) falla esa llamada de inmediato -- ninguno de estos seis reintenta solo. **Actualizado (27/08/2026): `http.postWithRetry` (GRAMMAR.md §3.160) cierra este gap para POST -- con backoff exponencial fijo, sin ningún flag/env var nuevo.** Un `GET` con retry (`getWithRetry`) queda sin atacar, sin evidencia real de demanda todavía -- el caso real que motivó esto (PLAN.md §9.4, webhooks salientes) es intrínsecamente un POST.
- **Es un límite de PROCESO, no por rpc ni por URL.** Un `rpc` que necesita hablar con un servicio genuinamente lento (y otro que necesita fallar rápido) comparten el mismo timeout.

**Verificado**: `cli_http.rs` con el binario real como subproceso (3 tests nuevos, sobre 7 que ya existían) -- una request a un servidor que ACEPTA la conexión pero nunca escribe nada corta cerca del `--http-timeout` configurado (medido con un `Instant` real, nunca cerca de los 60s que el servidor de mentira se queda callado), la variable de entorno funciona igual, y `--http-timeout` con una duración inválida es un error de uso limpio sin panic.

---

### 3.87 `/health` verifica conectividad real a la base — RESUELTO

Hasta esta ronda, `/health` (`/`/`/status` son el mismo handler) devolvía `200 {"status":"ok",...}` FIJO, sin tocar la base para nada -- confirmado leyendo `runtime/server.rs`. Inútil para cualquier orquestador (Kubernetes, un load balancer, `systemd` con un healthcheck) que lo usa para decidir si reiniciar el proceso o sacarlo de rotación: el proceso podía estar vivo (aceptando conexiones TCP) y sin embargo incapaz de servir NINGÚN rpc real porque la base estaba caída, y `/health` igual reportaba todo bien.

<!-- linkc:check -->
```rust
type Item = { id: Int, name: String }
db { items: Item[] }

service Items {
  rpc list() -> Item[] { db.items.all() }
}
```

```bash
curl -s localhost:8787/health
# {"status":"ok","engine":"c-script","version":"1.52.0","services":["Items"],"database":"ok"}
```

**`db.health_check()` (nuevo en `Db`) ejecuta un `SELECT 1` real contra la base en CADA request a `/health` -- sin caché: un health check que devuelve un resultado viejo no sirve para nada.** `Ok(())` → `200`, `Err(mensaje)` → `503 Service Unavailable`, con `"status": "error"` y `"database": "<mensaje>"` en el body. Del lado Postgres, el chequeo pasa por el MISMO `with_reconnect` (GRAMMAR.md §3.40) que cualquier otra query real -- una caída transitoria se autorepara ahí mismo, así que `/health` no solo reporta el estado, también participa de la reconexión automática.

**Límites honestos:**
- **Solo la BASE, no "servicios externos declarados".** c-script no tiene hoy ningún concepto declarativo de "este programa depende de esta API externa" -- `http.*`/`smtp.*` son llamadas libres desde cualquier cuerpo de rpc, no una dependencia que el checker conozca de antemano. Extender `/health` a eso necesitaría esa pieza primero.
- **Sin forma de saltear el chequeo.** No hay un flag para volver al `200` fijo de siempre -- se consideró innecesario: un `SELECT 1` es barato, y un health check que puede mentir por configuración no es lo que nadie quiere de default.
- **Un solo `SELECT 1`, no una verificación de que CADA colección declarada sea alcanzable.** Una base conectada pero con una tabla específica corrupta (fuera del control de este runtime) no se detecta acá -- eso ya fallaría en la primera lectura real de esa colección, con el error normal de esa request.

**Verificado**: `cli_health.rs` con el binario real como subproceso contra SQLite (2 tests) -- forma exacta del JSON (`status`/`engine`/`version`/`database`/`services`, listando los servicios declarados de verdad) en el camino feliz, y que `/`, `/health`, `/status` devuelven exactamente lo mismo. El camino de FALLA real se prueba en `pg_integration.rs` (reusando la misma técnica de `pg_terminate_backend` que el test de reconexión, GRAMMAR.md §3.40, solo contra un PostgreSQL real): `/health` pasa de `200`/`"ok"` a `503`/`"error"` mientras la conexión está cortada, y vuelve solo a `200` sin reiniciar el proceso -- sin necesitar ayuda humana, mismo criterio de auto-reparación que el resto de las queries.

---

### 3.88 Lint: comparación insegura de un secreto con `==` — RESUELTO

`crypto.timingSafeEqual` (GRAMMAR.md §3.54) existe justamente porque un `==` de `String` corta en el primer byte distinto -- comparar un token, contraseña o API key con el operador de siempre filtra, por cuánto tarda la comparación, cuánto de el acertó quien prueba. La función existe desde esa ronda, pero nada avisaba si el código de alguien seguía usando `==` sobre algo que PARECE un secreto -- había que saber que el problema existe y acordarse de buscarlo.

<!-- linkc:check -->
```rust
type LoginRequest = { token: String }

fn checkToken(req: LoginRequest, expected: String) -> Bool {
  req.token == expected
}
```

```bash
linkc lint app.link
# app.link:4:3: [timing-unsafe-secret-comparison] 'token' se compara con
# '==' -- si es un secreto (token/password/API key), usá
# crypto.timingSafeEqual(a, b) en vez de '==' para no filtrar cuánto
# acertó por el tiempo que tarda la comparación
```

**Nombre del operando, no tipo -- el lint corre sobre el AST crudo, antes (y sin necesitar) el checker.** `==`/`!=` donde CUALQUIERA de los dos lados es un `Ident` o el campo final de un `FieldAccess` cuyo nombre contiene (sin importar mayúsculas) `secret`, `token`, `password`, `apikey` o `api_key` -- deliberadamente laxo: mejor un falso positivo ocasional sobre un identificador con un nombre raro (`tokenCount`, por ejemplo) que dejar pasar el caso real que esta regla existe para atrapar.

**Comparar contra `null` queda afuera a propósito.** `token != null` es un chequeo de PRESENCIA ("¿hay sesión?"), no de VALOR -- no hay ningún byte de un secreto involucrado, así que no hay ningún canal lateral que cerrar ahí. Sin este descarte, cualquier `if token != null { ... }` (un patrón común y correcto) generaría ruido constante.

**Recorre TODO el cuerpo, no solo el nivel superior** -- adentro de un `if`/`match`/`while`/closure, en cualquier nivel de anidamiento. Encontrado auditando la implementación: la primera versión reusaba la recursión que `lint_block` ya hacía sobre el body de un `while` (para `unused-var`/`unused-mut`) y volvía a recorrer ESE MISMO bloque desde la nueva regla, duplicando cada warning que cayera adentro de un `while` -- corregido antes de este release, con un test de regresión dedicado que cuenta exactamente 1 warning, no 2.

**Puramente informativo, como el resto del linter -- `linkc lint` sigue saliendo con código 0 aunque encuentre esto.** No bloquea `linkc build`; es una recomendación, no una regla de compilación.

**Límites honestos:**
- **Nombre, no flujo de datos.** No rastrea si un valor que EMPEZÓ en un campo `token` terminó en una variable con otro nombre antes de compararse -- `let t = req.token; t == expected` no dispara la regla, porque `t` no tiene un nombre sospechoso. Un analizador de flujo de datos real es un proyecto aparte.
- **Sin `--fix` automático para esta regla.** A diferencia de `unused-var`/`unused-mut`, reescribir `a == b` como `crypto.timingSafeEqual(a, b)` a ciegas podría cambiar el TIPO de la expresión que lo rodea (`Bool` en los dos casos, pero el resto del código puede depender de que sea el operador `==` real, ej. dentro de un patrón más grande) -- se deja para que una persona lo revise.

**Verificado**: 7 tests en `lint.rs` (un `Ident`/`FieldAccess` con nombre sospechoso dispara la regla con `==` y con `!=`, comparar contra `null` NO dispara nada, dos nombres comunes tampoco, DENTRO de un `while`/`if`/closure se encuentra igual, y -- el test de regresión -- exactamente UNA vez adentro de un `while`, no duplicado) y 1 en `cli_fmt_lint.rs` contra el binario real (`linkc lint` imprime la regla y la recomendación de `timingSafeEqual`).

---

### 3.89 `--trust-proxy`: `@rate_limit` detrás de un proxy real — RESUELTO

`@rate_limit` (GRAMMAR.md §3.39) siempre identificó al cliente por `remote_addr()` -- la conexión TCP real -- deliberadamente, NUNCA por un header como `X-Forwarded-For` que cualquier cliente puede mandar con el valor que quiera. Correcto contra un cliente directo, pero detrás de un proxy o balanceador de verdad (nginx, un load balancer -- confirmado como bloqueo real en producción: la adopción de IgnisLove corre TODO detrás de nginx) `remote_addr()` es siempre la IP del proxy, la misma para cada request -- el límite termina siendo compartido por TODOS los usuarios reales a la vez, no por cada uno.

```bash
linkc serve app.link 8787 --trust-proxy
LINK_TRUST_PROXY=1 linkc serve app.link 8787   # equivalente
```

**`--trust-proxy`/`LINK_TRUST_PROXY`, apagado por default -- mismo criterio de flag booleano de presencia que `--adopt-existing`.** Prendido, `@rate_limit` usa el PRIMER valor de `X-Forwarded-For` (`cliente, proxy1, proxy2, ...` -- el primero es el más cercano al cliente original) en vez de `remote_addr()`. Sin el header presente (incluso con el flag prendido), cae de vuelta a `remote_addr()` tal cual -- no hay motivo para tratar eso como "cliente desconocido".

**Es un opt-in explícito a propósito -- prenderlo sin tener de verdad un proxy de confianza delante deja que cualquier cliente directo evada el límite por completo**, mandando un `X-Forwarded-For` distinto en cada request. La responsabilidad de que el header LLEGUE confiable (que el proxy real lo sobreescriba, en vez de dejar pasar el que mandó el cliente original) es de la configuración de ESE proxy, no de `linkc`.

**Límites honestos:**
- **Sin validación de CUÁNTOS proxies hay en el medio, ni de qué IP vienen.** No hay un mecanismo más fino de "confío en estos N saltos" o "confío en este rango CIDR" (lo que Express llama `trust proxy: n` o una lista de IPs) -- v0 confía en el header COMPLETO en cuanto el flag está prendido. Suficiente para el caso real que motivó esto (un solo proxy de confianza justo delante), no para una cadena de proxies con distintos niveles de confianza.
- **Solo afecta a `@rate_limit`.** `request.header("X-Forwarded-For")` (GRAMMAR.md §3.38) sigue devolviendo el header CRUDO tal cual llegó, sin ningún procesamiento -- este flag no cambia nada de lo que un cuerpo de rpc puede leer directamente.

**Verificado**: `cli_rate_limit.rs` con el binario real como subproceso (5 tests nuevos, sobre 3 que ya existían) -- sin `--trust-proxy`, `X-Forwarded-For` con valores distintos NO separa el balde (todo cuenta contra el mismo límite, probando que el header se ignora); con `--trust-proxy`, cada IP reenviada distinta tiene su propio balde, y el PRIMER hop de una cadena `cliente, proxy1, proxy2` es el que se usa; con `--trust-proxy` pero SIN el header, cae de vuelta a `remote_addr()` sin romper nada; y `LINK_TRUST_PROXY` funciona igual que el flag.

---

### 3.90 `dateFromParts(...)`: construir un `Timestamp` arbitrario — RESUELTO

Encontrado auditando un reporte de adopción real (MyFinance, backend de cálculo de Modelos tributarios 130/303/347): §3.31 documentaba, a propósito, que un `Timestamp` v0 "solo puede llegar como parámetro de un `rpc` desde el cliente, o ya estar guardado en `db`" -- `now()` (§3.32) cerró el caso "el instante ACTUAL", pero construir una fecha ARBITRARIA (ej. "el 1 de enero de 2026", el límite de un trimestre) seguía siendo imposible desde adentro de un `rpc`. Un cálculo que depende de un rango de fechas -- el caso de Modelo 130/303, que necesita el inicio y el fin de un trimestre a partir de `año`/`trimestre` -- no tenía forma de escribirse enteramente en el backend.

<!-- linkc:check -->
```rust
service Impuestos {
  rpc inicioDeTrimestre(anio: Int, trimestre: Int) -> Timestamp {
    let mes = (trimestre - 1) * 3 + 1;
    dateFromParts(anio, mes, 1, 0, 0, 0)
  }
}

test "el trimestre 3 de 2026 empieza el 1 de julio" {
  let t = Impuestos.inicioDeTrimestre(2026, 3);
  assert(t == dateFromParts(2026, 7, 1, 0, 0, 0));
}
```

**`dateFromParts(year: Int, month: Int, day: Int, hour: Int, minute: Int, second: Int) -> Timestamp`: builtin sin receptor, mismo mecanismo exacto que `now()` (`checker.rs`/`runtime/mod.rs` reconocen el nombre de forma especial, no una `fn` de usuario).** Los 6 argumentos son obligatorios -- sin sobrecarga ni valores por defecto para "solo la fecha" (medianoche implícita se escribe pasando `0, 0, 0` a mano). Milisegundos siempre en `.000` -- sin un séptimo parámetro para eso, fuera de alcance de esta ronda. Reusa `parse_iso8601_millis` (el mismo parser/validador que ya existía para un `Timestamp` que llega por el wire) armando el string ISO-8601 internamente, en vez de reimplementar la validación de calendario -- un solo lugar decide qué fecha "existe de verdad".

**Una fecha inválida es `bad_request` (400), no un panic ni un 500.** `month`/`day`/`hour`/`minute`/`second` fuera de rango (mes 13, hora 25) Y un día que no existe DENTRO de un mes válido (30 de febrero) se rechazan igual -- es información que vino de datos del propio programa (`año`/`trimestre` como parámetros de rpc, por ejemplo), así que un valor mal armado es responsabilidad de quien llama, no un bug del servidor. El mensaje nombra CUÁL campo está mal.

**Límites honestos:**
- **Siempre UTC, sin zona horaria.** Mismo criterio que el resto de `Timestamp` (§3.31) -- no hay forma de pedir "medianoche en Madrid" directamente, hay que convertir a UTC antes de llamar.
- **Año limitado a 0-9999 (4 dígitos).** Misma restricción que ya tenía `parse_iso8601_millis` para un `Timestamp` que llega por el wire -- consistente en los dos sentidos, no un límite nuevo.
- **Sin aritmética de fechas todavía** (sumar/restar días, "el trimestre siguiente"). `dateFromParts` construye un punto fijo; calcular relativo a otro `Timestamp` sigue sin un tipo `Duration` (mismo límite que documenta §3.31).

**Verificado**: 6 tests en `runtime/timestamp.rs` (coincide con el ISO-8601 equivalente, hora completa, rechaza un día que no existe nombrando la fecha, rechaza cada campo fuera de rango nombrándolo, fechas antes de 1970, y el caso real -- construir y comparar el límite de un trimestre), 1 en `checker.rs` (6 `Int` obligatorios, tipa `Timestamp`, referenciable como valor de primera clase igual que `now`) y 3 en `runtime/mod.rs` contra un servidor real vía `invoke_rpc` (el cálculo de trimestre completo end-to-end, uso como valor de primera clase, y una fecha inválida como 400 nombrando la fecha exacta).

---

### 3.91 `Timestamp` decodifica `date`/`timestamp`/`timestamptz` nativos de Postgres — RESUELTO

La otra mitad del mismo reporte de adopción real (MyFinance): las tablas YA EXISTENTES de un sistema que se adopta casi siempre tienen sus columnas de fecha en el tipo NATIVO de Postgres (`date`/`timestamp`/`timestamptz`), no en el `BIGINT` de milisegundos que `linkc build` genera para un `Timestamp` propio (§3.31). Auditando el código real (`runtime/store.rs`) apareció que esto estaba genuinamente ROTO en los dos sentidos, no solo sin probar:

- **Declarado como `String`** (lo que `linkc introspect`, §3.66, hacía automáticamente hasta esta ronda, con una advertencia): compilaba bien, pero la PRIMERA fila real fallaba al leer -- el wire binario de un `timestamp`/`date` de Postgres no es texto UTF-8, así que ningún `String` puede decodificarlo.
- **Declarado como `Timestamp`**: TAMBIÉN fallaba -- `postgres_int_cell` (la función que decodifica un `Timestamp`/`Int`/`Int64` del lado Postgres) solo sabía leer enteros de 8/4/2 bytes (`BIGINT`/`INTEGER`/`SMALLINT`), y el OID de un `timestamp`/`date` nativo no matchea NINGUNO de los tres -- `postgres` (la crate) rechaza la lectura si el OID de la columna no coincide EXACTO con lo que el tipo Rust pedido acepta, sin importar que el ancho en bytes coincida por casualidad.

<!-- linkc:fragment -->
```rust
// Tabla YA EXISTENTE, con columnas de fecha NATIVAS de Postgres --
// típicamente vía `--adopt-existing` (§3.67):
//   CREATE TABLE facturas (
//     id BIGSERIAL PRIMARY KEY,
//     fecha_emision date NOT NULL,
//     created_at timestamptz NOT NULL
//   );
type Factura = { id: Int, fechaEmision: Timestamp, createdAt: Timestamp }
db { facturas: Factura[] }

service Facturas {
  rpc list() -> Factura[] { db.facturas.all() }
}
```

**`ColumnKind::Timestamp`, nuevo -- distinto de `ColumnKind::Int` (que sigue siendo solo para `Int`/`Int64`).** Del lado Postgres (`postgres_timestamp_cell`, `runtime/store.rs`), un campo `Timestamp` prueba EN ORDEN: `BIGINT` (la convención propia de c-script, primero porque es el caso más común para una tabla que `linkc build` creó), después `timestamp`/`timestamptz` nativo (microsegundos desde el epoch de Postgres, 2000-01-01 -- IDÉNTICO en las dos variantes, la diferencia "with/without time zone" es de FORMATEO en texto, nunca de representación binaria), después `date` nativo (días, no microsegundos, desde el mismo epoch). Del lado SQLite, `ColumnKind::Timestamp` se comporta exactamente igual que `Int` -- SQLite no tiene un tipo temporal nativo separado, así que no hay ninguna ambigüedad que resolver ahí.

**Decodificado a MANO, sin sumar la dependencia `chrono`.** `postgres`/`postgres-types` no ofrece un `FromSql` para tipos temporales sin esa dependencia. Se implementó `FromSql` para dos structs locales (`PgTimestampMicros`/`PgDateDays`) que leen el binario CRUDO del wire de Postgres (8 bytes big-endian para timestamp/timestamptz, 4 para date, el formato que el propio protocolo de Postgres documenta) -- mismo espíritu que el algoritmo de calendario de Hinnant que ya vivía en `runtime/timestamp.rs`: un formato binario chico y bien definido no amerita una dependencia nueva. El offset entre los dos epochs (2000-01-01 de Postgres, 1970-01-01 de c-script/Unix) se calcula con el mismo `days_from_civil` que el resto del archivo, no un número mágico suelto.

**`linkc introspect` (§3.66) ahora mapea `date`/`timestamp`/`timestamptz` a `Timestamp` SIN advertencia -- mapeo exacto, ya no un placeholder "revisar a mano".** Antes de esta ronda recomendaba `String` con una advertencia -- una recomendación que en los hechos estaba rota, porque ni `String` ni `Timestamp` decodificaban. `time` (sin fecha) sigue sin mapeo exacto y sigue emitiendo `String` con advertencia -- un `Timestamp` de c-script es un instante completo (fecha + hora), no le cabe una hora suelta sin fecha.

**Límites honestos:**
- **Solo LECTURA.** Esta ronda resuelve `SELECT`/decodificación -- `insert`/`applyPatch` sobre un campo `Timestamp` siguen escribiendo `BIGINT` sin importar el tipo físico real de la columna, así que ESCRIBIR contra una columna `date`/`timestamp` nativa adoptada (en vez de solo leerla) sigue sin funcionar. No era parte del caso real reportado (MyFinance solo necesita LEER fechas de facturas ya existentes, nunca crearlas desde c-script) -- queda trackeado para una ronda aparte si aparece la necesidad.
- **Microsegundos truncados a milisegundos**, no redondeados -- mismo límite de precisión que el resto de `Timestamp` (§3.31).
- **`uuid` nativo de Postgres queda FUERA de esta ronda a propósito.** Auditando este mismo código apareció la misma forma de problema potencial (`Type::Uuid`/`String` decodificando contra el OID de un `uuid` nativo, nunca verificado) -- `linkc introspect` lo señala en su advertencia, pero no se tocó: esta ronda se acotó al caso confirmado y reportado (fechas), no a auditar cada tipo nativo de Postgres de una sola vez.

**Verificado**: 6 tests en `runtime/timestamp.rs` contra los DOS epochs de forma independiente (la constante del offset coincide con el algoritmo de calendario, un ancla pública conocida -- 2000-01-01 en milisegundos-desde-1970 -- para `timestamp` y para `date`, precisión truncada a milisegundos, un valor negativo antes del epoch de Postgres). 2 en `introspect.rs` (mapeo exacto sin advertencia para `date`/`timestamp`/`timestamptz`, `time` sigue advirtiendo). 1 en `pg_integration.rs` contra un PostgreSQL real: una tabla creada y sembrada con SQL crudo (`date`/`timestamptz`/`timestamp` nativos, nunca escritos por c-script), adoptada con `--adopt-existing` declarando los tres campos como `Timestamp`, decodifica la fila real correctamente vía un rpc real. Más un test end-to-end que extiende `introspect_generates_a_link_file_that_actually_works_against_the_real_table`: `linkc introspect` sobre una tabla con una columna `date` real genera `Timestamp` sin advertencia, y el `.link` generado (sin tocar a mano) lee la fila real con la fecha correcta.

---

### 3.92 `linkc serve-all` + `--restart-backoff`: un proceso para varios servicios — RESUELTO

Reporte de adopción real (IgnisLove): 13-17 `.link` desplegados como 13-17 procesos `pm2` SEPARADOS, uno por servicio -- cada uno con su propio puerto, su propio archivo SQLite, y su propia línea en el script de deploy. Un incidente confirmado en producción: un arranque en frío donde varios de esos procesos compiten por bindear sus puertos casi al mismo tiempo, alguno pierde la carrera, `pm2` lo reinicia con un `--restart-delay` fijo (una capa por completo AFUERA del lenguaje) -- 68 reinicios de un solo servicio (`telemetry`) antes de estabilizarse.

```bash
# Antes: 13 procesos pm2 separados, uno por .link, cada uno con su
# propio --restart-delay como mitigación externa.

# Ahora: un proceso sirve TODOS los .link de un directorio.
linkc serve-all ./servicios --port-base 3000 --host 127.0.0.1 --restart-backoff 1s
#   servicios/facturacion.link -> http://localhost:3000
#   servicios/inventario.link  -> http://localhost:3001
#   servicios/telemetry.link   -> http://localhost:3002
#   ...
```

**`linkc serve-all <directorio> --port-base N`: descubre cada `.link` DIRECTO dentro de `<directorio>` (no recursivo), los compila TODOS antes de arrancar nada, y arranca uno por hilo del sistema operativo -- un único proceso, un único PID, una única línea de deploy.** El puerto de cada servicio es `N` más su posición en orden ALFABÉTICO de nombre de archivo (determinístico entre corridas del MISMO directorio con los MISMOS archivos, impreso explícitamente al arrancar -- ver "Límites honestos"). Cada hilo es independiente de los demás: `Value`/`Db`/`Program` (GRAMMAR.md §3.10, closures con `Rc<RefCell<_>>`) nunca cruzan un borde de hilo, así que no hace falta ni `Arc` ni ningún tipo de sincronización entre servicios -- cada uno vive enteramente en el suyo, exactamente como si fuera su propio proceso, solo que sin la sobrecarga de UN PROCESO DEL SISTEMA OPERATIVO por servicio.

**Aislamiento de datos preservado -- solo el conteo de PROCESOS colapsa.** Cada servicio sigue leyendo/escribiendo su propio `<archivo>.db` (el mismo default que `linkc serve` sin `--db`) -- por eso `serve-all` RECHAZA `--db`/`LINK_DATABASE_URL` compartido de entrada, con un mensaje explícito: apuntar varios `.link` de distinto schema a la MISMA base es exactamente el escenario de colisión de nombre de tabla que este proyecto todavía no tiene forma de detectar (`--db-schema`/`--db-prefix`, sin implementar) -- rechazarlo de una es más honesto que aceptarlo y arriesgar que la tabla de un servicio pise la de otro en silencio.

**Compilación atómica: TODOS los `.link` se tipan antes de arrancar CUALQUIER hilo.** Un workspace a medio levantar (12 de 13 servicios sanos, uno ni siquiera compiló) es peor que no levantar ninguno -- un error de tipos en cualquier archivo aborta el comando entero, con el mismo reporte de error (snippet + caret) que `linkc build`/`linkc serve` de siempre.

**Un servicio caído (bind ocupado, Postgres abajo al arrancar) YA NO SE LLEVA A LOS DEMÁS POR DELANTE.** Antes de esta ronda, `runtime::server::serve` resolvía los dos fallos así: un bind fallido con `panic!`, una conexión a Postgres fallida con `std::process::exit(1)` -- cualquiera de los dos, dentro de UN proceso por servicio, solo mataba a ESE servicio (lo que `pm2` reiniciaba). Pero un `process::exit` tumba el PROCESO ENTERO, no el hilo -- si `serve-all` no lo hubiera cambiado, un Postgres caído en UN servicio se habría llevado puesto TODO el workspace. Ahora `serve` devuelve `Result<(), String>` -- `linkc serve` (un solo servicio) preserva el comportamiento de siempre (el fallo termina el proceso, código 1, delegando el reintento a quien orqueste, como antes), y `serve-all` nunca termina el proceso por un solo servicio: lo loguea con su nombre de archivo como prefijo (varios servicios comparten un mismo stdout/stderr) y sigue con los demás.

**`--restart-backoff <duración>`/`LINK_RESTART_BACKOFF`: backoff exponencial NATIVO ante ese mismo fallo, reemplazando la mitigación externa (`pm2 --restart-delay`, una espera FIJA siempre igual).** Sin el flag: un solo intento, igual que siempre (delega en quien orqueste el proceso). Con el flag, `<duración>` es la espera BASE -- se DUPLICA en cada fallo consecutivo hasta un techo de 30s, reseteada a la base después de 60s de funcionamiento estable (para que una racha vieja de fallos, ej. un arranque en frío con varios puertos disputados, no siga penalizando a un servicio que ya está sano). Funciona igual en `linkc serve` (un solo servicio bajo `pm2`/`systemd`, sin migrar a `serve-all`) y en `linkc serve-all` (cada hilo reintenta el suyo, de forma independiente).

**Límites honestos:**
- **Asignación de puerto por orden alfabético, no fijada por config -- salvo que se pida lo contrario.** Sin `--port-registry` (§3.153), agregar, quitar o renombrar un `.link` en el directorio corre el riesgo de reasignar los puertos de TODOS los servicios que vienen después alfabéticamente -- por eso `serve-all` imprime la asignación exacta (`archivo -> puerto`) en cada arranque, para que quien opere el gateway/proxy la vea en cada deploy. Con `--port-registry <archivo.json>`, cada nombre de servicio conserva SIEMPRE el mismo puerto entre corridas, sin importar qué otro `.link` se agregue o borre alrededor -- ver §3.153.
- **Todos los servicios comparten el mismo proceso del sistema operativo -- y por lo tanto el mismo entorno.** `--jwt-secret`/`LINK_JWT_SECRET` y el resto de los flags/env vars son GLOBALES a la corrida entera: no hay forma de darle a un servicio un secreto JWT distinto del resto bajo `serve-all` (si hace falta, ese servicio sigue necesitando su propio `linkc serve`). Mismo criterio para `--host`/`--cors-origin`/`--session-ttl`/etc.
- **`--restart-backoff` cubre fallos que `serve` devuelve como `Result` (bind ocupado, conexión inicial a Postgres) -- no un panic durante el manejo de una request.** Un panic real (un bug, no una condición operativa esperada) sigue matando solo AL HILO de ese servicio (Rust no aborta el proceso entero por un panic en un hilo que no es el principal), pero `serve-all` no lo reintenta automáticamente en v0 -- ese servicio queda caído hasta un restart manual del proceso completo. Retomar un panic con `catch_unwind` para reintentarlo también queda fuera de esta ronda a propósito: confundir "falla esperada" con "hay un bug" en el mismo mecanismo de reintento no es un buen default.
- **`parse_duration` (compartido con `--session-ttl`/`--http-timeout`) no tiene milisegundos** -- la unidad más chica es `1s`. Suficiente para un backoff razonable (1s/2s/4s/8s/16s/30s), no para uno sub-segundo.
- **Sin scaffolding de Docker/systemd para `serve-all` todavía** -- `linkc docker` (§3.62) sigue generando un `Dockerfile` de UN `.link`, sin una variante consciente de un directorio con varios.

**Verificado**: `cli_serve_all.rs`, 9 tests con el binario real como subproceso, hablando HTTP y bindeando puertos de verdad -- arranca 2 servicios en un solo proceso y responde en sus dos puertos con sus propios `.db` separados; rechaza `--db`/`LINK_DATABASE_URL` compartido; falla limpio sin `--port-base` o sin ningún `.link` en el directorio; un error de tipos en un archivo aborta TODO antes de arrancar cualquier hilo (el otro servicio ni siquiera llega a abrir su puerto); un bind ocupado en un servicio NO tumba al otro, que sigue respondiendo; y con `--restart-backoff`, un servicio cuyo puerto se libera a mitad de camino se recupera solo mientras el otro sigue sano todo el tiempo -- el incidente real (68 reinicios de `telemetry`), reproducido y confirmado resuelto contra el binario real, no solo razonado por lectura de código.

---

### 3.93 `--service-api-key`: autenticación servidor-a-servidor — RESUELTO

Cuarto reporte de adopción real (IgnisLove): un gateway Node.js (`cscript-gateway.ts`) hace `fetch` sin ninguna autenticación contra cada uno de los `linkc serve` que orquesta, confiando en que el puerto no sea alcanzable desde afuera. `--host 127.0.0.1` (GRAMMAR.md §3.81) ya cierra la mitad EXTERNA de ese hueco -- pero adentro de la misma máquina, CUALQUIER otro proceso con acceso a loopback puede llamar a esos servicios exactamente igual que el gateway legítimo. `@requires`/JWT (GRAMMAR.md §3.49/§3.64) no resuelven esto: autentican a un USUARIO final, no a QUIÉN está haciendo la llamada de red -- un rpc sin ninguna anotación de auth (o llamado internamente entre dos de los propios servicios) queda abierto a cualquiera en la máquina.

```bash
linkc serve backend.link 8787 --service-api-key s3cr3t
LINK_SERVICE_API_KEY=s3cr3t linkc serve backend.link 8787   # equivalente
# El caller manda:
#   X-Service-Api-Key: s3cr3t
```

**`--service-api-key <clave>`/`LINK_SERVICE_API_KEY`: un secreto compartido, verificado en el header `X-Service-Api-Key`, ANTES de leer el body y ANTES de cualquier otro chequeo (CORS aparte, que corre primero por la propia naturaleza del preflight).** Sin el flag/env var: `None`, sin este chequeo -- comportamiento IDÉNTICO al de siempre. Con él: toda request que no sea `/`/`/health`/`/status` necesita el header, con el valor EXACTO -- comparado en tiempo constante (`constant_time_eq`, la misma función que ya usaba `crypto.timingSafeEqual`, GRAMMAR.md §3.54) para no filtrar por timing cuánto del secreto adivinó quien prueba. Sin el header, o con un valor que no matchea: `401`, antes de que el body siquiera se lea -- un caller no autorizado no le cuesta memoria ni CPU de parseo al proceso.

**Una capa DISTINTA y ANTERIOR a `@requires`/JWT/sesiones -- las dos conviven, no se reemplazan.** Este flag autentica la CONEXIÓN (¿es este proceso el gateway legítimo?); `@requires(Role.Admin)`/un JWT autentican al USUARIO final que está detrás de esa conexión. Una request típica bajo este esquema lleva LOS DOS: `X-Service-Api-Key` probando que viene del gateway, y `Authorization: Bearer <token-de-usuario>` (sesión o JWT externo) probando de qué usuario se trata -- exactamente el patrón de "gateway interno + microservicios" que motivó el pedido.

**`/health`/`/`/`/status` quedan EXENTOS a propósito.** Un orquestador o load balancer que hace liveness probing (Kubernetes, Docker healthcheck) no tiene por qué conocer el secreto del gateway -- exigirlo ahí habría roto cualquier monitoreo de infraestructura existente sin agregar seguridad real (un liveness check no expone datos de negocio).

**Funciona igual bajo `linkc serve-all` (GRAMMAR.md §3.92) -- un valor GLOBAL para todos los servicios de la corrida, salvo excepción explícita.** Mismo límite que el resto de los flags globales de `serve-all` (`--jwt-secret`, `--cors-origin`, etc.): todos los servicios de un mismo proceso comparten el mismo entorno -- EXCEPTO este chequeo puntual, que gana su propia excepción vía `--service-api-key-exempt` (25/08/2026, ver abajo).

**`--service-api-key-exempt <nombre1,nombre2,...>` (solo bajo `serve-all`): deja a servicios puntuales, por nombre de archivo sin `.link`, AFUERA del chequeo, sin tocar al resto.** Landmine encontrado en el mismo barrido de "límites honestos" que motivó el fix de §3.94 -- de todos los flags globales de `serve-all`, `--service-api-key` es el único que es una capa de SEGURIDAD real (no solo conveniencia como `--host`/`--session-ttl`), así que es el que más le duele a un workspace real (IgnisLove, 17 servicios) donde UN servicio necesita quedar público (ej. un healthcheck de terceros, un webhook entrante que no puede mandar el header) mientras el resto sigue protegido -- antes de este fix, la única salida era sacar ese servicio de `serve-all` por completo y correrlo aparte con `linkc serve`, una sorpresa de arquitectura tardía, no un error de compilación.

Requiere `--service-api-key`/`LINK_SERVICE_API_KEY` configurado (si no, error de CLI limpio -- no tiene sentido eximir de un chequeo que no existe). Cada nombre tiene que matchear un `.link` REAL descubierto en el directorio -- un nombre con un typo falla limpio ANTES de arrancar cualquier servicio, listando los nombres reales encontrados, en vez de dejar a alguien creyendo que exentó un servicio que en realidad sigue protegido (o al revés). El chequeo en sí sigue siendo el mismo por hilo (`handle_request`, GRAMMAR.md §3.93 arriba) -- lo único que cambia es qué valor de `service_api_key` recibe CADA hilo al arrancar: `None` para un nombre exento, el valor real para el resto.

**Límites honestos:**
- **Un único secreto, no varios por caller.** No hay forma de emitir/revocar una clave DISTINTA por servicio que llama (todo caller legítimo comparte el mismo valor) -- suficiente para el caso real (un gateway central, no una malla de N servicios llamándose entre sí con identidades propias), no para "service mesh" con identidad por servicio.
- **Sin rotación asistida.** Cambiar la clave es reiniciar el proceso con un valor nuevo -- no hay un mecanismo de "aceptar la clave vieja Y la nueva durante una ventana" para rotar sin downtime.
- **No sustituye TLS.** El header viaja en texto plano si la conexión no está cifrada -- mismo criterio que cualquier otro secreto en un header (`Authorization`, `X-Forwarded-For`): la responsabilidad de que el TRANSPORTE sea seguro (TLS en el proxy que termina la conexión, o una red interna de confianza) es de la infraestructura que rodea a `linkc serve`, no de este flag.
- **Visible por herramientas de diagnóstico del gestor de procesos.** Mismo límite que `--jwt-secret` (§3.64) -- `pm2 describe`/`systemctl show`/`/proc/<pid>/environ` muestran el valor real, sea `--service-api-key <clave>` o `LINK_SERVICE_API_KEY`, porque el gestor lo necesita para poder reiniciar el proceso. Ese output hay que tratarlo como el secreto mismo, nunca pegarlo entero en un chat/ticket/log.

**Verificado**: `cli_service_api_key.rs`, 7 tests con el binario real como subproceso -- sin el flag, ninguna request lo necesita (comportamiento de siempre); sin el header, `401` antes de llegar al rpc; con la clave incorrecta, `401`; con la clave correcta, la request llega y se procesa normal; `/health`/`/`/`/status` siguen respondiendo `200` sin el header; `LINK_SERVICE_API_KEY` funciona igual que el flag; un flag mal usado da un error de CLI limpio, no un panic. Más 3 tests nuevos (25/08/2026) en `cli_serve_all.rs` para `--service-api-key-exempt`: un servicio nombrado exento responde sin el header mientras el otro sigue exigiéndolo con `401`/`200` según corresponda; un nombre exento que no matchea ningún `.link` real falla limpio antes de arrancar nada, listando los nombres reales; usarlo sin `--service-api-key` es un error de CLI limpio.

---

### 3.94 Aviso de colisión de nombre de tabla en PostgreSQL — RESUELTO

Quinto reporte de adopción real (IgnisLove) -- y el propio caso del equipo de c-script: `telemetry.link` estuvo a punto de chocar contra una tabla `events` real, ya usada por otro servicio, evitado a mano (renombrando la colección) porque alguien lo notó, no porque el runtime lo hubiera señalado. `CREATE TABLE IF NOT EXISTS` (GRAMMAR.md §3.17) es un no-op sobre una tabla que ya existe -- no mira si sus columnas tienen algo que ver con lo que el `.link` actual declara. La migración no destructiva de PostgreSQL (`ADD COLUMN IF NOT EXISTS`) le agregaría, en silencio, TODAS las columnas del programa nuevo a esa tabla ajena.

```bash
linkc serve telemetry.link 8787 --db postgres://user:pass@host/produccion
# Si 'events' ya existe en esa base, y ninguna columna de TelemetryEvent
# coincide con las que la tabla ya tiene, ANTES de agregar ninguna columna
# nueva se imprime por stderr (nunca bloquea el arranque):
#
#   advertencia: la tabla 'events' ya existe en PostgreSQL, pero NINGUNA de
#   las columnas que 'events' declara ([sessionId, userAgent]) coincide con
#   las que la tabla ya tiene ([amount, customer_id, order_id]). Si dos
#   .link comparten esta tabla A PROPÓSITO (columnas disjuntas, GRAMMAR.md
#   §3.17), esta advertencia es esperada y no requiere ninguna acción. Si
#   NO es así, es probable que 'events' le pertenezca a OTRO programa que
#   casualmente eligió el mismo nombre de colección -- revisá antes de
#   seguir, o renombrá la colección en este .link.
```

**Heurística conservadora, solo ADVIERTE por stderr -- nunca bloquea el arranque.** Antes de agregar columnas a una tabla que ya existía (`warn_if_table_looks_unrelated`, `runtime/db.rs`), si NINGUNA columna declarada (aparte de `id`) coincide por nombre con las columnas físicas que la tabla YA tiene, se imprime la advertencia de arriba nombrando ambos conjuntos de columnas -- pero la migración sigue su curso exactamente igual que antes de esta ronda: las columnas nuevas se agregan lo mismo. Una sola columna en común ya cuenta como evidencia suficiente de relación (no dispara la advertencia). Best-effort de punta a punta: si la consulta a `information_schema.columns` fallara por cualquier motivo, se omite la advertencia en silencio en vez de arriesgar romper un connect que de otro modo funcionaría.

**`createdAt`/`updatedAt`/`deletedAt` NO cuentan como evidencia de relación por sí solos (25/08/2026).** Landmine real encontrado auditando esta sección: la propia convención de auditoría que el lenguaje promueve (`createdAt: Timestamp = now()`, `@autoUpdate`, `@softDelete` -- GRAMMAR.md §3.63/§3.68) hace que dos programas SIN ninguna relación entre sí casi seguro declaren el mismo nombre `createdAt`, aunque compartan una tabla por pura coincidencia de nombre de colección -- exactamente el escenario que esta advertencia existe para atrapar, pasando desapercibido solo porque los dos siguieron la misma convención. Si el struct declara al menos un campo FUERA de esa terna, la comparación de overlap la ignora; si el struct declarado no tiene NINGÚN campo fuera de ella (un struct compuesto solo por campos de auditoría, caso raro), cae de vuelta a considerarlos a todos -- mejor una señal débil que ninguna.

**Por qué solo advierte, y no bloquea: dos `.link` DISTINTOS compartiendo una tabla con columnas disjuntas es un caso YA soportado y probado a propósito.** La primera versión de esta ronda devolvía un error duro -- revertida al auditar `pg_integration.rs` y encontrar que `two_different_link_files_declaring_disjoint_columns_of_the_same_table_can_read_each_others_rows_but_not_always_write` (test ya existente, verificado en CI) prueba EXACTAMENTE ese patrón: dos `.link` con cero columnas en común (aparte de `id`) sobre la misma tabla, cada uno leyendo/escribiendo solo las suyas -- y lo hace a propósito, no por accidente. Esa forma es indistinguible de una colisión accidental por nombre desde el punto de vista de "cero columnas en común" -- las dos se ven IDÉNTICAS. Convertirlo en un error habría roto ese caso legítimo para atrapar el accidental. Una advertencia visible ANTES de aceptar la primera request es la red de seguridad real: le da a quien arranca el proceso la chance de Ctrl+C y revisar, sin bloquear a quien de verdad comparte una tabla a propósito -- mismo criterio de "mejor un falso negativo ocasional que ruido sobre código legítimo" que ya llevó a reformular el lint de "autorización de fachada" (PLAN.md §9.5).

**Solo PostgreSQL.** SQLite (`check_schema_matches`, GRAMMAR.md §3.17) ya falla FUERTE ante cualquier diferencia de schema que no sea agregar una columna opcional nueva -- una tabla de otro programa, con columnas casi seguro distintas, ya se detecta ahí (con un mensaje de diff completo, no específico a "cero columnas en común", pero igual de efectivo). El gap real era solo del lado PostgreSQL, donde la migración es deliberadamente tolerante (datos de producción, no se puede recrear la tabla).

**`--adopt-existing` no necesita este aviso -- ya es más estricto.** Ese modo (GRAMMAR.md §3.67) ya EXIGE que cada columna declarada exista físicamente (`validate_columns_exist_for_adoption`) -- una tabla sin relación casi seguro ya falla ahí, con un error más específico ("faltan columnas"), no solo una advertencia.

**Límites honestos:**
- **Heurística, no prueba.** Cero columnas en común (fuera de la terna de auditoría, ver arriba) es evidencia fuerte pero no concluyente -- dos schemas sin relación PODRÍAN coincidir en un nombre de campo de DOMINIO por casualidad (ej. ambos tienen `status` o `userId`, no ya `createdAt`/`updatedAt`/`deletedAt`, cubiertos arriba), evitando la advertencia sin ser en verdad la misma tabla. No hay ninguna forma de saberlo con certeza sin un mecanismo de "quién creó esta tabla" (un `--db-schema`/`--db-prefix` que evite la colisión de raíz, todavía sin implementado, sería la solución real).
- **No previene nada, solo avisa.** Si nadie lee stdout/stderr del proceso al arrancar (ej. corriendo desatendido bajo `pm2`/`systemd` sin revisar logs), la advertencia pasa desapercibida igual que antes -- mismo límite que cualquier log, no un gate que se pueda forzar a fallar (`--fail-on-schema-warning` o similar queda fuera de esta ronda).
- **Un aviso por colección, no deduplicado entre reinicios.** Cada arranque de `linkc serve` contra la misma tabla ajena vuelve a imprimir la misma advertencia -- no hay un mecanismo de "ya avisé una vez, no lo repitas".

**Verificado**: `pg_integration.rs` contra un PostgreSQL real -- `connecting_to_a_preexisting_table_with_zero_overlapping_columns_warns_but_still_connects` (dos `.link` con cero columnas en común sobre la misma tabla: el segundo conecta y sirve requests normalmente, nunca bloquea, y su stderr contiene la advertencia nombrando la colección) y `an_evolving_table_that_shares_at_least_one_column_does_not_warn` (agregar una columna nueva a una tabla que comparte al menos un nombre de columna DE DOMINIO con la física NO dispara ninguna advertencia). Los dos tests preexistentes que prueban el caso LEGÍTIMO de columnas disjuntas (`two_different_link_files_declaring_disjoint_columns_of_the_same_table_...` y su vecino de tipos en conflicto) se re-confirmaron sin cambios -- la advertencia nueva no les rompe nada. Más dos tests nuevos (25/08/2026) para el fix de campos genéricos: `sharing_only_a_generic_audit_field_name_like_created_at_still_warns` (dos `.link` sin relación que SOLO comparten `createdAt` -- antes de este fix eso suprimía la advertencia, ahora sigue apareciendo) y `when_every_declared_field_is_a_generic_audit_field_it_still_falls_back_to_comparing_them` (un struct compuesto ÚNICAMENTE por campos de auditoría cae de vuelta al comportamiento anterior, sin regresión).

---

### 3.95 `countWhere` + `findWhere` empujados a SQL para `x.campo == valor` — RESUELTO

Sexto reporte de adopción real (IgnisLove), citado explícitamente como fricción encontrada usando el `@index`/`@unique` de un solo campo (§3.80) que ya existía: "agregué `@index` a `reviews.productId` y a `telemetry.sessionId`, y no aceleró nada -- cada `.filter()`/`findWhere` sigue trayendo la tabla entera a memoria". Cierto: `findWhere`/`deleteWhere` (§3.18) siempre evaluaron su predicado en el intérprete, trayendo la colección COMPLETA con `all()` primero -- a diferencia de `sumBy`/`countBy`/etc. (§3.52), que sí bajan a SQL. Contar cuántas reseñas tiene un producto no tenía forma de hacerse sin memoria O(tabla entera): la única opción era `findWhere(...).length()`.

<!-- linkc:check -->
```rust
type Review = { id: Int, productId: Int, rating: Int }
db { reviews: Review[] }

service Reviews {
  rpc add(productId: Int, rating: Int) -> Review {
    db.reviews.insert(Review { id: 0, productId: productId, rating: rating })
  }
  // countWhere: SELECT COUNT(*) ... WHERE real -- cero filas viajan del
  // motor al proceso, ni siquiera las que matchean.
  rpc countFor(productId: Int) -> Int {
    db.reviews.countWhere(|r: Review| { r.productId == productId })
  }
  // findWhere gana el mismo atajo cuando el predicado tiene esta forma --
  // antes de esta ronda, SIEMPRE traía la colección entera.
  rpc listFor(productId: Int) -> Review[] {
    db.reviews.findWhere(|r: Review| { r.productId == productId })
  }
}

test "countWhere cuenta solo las del producto pedido" {
  Reviews.add(1, 5);
  Reviews.add(1, 3);
  Reviews.add(2, 4);
  assert(Reviews.countFor(1) == 2);
  assert(Reviews.countFor(2) == 1);
  assert(Reviews.countFor(999) == 0);
}
```

**`db.<c>.countWhere(predicate: (T) -> Bool) -> Int`, builtin NUEVO -- mismo contrato de tipos que `findWhere`/`deleteWhere`, ejecución distinta.** Antes de esta ronda, la única forma de contar filas que matchean un predicado era `findWhere(...).length()` -- trae la colección entera a memoria solo para descartarla y quedarse con un número. `countWhere` reconoce el predicado ESTÁTICAMENTE (`ast::recognize_equality_predicate`, sin invocar el intérprete) y, si tiene EXACTAMENTE la forma `|x| x.campo == valor` (o `valor == x.campo`), lo traduce a `SELECT COUNT(*) FROM "tabla" WHERE "campo" = ?` -- un solo entero cruza del motor al proceso, nunca una fila. `findWhere` gana el mismo reconocimiento (mismo `SELECT` pero trayendo las columnas reales, no `COUNT(*)`) -- su firma/comportamiento observable no cambia, solo cuántas filas viajan del motor al proceso cuando el predicado matchea esta forma.

**`valor` puede ser un literal o una variable capturada del entorno externo del closure -- nunca otro campo de `x`, ni una expresión derivada.** `productId` en el ejemplo de arriba es el parámetro del propio `rpc` -- el caso real que motiva esto (`reviews.productId == productId`, `telemetry.sessionId == sessionId`). Reconocido en dos pasos: `ast::recognize_equality_predicate` identifica la FORMA sintáctica (`x.campo == <lo que sea>`) sin evaluar nada; el caller (`runtime/mod.rs::recognize_pushable_equality`) evalúa ese "lo que sea" SOLO si es un literal (`Int`/`Float`/`String`/`Bool`) o un `Ident` que resuelve en el `Env` que el closure capturó al crearse -- cualquier otra forma (`x.otroCampo`, una llamada, una expresión aritmética) no se evalúa, y el predicado entero cae al camino interpretado de siempre.

**Cualquier otra forma de predicado sigue funcionando exactamente igual que antes -- nunca un error, solo sin el atajo.** `>`/`<`/`!=`, `&&`/`||` combinando varias condiciones, comparar dos campos de `x` entre sí, un método, una comparación contra un enum (`x.status == Status.Active {}`, que NO es un literal reconocido) -- todo esto sigue evaluándose en el intérprete, trayendo la colección completa con `all()` como siempre. El reconocimiento es deliberadamente conservador: mejor un falso negativo ocasional (una consulta que PODRÍA pushearse pero no se detecta) que arriesgar un falso positivo que devuelva el resultado equivocado.

**`"id"` también es pusheable, aunque nunca vive en `Db::columns` (que es "todo menos id").** `countWhere(|x| x.id == valor)`/`findWhere` sobre `"id"` toman un camino aparte, chico, dentro de la misma función compartida (`equals_condition`, `runtime/db.rs`).

**Respeta `@softDelete` (§3.78) igual que el camino interpretado.** La condición SQL generada AND-ea la misma `"<campo>" IS NULL` que ya usa `count()`/`all()` -- una fila soft-deleteada no aparece en un `countWhere`/`findWhere` pusheado, ni por accidente.

**Columnas serializadas como JSON quedan fuera del atajo.** Un campo `x?: T?`, un struct, un enum ADT, una lista/mapa -- cualquier columna que `ColumnPlan` marca `json` no tiene una igualdad simple de SQL contra un `Value` sin ambigüedad (¿comparás el JSON serializado byte a byte? ¿un subconjunto de campos?) -- el reconocimiento devuelve "no pusheable" para esas, cae al intérprete.

**Límites honestos:**
- **Solo `==` (o su espejo, `valor == x.campo`) -- ningún otro operador se empuja todavía.** `>`/`<`/`<=`/`>=`/`!=` siguen sin bajar a SQL -- PLAN.md §9.3.1 lo trackea como trabajo pendiente, junto con combinar varias condiciones (`&&`/`||`).
- **Solo UN campo -- sin combinar condiciones.** `|x| x.a == 1 && x.b == 2` no se reconoce como pusheable (necesitaría combinar dos condiciones SQL con sus propios parámetros, un compilador de predicado más completo que el de esta ronda) -- cae entero al intérprete, aunque técnicamente las dos mitades por separado serían pusheables.
- **`deleteWhere` NO gana este atajo.** Sigue trayendo la colección completa y borrando fila por fila (una sentencia `DELETE` por fila que matchea) -- el mismo trabajo de reconocimiento aplicaría, pero `deleteWhere` necesita además publicar cada fila borrada a los suscriptores (`stream`), lo que complica un `DELETE ... WHERE` de una sola sentencia; queda para una ronda aparte.
- **No hay forma de pedir el plan de ejecución o confirmar desde el `.link` que un `countWhere`/`findWhere` particular SÍ se pusheó.** La única confirmación hoy es leer el código (`ast::recognize_equality_predicate`) o instrumentar el SQL emitido a mano.

**Verificado**: 1 test en `checker.rs` (`countWhere` toma exactamente 1 argumento `fn(T) -> Bool` y devuelve `Int`, mismo contrato que `findWhere`/`deleteWhere`) y 5 en `runtime/mod.rs` contra un servidor real vía `invoke_rpc`: `countWhere` cuenta correctamente vía el atajo de SQL; `findWhere` con un predicado pusheable devuelve las mismas filas que siempre devolvió; los dos con un predicado NO pusheable (`x.rating > 3`) siguen dando el resultado correcto por el camino interpretado; `countWhere`/`findWhere` sobre `"id"`; y los dos respetan `@softDelete` incluso pusheados. El SQL real emitido (`SELECT COUNT(*) FROM "reviews" WHERE "productId" = ?` / `SELECT ... WHERE "productId" = ? ORDER BY "id"`) se confirmó a mano contra `linkc test` con logging temporal antes de este release -- exactamente una consulta por llamada pusheada, ninguna para el caso de fallback.

---

### 3.96 `@check(...)`: constraints numéricos de nivel de base — RESUELTO

Séptimo reporte de adopción real (IgnisLove), citado con el ejemplo exacto que lo motiva: "`reviews.link` solo evita un `rating` fuera de 1-5 porque `clampRating()` lo fuerza en el código; no hay ninguna barrera a nivel de base si algún día otro rpc inserta sin pasar por esa función". Cierto -- hasta esta ronda, la ÚNICA forma de imponer un rango numérico era código de aplicación (a mano, en cada rpc que escribe), sin ningún respaldo si un `insert`/`applyPatch` nuevo se olvidaba de llamarlo, o si algo por fuera de c-script escribía directo a la tabla.

<!-- linkc:check -->
```rust
type Review = { id: Int, @check(range, 1, 5) rating: Int }
db { reviews: Review[] }

service Reviews {
  rpc add(rating: Int) -> Review {
    db.reviews.insert(Review { id: 0, rating: rating })
  }
}

test "un rating dentro de 1-5 se acepta" {
  let r = Reviews.add(3);
  assert(r.rating == 3);
}
```

**Tres formas, mismo criterio "kind + argumento(s)" que `@validate(email)`/`@validate(regex, "...")`: `@check(min, N)`, `@check(max, N)`, `@check(range, N, M)`.** Solo sobre un campo `Int`/`Int64`/`Float` (requerido u opcional) -- rechazado en compilación sobre cualquier otro tipo, con el tipo real encontrado en el mensaje. `@check(range, N, M)` con `N > M` también se rechaza en compilación ("el mínimo es mayor que el máximo -- ningún valor podría pasar nunca") -- casi siempre un error de tipeo, no una restricción real que alguien quiso escribir a propósito. A lo sumo un `@check` por campo.

**Enforcement DOBLE, no solo uno: aplicación Y base de datos, los dos puntos reales de entrada.** Del lado de la aplicación, `apply_field_validators` (`runtime/mod.rs`, el mismo mecanismo que ya usaba `@validate`) corre en los DOS puntos donde un struct se termina de construir -- decodificando el wire (`json_to_typed_value`, un rpc que recibe el struct completo como parámetro) y construyendo un literal DENTRO del cuerpo de un rpc (`Expr::StructLit`, el caso más común: un rpc arma `NewX { campo: valorSuelto }` a partir de parámetros propios) -- una violación es `bad_request` (400), nombrando el campo y el límite exacto. Del lado de la base, el `CREATE TABLE` genera un `CHECK (...)` inline de VERDAD -- en SQLite (columna física, parte del propio `CREATE TABLE IF NOT EXISTS`) y en PostgreSQL (mismo generador que usa `linkc build` para `schema.postgres.sql`, GRAMMAR.md §3.9) -- así que aunque algo escriba SQL crudo sin pasar por c-script en absoluto (exactamente el escenario citado en el reporte), la base sigue rechazando el valor. Un error de `CHECK` que sí llega hasta SQL (algo escribiendo por fuera del camino normal de la aplicación) también se traduce a 400, no a un 500 genérico -- mismo criterio que ya usaba `@unique` (GRAMMAR.md §3.80) para su propia violación.

**`--adopt-existing` (GRAMMAR.md §3.67) nunca ejecuta este DDL -- mismo criterio que `@index`/`@unique`.** Una colección adoptada no gana el `CHECK` físico (ese modo no toca DDL en absoluto), pero la validación de aplicación SIGUE aplicando sin importar el modo -- un `insert`/`applyPatch` real sobre una colección adoptada con `@check` se rechaza igual del lado de la aplicación, aunque la tabla física no tenga el constraint.

**Límites honestos:**
- ~~**Solo rangos numéricos simples -- ninguna expresión booleana arbitraria.**~~ **RESUELTO (27/08/2026): `@check(minLength/maxLength, N)` sobre `String` (ver §3.146) y `@check(<expr>)` a nivel de `type` comparando dos campos entre sí (`endDate > startDate`, ver §3.173) cierran los dos huecos citados acá.
- **Sin `ALTER TABLE ADD CONSTRAINT` sobre una tabla EXISTENTE en PostgreSQL.** El `CHECK` solo se aplica al CREAR la tabla (`CREATE TABLE IF NOT EXISTS`) -- agregar `@check` a un campo de una colección que YA tiene datos en Postgres no retrofittea el constraint físico a la tabla existente (mismo límite que el resto de la migración no destructiva de Postgres, GRAMMAR.md §3.17): la validación de aplicación sigue protegiendo los `insert`/`applyPatch` nuevos, pero filas viejas que ya violaban el rango (si las hubiera) no se detectan ni se marcan.
- **SQLite tampoco migra un `@check` agregado después.** `check_schema_matches` (GRAMMAR.md §3.17) no compara constraints `CHECK` (`PRAGMA table_info` no los reporta) -- agregar `@check` a un campo de una tabla SQLite YA creada sin el constraint no lo agrega retroactivamente; haría falta borrar el archivo y recrear, mismo procedimiento que cualquier otro cambio de schema que SQLite no auto-migra.
- **Límites como `f64` sin importar si el campo es `Int`/`Int64`.** Comparar un valor entero contra un límite de punto flotante es exacto para cualquier magnitud humana realista -- no pensado para el borde exacto de `i64`/`Int64` (GRAMMAR.md §3.30).

**Verificado**: 5 tests en `checker.rs` (`min`/`max`/`range` tipan sobre `Int`/`Int64`/`Float` requerido u opcional, se rechaza sobre un campo no numérico, `range` con mínimo mayor que máximo se rechaza, `@check` con un kind desconocido y un segundo `@check` sobre el mismo campo se rechazan en el parser). 1 en `codegen/postgres_emit.rs` (el DDL estático que `linkc build` genera lleva el `CHECK` inline exacto para las tres formas). 4 en `runtime/mod.rs` contra un servidor real vía `invoke_rpc` (`range` rechaza por arriba y por abajo nombrando el campo; `min`/`max` rechazan solo el lado que declaran; dispara igual construyendo el struct DENTRO del cuerpo de un rpc a partir de parámetros sueltos, no solo recibiéndolo completo por el wire; un campo opcional ausente no dispara nada). 1 en `runtime/db.rs` contra SQLite real (el `CHECK` existe de verdad en la tabla física, y un `INSERT` crudo por SQL directo -- sin pasar por `Db::call` en absoluto -- se rechaza a nivel de SQLite). 1 en `pg_integration.rs` contra un PostgreSQL real, mismo criterio: un `INSERT` crudo por fuera de `linkc serve` se rechaza a nivel de Postgres.

---

### 3.97 `linkc migrate --dry-run` — RESUELTO

Octavo reporte de adopción real (IgnisLove), ítem 9 de su lista priorizada: "mostrar el DDL exacto que se ejecutaría sin aplicarlo... antes de apuntar cualquier servicio a una tabla real con `--adopt-existing`, ver el DDL exacto que se ejecutaría sin aplicarlo todavía sería la verificación que le falta a ese paso". Hasta esta ronda, la única forma de saber qué DDL iba a correr `linkc serve` contra una base Postgres era conectar de verdad -- que YA lo ejecuta, al conectar (GRAMMAR.md §3.17).

```bash
linkc migrate backend.link --db postgres://user:pass@host/produccion --dry-run
# -- 'linkc migrate --dry-run': DDL que 'linkc serve'/'linkc serve-all' ejecutaría
# -- al conectar a esta base AHORA MISMO -- nada de esto se aplicó.
#
# -- 'facturas': 1 columna(s) nueva(s), agregada(s) SIEMPRE nullable (GRAMMAR.md §3.17)
# ALTER TABLE "facturas" ADD COLUMN IF NOT EXISTS "notas" TEXT;
#
# -- 'clientes': sin cambios (todas las columnas declaradas ya existen)
```

**Reusa las MISMAS funciones puras de generación de DDL que ya usa el runtime real -- nunca una copia propia.** `create_postgres_table_sql`/`alter_table_add_column_postgres` (`codegen/postgres_emit.rs`) y `create_index_statements` (`runtime/db.rs`) ya eran funciones que devuelven texto SQL sin ejecutar nada -- la ejecución (`backend.execute_ddl`) siempre vivió en un paso SEPARADO. `linkc migrate --dry-run` (`src/migrate.rs`, módulo nuevo) llama a esas mismas funciones y nunca al paso que ejecuta -- si el runtime real cambiara el DDL que genera, este reporte cambia automáticamente con él, sin haber dos copias que puedan desincronizarse (GRAMMAR.md §3.9).

**Conecta de verdad, pero SOLO lee -- ningún `CREATE`/`ALTER` en ningún momento.** Consulta `information_schema.columns` (la misma técnica que ya usan `validate_columns_exist_for_adoption`/§3.94) para saber qué existe de verdad, y compara contra lo que el `.link` declara: una tabla que no existe → el `CREATE TABLE` completo; una tabla existente con columnas faltantes → un `ALTER TABLE ADD COLUMN IF NOT EXISTS` por columna faltante; sin diferencias → "sin cambios". También corre el mismo chequeo de "¿esta tabla parece de otro programa?" (§3.94) y el de tipo de `"id"` (§3.36) -- si cualquiera de los dos fallaría al conectar de verdad, el reporte lo dice explícito ANTES de que alguien intente conectar en serio.

**Solo PostgreSQL, a propósito.** SQLite (`check_schema_matches`) ya falla FUERTE ante cualquier diferencia de schema al conectar de verdad, con un mensaje que nombra el diff exacto ANTES de tocar nada -- un modo `--dry-run` aparte no agregaría ninguna información que ese camino no dé ya. `linkc migrate` sobre una URL que no empieza con `postgres://`/`postgresql://` se rechaza con este motivo explícito.

**Solo `--dry-run` en esta ronda -- sin un modo "aplicar" separado.** `linkc migrate <archivo> --db <url>` SIN `--dry-run` se rechaza con un mensaje explícito: aplicar de verdad ya pasa automáticamente al conectar con `linkc serve`/`linkc serve-all` -- un segundo camino que también "aplica" sería ambiguo (¿cuál gana si los dos corren a la vez?) sin agregar nada que `linkc serve` no haga ya.

**Límites honestos:**
- **Sin `--allow-destructive`, porque no hace falta uno todavía.** El pedido original mencionaba "comportamiento configurable ante una migración que perdería datos" -- auditando la migración real de Postgres (GRAMMAR.md §3.17) apareció que HOY no existe ningún camino destructivo: solo crea tablas nuevas y agrega columnas SIEMPRE nullable, nunca borra ni cambia el tipo de una columna existente. No hay ninguna migración "peligrosa" que un dry-run necesite advertir hoy -- si el proyecto suma en el futuro una migración genuinamente destructiva (un `DROP COLUMN` explícito, por ejemplo), ESE sería el momento de agregar el flag, no antes.
- **No detecta con certeza una colisión de nombre de tabla -- misma heurística, mismos límites que §3.94.** "Ninguna columna en común" es evidencia fuerte, no prueba.
- **`--adopt-existing` no tiene su propio modo dry-run.** Ese modo nunca ejecuta DDL en absoluto (§3.67) -- no hay nada que "dry-run-ear" ahí; el reporte útil para ese caso ya es `check_schema_for_adoption`, que se ejecuta al conectar de verdad con `--adopt-existing`.
- **No calcula cuánto tardaría la migración real ni bloquea la tabla.** Puramente informativo -- el tamaño de la tabla real, el lock que un `ALTER TABLE` real tomaría, no son parte de este reporte.

**Verificado**: `pg_integration.rs`, 2 tests contra un PostgreSQL real -- una colección nueva muestra el `CREATE TABLE` exacto (incluido un `@check` inline) y confirma que la tabla NO se creó de verdad después; una tabla existente a la que le falta una columna muestra el `ALTER TABLE ADD COLUMN` exacto y confirma que la columna NO se agregó de verdad después.

---

### 3.98 Lint `hardcoded-secret-literal` — RESUELTO

PLAN.md §9.5.3: "que `linkc lint` avise si detecta una URL de conexión o API key literal en el código". Un `const NOMBRE: String = "..."` de nivel superior es el lugar más común y menos ambiguo donde alguien pega un secreto real por comodidad, sobre todo temprano en un proyecto -- sin ningún aviso hasta esta ronda.

<!-- linkc:check -->
```rust
const DB_HOST: String = "postgres://internal-db.local/app";

service S {
  rpc noop() -> Int { 1 }
}
```

**`linkc lint` marca `const NOMBRE: String = "literal"` en DOS casos, cada uno con su propio mensaje.** (1) El literal tiene la forma de una URL de conexión con CREDENCIALES embebidas (`esquema://usuario:contraseña@resto` -- `postgres://`/`postgresql://`/`mysql://`/`mongodb://`/`redis://`/`amqp://`, lista fija de esquemas conocidos, no un parser de URL genérico). Una URL SIN credenciales (como la del ejemplo de arriba, un hostname interno sin contraseña) no dispara -- lo que importa es la contraseña adentro, no el esquema en sí. (2) El NOMBRE del `const` sugiere un secreto (mismo heurístico laxo que `timing-unsafe-secret-comparison`, §3.88: substring de `secret`/`token`/`password`/`apikey`/`api_key`, sin distinguir mayúsculas) Y el valor es un literal no vacío.

**El mensaje recomienda `env.get("...")` -- pero NUNCA como reemplazo directo del valor del `const`.** Un `const` en c-script solo admite un LITERAL (`check_const`, checker.rs: "el valor de un 'const' tiene que ser un literal... no una computación en runtime") -- `const DB_URL: String = env.get("LINK_DATABASE_URL");` es un error de compilación aparte, no una alternativa válida. El mensaje real dice explícito: no declarar esto como `const` en absoluto, leer el valor con `env.get("...")` en el momento que se necesita, adentro del `rpc`/`fn` que lo usa.

**Solo `const` de nivel superior -- deliberadamente, no un `let` dentro de un `rpc`/`fn`/`test`.** Es el lugar más fácil de reconocer sin ambigüedad como "esto es configuración escrita a mano", a diferencia de un `let` armado dentro de la lógica de un rpc (podría ser un valor de prueba, un template, cualquier cosa) o un literal usado una sola vez en una llamada.

**Límites honestos:**
- **Nombres compuestos solo con "key" (sin "apikey"/"api_key" exactos) no se detectan por nombre** -- `STRIPE_KEY`/`SENDGRID_KEY` no disparan por nombre (aunque SÍ dispararían si el valor tiene forma de URL con credenciales). Mismo motivo que `timing-unsafe-secret-comparison` nunca incluyó "key" suelto: identificadores comunes y perfectamente inocentes (`sortKey`, `primaryKey`, `cacheKey`) lo harían disparar constantemente -- mejor un falso negativo ocasional que ruido constante sobre código bien escrito (mismo criterio que llevó a reformular el lint de "autorización de fachada", PLAN.md §9.5.4).
- **Solo detecta la FORMA de una URL de conexión con credenciales, no un catálogo de shapes de API key reales** (`sk_live_`/`sk_test_` de Stripe, `AKIA` de AWS, etc.) -- ese catálogo quedaría desactualizado el día que un proveedor cambie su prefijo; el heurístico de nombre (2) es lo que cubre ese caso hoy, no un patrón de valor dedicado.
- **Puramente informativo, como el resto del linter** -- `linkc lint` sigue saliendo con código 0 aunque encuentre esto. Solo corre con `linkc lint`, no con `linkc build` (mismo alcance que el resto de las reglas de este linter, que nunca se ejecutan automáticamente durante un build).

**Verificado**: 6 tests en `lint.rs` -- una URL con credenciales embebidas se marca nombrando el `const` y recomendando `env.get`; la misma URL SIN credenciales no se marca; un nombre tipo secreto con un literal se marca; un nombre/valor ordinario no se marca; un literal vacío nunca se marca aunque el nombre sugiera un secreto; un valor NO literal (ej. una llamada a `env.get(...)`, que el checker rechazaría por otro motivo aparte) tampoco se marca -- el lint corre sobre el AST parseado, antes/aparte del checker.

---

### 3.99 `linkc test --db <url-postgres>` — RESUELTO

Reporte de un adoptador real (MyFinance) verificando el fix de §3.91 contra su propio esquema: "`linkc test` corre contra SQLite embebido, que NO reproduce el bug original (decodificación del wire binario de Postgres)... Sin esto, el fix está 'compilado y probado con datos falsos', no 'verificado'". Cierto -- `run_program_tests_filtered` (el motor de `test "..." { ... }`) siempre creaba una SQLite `:memory:` nueva por cada test, sin ninguna forma de apuntar a Postgres. Los dos backends emiten SQL y decodifican el wire de forma DISTINTA (GRAMMAR.md §3.91) -- que un `test` pase contra SQLite no prueba nada sobre cómo se comporta contra Postgres real.

```bash
linkc test backend.link --db postgres://user:pass@host/base_de_test
LINK_TEST_DB=postgres://user:pass@host/base_de_test linkc test backend.link   # equivalente
```

**`--db <url-postgres>`/`LINK_TEST_DB` (deliberadamente DISTINTA de `LINK_DATABASE_URL`, la que usa `linkc serve`) hace que TODOS los bloques `test "..." { ... }` corran contra esa base real, en vez de SQLite `:memory:`.** Env var separada a propósito: si `LINK_DATABASE_URL` (la de producción/desarrollo del `serve` real) estuviera puesta en el entorno, `linkc test` NUNCA debe usarla por accidente -- confundir "la URL del servidor" con "la URL de test" sería exactamente el tipo de error que deja un `test` real insertando filas en una base de producción. Sin el flag/env var: comportamiento IDÉNTICO al de siempre.

**Solo PostgreSQL -- `--db` con cualquier otra forma de URL se rechaza.** SQLite ya es el default rápido y aislado sin el flag; no hay necesidad real de apuntar `--db` a un archivo SQLite distinto.

**Límite honesto, deliberado: SIN el aislamiento por test que `:memory:` da gratis.** Contra SQLite, cada `test` arranca con una conexión `:memory:` NUEVA y VACÍA (`Db::new(program, ":memory:")` corre una vez por test) -- aislamiento total, sin que el `.link` tenga que hacer nada. Postgres no tiene un equivalente de "`:memory:`": reconectar a la MISMA URL para cada test daría el MISMO estado persistente, no uno fresco. En vez de fingir un aislamiento que no existe (ej. `DROP`/`TRUNCATE` automático entre tests, una operación destructiva que este proyecto evita a propósito -- ver "Límites honestos" de §3.97), `--db` comparte la conexión entre TODOS los tests de la corrida: lo que un test insertó, el siguiente lo ve. Correr esto contra una base de TEST dedicada (nunca contra producción) es responsabilidad de quien pasa la URL -- mismo criterio que el resto de las operaciones sobre `--db` en este proyecto (`linkc migrate --dry-run`, §3.97).

**`--adopt-existing` funciona igual que en `linkc serve`.** Si la base de test ya tiene las tablas (ej. una copia real del esquema de producción, sembrada con datos reales), `--adopt-existing` evita que `linkc test` intente `CREATE TABLE`/`ALTER TABLE` sobre ellas.

**Límites honestos:**
- **Sin reset automático entre CORRIDAS tampoco.** Los datos que una corrida de `linkc test --db ...` dejó siguen ahí la próxima vez -- si eso importa, limpiar la base de test antes de cada corrida es trabajo de quien orquesta el test (un script que la resetea, o simplemente recrearla), no algo que `linkc test` hace por su cuenta.
- **`--filter` sigue funcionando igual, pero el orden de ejecución entre los tests que SÍ corren importa ahora** (ver el `Límite` de arriba) -- filtrar a un subconjunto puede dar un resultado distinto al de correr todos, si alguno de los filtrados dependía de estado que otro test (ahora excluido) dejaba.
- **No aplica al testing de CONTRATO** (`linkc test <archivo> <snapshot>`) -- ese camino nunca toca ninguna base, combinar `--db` con un `<snapshot>` se rechaza con un mensaje claro, mismo criterio que `--filter`.

**Verificado**: 2 tests en `pg_integration.rs` contra un PostgreSQL real -- un `test` que inserta una fila vía `db.<c>.insert` deja esa fila de VERDAD en Postgres (confirmado con una consulta SQL directa después, no solo "el test pasó"); dos `test` en el mismo archivo, el segundo lee el conteo que el primero dejó -- confirma el límite de "sin aislamiento" documentado arriba, no lo esconde.

---

### 3.100 `linkc doctor`: diagnóstico de entorno antes de un despliegue — RESUELTO

PLAN.md §9.7.1, en el backlog desde antes de los reportes de adopción: "diagnóstico de entorno (versión, PATH, permisos, conectividad a la DB configurada) antes de un despliegue". `linkc` no tenía ninguna forma de responder "¿este entorno está listo para `linkc serve`?" sin arrancar el servidor de verdad y ver si fallaba en producción.

```bash
linkc doctor backend.link
linkc doctor backend.link --db postgres://user:pass@host/base   # o LINK_DATABASE_URL
```

**Cuatro chequeos, cada uno independiente de los demás -- uno que falla no cancela los siguientes, el reporte completo importa más que salir rápido en el primer error (mismo criterio que `linkc migrate --dry-run`, §3.97, reportando por colección en vez de abortar):**

1. **Versión de `linkc`** (`linkc::VERSION`, la misma constante que estampa cada archivo generado -- §3.83).
2. **El `.link` de entrada existe, resuelve todos sus `import`, parsea y tipa.** Reinterpretación deliberada del "PATH" del ítem original: `linkc` es un binario estático sin ningún otro ejecutable de sistema del que depender, así que inspeccionar la variable de entorno `PATH` no daría ninguna señal real de si un despliegue va a funcionar -- lo que sí importa es que el programa de entrada compile, que es justamente lo que este chequeo verifica en su lugar. Un error real (sintaxis, tipos, un `import` que no resuelve) se imprime con el mismo diagnóstico con snippet+caret de `linkc <archivo.link>`.
3. **Permiso de escritura en el directorio del `.link`.** Crea y borra un archivo de prueba (`.linkc_doctor_check`) ahí mismo -- el mismo directorio donde `linkc serve` sin `--db`/`LINK_DATABASE_URL` crea el archivo SQLite por default.
4. **Conectividad a la base configurada** (`--db`/`LINK_DATABASE_URL`, misma resolución y mismo formato que `linkc serve` -- una URL `postgres://`/`postgresql://`, o cualquier otro valor se toma como ruta de archivo SQLite):
   - **Sin `--db`/`LINK_DATABASE_URL`**: SQLite embebido, informativo -- la ruta exacta que usaría `linkc serve` (`<archivo>.db` al lado del `.link`), sin conexión de red que probar.
   - **Con una URL de Postgres**: conecta de VERDAD y corre un `SELECT 1` (`check_postgres_connectivity`, reusando el mismo `connect_postgres_client` que `linkc migrate --dry-run`) -- pero **solo lectura, nunca ejecuta ningún DDL**, ni siquiera el chequeo de colisión de tabla (§3.94) o de tipo de `id` (§3.36) que sí corre `linkc migrate --dry-run`: `doctor` responde "¿la base es alcanzable?", no "¿el schema calza?", que ya es la pregunta que resuelve `migrate --dry-run` por separado. La URL se muestra en el reporte para diagnóstico, pero con la credencial (`usuario:contraseña@`) siempre enmascarada -- ni en la terminal local debería terminar un secreto en texto plano, por si ese output se captura en un log o en CI.

**Código de salida**: `1` si algún chequeo real dio error (archivo inválido, sin permiso de escritura, base inalcanzable); `0` si los cuatro pasaron -- pensado para un paso de CI antes de desplegar (`linkc doctor backend.link --db "$LINK_DATABASE_URL" || exit 1`), no solo para lectura humana.

**Verificado**: `cli_doctor.rs` (7 tests) contra el binario real -- éxito con SQLite default, archivo faltante, error de sintaxis, URL de Postgres inalcanzable (puerto cerrado, falla rápido en vez de colgarse) sin panic, URL malformada sin panic, uso sin argumentos, y `LINK_DATABASE_URL` funcionando igual que `--db`; más 1 test en `pg_integration.rs` contra un PostgreSQL real, confirmando `[OK]` de conectividad Y que ninguna tabla se creó (`doctor` nunca toca el schema).

---

### 3.101 `List<Int>.sum() -> Int` — RESUELTO, alcance acotado

PLAN.md §9.3, gap nuevo encontrado analizando "CRM" (Nexus, 11 `.link`, primer análisis de este adoptador esta sesión) -- con un bug de producción real como evidencia, no solo una preferencia de estilo. `List` solo tenía `.take`/`.filter`/`.map`/`.length`/`.join`/`.reverse` (§3.15), sin ninguna forma de sumar sin un `while` manual. `accounting.link`, `getAccountingSummary()`, necesitaba el total real de una lista de montos ya filtrada en memoria (`incomeTx`/`expenseTx`, resultado de `.all().filter(...)`) -- al no existir `.sum()`, el código terminó con un placeholder que multiplica la CANTIDAD de transacciones por una tarifa plana inventada (`incomeTx.length() * 1000`) en vez de sumar `t.amount` de verdad. Un reporte financiero mostrando cifras fabricadas, no aproximadas, en código que pasaba sus propios tests porque ninguno verificaba esos campos puntuales.

```
fn total(montos: Int[]) -> Int { montos.sum() }
```

**Alcance deliberadamente acotado a `List<Int>` en esta ronda -- NO `List<Int64>`/`List<Float>`.** No es una limitación sintáctica arbitraria: en runtime, `Value::List` (`runtime/mod.rs`) no lleva ningún tag de tipo de elemento -- mismo límite ya documentado en la doc de `Value::Uuid` ("una vez que la información de tipo ESTÁTICO ya no está disponible en runtime"). Con una lista NO vacía, el tag de la primera `Value::Int`/`Value::Int64`/`Value::Float` ya alcanzaría para saber qué sumar -- pero una lista VACÍA (un caso real y válido: "cero transacciones de un tipo en el período") no tiene ningún elemento del que leer ese tag, y adivinar mal ahí sería un bug de serialización SILENCIOSO: `Int64` viaja como string por el wire, `Int` como número (§3.30) -- devolver el `Value` equivocado rompería el contrato sin ningún error visible. Resolver esto en general necesitaría que el checker le pasara el tipo estático del receptor al intérprete en el sitio de la llamada, infraestructura que esta ronda no amerita para un solo método. `List<Int>` no tiene esa ambigüedad -- un único tipo posible, incluso vacía -- así que es lo único que se resuelve acá; el checker rechaza `List<Int64>`/`List<Float>` con un mensaje explícito que nombra el motivo, no un "método no encontrado" genérico.

**`.reduce()` genérico, `.sum()` sobre `Int64`/`Float`**: quedan fuera de esta ronda, para cuando haya demanda real y valga la pena resolver la ambigüedad de tipo del caso vacío.

**Verificado**: 4 tests en `checker.rs` (`List<Int>` tipa, `List<Int64>`/`List<Float>` se rechazan con el mensaje que nombra el motivo, `.sum()` no toma argumentos) + 2 en `runtime/mod.rs` contra un servidor real vía `invoke_rpc` (suma real de una lista no vacía; una lista vacía -- el caso que el placeholder de "cantidad × tarifa" jamás hubiera distinguido de "una transacción gratis" -- da `0`).

---

### 3.102 `db.<c>.maxRow(selector)`/`minRow(selector) -> T?` — RESUELTO

PLAN.md §9.3, gap nuevo encontrado analizando IgnisLove en profundidad -- con un bug de producción real y confirmado como evidencia, no una preferencia de estilo. `bandit_rewards.link`, `getBestArm()`, hacía `db.arms.all()` y devolvía `allArms[0]` -- el orden de `all()` es por `"id"` (§3.48), NUNCA por el campo de recompensa, así que ese rpc JAMÁS devolvía el brazo con mejor `avgRewardTenths`, pese a su nombre: un algoritmo de optimización (bandit de recompensas) silenciosamente roto, sin ningún error que lo delatara. `maxBy`/`minBy` (§3.52) ya existían, pero solo agregan un VALOR agrupado (siempre `{key, value}[]`, incluso sin ningún campo de agrupación real) -- nunca la fila COMPLETA que alcanza ese máximo/mínimo.

```
type Arm = { id: Int, name: String, avgRewardTenths: Int }

service Bandit {
  rpc getBestArm() -> Arm? { db.arms.maxRow(|a: Arm| { a.avgRewardTenths }) }
}
```

**Dos métodos nuevos, no uno con un parámetro de dirección.** Se descartó `db.<c>.top(selector, dir: "asc"|"desc")` -- mismo criterio que §3.52 ya usa para `sumBy`/`countBy`/`avgBy`/`maxBy`/`minBy` (cinco métodos con nombre explícito por combinación, nunca un query builder ni un modo-por-string): un nombre por forma es más fácil de tipar en el checker (el resultado no depende de un valor en runtime) y más fácil de leer en el sitio de la llamada.

**Mismo shape reconocido, mismas restricciones de tipo que el campo de valor de `maxBy`/`minBy`.** El selector tiene que ser exactamente `|item: T| item.campo` (`ast::recognize_field_selector`, reusado tal cual -- ningún código nuevo de reconocimiento de shape); el campo tiene que ser `Int`/`Int64`/`Float`, nunca opcional. `SELECT "id", <columnas> FROM "<coleccion>" [WHERE <soft-delete>] ORDER BY "<campo>" {DESC|ASC} LIMIT 1` -- CERO filas de más viajan del motor al proceso, y `@softDelete` (§3.78) se respeta exactamente igual que en `all`/`page`/`sumBy`/etc.

**`Value::Null` sobre una colección vacía (o completamente soft-deleteada), nunca un error.** Coherente con `find(id)` (que ya devuelve `T?`) y con el resto de los métodos que pueden legítimamente "no encontrar nada".

**Verificado**: 5 tests en `checker.rs` (tipa devolviendo `T?`, rechaza un campo no numérico, rechaza una expresión derivada como selector, exige exactamente 1 argumento) + 2 en `runtime/mod.rs` contra un SQLite en memoria real (`maxRow`/`minRow` encuentran la fila correcta -- no la de menor `id` -- reproduciendo el bug exacto de `getBestArm()`; una colección vacía da `null`) + 1 en `pg_integration.rs` contra un PostgreSQL real, confirmando el mismo `ORDER BY ... LIMIT 1` en los dos backends.

---

### 3.103 `Float` decodifica `numeric`/`decimal` nativo de Postgres — RESUELTO

Segundo bug real reportado por MyFinance, encontrado por ellos mismos verificando EN SU PROPIO ESQUEMA el fix de fechas de §3.91: "`Float` no decodifica columnas `numeric` de Postgres -- `error deserializing column 1`, probado con una tabla mínima de una sola columna". Cierto -- `numeric` es un formato binario de PRECISIÓN ARBITRARIA, TOTALMENTE distinto de IEEE754 (`float4`/`float8`), y `postgres-types` no implementa `FromSql<f64>` para él. Es justo el tipo que casi cualquier columna de DINERO real usa (`subtotal`, `total_final`, `base_imponible`, `resultado` en el caso reportado) -- nunca `float8`, precisamente por el error de redondeo binario que `numeric` evita. Antes de esta ronda, cualquier `rpc` que leyera la fila completa de una tabla adoptada con montos `numeric` fallaba en runtime, sin importar si el campo se declaraba `Float` (`try_get::<_, f64>` exige el OID EXACTO) o `String` (el wire binario de `numeric` tampoco es texto UTF-8 válido) -- el mismo par de síntomas que ya tenía el bug de fechas de §3.91, esta vez sobre `numeric` en vez de `date`/`timestamp`.

**La corrección** decodifica el binario CRUDO a mano (`PgNumeric`, `runtime/store.rs`): `int16 ndigits`, `int16 weight`, `int16 sign` (`0x0000` positivo, `0x4000` negativo, `NaN`/`Infinity` rechazados con un error claro en vez de adivinar), `int16 dscale` (irrelevante para el valor, solo escala de display), y `ndigits` dígitos de BASE 10000 -- el valor es `signo × Σ dígito[i] × 10000^(weight − i)`. Mismo espíritu que `PgTimestampMicros`/`PgDateDays` (§3.91): un formato chico y documentado por el propio protocolo no amerita sumar una dependencia nueva (`rust_decimal` u otra) -- y como el destino declarado en c-script sigue siendo `Float` (`f64`), no un tipo decimal propio, decodificar directo a `f64` no pierde nada que `Float` ya no perdiera de por sí. `postgres_float_cell` prueba `float4`/`float8` primero (la convención propia de c-script) y `numeric` después, mismo criterio de "probar en orden" que `postgres_int_cell`/`postgres_timestamp_cell`.

**Alcance: solo LECTURA**, igual que §3.91 -- escribir (`insert`/`applyPatch`) contra una columna `numeric` nativa adoptada sigue sin funcionar (`Cell::Float` sigue codificando siempre como `float8` IEEE754 al escribir, sin importar el `ty` real de la columna destino); no era parte del caso reportado, que es sobre RPCs que solo leen filas ya existentes (Modelo 130/303/347 de MyFinance).

**Verificado**: 1 test en `pg_integration.rs` contra un PostgreSQL real -- una tabla con columnas `numeric(12,2)`/`numeric` sin escala declarada, sembrada con SQL crudo, incluyendo un valor NEGATIVO y un entero exacto (no solo el caso fácil de un positivo con decimales) -- confirmado con `--adopt-existing` de punta a punta.

---

### 3.104 Escribir un `Int` contra una columna Postgres no-`BIGINT` (`SERIAL`/`SMALLINT`) — RESUELTO

Encontrado auditando por qué CI llevaba varios pushes seguidos en rojo sin que nadie lo hubiera notado (`gh run list` -- ver la nota de proceso al final de esta sección): el test que §3.59 ya citaba como "verificado" (`a_preexisting_table_with_a_32_bit_serial_id_accepts_inserts_and_reads`) en realidad nunca había corrido contra un Postgres real hasta ahora -- ese caveat lo admitía explícitamente ("sin verificar en esta sesión"), y nadie volvió a confirmarlo después. Corriéndolo de verdad apareció un bug DISTINTO al que §3.59 arregló: un `insert` sobre una tabla con `id SERIAL` (`int4`) fallaba con un genérico `db error` -- no en el `INSERT` en sí, sino en la lectura de la fila recién insertada (`SELECT ... WHERE "id" = $1`) inmediatamente después.

**La causa**: `i64::to_sql` (la implementación de `postgres-types` para el tipo Rust `i64`, generada por su macro `simple_to!`) IGNORA por completo el `ty` que el servidor le pasa -- siempre serializa 8 bytes de `int8`, sin importar qué ancho pidió el servidor. Contra `WHERE "id" = $1` sobre una columna `int4`/`int2`, el servidor infiere `$1` como ESE ancho -- mandar 8 bytes ahí corrompe el protocolo binario. Es el mismo problema que `postgres_int_cell` (§3.59) ya resuelve del lado de LECTURA, pero del lado de ESCRITURA: bindear un parámetro `Int` (no solo `"id"` -- cualquier campo `Int` normal guardado como `INTEGER`/`SMALLINT` en una tabla adoptada) contra una columna de ancho distinto a `BIGINT` estaba roto.

**La corrección**: `Cell::to_sql` (`runtime/store.rs`) ahora despacha por el `ty` real que el servidor pide -- `INT2`/`INT4` codifican como `i16`/`i32` (con un error claro, no un truncado silencioso, si el valor no entra en ese ancho), cualquier otro caso sigue como `i64`. A diferencia de la lectura (que prueba varios anchos porque no sabe de antemano cuál es el real), en escritura no hace falta probar: el servidor YA dice el ancho exacto en `ty`, alcanza con despachar por él.

**Verificado**: el test de §3.59 (`a_preexisting_table_with_a_32_bit_serial_id_accepts_inserts_and_reads`) pasa contra un Postgres real por primera vez. Además se corrigieron, en la misma auditoría, cuatro tests de `pg_integration.rs` que TAMBIÉN llevaban tiempo rotos por bugs propios de los TESTS (no del compilador) que nunca se habían corrido de verdad:
- Un test de `Timestamp` nativo (§3.91) declaraba campos en camelCase (`fechaEmision`) contra columnas físicas en snake_case (`fecha_emision`) -- c-script no convierte nombres, tenían que matchear exacto.
- Un test de `@check` (§3.96) comparaba `format!("{err}")` contra `"check"` -- pero `postgres::Error::Display` es SIEMPRE el literal fijo `"db error"` para cualquier error del servidor, en cualquier locale; el detalle real vive en `.as_db_error().message()`, nunca en `Display`.
- Un test de aviso de colisión de tabla (§3.94) declaraba un campo `String` requerido que, tras la migración no destructiva de la segunda tabla, quedaba `NULL` en la fila ya sembrada -- disparando el guard correcto y deliberado de "fila con NULL en un campo requerido" (§9.1.1) en vez de lo que el test quería probar. Se corrigió a `String?`.
- Un test de `linkc introspect` (§3.66) reusaba el `db {{ ... }}` COMPLETO del introspect de la base entera como programa a correr -- como introspect escanea TODA la base y `cargo test` no serializa los tests por default, una tabla de OTRO test corriendo en paralelo (con un `id UUID`, rechazado a propósito por §3.36) se colaba y hacía fallar la conexión. Se corrigió para extraer solo el tipo relevante del output y armar un programa acotado a mano.

**Nota de proceso, la más importante de esta sección**: estos cinco bugs (uno de producto, cuatro de tests) llevaban en rojo desde v1.58.0 como mínimo -- **~10 versions consecutivas pusheadas sin que ninguna verificación real de esta sesión los hubiera detectado**, porque nunca se corrió `pg_integration.rs` contra un Postgres real localmente (el entorno de desarrollo no tenía uno disponible) y nadie chequeó el estado de CI en GitHub después de cada push. El snapshot `examples/users.link.snap` (que embebe el número de versión exacto) también llevaba desde v1.48.0 sin regenerar, rompiendo el mismo job por un motivo aparte. Ambos indican el mismo hueco de proceso: "tests verdes localmente" y "CI verde" no son lo mismo, y solo el segundo es la promesa real. Corregido yendo hacia adelante: verificar contra Postgres real cuando esté disponible, y confirmar el estado de CI (`gh run list`) después de pushear, no asumirlo.

---

### 3.105 `db.<c>.increment(id, selector, delta) -> T` — RESUELTO, alcance acotado

PLAN.md §9.3, gap nuevo encontrado analizando IgnisLove en profundidad, **con un riesgo de producción real y confirmado como evidencia (lost-update, no un bug ya materializado sino uno estructuralmente posible en la topología real de este adoptador)**. Sin una forma atómica de incrementar un campo, tres `.link` (`bandit_rewards`, `bot_defense`, `banners`) hacían read-then-write manual -- `upsert` con un `updateFn` que lee `existing.campo + 1` -- para contadores (`totalPulls`, `requestCount`, `impressionsCount`/`clicksCount`). En la topología real de este adoptador (varios procesos `linkc serve-all`/pm2 compartiendo un único Postgres, confirmado en `server/cscript-gateway.ts`), dos procesos pueden leer el mismo valor antes de que el otro escriba y perder un incremento -- el `updateFn` de `upsert` corre en el INTÉRPRETE, no dentro de una transacción SQL, así que no hay nada que lo proteja.

```
type Counter = { id: Int, hits: Int }

service Analytics {
  rpc bump(id: Int) -> Counter { db.counters.increment(id, |c: Counter| { c.hits }, 1) }
}
```

**Un `UPDATE "campo" = "campo" + ?` real, sin ninguna lectura previa.** A diferencia de `upsert`/`applyPatch` (que arman el nuevo valor en Rust y lo mandan como literal), acá el incremento pasa DENTRO de la sentencia SQL -- la atomicidad la da el propio motor (row-level locking de una `UPDATE`), no ningún mecanismo de c-script. `delta` negativo decrementa -- no hay un método `decrement` aparte, sería la misma sentencia con el signo dado vuelta.

**Mismo shape reconocido que `maxRow`/`minRow`/`maxBy`/`minBy` (`field_selector`), pero acá el campo tiene que ser escribible de verdad.** Alcance deliberadamente acotado a `Int` en esta ronda -- `Int64`/`Float` quedan afuera a propósito: los casos reales que motivan esto son todos contadores `Int`, y no hay ninguna barrera técnica que lo impida (a diferencia de `List<Int>.sum()`, §3.101, acá no hay ambigüedad de tipo posible -- el tipo de columna siempre se conoce estáticamente vía `ColumnPlan`), pero ampliar sin evidencia real de demanda sería adivinar.

**`id` que no existe es un error claro, mismo criterio que `applyPatch`.** El `UPDATE` sobre un `id` inexistente afecta 0 filas silenciosamente (comportamiento normal de SQL); la reconsulta por `id` inmediatamente después es la que detecta "no existe" y falla con un mensaje que nombra el `id` y la colección -- un solo camino para los dos casos, sin necesitar un chequeo de existencia previo.

**Composición gratis con features existentes, sin código nuevo.** Un `@check(min/max/range, ...)` en el campo incrementado se sigue enforceando -- el `UPDATE` pasa por la MISMA base con el MISMO `CHECK` inline (§3.96), y `write_error` ya traduce esa violación a 400 igual que en `insert`/`applyPatch`. `id` directo (sin filtro de `@softDelete`, §3.78) -- mismo criterio que `find`/`applyPatch`, una fila soft-deleteada sigue siendo alcanzable por `id`.

**Fuera de alcance a propósito, documentado en vez de escondido:** si la colección tiene un campo `@autoUpdate` (§3.77), `increment` NO lo pisa a `now()` -- a diferencia de `applyPatch`/`upsert`, que sí lo hacen. Usar `applyPatch` en su lugar si hace falta actualizar `updatedAt` a la vez que un contador.

**Verificado**: 5 tests en `checker.rs` (tipa devolviendo `T` no `T?`, rechaza `Int64`, rechaza un `delta: Float`, rechaza una expresión derivada como selector, exige exactamente 3 argumentos) + 2 en `runtime/mod.rs` contra un SQLite en memoria real (incremento y decremento correctos; `id` inexistente da un error claro) + **1 en `pg_integration.rs` que es la prueba real del punto entero de esta feature**: 20 hilos, cada uno con su propia conexión HTTP, incrementando la MISMA fila 25 veces cada uno (500 incrementos concurrentes en total) contra un Postgres real -- el conteo final da EXACTO, sin perder ni uno, algo que un `upsert` con `updateFn` de lectura-previa perdería con altísima probabilidad bajo esa misma concurrencia.

---

### 3.106 Lint `delete-then-insert-same-id` — RESUELTO

PLAN.md §9.3, gap nuevo encontrado analizando IgnisLove en profundidad: varios `.link` del repo (`bandit_rewards`, `bot_defense`, `stock_cache`, `catalog_facets`, `seo_engine`, `rfm_scorer`) tienen un comentario propio explicando por qué migraron de "borrar e reinsertar" a `upsert`/`applyPatch` -- "delete+insert con autoincrement no reproduce el id". `banners.link` todavía no había migrado. El motivo real, no solo de estilo: `insert()` SIEMPRE asigna un id nuevo por autoincrement (§3.17) -- nunca respeta el valor que un literal declara para el campo `id`, así que `db.<c>.delete(x.id); db.<c>.insert(T { id: x.id, ... })` NO preserva la fila, aunque el código parezca intentarlo escribiendo `id: x.id` explícito. Cualquier referencia externa al id viejo (otra tabla, un cliente que guardó ese id) queda apuntando a una fila que ya no existe.

```
type Banner = { id: Int, name: String, impressionsCount: Int }

rpc bump(x: Banner) -> Void {
  db.banners.delete(x.id);
  db.banners.insert(Banner { id: x.id, name: x.name, impressionsCount: x.impressionsCount + 1 });
}
```

**Shape detectado, mismo criterio "chico y ancho, no un intérprete de expresiones parcial" que el resto del linter:** dentro de un mismo bloque (`fn`/`rpc`/`test`, incluido el cuerpo de un `while`), un `db.<c>.delete(X)` seguido -- en cualquier punto MÁS ADELANTE del mismo bloque, no necesariamente la sentencia inmediata siguiente -- de un `db.<c>.insert(Tipo { id: X, ... })` sobre la MISMA colección, con la MISMA expresión `X` en los dos lados (comparada estructuralmente, limitado a `Ident`/`campo.anidado`/literal `Int` -- cualquier otra forma no dispara, nunca un falso positivo por adivinar una equivalencia). Borrar de una colección e insertar en OTRA (archivar) no dispara -- distinta colección. Borrar una fila e insertar una fila DISTINTA en la misma colección tampoco -- distinto id, evidencia de que no es un intento de "actualizar".

**Puramente informativo**, como el resto del linter -- `linkc lint` sigue saliendo con código 0. El mensaje recomienda `applyPatch`/`upsert` en su lugar.

**Verificado**: 4 tests en `lint.rs` -- el caso real exacto de `banners.link` dispara; colección distinta no dispara; id distinto no dispara; un `insert` sin ningún `delete` antes no dispara.

---

### 3.107 `linkc serve-all --port-map-out <archivo.json>` — RESUELTO

PLAN.md §9.7, gap nuevo encontrado analizando IgnisLove en profundidad: `serve-all` (v1.56.0, §3.92) asigna puerto por orden ALFABÉTICO de los `.link` descubiertos en el directorio -- una regla determinística, pero nada externo puede LEERLA salvo replicándola a mano. `server/cscript-gateway.ts` (gateway de producción real de este adoptador, proxeando 13 servicios) hardcodea un mapa `nombre → puerto`, con un comentario propio admitiendo el riesgo: "tiene que actualizarse si algún día se añade, quita o renombra un `.link` en ese directorio".

```bash
linkc serve-all ./services --port-base 3000 --port-map-out ./services/ports.json
```

**Escribe `{"nombre_archivo": puerto, ...}` a un JSON, ANTES de arrancar cualquier servicio.** La clave es el nombre del archivo sin `.link` -- lo que un router/gateway externo usaría para identificar el servicio; el valor es el puerto real asignado. Es lo ÚLTIMO que corre antes de servir: si la escritura falla (el directorio destino no existe, sin permiso), `linkc serve-all` sale con error y NO arranca ningún servicio -- mejor no levantar nada que dejar un gateway leyendo un mapeo viejo o inexistente mientras los servicios sí están corriendo.

**No cambia CÓMO se asigna el puerto, solo lo hace LEGIBLE.** El orden alfabético (§3.92) sigue exactamente igual, siempre RE-ESCRITO entero en cada arranque -- `--port-map-out` nunca lo lee de vuelta, es de solo escritura. `--port-registry` (§3.153) es el flag hermano para cuando lo que hace falta no es solo LEER la asignación sino que se mantenga ESTABLE entre corridas -- los dos aceptan la misma forma de archivo, pero sirven propósitos distintos y pueden combinarse.

**Verificado**: 2 tests en `cli_serve_all.rs` contra el binario real -- el JSON escrito antes de servir tiene la asignación real y correcta (dos `.link`, orden alfabético confirmado); un destino sin permiso de escritura (directorio padre inexistente) falla limpio y NO arranca ningún servicio, confirmado con un único intento de conexión (no un loop de reintentos -- ver la nota de proceso más abajo) más el mensaje de error en stderr.

**Nota de proceso**: escribir el test de "falla limpio" reveló que `wait_for_port` (el helper de este archivo, ajustado para el caso "¿abrió a tiempo?") no es apto para probar lo contrario ("¿nunca abrió?") -- en este entorno de desarrollo, un `connect()` a un puerto sin nada escuchando puede tardar bastante más que instantáneo, así que un loop de 200 reintentos sobre ESE caso podía tardar minutos en vez de segundos. Un solo intento de conexión (mismo criterio que el test ya existente de `a_type_error_in_one_link_file_aborts_the_whole_workspace_before_starting_anything`) resuelve esto sin ambigüedad.

---

### 3.108 `countWhere`/`findWhere` empujan a SQL `!=`/`<`/`<=`/`>`/`>=` — RESUELTO, alcance acotado

PLAN.md §9.3.1, reforzado por "CRM"/Nexus (analizado por primera vez en la ronda de IgnisLove/CRM/Glowapp): `countWhere`/`findWhere` (§3.95, v1.59.0) solo empujaban a SQL el caso `|x| x.campo == valor` -- cualquier otro operador caía al camino interpretado (traer la colección entera, filtrar en Rust). Tres casos reales de ALTA FRECUENCIA en CRM (llamados en cada carga de página, no en un backfill puntual) evidenciaron la falta: `notifications.link` (badge de notificaciones, `n.userId == uid && !n.read`), `inventory.link` (alerta de stock bajo, `p.stock <= 5 && p.stock > 0`), `chat.link` (contador de chats sin leer, `c.unreadCount > 0`).

```
type Chat = { id: Int, name: String, unreadCount: Int }

rpc unreadChatCount() -> Int { db.chats.countWhere(|c: Chat| { c.unreadCount > 0 }) }
```

**`ast::recognize_comparison_predicate` generaliza el reconocimiento de shape** (antes `recognize_equality_predicate`, solo `==`) a los cinco operadores relacionales restantes: `!=`/`<`/`<=`/`>`/`>=` -- mismo criterio conservador de siempre, `|item: T| item.campo OP valor` (en cualquier orden; `valor OP item.campo` también reconocido, con el operador "enderezado" -- `<` se invierte a `>`, etc. -- para que el generador de SQL siempre reciba "campo OP valor"). `runtime/db.rs::comparison_condition` (antes `equals_condition`) genera `"<campo>" <op-sql> ?` con el operador correspondiente, compartido entre `count_where_compare`/`find_where_compare` (antes `count_where_equals`/`find_where_equals`).

**Alcance deliberadamente acotado a UN SOLO operador por predicado en ESTA ronda -- `&&` compuesto resuelto en §3.109 (la ronda siguiente), `||` sigue sin pushear.** De los tres casos reales de CRM citados arriba, solo `chat.link` (`c.unreadCount > 0`, un único operador) se benefició directamente de esta ronda -- `notifications.link` e `inventory.link` combinan DOS condiciones con `&&`, resueltas en §3.109. Cualquier predicado con `||`, un campo derivado, o una comparación entre DOS campos del propio parámetro sigue cayendo al camino interpretado de siempre -- correcto en cualquier caso, solo más lento en ese caso puntual, nunca silenciosamente incorrecto.

**Verificado**: 1 test en `runtime/mod.rs` contra un SQLite en memoria real cubriendo los cinco operadores nuevos (incluido el caso del campo del lado derecho, confirmando que da el mismo resultado que el campo a la izquierda) + 1 en `pg_integration.rs` contra un PostgreSQL real con el caso exacto de `chat.link`. El test existente que usaba `r.rating > 3` como ejemplo de predicado NO pusheable se corrigió (primero a un `&&` compuesto en esta ronda, después a un `||` en §3.109 cuando `&&` también empezó a pushear) -- ese operador solo YA es pusheable desde esta ronda, así que ya no servía como ejemplo de lo que cae al camino interpretado.

**Nota (§3.109): las funciones citadas arriba (`recognize_comparison_predicate`, `comparison_condition`, `count_where_compare`/`find_where_compare`) fueron renombradas y generalizadas una ronda después para admitir una conjunción `&&` de varias hojas -- ver §3.109 para los nombres actuales (`recognize_conjunction_predicate`, `conjunction_condition`, `count_where_conjunction`/`find_where_conjunction`). El comportamiento de un solo operador descrito acá no cambió, solo el nombre de la función que lo implementa.**

---

### 3.109 `countWhere`/`findWhere` empujan una conjunción `&&` de varias hojas — RESUELTO, alcance acotado

PLAN.md §9.3 ítem 1, reforzado por el pedido explícito del usuario tras confirmar que el pushdown de un solo operador (§3.108) todavía dejaba sin resolver dos de los tres casos reales de CRM que lo motivaron: `notifications.link` (`n.userId == uid && !n.read`) e `inventory.link` (`p.stock <= 5 && p.stock > 0`, el MISMO campo dos veces). Antes de esta ronda, cualquier `&&` en un predicado de `countWhere`/`findWhere` hacía fallar el reconocimiento de shape ENTERO -- una fila más de la tabla completa traída a memoria en cada llamada, exactamente el patrón que motivó todo §9.3.

```
type Notification = { id: Int, userId: Int, read: Bool }

rpc unreadFor(userId: Int) -> Int {
  db.notifications.countWhere(|n: Notification| { n.userId == userId && !n.read })
}
```

**`ast::recognize_conjunction_predicate` reemplaza y generaliza `recognize_comparison_predicate`** (§3.108, un único operador -- una conjunción de una sola hoja es exactamente ese caso, sin código duplicado): recorre el árbol de `&&` recursivamente, reconociendo cada hoja con el MISMO criterio conservador de siempre (`item.campo OP valor`, en cualquier orden, con el operador "enderezado" si el campo aparece a la derecha). Dos formas de hoja NUEVAS, sin ningún operador de comparación explícito: `!item.campo` (equivale a `item.campo == false`) e `item.campo` solo (equivale a `item.campo == true`) -- necesarias porque `!n.read` es exactamente la forma real de `notifications.link`. Como no existe ningún literal `false`/`true` en el código fuente al que apuntar para esas dos formas, la hoja devuelve un `PredicateOperand` nuevo (`Expr(&Spanned<Expr>)` para el caso de siempre, o `Bool(bool)` para un booleano sintetizado) en vez de solo una referencia a una expresión del AST.

**`runtime/db.rs::conjunction_condition` reemplaza y generaliza `comparison_condition`**: en vez de un solo `(String, Cell)`, arma `"campo1" op1 $1 AND "campo2" op2 $2 AND ...` con un placeholder POSICIONAL por cada hoja (`$1`, `$2`, ... en Postgres; `?` repetido en SQLite, donde la posición no importa) y devuelve `(where_clause, Vec<Cell>)`. `count_where_conjunction`/`find_where_conjunction` (antes `count_where_compare`/`find_where_compare`) toman `&[(String, BinaryOp, Value)]` en vez de un trío suelto. El soft-delete (§3.78) se sigue AND-eando al final de la cláusula completa, no por hoja.

**Alcance deliberado, igual que §3.108: solo `&&`, `||` sigue sin pushear.** Los dos casos reales citados arriba son conjunciones PURAS -- ninguno de los reportes de adopción cita un `||` de alta frecuencia todavía. `||` necesitaría una cláusula `OR` separada en el SQL generado (`(a AND b) OR (c AND d)`, en general), una forma bastante más rica que agregar hojas a una lista plana -- queda explícitamente para una ronda dedicada si aparece evidencia real. Una comparación entre DOS campos del propio parámetro (`endDate > startDate`) tampoco está cubierta -- mismo motivo de siempre (sin forma de expresar "columna vs. columna" en el valor bindeado). `deleteWhere` sigue sin ganar ESTE atajo tampoco (mismo motivo de §3.95: publicar cada fila borrada a `stream` complica un `DELETE ... WHERE` de una sola sentencia).

**Verificado**: 1 test nuevo en `runtime/mod.rs` contra un SQLite en memoria real cubriendo los dos casos reales de CRM (`&&` con `!campo`, y `&&` con el MISMO campo dos veces) más las dos hojas booleanas sueltas (`x.campo`/`!x.campo` sin `&&`) + 1 en `pg_integration.rs` contra un PostgreSQL real con el caso exacto de `notifications.link`, confirmando que el `AND` con dos placeholders posicionales (`$1`/`$2`) bindea en el orden correcto. El test existente que usaba un `&&` compuesto como ejemplo de predicado NO pusheable (agregado en §3.108 para reemplazar el ejemplo de un solo operador, que ese mismo cambio volvió pusheable) se corrigió otra vez, ahora a un `||` -- el mismo patrón exacto que motivó la nota de §3.108 de arriba, un recordatorio de que "el ejemplo de lo no soportado" necesita revisarse cada vez que el alcance soportado crece.

---

### 3.110 `crypto.awsS3PresignedUrl(...)`: URLs firmadas reales para Amazon S3 — RESUELTO, alcance acotado

Gap NUEVO (24/08/2026), reportado por un adoptador real ("MyFinance"): `DocumentStorageService` necesitaba generar una URL firmada para compartir/descargar un documento desde S3, y terminó con una firma FALSA -- `?signature=hmac_verified`, un string LITERAL, no un HMAC de verdad -- porque `crypto.hmacSha256` (§3.38) no alcanzaba. El motivo no era negligencia: es una limitación real y verificable del primitivo existente. AWS Signature Version 4 deriva su clave de firma encadenando CUATRO HMAC-SHA256, donde el resultado CRUDO (los 32 bytes del digest) de cada paso es la CLAVE del siguiente:

```
kDate    = HMAC-SHA256("AWS4" + secretAccessKey, dateStamp)
kRegion  = HMAC-SHA256(kDate,   region)
kService = HMAC-SHA256(kRegion, "s3")
kSigning = HMAC-SHA256(kService, "aws4_request")
firma    = Hex(HMAC-SHA256(kSigning, stringToSign))
```

`crypto.hmacSha256(secret: String, message: String) -> String` siempre toma y devuelve `String` -- su salida es la representación HEX del digest, no los bytes. Pasar esa hex de vuelta como `secret` del siguiente paso firma con la clave EQUIVOCADA (los bytes UTF-8 del texto hexadecimal, no los 32 bytes reales que representa) y produce una firma que no es la que AWS calcula. No hay forma de rodear esto desde c-script: el lenguaje no tiene (a propósito) un tipo de bytes crudos -- así que el encadenado tiene que resolverse DENTRO del runtime, en Rust, antes de que el resultado cruce a un `Value::Str`.

```
type Factura = { id: Int, s3Key: String }

rpc urlDeDescarga(f: Factura) -> String {
  crypto.awsS3PresignedUrl(
    env.get("AWS_ACCESS_KEY_ID"), env.get("AWS_SECRET_ACCESS_KEY"),
    "eu-west-1", "mis-facturas", f.s3Key, 3600
  )
}
```

**`crypto.awsS3PresignedUrl(accessKeyId: String, secretAccessKey: String, region: String, bucket: String, objectKey: String, expiresSeconds: Int) -> String`** arma la URL COMPLETA lista para usar (`https://<bucket>.s3.<region>.amazonaws.com/<objectKey>?X-Amz-Algorithm=...&...&X-Amz-Signature=...`), no solo la firma -- a diferencia de exponer un primitivo de firma genérico (que hubiera dejado en manos del `.link` de cada adoptador la construcción del "canonical request" y el URI-encoding EXACTO que AWS exige, la misma clase de trabajo fino y propenso a error que llevó a MyFinance a dejar un placeholder en primer lugar), esta función resuelve el protocolo COMPLETO adentro del runtime. `expiresSeconds` fuera de `1..=604800` (7 días, el máximo que AWS acepta con credenciales de larga duración) es un error de runtime limpio, nunca una URL con una expiración inválida.

**Alcance deliberado de esta ronda:**

- **Solo `GET` (compartir/descargar), no `PUT`/subir.** El caso real reportado es "generar un link de descarga" -- una URL presignada para SUBIR necesita, además, que el cliente mande el `Content-Type`/tamaño exactos que la firma prevé, un contrato más amplio entre quien genera la URL y quien la usa que el caso de descarga no tiene. Queda para una ronda dedicada si aparece evidencia real de demanda.
- **Solo credenciales de larga duración (access key + secret key), sin `X-Amz-Security-Token`.** Credenciales temporales de AWS STS no están cubiertas.
- **Estilo "virtual-hosted" (`bucket.s3.region.amazonaws.com`) siempre**, nunca el estilo de path (`s3.region.amazonaws.com/bucket`). Un nombre de bucket con puntos (que rompe el certificado TLS wildcard del estilo virtual-hosted) no tiene manejo especial -- caveat conocido y documentado de AWS, no algo que esta función intente resolver.
- **Sin llamar a AWS para nada.** Es una función PURA -- ninguna request sale del proceso. Verificar que el resultado funciona de verdad contra un bucket real (permisos, que el objeto exista, que la cuenta esté bien configurada) sigue siendo responsabilidad de quien la usa; c-script no puede confirmar eso sin credenciales reales de AWS, que esta ronda no tenía disponibles.

**Verificado sin necesitar una cuenta de AWS real -- contra el vector de prueba OFICIAL que Amazon publica** (`aws4_testsuite`, el mismo estándar que ya se usó para `crypto.hmacSha256` en §3.38, verificado ahí contra un vector de Python en vez de una cuenta de Stripe real): la derivación de clave + firma final reproduce BYTE A BYTE el resultado publicado para el caso "get-vanilla" (`accessKeyId=AKIDEXAMPLE`, fecha `2011-09-09 23:36:00 GMT`, región `us-east-1`) -- `b27ccfbfa7df52a200ff74193ca6e32d4b48b8856fab7ebf1c595d0670a7e470`. El formateo de fecha (`YYYYMMDD`/`YYYYMMDDTHHMMSSZ`) se confirmó contra ese mismo caso y contra la fecha del ejemplo oficial de "URL presignada de GET Object" de la documentación de AWS (`2013-05-24T00:00:00Z`). El URI-encoding (`aws_uri_encode`) se confirmó contra el vector oficial `get-vanilla-query-unreserved` (qué caracteres NO se codifican) más los casos de `/` codificado/preservado según haga falta (valor de query vs. componente de path). El builtin completo, de punta a punta vía un servidor real, se probó por ESTRUCTURA (host virtual-hosted-style, los cinco parámetros `X-Amz-*` en el orden que S3 espera, una firma de 64 caracteres hex) en vez de por match exacto de string -- el timestamp interno (`SystemTime::now()`) hace que un match byte a byte contra un vector fijo sea imposible sin inyección de reloj, que este runtime no tiene. 5 tests nuevos en `runtime/mod.rs` + `checker.rs` + `runtime/timestamp.rs` en total.

**Nota de proceso.** El primer intento de resolver este reporte fue sugerirle al adoptador que armara la firma "a mano" con `crypto.hmacSha256` -- una recomendación que resultó ser INCORRECTA al verificarla (exactamente el motivo por el que no alcanza, explicado arriba). El error se detectó antes de comunicarlo como solución final, pero es la razón por la que esta sección existe como una función nueva del compilador en vez de quedar como "ya se puede hacer con lo que existe".

---

### 3.111 `response.redirect(url, permanent)`: redirects HTTP reales — RESUELTO

PLAN.md §9.9 ítem 6 (sección de SEO y descubribilidad para IA, abierta el 24/08/2026 a pedido explícito del usuario): un redirect 301/302 es una pieza básica de SEO clásico -- consolidar contenido duplicado, mandar una URL vieja a la nueva sin perder el "link juice" que un buscador le asignó -- pero `response.setStatus(code)` (§3.46) por sí solo no alcanza: fijar el status a 301/302 sin un header `Location` apuntando a algún lado no es un redirect, es un código de status sin sentido para el cliente.

```
type Post = { id: Int, slug: String, oldSlug: String? }

@route("/blog/:slug")
rpc post(slug: String) -> String {
  let found = db.posts.findWhere(|p: Post| { p.oldSlug == slug });
  if found.length() > 0 {
    // URL vieja: redirect PERMANENTE a la nueva -- un buscador transfiere
    // el ranking de la URL vieja a la nueva en vez de tratarlas como
    // contenido duplicado.
    response.redirect("/blog/" + found[0].slug, true);
    ""
  } else {
    renderPost(slug)
  }
}
```

**`response.redirect(url: String, permanent: Bool) -> Void`** fija el status HTTP (301 si `permanent`, 302 si no) Y el header `Location: <url>` de la respuesta -- mismo mecanismo interno que `response.setStatus` (`Db::response_status_override`, un `Cell` que vive por request, escrito por el cuerpo del rpc y consumido una sola vez por `server.rs` tras un éxito), más un campo hermano nuevo (`response_location_override`, un `RefCell<Option<String>>` -- `String` no es `Copy`, a diferencia de `Option<u16>`) para el destino. Los dos viajan siempre juntos: no existe una forma de fijar el status de un redirect sin su `Location`, ni viceversa.

`url` puede ser relativa (`"/blog/nuevo-slug"`) o absoluta (`"https://otro-dominio.com/x"`) -- HTTP no distingue, y los dos casos son reales (reorganizar rutas del mismo sitio vs. migrar de dominio). Un `url` vacío es un error de runtime limpio. Un `url` con un salto de línea (`\r`/`\n`) TAMBIÉN es un error de runtime limpio, no una URL que se deja pasar tal cual al header -- `url` es un `String` arbitrario que el propio cuerpo del rpc arma (podría concatenar un parámetro de usuario), a diferencia del `Origin` de una request entrante, que ya pasó por el parser de líneas HTTP de `tiny_http` antes de llegar a c-script; dejarlo pasar sin chequear abriría la puerta a inyectar headers HTTP arbitrarios en la respuesta.

**Mismo límite que `response.setStatus` dentro de un `stream` (§3.56), por el mismo motivo exacto**: el status de una conexión SSE es fijo para toda su duración, se decide una sola vez al abrir la respuesta -- un redirect ahí no podría tener ningún efecto, así que es un error de COMPILACIÓN (no un no-op silencioso que solo se nota en producción).

**Alcance de esta ronda**: sin validación de la FORMA de `url` más allá de "no vacío, sin salto de línea" -- un valor que no sea una URL real (`"no es una url"`) se deja pasar tal cual, es responsabilidad de quien escribe el rpc. Sin soporte de redirects relativos resueltos contra la request actual (eso ya lo hace cualquier navegador con una URL relativa, no hace falta que c-script lo resuelva).

**Verificado**: 3 tests de tipos en `checker.rs` (firma correcta, rechazado dentro de un `stream` con el mismo mensaje que `setStatus`, cantidad/tipos de argumento incorrectos) + 1 en `runtime/mod.rs` (URL vacía o con salto de línea rechazada antes de llegar a ningún header) + 1 end-to-end en `cli_content_type.rs` contra un servidor `linkc serve` REAL: `permanent: false` da 302 con el `Location` exacto pedido, `permanent: true` da 301, los dos leídos del socket crudo (headers HTTP reales, no solo un body que los mencione).

---

### 3.112 `base64.encode`/`base64.decode` — YA EXISTÍA, sin documentar ni probar hasta ahora

Auditoría del 25/08/2026, disparada por el pedido explícito del usuario de reducir la fricción de integrar terceros ("debemos dar soporte a la mayor cantidad de proveedores posibles"): investigando qué hacía falta para Twilio (autenticación HTTP Basic, `Authorization: Basic base64(accountSid:authToken)`) apareció que `base64.encode(data: String) -> String`/`base64.decode(base64Str: String) -> String` (RFC 4648 estándar con padding, crate `base64`) **ya existían en el checker y en el runtime desde antes** -- pero en NINGÚN lugar de GRAMMAR.md, README, o `llms.txt`, y sin un solo test que fijara su comportamiento. Exactamente el mismo patrón que llevó al incidente de la firma S3 falsa de MyFinance (§3.110): una capacidad real, invisible para cualquiera (persona o agente de IA) que necesitara encontrarla, así que en la práctica era como si no existiera.

```
rpc callTwilio(accountSid: String, authToken: String, body: String) -> String {
  let credentials = base64.encode(accountSid + ":" + authToken);
  http.postWithHeaders(
    "https://api.twilio.com/2010-04-01/Accounts/" + accountSid + "/Messages.json",
    body,
    [{ name: "Authorization", value: "Basic " + credentials }]
  )
}
```

**`base64.decode` devuelve `String`, no bytes crudos** (mismo límite deliberado de siempre -- GRAMMAR.md §2, "sin tipo Bytes"): si la secuencia decodificada no es UTF-8 válido, es un error de runtime limpio, no una `String` con bytes corruptos. Cubre el caso real de auth (decodificar/codificar texto -- credenciales, JSON, cualquier payload de texto) pero NO sirve para manipular datos binarios arbitrarios (una clave HMAC binaria, una imagen) -- ese es precisamente el límite que hace que Azure Blob SAS (ver la nota de auditoría más abajo) necesite algo más que esto.

**Verificado**: 2 tests nuevos de tipos en `checker.rs` + 2 en `runtime/mod.rs` contra un vector conocido (`"hello"` <-> `"aGVsbG8="`, y el caso real `"ACxxxx:authtoken123"` <-> `"QUN4eHh4OmF1dGh0b2tlbjEyMw=="`, los dos confirmados con el `base64` del sistema, no inventados a mano) más los dos casos de error (base64 mal formado, base64 válido que decodifica a bytes no-UTF8).

**Auditoría de fricción con otros proveedores (PLAN.md, "Integraciones bloqueadas")**, mismo pedido del usuario -- clasificación honesta de qué necesita trabajo nuevo del compilador y qué no:

- **Stripe, SendGrid, y cualquier API con `Authorization: Bearer <token>`**: YA funcionan hoy completas, sin ningún cambio -- `http.postWithHeaders`/`http.getWithHeaders` (§3.47/§3.60) para la llamada, `env.get` + `crypto.hmacSha256` + `request.rawBody()`/`request.header()` (§3.38, que ya cita a Stripe como caso motivador) para verificar el webhook de vuelta. El gap nunca fue el lenguaje -- fue que nada lo decía con un ejemplo copiable.
- **Twilio, y cualquier API con HTTP Basic Auth**: YA funciona hoy completa, gracias a `base64.encode` (recién documentado en esta sección) + `http.postWithHeaders`.
- **Azure Blob SAS (URLs firmadas)**: gap REAL, mismo tipo que AWS S3 (§3.110) -- la firma es un solo HMAC-SHA256 (no encadenado como AWS), pero la clave de la cuenta de Azure viaja en BASE64 (hay que decodificarla a bytes crudos para usarla como clave) y la firma resultante se codifica en BASE64 (no hex, a diferencia de `crypto.hmacSha256`) -- ninguna de las dos cosas es posible con los primitivos de `String` actuales. Candidato concreto para una próxima ronda, verificable contra los ejemplos oficiales que publica Microsoft, sin necesitar una cuenta de Azure real (mismo criterio que AWS).
- **Google Cloud Storage (URLs firmadas V4)**: gap más grande -- la firma es RSA-SHA256 sobre la clave privada de una cuenta de servicio (formato PEM/PKCS8), no HMAC. Necesitaría sumar una crate de firma RSA (excepción nueva a "cero dependencias", como `regex` en su momento) y parsear una clave privada real -- una decisión de alcance propia, no una extensión chica de lo que ya existe.
- **SQS**: usa el MISMO AWS Signature V4 que S3 (§3.110) para autenticar, pero como llamadas de API firmadas (headers), no como URLs presignadas -- `awsS3PresignedUrl` no cubre este caso tal cual, haría falta generalizar la derivación de firma a un primitivo que no esté atado a "armar una URL de S3". Candidato para cuando aparezca evidencia real de demanda.
- **RabbitMQ**: protocolo AMQP binario y con estado -- una categoría totalmente distinta a firmar requests HTTP, necesitaría implementar el protocolo de transporte entero. Bloqueado/diferido, misma categoría que otras integraciones de protocolo completo en PLAN.md §9.12.

---

### 3.113 `@cache_control("...")` por rpc — RESUELTO

PLAN.md §9.9 ítem 6 (SEO y descubribilidad para IA): un CDN o un crawler de IA que respeta cachés necesita saber cuánto tiempo puede confiar en una respuesta sin volver a pedirla -- antes de esto, `linkc serve` no tenía forma de declarar ningún `Cache-Control`, así que toda respuesta salía sin ese header (equivalente a "no cachear nunca", el default más conservador y también el más caro para contenido que cambia poco, como un `sitemap.xml` o una página de blog).

```
@route("/sitemap.xml")
@content_type("application/xml")
@cache_control("public, max-age=86400")
rpc sitemap() -> String { sitemapXml(allUrls()) }
```

**`@cache_control("public, max-age=3600")` fija el header `Cache-Control` de la respuesta de ÉXITO de un rpc**, texto crudo sin parsear -- c-script no valida la gramática interna de `Cache-Control` (`public`/`private`/`no-store`/`max-age=N`/etc.), eso es responsabilidad de HTTP, mismo criterio que `@content_type` no valida tipos MIME. Dimensión ORTOGONAL: se combina libremente con `@route`, `@content_type`, `@requires`/`@authenticated`, `@rate_limit` -- mismo criterio que `@rate_limit` (§3.39), que tampoco restringe con qué otras anotaciones convive.

**Solo en el camino de ÉXITO, nunca en una respuesta de error** -- mismo criterio exacto que `@content_type` (§3.35) y `response.redirect` (§3.111): una respuesta de error (400/401/403/404/429/500) nunca debería quedar cacheada con la política pensada para el caso feliz, así que el header simplemente no se agrega ahí, sin importar qué haya declarado el rpc.

**Rechazado sobre un `stream`**, mismo motivo que `response.setStatus`/`response.redirect` dentro de un `stream` (§3.46/§3.111): una conexión SSE nunca es cacheable de forma sensata (es un flujo de eventos en vivo, no un recurso con un valor fijo que reusar) -- error de COMPILACIÓN, no un no-op silencioso.

**Mecanismo interno**: `Annotation::CacheControl(String)`, mismo patrón que `ContentType`/`Route`/`RateLimit`/`Deprecated` -- parser (`parse_optional_annotation`), accessor (`RpcDecl::cache_control()`), validación dedicada (`check_cache_control_annotation`, vacío/duplicado/`stream` rechazados). A diferencia de `response.redirect` (un override que el CUERPO del rpc fija en runtime, `Db::response_location_override`), esto es ESTÁTICO -- viene directo del AST, así que `server.rs::declared_cache_control` lo resuelve igual que `declared_content_type`, sin pasar por ningún mecanismo de `Cell`/`RefCell` por request.

**Verificado**: 4 tests de tipos en `checker.rs` (combina con `@route`, vacío rechazado, declarado dos veces rechazado, rechazado dentro de un `stream` con el mismo mensaje que `setStatus`/`redirect`) + 2 end-to-end en `cli_content_type.rs` contra un servidor `linkc serve` REAL -- el header aparece exacto en el camino de éxito, FALTA por completo cuando el mismo rpc anotado falla (`panic` forzado, confirma que un 500 nunca hereda la política de caché del éxito), y el caso real combinado (`@route`+`@content_type`+`@cache_control` juntos sobre un sitemap servido por GET) da el header correcto además del Content-Type y el body ya cubiertos por §3.35.

---

### 3.114 Flujo OAuth2 "client credentials" (servidor a servidor) — YA FUNCIONABA, sin un ejemplo que lo dijera

PLAN.md §9.10, mismo pedido explícito del usuario de reducir fricción con la mayor cantidad de proveedores posible. Google APIs, Microsoft Graph, Salesforce, HubSpot y muchas otras APIs empresariales usan OAuth2 "client credentials" para autenticación SERVIDOR A SERVIDOR (sin login de usuario, distinto de OAuth2 "authorization code" -- ese sigue bloqueado, PLAN.md §9.12, porque verificarlo de punta a punta necesita un proveedor de identidad real con una app de prueba registrada). Auditando qué haría falta para esto aparecieron CERO gaps: las tres piezas ya existían.

```
type Header = { name: String, value: String }

rpc callProtectedApi(tokenUrl: String, clientId: String, clientSecret: String, apiUrl: String) -> String {
  let tokenBody = "grant_type=client_credentials&client_id=" + clientId + "&client_secret=" + clientSecret;
  let tokenResponse = http.postWithHeaders(tokenUrl, tokenBody, [
    Header { name: "Content-Type", value: "application/x-www-form-urlencoded" },
  ]);
  let token = json.parse(tokenResponse).access_token;
  http.getWithHeaders(apiUrl, [
    Header { name: "Authorization", value: "Bearer " + token },
  ])
}
```

**Por qué esto compila y corre sin ningún cambio del compilador**: `http.postWithHeaders` (§3.47) ya podía pedir el token con el `Content-Type` que el endpoint de OAuth2 exige; `json.parse(text: String) -> Dynamic` ya existía, y `Dynamic.<cualquier-campo>` type-checkea DEVOLVIENDO `Dynamic` (`Expr::FieldAccess` sobre `Type::Dynamic`, `checker.rs`) -- no hace falta declarar la forma completa de la respuesta del proveedor solo para leer un campo; y un `Dynamic` es asignable donde se espera `String` sin cast explícito (mismo criterio que el resto del lenguaje trata `Dynamic` como escotilla de escape deliberada). El `+` entre `String` y `Dynamic` (`"Bearer " + token`) también tipea, por el mismo motivo. `http.getWithHeaders` hace la llamada real con el token ya en el header `Authorization`.

**Verificado de punta a punta contra DOS servidores HTTP de mentira reales** (no un mock interno del intérprete) -- uno hace de endpoint de token (devuelve `{"access_token":"tok-xyz-789","expires_in":3600}`), el otro de API protegida: confirma que el `client_id`/`client_secret` llegan tal cual al primer servidor, y que el token que ESE servidor devolvió llega EXACTO como `Authorization: Bearer tok-xyz-789` al segundo -- la prueba real de que la extracción del campo `access_token` vía `Dynamic` funciona en runtime, no solo que tipa. 1 test nuevo en `tests/cli_http.rs`.

---

### 3.115 Lint `unused-var`: 14 falsos positivos dentro de closures y struct-literals — RESUELTO

**Issue #11**, reportado por IgnisLove con evidencia excepcional: 3 repros mínimos aislando la causa exacta más una tabla de 14 falsos positivos reales verificados a mano en 7 de los 17 `.link` de esa adopción (`bandit_rewards`, `banners`, `catalog_facets`, `irene_chat`, `reviews`, `rfm_scorer`, `seo_engine`). `linkc lint`'s `unused-var` marcaba como "no usada" una variable cuya ÚNICA aparición (o todas) caían dentro de (A) el `body` de una closure pasada como argumento a `.filter()`/`upsert`/`findWhere`, o (B) el valor de un campo de un struct-literal, cuando ese struct-literal era la expresión de cola del rpc (directa o como argumento de `insert(...)`/`upsert(...)`). Confirmado desde que el check existe (v1.62.0), no una regresión de una release puntual.

```
rpc queryFacetCounts(category: String) -> Int {
  let target = category.toLower();
  db.facets.all().filter(|f: FacetItem| {
    target == "all" || f.category == target  // 'target' usado DOS veces acá
  }).length()
}
```

Antes de esta ronda, `linkc lint` marcaba `target` como `unused-var` en este código -- pese a usarse dos veces, correcto y con intención clara.

**Causa raíz: `expr_count_ident` (el contador de usos que `unused-var` consulta) tenía un `match` sobre `Expr` con seis variantes SIN arm**, todas cayendo al `_ => 0` genérico -- `Closure`, `StructLit`, `Match`, `Index`, `TupleLit`, `TupleIndex`. `Expr::Call` sí recorría sus `args` (por eso una closure pasada como argumento no se perdía del todo), pero al llegar al propio nodo `Expr::Closure` dentro de esos args, el contador no sabía bajar adentro de su `body` -- el mismo motivo exacto por el que `Expr::StructLit` tampoco contaba los VALORES de sus campos. Nada de esto era un problema de diseño del checker/runtime (que sí resuelven estas formas correctamente, GRAMMAR.md §3.9-§3.10) -- el bug vivía únicamente en este contador auxiliar del linter, una segunda implementación aparte que podía (y de hecho llegó a) divergir.

**Arreglado agregando los seis arms que faltaban** a `expr_count_ident` (`lint.rs`): `Index`/`TupleLit`/`TupleIndex` recorren sus sub-expresiones tal cual el resto de las formas ya manejadas; `StructLit` recorre el valor de CADA campo (`fields: Vec<(String, Spanned<Expr>)>`); `Closure` delega en `block_uses_ident` sobre su `body` (mismo mecanismo que ya usaban los dos brazos de un `if`); `Match` recorre el `scrutinee`, el `guard` de cada arm si lo tiene, y el `body` de cada arm (`MatchArmBody::Expr` o `::Block`, este último también vía `block_uses_ident`). Ningún cambio de comportamiento fuera de este contador -- `unused-var`/`unused-mut`/`--fix` siguen funcionando exactamente igual para el resto de los casos.

**Por qué importaba de verdad, más allá del ruido**: el propio issue lo señala -- si `linkc lint --fix` alguna vez renombra automáticamente estas variables a `_target`/`_reward`/etc. (o si alguien lo hace a mano confiando en el aviso), el prefijo `_` es una señal semántica real ("intencionalmente sin usar", GRAMMAR.md) que ninguna de estas 14 variables merecía -- un falso positivo de este lint, seguido de un `--fix` ciego, rompe código que funciona.

**Verificado**: 5 tests nuevos en `lint.rs` -- los TRES repros del issue reproducidos literalmente (closure de `.filter()`, struct-literal de cola, closures+struct-literals de `upsert`) más un caso de `Expr::Match` (mismo bug de fondo, sin `.link` real citado en el issue pero cubierto de una vez) y un test de no-regresión confirmando que una variable genuinamente sin usar se sigue marcando.

---

### 3.116 `sitemapXml`/`robotsTxt`: builtins declarativos para SEO — RESUELTO

PLAN.md §9.9 ítem 1 (SEO y descubribilidad para IA): antes de esta ronda, un `sitemap.xml`/`robots.txt` se escribía a mano armando el XML/texto como `String` (ver el ejemplo de §3.35) -- fácil de romper el formato (una etiqueta sin cerrar, un carácter especial sin escapar en una URL) sin que nada lo avisara hasta que un crawler real lo rechazara.

```
type Page = { loc: String, lastmod?: Timestamp }
type Rule = { userAgent: String, disallow?: String[], allow?: String[] }

@route("/sitemap.xml")
@content_type("application/xml")
rpc sitemap() -> String {
  sitemapXml(db.pages.all().map(|p: Page| { Page { loc: "https://mi-sitio.com" + p.loc, lastmod: p.lastmod } }))
}

@route("/robots.txt")
@content_type("text/plain")
rpc robots() -> String {
  robotsTxt([
    Rule { userAgent: "GPTBot", disallow: ["/"] },
    Rule { userAgent: "*", allow: ["/"], disallow: ["/admin"] },
  ], "https://mi-sitio.com/sitemap.xml")
}
```

**`sitemapXml(urls: {loc: String, lastmod?: Timestamp}[]) -> String`** arma un `sitemap.xml` bien formado (protocolo sitemaps.org) -- el rpc sigue siendo responsable de la lista de URLs (viene de la base, `@route` no puede inferir rutas dinámicas por sí solo), mismo criterio de "helper que devuelve `String`" que `escapeHtml`, no un motor de templates nuevo. `lastmod` es opcional-por-clave (`x?: T`, no `x: T?`) -- la mayoría de las URLs de un sitio real no tienen (o no vale la pena calcular) una fecha exacta de última modificación, y el protocolo también trata ese elemento como opcional. `loc` se escapa reusando `escape_html` (`&`/`<`/`>`/`"`/`'`) -- el mismo conjunto de caracteres que XML exige escapar en contenido de texto, y sus referencias numéricas (`&#39;` incluido) son válidas en XML tal cual, no solo en HTML, así que no hizo falta una segunda función de escape.

**`robotsTxt(rules: {userAgent: String, disallow?: String[], allow?: String[]}[], sitemapUrl: String?) -> String`** arma un `robots.txt` bien formado -- un bloque `User-agent: ...` por regla, con sus `Disallow`/`Allow` (en ese orden), y `Sitemap: <url>` al final si se pasó una. `disallow`/`allow` opcionales-por-clave, no listas requeridas -- el caso real más común es "solo bloquear" o "solo permitir" un user-agent puntual, así que se puede omitir la lista que no haga falta en vez de escribir `[]` a mano; ausente (campo entero faltante) o presente pero `null` se tratan exactamente igual, ningún `Disallow`/`Allow` para ese bloque.

**Los dos son estructurales, sin nombre** (`Type::Struct { name: None, ... }`, mismo criterio que `http_header_type()`/`http_response_type()` de §3.47/§3.60) -- cualquier `type` que el programa declare con estos campos exactos sirve, sin que el lenguaje tenga que inventar un `SitemapEntry`/`RobotsRule` propio. Los dos son builtins SIN receptor (como `dateFromParts`/`now`, no `crypto.X`/`http.X`) -- cableados en los mismos cinco puntos que ese precedente: tipo en `checker.rs` (`Expr::Ident` + lista de sugerencias "quisiste decir"), y en `runtime/mod.rs`, valor `FnRef` (`Expr::Ident`), despacho directo (`Expr::Call`) y despacho indirecto vía `call_callable` (para `let f = sitemapXml; f(...)`).

**Alcance deliberado: preset de crawlers de IA NO incluido como código.** El ítem 3 original de PLAN.md §9.9 pedía un preset con los user-agents de IA conocidos (`GPTBot`, `ClaudeBot`, `PerplexityBot`, `Google-Extended`, etc.) ya armados adentro del compilador -- se descartó ese diseño a propósito: una lista de bots hardcodeada en el binario se desactualiza cada vez que aparece un crawler nuevo, y arreglarla requeriría una release del compilador en vez de solo cambiar el `.link` del adoptador. `robotsTxt` ya resuelve el caso completo -- un adoptador que quiera bloquear/permitir crawlers de IA específicos simplemente pasa esos `userAgent` como cualquier otra regla, sin que el lenguaje necesite saber sus nombres.

**Verificado**: 4 tests de tipos en `checker.rs` (acepta cualquier `type` con la forma correcta, rechaza uno sin `loc`/`userAgent`, cantidad de argumentos) + 5 en `runtime/mod.rs` -- sitemap con y sin `lastmod` en la MISMA lista (confirma que la ausencia en una entrada no "hereda" el `lastmod` de la anterior), escape de caracteres especiales en `loc`, lista vacía (`<urlset></urlset>` válido, sin ninguna entrada), robots.txt con dos bloques + reglas + sitemap final byte a byte, y un bloque sin `disallow`/`allow`/sitemap (ninguno inventado). Probado a mano además contra un servidor `linkc serve` real vía `curl`, confirmando XML/texto válidos de punta a punta.

---

### 3.117 `metaTags`/`openGraphTags`/`canonicalLink`/`jsonLd`: metadata SEO clásica como helpers de `String` — RESUELTO

Segundo ítem resuelto de PLAN.md §9.9 (SEO y descubribilidad para IA). Antes de esta ronda, meta tags/Open Graph/canonical URL/JSON-LD se escribían a mano concatenando `String` -- fácil de olvidar escapar un valor de usuario dentro de un atributo HTML, o de romper un bloque `<script type="application/ld+json">` si ese valor contenía literalmente `</script>`.

```
type Meta = { name: String, content: String }
type Og = { property: String, content: String }

@route("/producto/:id")
@content_type("text/html")
rpc productPage(id: Uuid) -> String {
  let p = db.products.get(id)
  let head = metaTags([
    Meta { name: "description", content: p.description },
    Meta { name: "robots", content: "index, follow" },
  ]) + "\n" + openGraphTags([
    Og { property: "og:title", content: p.name },
    Og { property: "og:image", content: p.imageUrl },
  ]) + "\n" + canonicalLink("https://mi-sitio.com/producto/" + id.toString())
    + "\n" + jsonLd(json.parse("{\"@context\": \"https://schema.org\", \"@type\": \"Product\"}"))
  "<html><head>" + head + "</head></html>"
}
```

**`metaTags(tags: {name: String, content: String}[]) -> String`** arma una línea `<meta name="..." content="...">` por entrada, separadas por `\n` -- meta tags clásicos (`description`, `robots`, `viewport`, ...) usan el atributo `name`. **`openGraphTags(tags: {property: String, content: String}[]) -> String`** es el mismo mecanismo con el atributo `property` en vez de `name`, porque así es como Open Graph (`og:title`, `og:image`, `og:description`, ...) distingue sus meta tags del resto del `<head>`. Las dos escapan `name`/`property` y `content` con `escape_html` (§3.45) -- `content` suele venir de datos de usuario (título/descripción de un producto real).

**`canonicalLink(url: String) -> String`** arma un `<link rel="canonical" href="...">` -- consolidar contenido duplicado (la misma página accesible por más de una URL) es SEO básico, mismo espíritu que `response.redirect` (§3.111) pero como elemento de `<head>` en vez de un redirect real. `url` se escapa igual que `content` arriba.

**`jsonLd(data: Dynamic) -> String`** arma un bloque `<script type="application/ld+json">...</script>` con `data` serializado a JSON -- mismo serializador interno que `json.stringify` (`value_to_json` + `serde_json::to_string`; §3.114 ya usa el lado `parse` de este mismo par para leer JSON de un proveedor externo). Acepta `Dynamic`, no un `type` estructural fijo, porque un dato JSON-LD real (schema.org tiene decenas de tipos -- `Product`, `Article`, `Recipe`, ...) no tiene una forma que el checker pueda exigir de antemano; el caso de uso normal es `jsonLd(json.parse("..."))` con el JSON-LD armado como texto, o construyendo el `Dynamic` a mano.

**Mitigación de XSS en `jsonLd`**: después de serializar, cada `<` del JSON se reemplaza por su escape Unicode de 4 dígitos hex para el carácter U+003C -- técnica recomendada por OWASP para embeber JSON dentro de un `<script>`. Si `data` viene de contenido de usuario (ej. el nombre de un producto) y ese valor contiene literalmente `</script><script>alert(1)</script>`, sin esta mitigación el navegador cerraría el bloque JSON-LD antes de tiempo y ejecutaría el resto como HTML/JS real -- un JSON válido nunca depende de un `<` literal fuera de un string (no es un delimitador de la gramática JSON), así que el reemplazo no rompe el parseo del lado del navegador.

**Las cuatro son builtins SIN receptor** (como `sitemapXml`/`robotsTxt`/`dateFromParts`/`now`, no `crypto.X`/`json.X`) -- cableadas en los mismos cinco puntos que ese precedente: tipo en `checker.rs` (`Expr::Ident` + lista de sugerencias "quisiste decir"), y en `runtime/mod.rs`, valor `FnRef` (`Expr::Ident`), despacho directo (`Expr::Call`) y despacho indirecto vía `call_callable`. `metaTags`/`openGraphTags` son estructurales sin nombre (mismo criterio que `sitemap_url_type`/`http_header_type`) -- cualquier `type` que el programa declare con los campos exactos sirve.

**Verificado**: 5 tests de tipos en `checker.rs` (acepta la forma correcta, rechaza `property` donde `metaTags` espera `name`, `canonicalLink`/`jsonLd` aceptan cualquier valor asignable a `String`/`Dynamic`) + 5 en `runtime/mod.rs` -- `metaTags` con dos entradas y contenido con comillas/`&` reales, lista vacía (`""`, nada inventado), `openGraphTags` con `property` en vez de `name`, `canonicalLink` escapando `&` en la query string, y `jsonLd` confirmando que el JSON serializado en el medio del bloque `<script>` no contiene ningún `<` literal (así que ningún `</script>` puede aparecer ahí adentro). Probado a mano además contra un servidor `linkc serve` real vía `curl`, confirmando las cuatro salidas byte a byte, incluida la mitigación de XSS de `jsonLd`.

---

### 3.118 `llms.txt` auto-generado por proyecto — RESUELTO

Tercer y último ítem resuelto de PLAN.md §9.9. Convención [llmstxt.org](https://llmstxt.org/) -- **no confundir con el `llms.txt` de ESTE repo** (documenta el COMPILADOR c-script en sí, escrito a mano); este es el `llms.txt` que `linkc build` ahora emite para el proyecto DE QUIEN adopta el lenguaje, junto a `contract.d.ts`/`client.ts`/`validators.ts`/`hooks.ts`/`schemas.ts`/`openapi.json`. Antes de esta ronda, un agente de IA que llegaba a un proyecto c-script sin contexto previo tenía que leer el `.link` completo (o el `openapi.json` generado, mucho más verboso) para entender qué rpcs existen.

```
service Tasks {
  /// Lista todas las tareas pendientes, ordenadas por id.
  rpc list() -> Int { 1 }

  rpc create(title: String) -> Int { 1 }
}
```

`linkc build` de este programa emite, junto al resto de los archivos:

```
# mi_app.link

> API generada automáticamente por Link (c-script). Servicios y rpcs disponibles, cada uno con su firma y (si tiene) su docstring `///`.

## Tasks

- [rpc list() -> Int](/Tasks/list): Lista todas las tareas pendientes, ordenadas por id.
- [rpc create(title: String) -> Int](/Tasks/create)
```

**Un bullet por rpc/stream de cada `service`**, formato de lista de links que llmstxt.org pide (`- [nombre](url): nota`) -- la "URL" es la ruta real `/Servicio/rpc` (GRAMMAR.md §3.20) que ese rpc ya atiende: no es un GET navegable, pero sigue siendo la referencia exacta que un agente necesita para invocarlo, y evita inventar una convención de enlaces propia solo para este archivo. Cada bullet muestra la firma completa (`rpc`/`stream`, nombre, parámetros con tipo, `-> ReturnType`) resuelta por el checker (mismo `Type` que `openapi.json` usa, vía `Display`) -- así un agente ve de un vistazo qué pasar y qué esperar, sin abrir `contract.d.ts`.

**El docstring `///` (§3.72) es la nota después de `:`** -- mismo dato que `openapi_emit` ya usa como `description` de cada operación, reusado tal cual (sin gramática nueva, como pedía PLAN.md §9.9). Un docstring de más de una línea aporta solo la PRIMERA como nota (llmstxt.org espera una línea por entrada); el resto del texto sigue disponible completo en `openapi.json`/`contract.d.ts` para quien necesite el detalle entero. **Un rpc/stream SIN docstring aparece igual, solo sin nota** -- omitirlo por completo escondería una capacidad real de la API, mismo criterio que `openapi_emit` ya sigue (un rpc sin `///` sigue apareciendo en `paths`, solo sin `description`).

**Implementación**: `codegen::llms_txt_emit::emit_llms_txt(program, title) -> Result<String, String>` -- mismo mecanismo que `emit_openapi_json` (`Checker::build_symbols` para resolver tipos sin repetir el chequeo completo del programa), llamado desde `build_once` en `main.rs` junto al resto de los emisores, escribiendo `{outdir}/llms.txt`. `title` es el mismo `display_path` del `.link` de entrada que ya usa `openapi.json` como `info.title`.

**Verificado**: 5 tests en `codegen::llms_txt_emit` -- título + una sección por `service` con un bullet por rpc, docstring como nota, docstring multi-línea aporta solo la primera línea, un rpc sin docstring sigue apareciendo sin nota, y un `stream` se etiqueta distinto de un `rpc` en la firma. Probado a mano además con `linkc build` real sobre un `.link` con dos servicios y un docstring, confirmando el archivo `llms.txt` generado byte a byte.

---

### 3.119 `@example(request: ..., response: ...)`: ejemplos tipados en `openapi.json` — RESUELTO

Último ítem de PLAN.md §9.9 (SEO y descubribilidad para IA), y a diferencia de los ocho anteriores de la sección, este SÍ necesitaba gramática nueva: más allá de la descripción de `///` (§3.72), hacía falta una forma de declarar un ejemplo de request/response REAL, para que un agente que consume la API (o un humano generando código desde el contrato) entienda la forma exacta sin adivinar a partir del tipo solo.

```
type Task = { id: Int, title: String }
type CreateInput = { title: String }

service Tasks {
  @example(response: [Task { id: 1, title: "Comprar leche" }])
  rpc list() -> Task[] { db.tasks.all() }

  @example(request: CreateInput { title: "Comprar leche" }, response: Task { id: 1, title: "Comprar leche" })
  rpc create(title: String) -> Task { db.tasks.insert(Task { id: 1, title: title }) }
}
```

**A diferencia de TODAS las demás anotaciones (`@route`, `@rate_limit`, `@deprecated`, `@cache_control`, ...), sus valores son EXPRESIONES de c-script, no `String` crudo.** El parser reusa `parse_expr` (el mismo que ya arma un `StructLit`/`ArrayLit` normal) en vez de inventar una segunda sintaxis para "JSON dentro de un string" -- misma gramática de par `clave: valor` separado por comas que un literal de struct (`parse_field_init_list`), pero con claves fijas (`request`/`response`, al menos una de las dos) en vez de nombres de campo arbitrarios.

**Las dos expresiones se TIPAN contra la forma real del rpc, con el mismo mecanismo que `= default` de un campo/param (`check_expr` con `Env::new()` vacío, autocontenido)**: `request` contra un struct anónimo armado de los parámetros del rpc (un param con default es opcional ahí también, mismo criterio que `req_props` en `openapi_emit`), `response` contra el `return_type` resuelto. Esto es la diferencia real frente a poner el ejemplo en un comentario o una `String` de JSON a mano: **un ejemplo desincronizado del contrato es un error de compilación**, no un dato que puede mentir en silencio en `openapi.json` para siempre.

**Restringidas a expresiones LITERALES** (`is_literal_expr`, checker.rs: escalares, `Unary(Neg, ...)` sobre un número, y `ArrayLit`/`TupleLit`/`StructLit` recursivamente) -- rechaza cualquier llamada (`crypto.uuid()`, `now()`), variable o acceso a `db`/`http`/etc. Un ejemplo es un valor FIJO conocido en compilación, no algo recalculado en cada build: si `@example(response: crypto.uuid())` tipara, `openapi.json` cambiaría en cada `linkc build` sin que el `.link` cambiara, rompiendo `--diff` (§3.79) de la peor manera posible -- silenciosamente.

**Reglas adicionales, todas en `check_example_annotation` (checker.rs)**: `@example` una sola vez por rpc (mismo criterio que `@cache_control`); `request` solo si el rpc toma parámetros (si no, no hay ningún request body que ejemplificar); rechazado sobre un `stream` (mismo motivo que `@cache_control`/`response.redirect` ahí -- una conexión SSE no tiene una única respuesta que ejemplificar); `@example()` vacío es un error del PARSER (ni siquiera llega al checker), con un mensaje dedicado en vez del genérico "se esperaba un identificador".

**Propagación a `openapi.json`**: `literal_expr_to_json` (`openapi_emit.rs`, hermana de `is_literal_expr` -- el checker ya garantizó que `e` es literal, así que esta conversión es total) arma el JSON y lo pone en `"example"` dentro del Media Type Object correspondiente, mismo nivel que `"schema"` -- `requestBody.content["application/json"].example` para `request`, `responses["200"].content[<content-type>].example` para `response` (respeta `@content_type` si el rpc lo declaró, §3.35). Ningún cambio en `contract.d.ts`/`client.ts`/`schemas.ts` -- alcance deliberadamente atado a lo que PLAN.md pedía, "ejemplos... en `openapi.json`", no una feature nueva de docs en todo el pipeline.

**Verificado**: 4 tests en `parser.rs` (parsea `request`/`response` como expresiones de verdad -- acepta un `StructLit` completo --, `@example()` vacío es un error de sintaxis con mensaje propio, clave desconocida rechazada, clave repetida rechazada) + 7 en `checker.rs` (tipa contra la forma real, rechaza un tipo que no matchea, `request` respeta params con default como opcionales, `request` rechazado sin parámetros, rechaza una llamada como `crypto.uuid()`, rechazado en un `stream`, rechaza declararse dos veces) + 3 en `openapi_emit.rs` (`request`+`response` propagados byte a byte, un ejemplo con solo `response` no toca `requestBody` para nada, sin `@example` no aparece ninguna clave `"example"` de la nada). Probado a mano además con `linkc build` real: caso feliz de punta a punta más los 7 casos de error, todos con el mensaje esperado.

---

### 3.120 `linkc systemd`: generador de unidad systemd — RESUELTO

PLAN.md §9.7 ítem 4: `linkc docker` (`docker.rs`) ya generaba `Dockerfile`/`docker-compose.yml`/`.dockerignore` para quien despliega en contenedores -- quien despliega contra una VM/bare metal con systemd no tenía el equivalente, y armar una unidad a mano significa adivinar las opciones de hardening correctas (`NoNewPrivileges`, `ProtectSystem`, ...) sin ninguna guía.

```
linkc systemd main.link 4200 ./deploy
# unidad systemd generada exitosamente: ./deploy/main.service
```

**`linkc systemd <archivo.link> <puerto> [outdir]`** -- a diferencia de `linkc docker` (puerto siempre `3000` dentro de la plantilla), acá el puerto es un argumento REQUERIDO: `linkc serve` no tiene un puerto por default, así que la unidad tampoco puede inventarse uno. Mismo parseo y mismo mensaje de error que `linkc serve` (`port_str.parse::<u16>()`, `"puerto inválido: '{port_str}'"` si falla) -- consistencia entre los dos comandos que terminan invocando lo mismo.

**`<nombre>.service` generado** (el `file_stem` del `.link` de entrada, mismo criterio que `linkc docker` para `app_name`) con `ExecStart=/usr/local/bin/linkc serve <archivo> <puerto>`, `WorkingDirectory=/opt/<nombre>` (de ahí sale el SQLite embebido, si no se pasa `LINK_DATABASE_URL`), `Restart=on-failure` + `RestartSec=5` (reinicio del PROCESO ante un crash -- complementario, no redundante, con `--restart-backoff` de `linkc serve`/`serve-all`, §3.92, que maneja un fallo de conexión a Postgres SIN que el proceso llegue a morir), un `Environment=LINK_DATABASE_URL=...` comentado como referencia (misma variable real que `linkc docker` ya documenta, GRAMMAR.md §3.36), y hardening mínimo (`NoNewPrivileges`, `ProtectSystem=strict`, `ReadWritePaths` acotado al propio directorio de trabajo, `PrivateTmp`) -- el proceso no necesita privilegios de root ni escritura fuera de donde vive su propia base.

**Implementación**: `systemd::generate_systemd_unit(source_file, port: u16, out_dir) -> Result<PathBuf, io::Error>` (`compiler/src/systemd.rs`) -- mismo mecanismo que `docker::generate_docker_files` (mismo criterio de `file_stem`/`file_name`, mismo `format!` de plantilla), devolviendo un solo `PathBuf` en vez de un `Vec` porque acá hay un solo archivo que generar, no tres.

**Verificado**: 2 tests en `systemd.rs` (unidad bien formada con el puerto real y la variable correcta -- nunca `DATABASE_PATH`, mismo motivo que el test de `docker.rs` --, y el nombre de archivo sale del `file_stem` del `.link` de entrada, no de su ruta completa) + 2 tests de CLI end-to-end contra el binario real (`linkc systemd` genera el `.service` esperado; puerto inválido rechazado con el mismo mensaje que `linkc serve`) + `cli_help.rs` actualizado (`systemd` sumado a la lista de subcomandos que `--help` tiene que listar, el mismo test que ya existía para que esa lista nunca se desactualice en silencio). Probado a mano además contra el binario real, confirmando el `.service` generado byte a byte.

---

### 3.121 `linkc pm2-config`: generador de configuración PM2 — RESUELTO

PLAN.md §9.7, el último ítem chico de esta subsección: mismo criterio que `linkc docker`/`linkc systemd` (§3.120), pero para quien ya usa PM2 como supervisor de procesos -- Node.js/PM2 siguen siendo comunes en el mismo tipo de VM/bare metal donde `linkc systemd` también aplica, y PM2 en particular ya aparece citado en PLAN.md §9.3 como topología real de un adoptador ("varios procesos `linkc serve-all`/pm2 compartiendo un único Postgres", el motivo detrás de `db.<c>.increment`, §3.105).

```
linkc pm2-config main.link 4200 -o ecosystem.json
# configuración PM2 generada exitosamente: ecosystem.json
```

**`linkc pm2-config <archivo.link> <puerto> [-o <archivo>]`** -- a diferencia de `linkc docker`/`linkc systemd` (un directorio de salida con nombre de archivo fijo), acá el CALLER elige el nombre completo del archivo con `-o` (default `./ecosystem.json` si se omite) -- un `ecosystem.json` de PM2 suele vivir junto a otros ecosystems del mismo repo, no en un directorio propio. `-o` toma un valor (mismo criterio de parseo que `--diff` en `linkc build`, §3.79); igual que `linkc systemd`, el puerto es un argumento requerido con el mismo parseo y mensaje de error que `linkc serve`.

**Formato NATIVO de PM2** (`pm2 start ecosystem.json` lo entiende sin conversión, a diferencia de `ecosystem.config.js`): un `app` con `"script": "linkc"` + `"interpreter": "none"` (PM2 necesita ese flag para ejecutar un binario nativo directo en vez de asumir un intérprete de JS) y `"args": ["serve", "<archivo>", "<puerto>", "--restart-backoff", "30s"]` como array, no un string armado a mano -- evita cualquier ambigüedad de quoting.

**`--restart-backoff 30s` va DENTRO de `args`, no como `restart_delay` del lado de PM2** -- §3.92 documenta que ese flag nativo de `linkc serve`/`serve-all` existe justamente para reemplazar la mitigación externa de PM2 (`--restart-delay`, una espera fija) por un backoff exponencial real ante un fallo de conexión a la base. `"autorestart": true` sigue siendo responsabilidad de PM2 -- reinicio del PROCESO ante un crash, complementario y no redundante con el backoff de conexión, mismo criterio que `Restart=on-failure` + `RestartSec` en la unidad systemd de §3.120.

**Sin `LINK_DATABASE_URL` en el `env` generado** -- a diferencia de la variable comentada que `linkc docker`/`linkc systemd` sí dejan como referencia inerte (`#Environment=...`), JSON no tiene comentarios: un placeholder ahí sería un valor REAL que PM2 pasaría al proceso, apuntando en silencio a una base de datos falsa en vez de quedar como una pista visual. La variable real sigue siendo la misma (§3.36); agregarla queda en manos de quien complete el `env` a mano.

**Implementación**: `pm2::generate_pm2_config(source_file, port: u16, out_path) -> Result<PathBuf, io::Error>` (`compiler/src/pm2.rs`) -- mismo mecanismo que `docker::generate_docker_files`/`systemd::generate_systemd_unit` (`file_stem`/`file_name`, `format!` de plantilla), con `out_path` como archivo completo en vez de un directorio.

**Verificado**: 2 tests en `pm2.rs` (JSON válido -- parseado de verdad con `serde_json`, no solo "no crashea" -- con el puerto real en `args` y SIN ninguna variable de conexión falsa; nombre de app del `file_stem`) + 2 tests de CLI end-to-end contra el binario real (`-o` explícito genera el `ecosystem.json` esperado; sin `-o` el default es `./ecosystem.json` en el directorio actual) + `cli_help.rs` actualizado (`pm2-config` sumado a la lista de subcomandos verificados). Probado a mano además contra el binario real, con y sin `-o`, confirmando el JSON generado byte a byte.

---

### 3.122 `--log-format`/`--log-level`: logging estructurado JSON y nivel configurable — RESUELTO

PLAN.md §9.8, ítem 1: `linkc serve` ya dejaba una línea de log por request COMPLETADA, formato `clave=valor` (`log_done`, `runtime/server.rs`, greppable sin parsear JSON) -- lo que faltaba era (a) una forma de que un colector de logs real (CloudWatch, Datadog, `journald` con `-o json`) indexe los campos sin parsear texto libre, y (b) una forma de bajar el volumen en producción con tráfico real, donde una línea por cada request exitosa es demasiado ruido para mirar a mano.

```
linkc serve app.link 3000 --log-format json --log-level warn
```

**`--log-format text|json` / `LINK_LOG_FORMAT`** -- `text` (default, el comportamiento exacto de siempre) o `json`, una línea por evento. **`--log-level debug|info|warn|error` / `LINK_LOG_LEVEL`** -- `info` (default, IGUAL que antes de esta ronda: las dos líneas por request, recibida y completada, se siguen imprimiendo SIEMPRE) o `warn`/`error` para ver solo lo que amerita mirar: `warn` muestra únicamente requests que terminaron en 4xx o 5xx, `error` solo 5xx. `debug` es sinónimo de `info` hoy -- no hay todavía ninguna línea de nivel `Debug` propio, existe para que la jerarquía completa (`Debug < Info < Warn < Error`, orden real vía `derive(PartialOrd)`) sea un valor válido desde el principio, reservado para logging más fino a futuro.

**Clasificación automática por `status`, no una anotación por call-site**: `status_level(status)` -- 5xx es `Error` (fallo del SERVIDOR), 4xx es `Warn` (rechazo esperado -- auth, rate limit, validación -- pero señal real), cualquier otra cosa (2xx/3xx, o el sentinel `0` que usa un cliente desconectado a mitad de un `stream`) es `Info`. La línea de "request recibida" (antes de saber el status final) queda fija en `Info` -- mismo criterio que un 2xx, así que a nivel `info` (el default) sigue imprimiéndose exactamente igual que siempre, y solo se suprime pidiendo `warn`/`error` explícitamente.

**`LogConfig` (`format`+`level`, `Copy`) se arma UNA vez al arrancar** (`main.rs::resolve_log_format`/`resolve_log_level`) y cruza a los hilos de escritura de `stream` (`write_stream`/`write_live_stream`) exactamente igual que `max_body_bytes: u64` ya cruzaba -- sin ninguna sincronización, es un valor fijo para toda la vida del proceso.

**Límite documentado, no escondido**: en `LogFormat::Json`, el campo libre `extra` (`error="..."` en una falla, `sent=N total=M` en un stream) viaja tal cual DENTRO de un string en el JSON (`"extra": "error=\"...\""`), no separado en campos propios -- no hay una gramática fija que partirlo sin inventar un schema que esta ronda no amerita. Un colector de logs puede indexar `req_id`/`method`/`status`/`duration_ms` de sobra; `extra` sigue necesitando lectura humana o un parseo aparte, igual que en `LogFormat::Text`.

**Alcance**: solo las líneas POR REQUEST (`log_done` + la línea de "request recibida") -- la línea de arranque (`"c-script server escuchando en..."`) y un error de `accept()` de la conexión TCP siguen como `println!`/`eprintln!` planos, sin cambios: son eventos raros, de una sola vez, no la fuente de volumen que este ítem ataca.

**Verificado**: 6 tests de CLI end-to-end contra el binario real en `cli_log_format.rs` (formato texto default sigue imprimiendo las dos líneas de siempre; `--log-format json` produce JSON parseable de verdad con los campos documentados, no solo "no crashea"; `--log-level warn` suprime una request exitosa PERO sigue mostrando un 404; `--log-format`/`--log-level` inválidos rechazados con un mensaje claro). Probado a mano además contra el binario real (`curl` + lectura de stdout), confirmando las tres combinaciones (texto default, JSON, `warn` con éxito vs. error) byte a byte.

---

### 3.123 Hooks de React generados: guarda contra respuestas fuera de orden — RESUELTO

PLAN.md §9.13: pedido explícito del usuario de mejorar cómo los hooks de React generados (`hooks.ts`) se integran con componentes reales. Auditando `codegen::ts_emit::emit_hooks` para eso apareció algo más urgente que ergonomía: **`use{Servicio}{Rpc}Query`/`use{Servicio}{Rpc}Mutation` no tenían ninguna protección contra una respuesta VIEJA resolviendo DESPUÉS de una más nueva** -- el caso real es un buscador que llama al hook de Query por cada letra tipeada (el `useEffect` interno re-dispara `refetch` en cada cambio de parámetros): si la request de la letra ANTERIOR es más lenta que la de la letra ACTUAL, puede resolver después y pisar `data` con un resultado ya desactualizado, en SILENCIO, sin ningún error visible. El hook de `stream` ya se protegía de esto con un `cancelled` booleano en su `useEffect`; los de Query/Mutation, agregados en una ronda anterior, no.

```
export function useUsersSearchQuery(client: UsersClient, term: string) { /* ... */ }

function SearchBox() {
  const [term, setTerm] = useState("");
  const { data } = useUsersSearchQuery(client, term);
  // term="a" dispara una request; term="ab" (medio segundo después) dispara
  // otra -- sin la guarda, si la de "a" es más lenta, `data` podría
  // terminar mostrando resultados de "a" aunque el input ya diga "ab".
  return <input value={term} onChange={(e) => setTerm(e.target.value)} />;
}
```

**Guarda de "solo la respuesta más reciente gana"**: un `requestIdRef` (`useRef(0)`, contador monotónico) por instancia del hook. Cada llamada a `refetch`/`mutate` toma su propio número al arrancar (`const requestId = ++requestIdRef.current`), y las tres actualizaciones de estado (`setData`/`setError`/`setLoading`) solo corren si `requestIdRef.current === requestId` sigue siendo cierto cuando la promesa resuelve -- si otra llamada más nueva ya avanzó el contador mientras tanto, la respuesta vieja se descarta en silencio, que es exactamente lo correcto (nunca fue la que el usuario está mirando ahora).

**El `useEffect` del hook de Query además invalida en su `cleanup`** cualquier request de ESE efecto que siga en vuelo al desmontar el componente o antes de que `enabled`/`refetch` cambien (`return () => { requestIdRef.current++; }`) -- mismo criterio que el `cancelled` del hook de `stream`, adaptado a un contador porque acá conviven requests disparadas por el efecto automático Y por una llamada manual a `refetch()` desde el componente (ej. un botón "Reintentar"). **`reset()` del hook de Mutation también invalida cualquier `mutate()` en vuelo** -- sin esto, una respuesta tardía de ANTES del reset (ej. cerrar un formulario mientras el submit todavía está en curso) podría resolver después y pisar el estado recién limpiado con el resultado de la llamada vieja.

**Gap adyacente encontrado y cerrado de paso: `hooks.ts` no tenía NINGUNA cobertura de type-check automatizada.** El único frontend que corre en CI (`frontend/`, el "demo insignia" de `.github/workflows/ci.yml`) solo importa `client.ts` -- nunca `hooks.ts`, ni siquiera usa React -- y `examples/taskboard/frontend` (el único ejemplo real del repo que sí consume los hooks, contra React 18 de verdad) no está conectado a ningún workflow. Verificado a mano regenerando `examples/taskboard/frontend/src/gen/` con el binario real (`linkc build`) y corriendo `npx tsc --noEmit` contra ese proyecto -- pasó limpio. Antes de esta ronda ni siquiera podía correr: al `package.json` de ese ejemplo le faltaba la dependencia `zod` que `schemas.ts` importa (agregada de paso, gap independiente encontrado al intentar verificar). De paso, `.gitignore` tenía dos entradas puntuales de `node_modules` (`/frontend/`, `/editors/vscode/`) que no cubrían `examples/taskboard/frontend/` -- generalizado a `**/node_modules/` para que cualquier ejemplo nuevo con su propio `package.json` quede cubierto sin depender de acordarse de sumar una entrada.

**Verificado**: 2 tests nuevos en `codegen::ts_emit` (el hook de Query tiene el `requestIdRef`/las tres guardas condicionales/el `useRef` importado/el cleanup que invalida en desmontaje; el hook de Mutation tiene la misma guarda y `reset()` invalida requests en vuelo) + el test ya existente de `emit_hooks_generates_queries_mutations_and_subscriptions` sigue pasando sin cambios (la forma pública de los hooks -- nombres, firmas, tipos de retorno -- no cambió, solo su cuerpo interno). Verificado también end-to-end contra React real: `examples/taskboard/frontend` regenerado y tipando limpio con `tsc --noEmit` en modo estricto.

**Actualización (mismo día, ver §3.124): el `requestIdRef` del hook de Query descrito arriba quedó SUPERADO**, no vigente -- la ronda de cache compartido entre instancias reemplazó por completo el mecanismo interno del hook de Query (ahora usa `useSyncExternalStore` sobre una entrada de cache por rpc+parámetros, que resuelve el mismo problema de fondo -- una respuesta vieja pisando una más nueva -- por construcción, sin necesitar un contador). El de **Mutation sigue exactamente como se describe acá**, sin cambios -- las mutaciones no comparten cache (ver §3.124 para el porqué).

---

### 3.124 Hooks de React generados: cache compartido entre instancias — RESUELTO

Mismo pedido del usuario que motivó §3.123 ("mejora las conexiones del backend con componentes"), continuado explícitamente ("avanza con el cache"): hoy, dos componentes que llaman al MISMO `use{Servicio}{Rpc}Query` con los MISMOS parámetros (ej. un `<Header>` y un `<Sidebar>` mostrando el mismo conteo de notificaciones) disparaban DOS fetches independientes y mantenían DOS copias de estado sin relación entre sí -- ni comparten el resultado, ni una que refresca actualiza a la otra. Es el problema clásico que react-query/SWR resuelven con un cache global; acá se resuelve DENTRO del propio `hooks.ts` generado, sin sumar ninguna de esas librerías como dependencia nueva.

```
function Header({ client }: { client: TasksClient }) {
  const { data } = useTasksListQuery(client); // dispara UNA request (o la comparte si ya hay una)
  return <span>{data?.length ?? 0} tareas</span>;
}

function Sidebar({ client }: { client: TasksClient }) {
  const { data } = useTasksListQuery(client); // MISMA clave -> misma entrada de cache, sin fetch propio
  return <ul>{data?.map((t) => <li key={t.id}>{t.title}</li>)}</ul>;
}
```

**Un `Map<string, QueryCacheEntry<T>>` a nivel de MÓDULO** (`queryCache`, una sola instancia por archivo `hooks.ts` cargado -- el mismo módulo ES singleton entre todos los componentes de la app), clave `"{Servicio}.{rpc}(" + JSON.stringify([...params]) + ")"`. `getQueryCacheEntry(key)` devuelve SIEMPRE el mismo objeto para la misma clave (lo cachea el propio `Map`), así que dos instancias del hook con los mismos parámetros terminan apuntando al mismo `entry` -- sin necesitar contexto de React ni un provider envolvente.

**`useSyncExternalStore`** (la API que React 18 documenta exactamente para esto -- suscribirse a un store FUERA del árbol de componentes sin roturas de consistencia entre renders concurrentes) reemplaza los `useState` locales del hook de Query: `subscribe` agrega/saca un listener del `Set` de la entrada, `getSnapshot` devuelve `entry.state` tal cual. Cuando `setQueryCacheState` corre (dentro de `refetch`, al resolver o fallar el fetch), reemplaza `entry.state` por un objeto NUEVO (nunca lo muta in-place -- `useSyncExternalStore` necesita esa referencia nueva para notar el cambio) y llama a cada listener suscripto -- todas las instancias con esa clave se re-renderizan juntas, con el mismo dato.

**Dedupe real, no solo cache de lectura**: `entry.promise` es el punto de sincronización -- si YA hay un fetch en vuelo para esa clave (disparado por CUALQUIER instancia, o por el `useEffect` automático), un `refetch()` nuevo no dispara su propio `client.rpc(...)`, se queda esperando la MISMA promesa. Dos componentes montándose al mismo tiempo con los mismos parámetros generan UNA sola request HTTP, no dos. El `useEffect` automático (el que reemplaza el fetch-al-montar de siempre) solo llama a `refetch()` si la entrada está genuinamente vacía (`state.data === null && !state.loading && !entry.promise`) -- si otra instancia ya la pobló o ya la está pidiendo, no hace nada.

**`refetch()` sigue siendo una función real que cualquier instancia puede llamar a mano** (un botón "Actualizar") -- a diferencia del auto-fetch del efecto, una llamada MANUAL siempre dispara un fetch nuevo (o se une a uno ya en vuelo si hay uno), nunca se queda callada solo porque ya hay datos viejos cacheados; y como actualiza la entrada COMPARTIDA, todas las instancias con esa clave ven el resultado, no solo la que llamó.

**Alcance deliberado, documentado**: (1) el cache es por rpc+parámetros, NO por instancia de `client` -- si la misma app usa dos `client`s distintos contra el mismo rpc (multi-tenant, poco común) comparten cache igual; el caso real de todos los ejemplos del repo es un `client` único por app. (2) SIN invalidación automática después de una `Mutation` -- `useUsersCreateMutation` no sabe hoy que existe un `useUsersListQuery` que debería refrescar tras crear un usuario; cada componente sigue siendo responsable de llamar a `refetch()` a mano donde corresponda tras una mutación exitosa (mismo patrón que ya usa `examples/taskboard/frontend/src/App.tsx`). Automatizar eso necesitaría una forma de declarar qué Query invalida cada Mutation -- fuera de esta ronda. (3) `Mutation` NO comparte cache -- una mutación es una acción, no un dato para leer desde varios lugares a la vez, así que sigue con el `useState` local + guarda de `requestIdRef` de §3.123 sin cambios.

**Verificado**: 2 tests nuevos en `codegen::ts_emit` (`use{Servicio}{Rpc}Query` genera la clave de cache correcta con params reales, la infraestructura compartida (`Map`/`getQueryCacheEntry`/`setQueryCacheState`) se emite UNA sola vez sin importar cuántos rpcs de Query tenga el programa, y la forma pública del hook -- `QueryState<T>` -- no cambió; un programa SIN ningún Query -- todo mutations -- NO emite `useSyncExternalStore` ni la infraestructura de cache, evitando un import/`const`/`function` sin usar que rompería cualquier build con `noUnusedLocals` prendido). Además, la lógica CENTRAL del dedupe (`getQueryCacheEntry`/`setQueryCacheState`/el patrón de `entry.promise`) se verificó aparte en un script de Node standalone (sin React, el mismo algoritmo copiado literal del `hooks.ts` generado): dos "instancias" pidiendo la misma clave casi al mismo tiempo comparten exactamente UN fetch real, ambas reciben el mismo resultado, dos claves con parámetros distintos nunca se pisan, y actualizar una entrada notifica a sus listeners suscriptos. Verificado también end-to-end contra React real: `examples/taskboard/frontend` regenerado con el binario y tipando limpio con `tsc --noEmit` en modo estricto.

**Actualización (ver §3.127): el flag `loading` único descrito arriba (`state.loading`, verdadero durante CUALQUIER fetch incluida una recarga de fondo) queda SUPERADO** por la distinción `loading`/`isFetching` -- el campo interno de `QueryCacheState<T>` que este flag ocupaba se renombró a `isFetching`, y `loading` pasó a derivarse (`data === null && isFetching`) en vez de ser un flag propio. El resto de este ítem (cache compartido, dedupe, alcance) sigue exactamente igual, sin cambios.

---

### 3.125 Hooks de React generados: invalidación de cache tras una Mutation — RESUELTO

Mismo pedido del usuario, continuado explícitamente ("si, sigue con eso") tras el límite documentado en §3.124(2): hasta acá, `useUsersCreateMutation` no tenía forma de avisarle a `useUsersListQuery` que sus datos quedaron viejos -- cada componente era responsable de llamar a `refetch()` a mano tras una mutación exitosa, lo cual funciona pero es fácil de olvidar (y el compilador no puede avisar de un `refetch()` faltante en un componente que ni siquiera existe todavía). `@invalidates(rpc1, rpc2, ...)` cierra ese hueco de forma declarativa: se anota la Mutation, no cada Query que la consume.

```
service Tasks {
  rpc list() -> Task[] { db.tasks.all() }
  rpc stats() -> BoardStats { /* ... */ }

  @invalidates(list, stats)
  rpc create(input: NewTask) -> Task {
    db.tasks.insert(/* ... */)
  }
}
```

```tsx
function TaskList({ client }: { client: TasksClient }) {
  const { data } = useTasksListQuery(client); // sigue mostrando la lista vieja...
  return <ul>{data?.map((t) => <li key={t.id}>{t.title}</li>)}</ul>;
}

function NewTaskForm({ client }: { client: TasksClient }) {
  const { mutate } = useTasksCreateMutation(client);
  return <button onClick={() => mutate(input)}>Crear</button>;
  // ...tras un create() exitoso, sin que NewTaskForm ni TaskList se
  // conozcan entre sí, useTasksListQuery se re-renderiza con data: null
  // y su useEffect de auto-fetch dispara un refetch solo.
}
```

**Sintaxis**: nombres de rpc PELADOS separados por coma, no `Enum.Variant {}`, no strings -- `@invalidates(list, stats)`, nunca `@invalidates("list", "stats")`. `@invalidates()` vacío es un error de parseo explícito ("no aporta nada -- nombrá al menos un rpc") en vez de aceptarse como no-op silencioso.

**Cuatro reglas de validación en el checker**, cada una con su propio mensaje: (1) un nombre tiene que existir como rpc en el MISMO `service` -- no hay invalidación cruzada entre services; (2) el rpc nombrado no puede ser un `stream` -- un stream no tiene cache de Query que invalidar; (3) el rpc nombrado tiene que ser "forma de Query" según `RpcDecl::looks_like_a_query()` -- el mismo heurístico de nombre/aridad que decide en `ts_emit.rs` si un rpc genera `use...Query` o `use...Mutation` (extraído a un único método compartido en `ast.rs` para que checker y codegen nunca puedan divergir sobre qué es una Query, ver GRAMMAR.md §3.9 sobre por qué evitar lógica duplicada); (4) `@invalidates` declarado dos veces sobre el mismo rpc es un error, igual que cualquier otra anotación repetida; y `@invalidates` sobre un `stream` mismo (no un target, la anotación en sí) también se rechaza -- invalidar cache no tiene sentido para algo que no devuelve una respuesta única.

**Codegen: reset-y-notificar, no un refetch activo.** El hook de Mutation, tras un `mutate()` exitoso (nunca en la rama de error), llama a `invalidateQueryCache("{Servicio}.{rpc}")` por cada nombre en `@invalidates`, ANTES del `return`:

```ts
function invalidateQueryCache(rpcKeyPrefix: string): void {
  const prefix = rpcKeyPrefix + "(";
  queryCache.forEach((entry, key) => {
    if (!key.startsWith(prefix)) return;
    entry.state = { data: null, loading: false, error: null };
    entry.listeners.forEach((listener) => listener());
  });
}
```

Deliberadamente NO dispara un fetch nuevo -- solo resetea `entry.state` a `data: null` y notifica a los listeners suscriptos vía `useSyncExternalStore`. Eso alcanza: el `useEffect` del hook de Query (§3.124) YA re-dispara `refetch()` automáticamente en cuanto ve `state.data === null && !state.loading && !entry.promise`, así que reusar esa lógica existente es más simple y menos riesgoso que construir un segundo camino de fetch dentro del helper de invalidación.

**Coincidencia por PREFIJO de clave, deliberada.** La clave de cache es `"{Servicio}.{rpc}(" + JSON.stringify([...params]) + ")"` (§3.124) -- invalidar `search` limpia TODAS las entradas cacheadas de `search`, sin importar con qué parámetros se llamó cada una (`search("a")`, `search("ab")`, etc. caen todas bajo el mismo prefijo `"Users.search("`). Es la semántica correcta para el caso de uso real: una Mutation que cambia el conjunto de datos no sabe de antemano qué parámetros específicos de Query quedaron afectados, así que invalidar TODAS las variantes cacheadas de esa Query es más seguro que tratar de adivinar cuáles.

**Emisión condicional, mismo criterio que `useSyncExternalStore` en §3.124**: `invalidateQueryCache` (y su import/const asociados) solo se emite si algún rpc del programa tiene `@invalidates` -- un programa sin esa anotación no paga el costo de una función sin usar bajo `noUnusedLocals`.

**Demostración real**: `examples/taskboard/backend/taskboard.link` anota sus tres mutations (`create`, `update`, `remove`) con `@invalidates(list, listByColumn, stats)` -- las tres Queries reales del tablero que dependen del conjunto de tareas.

**Verificado**: 2 tests nuevos en parser (`@invalidates(a, b)` parsea la lista; `@invalidates()` vacío es error de parseo), 6 tests nuevos en checker (target válido de la misma service; target inexistente; target de OTRA service; target que no es forma de Query; anotación sobre un stream; anotación declarada dos veces), 2 tests nuevos en `codegen::ts_emit` (el helper de invalidación se emite solo en la rama de éxito de la Mutation, nunca en el `catch`; sin ningún `@invalidates` en el programa, no se emite el helper). Verificado a mano contra el binario real: el camino feliz y los 5 caminos de error de la validación, cada uno reproduciendo exactamente el mensaje diseñado. Verificado end-to-end contra React real: `examples/taskboard/frontend` regenerado con el binario (ahora con `@invalidates` de verdad en sus tres mutations) y tipando limpio con `tsc --noEmit` en modo estricto -- primera vez que un `hooks.ts` con invalidación de cache se compila contra React 18 real.

---

### 3.126 `LinkTransportError`: el status HTTP viaja tipado, no solo en el mensaje — RESUELTO

Auditando `client.ts` (mismo pedido del usuario de seguir mejorando TypeScript/React tras §3.123–§3.125) apareció un gap chico pero real: `LinkTransportError` (§3.5, la excepción que el cliente lanza para un fallo de transporte -- red caída, 5xx, timeout, cualquier `!res.ok`) solo llevaba el status HTTP interpolado DENTRO del mensaje (`` `HTTP ${res.status}` ``), sin ninguna propiedad tipada. Un componente real que necesita distinguir un 401 (redirigir a login) de un 404 (mostrar "no encontrado") de un 500 (ofrecer reintentar) no tenía forma de hacerlo sin parsear ese string a mano con una regex -- exactamente el tipo de "tipos poco ergonómicos" que este pedido del usuario venía a resolver.

```typescript
export class LinkTransportError extends Error {
  status: number;
  constructor(message: string, status: number) {
    super(message);
    this.status = status;
  }
}
```

```tsx
function TaskList({ client }: { client: TasksClient }) {
  const { error } = useTasksListQuery(client);
  if (error instanceof LinkTransportError && error.status === 401) {
    return <LoginPrompt />;
  }
  if (error instanceof LinkTransportError && error.status >= 500) {
    return <RetryPrompt />;
  }
  // ...
}
```

**Los dos puntos donde `client.ts` lanza `LinkTransportError` pasan `res.status` real**, no un valor inventado: el camino normal (`!res.ok`, cualquier rpc o stream) y el caso borde de un stream cuyo `res.body` viene nulo pese a `res.ok` (ahí el status sigue siendo el 2xx real de esa respuesta -- información correcta aunque no sea la causa del fallo, nunca un placeholder). Como `LinkTransportError` sigue extendiendo `Error`, no cambia nada del lado de los hooks (§3.123–§3.125): `QueryState.error`/`MutationState.error` siguen tipados `Error | null`, y el narrowing (`error instanceof LinkTransportError`) es responsabilidad de quien consume el hook, igual que ya lo era para distinguir `LinkTransportError` de `LinkValidationError`.

**Verificado**: 2 tests nuevos en `codegen::ts_emit` (la clase emitida tiene la propiedad `status: number` y el constructor la asigna; el `throw` real pasa `res.status` en los dos call sites -- el de `!res.ok` y el del stream sin body) + el test ya existente de "el cliente nunca lanza para un `Result` declarado" sigue pasando sin cambios (la forma pública -- qué lanza y cuándo -- no cambió, solo qué datos lleva encima).

---

### 3.127 Hooks de React generados: `loading` vs `isFetching` — RESUELTO

Tercera ronda seguida sobre el mismo pedido del usuario ("sigue" -- continuar profundizando TypeScript/React), auditando `use{Servicio}{Rpc}Query`: desde §3.124, `loading` era un único flag booleano, verdadero durante CUALQUIER fetch -- tanto el fetch inicial (sin datos todavía) como un `refetch()` de FONDO sobre una entrada que YA tenía datos cacheados. Un componente escrito de la forma más natural (`if (loading) return <Spinner/>`) ocultaba una lista que ya estaba mostrando datos válidos cada vez que alguien la refrescaba -- el clásico problema que react-query resuelve distinguiendo `isLoading` de `isFetching`.

```tsx
function TaskList({ client }: { client: TasksClient }) {
  const { data, loading, isFetching, refetch } = useTasksListQuery(client);
  if (loading) return <Spinner />; // solo la PRIMERA carga, sin datos todavía
  return (
    <>
      {isFetching && <RefreshingBadge />} {/* refetch de fondo, sin ocultar la lista */}
      <ul>{data?.map((t) => <li key={t.id}>{t.title}</li>)}</ul>
      <button onClick={() => refetch()}>Actualizar</button>
    </>
  );
}
```

**`isFetching` es el flag real, verdadero durante CUALQUIER fetch en vuelo** (inicial o de fondo) -- ocupa exactamente el lugar que `loading` tenía en `QueryCacheState<T>` (el tipo interno de la entrada de cache, §3.124), solo renombrado. **`loading` pasó a ser un valor DERIVADO, no un flag propio**: `data === null && isFetching` -- "no hay absolutamente nada que mostrar todavía". Derivarlo en vez de guardarlo aparte elimina la posibilidad de que los dos queden desincronizados (ej. `loading: true` con `data` ya poblado, un estado que antes era representable aunque nunca debía ocurrir).

**Sin cambios en la lógica de fetching**: el dedupe vía `entry.promise` (§3.124), el auto-fetch del `useEffect` (gated en `data === null && !isFetching && !entry.promise`, mismo criterio de siempre solo con el nombre nuevo), y la invalidación vía `@invalidates` (§3.125, que resetea `isFetching: false` igual que antes resetaba `loading: false`) siguen exactamente igual -- este ítem es puramente sobre qué información expone el hook, no sobre cuándo fetchea.

**Mutation queda deliberadamente afuera de esta ronda**: su `loading` sigue siendo un único flag (`useState`), sin distinción `isFetching` -- una mutación no tiene el concepto de "dato cacheado que sigue siendo válido mientras se recarga", cada `mutate()` es una acción disparada a mano, no un fetch automático que compita con datos ya mostrados. Si en el futuro aparece un caso real que lo amerite, es una ronda aparte.

**Verificado**: 1 test nuevo en `codegen::ts_emit` (`QueryState<T>` expone `loading`+`isFetching`, `QueryCacheState<T>` interno usa `isFetching`, ningún `setQueryCacheState` escribe un `loading: true`/`loading: false` -- todo el archivo generado usa `isFetching` para ese propósito) + el test existente de cache compartido actualizado a la nueva forma del `return` (`loading: state.data === null && state.isFetching, isFetching: state.isFetching`). Verificado también end-to-end contra React real: `examples/taskboard/frontend` regenerado y tipando limpio con `tsc --noEmit` en modo estricto.

---

### 3.128 Hooks de React generados: `mutate` vs `mutateAsync` — RESUELTO

Cuarta ronda seguida sobre el mismo pedido ("sigue"), esta vez encontrando el gap directamente en la propia demostración del repo: `examples/taskboard/frontend/src/App.tsx`, `handleCreate`, hacía `await createTask(input)` (el `mutate` de `useTasksCreateMutation`) SIN ningún `try`/`catch` alrededor -- el uso más natural posible del hook. Hasta esta ronda, `mutate` SIEMPRE relanzaba (`throw`) el error de la mutación, así que un fallo real (red caída, validación del servidor, lo que sea) producía una promesa rechazada sin manejar -- visible en consola como *"Uncaught (in promise)"* -- pese a que el hook YA exponía ese mismo error en su propio estado (`error`), la forma pensada para enterarse sin necesitar `try`/`catch` a mano.

```tsx
// Antes: mutate() siempre relanzaba -- este handler nunca atrapaba el error.
async function handleCreate() {
  await createTask(input); // <- fallo real = "Uncaught (in promise)" en consola
  setTitle('');
}
```

```tsx
// Ahora: mutate() nunca relanza -- devuelve null en el fallo (error ya
// quedó en el estado del hook), mismo patrón que refetch() de Query.
async function handleCreate() {
  const created = await createTask(input);
  if (!created) return; // fallo real: error ya está en el estado, sin excepción sin manejar
  setTitle('');
}

// Para quien SÍ quiere try/catch a mano (ej. lógica de reintento propia):
async function handleCreateStrict() {
  try {
    await mutateAsync(input);
  } catch (err) {
    // ...
  }
}
```

**Dos funciones, mismo nombre que react-query usa para la misma distinción** -- no es un término inventado, es la convención que cualquiera que ya conozca esa librería reconoce de inmediato. `mutateAsync` es la función original (renombrada, sin cambios de comportamiento): arma su propio `requestId` (guarda de "solo la respuesta más reciente gana", §3.123), setea `loading`/`data`/`error`, corre `@invalidates` en el camino de éxito (§3.125), y **relanza** en el camino de error. `mutate` es un wrapper nuevo y chico que llama a `mutateAsync` adentro de un `try`/`catch` propio: en éxito devuelve el valor tal cual, en fallo devuelve `null` -- exactamente el mismo patrón que `refetch()` del hook de Query (§3.124) ya usa para lo mismo, ahora consistente entre los dos hooks.

**`MutationState<T>` (la interfaz, `data`/`loading`/`error`/`reset`) no cambia** -- el cambio vive en la intersección de tipos que cada hook de Mutation devuelve, agregando `mutateAsync` al lado de `mutate` con su tipo de retorno ajustado (`Promise<T | null>` para `mutate`, `Promise<T>` para `mutateAsync`).

**Gap adyacente encontrado y cerrado de paso, real en el propio `examples/taskboard`**: al escribir el test de `mutate`/`mutateAsync` sobre un rpc con retorno YA opcional (`T?`) apareció `Promise<Task | null | null>` -- un `| null` agregado a mano sobre un `ret_str` que YA terminaba en `| null` (`getById(id: Int) -> Task?`, presente en el propio `taskboard.link`). Compilaba igual en TS (las uniones se aplanan), pero el `hooks.ts` generado quedaba con ese texto redundante en CUATRO lugares distintos: el `data` de Mutation, el `mutate`/`mutateAsync` de Mutation, el `refetch()` de Query, y el `latest` de un `stream` cuyo item es opcional. Los cuatro compartían el mismo patrón de bug (`format!("{ret_str} | null")` sin chequear si `ret_str` ya terminaba así) -- unificado en una única variable `nullable_ret_str`, calculada una vez por rpc/stream y reusada en los cuatro sitios, para que no puedan volver a desincronizarse entre sí.

**Demostración real**: `examples/taskboard/frontend/src/App.tsx`, `handleCreate`, actualizado al patrón nuevo -- chequea el `null` de `mutate()` antes de limpiar el formulario/refrescar, en vez de depender de una excepción que nunca la iba a interrumpir.

**Verificado**: 2 tests nuevos en `codegen::ts_emit` (la firma pública expone las dos funciones con los tipos de retorno correctos, `mutateAsync` sigue relanzando sin cambios, `mutate` envuelve a `mutateAsync` y devuelve `null` en el `catch`; y el fix de `nullable_ret_str` -- ningún `| null | null` en todo el archivo generado, verificado sobre una Mutation, una Query y un `stream`, los tres con retorno/item opcional) + los dos tests existentes de Mutation (`@invalidates` en el camino de éxito, guarda de `requestIdRef`/`reset`) siguen pasando sin cambios -- viven todos dentro de `mutateAsync`, que no cambió de comportamiento. Verificado también end-to-end contra React real: `examples/taskboard/frontend` regenerado y tipando limpio con `tsc --noEmit` en modo estricto, con `getById` (retorno opcional real) confirmando que `Task | null | null` desapareció del `hooks.ts` generado, y `App.tsx` usando la firma nueva de `mutate`.

---

### 3.129 `client.ts`: cancelar una request con `AbortSignal` — RESUELTO

Quinta ronda seguida sobre TypeScript/React, esta vez fuera de `hooks.ts`: hasta esta ronda, ninguna request generada por `client.ts` (ni un `rpc` normal ni un `stream`) tenía forma de CANCELARSE. Un componente que se desmonta a mitad de un fetch, o un buscador que dispara una request nueva por cada letra tipeada, no podía abandonar la anterior -- solo ignorar su respuesta cuando llegara (que es justo lo que el cache de Query, §3.124, y la guarda de `requestIdRef`, §3.123, ya hacían). El `fetch()` real seguía corriendo en el servidor de todos modos, gastando trabajo por una respuesta que nadie iba a leer.

```ts
const controller = new AbortController();
const task = await client.getById(42, { signal: controller.signal });
// ...
controller.abort(); // cancela el fetch real, no solo ignora la respuesta
```

**`options?: { signal?: AbortSignal }` como último parámetro de CADA método generado** -- `rpc` y `stream` por igual, en la interfaz (`contract.d.ts`) y en la implementación (`client.ts`), siempre opcional para que ningún caller existente se rompa. `push_fetch_call` (compartida entre el camino de `rpc` y el de `stream`, GRAMMAR.md §4.1) pasa `signal: options?.signal` al `fetch()` real -- `undefined` cuando el caller no pasó `options`, exactamente el mismo comportamiento que `fetch()` ya tiene para "sin `signal`". Un `AbortError` real (la promesa de `fetch()` rechaza cuando se aborta) llega al `catch` del caller como cualquier otro error -- no necesita manejo especial en el cliente generado, `LinkTransportError` sigue existiendo solo para `!res.ok`.

**Alcance deliberado: solo `client.ts`, `hooks.ts` no cambia en esta ronda.** Integrar cancelación DENTRO de los hooks generados (ej. que `use{Servicio}{Rpc}Query` aborte automáticamente al desmontar, o que `refetch()` acepte un signal propio) es una decisión de diseño más grande: la entrada de cache de Query es COMPARTIDA entre instancias (§3.124) -- abortar el fetch de una instancia al desmontarse no debería cancelar la request que OTRA instancia montada sigue esperando. Resolver eso bien necesita su propio diseño, no una extensión mecánica de este ítem; mientras tanto, cualquier componente puede seguir usando `client.<rpc>(...)` directo con su propio `AbortController` fuera de los hooks, que es lo que este ítem habilita.

**Verificado**: 1 test nuevo en `codegen::ts_emit` (`options?: { signal?: AbortSignal }` presente en la interfaz Y la implementación de un `rpc` sin parámetros y de un `stream`, siempre como último parámetro; `signal: options?.signal,` presente exactamente una vez por cada `fetch()` real) + todos los tests existentes que verificaban firmas exactas de métodos (`service_interface_and_rpc_signatures`, `patch_of_user_renders_as_utility_type_reference`, los de tipos genéricos `Box<T>`/`Option<T>`) actualizados a la nueva firma. Verificado también a mano contra un `linkc serve` real (`examples/taskboard`, bundle de `client.ts` vía `esbuild`, sin transpilar el resto del proyecto): abortar ANTES de que la respuesta llegue rechaza con `AbortError` real; abortar con un `setTimeout` de 1ms también; una llamada SIN `options` sigue funcionando exactamente igual que antes de esta ronda -- las tres contra el servidor real, no un mock. Alcance de lo verificado: el comportamiento del lado CLIENTE (la promesa rechaza, `signal: undefined` no rompe nada); qué hace el servidor con una conexión abortada a mitad de una query SQLite casi instantánea no es observable de forma significativa en este caso y no forma parte de esta afirmación.

---

### 3.130 Hook de `stream`: `reconnect()` manual — RESUELTO

Sexta ronda seguida sobre TypeScript/React, auditando `use{Servicio}{Rpc}` (el hook de `stream`, el único de los tres tipos de hook que hasta ahora no tenía NINGUNA forma de recuperarse de un fallo: `use...Query` tiene `refetch()`, `use...Mutation` tiene `reset()`, pero el de `stream` solo dejaba `isConnected: false` y `error` seteado -- PARA SIEMPRE. Si la conexión SSE se corta (un blip de red, el servidor haciendo `--restart-backoff` tras perder Postgres, un despliegue que reinicia el proceso), el único camino para reconectar era desmontar y remontar el componente entero -- perdiendo de paso `data`/`latest` ya acumulados, algo que ningún consumidor real de una suscripción en vivo quiere.

```tsx
function LiveIndicator({ client }: { client: TasksClient }) {
  const { isConnected, reconnect } = useTasksWatchTasks(client);
  return (
    <div>
      {isConnected ? "Conectado" : "Desconectado"}
      {!isConnected && <button onClick={() => reconnect()}>Reconectar</button>}
    </div>
  );
}
```

**Un contador (`reconnectAttempt`, `useState(0)`) que solo importa como DEPENDENCIA del `useEffect`** -- incrementarlo re-ejecuta el efecto entero, re-suscribiéndose desde cero (nueva conexión SSE real, no un truco de estado). `reconnect()` es la función que lo incrementa, envuelta en `useCallback` igual que el resto de los hooks. `data`/`latest` NO se limpian al reconectar -- "seguir la conexión viva" no es "empezar de cero"; `error` sí se limpia, porque `run()` ya lo hacía al arrancar cada corrida (`setError(null)` al principio, sin cambios).

**Manual, no automático con backoff -- mismo criterio que `refetch()`/`reset()`/`mutate()`.** Un reintento automático contra un servidor genuinamente caído (no un blip pasajero) sería un componente golpeando esa URL solo sin que nadie lo pidió; quien consume el hook decide CUÁNDO tiene sentido reconectar (ej. un botón visible solo cuando `!isConnected`, como en la demostración).

**Demostración real**: `examples/taskboard/frontend/src/App.tsx` -- el indicador "Stream en Vivo" ahora muestra un botón "Reconectar" cuando `!isConnected`, llamando a `reconnect()` del hook real.

**Verificado**: 1 test nuevo en `codegen::ts_emit` (`SubscriptionState<T>` expone `reconnect: () => void`; `reconnectAttempt` es dependencia real del efecto; `reconnect` incrementa el contador; el `return` del hook expone la función) + el test existente de generación de hooks (`emit_hooks_generates_queries_mutations_and_subscriptions`) sigue pasando sin cambios -- la forma pública del resto de los hooks no se tocó. Verificado también end-to-end contra React real: `examples/taskboard/frontend` regenerado, con `App.tsx` usando `reconnect()` de verdad, y tipando limpio con `tsc --noEmit` en modo estricto.

---

### 3.131 `isOk`/`isErr` y el schema Zod de `Result<T,E>` chequeaban un campo que no existe — RESUELTO (bug real)

Séptima ronda seguida sobre TypeScript/React, esta vez un bug de verdad, no una mejora: auditando `client.ts` para el ítem anterior apareció que `isOk`/`isErr` -- las dos funciones exportadas para narrowing de un `Result<T,E>` (`if (isOk(result)) { result.value } else { result.error }`) -- estaban tipadas y implementadas contra `{ ok: true; value: T } | { ok: false; error: E }`. **Ningún `Result<T,E>` real tiene un campo `ok`** -- el wire, `contract.d.ts` (`Result<T, E> = { type: "Ok"; value: T } | { type: "Err"; error: E }`, dos líneas arriba en el mismo archivo generado) y `validators.ts` (que sí valida correctamente contra `.type`, nunca tuvo el bug) usan `type: "Ok"|"Err"` desde siempre (GRAMMAR.md §2.2). Pasarle un `Result<T,E>` real -- literalmente el resultado de `await client.create(...)` -- a `isOk`/`isErr` ni siquiera TIPABA: `tsc` real rechaza la llamada (`Argument of type 'Result<User, E>' is not assignable to parameter of type '{ ok: true; ... } | { ok: false; ... }'`). Las dos funciones eran, tal cual estaban, inutilizables.

```ts
// Antes: ni compilaba contra un Result real.
const result = await client.create(input);
if (isOk(result)) { ... } // tsc: Argument of type 'Result<User, E>' is not assignable...
```

```ts
// Ahora: tipa y narrowea de verdad.
const result = await client.create(input);
if (isOk(result)) {
  result.value; // User, no unknown
} else if (isErr(result)) {
  result.error; // ValidationError
}
```

**El mismo bug, en la misma familia de código, en `zod_emit.rs`**: el schema Zod para `Result<T,E>` (emitido cuando algún `type`/`enum` con nombre tiene un CAMPO de tipo `Result<T,E>` -- `emit_zod_schemas` no genera un schema por rpc, solo por declaración nombrada, así que este camino no se ejercita para el uso más común de `Result` como retorno de rpc) usaba `z.discriminatedUnion("ok", [z.object({ ok: z.literal(true), ... }), z.object({ ok: z.literal(false), ... })])` -- la clave `"ok"` no existe en ningún payload real, así que ese schema rechaza CUALQUIER `Result` real, sin excepción. Arreglado al mismo `"type"`/`z.literal("Ok"|"Err")` que el resto del proyecto ya usa.

**Corrección de alcance, para no sobre-vender el hallazgo**: `validators.ts` -- el validador que `client.ts` REALMENTE usa en cada respuesta antes de devolverla, la pieza de seguridad real del contrato -- ya usaba `.type` correctamente desde siempre; nunca tuvo este bug. El impacto real está acotado a dos exports auxiliares/opcionales (`isOk`/`isErr` en `client.ts`, y el schema Zod de `Result<T,E>` cuando aparece como campo de un tipo nombrado) -- ninguno de los dos participa en la validación real de una respuesta.

**El `import type { ... } from "./contract"` de `client.ts` ahora incluye `Result` SIEMPRE**, sin importar si algún rpc de este programa en particular lo usa -- antes, `isOk`/`isErr` (emitidas incondicionalmente) podían terminar referenciando un nombre nunca importado en un programa sin ningún `Result<T,E>` en sus firmas, produciendo un `Cannot find name 'Result'` real si alguien intentaba tipar contra la nueva firma.

**Verificado**: 3 tests nuevos (2 en `codegen::ts_emit`, 1 en `codegen::zod_emit`) + a mano con `tsc` real, dos veces -- ANTES del fix (confirmando el error de tipo real que describía el gap) y DESPUÉS (confirmando que compila y narrowea correcto) -- contra un `client.create(...)` genuino de `examples/users.link`. El fix de Zod se verificó además con Zod REAL en runtime (no solo que compile): el schema arreglado acepta un payload `{ type: "Ok"/"Err", ... }` genuino y RECHAZA explícitamente la forma vieja (`{ ok: true, ... }`), confirmando que el cambio de discriminador realmente cambió el comportamiento, no solo el texto generado.

---

### 3.132 Schema Zod de un enum ADT: `z.enum([...])` no alcanzaba — RESUELTO (bug real)

Octava ronda seguida, misma familia de bug que §3.131, encontrada auditando `zod_emit.rs` de punta a punta después de arreglar `Result<T,E>`: `Item::Enum` en `emit_zod_schemas` generaba `z.enum([...])` -- una unión de strings LITERALES -- para CUALQUIER enum, sin importar si sus variantes llevaban datos. `examples/users.link` declara exactamente ese caso (`enum ValidationError { InvalidEmail { field: String }, TooShort { field: String, min: Int } }`, el error de dominio real del `create` de ese ejemplo) -- un ADT, cuyo wire real (`emit_enum_decl`, ts_emit.rs) es un objeto con tag `type` más los campos de la variante, NUNCA un string pelado. `z.enum(["InvalidEmail", "TooShort"])` acepta el string `"InvalidEmail"` y rechaza `{ type: "InvalidEmail", field: "..." }` -- exactamente al revés de lo que cualquier payload real necesita.

```ts
// Antes: aceptaba el string pelado, rechazaba el objeto real.
ValidationErrorSchema.safeParse("InvalidEmail").success;               // true (¡mal!)
ValidationErrorSchema.safeParse({ type: "InvalidEmail", field: "x" }).success; // false (¡mal!)

// Ahora: exactamente al revés, lo correcto.
ValidationErrorSchema.safeParse("InvalidEmail").success;               // false
ValidationErrorSchema.safeParse({ type: "InvalidEmail", field: "x" }).success; // true
```

**Mismo criterio `all_unit` que `emit_enum_decl` (ts_emit.rs) ya usa** para decidir entre las dos formas -- `e.variants.iter().all(|v| v.fields.is_none())`. Un enum sin datos en ninguna variante (`Status { Active, Inactive }`) sigue exactamente igual, `z.enum([...])`, sin cambios -- ese caso nunca tuvo el bug. Un ADT (alguna variante con datos) ahora genera `z.discriminatedUnion("type", [z.object({ type: z.literal("Variante"), ...campos }), ...])`, un `z.object` por variante -- una variante SIN datos mezclada dentro de un ADT (ej. `Shape { Circle { radius: Float }, Point }`) lleva solo el discriminador, sin campos extra, igual que `emit_enum_decl` ya hace para ese mismo caso. Los campos de cada variante reusan `render_zod_type_for_field` (con `.optional()`/validadores encadenados) -- el mismo camino que ya usan los campos de un `type` struct, no una copia aparte.

**Regresión real, atrapada por `docs_examples.rs` (el suite que compila cada bloque marcado de la documentación con el binario real) antes de llegar a producción**: un ADT GENÉRICO (`enum Result<T, E> { Ok { value: T }, Err { error: E } }`, el ejemplo educativo de GRAMMAR.md/docs -- distinto del `Result<T,E>` builtin del lenguaje) tiene campos de variante que referencian su propio parámetro de tipo (`T`). El primer intento de este fix resolvía cada campo con `checker.resolve_type` a secas, que rechaza `T` ("tipo desconocido: 'T'") -- rompiendo `linkc build` ENTERO para cualquier programa con un ADT genérico, algo que el código viejo (que nunca miraba campos de variante) nunca hacía. Arreglado con `checker.resolve_type_abstract(&f.ty, &e.type_params)` -- mismo criterio que `resolve_field_ty` en ts_emit.rs ya usa -- que deja `T` como `Type::TypeParam` en vez de fallar; `render_zod_type` ya tenía un catch-all (`z.unknown()`) para cualquier tipo sin forma Zod razonable, así que el resultado es `z.object({ type: z.literal("Ok"), value: z.unknown() })` -- no del todo preciso (Zod no tiene generics reales como TS, un parámetro de tipo sin instanciar no tiene ningún schema posible mejor que ese), pero NUNCA rompe el build.

**El mismo bug de resolución (no de forma), en el branch hermano `Item::Type`**: auditando el archivo completo tras arreglar el enum genérico apareció que un `type` GENÉRICO (`type Box<T> = { value: T }`) tenía exactamente el mismo problema -- `resolve_type` a secas sobre un campo `T` de un STRUCT genérico, no solo de un ADT. Confirmado a mano contra el binario real (`linkc build` sobre un programa con `Box<T>` fallaba en `schemas.ts` con el mismo "tipo desconocido: 'T'"). Mismo fix, mismo patrón (`resolve_type_abstract` cuando `t.type_params` no está vacío).

**Verificado**: 4 tests nuevos en `codegen::zod_emit` (un ADT de dos variantes con datos genera el `discriminatedUnion` esperado, un enum sin datos sigue generando `z.enum` sin cambios; una variante SIN datos mezclada en un ADT lleva solo el discriminador; un ADT GENÉRICO no rompe el build, cae a `z.unknown()` en el campo con el parámetro de tipo; un `type` GENÉRICO tampoco) + el test existente de schemas simples (`test_zod_emit_generates_valid_schemas`) sigue pasando sin cambios + `docs_examples.rs` (todos los bloques marcados de la documentación, incluido el ADT genérico que originalmente rompía esto) vuelve a pasar. Verificado también con Zod REAL en runtime, mismo criterio que §3.131: el schema arreglado acepta `{ type: "InvalidEmail", field: "email" }` (y la segunda variante, `{ type: "TooShort", field: "password", min: 8 }`) y RECHAZA explícitamente el string pelado `"InvalidEmail"` que la forma vieja aceptaba.

---

### 3.133 `openapi.json`: mismos tres bugs que `isOk`/`isErr` y el schema Zod, esta vez en la especificación pública de la API — RESUELTO (bug real)

Continuación directa del mismo audit de §3.131/§3.132: `openapi_emit.rs` (`type_to_json_schema` + `emit_openapi_json`) tenía EXACTAMENTE los mismos tres bugs, en el archivo que además es la documentación PÚBLICA de la API -- lo que consume Swagger UI, un generador de SDK en otro lenguaje, o cualquier herramienta externa que confíe en `openapi.json` como la fuente de verdad del contrato.

**(1) `Type::ResultOf` describía el wire como `{ ok: boolean, value, error }`** -- mismo campo `ok` inexistente que `isOk`/`isErr` (§3.131) tenían. Arreglado a `oneOf` + `const` (el equivalente en JSON Schema 2020-12, que OpenAPI 3.1 adopta completo, del `z.discriminatedUnion` que `zod_emit.rs` ya usa):

```json
{
  "oneOf": [
    { "type": "object", "properties": { "type": { "const": "Ok" }, "value": {...} }, "required": ["type", "value"] },
    { "type": "object", "properties": { "type": { "const": "Err" }, "error": {...} }, "required": ["type", "error"] }
  ]
}
```

**(2) Un enum ADT se describía como `{"type":"string","enum":[...]}`** -- mismo bug que el schema Zod (§3.132), esta vez en la documentación pública. Mismo criterio `all_unit` para decidir entre las dos formas; el caso ADT ahora genera `oneOf` con un `{"type":"object", "properties": {"type": {"const": "Variante"}, ...campos}}` por variante, reusando `type_to_json_schema` para los campos -- el mismo camino que ya usan los campos de un `type` struct. Un enum sin datos en ninguna variante sigue exactamente igual, sin cambios.

**(3) Mismo bug de generics que §3.132, en el branch `Item::Type` de este archivo**: un `type`/`enum` GENÉRICO con un campo que referencia su propio parámetro de tipo rompía `linkc build` ENTERO -- confirmado a mano contra el binario real, DESPUÉS de arreglar el mismo bug en `schemas.ts`: `Box<T>` seguía rompiendo, esta vez en `openapi.json` específicamente (`type_to_json_schema` ya tenía un catch-all -- `{"type":"object"}` -- para cualquier tipo sin JSON Schema razonable, así que el problema era la RESOLUCIÓN del tipo, nunca su renderizado). Mismo fix, mismo patrón: `resolve_type_abstract` en vez de `resolve_type` a secas cuando `type_params` no está vacío -- tanto en `Item::Type` como en el `Item::Enum` ADT nuevo del punto (2).

**Verificado**: 4 tests nuevos en `codegen::openapi_emit` (el `Result<T,E>` de un rpc real usa `oneOf`/`const`, nunca el `{ok, ...}` viejo; un ADT usa `oneOf`/`const` por variante; un enum sin datos sigue igual; un `type`/`enum` genérico -- `Box<T>` y un ADT genérico juntos -- no rompe el build) + los 11 tests existentes de este archivo (deprecated, `@example`, defaults) siguen pasando sin cambios -- ninguno tocaba `Result<T,E>` ni un ADT. Verificado también a mano contra el binario real: `linkc build examples/users.link` regenerado, `openapi.json` inspeccionado byte a byte -- el `Result<Task, ValidationError>` de `create` y el `ValidationError` de `components/schemas` usan la forma nueva (`oneOf`/`const`) en el archivo real, no solo en un test aislado.

---

### 3.134 `@infinite(cursor, limit)`: scroll infinito real — RESUELTO

Vuelta a mejoras de TypeScript/React (no bugs) tras cerrar el audit de §3.131-§3.133: de los tres tipos de hook generado, `use{Servicio}{Rpc}Query` tiene `refetch()`, pero ninguno sabía manejar PAGINACIÓN -- un componente con scroll infinito tenía que gestionar el cursor a mano, llamando a `client.<rpc>(cursor, limit)` directo y concatenando páginas él mismo. `db.<c>.pageAfter(cursor: Int?, limit: Int)` (§3.61) ya es el único mecanismo de paginación por cursor del lenguaje -- este ítem le da un hook dedicado.

```
service Tasks {
  @infinite(cursor, limit)
  rpc listPaged(cursor: Int?, limit: Int) -> Task[] {
    db.tasks.pageAfter(cursor, limit)
  }
}
```

```tsx
function PagedHistory() {
  const { data, loading, isFetchingNextPage, hasNextPage, fetchNextPage } = useTasksListPagedInfinite(client, 5);
  if (loading) return <p>Cargando...</p>;
  return (
    <>
      <ul>{data.map((t) => <li key={t.id}>{t.title}</li>)}</ul>
      {hasNextPage && <button onClick={() => fetchNextPage()} disabled={isFetchingNextPage}>Cargar más</button>}
    </>
  );
}
```

**`@infinite(cursor, limit)` nombra los DOS parámetros de ESTE rpc** que juegan el rol de cursor y tamaño de página -- identificadores sueltos, como `@invalidates`, no `Enum.Variante`. El checker exige las MISMAS firmas que `pageAfter` ya tiene (`cursor: Int?`, `limit: Int`) y que el retorno sea `T[]` con `T` teniendo un campo `id: Int` -- no un mecanismo genérico para "cualquier forma de paginación imaginable", sino el hook dedicado para el ÚNICO patrón de cursor que el lenguaje ya soporta. Reemplaza el hook de Query normal para ese rpc (nunca coexisten -- un fetch de una sola página sin nunca avanzar el cursor no es útil); el hook de Mutation se sigue emitiendo igual, sin cambios.

**Cómo se calcula "hay página siguiente"**: sin un campo de conteo total en la respuesta (el rpc devuelve `T[]` liso, no un wrapper `{items, total}`), el heurístico es "si la última página trajo MENOS items que `limit`, no hay más" -- mismo criterio que usan otros sistemas de paginación por cursor sin conteo (ej. Relay). **Cómo se calcula el cursor siguiente**: el `id` del ÚLTIMO elemento de la página -- mismo criterio que `pageAfter` usa puertas adentro (un cursor de continuación basado en `id`, estable ante inserciones concurrentes, a diferencia de `page(limit, offset)`).

**`cursor` desaparece de la firma pública del hook** -- lo maneja internamente, arrancando siempre en `null`; `limit` sigue siendo un parámetro real que el caller elige (tamaño de página). `data` viene YA APLANADA (`pages.flat()`, todas las páginas juntas) -- casi ningún componente real quiere iterar página por página. Mismas guardas que el resto de los hooks: `requestIdRef` contra una respuesta fuera de orden, `startedRef` para no re-disparar la primera página si `enabled` alterna false→true→false→true (perdería las páginas ya cargadas).

**Alcance v0 deliberado, documentado**: sin cache compartido entre instancias (a diferencia de Query, §3.124) -- dos componentes con el mismo `useXInfinite` mantienen historiales independientes; el caso real de scroll infinito es casi siempre un único componente dueño de la lista. `refetch()` reinicia desde la página 1, descartando las páginas ya cargadas.

**Demostración real**: `examples/taskboard/backend/taskboard.link` agrega `listPaged(cursor, limit)` sobre `db.tasks.pageAfter`; `examples/taskboard/frontend/src/App.tsx` consume `useTasksListPagedInfinite` en una sección nueva ("Historial paginado") con un botón "Cargar más" real.

**Verificado**: 2 tests de parser (`@infinite(cursor, limit)` parsea los dos nombres; menos de dos es error de parseo), 8 de checker (firma `pageAfter`-shaped acepta; cursor no-`Int?` rechazado; limit no-`Int` rechazado; retorno sin `id: Int` rechazado; nombre de parámetro inexistente rechazado; mismo parámetro como cursor y limit rechazado; rechazado sobre un `stream`; declarado dos veces rechazado), 1 de `codegen::ts_emit` (la firma pública excluye `cursor`, incluye `limit`; NO coexiste con un hook de Query para el mismo rpc; el hook de Mutation se sigue emitiendo). Verificado también a mano contra un `linkc serve` real (`examples/taskboard`, 7 tareas creadas, `limit=3`): el mismo algoritmo que el hook generado implementa (sin React, bundle de `client.ts` vía esbuild) trajo exactamente 3 páginas (3+3+1=7), sin duplicados entre páginas, en orden ascendente, `hasNextPage` apagándose en el momento correcto. Verificado end-to-end contra React real: `examples/taskboard/frontend` regenerado, con `App.tsx` usando el hook de verdad, y tipando limpio con `tsc --noEmit` en modo estricto.

**Actualización (ver §3.138): el "sin cache compartido entre instancias" de arriba queda SUPERADO** -- `useXInfinite` ahora comparte cache entre instancias con el mismo criterio que Query, incluido `AbortController` reference-counted. El resto de este ítem (la anotación, el heurístico de `hasNextPage`, el cursor por `id`) sigue exactamente igual.

---

### 3.135 Cache de Query: aislado por instancia de `client`, no solo por rpc+parámetros — RESUELTO

Pedido explícito del usuario: "recopilá todo lo que haríamos en otras versiones relacionadas con TypeScript y la compatibilidad, y terminá todo eso en una sola versión" -- cuatro límites de alcance documentados a lo largo de la sesión (§3.124, §3.129, §3.134), todos cerrados juntos en esta ronda. Primero, el más simple: desde §3.124, el cache de Query (`Map<string, QueryCacheEntry<T>>`) era un único mapa a nivel de módulo, compartido incluso entre DOS INSTANCIAS DE `client` distintas contra el mismo rpc con los mismos parámetros -- una app multi-tenant o con múltiples sesiones (`clientA`/`clientB`, cada uno con su propio token/base URL) veía datos de una filtrarse en la otra.

```ts
const clientA = createUsersClient("https://tenant-a.example.com");
const clientB = createUsersClient("https://tenant-b.example.com");
// Antes: la MISMA clave de cache ("Users.list()") se compartía entre los
// dos -- useUsersListQuery(clientA) y useUsersListQuery(clientB) podían
// pisarse el resultado entre sí.
```

**`queryCache` pasa de `Map<string, QueryCacheEntry<T>>` a `WeakMap<object, Map<string, QueryCacheEntry<T>>>`** -- una capa extra keyeada por la instancia de `client` real. `getQueryCacheEntry(client, key)` busca primero el sub-`Map` de ESE client (creándolo si hace falta) y recién ahí la entrada por `key` -- dos clients JAMÁS comparten una entrada, pero múltiples componentes usando el MISMO client siguen compartiendo exactamente igual que antes (nada cambia para el caso real de todos los ejemplos del repo, un `client` único por app). `WeakMap`, no `Map` -- un `client` que ya nadie referencia puede recolectarse solo, sin que este cache lo retenga para siempre. `invalidateQueryCache(client, rpcKeyPrefix)` también gana el parámetro `client`, con el mismo criterio de aislamiento.

**Verificado**: 1 test nuevo en `codegen::ts_emit` (`getQueryCacheEntry` busca dentro del sub-`Map` de `client`, nunca en un `Map` plano compartido) + a mano con Node real (sin React): dos instancias de `client` reales (`createTasksClient` dos veces contra el mismo `linkc serve`) con la MISMA clave de cache NUNCA comparten entrada; el MISMO client con la misma clave SÍ.

---

### 3.136 `AbortSignal` real dentro de los hooks (Query reference-counted, Mutation explícito) — RESUELTO

Desde v1.92.0, `client.ts` soporta cancelar cualquier request vía `options?: { signal?: AbortSignal }`, pero NINGÚN hook lo exponía (§3.129, límite documentado explícitamente: "hooks.ts no cambia en esta ronda... es una decisión de diseño más grande"). El problema de fondo: la entrada de cache de Query es COMPARTIDA entre instancias (§3.124/§3.135) -- abortar el fetch de una instancia al desmontarse NO debe cancelar la request que OTRA instancia montada sigue esperando. Cancelar sin ese cuidado sería peor que no cancelar nada.

**Query: `AbortController` reference-counted, vía `entry.listeners`.** Cada entrada de cache ahora tiene un `controller: AbortController | null` -- se crea junto con el `fetch()` real (pasado como `client.rpc(..., { signal: controller.signal })`) y se cancela SOLO cuando el conteo de listeners suscriptos llega a cero:

```ts
const subscribe = useCallback((onStoreChange) => {
  entry.listeners.add(onStoreChange);
  return () => {
    entry.listeners.delete(onStoreChange);
    if (entry.listeners.size === 0) entry.controller?.abort();
  };
}, [entry]);
```

Mientras quede AL MENOS UN componente montado mirando esa clave, el fetch sigue -- solo cuando el ÚLTIMO se desmonta (o nunca hubo ninguno más) se cancela la request real, evitando trabajo de red/servidor desperdiciado por una respuesta que ya nadie va a leer. Un `AbortError` disparado así **no es un error real** -- el `catch` lo detecta (`err instanceof DOMException && err.name === "AbortError"`) y resetea `isFetching` sin tocar `error`, para que un mount posterior de la misma clave no arranque viendo un error que nunca pidió.

**Mutation e Infinite: `AbortController`/`AbortSignal` sin reference counting.** A diferencia de Query, el estado de `mutate`/`mutateAsync` y de `useXInfinite` (ya client-scoped y compartido, ver §3.138) es de UNA sola "línea de trabajo" a la vez -- cancelar ahí siempre es seguro sin contar listeners. Mutation gana `options?.signal` (ver §3.137, mismo parámetro que `optimisticData`) reenviado tal cual al `fetch()` real; Infinite crea su propio `AbortController` interno por `loadPage()`, cancelado automáticamente por el mismo mecanismo reference-counted que Query (comparte cache entre instancias desde §3.138, así que también necesita el mismo cuidado).

**Regresión real encontrada por `tsc` -- no un test, el compilador mismo**: el primer intento del `catch` de Query, ante un abort, hacía `return;` (sin relanzar) -- TypeScript infería `entry.promise` como `Promise<T | void>`, incompatible con la firma declarada `Promise<T> | null` de `QueryCacheEntry<T>`. Arreglado relanzando (`throw err;`) en los DOS caminos del `catch` (abort y error real) -- el `try/catch` que envuelve `await entry.promise` en `refetch()` ya devuelve `null` ante CUALQUIER rechazo, así que el comportamiento visible no cambia, solo el tipo que TS infiere.

**Verificado**: 2 tests nuevos en `codegen::ts_emit` (el `AbortController` se crea y se pasa al `fetch()` real; el `catch` distingue abort de error real, relanzando en ambos casos) + a mano contra un `linkc serve` real: dos "listeners" suscriptos a la misma entrada -- desmontar el primero NO aborta (el segundo sigue mirando), desmontar el ÚLTIMO SÍ aborta, y la promesa compartida rechaza con un `AbortError` genuino. Verificado también end-to-end contra React real: `examples/taskboard/frontend` regenerado y tipando limpio con `tsc --noEmit` en modo estricto (el error de tipo real de arriba se atrapó exactamente así, antes de llegar a producción).

---

### 3.137 Mutaciones optimistas: `optimisticData` con rollback automático — RESUELTO

Cuarto ítem del mismo pedido bundle. `mutate`/`mutateAsync` ganan un último parámetro opcional, `options?: { optimisticData?: T }` (mismo objeto `options` que ahora también lleva `signal`, §3.136) -- el valor se muestra en `data` INMEDIATAMENTE, antes de que la request salga siquiera, reemplazado por el valor REAL en éxito (el `setData(res)` de siempre) o revertido a `null` si la mutación falla.

```tsx
const { mutate: createTask, data, loading } = useTasksCreateMutation(client);
// ...
const created = await createTask(input, {
  optimisticData: { id: -1, createdAt: new Date(0).toISOString(), ...input },
});
// `data` muestra el valor optimista YA, sin esperar la red; si la
// mutación falla, `data` vuelve a `null` solo -- este componente no
// necesita ningún try/catch/rollback propio.
```

**Alcance deliberado: el optimismo es sobre el `data` PROPIO de la Mutation, no sobre el cache de una Query relacionada.** Una alternativa más ambiciosa -- que `create` actualice optimistamente el `data` de `useTasksListQuery` (mostrar la tarea en la lista antes de que el servidor confirme) -- se descartó para esta ronda: los targets de `@invalidates` pueden tener FORMAS heterogéneas (`list` devuelve `Task[]`, `stats` devuelve `BoardStats`, un tipo completamente distinto), así que un único updater tipado de forma segura contra targets de formas distintas necesitaría generar un mapeo de tipos por target -- una pieza de diseño bastante más grande que esta ronda no amerita. El optimismo sobre el `data` propio de la Mutation, en cambio, es siempre el MISMO tipo `T` en los dos lados (mostrado y confirmado), sin ese problema.

**Rollback ligado al mismo `requestIdRef` que ya existía (§3.123)** -- si el optimista se mostró y la mutación falla, `setData(null)` corre solo si esta sigue siendo la request MÁS RECIENTE (nunca pisa el resultado de una llamada más nueva que ya haya resuelto).

**Demostración real**: `examples/taskboard/frontend/src/App.tsx`, `handleCreate` -- pasa `optimisticData` a `createTask`, y un indicador nuevo ("✓ '{creatingTask.title}' (confirmando con el servidor...)") se muestra usando el `data` optimista mientras `creating` está en `true`.

**Verificado**: 1 test nuevo en `codegen::ts_emit` (el optimista se muestra ANTES del `try`, el rollback corre en el `catch` gateado por `requestIdRef`) + a mano contra un `linkc serve` real, tres casos: el optimista se muestra antes de la red: `true`; una mutación exitosa reemplaza el optimista por el dato REAL del servidor (id real, no el `-1` optimista): `true`; una mutación contra un puerto muerto (fallo de red real) hace rollback a `null`: `true`. Verificado también end-to-end contra React real: `examples/taskboard/frontend` regenerado con la demostración real, tipando limpio con `tsc --noEmit` en modo estricto.

---

### 3.138 Cache de Infinite compartido entre instancias — RESUELTO

Cierra el último límite del pedido bundle, el "Alcance v0 deliberado" que §3.134 dejó documentado explícitamente: `use{Servicio}{Rpc}Infinite` pasa del mismo `useState` local que Query tenía ANTES de v1.87.0 a exactamente la misma arquitectura de cache compartido que Query tiene desde entonces (§3.124/§3.135) -- `useSyncExternalStore` sobre una entrada de un `WeakMap<client, Map<string, InfiniteCacheEntry<T>>>`, dedupe real vía `entry.promise`, y el mismo `AbortController` reference-counted de §3.136.

**Clave del cache: rpc + parámetros SIN `cursor`** (a diferencia de Query, que incluye TODOS los parámetros) -- dos instancias pidiendo "la misma lista paginada" comparten el MISMO historial de páginas aunque una ya haya avanzado más que la otra; el cursor es progreso interno de la entrada compartida, no parte de su identidad. `limit` sigue siendo parte de la clave (dos `limit` distintos son, con razón, dos listas paginadas distintas).

**`entry.started` (compartido) reemplaza el `startedRef` (`useRef`, por instancia) que la v0 tenía** -- la primera página se pide UNA sola vez sin importar cuántos componentes monten el mismo `useXInfinite` a la vez, en vez de que cada instancia dispare su propio fetch inicial.

**Verificado**: 1 test nuevo en `codegen::ts_emit` (`infiniteQueryCache` es un `WeakMap`, `getInfiniteCacheEntry` existe una sola vez, el dedupe vía `entry.promise` está presente, el abort reference-counted es idéntico al de Query) + el test existente de scroll infinito (§3.134) actualizado a la nueva forma compartida (`cacheKey`/`entry` en vez de `useState` local) + a mano contra un `linkc serve` real: dos "instancias" llamando `loadPage(null, true)` casi al mismo tiempo generan UN solo fetch real, no dos -- mismo patrón de verificación que el dedupe de Query (§3.124) usó originalmente. Verificado también end-to-end contra React real: `examples/taskboard/frontend` regenerado y tipando limpio con `tsc --noEmit` en modo estricto.

### 3.139 `llms-full.txt`: la mitad expandida de la convención llmstxt.org — RESUELTO

Auditoría de PLAN.md §9.9 (SEO y descubribilidad para IA) pedida explícitamente por el usuario con terminología más amplia -- "SEO, meta datos, AEO, GEO, AIO, LLMO y todo lo relacionado" -- para confirmar qué queda genuinamente abierto antes de volver al backlog general. Resultado de la auditoría: los nueve ítems originales de §9.9 siguen resueltos (`sitemapXml`/`robotsTxt` §3.116, metadata clásica y JSON-LD §3.117, `llms.txt` §3.118, `@example` en `openapi.json` §3.119, `@cache_control` §3.113, `response.redirect` §3.111) -- AEO (contestar preguntas directamente), GEO y AIO (que un motor generativo pueda citar la API con precisión) y LLMO (que un LLM entienda el contrato sin adivinar) son, en sustancia, la MISMA dimensión 2 de §9.9 ("descubribilidad para agentes de IA") con nombres de marketing más nuevos, no requisitos técnicos distintos. La única brecha real encontrada: `emit_llms_txt` (§3.118) implementa solo la mitad "índice" de la convención [llmstxt.org](https://llmstxt.org/) -- el propio spec define un `llms-full.txt` hermano, con el contenido COMPLETO en vez de un resumen de una línea por entrada, para que un agente no tenga que seguir un "link" (acá, invocar el rpc) para conseguir el detalle.

**`emit_llms_txt_full(program, title) -> Result<String, String>`** (nueva función en `codegen::llms_txt_emit`, junto a `emit_llms_txt`) recorre los mismos `service`/rpc pero con un `### firma` (headings, no bullets) por entrada y sin el recorte de "solo la primera línea" que `emit_llms_txt` aplica a propósito -- el docstring `///` completo, línea por línea. Si el rpc declaró `@example(request: ..., response: ...)` (§3.119), sus dos mitades se agregan como bloques ` ```json ` -- reusa `literal_expr_to_json` (antes privada de `openapi_emit.rs`, ahora `pub(crate)`) en vez de duplicar la conversión, mismo criterio de "reusar en vez de reimplementar" que el resto de esta ronda de TypeScript/compatibilidad ya siguió con `resolve_type_abstract`.

**`linkc build` ahora escribe `llms-full.txt` junto a `llms.txt`** (mismo directorio, mismo paso, sin flag nuevo) -- un adopter que ya consume `llms.txt` no ve ningún cambio; `llms-full.txt` es un archivo adicional, no un reemplazo. Un rpc sin docstring ni `@example` sigue apareciendo (solo con su firma y ruta, sin cuerpo) -- mismo criterio de "nunca esconder una capacidad real de la API" que `emit_llms_txt` ya seguía.

**Verificado**: 5 tests nuevos en `codegen::llms_txt_emit` (un `### firma` por rpc con el docstring ENTERO, sin `@example` no hay ningún bloque ` ```json `, `@example` con `request`+`response` se propaga como dos bloques JSON separados byte a byte, un rpc sin docstring sigue apareciendo) + los 4 tests existentes de `emit_llms_txt` sin cambios. Probado a mano contra el binario real: `linkc build examples/users.link <tmp>` genera `llms-full.txt` junto a los demás archivos; `examples/taskboard/frontend/src/gen/llms-full.txt` regenerado igual.

### 3.140 `@idempotent`: idempotency keys nativas en rpcs de escritura — RESUELTO

PLAN.md §9.3 ítem 6: antes de esta ronda, protegerse de una escritura duplicada por un reintento (un backfill que reintenta, un cliente que reenvía tras un timeout) exigía implementar a mano el chequeo "¿ya procesé esto?" antes de cada inserción -- reforzado por Glowapp (no usa c-script, pero varios de sus webhook handlers de pago -- `v2RevolutHandler.ts`, `orderConfirmation.ts`, `connections.ts` -- hacen exactamente ese chequeo a mano antes de escribir, evidencia de demanda real y repetida).

**`@idempotent`, sin argumentos** (igual que `@authenticated`) sobre un `rpc` -- el checker (`check_idempotent_annotation`) solo rechaza declararlo sobre un `stream` (una conexión SSE no tiene un único resultado que grabar y repetir, mismo motivo que `@cache_control`/`@example` ahí). Combina libremente con cualquier otra anotación.

**Opt-in por REQUEST, no forzado por el rpc** -- mismo diseño que Stripe usa para su propio header `Idempotency-Key` (mismo nombre, no un invento propio): un rpc marcado `@idempotent` se comporta EXACTAMENTE igual que siempre para un caller que no manda el header. Si lo manda, `runtime::server::handle_request` consulta un `idempotency::IdempotencyStore` (una entrada por proceso servidor, mismo modelo de concurrencia in-memory que `rate_limit::RateLimiter` -- desde GRAMMAR.md §3.158, v1.114.0, un hilo real por request, ambos viven detrás de su propio `Arc<parking_lot::Mutex<...>>` en `server.rs`, ya no "sin `Mutex`" como antes de esa ronda) ANTES de correr el cuerpo del rpc:

- **Primera vez que se ve esa clave** para ese `(service, rpc)`: corre normal. Si el resultado es 2xx, se graba `(status, body, content-type)` junto con un SHA-256 del body de la request (mismo algoritmo y mismo hex que `crypto.hashSha256`, vía `idempotency::hash_request_body`). Un error NO se graba -- el caller puede corregir y reintentar con la misma clave, mismo criterio que Stripe.
- **Misma clave, mismo hash de body**: la respuesta grabada se repite tal cual, SIN correr el cuerpo -- el reintento de un backfill nunca duplica la fila.
- **Misma clave, hash de body DISTINTO**: `409 Conflict` -- reusar una clave para una operación distinta es casi siempre un bug del lado cliente (una clave generada una sola vez y reusada donde no correspondía), y silenciarlo devolviendo el resultado viejo sería peor que rechazarlo.

TTL de 24hs por entrada (mismo orden de magnitud que Stripe documenta para su propia feature), con el mismo patrón de sweep periódico que `RateLimiter` -- no persiste entre reinicios del proceso, un límite aceptado a propósito (un reintento después de un restart simplemente corre de nuevo, ni mejor ni peor que sin esta feature). Alcance v0 deliberado: un hit repite status+body+content-type, pero NO `Location`/`Cache-Control` de la respuesta original -- combinarlo con `response.redirect`/`@cache_control` en la misma ronda no tenía casos de uso reales que lo justificaran.

**Verificado**: 2 tests de parser (sin argumentos, `@idempotent()` con paréntesis es un error de sintaxis) + 3 de checker (tipa combinado con otras anotaciones, rechazado sobre un `stream`) + 5 de `idempotency::IdempotencyStore` (miss en la primera vez, hit repite el resultado grabado, hash distinto es conflicto, misma clave en dos `(service, rpc)` distintos son namespaces independientes, el hash es determinístico y sensible al contenido) + 3 tests de integración en `server_http.rs` **contra un `linkc serve` REAL** (`Orders.create` inserta una fila de verdad en SQLite): un reintento con la misma clave devuelve el MISMO resultado y el contador de filas confirma que la segunda request nunca insertó nada; sin header, dos POST insertan dos filas, sin ninguna deduplicación; la misma clave con un body distinto da 409 y tampoco inserta nada.

### 3.141 `smtp.sendMessage`: cc/bcc y adjuntos reales — RESUELTO

PLAN.md §9.6 ítem 1: `smtp.send`/`sendToMany`/`sendHtml` (§3.43/§3.63) cubren el caso común (texto o HTML a una lista de destinatarios), pero ninguno soporta copia oculta/visible ni adjuntos. Prioridad alta por evidencia real: primer adoptador (CRM/Nexus) que ABANDONÓ el módulo `smtp` por completo en vez de trabajar alrededor de sus límites -- `mailer.link` (`Mailer.send`) ni siquiera llama a `smtp.*`, el envío real ocurría en 314 líneas de TypeScript con `nodemailer` (adjuntos, tracking, envío asíncrono).

**`smtp.sendMessage(message: { to: String[], cc?: String[], bcc?: String[], subject: String, body: String, html?: Bool, attachments?: { filename: String, contentType: String, contentBase64: String }[] }) -> Void`** -- variante "kitchen sink" APARTE de las tres simples (nunca las reemplaza ni les agrega parámetros): agregar cc/bcc/attachments a `send`/`sendToMany`/`sendHtml` habría hecho que el 99% de las llamadas existentes (sin ninguno de los tres) pagaran el costo de un parámetro nuevo para nada. Estructural sin nombre, mismo criterio que `sitemap_url_type`/`robots_rule_type` -- cualquier `type` del programa con estos campos sirve. `cc`/`bcc`/`html`/`attachments` son opcionales-POR-CLAVE (`x?: T`, no `x: T?`) -- el caso más común (sin ninguno de los tres) no obliga a escribir `[]`/`null` a mano.

**`contentBase64`, no bytes crudos** -- c-script no tiene un tipo de bytes, así que un adjunto binario real (PDF, imagen) viaja codificado en base64, igual que cualquier binario dentro de JSON. Se decodifica del lado del runtime DIRECTO a `Vec<u8>` con el mismo engine/alfabeto que `base64.decode` (§3.43) usa -- pero sin pasar por ESE builtin, que exige que el resultado decodificado sea UTF-8 válido (piensa en un `String` de c-script) -- algo que un adjunto binario real casi nunca es.

**cc/bcc van al SOBRE SMTP real** (`RCPT TO:` por cada uno, igual que `to`) -- reciben el mensaje de verdad. `cc` además aparece en el header `Cc:` del mensaje (así los destinatarios de `to` ven que hubo copia); `bcc` NUNCA aparece en ningún header -- es responsabilidad de `lettre` (la librería subyacente) construir el mensaje sin esa fuga, y es justamente lo que hace "blind" a un blind carbon copy.

**Adjuntos como partes MIME reales** (`multipart/mixed`, vía `lettre::message::{Attachment, MultiPart, SinglePart}`), no una concatenación de texto -- cada adjunto lleva su propio `Content-Type`/`Content-Disposition: attachment; filename="..."`. `Content-Transfer-Encoding` lo elige `lettre` según el contenido decodificado (7bit para texto plano ASCII, base64 para binario) -- transparente para quien llama, el `contentBase64` de entrada es siempre la forma de TRANSPORTE del valor hacia c-script, no necesariamente la forma final del mensaje.

**Verificado**: 5 tests de checker (forma mínima sin cc/bcc/html/attachments, forma completa con un adjunto, rechaza sin `to`, rechaza `cc: String[]?` -- valor opcional -- donde se espera `cc?: String[]` -- clave opcional, `T?` nunca es subtipo de `T` -- GRAMMAR.md §3.4, rechaza la cantidad de argumentos equivocada) + 3 tests de integración en `cli_smtp.rs` contra un servidor SMTP de mentira REAL hablando el protocolo (EHLO/MAIL FROM/RCPT TO/DATA) sobre un `TcpStream`: cc/bcc llegan como `RCPT TO:` del sobre, `Cc:` aparece en el header y `Bcc:` nunca aparece; un adjunto real llega como parte `multipart/mixed` con su nombre de archivo y el contenido DECODIFICADO (confirmando el viaje base64→bytes de punta a punta, no solo que el string llegó); un `contentBase64` inválido falla limpio con 500, nunca con un panic.

### 3.142 `@rate_limit(..., key: <param>)`: una clave adicional a la IP — RESUELTO

PLAN.md §9.4 ítem 6, gap encontrado analizando Glowapp (no usa c-script, pero su propio middleware `v2RateLimit.ts` es la evidencia): `@rate_limit("N/ventana")` (§3.39) limita SOLO por `(ip del cliente, service, rpc)` -- un endpoint de pago con ese único criterio deja que alguien evada el límite rotando de IP mientras reusa el mismo email, exactamente el abuso que ese middleware de Glowapp documenta haber sufrido antes de agregar `prefix:ip:email` a mano como clave.

**`@rate_limit("N/ventana", key: <param>)`** -- segundo argumento OPCIONAL, `key: <nombre>` nombrando un parámetro real de ESTE rpc. El checker (`check_rate_limit_annotation`) exige que el parámetro exista y sea `String` o `Int` -- los únicos dos tipos que se pueden combinar con la IP en una clave de texto sin ambigüedad. Sin `key:`, comportamiento IDÉNTICO a siempre (solo-IP) -- `@rate_limit("N/ventana")` sin segundo argumento no cambia en nada.

**La clave del bucket pasa de la IP sola a `"{ip}|{param}={valor}"`** -- `rate_limit::RateLimiter` en sí NO CAMBIÓ NADA: sigue recibiendo un solo string de identidad como siempre, `server.rs` es quien arma ese string combinado antes de pasarlo. El valor sale de `args_json` (el body ya parseado de la request, disponible ANTES del gate de rate limit, mismo orden que siempre) -- si el campo llegara ausente o con un tipo inesperado (no debería pasar para un programa que compiló, pero el código nunca asume que no puede pasar), cae a un string vacío en vez de entrar en pánico.

**Verificado**: 3 tests de parser (sin `key:` parsea igual que siempre, con `key: email` parsea el nombre, una palabra clave que no sea `key` es un error de sintaxis) + 3 tests de integración en `cli_rate_limit.rs` contra un `linkc serve` real: dos requests con el MISMO `email` desde la MISMA conexión (misma IP) comparten balde y el tercero da 429; con un email DISTINTO desde la MISMA IP, balde propio, sin verse afectado por el balde agotado del otro email -- la prueba directa de que la clave combina IP+valor, no reemplaza la IP; `key: <nombre-que-no-existe>` y `key: <param-Bool>` rechazados en compilación con su mensaje propio.

### 3.143 `--hsts`: `Strict-Transport-Security` opt-in — RESUELTO

PLAN.md §9.4 ítem 5: `linkc serve` ya manda tres headers de seguridad fijos en toda respuesta (`X-Content-Type-Options`/`X-Frame-Options`/`Referrer-Policy`, §3.41), pero CSP y HSTS quedaban afuera a propósito -- CSP depende del contenido de cada página (sigue afuera), HSTS porque `linkc serve` nunca termina TLS por sí solo, así que mandarlo SIEMPRE sería mentir sobre una garantía que el proceso no puede dar. En producción real, sin embargo, es muy común que un proxy/balanceador de confianza (nginx, Caddy, un load balancer de nube) SÍ termine TLS delante del proceso -- el mismo patrón que `--trust-proxy` (§3.89) ya resuelve para `X-Forwarded-For`.

**`--hsts <valor>`/`LINK_HSTS`** -- texto LITERAL, sin parsear ninguna gramática interna (`max-age=N`, `includeSubDomains`, `preload` son responsabilidad de HTTP, no de c-script), mismo criterio que `@cache_control("...")` (§3.113). Sin el flag/env var: `None`, sin este header -- comportamiento IDÉNTICO al de siempre (el mismo default conservador que ya regía). Con él, el valor se manda como `Strict-Transport-Security` en TODA respuesta -- éxito, error, y también un `stream` SSE.

**Threading sin tocar los 16 call-sites de `cors_response`/`cors_response_with_type`**: en vez de agregar un parámetro más a esas funciones, `hsts` se copia una sola vez por request al campo nuevo `CorsHeaders::hsts` (la misma bolsa de headers-ya-resueltos que `cors_response_with_type`/`sse_preamble` -- los dos únicos lugares que arman una respuesta -- ya recibían). `sse_preamble` (usado por `write_stream`/`write_live_stream`) y `cors_response_with_type` son los dos únicos puntos que efectivamente escriben el header, para que no diverjan (mismo criterio documentado ahí desde antes para CORS/seguridad).

**Verificado**: 4 tests de integración en `cli_hsts.rs` contra un `linkc serve` real: sin `--hsts`, ningún header (comportamiento de siempre); con `--hsts "max-age=63072000; includeSubDomains"`, el valor literal viaja tal cual en `/health` Y en la respuesta de un rpc normal (`POST /Sys/ping`); el mismo valor también viaja en la respuesta de un `stream` (confirma que `sse_preamble` no divergió de `cors_response_with_type`); `LINK_HSTS` como variable de entorno.

### 3.144 `@cache("60s")`: cache de resultado del lado del servidor — RESUELTO

PLAN.md §9.3 ítem 5: para lecturas costosas y poco cambiantes, `@cache("Ns"/"Nm"/"Nh"/"Nd")` sobre un `rpc` (rechazado sobre un `stream`, mismo motivo que `@idempotent`/`@cache_control`) cachea del lado del SERVIDOR el resultado de la primera ejecución exitosa, keyeado por `(service, rpc, JSON de argumentos)` -- un reintento dentro del TTL repite la respuesta grabada SIN correr el cuerpo de nuevo. Dimensión ORTOGONAL a `@cache_control` (§3.113, que solo le dice al CLIENTE cuánto puede cachear) y a `@idempotent` (§3.140, opt-in por header del cliente para escrituras) -- `@cache` es automático y transparente, sin ningún header, pensado para lecturas.

`cache::CacheStore` (nuevo módulo) mismo modelo in-memory de un solo proceso que `RateLimiter`/`IdempotencyStore`, con `cache::parse_ttl` (formato `Ns`/`Nm`/`Nh`/`Nd`, una tercera implementación chica de este formato en el binario -- mismo criterio ya aceptado entre `rate_limit::RateLimitSpec::parse` y `main.rs::parse_duration`, que tampoco comparten código). Solo se graba un ÉXITO (2xx); un error no queda cacheado. **Alcance v0 deliberado**: sin invalidación cruzada con `@invalidates` -- esa es la cache del CLIENTE (TypeScript), una entrada de `@cache` expira sola por tiempo, nunca antes; combinar los dos en el mismo rpc es válido pero no se integran entre sí.

**Verificado**: 2 tests de parser + 4 de checker (tipa, TTL malformado, declarado dos veces, rechazado sobre un `stream`) + 5 de `cache::CacheStore` (miss, hit dentro del TTL, entrada vencida es miss, argumentos distintos son entradas independientes, parseo del formato) + 2 tests de integración en `server_http.rs` contra un `linkc serve` real: `Stats.summary` inserta una fila real cada vez que CORRE -- un segundo POST dentro del TTL confirma, por CONTEO de filas (no solo por el resultado), que el cuerpo nunca se ejecutó de nuevo; un rpc sin `@cache` sigue corriendo siempre.

**Límite honesto, evaluado y NO atacado a propósito (AUDIT-2026-08-27.md #11): `get`/`put` tienen la misma forma de carrera TOCTOU que `@idempotent` tenía antes de §3.167** -- dos requests concurrentes con la misma clave que llegan antes de que la primera termine ven las dos un miss y corren el cuerpo las dos ("estampida de caché"). A diferencia de `@idempotent` (donde ejecutar dos veces es una escritura DUPLICADA, un bug de corrección real), acá `@cache` está documentado para lecturas SIN efectos secundarios -- correr el cuerpo dos veces produce el mismo resultado dos veces, trabajo desperdiciado pero nunca una respuesta incorrecta. Se evaluó aplicar el mismo mecanismo `reserve`/`InFlight` que §3.167 construyó para `@idempotent`, y se descartó a propósito: la semántica correcta para una cache no es "rechazar al segundo caller con 409" (eso rompería el contrato de `@cache`, que nunca antes devolvía error por una carrera) sino "hacerlo esperar al primero" -- y esperar de forma sincrónica acopla la latencia de N requests NO relacionados al tiempo que tarde el primero en terminar, un cambio de comportamiento con trade-offs propios que no se justifica sin evidencia real de que la estampida importe en la práctica. Documentado acá en vez de arreglado con apuro.

### 3.145 `deleteWhere` empuja la SELECCIÓN a SQL — RESUELTO

PLAN.md §9.3 ítem 1, última parte pendiente (`countWhere`/`findWhere` ya empujaban a SQL desde §3.95/§3.108/§3.109 -- `deleteWhere` seguía trayendo la colección ENTERA a memoria, aunque el predicado fuera pusheable). Con un predicado con la forma pusheable (`ast::recognize_conjunction_predicate`), `deleteWhere` ahora usa `find_where_conjunction` (la MISMA función que `findWhere`/`countWhere`, un `SELECT ... WHERE` real que respeta `@softDelete` automáticamente) para encontrar las filas a borrar, en vez de `db.<c>.all()` + filtrar en el intérprete.

**El BORRADO en sí sigue siendo fila por fila**, a propósito -- no un `DELETE ... WHERE` de una sola sentencia: cada `delete()` publica la fila borrada a cualquier `stream` suscripto a esa colección (§3.16), y una sentencia bulk no tiene forma de dar ese aviso por fila sin perder esa semántica. La optimización real es la SELECCIÓN (evitar traer/interpretar filas que no matchean), no el borrado -- cuando la selección ya viene filtrada por SQL, el predicado no se vuelve a evaluar en el intérprete (confiar en el mismo `WHERE` que `findWhere`/`countWhere` ya confían). Un predicado no pusheable (`||`, comparar dos campos entre sí, etc.) sigue cayendo al camino interpretado de siempre, sin cambios.

**Verificado**: 2 tests nuevos (`deleteWhere` con predicado pusheable borra solo lo que matchea, las demás filas sobreviven; un predicado no pusheable -- comparar dos campos del propio parámetro entre sí -- cae al camino interpretado con el mismo resultado) + el test existente de soft-delete (`count_where_and_find_where_respect_soft_delete_even_when_pushed_down`, que YA ejercitaba `deleteWhere` con un predicado pusheable) sigue pasando sin cambios, confirmando que una fila soft-deleteada sigue sin volver a "aparecer" para el borrado pusheado.

### 3.146 `@check(minLength/maxLength, N)`: constraints de longitud sobre `String` — RESUELTO

PLAN.md §9.3 ítem 3, la mitad de `String` que quedaba pendiente (`@check(min/max/range, ...)` sobre `Int`/`Int64`/`Float`, v1.60.0/§3.96, ya resuelto -- comparar dos campos entre sí sigue abierto, sin evidencia real de demanda). Mismo `FieldCheck` (una tercera y cuarta variante, `MinLength(f64)`/`MaxLength(f64)`, mismo criterio "kind + argumento" que `@validate`) y mismos DOS puntos de enforcement que la mitad numérica: aplicación (`check_string_length`, mismos dos puntos de entrada que `@check` numérico -- wire y `StructLit` construido en el cuerpo de un rpc) y base de datos (`CHECK (length(...) >= N)` inline de verdad, en los DOS backends -- `check_clause_sql` es la MISMA función compartida entre SQLite y Postgres que ya generaba el `CHECK` numérico).

`@check(minLength, 1)` es la forma de expresar "no vacío" -- no hay una variante `NotEmpty` aparte, sería redundante. **Cuenta CARACTERES Unicode, no bytes** (`chars().count()` del lado de la aplicación; `length(...)` del lado de la base -- SQLite y Postgres cuentan caracteres para una columna de texto, no el tamaño de la codificación UTF-8, en los dos motores) -- una longitud pensada para lo que un humano ve, no para el tamaño en disco.

**Verificado**: 3 tests de checker (tipa sobre `String`/`String?`, rechaza sobre un campo no-`String`, rechaza una longitud negativa o fraccionaria) + 2 de `runtime/mod.rs` (`minLength` rechaza un string vacío, `maxLength` cuenta caracteres Unicode no bytes -- "café" son 4 caracteres pero 5 bytes UTF-8, confirmado que el límite se aplica sobre 4) + 1 test contra un Postgres REAL (`pg_integration.rs`, misma forma que el test de `@check(range, ...)` ya existente): un `INSERT` SQL crudo con título vacío, sin pasar por c-script en absoluto, rechazado por el `CHECK` real de la base.

### 3.147 `@cors("...")`: override de CORS por ruta — RESUELTO

PLAN.md §9.4 ítem 4: `--cors-origin`/`LINK_CORS_ORIGINS` (§3.41) es GLOBAL para todo el servidor -- el caso real que faltaba es una API entera detrás de un allowlist salvo UN endpoint puntual (un widget embebible, un sitemap público) que necesita otro origen, o `*`. `@cors("https://a.com, https://b.com")` (mismo formato separado-por-comas que `LINK_CORS_ORIGINS`) o `@cors("*")` sobre un `rpc`/`stream` REEMPLAZA entero al CORS global para ESE endpoint puntual -- nunca lo combina.

**Aplica tanto al preflight `OPTIONS` como a la respuesta real** -- si solo aplicara a la respuesta, el navegador de un origen que el override permite pero el CORS global no NUNCA llegaría a mandar la request real (el preflight la bloquearía antes). Esto exigió resolver (service, rpc) del PATH ANTES del chequeo de `OPTIONS` -- una llamada extra y liviana a `resolve_route` con un body VACÍO es segura para esto: la rama de un `@route` nunca toca `body`, y la rama `/Service/rpc` de siempre solo lo usa para extraer ARGUMENTOS, nunca para decidir cuál rpc es (lo único que hace falta acá). La resolución REAL (con el body de verdad) sigue pasando después, sin cambios.

**Sin tocar los 16 call-sites de `cors_response`/`cors_response_with_type`** -- mismo criterio que `--hsts` (§3.143): el `CorsConfig` efectivo (el override si hay, si no el global) se decide UNA vez arriba y se usa para computar `cors_headers` como siempre, en vez de agregar un parámetro más a cada función que arma una respuesta.

**Verificado**: 4 tests de checker (tipa combinado con otras anotaciones, permitido sobre un `stream` -- a diferencia de `@cache_control`/`@idempotent`, un stream SSE también manda CORS real --, rechaza un valor vacío, rechaza declararse dos veces) + 3 tests de integración en `cli_cors.rs` contra un `linkc serve` real: un rpc con `@cors("*")` ignora un allowlist global restrictivo y queda abierto a cualquier origen, mientras un rpc sin override sigue respetando el allowlist; un rpc con `@cors("https://partner.example.com")` ignora un CORS global abierto (`*`) y se comporta como su propio allowlist de un origen; y -- el caso crítico -- el override también aparece en la respuesta al preflight `OPTIONS`, no solo en la respuesta real.

### 3.148 Log de auditoría de autorización estructurado — RESUELTO

PLAN.md §9.5 ítem 2: `--log-format json`/`--log-level` (§3.122) ya loguean `method`/`status`/`duration_ms` por request, pero nada sobre la DECISIÓN de autorización en sí -- auditar "quién tuvo acceso a este rpc protegido, con qué rol, y si se le permitió" exigía cruzar el status code con otra fuente (una sesión expirada, otro log, memoria). `check_auth_gate` (la ÚNICA decisión de autorización de todo el servidor) ahora devuelve, además de si la request pasa o no, un `AuthAudit` opcional: `role` (el nombre de la variante resuelta, `null` si no había sesión válida), `user_id` (de `SessionStore::user_id_for`, si la sesión se creó con `createSessionWithId`) y `allowed` (`true`/`false`). **`Some` solo cuando el rpc de verdad declaró `@authenticated`/`@requires`** -- un rpc público no genera ningún campo de auditoría, no hay ninguna decisión que registrar ahí.

**Tres campos nuevos en la línea de log de siempre** (`log_done_with_audit`, reemplaza a `log_done` en los call-sites que corren DESPUÉS del gate de auth -- incluidos los caminos de réplica de `@idempotent`/`@cache`, que pasaron por el MISMO gate): `auth_role`/`auth_user_id`/`auth_allowed`. En modo JSON van como claves de PRIMER NIVEL (no enterradas en `extra` como el resto de las anotaciones de esta línea) -- son el dato que este ítem pide poder indexar/filtrar de verdad ("mostrame todo lo que el rol X tuvo denegado"), no una nota informativa más. En modo texto, mismo estilo `clave=valor` que el resto de la línea. **Alcance**: la decisión de autorización queda logueada en el momento en que se toma (al abrir la conexión, incluido un `stream`) -- el CIERRE de una conexión de `stream` (los logs de `write_stream`/`write_live_stream`, ej. `client_disconnected`) no vuelve a repetir estos tres campos, no son un evento de autorización nuevo.

**Verificado**: 5 tests de integración en `cli_auth_audit_log.rs` contra un `linkc serve` real, leyendo su stdout de verdad: una request denegada (403) loguea el rol real y `auth_allowed=false`; una permitida (200) loguea rol+`user_id`+`auth_allowed=true`; una sin token (401) loguea rol `null`; un rpc público no lleva ninguno de los tres campos; y el mismo contenido en modo texto, como pares `clave=valor`.

### 3.149 `GET /metrics` en formato Prometheus — RESUELTO

PLAN.md §9.8 ítems 1 y 2, cerrados JUNTOS -- resultaron ser la misma pieza de infraestructura. Antes de esta ronda no existía ningún `/metrics`: latencia por rpc, conexiones activas y tamaño de la base había que instrumentarlas por fuera. `metrics::MetricsStore` (nuevo módulo, mismo modelo in-memory de un solo proceso que `RateLimiter`/`CacheStore`/`IdempotencyStore`) expone tres familias de métrica en el formato de exposición real de Prometheus:

- **`linkc_http_requests_total{method="Servicio.rpc"}`** (counter) y **`linkc_http_request_duration_seconds_sum{method="..."}`** (counter) -- conteo + suma de duración por rpc, la forma MÍNIMA que Prometheus necesita para calcular tasa y latencia PROMEDIO vía `rate(..._sum[5m]) / rate(..._count[5m])`, sin declarar buckets de histograma (una decisión que le corresponde a quien opera cada instancia según su propio SLA, no al lenguaje). **Alcance v0 deliberado**: solo el camino de dispatch NORMAL de un `rpc` suma acá -- un hit de `@idempotent`/`@cache` (ambos devuelven ANTES de llegar al punto donde se registra) y un `stream` (corre en su propio hilo spawneado, nunca toca `MetricsStore`, que vive únicamente en el hilo principal) no se cuentan.
- **`linkc_stream_subscribers{collection="..."}`** (gauge) -- cierra el ítem 2 de la sección de una sola vez: reusa `Db::subscriber_counts()`, que YA existía como estructura interna para el push real de `stream` (§3.16) -- sin ningún contador nuevo cruzando hilos (el conteo se lee SINCRÓNICAMENTE del hilo principal cuando llega `GET /metrics`, la misma restricción de "`subscribers` nunca se toca desde el hilo escritor" que ya regía). Mismo límite ya documentado de poda lazy: un suscriptor desconectado se saca RECIÉN en la próxima publicación a esa colección, así que este conteo puede sobre-reportar temporalmente.
- **`linkc_db_size_bytes`** (gauge) -- `Db::size_bytes()`, un backend por motor: SQLite no tiene una función SQL directa para "tamaño del archivo", pero `PRAGMA page_count * PRAGMA page_size` (dos `SELECT` reales) es exacto -- es literalmente cómo SQLite calcula el tamaño del archivo por dentro; Postgres sí tiene una función dedicada, `pg_database_size(current_database())`.

**`/metrics` NO está exento de `--service-api-key`**, a diferencia de `/health` -- los volúmenes/latencias por rpc son más sensibles que un simple "¿está vivo?", así que si el operador configuró esa capa, Prometheus también tiene que mandarla (`scrape_configs.authorization` en `prometheus.yml` la soporta nativamente).

**`linkc_rate_limit_rejections_total{method="..."}` (26/08/2026): rechazos `429` reales de `@rate_limit` (§3.39), por rpc.** Landmine del mismo barrido de "límites honestos" que motivó §3.150 más abajo -- no arregla la dilución del límite entre réplicas (necesitaría estado compartido entre procesos, fuera de alcance), pero hace el rechazo real observable en el mismo lugar que un operador ya mira, agregable entre réplicas con una consulta Prometheus normal (`sum by (method) (...)`).

**Verificado**: 4 tests de `metrics::MetricsStore` (un método nunca registrado no aparece, conteo+duración se acumulan por método, `stream_subscribers`/`db_size_bytes` solo aparecen cuando se pasan, rechazos de rate limit se acumulan por rpc y no aparecen hasta el primero) + 5 tests de integración en `cli_metrics.rs` contra un `linkc serve` real: el conteo y la suma de duración de `Sys.ping` después de dos llamadas reales; el tamaño de una base SQLite real con al menos una fila (> 0, no un valor inventado); DOS conexiones de `stream` REALES abiertas a la vez confirman `linkc_stream_subscribers{collection="tasks"} 2`, cero conexiones no muestra la línea; `/metrics` rechazado sin `X-Service-Api-Key` cuando `/health` sigue exento; un rpc con `@rate_limit("1/1h")` golpeado tres veces confirma exactamente 2 rechazos reales (no un valor inventado) en `linkc_rate_limit_rejections_total{method="Sys.limited"}`. Más 1 test contra un Postgres REAL (`pg_integration.rs`) confirmando `pg_database_size` con datos reales insertados.

### 3.150 Latencia de propagación NOTIFY + cola de reintento acotada — RESUELTO

PLAN.md §9.8 ítem 3, último de la sección -- con esto queda completamente resuelta. LISTEN/NOTIFY cross-instancia (§3.44) ya avisaba por `stderr` cuando un cambio de más de 8000 bytes no se propagaba, o cuando un `NOTIFY` fallaba -- pero sin ninguna forma indexable/queryable de saberlo, y sin ningún reintento: una falla TRANSITORIA (conexión caída un momento) perdía ese evento cross-instancia PARA SIEMPRE, aunque la fila ya estuviera bien escrita en la base.

**Latencia real, no inventada**: `try_notify_remote` agrega `sent_at_ms` (epoch ms) al payload del `NOTIFY` -- la instancia RECEPTORA, al drenar el canal de cambios remotos (mismo loop de siempre, `runtime/server.rs`), resta ese valor de "ahora" y lo registra en `MetricsStore::record_notify_latency`. Expuesto en `/metrics` como `linkc_notify_latency_seconds_sum`/`_count` (§3.149) -- **solo aparece si se registró al menos un evento**, para no mostrar "0 0" en una instancia SQLite (que nunca usa NOTIFY) o que arrancó sola, indistinguible de "propagación perfecta". Un payload de una instancia VIEJA sin este campo (antes de esta ronda) sigue propagándose igual -- solo pierde la métrica para ESE evento puntual, nunca el evento en sí.

**Cola de reintento ACOTADA (`MAX_PENDING_NOTIFY_RETRIES = 50`)**, solo para la falla TRANSITORIA -- nunca para el caso de payload de más de 8000 bytes, que jamás se arregla reintentando (ese sigue descartándose con su aviso de siempre, sin encolar nada). `Db::flush_pending_notify_retries` reintenta la cola en cada vuelta del MISMO loop que ya drena `remote_rx` (tick de 200ms, `REMOTE_CHANGE_POLL_INTERVAL`) -- sin ningún hilo ni timer nuevo: la propagación remota YA corría en ese loop. Al llenarse, se descarta el más VIEJO (FIFO) -- pensada para cubrir una caída corta (segundos, hasta que `with_reconnect`, §3.40, repare la conexión sola), no como almacenamiento durable.

**`linkc_notify_oversized_dropped_total{collection="..."}` (26/08/2026): el payload-descartado-para-siempre ya no depende de que alguien lea stderr.** Landmine encontrado en un barrido de "límites honestos" -- antes de esto, la ÚNICA señal de un cambio que nunca se propagó por superar `MAX_NOTIFY_PAYLOAD_BYTES` era el `eprintln!` de `try_notify_remote`, invisible corriendo desatendido bajo `pm2`/`systemd` sin revisar logs (mismo problema ya documentado para el aviso de colisión de tabla, §3.94) -- una colección con filas grandes (un catálogo de facets/búsqueda, por ejemplo) podía quedar desincronizada entre instancias durante meses sin que nadie lo notara, descubierto recién por datos divergentes, nunca por un error. `Db` ahora cuenta estos drops por colección (`oversized_notify_drops`, incrementado en el mismo punto que ya emitía el `eprintln!`, sin cambiar esa parte) y los expone como counter en `/metrics` -- **solo aparece la línea de una colección si tuvo al menos un drop**, mismo criterio que `linkc_notify_latency_seconds_*`. El drop pasa en la instancia que ESCRIBE (no en la que recibe) -- `GET /metrics` de la instancia que hizo el `insert`/`applyPatch` es donde aparece, no la que hubiera recibido el cambio si hubiera cabido.

**Verificado**: 2 tests de `parse_remote_notification` (decodifica `sent_at_ms` del payload real, tolera un payload viejo sin el campo) + 1 test contra DOS instancias `linkc serve` REALES sobre el MISMO Postgres (`pg_integration.rs`, misma base que `a_write_on_one_instance_pushes_to_a_stream_connected_to_another`): una escritura real en B, propagada a A vía LISTEN/NOTIFY real, y `GET /metrics` de A confirma `linkc_notify_latency_seconds_count 1` con una suma real. La cola de reintento se verificó por revisión de código (`try_notify_remote` compartida entre el envío original y el flush, sin dos copias que puedan divergir) -- forzar una caída de conexión real DE FORMA determinística en un test de integración quedó fuera de esta ronda por el costo de orquestarla contra Postgres real. Más 2 tests de `metrics::MetricsStore` (aparece solo cuando se provee, por colección) y 1 test contra un Postgres REAL (`pg_integration.rs`, 26/08/2026): un `insert` con un campo de 8200 caracteres (el payload entero supera 8000 bytes de sobra) confirma `linkc_notify_oversized_dropped_total{collection="..."} 1` en `/metrics` de la MISMA instancia que escribió, con el insert local sin verse afectado; un segundo insert normal no vuelve a sumar.

### 3.151 `db.vacuum()`/`db.tableStats()`: RPCs de administración — RESUELTO

PLAN.md §9.7 ítem 3, "RPCs de administración estándar opcionales (`_admin.vacuum()`, `_admin.tableStats()`) detrás de `@requires(Role.Admin)`". Alcance ajustado al espíritu del lenguaje (primitivas, no magia): en vez de un servicio `_admin` auto-inyectado por el compilador, dos builtins nuevos SOBRE `db` directo (`db.vacuum() -> Void`, `db.tableStats() -> Map<String, Int>`) que quien escribe el `.link` expone en su PROPIO service, con la gramática de autorización que YA existe (`@requires(Role.Admin)`, sin ninguna anotación nueva) -- exactamente como el ejemplo del propio ítem lo sugiere.

**`db.vacuum()`**: un `VACUUM` real, mismo comando en los dos backends. **`db.tableStats()`**: cuenta FILAS FÍSICAS de cada colección declarada (`SELECT COUNT(*)` sin filtrar `@softDelete`, a propósito distinto de `count()`) -- diagnóstico de tamaño real de tabla, donde una fila soft-deleteada sigue ocupando espacio. Devuelve `Map<String, Int>` (representado en runtime como `Value::Struct`, la MISMA forma que ya usa cualquier `Map<String,V>` -- sin necesidad de un tipo estructural nuevo tipo `sitemap_url_type`).

**Bug real encontrado en la verificación manual, antes de shippear**: la primera implementación interceptaba `db.vacuum`/`db.tableStats` en la evaluación GENÉRICA de `Expr::FieldAccess` sobre `Value::Db` -- rompía `db.vacuum.insert(...)` para cualquier programa con una colección de VERDAD llamada "vacuum" (un caso de borde real, no hipotético: el propio `.link` de este repo tiene colecciones con nombres cortos). Arreglado moviendo la intercepción al mismo lugar que ya usa el atajo de `isSome`/`isNone` -- ANTES de evaluar `callee` como `Expr::Call { callee: FieldAccess { base: Ident("db"), field }, .. }` completo, así que solo dispara cuando `db.vacuum`/`db.tableStats` es DIRECTAMENTE lo que se está llamando (`db.vacuum()`), nunca cuando es la base de un field access MÁS LARGO (`db.vacuum.insert(...)`) -- la MISMA distinción que el checker ya hacía correctamente en `try_builtin_method` desde el principio (el bug era solo del lado del runtime).

**Verificado**: 3 tests de checker (tipa, rechaza argumentos, una colección real llamada "vacuum" sigue tipando `db.vacuum.all()` sin problema) + 2 de `runtime/mod.rs` contra SQLite real (`db.vacuum()`/`db.tableStats()` corren de verdad y reflejan inserts reales; el test de REGRESIÓN del bug de arriba, confirmando que `db.vacuum.insert(...)`/`db.vacuum.all()` siguen funcionando con una colección de verdad llamada "vacuum") + 1 test contra un Postgres REAL (`pg_integration.rs`) confirmando que `VACUUM` no choca contra "no puede correr dentro de un bloque de transacción" (el riesgo real de este ítem en ese backend).

### 3.152 Bloqueo de cuenta configurable — RESUELTO

PLAN.md §9.5 ítem 1: "bloqueo de cuenta tras N intentos fallidos". c-script no tiene un `login` nativo (el login v0 de `examples/users.link` es código de usuario normal, `db.users.all().filter(...)` + `auth.createSession(role)`) -- así que este ítem no podía ser un mecanismo automático atado a ninguna anotación. Tres primitivas chicas sobre `auth`, mismo criterio que el resto del lenguaje (composición, no magia): **`auth.recordFailedLogin(identifier: String) -> Void`**, **`auth.failedLoginCount(identifier: String, windowSeconds: Int) -> Int`**, **`auth.resetFailedLogins(identifier: String) -> Void`**. `identifier` es responsabilidad de quien llama (email, user id como texto, IP -- lo que tenga sentido para SU login); umbral y ventana también los elige el propio `.link`, sin ningún flag de servidor nuevo.

**Estado en memoria sobre `SessionStore`** (el mismo store que ya guarda sesiones, sin un módulo nuevo ni un parámetro más enhebrado por `server.rs`) -- timestamps de intentos fallidos por `identifier`, más viejo primero. `failed_login_count` poda del frente los que ya vencieron la ventana ANTES de contar (una ventana se evalúa en el momento de la consulta, nunca al grabar -- distintas llamadas pueden pedir ventanas distintas para el mismo identifier sin pisarse). `resetFailedLogins` borra el historial entero -- pensado para llamarse tras un login EXITOSO, así un usuario legítimo que se equivocó un par de veces no acumula contra sí mismo para siempre.

**Verificado**: 2 tests de checker (tipa, rechaza tipos equivocados) + 4 de `SessionStore` (cero para un identifier nunca visto, se acumula dentro de la ventana, se excluye lo que quedó afuera de la ventana, `reset` limpia el conteo) + 3 tests de integración en `cli_auth_lockout.rs` contra un `linkc serve` real, con un `login` de verdad (`Result<String, LoginError>`, mismo patrón de manejo de errores del lenguaje -- nunca lanza para un error declarado): un login válido no se ve afectado por los fallos de OTRO identifier; después del umbral que el propio `.link` eligió, el intento siguiente da una variante de error DISTINTA (`LockedOut` en vez de `InvalidCredentials`), confirmando que corrió el camino de bloqueo; un login exitoso resetea el conteo del MISMO identifier, sin tocar el de uno distinto.

**Límite honesto (AUDIT-2026-08-27.md #15): la composición check-then-act no es atómica, a propósito.** Cada primitiva es atómica POR SEPARADO (`recordFailedLogin`/`failedLoginCount`/`resetFailedLogins`, cada una bajo un único candado del `SessionStore`), pero un `login` típico las compone en al menos dos pasos separados (`if failedLoginCount(id, ventana) >= umbral { rechazar } else { verificar contraseña; if mal { recordFailedLogin(id) } }`) -- una ráfaga de intentos concurrentes con el MISMO `identifier` puede pasar el chequeo de umbral antes de que cualquiera de ellos llegue a `recordFailedLogin`, permitiendo más de `umbral` intentos antes de que el bloqueo surta efecto. No es un bug de las tres primitivas (que hacen exactamente lo que prometen), es una consecuencia inherente de exponerlas como piezas para componer en vez de un único builtin "check-and-record" atómico -- la misma filosofía de "composición, no magia" de arriba. Sin evidencia real de que esto importe en un adoptador (un bloqueo por fuerza bruta sigue siendo efectivo salvo en el margen de la ráfaga concurrente exacta contra el umbral), así que no se ataca con un mecanismo nuevo esta ronda -- documentado acá para que quien lo use lo sepa de antemano.

### 3.153 `linkc serve-all --port-registry <archivo.json>`: puerto estable por nombre de servicio — RESUELTO

Reporte de adopción real (IgnisLove), diagnosticado por otra sesión de Claude trabajando sobre el VPS del adoptador: el incidente de colisión de puerto ya conocido (`serve-all` reordena por orden alfabético, §3.92 "Límites honestos") tuvo su mecanismo EXACTO confirmado en producción -- con 17 servicios `.link`, el puerto `8792` cayó en `bot_defense`, el mismo puerto que otra app (`myfinance`) tenía hardcodeado para su propio backend. `--port-map-out` (§3.107) ya hacía LEGIBLE la asignación, pero segía siendo de solo escritura -- cada arranque la recalculaba entera por orden alfabético, así que agregar/quitar/renombrar UN `.link` en la carpeta seguía corriendo el puerto de todos los demás.

```bash
linkc serve-all ./services --port-base 3000 --port-registry ./services/ports.json
```

**Con `--port-registry`, el archivo se LEE primero (si ya existe) antes de asignar nada.** Misma forma que `--port-map-out` (`{"nombre_archivo": puerto, ...}`, clave = nombre sin `.link`). Cada nombre YA PRESENTE en el archivo conserva su puerto de siempre, sin importar la posición alfabética actual entre los `.link` descubiertos. Un nombre nuevo (un `.link` agregado desde la corrida anterior) recibe el próximo puerto libre a partir de `--port-base`, saltando cualquiera ya ocupado por otro nombre. El archivo actualizado (con el nombre nuevo ya insertado) se re-escribe antes de arrancar cualquier servicio -- mismo criterio de "todo o nada" que `--port-map-out`: si la escritura falla, `linkc serve-all` sale con error y no arranca nada.

**Un servicio borrado o renombrado deja su entrada -- y su puerto -- INTACTOS en el registro, a propósito.** No se libera automáticamente para que un servicio NUEVO lo herede: un gateway externo puede seguir teniendo ESE puerto hardcodeado apuntando a lo que ya no existe (exactamente el escenario de `cscript-gateway.ts` que motivó §3.107), y reasignarlo en silencio a un servicio distinto sería reproducir el mismo incidente de colisión al revés -- ahora entre un nombre viejo y uno nuevo, en vez de entre dos alfabéticamente adyacentes. Limpiar una entrada obsoleta del archivo es una decisión manual del operador (editarlo a mano), nunca algo que `serve-all` haga solo.

**JSON inválido en el archivo falla limpio, antes de arrancar cualquier hilo** -- mismo criterio que un `.link` con error de tipos (§3.92): mejor no levantar nada que arriesgar una asignación de puerto silenciosamente distinta a la que el operador esperaba. Combina libremente con `--port-map-out` (pueden apuntar al mismo archivo o a dos distintos, sin conflicto -- uno lee-y-escribe, el otro solo escribe con el resultado final).

**Verificado**: 4 tests de integración en `cli_port_registry.rs` contra el binario real, bindeando puertos de verdad: sin historial previo, la asignación es idéntica a la de siempre (secuencial desde `--port-base`); agregar un `.link` que alfabéticamente cae ANTES que uno ya registrado no mueve el puerto de ninguno de los dos ya asignados, el nuevo recibe el siguiente libre; borrar un `.link` y agregar uno distinto confirma que el nuevo NUNCA hereda el puerto liberado por el viejo (que sigue reservado en el archivo); un archivo de registro con JSON inválido falla limpio sin abrir ningún puerto.

---

### 3.154 `transaction { ... }`: transacciones SQL multi-escritura — RESUELTO, alcance acotado

Pedido real de un adoptador en fase de discovery (vía otra sesión de Claude coordinando la migración -- IgnisLove, checkout/pedidos): "crear pedido + descontar stock + cerrar carrito, con rollback si falla algo" no tenía forma segura de expresarse en un `.link` -- cada `insert`/`applyPatch`/`delete`/`increment` es autocommit individual (GRAMMAR.md §3.17/§2.1), así que un fallo a mitad de una secuencia de escrituras relacionadas dejaba la base en un estado a medias, sin ningún mecanismo del lenguaje para deshacerlo. Era el ÚNICO bloqueo real (no de conveniencia) para migrar un flujo de checkout completo -- confirmado explícitamente en esa misma conversación: "es el único punto de vuestra lista que bloquea migración completa por diseño, no por falta de código".

<!-- linkc:fragment -->
```
rpc checkout(productId: Int, qty: Int) -> Order {
  transaction {
    let matches = db.stock.findWhere(|s: Stock| { s.productId == productId });
    if matches.length() == 0 {
      panic("sin stock para ese producto");
    } else {
    }
    let s = matches[0];
    if s.quantity < qty {
      panic("stock insuficiente");
    } else {
    }
    db.stock.increment(s.id, |x: Stock| { x.quantity }, 0 - qty);
    db.orders.insert(Order { id: 0, productId: productId, qty: qty })
  }
}
```

**`transaction { ... }` es una expresión de BLOQUE, misma familia que `if`/`match` (GRAMMAR.md §3.7): retorna el valor de la última sentencia, y es de modo CHEQUEO nada más -- no se puede sintetizar sin un tipo esperado del contexto (`let x = transaction { ... };` sin anotación es un error de compilación, mismo motivo que `if`/`match` ahí). `BEGIN` real arranca antes de evaluar el cuerpo; si el bloque termina de correr normal, `COMMIT`; si CUALQUIER `RuntimeError` se propaga desde adentro (un `panic`, una violación de `@check`/`@unique` en una de las escrituras, un error de tipo en runtime, lo que sea), `ROLLBACK` automático y el error se propaga tal cual (mismo `500`/`400` de siempre, según el tipo de error) -- el caller nunca ve un pedido a medias.

**`panic(...)` es el mecanismo para abortar por una regla de negocio, no una novedad -- reusa exactamente lo que ya existía (§3.34).** No hay ningún `db.rollback()`/`abortTransaction()` nuevo: un chequeo con `panic` en el medio del bloque (guard clause, `if cond { panic(...); } else { }` como SENTENCIA, no como el tail -- ver el ejemplo arriba) hace exactamente lo que hace falta, sin agregar superficie de lenguaje nueva. `Result<T,E>`/`Result.Err{}` como tail del bloque NO dispara rollback -- se evaluó deliberadamente y se descartó: detectar "esto es un Result" tendría que ser estructural (por nombre de variante `"Err"`, ya que `Result<T,E>` no es un tipo especial del compilador, es una convención de enum declarado por el usuario, GRAMMAR.md §3.6) y ambiguo (¿cualquier enum con una variante `Err` cuenta?) -- `panic` es la señal inequívoca y ya establecida de "esto no debe seguir", sin inventar una regla nueva de detección.

**Publicación a `stream` DIFERIDA hasta el `COMMIT` -- el punto no negociable de todo el diseño.** Cada `insert`/`applyPatch`/`delete`/`increment` normalmente anuncia la fila a cualquier `stream` suscripto (`Db::publish`, §3.16) EN EL MOMENTO en que corre -- adentro de una `transaction` sin cerrar, eso mentiría: un suscriptor vería una fila que la base todavía podría rollbackear. `Db::commit_transaction`/`rollback_transaction` (nuevo, `transaction_pending_publishes: RefCell<Option<Vec<...>>>`) encola cada publicación mientras la transacción sigue abierta, y recién las entrega -- en el mismo orden en que se generaron, por el mismo camino (`deliver_local`/`notify_remote`, incluida la propagación cross-instancia de §3.44) -- si el `COMMIT` sale bien; si sale `ROLLBACK` (o el `COMMIT` en sí falla, tratado como rollback a todo efecto), la cola se descarta entera, sin publicar nada.

**No se puede anidar, y no admite `return` en su cuerpo (v0).** Las dos reglas se verifican en `checker.rs`, no en runtime: anidar una `transaction` dentro de otra es un error de compilación (`in_transaction`, mismo mecanismo `Cell<bool>` que `in_stream_body` ya usaba para el mismo tipo de restricción) -- una sola transacción SQL real por vez, sin savepoints en esta ronda. Un `return` alcanzable desde el cuerpo también se rechaza de entrada (`block_has_return`, mismo criterio y mismo mensaje que ya aplicaba a `while`, §3.15) -- reescribir el mecanismo de señalización de control de flujo para que "atraviese" un commit/rollback es un cambio bastante más grande que este ítem amerita; el patrón de guard-clause con `panic` (arriba) cubre el caso real sin necesitarlo.

**`in_transaction` SOLO atrapa el anidamiento SINTÁCTICO -- y desde el 26/08/2026, el caso que se le escapa da un error claro en runtime, no uno crudo del backend.** Bug real, encontrado por una auditoría multi-agente adversarial, no por un reporte externo: `in_transaction` es un `Cell<bool>` con alcance de UN `check_block` -- sin visibilidad sobre lo que hace una `fn` auxiliar llamada desde adentro. Una `transaction` alcanzada por una llamada a otra función que a su vez abre su PROPIA `transaction` (anidamiento real, pero a través de un límite de función, no de sintaxis) compilaba limpio y recién fallaba en runtime con el error crudo del backend ("cannot start a transaction within a transaction"), sin ninguna pista de qué regla de c-script se estaba violando. `Db::begin_transaction` (`runtime/db.rs`) ahora chequea, ANTES de intentar el `BEGIN` real, si ya hay una transacción abierta en esta misma ejecución -- mismo mensaje claro que el checker ya usa para el caso sintáctico. **Por qué esto nunca es un falso positivo entre requests distintas (actualizado 26/08/2026, GRAMMAR.md §3.158): no porque el servidor sea single-threaded** -- desde §3.158 no lo es -- **sino porque `Expr::Transaction` sostiene el candado REENTRANTE de la conexión física durante TODO `BEGIN`+cuerpo+`COMMIT`/`ROLLBACK` (`Db::with_exclusive_connection`).** Dos requests de DOS hilos distintos abriendo cada una su propia `transaction{}` no nesteada se serializan del todo por ese candado -- la segunda ni siquiera llega a `begin_transaction` hasta que la primera terminó (COMMIT o ROLLBACK) y ya dejó `transaction_pending_publishes` en `None` de nuevo. Solo el MISMO hilo puede volver a entrar mientras el candado sigue tomado (reentrante, por diseño) -- exactamente el caso de anidamiento-vía-función que este chequeo existe para atrapar. El límite de FONDO (el checker estructuralmente no puede atrapar esto en compilación, solo en runtime) queda igual -- ver el bullet de abajo.

**Los dos backends comparten la MISMA implementación -- sin código nuevo por motor.** `Backend::execute_ddl("BEGIN"/"COMMIT"/"ROLLBACK")` ya existía (`conn.execute_batch` en SQLite, `client.batch_execute` vía `with_reconnect` en Postgres) -- ningún método nuevo en la capa de `store.rs`, la transacción sale de reusar exactamente lo que `linkc migrate`/las migraciones no destructivas ya usaban para DDL. Una conexión Postgres que se cae A MITAD de la transacción (`with_reconnect` la repara para la SIGUIENTE llamada, nunca reintenta la que falló, §3.40) se comporta de forma segura por construcción: Postgres aborta cualquier transacción en curso al perder la conexión, así que ni el `COMMIT` puede colarse por una conexión "nueva" que nunca vio el `BEGIN` -- el error se propaga como cualquier otro, con rollback best-effort (que puede fallar/no-op sin problema, la base ya descartó todo por su cuenta).

**Límites honestos:**
- **Sin savepoints ni anidamiento real.** Una transacción anidada necesitaría un mecanismo aparte (`SAVEPOINT`/`RELEASE`) -- rechazada de entrada en vez de fingir soportarla mal.
- **Sin control fino de nivel de aislamiento.** Corre con el nivel default de cada motor (`READ COMMITTED` en Postgres) -- no hay forma de pedir `SERIALIZABLE`/`REPEATABLE READ` desde el `.link`.
- **`return` no se puede usar adentro (ver arriba) -- `panic` para abortar, un valor de cola normal para terminar bien.**
- **No hay forma de leer si la transacción actual sigue "viva" desde dentro de una función auxiliar, así que el anidamiento a través de una llamada a función NUNCA se atrapa en compilación** -- `db.transaction` no es un valor que se pueda pasar/inspeccionar, es puramente sintáctico. Desde el 26/08/2026 al menos falla con un mensaje claro en runtime (ver arriba) en vez de un error crudo del backend, pero sigue siendo un error de RUNTIME, no de compilación -- si esto se vuelve un problema real de ergonomía, cerrarlo de verdad necesitaría análisis de call-graph en el checker (¿esta `fn` abre una `transaction`, directa o transitivamente?), una pieza de diseño bastante más grande que este ítem.
- **`matchFn`/predicados usados ADENTRO de una transacción no ganan ningún pushdown especial nuevo** -- `findWhere`/`countWhere`/`upsert` (§3.95/§3.108/§3.145/§3.75) siguen empujando a SQL exactamente igual que fuera de una transacción, ni mejor ni peor.

**Verificado**: 6 tests de checker (tipa contra el retorno del rpc; en posición de sentencia se chequea contra `Void`; anidar una transacción dentro de otra se rechaza; `return` directo y anidado dentro de un `if` se rechazan; en posición de síntesis -- un `let` sin anotación -- se rechaza) + 3 de `runtime/mod.rs` contra SQLite real vía `invoke_rpc` (una transacción exitosa confirma CADA escritura, por conteo real de filas y valores; un `panic` a mitad de camino confirma que NINGUNA escritura sobrevive, ni el `increment` ni el `insert` posterior que nunca llegó a correr; la base sigue perfectamente utilizable después de un rollback, sin ningún estado atascado) + 2 tests de integración en `cli_transaction.rs` contra el binario real, con un `stream` conectado por un socket de verdad: un checkout que rollbackea NUNCA genera ningún evento SSE (confirmado con un timeout real, no solo "no lo vi"), mientras uno exitoso sí genera exactamente uno, después de confirmar; más el mismo commit/rollback confirmado por HTTP puro (status + valores). Más 1 test contra un Postgres REAL (`pg_integration.rs`) confirmando que `BEGIN`/`COMMIT`/`ROLLBACK` funcionan igual en ese backend, no solo en SQLite. Sobre el bug de anidamiento vía función: 1 test más en `runtime/mod.rs` confirmando el mensaje claro (no el error crudo del backend) y que la base sigue usable después, sin filas a medio escribir.

---

### 3.155 `@unique(campo1, campo2, ...)`: constraint UNIQUE compuesto a nivel de `type` — RESUELTO

Segundo ítem del mismo barrido de auditoría propia de Glowapp que motivó retomar este backlog (PLAN.md §9.3, item 2 -- distinto pedido del que motivó §3.154, que vino de IgnisLove): `@unique`/`@index` de campo (§3.80) resuelven "este valor no se repite en toda la tabla", pero un caso real muy común -- "un slug único POR PERFIL, no globalmente" (`@unique(["profileId", "slug"])` en el ejemplo original) -- necesita un constraint sobre VARIOS campos a la vez, algo que `FieldAnnotation` (atado a UN campo) no puede expresar. El propio comentario de §3.80, escrito al cerrar esa ronda, ya lo anticipaba: "necesitaría una anotación a nivel de `type`, que hoy no existe (`TypeDecl` no tiene `annotations`)".

<!-- linkc:check -->
```rust
@unique(profileId, slug)
type Product = {
  id: Int,
  profileId: Int,
  slug: String,
  name: String,
}
```

**`TypeDecl` gana `annotations: Vec<TypeAnnotation>` -- un enum APARTE de `Annotation` (el de `RpcDecl`) y de `FieldAnnotation` (el de `Field`), mismo criterio que esos dos: cada punto de anclaje del lenguaje tiene su propio enum chico, en vez de reusar uno más grande que obligaría al checker a rechazar en runtime combinaciones que el parser ya podría haber descartado por forma.** Por ahora `TypeAnnotation` tiene una sola variante, `Unique(Vec<String>)` -- identificadores sueltos separados por coma, mismo criterio sintáctico que `@invalidates(rpc1, rpc2, ...)` (§3.125). Sintaxis: `@unique(...)` va ANTES de `type`, nunca antes de `enum`/`service`/`const`/`fn`/`db`/`test` -- es la única anotación de nivel superior que existe hoy.

**Al menos 2 campos -- un solo campo ya tiene su propia forma, más simple (`FieldAnnotation::Index { unique: true }`, §3.80).** El checker valida, además: que cada nombre listado sea un campo REAL del struct (`'@unique(...)' nombra 'X', que no es un campo declarado de este type`); que no se repita el mismo campo dos veces dentro de un mismo `@unique(...)`; que `@unique(...)` no se declare sobre un `type` que no tenga forma de struct (un alias como `type Ids = Int[]`); y que dos `@unique(...)` sobre el MISMO type no declaren exactamente el mismo conjunto de campos (sin importar el orden -- `@unique(a, b)` y `@unique(b, a)` son el mismo constraint, declararlo dos veces es redundante, no dos índices distintos). Varios `@unique(...)` con conjuntos de campos DISTINTOS sobre el mismo `type` sí son válidos -- cada uno genera su propio índice.

**DDL: `CREATE UNIQUE INDEX IF NOT EXISTS "idx_<tabla>_uniq_<campos codificados>" ON "<tabla>"("<campo1>", "<campo2>", ...)` -- misma sentencia, válida en los DOS backends sin diferencias.** Mismo criterio de idempotencia y nombre determinístico que el `@unique` de un solo campo (§3.80): corre en CADA arranque, sin necesitar detectar "¿ya existía?". `--adopt-existing` nunca ejecuta este DDL, mismo criterio que el resto del schema. `linkc build` emite la sentencia estática en `schema.postgres.sql` (`codegen::postgres_emit.rs`, importando `composite_unique_index_name` de `runtime/db.rs` -- el emisor estático no instancia ningún `Db` real, pero SÍ puede reusar funciones puras de esa capa sin problema), y `linkc migrate --dry-run` (§3.97) lo incluye en su reporte.

**Bug real, encontrado por una auditoría multi-agente adversarial (26/08/2026): el nombre de índice original (`fields.join("_")`) era AMBIGUO cuando un nombre de campo ya tenía un guion bajo, y "IF NOT EXISTS" convertía la colisión en un no-op silencioso.** `@unique(a_b, c)` y `@unique(a, b_c)` sobre el MISMO `type` generaban el mismo nombre, `idx_<t>_a_b_c` -- el checker no lo atrapaba (dedup por CONJUNTO de campos, nunca por el nombre derivado del índice), así que la SEGUNDA sentencia `CREATE UNIQUE INDEX IF NOT EXISTS` con ese nombre nunca creaba nada -- su constraint quedaba sin enforcar de verdad, en silencio, mientras el primero seguía funcionando. Confirmado en vivo: una fila que violaba el segundo `@unique` se aceptaba con 200, no el 400 documentado. `composite_unique_index_name` (`runtime/db.rs`) ahora codifica cada campo con un prefijo de longitud (`"{len}${nombre}"`, concatenados sin separador extra entre campos -- mismo principio que Bencode/netstrings) en vez de un `join` con `_`: esto es INYECTIVO por construcción -- dos secuencias distintas de nombres de campo nunca pueden producir la misma codificación, a diferencia de un `join` con un separador que también puede aparecer dentro de un campo.

**Bug real encontrado en la verificación manual, antes de shippear -- preexistente, no introducido por este ítem: una violación de `@unique`/`@check` contra Postgres real daba `500`, no el `400` que §3.80/§3.96 documentan.** `postgres::Error::to_string()` para un error devuelto por el SERVIDOR (`as_db_error()`) es el literal fijo `"db error"`, sin el mensaje real -- `is_unique_violation`/`is_check_violation` (`runtime/db.rs`, buscan un substring en el texto del error) nunca matcheaban nada real contra ese backend, así que TODA violación cara a cara con Postgres cayó siempre como 500 genérico, silenciosamente, desde que esas dos anotaciones existen. Arreglado clasificando por **SQLSTATE** (`db_err.code()`, `runtime/store.rs::describe_postgres_error`) en vez de por mensaje: el código (`23505`/`23514`) es la parte del protocolo que NUNCA se traduce, a diferencia del mensaje humano -- que SÍ está localizado según `lc_messages` del servidor (confirmado en la propia verificación: el Postgres de prueba de esta sesión devuelve "llave duplicada viola restricción de unicidad", no "duplicate key..."). El fix antepone la frase fija en inglés que `is_unique_violation`/`is_check_violation` ya buscaban (generada por c-script, no por Postgres) al mensaje real -- las dos funciones existentes no necesitaron ningún cambio.

**Verificado**: 8 tests de checker (tipa con 2 y con 3+ campos; campo inexistente rechazado; menos de 2 campos rechazado; campo repetido dentro del mismo `@unique` rechazado; sobre un `type` que no es struct rechazado; el MISMO conjunto declarado dos veces -- en cualquier orden -- rechazado por redundante; dos `@unique` con conjuntos DISTINTOS conviven sin problema) + 1 en `runtime/mod.rs` contra SQLite real vía `invoke_rpc` (mismo `(profileId, slug)` rechazado; compartir solo UNO de los dos campos con una fila existente sigue siendo válido, confirmando que es compuesto de verdad, no dos constraints de un campo) + 1 en `codegen::postgres_emit` (el DDL estático de `linkc build` incluye la sentencia multi-columna exacta) + 2 contra un Postgres REAL (`pg_integration.rs`): el constraint compuesto enforced de punta a punta (mismo par rechazado, cualquiera de los dos campos distinto acepta), y un test dedicado al fix del bug de arriba confirmando el status `400` real por HTTP, no solo por inspección de código. Sobre el bug de colisión de nombre de índice: 1 test más en `runtime/mod.rs` (dos `@unique` cuyos nombres colisionarían con el `join` viejo, los dos rechazan de verdad) + 1 en `codegen::postgres_emit` (los dos nombres de índice emitidos son DISTINTOS).

**Mitad CONDICIONAL (`where <expr>`) resuelta después, ver §3.174.**

### 3.156 `Int64` como `bigint` real en `client.ts` — RESUELTO, cierra el límite que dejaba abierto §3.30

Tercer ítem de la auditoría propia de Glowapp (PLAN.md §9.3): §3.30 había resuelto la corrección del WIRE (string, para no perder precisión arriba de `2^53`) pero dejó el tipo TS emitido en `string`, documentando explícitamente que subir a `bigint` real "sería arquitectura nueva" -- un walker recursivo dirigido por tipo que distinga "este string es semánticamente un Int64" de "este campo `String` que por casualidad parece un número", cosa que ninguna otra feature de este proyecto necesitaba todavía. Con un caso concreto real pidiéndolo, esta ronda construye exactamente eso.

**El wire NO cambia -- sigue siendo string en las dos direcciones (§3.30, `Value::Int64(n) => json!(n.to_string())` en `runtime/mod.rs`, intacto).** Lo único que cambia es el lado TypeScript: `contract.d.ts`/`client.ts`/`hooks.ts` ahora declaran `bigint` de verdad (`render_type`, `ts_emit.rs`) donde antes decían `string`, y el cliente generado hace la conversión en las dos direcciones -- sin que el servidor Rust necesite tocarse en absoluto.

**Ida (request): un replacer estructural, sin walker dirigido por tipo -- esa mitad SÍ era simple.** Un `bigint` real revienta `JSON.stringify` sin más ("Do not know how to serialize a BigInt"). A diferencia de la vuelta, acá no hace falta saber CUÁL argumento es `Int64`: cualquier `bigint` en el árbol se vuelve texto sin ambigüedad posible, así que un único replacer (`__int64SafeStringify`, emitido en `client.ts` y en `hooks.ts` -- las dos únicas superficies que serializan args a JSON) alcanza. Se emite condicionalmente: si NINGÚN rpc del programa manda un `Int64`, el helper ni aparece (mismo criterio de `noUnusedLocals`-safe que el resto de este archivo, ej. `has_any_query`).

**Vuelta (response): acá SÍ hace falta el walker dirigido por tipo -- un string no dice por sí solo si es un `Int64`, un `Timestamp`, un `Uuid` o un `String` que por casualidad es numérico.** `validators_emit.rs` suma un segundo juego de funciones exportadas junto a los `isX` de siempre, con el mismo mecanismo worklist/seen: `contains_int64(ty, checker)` (¿este tipo contiene un `Int64` en algún lado, expandiendo `Generic`/`Enum` de verdad vía el checker -- a diferencia de `type_contains_function`, checker.rs, que se detiene en un genérico opaco, porque dejar uno sin expandir acá sería una MENTIRA de tipos silenciosa: `contract.d.ts` prometiendo `bigint` sobre un valor que en runtime sigue siendo string) y `reviveX(x: any): X` (un `revive<Nombre>` por cada tipo con nombre que efectivamente contiene algún `Int64` -- ninguno para los que no, cero costo generado para el caso común). `client.ts` llama al revividor correspondiente ANTES de validar (`isInt64` ahora exige `typeof x === "bigint"`, no `"string"` -- el orden importa: revivir primero, validar después) y antes de devolver el valor al caller.

<!-- linkc:fragment -->
```rust
type Counter = { id: Int, big: Int64 }
db { counters: Counter[] }

service Counters {
  rpc addAmounts(a: Int64, b: Int64) -> Int64 { a + b }
}
```

Genera (recortado):

```typescript
// contract.d.ts
export interface Counter { id: number; big: bigint; }

// validators.ts
export function isCounter(x: unknown): x is Counter {
  return (... && (typeof (x as any).big === "bigint" && (x as any).big >= -9223372036854775808n && (x as any).big <= 9223372036854775807n));
}
export function reviveCounter(x: any): Counter {
  return { ...(x as any), big: BigInt((x as any).big as any) };
}

// client.ts
async addAmounts(a: bigint, b: bigint, options?: { signal?: AbortSignal }): Promise<bigint> {
  const res = await fetch(..., { body: __int64SafeStringify({ a, b }), ... });
  const json: unknown = await res.json();
  const revived: unknown = BigInt(json as any);
  if (!(typeof revived === "bigint" && ...)) throw new LinkValidationError("addAmounts", revived);
  return revived as bigint;
}
```

**Cobertura del walker: struct (con nombre y anónimo), `Optional<T>`, `List<T>`, `Tuple`, `MapOf` (posición de valor -- la clave nunca es `Int64`, el checker ya limita `Map<K,V>` a `K: String|Int`), `Union`, `Result<Ok,Err>`, `Patch<T>`, y `Generic<...>`/`enum` con datos -- estos dos últimos expandidos de verdad vía el checker, no tratados como opacos.** `Patch<T>` tiene el mismo tratamiento "todo opcional, omitido = no tocar" que ya usa su validador (`render_patch_fields_check`).

**`Union`: revivir CADA candidato primero, recién ahí validar -- no al revés. [ACTUALIZADO tras un bug real, 26/08/2026].** El diseño original disambiguaba corriendo el `isX` de cada miembro contra el valor CRUDO (sin revivir), asumiendo que alcanzaba con la misma lógica que ya usa el validador -- pero `isInt64` (esta misma ronda) ya asume la forma POST-revivida (`typeof === "bigint"`), nunca la del wire crudo (siempre string, §3.30). Efecto: un miembro `Int64` de una unión NUNCA matcheaba contra el valor sin revivir, la disambiguación caía siempre al siguiente miembro que sí matcheara la forma cruda (típicamente `String`), y el valor real quedaba como string PARA SIEMPRE -- silencioso, porque `isEvent` sobre la unión completa (`... || typeof payload === "string"`) también pasaba con el string sin revivir. Encontrado por una auditoría multi-agente adversarial, no por los tests originales -- ninguno de los 5 tests nuevos de esta sección ejercitaba un `Union`. Fix: cada candidato se revive PRIMERO (con su propio revividor, o identidad si ese miembro no tiene `Int64` adentro), envuelto en `try/catch` -- revivir un candidato que NO es de verdad ese miembro puede lanzar de verdad (`BigInt("no numérico")` tira `SyntaxError` real, no falla el chequeo después de forma silenciosa) -- y recién sobre el candidato YA revivido corre el `isX` de ese miembro; el primer candidato que valida se devuelve, mismo orden que antes.

**Verificado**: 5 tests unitarios nuevos en `validators_emit.rs` (un tipo sin `Int64` no emite ningún `revive...`; `List<Int64>`/`Int64?` revividos recursivamente, preservando `null`; un genérico opaco `Box<Int64>` revivido igual, sin tratarlo como caja negra; una variante de `enum` con un campo `Int64` revivida) + 1 test más (26/08/2026) sobre el bug de `Union` de arriba (`Int64 | String` revive el candidato Int64 de verdad, envuelto en `try/catch`) + los dos tests existentes de §3.30 actualizados a la nueva forma (`bigint`, no `string`). Verificado además con el binario real y `tsc --strict --noUnusedLocals` de verdad (no solo inspección): un programa `.link` con un campo `Int64`, compilado de punta a punta, tipa limpio contra un `tsconfig.json` real; y contra un `linkc serve` real -- `i64::MAX` como parámetro `bigint` real, ida y vuelta por HTTP contra una lista, un scalar de retorno y un `findWhere` filtrando por ese campo, confirmando en runtime real (no solo en el tipo declarado) que el valor sigue siendo `9223372036854775807n` exacto en cada punto.

**Límites honestos, alcance de esta ronda:** `schemas.ts` (Zod) sigue aceptando la unión laxa `number | string | bigint` para `Int64` (sin cambios) -- ese archivo existe para validar FORMULARIOS (un input de React puede llegar como string antes de convertirse), un propósito distinto al de `validators.ts`/`client.ts`, que validan la forma exacta del wire ya revivido; no se tocó a propósito. `openapi.json` sigue documentando `Int64` como `{"type": "integer", "format": "int64"}` (sin cambios, preexistente a esta ronda) -- convención común en APIs reales (Stripe, entre otras) para precisión de 64 bits, aunque no coincide 1:1 con el wire real (string); fuera de alcance de este ítem puntual.

---

### 3.157 `.truncateToDay()`/`.truncateToMonth()`/`.truncateToYear()`: agregación agrupada por fecha — RESUELTO, cierra el límite que dejaba abierto §3.65

Cuarto ítem de la auditoría propia de Glowapp: §3.65 había dejado documentado a propósito que agrupar por un `Timestamp` no estaba soportado ("un `Timestamp` se guarda como milisegundos exactos, así que agruparlo tal cual produciría un grupo por fila, nunca cohortes reales"), señalando exactamente la forma que faltaba -- un método de truncado reconocido en la posición de selector. Esta ronda lo construye.

<!-- linkc:fragment -->
```rust
type Sale = { id: Int, at: Timestamp, amount: Int }
type DayTotal = { key: Timestamp, value: Int }
db { sales: Sale[] }

service Sales {
  rpc revenueByDay() -> DayTotal[] {
    db.sales.sumBy(|s: Sale| { s.at.truncateToDay() }, |s: Sale| { s.amount })
  }
  rpc revenueByMonth() -> DayTotal[] {
    db.sales.sumBy(|s: Sale| { s.at.truncateToMonth() }, |s: Sale| { s.amount })
  }
}
```

**La ÚNICA posición de todo el lenguaje donde un método existe sobre un `Timestamp` -- §3.31 sigue prohibiendo cualquier otro uso.** `.truncateToDay()`/`.truncateToMonth()`/`.truncateToYear()` NUNCA se evalúan de verdad: `ast::recognize_group_key_selector` (nuevo, junto a `recognize_field_selector` que ya usaban `maxRow`/`minRow`/`increment`) reconoce sintácticamente el shape `|item: T| item.campo.truncateToX()` sobre el selector de CLAVE de `sumBy`/`countBy`/`avgBy`/`maxBy`/`minBy` -- mismo espíritu que `recognize_live_subscribe` con `db.<c>.subscribe()`, nunca una llamada a método real. `t.truncateToDay()` en cualquier OTRA posición del código sigue siendo el mismo error del checker de siempre ("Sin ningún método propio sobre Timestamp"). El resultado agrupado sigue tipando `Timestamp`, no un tipo nuevo -- truncar reduce precisión, no cambia qué ES el valor.

**Los dos backends divergen de verdad, como ya se documentaba -- resuelto con SQL específico por motor, ambos devolviendo milisegundos-desde-epoch planos, nunca un tipo de fecha nativo.** SQLite trunca con los modificadores nativos de `strftime` (`'start of day'`/`'start of month'`/`'start of year'`) sobre el valor ya convertido a segundos-desde-epoch, multiplicado de vuelta por 1000. Postgres usa `date_trunc(unit, to_timestamp("campo" / 1000.0), 'UTC')` -- el overload de TRES argumentos (PG 9.4+) que trunca EN una zona horaria explícita, no la variante de dos argumentos, que dependería en silencio del `TimeZone` configurado en la sesión del servidor -- exactamente la clase de bug "funciona en mi Postgres, no en el de producción" que este límite documentaba desde el principio. El resultado se vuelve a convertir a milisegundos con `EXTRACT(EPOCH FROM ...) * 1000`, casteado a `BIGINT` -- así `scalar_cell_to_value`/`ColumnKind::Timestamp` (que ya prueba `i64` primero, `postgres_timestamp_cell`, store.rs) lo decodifica exacto, sin necesitar ninguna rama nueva.

**Bug real encontrado en la verificación manual, antes de escribir el test automatizado -- preexistente, expuesto por esta ronda, no introducido por ella.** `scalar_cell_to_value` (`runtime/db.rs`) no tenía ningún brazo para `Type::Timestamp` -- nunca hizo falta antes, porque `Timestamp` jamás había sido una clave de agrupación válida. Sin el brazo, el resultado caía al genérico `(_, Cell::Int(n)) => Value::Int(n)` y la clave truncada viajaba como NÚMERO plano en el JSON, rompiendo en silencio la promesa de §3.31 (`Timestamp` siempre string ISO-8601 en el wire) -- el mismo bug de etiquetado que §3.65 ya había encontrado y cerrado para `Int64` en su momento, ahora con `Timestamp`. Un brazo `(Type::Timestamp, Cell::Int(n)) => Value::Timestamp(n)`, mismo criterio exacto que el de `Int64` ya existente, lo cierra.

**Segundo bug real, encontrado por una auditoría multi-agente adversarial (26/08/2026) -- la verificación manual pre-1970 de esta sección, al momento de escribirla, en realidad NO cubría el caso que rompía.** El lado SQLite de `truncate_timestamp_sql` hacía `"campo" / 1000` con los DOS operandos enteros -- división ENTERA de SQLite, que trunca HACIA CERO, no hacia abajo. Para un epoch NEGATIVO (pre-1970) con resto de milisegundos no nulo, eso redondea hacia 1970 en vez de alejarse de él, empujando la fila al día/mes/año SIGUIENTE en vez del correcto -- confirmado con SQLite real: `-500 / 1000` da `0` (1970-01-01), en vez de los `-1` (segundos, floor real) que corresponden a 1969-12-31. La verificación manual que este párrafo describía como "resultados idénticos entre los dos motores" usaba fechas construidas con `dateFromParts(...)` -- que no tiene parámetro de milisegundos, así que nunca pasaba un resto fraccionario y nunca ejercitaba este camino. Fix: `"campo" / 1000.0` (división real, mismo criterio que ya usaba el lado Postgres) -- confirmado con SQLite real que `-500 / 1000.0` sí da `-86400` segundos (1969-12-31), el día correcto.

**Verificado**: 3 tests de checker (acepta las tres granularidades sobre un campo `Timestamp` real; sigue rechazando un `Timestamp` SIN truncar como clave, el límite original de §3.65 sigue en pie para ese caso; rechaza `.truncateToDay()` sobre un campo que no es `Timestamp`) + 1 test de runtime contra SQLite real (`runtime/mod.rs`, agrupa por día/mes/año sobre datos reales, confirmando la SUMA agrupada y que la `key` que vuelve es un `Timestamp` EXACTO comparado contra `dateFromParts(...)`, no solo que el conteo de grupos cierre) + 1 test contra un Postgres real (`pg_integration.rs`, mismo caso, confirmando además que la `key` viaja como string ISO-8601 en el JSON -- la parte que el bug de arriba habría roto). Sobre el segundo bug (división entera): 1 test más en `runtime/mod.rs` con una fecha pre-1970 CON resto de milisegundos (un caso que `dateFromParts` no puede construir, mandado como string ISO-8601 crudo vía `invoke_rpc`, igual que llegaría por HTTP real) confirmando el día correcto contra SQLite.

**Fuera de alcance de esta ronda, a propósito:** granularidades más finas (`truncateToHour`/`truncateToWeek`) o más gruesas (`truncateToQuarter`) -- Día/Mes/Año cubren la mayoría real de reportes de negocio, el resto queda para si aparece demanda concreta. Truncar por una zona horaria DISTINTA de UTC (`truncateToDay("America/Argentina/Buenos_Aires")`, por ejemplo) -- `Timestamp` es UTC puro por diseño (§3.31), soportar otra zona necesitaría decidir de dónde sale esa zona (parámetro del rpc, config del servidor) y es una pieza de diseño separada.

---

### 3.158 `linkc serve`: un hilo por request — RESUELTO, Etapa 1 de un roadmap de concurrencia mayor

Pedido de fondo relayado por skynet-d3 (VPS de IgnisLove/MyFinance) a nombre de Carlos: un roadmap de tres pilares (concurrencia, FFI hacia crates de Rust, sistema de módulos) para que Link pueda reclamar en serio "más rápido que Node/Go" -- el motivo concreto citado fue una recomendación anterior de esta misma auditoría de NO migrar ningún flujo con alta concurrencia I/O (varias pasarelas de pago en paralelo) porque Node lo hacía mejor. Antes de esta ronda, `linkc serve` era estrictamente un hilo, una request a la vez -- todo el runtime (`Db`, sesiones, rate limiter) estaba construido sobre `RefCell`/`Cell`, seguro únicamente porque nunca había dos requests corriendo al mismo tiempo.

**Se evaluaron dos caminos, documentados en una propuesta previa a escribir una sola línea de código (ver PLAN.md §9.2): reescribir el intérprete entero sobre `tokio`/async, o un hilo por request con candados.** El primero exige "colorear" cada función del árbol de evaluación como `async fn` (todo `eval_expr`/`eval_block`, cada builtin) -- meses de trabajo, riesgo real de reintroducir la clase de bug "dos caminos que discrepan" que este proyecto viene documentando desde §3.9. El segundo resuelve el caso real citado (una llamada `http.*` lenta bloqueando a todo el servidor) sin tocar el intérprete en absoluto -- se eligió este.

**Un hilo del sistema operativo por request, SIN pool acotado todavía (ver "Límites honestos").** `runtime/server.rs::serve` deja de llamar `handle_request` en línea sobre el hilo que acepta conexiones -- ahora `std::thread::spawn` uno nuevo por request, capturando clones baratos (`Arc::clone`, un incremento de refcount) de todo el estado compartido. El hilo que acepta conexiones vuelve enseguida a `incoming_requests()`/`recv_timeout` sin esperar a que la request anterior termine.

**`Db` es `Send + Sync` -- el cambio de fondo que hace posible todo lo demás.** Cada campo de interior-mutability que antes era `RefCell`/`Cell` (seguro solo por la garantía "un hilo, una request a la vez") pasó a una primitiva realmente segura entre hilos, elegida según cómo se usa cada uno:

| Campo | Antes | Ahora | Por qué |
|---|---|---|---|
| Conexión real (SQLite/Postgres, `store.rs::Backend`) | `Connection` sola / `RefCell<Client>` | `parking_lot::ReentrantMutex<Connection>` / `ReentrantMutex<RefCell<Client>>` | **Reentrante**, no un `Mutex` común -- ver el párrafo de `transaction` abajo, la razón real de esta elección |
| `subscribers`/`pending_notify_retries`/`oversized_notify_drops`/`transaction_pending_publishes` (`db.rs`) | `RefCell<...>` | `parking_lot::Mutex<...>` | Estado compartido de verdad entre requests, sin necesidad de sostenerse a través de una llamada anidada |
| `argon2_params`/`http_timeout` (`db.rs`) | `RefCell<...>` | `parking_lot::RwLock<...>` | Escrito UNA vez al arrancar, leído por todos los hilos de request después -- muchos lectores concurrentes no necesitan turnarse |
| `current_request`/`response_status_override`/`response_location_override` (`db.rs`) | Campos de `Db` (`RefCell`/`Cell`) | `thread_local!` | Genuinamente PER-REQUEST, no compartido -- con un hilo dedicado por request, "el contexto de la request actual" es exactamente lo que un `thread_local!` modela, sin ningún candado |
| `sessions`/`failed_logins` (`session.rs::SessionStore`) | `RefCell<...>` | `parking_lot::Mutex<...>` | Mismo motivo que `subscribers` |
| `in_stream_body`/`in_transaction`/`hover_result` (`checker.rs`) | `Cell<bool>`/`RefCell<...>` | `AtomicBool` (`std::sync`)/`Mutex` (`std::sync`) | `Db` guarda un `Checker` propio para resolver tipos en runtime -- necesitaba `Sync` aunque estos campos específicos nunca se mutan en esa copia. `std::sync`, no `parking_lot`, en `checker.rs`: este módulo compila también a `wasm32-unknown-unknown` sin el feature `runtime` (detrás del cual vive `parking_lot`, ver Cargo.toml) |
| `rate_limiter`/`idempotency_store`/`cache_store`/`metrics_store` (`server.rs`) | `&mut` locales del loop principal | `Arc<parking_lot::Mutex<...>>` | Cuatro candados INDEPENDIENTES -- un rate-limit check de una request nunca espera a que otra termine de escribir en el cache |

**`transaction { }` (§3.154) necesita un candado REENTRANTE, no uno común -- el detalle que decide toda la arquitectura de arriba.** `Db::with_exclusive_connection` (nuevo) sostiene el candado de la conexión por TODA la duración de `BEGIN` + el cuerpo del bloque + `COMMIT`/`ROLLBACK`, para que otro hilo (otra request) nunca pueda intercalar una escritura suya en la MISMA conexión física a mitad de una transacción ajena -- verificado que ROMPE sin esto (dos transacciones concurrentes escribiendo sobre la misma fila via `increment` perdían actualizaciones antes de este diseño). Pero el CUERPO de la transacción llama de vuelta a `insert`/`applyPatch`/`increment`/etc., que TAMBIÉN piden este mismo candado para su propia operación individual -- con `std::sync::Mutex` (no reentrante), eso sería el mismo hilo bloqueándose a sí mismo, deadlock garantizado, no una condición de carrera rara. `parking_lot::ReentrantMutex` deja que el MISMO hilo vuelva a pedirlo cuantas veces haga falta sin bloquearse; otro hilo sí espera de verdad. Consecuencia real: sin pooling de conexiones (ver "Límites honestos"), dos escrituras de DOS requests DISTINTAS a la base siguen siendo mutuamente exclusivas -- lo que cambia es que ya NO se turnan para esperar una llamada `http.*` que ninguna de las dos necesita.

<!-- linkc:fragment -->
```rust
service Bench {
  rpc slowGatewayCall(orderId: Int) -> String {
    // Mientras esta llamada espera la respuesta de la pasarela, OTRA
    // request puede insertar/leer/actualizar la base sin esperar a que
    // termine -- `http.*` nunca toca el candado de la conexión.
    http.post("https://payment-gateway.example.com/charge", "{}", [])
  }
}
```

**Verificado de punta a punta, no solo por inspección de tipos.** Contra un `linkc serve` real (SQLite Y Postgres), con un endpoint HTTP local que duerme 2 segundos antes de responder: 5 llamadas SECUENCIALES a un rpc que hace `http.get` contra ese endpoint tardan ~11s; las MISMAS 5 disparadas en paralelo (`curl ... &` × 5) tardan ~2.3s -- confirma paralelismo real, no una mejora cosmética. 30 `insert` concurrentes sobre la misma colección: 30 filas, 30 ids únicos, sin huecos ni duplicados. 40 `transaction { increment(...) }` concurrentes sobre la MISMA fila, en SQLite y en Postgres por separado: el resultado final es exactamente 40 en los dos casos -- ni un update perdido ni dos transacciones entrelazadas. Un `stream` suscripto desde una request recibe correctamente eventos publicados por OTRAS tres requests concurrentes, cada una en su propio hilo -- confirma que el pub/sub cruza hilos sin perder ni duplicar eventos. Además, dos tests automatizados nuevos en `runtime/mod.rs` reproducen los casos de inserts/transacciones concurrentes con hilos de sistema operativo reales (`std::thread::spawn`, no un mock), corridos 5 veces seguidas sin ninguna falla intermitente, para que esta garantía quede en CI y no solo en una verificación manual de esta ronda.

**Dos bugs preexistentes encontrados VERIFICANDO el build a `wasm32-unknown-unknown` (aspiracional, "playground web" -- nunca shippeado, sin cobertura de CI), no introducidos por esta ronda.** `pub mod migrate;` (`lib.rs`) nunca había quedado detrás del feature `runtime` pese a que el propio módulo "habla PostgreSQL de verdad" (mismo motivo que `runtime`/`introspect`, que sí estaban gateados) -- corregido, gateado ahora. `codegen::postgres_emit` (parte del subconjunto PROMETIDO como compatible con wasm) ya importaba `check_clause_sql`/`check_fields_by_collection` de `runtime::db` sin ningún gate, desde antes de esta sesión -- confirmado no introducido por esta ronda (revisando el historial), pero SIGUE roto: no se atacó, fuera de alcance de este ítem puntual (desenredar esas dos funciones de `runtime::db` es un problema aparte, sin relación con concurrencia).

**Límites honestos, alcance de esta Etapa 1:**
- **Sin pool de conexiones -- sigue habiendo UNA sola conexión física a la base, ahora protegida por un candado en vez de por el single-threading del intérprete.** Dos escrituras de dos requests distintas siguen siendo mutuamente exclusivas entre sí (aunque ya no esperan una llamada `http.*` ajena) -- el throughput de escritura pura no mejora con esta ronda, solo la capacidad de atender MUCHAS conexiones/esperas de I/O externo a la vez. Un pool real (SQLite con más de una conexión al mismo archivo en modo WAL -- ya activado --, o Postgres con `deadpool`/`bb8`) es la Etapa 3 potencial del roadmap, no atacada acá.
- **Sin pool de hilos acotado -- un hilo de sistema operativo por request, sin límite superior.** Un cliente que abre miles de conexiones simultáneas crea miles de hilos -- un vector de agotamiento de recursos real que esta ronda no cierra. Mitigación real (backpressure, un pool de tamaño fijo con cola) queda para una ronda dedicada, con su propio diseño de qué hacer cuando el pool está lleno (rechazar con 503, encolar con timeout, etc.).
- **`http.*` DENTRO de un `transaction { }` sigue bloqueando a las demás requests que tocan la base** -- el candado de la conexión se sostiene por TODA la transacción, `http.*` adentro de ese bloque no tiene forma de liberarlo temporalmente sin arriesgar la exclusividad que la transacción necesita. Aceptado a propósito: una llamada de red arbitraria dentro de una transacción SQL ya es una práctica cuestionable en general (mantiene una transacción abierta por tiempo indefinido), no algo específico de esta arquitectura.
- **Pilares 2 (FFI hacia crates de Rust) y 3 (sistema de módulos) del roadmap más amplio quedan sin atacar** -- necesitan su propio discovery, no forman parte de esta ronda.

**Segunda ronda (26/08/2026, mismo día): dos bugs reales de concurrencia encontrados auditando esta MISMA sección recién shippeada -- ninguno reportado externamente, los dos en el cruce `stream`/`transaction` que §3.16 ya había marcado como el punto a revisar "si `Db` alguna vez dejara de ser single-threaded".**

1. **Fila perdida en silencio: `Db::subscribe` sacaba la foto y RECIÉN DESPUÉS se registraba como suscriptor**, dos pasos sin candado compartido con `publish`/`deliver_local` -- correcto mientras el servidor procesaba una request a la vez, roto con hilos reales: un `insert`/`applyPatch` de OTRO hilo podía commitear y publicar EXACTAMENTE en esa ventana, sin quedar ni en la foto (ya tomada) ni en el canal (todavía sin registrar). Fix: registrar el sender Y sacar la foto bajo el MISMO candado (`Db::subscribers`) que usa `deliver_local` para entregar -- un duplicado ocasional es aceptable, una fila perdida no.
2. **Deadlock latente: si la entrega de eventos diferidos de `commit_transaction` corriera DENTRO de `with_exclusive_connection`** (como en el primer intento de esta ronda), un `transaction{}` confirmando (candado de conexión tomado, pidiendo el de `subscribers` para entregar) y un `subscribe()` concurrente a la MISMA colección (candado de `subscribers` tomado por el fix del punto 1, pidiendo el de conexión para `select_rows`) se esperarían mutuamente para siempre -- orden de candados cruzado. Fix: `commit_transaction` ahora DEVUELVE la lista de eventos pendientes en vez de entregarlos, y `Expr::Transaction` (`runtime/mod.rs`) los entrega DESPUÉS de soltar el candado de la conexión.

Ver §3.16 para el detalle completo de los dos bugs y su fix.

**Verificado**: 2 tests unitarios con hilos de sistema operativo reales de la primera ronda (`runtime/mod.rs`, 40 inserts concurrentes sin pérdida ni duplicado de id; 40 transacciones concurrentes sobre la misma fila dando exactamente 40) + 2 tests de compilación (`Db: Send + Sync`, `store.rs`; los tipos de conexión de `rusqlite`/`postgres` siguen siendo `Send`, invariante de la que depende toda esta arquitectura) + 2 tests MÁS de hilos reales de la segunda ronda (`subscribing_concurrently_with_a_real_insert_never_loses_the_new_row`, 300 vueltas con `std::sync::Barrier` forzando la carrera -- falla de forma reproducible con el orden viejo, confirmado revirtiendo el fix a mano antes de restaurarlo; `a_transaction_committing_concurrently_with_a_subscribe_on_the_same_collection_never_deadlocks`, 100 vueltas -- con la entrega adentro del candado de conexión el test literalmente SE CUELGA, confirmado matando a mano el proceso colgado tras 30s) + la suite completa (1215 tests, incluida integración contra Postgres real) sin ninguna regresión.

### 3.159 `@cron("Ns"/"Nm"/"Nh"/"Nd")`: tareas recurrentes nativas dentro de `linkc serve` — RESUELTO

**Origen**: PLAN.md §9.7 ítem 4, reprorizado el 24/08/2026 por evidencia fuerte de Glowapp (no usa c-script, pero es la señal de demanda) -- 10+ schedulers hand-rolled con `setInterval` (`appointmentReminderScheduler.ts`, `abandonedCartScheduler.ts`, `automationScheduler.ts`, `enrichmentScheduler.ts`, `goalsRecalculator.ts`, etc.), más un `schedulerSupervisor.ts` completo (registro de jobs, guard contra solapamiento, arranque escalonado, hasta un workaround para el límite de 32 bits de `setInterval` en intervalos largos). Atacado recién ahora porque necesitaba, sin saberlo hasta escribirlo, la infraestructura de hilos reales de §3.158 -- antes de esa ronda, correr una tarea de fondo real hubiera significado inventar concurrencia de un solo uso solo para esto.

**Sintaxis: una anotación sobre un `rpc` normal, reusando la gramática de anotaciones que ya existe -- ninguna palabra reservada ni bloque de nivel superior nuevo.**

<!-- linkc:fragment -->
```rust
service Jobs {
  @cron("5m")
  rpc sendReminders() -> Void {
    let due = db.appointments.findWhere(|a: Appointment| { a.remindedAt == null })
    // ... enviar recordatorios, marcar a.remindedAt = now() ...
  }
}
```

`"Ns"`/`"Nm"`/`"Nh"`/`"Nd"` -- mismo formato que `@cache`/`--session-ttl` (parser propio, `cron::parse_interval`, reimplementado a propósito en vez de compartir código con `cache::parse_ttl`: mismo criterio que el resto de estos parsers chicos del proyecto, ver el comentario de `cache.rs`). El servidor duerme el intervalo COMPLETO antes de la primera corrida (mismo criterio que `setInterval` de JS) -- así arrancar `serve`/`serve-all` con varias tareas no las dispara todas a la vez contra la base en el instante 0.

**`@cron` tiene que ser la ÚNICA anotación del rpc -- a diferencia del resto, que se combinan libremente.** Ninguna otra (`@route`/`@authenticated`/`@requires`/`@rate_limit`/`@cache`/`@idempotent`/`@cors`/`@cache_control`/etc.) tiene ningún efecto sobre algo que nunca recibe una request HTTP real -- en vez de dejarlas ahí sin efecto (la clase de confusión silenciosa que este proyecto evita sistemáticamente, GRAMMAR.md §3.9), el checker las rechaza de entrada, nombrando la combinación. Rechazado también sobre un `stream` (una tarea programada no es una conexión SSE que alguien suscribe). Sin parámetros -- nada externo dispara una corrida, así que no hay de dónde sacar sus argumentos -- y retorno obligatoriamente `Void` -- no hay ningún caller que reciba una respuesta.

**Nunca alcanzable vía HTTP, ni siquiera en su path por defecto.** El checker ya garantiza que `@cron` nunca coexiste con `@route`, pero eso solo bloquea una ruta amigable explícita -- el path `POST /{Service}/{rpc}` de siempre encuentra cualquier rpc por NOMBRE sin mirar sus anotaciones. `runtime::server::handle_request` chequea `is_cron_member` ANTES de cualquier otro procesamiento (antes de `@rate_limit`, antes del gate de auth) y devuelve 404 -- "no existe ese rpc", no 403: desde afuera, este endpoint genuinamente no existe. Tampoco aparece en ningún artefacto generado (`contract.d.ts`/`client.ts`/`hooks.ts`/`schemas.ts`/`openapi.json`/`llms.txt`/`llms-full.txt`) -- tanto la superficie generada como la superficie servida coinciden en que esto no es parte de la API pública.

**Ejecución: un hilo del sistema operativo dedicado POR tarea, spawneado una sola vez al arrancar `serve()`** -- reusa exactamente la infraestructura de §3.158 (`Arc<Db>`/`Arc<Program>`/`Arc<SessionStore>` clonados baratos hacia el hilo), sin ningún mecanismo nuevo de scheduling. Un error del cuerpo (`panic`, una violación de `@check`/`@unique`, lo que sea) se loguea (ver abajo) y el loop SIGUE -- una corrida fallida nunca apaga la tarea entera ni el servidor. Bajo `serve-all` (§3.92), cada servicio con tareas `@cron` las corre de forma independiente, sin relación entre sí.

**Observabilidad: una línea de log por corrida + dos contadores nuevos en `/metrics`.** `log_cron_tick` (mismo formato text/JSON que `log_done`, GRAMMAR.md §3.122, pero sin `req_id`/`status` HTTP -- una tarea programada no es una request) imprime `method`/`ok`/`duration_ms` en cada corrida; `ok=false` cuenta como `Error` para `--log-level`, mismo criterio que un 5xx. `linkc_cron_runs_total{method="..."}`/`linkc_cron_failures_total{method="..."}` (GRAMMAR.md §3.149) -- el segundo solo aparece para un rpc que de verdad tuvo al menos una falla, mismo criterio "nunca inventar un 0" que `linkc_rate_limit_rejections_total`.

**Límites honestos, alcance de esta ronda:**
- **Sin coordinación entre instancias.** Bajo N réplicas de `linkc serve` contra la misma base (detrás de un balanceador, GRAMMAR.md §3.92), CADA instancia corre su propia copia de la tarea de forma independiente -- una tarea pensada para correr "una vez cada 5 minutos" corre en realidad N veces cada 5 minutos, una por réplica. Mismo tipo de límite ya documentado para `@rate_limit` (GRAMMAR.md §3.39) y `IdempotencyStore`/`CacheStore` -- estado en memoria de UN proceso, sin coordinación distribuida. Resolverlo de verdad necesitaría un lock distribuido (una fila en la base con `SELECT ... FOR UPDATE`, o Redis) -- fuera de alcance sin evidencia real de demanda todavía; el cuerpo del rpc puede implementar su propia idempotencia si el caso real lo necesita (ej. un `@unique` que rechace un duplicado).
- **Sin catch-up.** Si el proceso está caído cuando "debería" haber corrido una tarea, esa corrida simplemente no pasa -- nunca se recupera al reiniciar. Mismo criterio de "aceptado a propósito" que el resto del estado en memoria de este proyecto.
- **Sin disparo manual ni introspección de "próxima corrida".** No hay forma de forzar una corrida fuera de horario ni de preguntarle al proceso cuándo va a correr de nuevo -- para eso, todavía hay que mirar los logs o `/metrics`.
- **Sin límite de concurrencia entre corridas de la MISMA tarea.** Si una corrida tarda más que su propio intervalo (una tarea de `"10s"` cuyo cuerpo tarda 30s), la próxima arranca igual, sin esperar a que la anterior termine ni saltear la vuelta -- dos ejecuciones de la misma tarea pueden solaparse en el tiempo. El `schedulerSupervisor.ts` de Glowapp que motivó este ítem SÍ tenía guard contra esto -- no se replicó acá por falta de evidencia de que el caso real (una tarea más lenta que su propio intervalo) haya ocurrido de verdad; queda para una ronda dedicada si aparece.

**Verificado**: 9 tests de checker (formato inválido, declarado dos veces, rechazado en un `stream`, rechaza combinar con `@rate_limit`, rechaza parámetros, rechaza retorno no-`Void`, camino feliz) + 2 tests unitarios de `cron::parse_interval` + 2 tests de integración contra un `linkc serve` REAL (subproceso real, `tests/server_http.rs`): una tarea `@cron("1s")` corre sola sin ningún request HTTP que la dispare (al menos 2 corridas confirmadas en 2.5s reales) y esa misma tarea da 404 al intentar invocarla por su path por defecto -- más 1 test de integración de `/metrics` (`tests/cli_metrics.rs`) confirmando el contador real de corridas y que el contador de fallas está ausente cuando ninguna corrida falló.

### 3.160 `http.postWithRetry(url, body, headers, maxAttempts)`: reintentos con backoff para webhooks salientes — RESUELTO

**Origen**: PLAN.md §9.4 ítem 2 ("webhooks salientes declarativos: registrar una URL de terceros y que el runtime reintente/firme automáticamente"). Auditado antes de escribir código, mismo criterio que el resto de esta sesión: la mitad de FIRMAR un webhook saliente ya funciona hoy sin ningún primitivo nuevo -- `crypto.hmacSha256` (§3.38) + `http.postWithHeaders` (§3.47) alcanzan para calcular una firma y mandarla como header, exactamente el patrón que la auditoría de proveedores de §3.112 ya documentó como funcionando para Stripe/SendGrid/etc. El gap real, el único que GRAMMAR.md §3.86 ya dejaba anotado como pendiente ("Reintentos... sigue sin resolver"), es que ninguna llamada `http.*` reintenta sola ante una falla transitoria -- hoy eso es responsabilidad manual de quien escribe el `.link`.

**`http.postWithRetry(url: String, body: String, headers: {name: String, value: String}[], maxAttempts: Int) -> String`** -- mismo criterio de "falla" que `post`/`postWithHeaders` ya usan (cualquier error de red O un status no-2xx es una falla; a diferencia de `postWithStatus`, esto nunca expone el status como dato, porque el punto es reintentar hasta lograr un 2xx, no inspeccionar el código). `maxAttempts` es el único parámetro nuevo -- un `Int`, no una duración ni una política -- porque es lo único que de verdad varía caso a caso: cuánto tolera cada llamada puntual antes de rendirse (un webhook de cobro vs. una notificación de baja prioridad). `maxAttempts <= 0` es un error de runtime limpio, nombrando el valor recibido, antes de mandar ninguna request real.

**Backoff FIJO, no configurable -- mismo criterio que `MAX_WHILE_ITERATIONS` (§3.15): un backstop razonable, no un sistema fino de política de reintentos por llamada.** `cron::parse_interval`/`--restart-backoff` (§3.92) sí exponen una duración configurable porque operan a la escala de "reiniciar un proceso servidor" -- acá la escala es "un reintento HTTP dentro de una sola request", donde una ventana de configuración extra no aporta nada real. Dobla desde 200ms (200ms, 400ms, 800ms, ...), techo de 5s -- deliberadamente mucho más corto que el techo de 30s de `--restart-backoff`, porque esto bloquea el hilo de ESA request (§3.158), nunca reintenta un proceso entero. El primer intento nunca espera.

**Límites honestos, alcance de esta ronda:**
- **No distingue 4xx de 5xx.** Un 400 (error del propio caller, nunca se va a arreglar solo) se reintenta exactamente igual que un 503 (falla transitoria real) -- `maxAttempts` intentos se gastan igual contra un error que reintentar no puede arreglar. Mismo criterio de "simple, sin evidencia de que la distinción haga falta todavía" que el resto de esta ronda; quien necesite ese comportamiento más fino ya puede construirlo a mano combinando `postWithStatus` (§3.60) con su propio loop.
- **No firma automáticamente.** A diferencia del pedido original ("reintente/firme automáticamente"), la firma sigue siendo responsabilidad de quien arma `headers` -- `crypto.hmacSha256(body, secret)` en un header más, mismo patrón ya documentado. No se agregó un parámetro `secret` dedicado porque distintos proveedores firman de formas incompatibles entre sí (header propio, algoritmo, qué se firma exactamente) -- `headers` genérico ya cubre cualquiera de ellos sin que el lenguaje tenga que conocer el esquema de ninguno.
- **Sin registro declarativo de URLs.** La mitad original "registrar una URL de terceros" (una tabla de webhooks configurada aparte del código, en vez de una llamada explícita en el cuerpo de un rpc) no se atacó -- sin evidencia real de que la forma imperativa (llamar a `http.postWithRetry` donde corresponda, ya sea directo o desde una tarea `@cron`, §3.159) sea insuficiente para el caso real.

**Verificado**: 3 tests de integración contra un servidor de mentira REAL (`tests/cli_http.rs`, subproceso real de `linkc serve`) -- dos fallas transitorias (500) con presupuesto de 3 intentos terminan en éxito; un 500 persistente con presupuesto de 2 intentos falla limpio (nunca cuelga, nunca reintenta de más); `maxAttempts=0` falla ANTES de mandar ninguna request real, confirmado con el servidor de mentira nunca recibiendo nada -- más 1 test unitario de `http_retry_backoff` confirmando la progresión exacta (200ms/400ms/800ms/1.6s/3.2s/techo de 5s) y que un `maxAttempts` enorme nunca desborda el cálculo del shift.

### 3.161 `import "./modulo.link";`: import "solo por efecto" — RESUELTO, cierra el último hueco real para partir un programa en módulos

**Origen: un discovery que corrigió el propio PLAN.md antes de escribir una línea de código.** PLAN.md §9.2 listaba "Pilar 3, sistema de módulos/paquetes" como pendiente de "su propio discovery (import por archivo vs. paquete versionado, cómo conviven dos `db {}` de archivos distintos)", con evidencia real de demanda (`main.link` de myfinance, 2058 líneas en un solo archivo). Auditar el código antes de diseñar nada mostró que esa descripción estaba **desactualizada**: el sistema de módulos ya existía y era mucho más completo de lo que el plan reflejaba -- imports multi-archivo por ruta relativa, `link.json` con dependencias nombradas, dependencias `git+<url>#<rev>` reales con checkout cacheado, `link.lock` con detección de deriva por SHA-256, detección de ciclos, caso diamante deduplicado, y rechazo de nombres duplicados entre archivos (todo §2.1). Las dos preguntas que el plan daba por abiertas ya tenían respuesta en el código: "import por archivo vs. paquete versionado" está resuelto (las dos formas existen), y "cómo conviven dos `db {}`" también (no conviven: es un error duro, ver "Límites honestos" abajo).

**El único hueco REAL, encontrado corriendo el binario y no leyéndolo: un módulo no podía aportar un `service`.** `service` no es importable por nombre a propósito (§2.1: no se referencia por nombre en ningún lado del lenguaje), y no existía ninguna forma de import sin nombres -- la gramática exigía al menos un identificador entre llaves, e `import {} from "./x.link";` era un error de parser. Consecuencia medida, no supuesta: para componer un programa a partir de módulos que aportan servicios, había que declarar en cada módulo un **tipo-fantasma** sin ningún uso real (`type BillingModule = { loaded: Bool }`) solo para tener algo que importar. Y ese fantasma **se filtraba al contrato público generado** -- confirmado inspeccionando el `gen/` real: aparecía como `export interface BillingModule` en `contract.d.ts` Y como `BillingModuleSchema` de Zod en `schemas.ts`. Es decir: la única forma de modularizar un programa ensuciaba exactamente el artefacto que es la tesis entera del proyecto ("el contrato es el código").

<!-- linkc:fragment -->
```rust
import "./billing.link";
import "./crm.link";
```

Sin llaves y sin `from` -- misma forma que TypeScript/JS usan para lo mismo, por la misma razón. Carga el módulo por lo que APORTA al programa fusionado, no por un nombre puntual que este archivo vaya a usar. La forma con llaves no cambia en nada (es puramente aditivo): las dos conviven en el mismo archivo sin ambigüedad, porque el parser decide por el token que sigue a `import` (un `Str` es la forma por efecto, un `{` la nombrada).

**Todo lo demás del sistema de módulos aplica igual, sin excepciones**: la resolución de `from` (relativo `./`/`../`, nombre pelado vía `link.json`, o `git+<url>#<rev>`) es la MISMA función; los ciclos se siguen detectando (la detección corre sobre la pila de ARCHIVOS, nunca sobre los nombres importados); un archivo inexistente falla igual de claro; un error de sintaxis en el módulo importado sigue nombrando el archivo y la línea. Lo único que una lista de nombres vacía saltea es, por construcción, la validación "¿existe este nombre ahí?" -- no hay ningún nombre que validar.

**Límites honestos, y qué queda REALMENTE abierto del Pilar 3:**
- ~~**Un solo `db { ... }` por programa, sigue siendo un error duro.**~~ **RESUELTO (27/08/2026), ver §3.172.** Cada módulo ya puede ser dueño de sus propias colecciones -- se fusionan en un solo namespace, con el único error duro que queda siendo un nombre de colección repetido (de paso, el gotcha de UX de la cascada de abajo también desapareció para el caso legítimo).
- **Sin visibilidad `pub`/privado** -- sin cambios respecto de §2.1: el `Program` fusionado sigue siendo la unión plana de todos los ítems nativos alcanzados, y un import no oculta nada de los demás archivos del cierre.
- **Sin re-exports** -- sin cambios. La forma por efecto no los introduce por la puerta de atrás: no exporta nada, solo carga.

**Verificado**: 4 tests unitarios nuevos en `modules.rs` -- un módulo que SOLO declara un `service` (sin nada importable) se carga y su service llega al `Program` fusionado junto con el `db {}` transitivo; las dos formas de import conviven en un mismo archivo aportando sus ítems; un archivo inexistente importado por efecto sigue fallando claro; un ciclo formado solo con imports por efecto se sigue detectando. Más verificación manual contra el binario real: el proyecto multi-módulo completo (un `schema.link` central + dos módulos de servicio) compila y genera un `contract.d.ts` con EXACTAMENTE los dos tipos de dominio y los dos clientes de servicio, **sin ningún tipo-fantasma** -- comparado lado a lado contra el mismo proyecto escrito con el workaround anterior, que sí los tenía. Cuatro formas malformadas (`import "a" "b";`, sin `;`, `import from "x";`, `import "a" from "b";`) confirmadas como errores de parser limpios.

### 3.162 Segunda auditoría adversarial: 3 bugs reales, dos de ellos creados por los fixes de la ronda anterior — RESUELTOS

**Origen: una segunda auditoría adversarial, pedida explícitamente por el usuario, con un agente READ-ONLY.** Detalle de proceso que importa: la ronda anterior había usado un agente `fork` para investigar y ese agente excedió su mandato (se le pidió solo investigar, implementó una feature completa). Esta vez se usó un agente `Explore`, que **estructuralmente no puede editar archivos** -- la restricción correcta para un auditor, y la lección aplicada. Los 3 hallazgos se reprodujeron a mano contra el binario real ANTES de tocar una línea; ninguno se dio por cierto por venir de un agente.

**El resultado incómodo y honesto: dos de los tres bugs los introdujeron los propios fixes de §3.158/§3.162 de la ronda anterior.** Tocar concurrencia tiene ese precio, y la disciplina de auditar lo recién shippeado es lo que los encontró antes que un adoptador.

**1. Deadlock: el servidor queda VIVO pero permanentemente colgado (introducido en v1.115.0).** §3.16 documentaba el orden de candados "subscribers→conexión" como invariante, y §3.154/§3.158 hacían que `transaction{}` entregara sus eventos diferidos DESPUÉS de soltar la conexión precisamente para respetarlo. Pero la MISMA ronda envolvió `upsert` entero en `Db::with_exclusive_connection` (para cerrar una carrera de fila duplicada) -- y el `insert`/`applyPatch` de adentro llega a `publish`→`deliver_local`, que pide `subscribers`, **con la conexión ya tomada**: el orden inverso. ABBA clásico.

Reproducido contra un `linkc serve` real (una colección de 4000 filas, 8 clientes martillando `upsert` y 8 abriendo un `stream` sobre la misma colección, en simultáneo): tras unos segundos el proceso queda vivo, `ping` (cómputo puro, sin candados) sigue respondiendo 200, y `health`/`/metrics`/cualquier escritura **no vuelven nunca**. Solo se recupera matando el proceso.

**Fix -- más simple que lo que había, no más complejo:** `subscribe()` registra al suscriptor PRIMERO, suelta el candado, y recién después saca la foto. Nunca sostiene los dos candados. Y sigue cumpliendo lo que el fix de v1.115.0 buscaba: si una escritura ocurre entre el registro y la foto, el suscriptor la recibe como EVENTO (ya está registrado) y quizá además la vea en la foto -- un duplicado ocasional, inofensivo (un consumidor de `stream` ya trata cada evento como el estado ACTUAL de esa fila, nunca como un delta). Lo que no puede pasar es que no aparezca en ninguna de las dos, que era el bug original. El fix de v1.115.0 había sobre-corregido: alcanzaba con reordenar, no hacía falta sostener el candado.

**2. `@cron` rompía el TypeScript generado (introducido en v1.116.0).** De los SEIS lugares que recorren los miembros de un `service` para emitir algo, `emit_service_interface` (`ts_emit.rs`) era el único que se había quedado sin el filtro de `@cron` -- los otros cinco (`emit_client`, `emit_hooks`, `openapi_emit`, y los dos de `llms_txt_emit`) sí lo tenían. Causa concreta del olvido: la edición que agregó el filtro usó un patrón sensible a la indentación, y ese sitio tiene otra -- se aplicó a dos de tres sin aviso. Efecto: `export interface JobsClient` declaraba `sweep(...)`, pero `class JobsClientImpl implements JobsClient` nunca lo define → **TS2420: "Class 'JobsClientImpl' incorrectly implements interface 'JobsClient'"**, confirmado con el `tsc` real del propio repo. `linkc build` reportaba `OK`. Exactamente la clase de desacuerdo entre capas que §3.9 existe para prevenir, y en el artefacto que es la tesis del proyecto. **Fix**: el brazo que faltaba, más una verificación sistemática de los seis sitios (los dos del checker correctamente NO filtran -- el checker sí debe tipar un `@cron`).

**3. División entera por cero era un PANIC de Rust (preexistente, pero mucho peor desde §3.158).** `a / 0` y `i64::MIN / -1` sobre `Int`/`Int64` panicaban en vez de dar un error de runtime, y el divisor casi siempre viene de datos del usuario -- trivialmente alcanzable. Antes de un hilo por request, un panic mataba el PROCESO (ruidoso, pero al menos consistente). Con hilos, mata solo ese hilo **sin pasar por ningún camino de limpieza** -- y adentro de un `transaction { }` deja el `BEGIN` abierto sobre la conexión compartida y `transaction_pending_publishes` en `Some(...)` para siempre.

Reproducido de punta a punta, con **pérdida silenciosa de datos ya confirmados al cliente**:

| paso | antes del fix |
|---|---|
| `goodTx("a")` | `1` -- commiteado |
| `boomTx(0)` | 500 (panic) |
| `count` | `2` -- la fila NO commiteada es visible (misma conexión, transacción abierta) |
| `goodTx("b")` | error: *"ya hay una transacción abierta"* -- **toda transacción futura del proceso falla para siempre** |
| `plainInsert("c")` | `{"id":3,...}` -- **200 OK devuelto al cliente** |
| `count` | `3` |
| reiniciar proceso, `count` | **`1`** |

Dos escrituras que el servidor confirmó como exitosas se descartaron en silencio. **Fix**: `/` y `%` sobre enteros pasan por `checked_div`/`checked_rem` (que cubren divisor cero Y el desborde de `i64::MIN / -1`) y devuelven un `RuntimeError` normal. El camino de `Float` queda sin guarda a propósito: IEEE-754 ya define `/0` como infinito/NaN, nunca panica. Tras el fix, el mismo escenario da un 500 limpio, la transacción rollbackea, las siguientes funcionan, y las 3 filas sobreviven al reinicio.

**Límite honesto que NO se cerró en esta ronda:** el fix 3 elimina el disparador de panic más alcanzable, pero **no** el problema de fondo -- cualquier OTRO panic dentro de un `transaction { }` (un `.expect()` de `db.rs`/`store.rs`, por ejemplo) sigue dejando el mismo estado corrupto, porque no hay ningún `catch_unwind` alrededor del cuerpo. Cerrarlo de verdad necesita esa red de seguridad y su propio diseño (qué hacer con un `Value` que no es `UnwindSafe`), no entra en una ronda de bugfix. Igual que sigue abierto que una tarea `@cron` muere para siempre en su primer panic, sin log ni métrica, contradiciendo lo que §3.159 promete.

**Verificado**: 6 tests de regresión nuevos -- `an_upsert_publishing_concurrently_with_a_subscribe_on_the_same_collection_never_deadlocks` (100 vueltas con hilos reales y `Barrier`, hermano del que ya cubría `transaction{}`), los tres de división/resto por cero y desborde, y el de `transaction{}` que divide por cero (rollbackea Y deja la base usable para la siguiente transacción). Más verificación manual contra binarios reales de los tres: el martillo que colgaba el servidor ahora lo deja respondiendo 200 en todo; el `tsc` real del repo compila limpio un proyecto con `@cron`; y el escenario de pérdida de datos termina con las 3 filas intactas tras reiniciar.

### 3.163 `catch_unwind` alrededor del cuerpo de `transaction { }` — RESUELTO, cierra el primer límite honesto de §3.162

§3.162 dejó dicho, explícitamente, que su fix (3) solo tapaba el disparador de panic más alcanzable (`/`/`%` por cero) -- **cualquier OTRO panic** dentro de un `transaction { }` (un `.expect()` en `db.rs`/`store.rs`, un desborde de `+`/`-`/`*`, lo que sea) seguía dejando exactamente el mismo estado corrupto: el hilo de la request muere en el unwind sin pasar por `rollback_transaction`, así que `transaction_pending_publishes` se queda en `Some(...)` para siempre y la conexión SQL compartida se queda con un `BEGIN` sin `COMMIT` ni `ROLLBACK`. Consecuencia idéntica a la de §3.162: toda `transaction { }` POSTERIOR en el proceso falla para siempre con "ya hay una transacción abierta", y toda escritura no transaccional posterior corre sobre esa conexión corrupta y se pierde en silencio al reiniciar.

**Fix: `eval_block` del cuerpo va envuelto en `std::panic::catch_unwind`, adentro de `Expr::Transaction` (`runtime/mod.rs`).** Un panic atrapado se traduce a un `RuntimeError` normal ("la transacción abortó por un error interno inesperado: {mensaje}") y toma el mismo camino que cualquier otro error del cuerpo: `rollback_transaction()`, que limpia `transaction_pending_publishes` y emite el `ROLLBACK` real. El mensaje del panic se extrae con un helper chico (`panic_payload_message`, compartido con el fix de §3.164) que sabe leer los dos payloads que casi siempre trae un panic de Rust (`&str` de un `panic!("...")` literal, `String` de uno formateado) y cae a un texto genérico para cualquier otro tipo.

<!-- linkc:fragment -->
```link
type Item = { id: Int, name: String }
db { items: Item[] }
service S {
    rpc riesgoso(a: Int64, b: Int64) -> Void {
        transaction {
            db.items.insert(Item { id: 0, name: "en vuelo" });
            // si esto panicara -- overflow, un bug en una fn nativa, lo
            // que sea -- ya no deja el BEGIN abierto para siempre: hace
            // ROLLBACK como cualquier otro error del cuerpo.
            let x = a + b;
        }
    }
}
```

**Por qué `AssertUnwindSafe` es seguro acá y no un escape hatch descuidado.** El compilador no puede probar `UnwindSafe` para el closure -- `env` es un `HashMap<String, Rc<RefCell<Value>>>` (§3.10) y ni `Rc` ni `RefCell` lo son en general, porque en el caso genérico un panic a mitad de mutación puede dejar datos a medio escribir visibles después. Lo que hace esto seguro específicamente ACÁ no es una promesa del type system: es que el ÚNICO estado que le importa a la ejecución SIGUIENTE (`transaction_pending_publishes`, el `BEGIN` de la conexión) se limpia explícitamente en el brazo `Err` con el mismo `rollback_transaction()` que ya corría para un `RuntimeError` normal. El `env`/las filas a medio insertar de ESTA transacción abortada no importan -- nunca se confirman (`ROLLBACK` real), y la petición entera ya terminó en error.

**Límite que se mantiene, a propósito: un panic en un `rpc` que NO usa `transaction { }` sigue matando solo ese hilo, sin ninguna limpieza especial más allá de lo que `parking_lot` ya hace al soltar sus candados en el unwind.** Fuera de una transacción no hay ningún `BEGIN`/estado pendiente que limpiar -- el caller HTTP recibe una conexión cortada (§3.158 documenta ese caso), no una respuesta, y el servidor sigue vivo para la próxima request. Trazar ESE panic con una línea de log propia (en vez de depender de lo que imprima el hook default de Rust a stderr) queda fuera de esta ronda -- es el "panics 500 no trazables" que ya estaba anotado como hallazgo menor de la auditoría de §3.162, deliberadamente no perseguido acá.

**Verificado**: `a_transaction_whose_body_panics_from_something_other_than_division_by_zero_also_rolls_back_and_leaves_the_db_usable` (`runtime/mod.rs`) -- dispara un desborde real de `+` sobre `i64` (código de producción sin arreglar a propósito, para probar el `catch_unwind` contra un panic GENÉRICO en vez de reproducir el caso puntual que §3.162 ya había cerrado con su propio `RuntimeError`), confirma que la fila del cuerpo se rollbackea y que una transacción POSTERIOR sigue funcionando. Gateado con `#[cfg(debug_assertions)]`: el desborde de `+` solo panica con `overflow-checks` activo, que Cargo prende por defecto en `dev` (lo que corre `cargo test`/CI) y apaga en `release` (donde `a + b` wrappea sin panicar) -- confirmado corriendo el mismo test bajo `cargo test --release`: no panicó, así que no había nada que atrapar y el test quedaría sin señal real.

### 3.164 `catch_unwind` alrededor de cada corrida de `@cron` — RESUELTO, cierra el segundo límite honesto de §3.162

El comentario que introduce el scheduler de `@cron` en `runtime/server.rs` (desde §3.159) siempre dijo "un error del cuerpo (panic, `@check`/`@unique`, lo que sea) se loguea y el loop SIGUE" -- pero eso era falso para el caso panic específicamente. `invoke_rpc_with_sessions` corre dentro de un `match Ok/Err` normal, y un panic real NO es un `Err`: atraviesa el `match` sin tocarlo y sigue desenrollando. `std::thread::spawn` no trae ningún `catch_unwind` propio, así que el unwind se lleva puesto TODO el hilo del scheduler -- el `loop` nunca vuelve a `std::thread::sleep`, y la tarea deja de correr **para siempre**, sin ninguna línea de log ni entrada de métrica que lo marque. Indistinguible, desde afuera, de "todavía no le tocaba el turno" -- el peor tipo de falla silenciosa para algo que por definición nadie está mirando activamente.

**Fix: la llamada a `invoke_rpc_with_sessions` va envuelta en `catch_unwind`, mismo `AssertUnwindSafe` y mismo `panic_payload_message` que §3.163.** El resultado se matchea a tres casos en vez de dos -- `Ok(Ok(_))` (corrida exitosa), `Ok(Err(e))` (un `RuntimeError` normal, como siempre), y el caso nuevo `Err(payload)` (panic real): los tres registran `metrics_store.record_cron_run` y loguean vía `log_cron_tick` -- el panic cuenta como falla, con el mensaje extraído en vez de perderse. El `loop` externo nunca se entera de que hubo un panic adentro; sigue durmiendo y reintentando en el próximo tick, exactamente la garantía que el comentario original prometía.

**Verificado con el binario real, no solo con el runtime en aislamiento** (`metrics_reports_a_cron_run_that_panics_as_a_failure_and_the_task_keeps_running`, `tests/cli_metrics.rs`): un `@cron("1s")` cuyo cuerpo SIEMPRE desborda `+` sobre `i64` (mismo disparador que §3.163, mismo gateo `#[cfg(debug_assertions)]` y mismo motivo -- en `release` el binario `linkc` real que este test lanza como subproceso tampoco panicaría). Se mide `linkc_cron_failures_total{method="Jobs.tick"}` en dos instantes separados por 1.5s y se confirma que **sigue creciendo** -- la señal de que el hilo del scheduler sobrevivió al primer panic y siguió despertando. Antes del fix, ese contador se hubiera quedado clavado en su primer valor (o en cero, si el panic pegó antes de la primera corrida completa) para siempre. `linkc_cron_runs_total` (que solo cuenta corridas OK, §3.149) se confirma en `0` -- ningún panic pudo colarse como éxito.

**Con esto se cierran los dos límites que §3.162 había dejado explícitamente abiertos** ("Límite honesto que NO se cerró en esta ronda", más arriba) -- ningún panic dentro de `transaction { }` ni dentro de `@cron` queda ya sin un camino de limpieza.

### 3.165 Tercera auditoría adversarial (27/08/2026): 2 bugs críticos — RESUELTOS

**Origen: una tercera auditoría adversarial, pedida explícitamente por el usuario ("haz una auditoría... añade cualquier fallo a un markdown"), con 5 agentes `Explore` (read-only) en paralelo, uno por capa (concurrencia/panics, consistencia de codegen, auth/secretos, superficie de `.unwrap()`/panic, capa SQL/DB).** Los 16 hallazgos quedan documentados en `AUDIT-2026-08-27.md` (fuera de este documento, a propósito -- es el reporte de auditoría completo, con severidad y estado de verificación por ítem) y priorizados en `AUDIT-FIX-PLAN-2026-08-27.md`. Los dos de severidad **crítica** se cierran en esta ronda; el resto queda en el plan para rondas siguientes.

**1. `crypto.randomToken(length)` con `length` negativo o gigante mataba el proceso `linkc serve` ENTERO.** `*n as usize` sobre un `i64` negativo reinterpreta los bits como un `usize` gigante (~1.8×10¹⁹); `length.max(8)` ponía un piso pero nunca un techo. `os_random_bytes` hace `vec![0u8; n]` con ese valor -- para un negativo, el propio macro `vec!` detecta que el pedido excede `isize::MAX` y panica ("capacity overflow", panic normal, mata solo el hilo); para uno grande pero por DEBAJO de ese límite (`i64::MAX` mismo, ~4.6×10¹⁸ bytes tras dividir por 2), el pedido sí llega al allocator real del sistema operativo, que no tiene esa memoria -- Rust llama a `handle_alloc_error`, que hace `std::process::abort()` **sin que `catch_unwind` pueda hacer nada, tire el hilo que tire**.

Reproducido en vivo, confirmando los dos escalones: `{"length": -1}` da el panic catcheable (proceso sigue vivo, `/health` responde `200` después); `{"length": 9223372036854775807}` mata el proceso de verdad -- el log real dice `memory allocation of 4611686018427387904 bytes failed`, y el puerto deja de aparecer como `LISTENING`. Bajo `serve-all` (§3.92), se lleva puesto **todos** los servicios coexistiendo en ese proceso, no solo el que declaró el rpc vulnerable -- y no hace falta ninguna autenticación si el rpc que expone `crypto.randomToken(length)` no la exige.

**Fix**: `length` se valida contra el rango `1..=1024` (generoso a propósito -- ningún token real necesita ni una fracción de eso) ANTES de convertir a `usize` y de tocar memoria, mismo criterio que `crypto.randomInt`/`dateFromParts` ya usan para sus propios límites -- un `RuntimeError` limpio en vez de un pedido de memoria imposible.

**2. `@cache` + `@authenticated`/`@requires` filtraba datos de un usuario hacia otro.** La clave de caché es únicamente `(service, rpc, argumentos)` (`compiler/src/cache.rs`) -- nunca incluye el token de sesión, el `userId` ni el rol. El gate de auth corre ANTES del lookup de caché, así que alcanza con TENER una sesión válida, no la sesión correcta, para llegar al valor cacheado. Cualquier rpc `@authenticated`/`@requires` + `@cache` cuya respuesta dependa de `auth.currentUserId()`/`auth.currentRole()` (el patrón que GRAMMAR.md §3.53 documenta y promueve para "mis notas"/"mi dashboard") es vulnerable -- peor cuantos menos argumentos tenga el rpc, porque un rpc sin argumentos (el caso más común de "dame mis datos") tiene una sola clave de caché para TODOS los usuarios.

Reproducido en vivo: Alice se registra, llama `myProfile` (`@authenticated` + `@cache("30s")`, cuerpo `db.users.find(auth.currentUserId())`), su perfil completo (incluyendo un campo `secret`) queda cacheado. Bob, con su PROPIO token de sesión válido, llama el mismo rpc con el mismo cuerpo vacío `{}` -- y recibe el perfil de Alice, `secret` incluido. El cuerpo de Bob nunca corrió.

**Fix**: rechazado en compilación -- `@cache` combinado con `@authenticated`/`@requires` en el mismo rpc es ahora un error del checker, hasta que exista un diseño real de scoping por sesión (incluir el `userId`/token en la clave de caché es la mejora natural, pendiente de evidencia real de demanda antes de construirla apurado). Mismo criterio que el proyecto ya usa para otras combinaciones sin sentido (`@cron` + cualquier otra anotación, §3.159) -- negar la combinación insegura por completo, no un aviso que se puede ignorar.

**Verificado**: `random_token_rejects_a_negative_or_absurdly_large_length_instead_of_crashing` (`runtime/mod.rs`) + repetición en vivo de los dos escalones del repro contra un `linkc serve` real (confirmando que el proceso sobrevive al primero y que el segundo, antes del fix, lo mataba). `cache_annotation_is_rejected_when_combined_with_authenticated`/`_with_requires` (`checker.rs`) + repetición en vivo del repro Alice/Bob antes del fix, y confirmación de que el mismo programa ahora falla en `linkc build` con un mensaje claro.

**Límite que se mantiene, a propósito**: el resto de los 14 hallazgos de `AUDIT-2026-08-27.md` (severidad alta/media/baja) quedan en `AUDIT-FIX-PLAN-2026-08-27.md`, priorizados para rondas siguientes -- no todos entran en una sola ronda de bugfix.

### 3.166 `Patch<T>`/`applyPatch` ahora corre `@validate`/`@check` — RESUELTO, cierra el hallazgo #3 de AUDIT-2026-08-27.md

`json_to_typed_value` (`runtime/mod.rs`) es la función que decodifica el body de una request contra el tipo declarado. Su brazo `Type::Struct { fields, name }` (usado por cualquier rpc que reciba un struct COMPLETO como parámetro) llama a `apply_field_validators(...)` después de construir el valor -- el mecanismo real detrás de `@validate(email)`/`@validate(regex, ...)` y `@check(min/max/range/minLength/maxLength, ...)` (§3.73/§3.96/§3.146). El brazo `Type::PatchOf(inner)` -- el que decodifica el argumento `Patch<T>` de `applyPatch`, la forma CANÓNICA de hacer una actualización parcial (GRAMMAR.md §2.1) -- construía el struct campo por campo y nunca llamaba a esa función. Como `@validate` no tiene NINGÚN respaldo a nivel de DDL (`check_clause_sql` solo emite `CHECK` para los `FieldCheck` numéricos/de longitud, nunca para email/regex), la llamada de aplicación era el ÚNICO punto de enforcement en todo el sistema -- y era justo el que `Patch<T>` se saltaba.

Reproducido en vivo: `POST /Users/create {"email":"not-an-email",...}` daba `400` (`@validate(email)`); `POST /Users/update {"id":1,"patch":{"email":"not-an-email"}}` daba `200`, con el email inválido persistido tal cual. `@check` (numérico/longitud) solía quedar cubierto igual porque además tiene un `CHECK` real de DDL -- **excepto en modo `--adopt-existing`**, donde el propio código nunca ejecuta DDL (`db.rs`), así que ahí tampoco había ningún backstop.

**Fix**: `Type::PatchOf(inner)` ahora también llama a `apply_field_validators`, usando el `name` que `Type::Struct` ya expone para resolver la declaración `ast::Field` original (mismo mecanismo, mismo `field_annotations_for`). `apply_field_validators` ya toleraba un valor PARCIAL -- por diseño, solo valida las claves que de verdad están presentes en el `Value::Struct` recibido, así que un patch que omite un campo con `@validate`/`@check` sigue sin tocarlo, ninguna semántica nueva que aprender.

**Verificado**: `validate_fires_on_applypatch_via_patch_of_t_not_just_on_the_full_struct` (`runtime/mod.rs`) -- confirma que `create` y `update` rechazan el mismo valor inválido igual, que un patch que NO toca el campo con `@validate` sigue funcionando sin tocar el valor existente, y que un patch con un valor válido sigue aplicando normal. Más repetición en vivo del repro exacto contra un `linkc serve` real: `update` con el email inválido ahora da `400` con el mismo mensaje que `create`.

### 3.167 `@idempotent`: la carrera de doble ejecución concurrente — RESUELTO, cierra el hallazgo #4 de AUDIT-2026-08-27.md

`lookup()` (mirar si la clave ya corrió) y `store()` (grabar que corrió) eran dos adquisiciones de candado SEPARADAS sobre `IdempotencyStore`, con el cuerpo ENTERO del rpc corriendo sin ningún candado sostenido entre medio. Dos requests con la misma `Idempotency-Key` que llegan casi al mismo tiempo veían las dos un `Miss` (ninguna había grabado todavía) y las dos corrían el cuerpo -- con el modelo de un hilo real por request (§3.158), esto dejó de ser un caso teórico. Reproducido en vivo: 30 requests concurrentes `POST /Payments/charge` con la misma `Idempotency-Key` insertaron **2 filas** para un solo cargo lógico -- exactamente el escenario que la anotación existe para impedir (un cliente con timeout+retry disparando dos intentos casi simultáneos). GRAMMAR.md §3.140 solo documentaba y probaba el caso de reintento SECUENCIAL (esperar la primera respuesta antes de reintentar).

**Fix -- `reserve` reemplaza a `lookup`, atómico bajo un único candado.** `IdempotencyStore::reserve(service, rpc, key, hash)` revisa el store Y, si no hay nada útil ahí (o lo que había venció), marca la clave EN VUELO -- las dos cosas bajo la MISMA adquisición de candado, así que dos hilos concurrentes nunca pueden ver los dos `Reserved` para la misma clave. `Lookup` gana una cuarta variante, `InFlight`: el segundo (y cualquier otro) hilo que intenta reservar una clave que otro ya está corriendo recibe `InFlight` en vez de `Miss`, y `server.rs` responde `409` sin correr el cuerpo -- mismo criterio que la API real de Stripe, que documenta exactamente este 409 para una `Idempotency-Key` con una request en vuelo, en vez de dejar correr dos ejecuciones concurrentes. Quien gana la reserva llama a `complete` (éxito, reemplaza la marca "en vuelo" por el resultado real, mismo criterio de siempre: solo se graba un 2xx) o a `release` (error -- libera la clave YA, para que un reintento inmediato con la misma clave pueda correr de nuevo sin esperar ningún timeout).

**Detalle de robustez, a propósito**: una marca "en vuelo" que se queda huérfana (el hilo que la reservó murió sin llamar a `complete`/`release` -- un panic en el cuerpo, por ejemplo) se reclama sola después de `IN_FLIGHT_STALE_AFTER` (120s, generoso contra el default de `--http-timeout`) -- nunca bloquea una clave para siempre.

**Verificado**: 6 tests unitarios nuevos en `idempotency.rs` (incluyendo el caso central -- un segundo `reserve` sobre una clave todavía en vuelo da `InFlight`, nunca otro `Reserved`) + `idempotent_never_runs_the_body_twice_under_real_concurrent_requests_with_the_same_key` (`server_http.rs`, hilos de sistema operativo REALES contra un `linkc serve` real, 30 requests concurrentes) + repetición en vivo del repro exacto del audit con `curl` fuera del harness de test -- confirmando **1 sola fila** donde antes del fix daba 2. Los 3 tests end-to-end preexistentes de `@idempotent` (replay secuencial, sin clave, conflicto) siguen pasando sin cambios de comportamiento observable.

### 3.168 Ronda 3 de AUDIT-FIX-PLAN-2026-08-27.md: 6 bugs de severidad media — RESUELTOS

Cierra los hallazgos #5-#10 de `AUDIT-2026-08-27.md` (severidad media), el paquete completo en una sola ronda -- mismo criterio que v1.119.0.

**5. `insert()` panicaba (no daba `RuntimeError`) si la fila se borraba entre el INSERT y el SELECT de confirmación.** El INSERT y el SELECT que confirma la fila recién creada son dos llamadas independientes al backend (fuera de `transaction{}`/`upsert`, cada una toma y suelta su propio candado) -- una ventana real donde un `deleteWhere` concurrente cuyo predicado matchea la fila recién insertada (por un valor de campo por defecto, por ejemplo) puede borrarla antes del SELECT. `applyPatch`, unas líneas más abajo, reconsulta por id con la MISMA forma después de escribir y ya usaba `.ok_or_else(...)` (`RuntimeError` limpio) -- `insert` usaba `.expect(...)` (panic) para la carrera idéntica, una asimetría sin motivo. **Fix**: mismo `.ok_or_else(...)` que `applyPatch`.

**6. Agregaciones (`sumBy`/`countBy`/`avgBy`/`maxBy`/`minBy`) panicaban sobre una columna `NULL` heredada de una migración.** `scalar_cell_to_value` (el decodificador de `select_grouped`) no tenía brazo para `Cell::Null`, caía al `panic!` genérico. Cadena de alcanzabilidad confirmada leyendo el código: `alter_table_add_column_postgres` (`codegen/postgres_emit.rs`) nunca agrega `NOT NULL`, sin importar si el campo es requerido en el `.link` -- así que agregar un campo REQUERIDO a una colección Postgres con filas viejas deja esas filas con `NULL` físico, y cualquier `sumBy`/etc. agrupando o valorando por ese campo panica al llegar a una de esas filas. `row_to_fields` (lectura normal) ya tenía este mismo caso cubierto con un `RuntimeError` limpio ("null\_but\_required") -- ese fix nunca se había aplicado al camino de agregación. **Fix**: mismo `RuntimeError` en `scalar_cell_to_value` para `Cell::Null`.

**7. El checker aceptaba un rpc `@cron` como blanco válido de `@invalidates`.** `looks_like_a_query()` devuelve `true` para cualquier rpc con cero parámetros, y todo `@cron` tiene cero parámetros por construcción -- sin chequeo explícito, `@invalidates(unRpcConCron)` compilaba `OK` aunque `emit_hooks` (el emisor real de hooks de Query) ya excluía `@cron` de generar ningún hook -- una llamada de invalidación de caché muerta para siempre en `hooks.ts`, confirmada en vivo (`linkc build` daba `OK`, `hooks.ts` contenía `invalidateQueryCache(client, "Jobs.sweep")` sin que ningún hook escribiera jamás bajo ese prefijo). **Fix**: `check_invalidates_annotation` excluye explícitamente un blanco con `.cron().is_some()`, mismo criterio que los 6 sitios de codegen.

**8. `linkc doc` no mostraba badges de auth/rate-limit/deprecated en un `stream`.** El brazo `Member::Stream` de `render_service` (`doc.rs`) solo renderizaba el badge estático "📡 Realtime" y nunca miraba `st.auth()`/`st.rate_limit()`/`st.deprecated()`/`st.cors()` -- a diferencia del brazo `Member::Rpc`, que sí los calculaba. Ninguna de esas anotaciones está restringida a `rpc` (GRAMMAR.md §3.14: auth corre igual para `rpc` Y `stream`). Confirmado en vivo: un `stream` con `@requires(Role.Admin)`+`@rate_limit`+`@deprecated` generaba HTML mostrando solo "Realtime", sin candado ni advertencia -- documentación que desinforma sobre qué está protegido (el enforcement REAL en runtime no estaba afectado, solo el documento generado). **Fix**: `annotation_badges`, una función compartida entre los dos brazos, reemplaza las dos implementaciones independientes que existían antes -- la raíz del bug era duplicación, no solo el síntoma puntual.

**9. `GET /metrics` sostenía el candado de `metrics_store` mientras contendía por el candado de la conexión a la base.** Por orden de evaluación de Rust, el `MutexGuard` temporal de `metrics_store.lock()` seguía vivo mientras se evaluaban los argumentos de `render_prometheus_text`, incluyendo `db.size_bytes()` (que pide el candado de conexión compartido que `transaction{}`/`upsert` sostienen por toda su duración, §3.158) -- un `GET /metrics` que caía en medio de una transacción larga quedaba bloqueado sosteniendo `metrics_store`, y cualquier otro hilo que lo necesitara (el registro de un rechazo de `@rate_limit`, el registro final de cada request normal) quedaba en cola detrás, sin relación con `/metrics` en sí. No es un deadlock -- nadie sostiene la conexión y después espera `metrics_store` -- solo latencia/contención innecesaria. **Fix**: los tres valores (`subscriber_counts`/`size_bytes`/`oversized_notify_drop_counts`) se calculan ANTES de tomar el candado de `metrics_store`, que ahora solo se sostiene para el formateo (puro cómputo en memoria).

**10. `lint`: `mixed-service-auth` daba falso positivo cuando un servicio mezclaba rpcs protegidos con un job `@cron`.** El cálculo de `has_auth`/`has_unauth` recorría TODOS los miembros sin excluir `.cron()` -- como un rpc `@cron` nunca puede llevar `@authenticated`/`@requires` (única anotación que admite) ni es alcanzable vía HTTP, cualquier servicio con un job `@cron` y al menos un rpc protegido disparaba el lint, aunque todos los endpoints HTTP reales estuvieran protegidos de manera uniforme -- justo el patrón que `@cron` fue diseñado para soportar. Confirmado en vivo: dos rpcs `@authenticated` + un `@cron` disparaban la advertencia; el mismo programa, después del fix, pasa limpio. **Fix**: excluir `.cron().is_some()` de los dos cálculos, mismo criterio que codegen.

**Verificado**: un test unitario nuevo por hallazgo (`scalar_cell_to_value_rejects_null_with_a_clean_error_instead_of_panicking` en `db.rs`; `invalidates_rejects_a_cron_target` en `checker.rs`; `a_protected_rate_limited_deprecated_stream_shows_all_three_badges`/`an_unprotected_stream_shows_publico` en `doc.rs`, primera cobertura de tests que ese módulo tiene; `mixed_service_auth_does_not_fire_for_a_cron_job_next_to_protected_rpcs`/`mixed_service_auth_still_fires_for_a_genuinely_public_rpc_next_to_a_protected_one` en `lint.rs`, ídem) + repetición en vivo contra el binario real para los hallazgos #7/#8/#10 (repro exacto del audit, confirmando el error/HTML/lint limpio después del fix). El #5 no tiene repro trivial por timing (documentado así en el propio plan) -- verificado por lectura de código (simetría exacta con `applyPatch`) más el test unitario del camino feliz sin regresión. El #9 no tiene repro de un solo request (cambio de orden de evaluación) -- verificado con los 7 tests preexistentes de `/metrics` (`cli_metrics.rs`) pasando sin cambio de comportamiento observable.

### 3.169 Ronda 4 de AUDIT-FIX-PLAN-2026-08-27.md: los 6 hallazgos restantes — CERRADA (3 código, 3 documentación deliberada)

Última ronda del plan derivado de `AUDIT-2026-08-27.md` -- con esto, los 16 hallazgos de la tercera auditoría adversarial quedan todos resueltos o documentados explícitamente como límites conocidos, ninguno silenciado.

**13. `--jwt-secret ""` / `--service-api-key ""` (string vacío explícito por flag) activaba la feature con un secreto vacío.** `read_flag_or_env` (`main.rs`) ya filtraba un valor de ENV VAR vacío (`.filter(|v| !v.trim().is_empty())`), pero un valor vacío que llega por FLAG pasaba tal cual -- inconsistencia entre los dos caminos que deberían comportarse igual. **Fix**: el filtro se aplica puntualmente en `resolve_service_api_key`/`resolve_jwt_config`, NO en `read_flag_or_env` en sí -- otros flags (`--host`, por ejemplo) tienen un contrato deliberado de "un valor vacío es un error explícito" que un filtro global habría roto (confirmado leyendo el test existente `an_empty_host_flag_is_rejected_instead_of_silently_binding_everywhere`).

**14. Panics de tipo-incompatible al decodificar filas de una tabla adoptada (`--adopt-existing`) con datos que no calzan.** `row_to_fields` (`db.rs`) tenía tres sitios que asumían "esta fila fue escrita por este mismo programa, con esta misma forma" y usaban `panic!` si eso no se cumplía: (a) un valor JSON guardado por una versión ANTERIOR del `.link` que ya no satisface el tipo anidado actual (`json_to_typed_value` devolviendo `Err`, ignorado con `.unwrap_or_else(|e| panic!(...))`); (b) una columna declarada JSON cuya `Cell` física no es `Json`/`Null`; (c) una columna nativa (`Int`/`String`/etc.) cuya `Cell` física no coincide con el tipo declarado -- alcanzable de verdad porque `check_schema_for_adoption` valida el tipo DECLARADO de cada columna pero SQLite tiene afinidad de tipo, no enforcement (una columna `INTEGER` puede seguir aceptando `TEXT` si algo la escribió así por fuera de c-script). **Fix**: los tres devuelven el mismo `RuntimeError` limpio que el resto de la función ya usa para "el schema no coincide", nombrando la colección, la fila y el campo.

**16. `+`/`-`/`*` (y el `-` unario) sobre `Int`/`Int64` seguían con aritmética cruda sin `checked_*`.** §3.162 solo había cerrado `/`/`%` -- `numeric_op` seguía usando `a+b`/`a-b`/`a*b` directo, mismo riesgo real: en perfil `dev` (`cargo test`/CI) un desborde panica; en `release` (los binarios publicados) wrappea EN SILENCIO, un bug de CORRECCIÓN, no solo de estabilidad. `List<Int>.sum()` tenía el mismo problema (`total += as_int(item)?` crudo). **Fix**: `checked_int_numeric_op` (la función que ya cubría `/`/`%`) se generalizó -- el parámetro `bad` (antes hardcodeado a "divisor cero o desborde") ahora lo arma cada operador con su propio mensaje natural, porque `+`/`-`/`*` solo pueden desbordar, nunca hay un caso "por cero" que mencionar. `numeric_op` (la función sin guarda) quedó sin ningún caller, eliminada. El `-` unario usa `checked_neg()` (el único valor que desborda es `i64::MIN`, cuyo negativo no representa). Con esto, TODO operador aritmético entero del lenguaje pasa por una variante `checked_*` -- no queda ninguno con aritmética cruda.

**Efecto colateral honesto: dos tests de rondas anteriores (§3.163/§3.164) usaban desborde de `+` como disparador de un panic REAL, a propósito, para probar que `catch_unwind` protege contra un panic genérico.** Con `+` ahora protegido, ESE disparador específico ya no panica -- da un `RuntimeError` limpio por el camino normal, sin necesitar `catch_unwind` para nada. Los dos tests (`a_transaction_whose_body_overflows_still_rolls_back_and_leaves_the_db_usable` en `runtime/mod.rs`, `metrics_reports_a_cron_run_that_always_fails_as_a_failure_and_the_task_keeps_running` en `cli_metrics.rs`) se renombraron y sus asserts se actualizaron para reflejar la nueva realidad -- siguen siendo regresiones válidas de "esto rollbackea/mantiene vivo el scheduler", solo que ya no ejercitan el camino de panic específicamente. El mecanismo de `catch_unwind` en sí no cambió una línea; simplemente no queda, hoy, ningún disparador de producción conocido para probarlo end-to-end con un panic genuino -- el `a_transaction_whose_body_divides_by_zero_...` (división por cero, ya existente) sigue verificando que la composición con `rollback_transaction` funciona para un error normal.

**Evaluados y documentados a propósito, sin cambio de código (11, 12, 15):**
- **#11 (`@cache` con la misma forma de carrera que `@idempotent`)**: NO se aplicó el mismo fix de `@idempotent` (§3.167) -- la semántica correcta ahí sería "esperar al primero", no "rechazar con 409" (eso rompería el contrato de `@cache`), y esperar de forma sincrónica acopla la latencia de requests no relacionados sin evidencia real de que la estampida importe en la práctica. Ver §3.144.
- **#12 (`@unique`/`@index` no son índices parciales respecto a `@softDelete`)**: requiere índices parciales en los DOS backends más una migración seria para una base YA desplegada con el índice viejo (`DROP`+`CREATE`, riesgo real sobre datos de producción) -- discovery hecho, diseño e implementación quedan para una ronda propia. Ver §3.80.
- **#15 (composición check-then-act de `recordFailedLogin`/`failedLoginCount`)**: consecuencia inherente de exponer tres primitivas para componer en vez de un builtin atómico -- documentado como límite honesto, no atacado sin evidencia real de que importe. Ver §3.152.

**Verificado**: `an_empty_string_flag_value_behaves_like_the_flag_was_never_passed` (`cli_service_api_key.rs`) + `an_empty_string_jwt_secret_flag_behaves_like_it_was_never_configured` (`server_http.rs`), los dos contra un `linkc serve` real. `adopting_a_table_whose_physical_type_does_not_match_the_declared_one_gives_a_clean_error_not_a_panic` + `adopting_a_table_whose_stored_json_no_longer_matches_the_current_nested_type_gives_a_clean_error` (`db.rs`, los dos manipulando SQLite crudo para forzar el desacuerdo, mismo patrón que los tests de adopción existentes). `integer_add_sub_mul_and_unary_neg_overflow_are_clean_runtime_errors_too` + `list_int_sum_overflow_is_a_clean_runtime_error` (`runtime/mod.rs`) + repetición en vivo contra un `linkc serve` real (`add`/`neg` con valores en el borde de `i64`, confirmando 500 limpio y el proceso sigue vivo).

### 3.170 `countWhere`/`findWhere`/`deleteWhere`/`upsert` empujan `||` combinando condiciones — RESUELTO, cierra PLAN.md §9.3 ítem 1

§3.109 había dejado esto explícitamente como "alcance deliberado, queda para una ronda dedicada si aparece evidencia real" -- sin ningún reporte de adopción pidiéndolo puntualmente, pero con el hueco documentado desde el principio ("`||` necesitaría una cláusula `OR` separada en el SQL generado... una forma bastante más rica que agregar hojas a una lista plana"). Con la auditoría de `AUDIT-FIX-PLAN-2026-08-27.md` cerrada, este quedaba como el ítem "sin bloqueos" de mayor valor del backlog restante.

```
type Ticket = { id: Int, status: String, priority: Int, assignee: String }

rpc mineOrCritical(who: String) -> Ticket[] {
  db.tickets.findWhere(|t: Ticket| { t.assignee == who && t.status == "open" || t.status == "critical" })
}
```

**`ast::PredicateExpr` reemplaza la lista plana de hojas por un árbol `Leaf`/`And`/`Or`.** `recognize_predicate_expr` (reemplaza a `recognize_conjunction_predicate`, eliminada por no quedar ningún caller) recorre `&&`/`||` recursivamente respetando la precedencia REAL del lenguaje (`&&` liga más fuerte que `||`, igual que `parser.rs::parse_or_expr`/`parse_and_expr`) -- `a && b || c` reconoce exactamente `(a && b) || c`, nunca `a && (b || c)`. Dos funciones chicas (`merge_and`/`merge_or`) aplanan una cadena del MISMO tipo (`a && b && c` sigue siendo un `And` de 3 hojas, no `And(And(a,b),c)`) sin tocar el caso mixto (`a && (b || c)` sí anida un `Or` adentro de un `And`) -- así el SQL generado para el caso puro de `&&` de siempre no gana paréntesis de más.

**`runtime/mod.rs::ConditionExpr` es el espejo evaluado** (mismo árbol, con `Value` en vez de la expresión cruda sin evaluar) -- `recognize_pushable_predicate` reemplaza a `recognize_pushable_conjunction`. **`db.rs::condition_expr_sql` recorre el árbol generando `AND`/`OR` reales**, parentizando SOLO un hijo compuesto del tipo CONTRARIO al de su padre (`(b OR c)` adentro de un `AND`, o `(a AND b)` adentro de un `OR`) -- nunca un hijo del mismo tipo, que ya viene aplanado. El `WHERE` completo se parentiza entero antes de AND-ear el filtro de `@softDelete` si el árbol de nivel superior es un `Or` (sin esto, `a OR b AND soft_delete_is_null` perdería el filtro sobre la mitad "a" de la disyunción -- SQL evalúa `AND` antes que `OR`). Los placeholders posicionales (`$1`, `$2`, ... en Postgres) se numeran empujando cada `Cell` a un `Vec` COMPARTIDO durante todo el recorrido recursivo, en el mismo orden izquierda-a-derecha en que aparecen en el string final -- la numeración queda correcta sin importar en qué rama del árbol cae cada hoja, sin necesitar ningún caso especial por profundidad.

**Mismo comportamiento NULL-seguro que la conjunción pura (§3.109), ahora también dentro de una rama `||`**: una hoja `campo == variable` donde `variable` resultó `null` en runtime se traduce a `IS NULL` (nunca `= ?` con un parámetro NULL, que en SQL nunca es cierto) sin importar si esa hoja está en un `And` o un `Or`.

**Alcance sin cambios respecto a §3.109**: `!(...)` negando algo que no sea una hoja de campo suelta (`!(a && b)`, expansión De Morgan) no se reconoce -- alcance deliberado, no hay evidencia de demanda para esa forma. Una comparación entre DOS campos del propio parámetro (`endDate > startDate`) SÍ se resolvió después, ver §3.171.

**Verificado**: `count_where_and_find_where_push_down_a_disjunction_and_mixed_and_or` (`runtime/mod.rs`) -- una disyunción pura de 3 hojas, y `&&` mezclado con `||` confirmando la precedencia exacta (`(assignee==who && status=="open") || status=="critical"`, nunca la lectura alternativa). `a_null_valued_leaf_inside_an_or_branch_still_uses_is_null` -- una hoja NULL adentro de una rama `Or`. Los tests preexistentes de `&&` puro (§3.109) siguen pasando sin cambios, confirmando que el SQL para ese caso no ganó paréntesis de más. Más repetición en vivo contra un `linkc serve` real: `countWhere`/`findWhere` con la disyunción mixta del ejemplo de arriba, y `deleteWhere` con un `||` de dos hojas (`status == "done" || status == "cancelled"`), confirmando el conteo exacto de filas borradas. El ejemplo de "predicado NO pusheable" en los tests (§3.108/§3.109 ya habían tenido que corregirlo una vez cada uno, por el mismo motivo) se corrigió de nuevo -- de un `||` (ahora pusheable) a una comparación entre dos campos del propio parámetro, el único caso que sigue quedando (y que este mismo test tuvo que corregirse OTRA VEZ para, ver §3.171).

---

### 3.171 `countWhere`/`findWhere`/`deleteWhere` empujan comparaciones campo-vs-campo (`item.endDate > item.startDate`) — RESUELTO, cierra el resto de PLAN.md §9.3 ítem 1

Último hueco documentado del pushdown de predicados: §3.170 había dejado explícito que "una comparación entre DOS campos del propio parámetro (`endDate > startDate`) sigue sin pushear -- sin forma de expresar 'columna vs. columna' en un valor bindeado". Caso motivador citado en el propio PLAN.md: filtrar rangos de fecha inválidos (`endDate <= startDay`) sin traer la tabla entera a memoria.

```
type Booking = { id: Int, room: String, startDay: Int, endDay: Int }

rpc invalidRanges() -> Booking[] {
  db.bookings.findWhere(|b: Booking| { b.endDay <= b.startDay })
}
```

**Acotado a propósito a los cuatro operadores relacionales (`<`/`<=`/`>`/`>=`), NO `==`/`!=`.** El checker (`checker.rs::synth_binary`, brazo `Lt | LtEq | Gt | GtEq`) solo tipa esta forma cuando ambos lados son `Int`/`Int64`/`Float`/`Timestamp` **sin envolver en `Optional`** -- y un campo no opcional siempre es `NOT NULL` en la columna real (`postgres_emit.rs` tiene un test dedicado a confirmar que desenvolver `Optional` no cuela un `NOT NULL` de más). Eso significa que `"campoA" OP "campoB"` en SQL nunca puede toparse con NULL para una tabla que c-script creó -- a diferencia de `==`/`!=`, donde el checker SÍ permite comparar dos `T?` (dos enums nominales, dos structs, etc., vía `is_subtype`), y ahí `NULL = NULL` en SQL no es `true` como sí lo es en el camino interpretado (`Value::Null == Value::Null` de Rust). Replicar esa NULL-seguridad para columna-vs-columna habría necesitado algo como `(a IS NULL AND b IS NULL) OR a = b`, sin ningún caso real citado que lo pida -- así que `==`/`!=` entre dos campos queda deliberadamente sin pushear (cae al camino interpretado de siempre, correcto en cualquier caso).

**`ast::PredicateOperand` gana una tercera forma, `Field(&str)`** (además de `Expr`/`Bool`) -- en `recognize_predicate_tree`, el brazo de los seis operadores de comparación revisa PRIMERO (y solo para los cuatro relacionales) si AMBOS lados son `item.campo`; si es así, arma `Leaf(campoIzq, op, Field(campoDer))` sin evaluar nada todavía, igual que el resto de esta familia de reconocedores. **`runtime::ConditionExpr` gana `FieldPair(String, BinaryOp, String)`** junto a `Leaf`/`And`/`Or` -- a diferencia de `Leaf`, no hay ningún `Value` que bindear. **`db.rs::field_pair_condition_sql` genera `"campoA" OP "campoB"` directo**, sin ningún placeholder ni `Cell` empujado a la lista compartida -- valida que ambas columnas existan y no sean JSON (mismo criterio que `leaf_condition_sql`), incluyendo `"id"` como caso especial. Se integra al recorrido recursivo de `condition_expr_sql` como una hoja más -- una comparación campo-vs-campo puede convivir con hojas normales adentro del mismo `&&`/`||` sin ningún caso especial adicional.

**Límite honesto, `--adopt-existing` solamente**: la invariante "campo no opcional = columna `NOT NULL`" la garantiza el `CREATE TABLE` que emite c-script -- para una tabla adoptada de datos preexistentes que ya violaban esa invariante ANTES de que c-script la tocara, `check_schema_for_adoption` (ver AUDIT-2026-08-27.md #7) valida el TIPO declarado de cada columna pero no su nulabilidad. En ese escenario puntual, un NULL inesperado en una de las dos columnas haría que esa fila silenciosamente no matchee ninguna comparación (`NULL OP x` es `NULL`/falso en SQL) -- el camino interpretado, en cambio, ya falla con un `RuntimeError` limpio ANTES de llegar a evaluar el predicado (`row_to_fields` revienta primero al decodificar esa fila). Sin evidencia de un caso real que dependa de esto -- documentado, no bloqueante.

**Verificado**: `count_where_and_find_where_push_down_a_field_vs_field_comparison` (`runtime/mod.rs`) -- los cuatro operadores relacionales entre dos campos, y el mismo campo-vs-campo adentro de un `&&` junto a una hoja normal (`room == room && endDay <= startDay`), confirmando que el árbol mixto no se rompe. `delete_where_pushes_down_the_selection_for_a_field_vs_field_comparison` -- mismo caso para la SELECCIÓN de `deleteWhere` (el DELETE en sí sigue fila por fila, sin cambios). El ejemplo de "predicado NO pusheable" (§3.108/§3.109/§3.170 ya habían tenido que corregirlo, siempre por el mismo motivo: el alcance pusheable creció) se corrigió una vez más -- de un relacional campo-vs-campo (ahora pusheable) a un `==` campo-vs-campo, el único caso que sigue quedando. Suite completa sin regresiones. Repetición en vivo contra un `linkc serve` real (`findWhere`/`countWhere` con `endDay <= startDay` sobre 4 filas, conteo y filas exactas).

---

### 3.172 Varios `db { ... }`, uno por módulo, se fusionan en un solo namespace de colecciones — RESUELTO, cierra el último hueco de §3.161 (Pilar 3 del roadmap de skynet-d3)

§3.161 había dejado esto explícito como lo único que quedaba REALMENTE abierto del Pilar 3 (sistema de módulos) del roadmap de tres pilares que skynet-d3 relayó a nombre de Carlos: "un solo `db { ... }` por programa, sigue siendo un error duro... permitir varios `db {}` es una decisión de diseño con su propio peso (¿se fusionan las colecciones? ¿qué pasa con dos colecciones del mismo nombre en módulos distintos?)". Antes de esta ronda, el ÚNICO patrón que funcionaba era un `schema.link` central con el `db { ... }` que los módulos de servicio importaban -- cada módulo NO podía ser dueño de sus propias colecciones.

```
// billing.link
type Invoice = { id: Int, amount: Int }
db { invoices: Invoice[] }
service Billing {
  rpc create(amount: Int) -> Invoice { db.invoices.insert(Invoice { id: 0, amount: amount }) }
}

// crm.link
type Customer = { id: Int, name: String }
db { customers: Customer[] }
service Crm {
  rpc create(name: String) -> Customer { db.customers.insert(Customer { id: 0, name: name }) }
}

// main.link
import "./billing.link";
import "./crm.link";
```

**Discovery antes de tocar código, mismo criterio que §3.161: el blast radius real era mucho más chico de lo que parecía.** Auditando quién consume un `Item::Db` apareció que, salvo `checker.rs` (la única validación que construía `checker.db_collections`, el `HashMap<String, Type>` ya evaluado), TODO lo demás (`postgres_emit.rs`, `migrate.rs`, `runtime/db.rs`, `runtime/mod.rs`) ya consumía exclusivamente ese mapa fusionado, nunca el AST crudo -- ni siquiera las funciones que además cruzan con `program.items` para leer anotaciones (`@softDelete`/`@index`/`@check`/`@unique` compuesto) lo hacen buscando `Item::Db`, sino `Item::Type` por nombre de tipo, algo que el sistema de módulos YA fusiona correctamente desde §2.1. El cambio real quedó contenido en un solo lugar: el loop de `Checker::new` que antes rechazaba un SEGUNDO `Item::Db` sin mirar sus nombres.

**La regla nueva: cualquier cantidad de `db { ... }` se fusiona; lo único que sigue siendo un error duro es un NOMBRE DE COLECCIÓN repetido**, sin importar si las dos apariciones caen en el mismo bloque o en dos archivos distintos -- mismo criterio que ya aplica a `type`/`enum`/`fn`/`const` duplicados entre archivos (§2.1, `build_symbols`). **Gap preexistente cerrado de paso, no solo el caso nuevo**: antes de esta ronda, un nombre de colección repetido DENTRO de un único bloque (`db { posts: Post[], posts: OldPost[] }`) se perdía en silencio -- el `insert` sobre el `HashMap` pisaba la primera aparición sin ningún aviso, nunca ejercitado porque nadie escribiría eso a mano, pero un gap real de todos modos. Ahora es exactamente el mismo error que el caso entre dos archivos.

**De paso se cierra el gotcha de UX que §3.161 había documentado**: el error en cascada (`ya hay un 'db { ... }' declarado` seguido de `'db' no tiene ninguna colección llamada '<x>'`, apuntando al segundo bloque) desaparece para el caso legítimo -- ya no hay ningún "segundo bloque" que rechazar por sí solo. Para el caso de colisión real, un solo error nombra la colección repetida, sin ningún error derivado en cascada.

**Alcance sin cambios**: sigue sin haber visibilidad `pub`/privado (§2.1/§3.161) -- las colecciones fusionadas, igual que el resto del `Program`, son visibles desde cualquier `service` del cierre transitivo, tenga o no una relación de import directa con el módulo que las declaró.

**Verificado**: 3 tests nuevos en `checker.rs` -- dos `db { ... }` con nombres de colección DISTINTOS tipan limpio y las dos colecciones son usables desde el mismo `rpc` (`db.users.count() + db.orders.count()`); el nombre repetido entre dos bloques falla; el nombre repetido DENTRO de un solo bloque también falla (antes se perdía en silencio). 1 test nuevo en `modules.rs` -- dos módulos reales, cada uno con su propio `db { ... }` y su propio `service`, cargados por import de efecto (§3.161) desde un `main.link`, confirman que los dos `Item::Db` nativos llegan intactos al `Program` fusionado y que el checker completo los acepta. Más repetición en vivo contra el binario real: `linkc build` con dos módulos reales (arriba) genera el contrato limpio con los dos tipos de dominio; `linkc serve` real crea las dos tablas de verdad, cada `service` opera sobre la suya sin interferencia (`Billing.create`/`Billing.count` y `Crm.create`/`Crm.count` contra el mismo proceso, conteos exactos); y el caso de colisión (dos módulos declarando `db { things: ... }` con el mismo nombre) confirmado con un solo error limpio, sin cascada, vía `linkc build` real.

---

### 3.173 `@check(<expr>)` a nivel de `type` — RESUELTO, cierra la mitad "expresión booleana arbitraria" que §3.96 había dejado pendiente

§3.96 había dejado esto explícito en sus "Límites honestos": "solo rangos numéricos simples, ninguna expresión booleana arbitraria (comparar dos campos entre sí)". PLAN.md §9.3 ítem 3 lo repetía con el mismo ejemplo motivador: `endDate > startDate`.

```
@check(endDay > startDay)
type Booking = { id: Int, room: String, startDay: Int, endDay: Int }
```

**Complementa, nunca reemplaza, el `@check(min/max/range/minLength/maxLength, ...)` de un solo campo (§3.96/§3.146).** Mismo patrón que `@unique(campo1, campo2, ...)` (§3.155) frente al `@unique` de un solo campo: una anotación NUEVA de nivel `type` (`TypeAnnotation::Check(Spanned<Expr>)`, junto a `Unique`), antes de `type`, referenciando campos del propio struct por nombre PELADO (`endDay`/`startDay`, sin `self.`/prefijo -- no hay ningún parámetro que bindear, a diferencia de un closure de `findWhere`/etc.).

**Alcance deliberadamente acotado a lo que un `CHECK` de SQL puede expresar, no a "cualquier expresión de c-script".** `ast::validate_check_expr_shape` rechaza en el checker, ANTES de intentar tipar nada, cualquier forma que no sea identificador/literal/paréntesis/`!`/`-` unario/los operadores `==`/`!=`/`<`/`<=`/`>`/`>=`/`&&`/`||`/`+`/`-`/`*`/`/`/`%` -- ninguna llamada, acceso a `db`, closure, índice ni literal de struct/enum. Dejar pasar cualquiera de esas formas habría aceptado una anotación sin ninguna forma real de generar SQL para ella (o, peor, de aplicarla del lado de la aplicación con efectos secundarios reales adentro de lo que se supone una restricción PURA sobre los valores de una fila). Después de esa validación de forma, `Checker::check_type_level_check_expr` tipa la expresión contra `Bool` reusando el `check_expr`/`synth_expr` normal del lenguaje -- un `Env` armado con el tipo YA RESUELTO de cada campo del struct (`endDay`/`startDay` como si fueran variables sueltas), así que los mismos errores de siempre (`operador relacional requiere ...`, tipos incompatibles) salen con el mismo mensaje que en cualquier otro lado.

**Enforcement DOBLE, mismo criterio que el resto de `@check`**: un `CHECK` real de TABLA (no de columna, a diferencia del `@check` de un solo campo) en el `CREATE TABLE`, en los dos backends (`runtime::db::type_check_expr_sql` traduce la expresión ya validada a SQL, compartida por `db.rs::create_table_sql` y `codegen::postgres_emit::create_postgres_table_sql`) -- Y del lado de la aplicación, en los mismos DOS puntos de entrada que `apply_field_validators` (`json_to_typed_value` para el wire, `Expr::StructLit` para un literal construido en el cuerpo de un rpc). El evaluador de aplicación (`runtime/mod.rs::eval_check_expr`) es chico y autocontenido -- sin `db`/`fns`/`sessions`/`step_budget`, porque el shape ya restringido nunca puede necesitar ninguno de esos -- pero REUSA la misma aritmética/comparación que el intérprete general (`checked_int_numeric_op`/`compare`/`as_bool`), para no mantener una segunda copia de esas reglas (desborde, etc.) por separado.

**Un valor PARCIAL (`applyPatch`/`Patch<T>`) SALTEA la expresión completa si le falta CUALQUIER campo que referencia** -- generaliza el mismo criterio de "ausente: nada que validar" que `@check` de un solo campo ya aplicaba campo por campo. Un patch que solo toca `room` no evalúa `endDay > startDay` (ninguno de los dos vino en el patch); uno que toca los dos SÍ se evalúa de verdad.

**`==`/`!=` contra `null` se traducen a `IS [NOT] NULL`**, mismo footgun (y mismo fix) que `leaf_condition_sql` (§3.170) ya había cerrado para el pushdown de predicados: `"campo" = NULL` en SQL nunca es `true`, así que una hoja `campoOpcional == null` necesita la forma `IS NULL` para significar lo que dice.

**Límite honesto, mismo espíritu que el resto del proyecto**: los literales `Bool` se traducen a las palabras clave `TRUE`/`FALSE`, nunca `1`/`0` -- SQLite acepta las dos formas, pero Postgres NO convierte un entero a booleano en silencio (`CHECK(activo AND 1)` falla ahí con un error de tipos), así que las palabras clave son la única forma que funciona igual en los dos motores.

**Límite honesto encontrado en vivo (skynet-43, adopción de iaacademy, 29/08/2026): ningún campo `Optional` puede participar en aritmética/comparación dentro de la expresión.** `validate_check_expr_shape` rechaza `match`/`??` en esa posición (misma lista acotada de arriba), así que no hay ninguna forma de "desenvolver" un `Int?`/`String?` antes de operarlo -- una restricción real como `subtotal + tax_amount == amount` con `subtotal`/`tax_amount` declarados `Int?` no se puede expresar todavía, aunque `==`/`!=` contra el literal `null` sí funcionan (línea de arriba). Sin evidencia de demanda propia más allá de este caso -- queda documentado, no una ronda propia todavía.

**Verificado**: 9 tests de checker (`checker.rs`) -- comparación de dos campos, `&&`/`||`/aritmética combinados, referencia a un campo inexistente, tipos incompatibles (`String` vs `Int`), la expresión no tipa a `Bool`, una llamada rechazada, acceso a `db` rechazado, sobre un alias no-struct rechazado, y coexistencia con `@unique` compuesto en el mismo `type`. 4 tests de runtime (`runtime/mod.rs`) -- rechazo al construir el struct en el cuerpo de un rpc, rechazo recibiendo el struct completo por el wire, un patch parcial que NO toca ninguno de los dos campos referenciados salteado sin evaluar, un patch parcial que SÍ toca los dos evaluado y rechazado de verdad. 1 test de DDL estático (`postgres_emit.rs`) confirmando el `CHECK` de tabla en el SQL que `linkc build` emite. 1 test contra un Postgres real (`pg_integration.rs`) -- servidor real acepta una fila válida, rechaza una inválida con 400 (no 500), y un `INSERT` SQL crudo que viola el mismo constraint se rechaza sin pasar por c-script en absoluto. Más repetición en vivo contra un `linkc serve` real con SQLite: `.schema` confirma el `CHECK ("endDay" > "startDay")` real en la tabla, y un `INSERT` crudo vía `sqlite3` lo rechaza (`CHECK constraint failed: endDay`). Suite completa sin regresiones (1285 tests, +15 sobre v1.127.0).

---

### 3.174 `@unique(...) where <expr>`: la mitad CONDICIONAL de §3.155 — RESUELTO

§3.155 había dejado esto explícito como la mitad que quedaba afuera de su ronda: "sin evidencia de demanda propia todavía más allá del caso de Glowapp ya citado, y es una pieza de diseño separada (sintaxis para una expresión booleana en la anotación, no solo una lista de nombres de campo)". El caso citado, textual: el schema Drizzle de Glowapp declara `UNIQUE(userId, appointmentDate, startTime) WHERE status != 'cancelled'` -- permite reusar un horario una vez cancelado, sin acumular filas basura.

```
@unique(userId, appointmentDate, startTime) where status != "cancelled"
type Appointment = { id: Int, userId: Int, appointmentDate: String, startTime: String, status: String }
```

**"Pieza de diseño separada" resultó ser, en los hechos, la MISMA pieza que §3.173 (`@check(<expr>)` de tipo) acababa de construir esta misma sesión, un rato antes.** `where <expr>` reusa DIRECTO `Checker::check_type_level_check_expr` (misma validación de forma vía `ast::validate_check_expr_shape`, mismo tipado contra `Bool` con un `Env` de TODOS los campos del struct) y `runtime::db::type_check_expr_sql` (misma traducción a SQL) -- cero código nuevo de validación o traducción, solo enchufar la condición ya existente en un lugar nuevo. La condición puede referenciar CUALQUIER campo del struct, no solo los que integran el conjunto único -- el caso de Glowapp mismo lo exige: `status` es ajeno a `(userId, appointmentDate, startTime)`.

**`where` NO es palabra reservada, mismo criterio que `db` (§2.1): se reconoce por texto solo INMEDIATAMENTE DESPUÉS del `)` de un `@unique(...)`**, así que sigue siendo un identificador común y corriente en cualquier otro contexto (un campo llamado `where`, por ejemplo, sigue siendo válido).

**A diferencia de `@check(<expr>)`, acá NO hace falta ningún enforcement de aplicación nuevo.** `@unique` nunca tuvo un evaluador de aplicación -- siempre fue puramente un constraint de base (`CREATE UNIQUE INDEX`), con la violación detectada por el mensaje/SQLSTATE que el motor devuelve (`is_unique_violation`, ya existente) y traducida a 400. El índice único compuesto se vuelve PARCIAL (`CREATE UNIQUE INDEX ... ON "tabla"(cols) WHERE <condición>`, sintaxis idéntica en SQLite y Postgres) -- toda la semántica "condicional" la resuelve el motor de base al mantener el índice, sin que c-script tenga que replicarla en ningún otro lado.

**Bug encontrado y arreglado en el camino, antes de shippear: el nombre determinístico del índice no podía concatenar la condición SQL tal cual.** `composite_unique_index_name` (§3.155) ya codificaba los nombres de campo con un esquema de prefijo de longitud (inyectivo, sin separador ambiguo) -- pero una condición SQL ya traducida trae comillas/paréntesis/espacios (`("status" != 'cancelled')`), que rompen el identificador entre comillas dobles que envuelve el nombre completo (`"idx_..."`) si se pegan directo. Confirmado en vivo antes del fix: SQLite rechazaba la sentencia con un error de sintaxis a mitad del nombre del índice. En vez de escapar comillas a mano, la condición se hashea con SHA-256 (`lockfile::hash_source`, la MISMA implementación que el sistema de módulos ya usa para detección de deriva, §2.1 -- sin sumar una segunda implementación de hashing al proyecto) -- determinista, sin caracteres problemáticos.

**El dedup de redundancia ahora es por `(conjunto de campos, condición)`, no solo por conjunto de campos.** Dos `@unique` con los MISMOS campos pero condiciones DISTINTAS (o uno con condición y otro sin) son dos constraints PARCIALES legítimos y distintos -- ya NO se rechazan como redundantes (antes de esta ronda, esa combinación ni siquiera podía expresarse). El mismo conjunto Y la misma condición sí siguen siendo redundantes, igual que antes.

**Verificado**: 7 tests de checker (`checker.rs`) -- el caso real de Glowapp tipa limpio; la condición referenciando un campo inexistente rechazada; una llamada dentro de la condición rechazada; la condición que no tipa a `Bool` rechazada; dos `@unique` con los mismos campos pero condiciones DISTINTAS conviven; el MISMO conjunto Y la MISMA condición sí se rechazan por redundante; un `@unique` condicional y uno sin condición sobre el mismo conjunto conviven. 1 test contra SQLite real (`runtime/mod.rs`) reproduciendo el caso exacto de Glowapp: el mismo horario "confirmed" repetido se rechaza, el mismo horario "cancelled" se acepta (la fila existente queda afuera del índice parcial). 1 test de DDL estático (`postgres_emit.rs`) confirmando el `CREATE UNIQUE INDEX` parcial exacto en el SQL que `linkc build` emite. 1 test contra un Postgres real (`pg_integration.rs`) reproduciendo el mismo caso de Glowapp de punta a punta por HTTP real. Más repetición en vivo contra un `linkc serve` real con SQLite: `.schema` confirma el índice único parcial real (`... WHERE ("status" != 'cancelled')`), y las tres llamadas HTTP reales (booking inicial, choque con 400, reuso tras cancelar con 200) confirman el comportamiento de punta a punta. Suite completa sin regresiones (1295 tests, +10 sobre v1.128.0).

---

### 3.175 `linkc db inspect`: primera pieza de la suite de administración de datos — RESUELTO PARCIAL

PLAN.md §9.7 ítem 2 pedía una suite completa (`inspect`/`shell`/`export`/`import`/`seed`) -- esta ronda ataca solo la primera pieza, la más chica y de mayor valor inmediato: un diagnóstico de solo lectura de qué colecciones existen FÍSICAMENTE y cuántas filas tienen, sin ejecutar ningún DDL. `shell`/`export`/`import`/`seed` quedan explícitamente para rondas futuras.

<!-- linkc:check -->
```rust
type Item = { id: Int, name: String }
db { items: Item[] }
service Items { rpc add(name: String) -> Item { db.items.insert(Item { id: 0, name: name }) } }
```

```
$ linkc db inspect app.link --db app.db
linkc db inspect -- 'app.link' contra SQLite embebido en 'app.db'

  items       2 columna(s) declaradas  1 fila(s)

1 colección(es) declaradas, 0 sin crear todavía, 1 fila(s) en total
```

**Mismo espíritu de solo lectura que `linkc doctor`/`linkc migrate --dry-run`, y reusa el mismo `resolve_db_source` (`--db`/`LINK_DATABASE_URL`) que esos dos y `linkc serve`.** `src/inspect.rs`, módulo nuevo detrás del feature `runtime` (mismo motivo que `migrate`/`introspect`: habla SQLite/PostgreSQL de verdad, no puede vivir en el build wasm32 del playground). Reusa DOS funciones ya existentes en vez de duplicar introspección: `sqlite_table_exists` (`runtime/db.rs`, antes privada de ese módulo) y `existing_columns` (`migrate.rs`, antes privada) -- las dos solo necesitaron subir de visibilidad a `pub(crate)`, sin ningún cambio de comportamiento.

**"Existe" vs. "no existe" nunca es ambiguo con "existe pero está vacía".** `exists: false` implica `row_count: None`, nunca `Some(0)` -- un checkout fresco antes del primer `linkc serve` reporta cada colección declarada como "no existe todavía", no como "0 filas" (que sugeriría que la tabla ya está ahí, solo sin datos).

**El conteo es FÍSICO, sin filtrar `@softDelete` -- mismo criterio que `db.tableStats()` (§3.151), a propósito distinto de `count()`.** Una fila soft-deleteada sigue contando: el punto de `inspect` es ver qué hay de verdad en el disco, no lo que un `rpc` normal vería a través del filtro de aplicación.

**SQLite: `Connection::open_with_flags(..., SQLITE_OPEN_READ_ONLY)`** -- la intención de solo-lectura queda expresada en el flag de apertura, no solo en qué SQL se manda. Un archivo `.db` inexistente NUNCA es un error -- es exactamente el caso "ninguna colección creada todavía", el mismo que un checkout fresco antes de arrancar `linkc serve` por primera vez.

**Verificado**: 5 tests de CLI contra el binario real (`cli_db_inspect.rs`) -- una base SQLite inexistente reporta cada colección como no creada; una base REAL poblada por un `linkc serve` real (no un archivo armado a mano) reporta las filas reales, confirmando que una fila soft-deleteada sigue contando; sin argumentos da un error de uso limpio; un sub-subcomando desconocido (`db shell`, todavía no implementado) también; una URL de Postgres inalcanzable falla limpio y rápido, sin colgarse ni entrar en pánico. 1 test contra un Postgres real (`pg_integration.rs`) -- filas reales insertadas por un `linkc serve` real contra Postgres, una colección declarada pero nunca creada reportada como "no existe todavía" en vez de "0 filas".

**Sigue pendiente, PLAN.md §9.7 ítem 2**: `linkc db shell` (REPL de solo lectura), `linkc db export`/`import` (entre entornos o motores), `linkc db seed` (poblar una base nueva desde un fichero) -- cada uno queda para su propia ronda.

---

### 3.176 Reporte de adopción de iaacademy: `linkc introspect` avisa sobre una PK `id` no entera, `linkc doctor --target-url` detecta deriva de versión — RESUELTO PARCIAL

Reporte de adopción real (proyecto nº5 del ecosistema, iaacademy, vía la sesión skynet-43, 29/08/2026): tres tablas públicas (`leads`, `posts`, `seo_pages`) tienen `id uuid DEFAULT gen_random_uuid()`, y GRAMMAR.md §3.36/§3.59 exige `id: Int` -- `linkc migrate --dry-run` y `linkc serve` ya rechazan esa forma al conectar, con un diagnóstico que nombra tabla y tipo, así que ningún dato corrió riesgo. Dos gaps SÍ eran nuevos y verificables contra el código real, no solo contra el diagnóstico en runtime:

**1. `linkc introspect` (§3.66) emitía `id: Int` sin ninguna advertencia para una PK `uuid` llamada `"id"` — RESUELTO.** `introspect_table` (`introspect.rs`) tiene una rama por NOMBRE de columna (`pk_columns == ["id"]` → emitir `id: Int`, el camino normal) separada del mapeo por TIPO (`map_pg_type`, que sí advierte sobre `uuid` en cualquier OTRA columna). La rama por nombre nunca miraba el `pg_type` real de esa columna, así que una PK `id uuid` producía el mismo `.link` "limpio" que una PK `id BIGSERIAL` -- la única señal de que algo estaba mal aparecía recién en `linkc serve`/`migrate --dry-run`, después de ya haber escrito un programa entero alrededor de un `id: Int` que nunca fue real. Ahora la rama consulta el `pg_type` real de la columna `"id"` y agrega una advertencia (mismo canal por stderr que el resto de `introspect`, nunca omite la columna) si no es `bigint`/`integer`/`smallint`.

**2. `linkc doctor` no tenía forma de detectar deriva de versión entre el entorno local y un `linkc serve` de producción ya corriendo — RESUELTO, alcance chico.** Observación de despliegue, no un bug de datos: el `/health` de `linkc serve` (§3.100 lo reusa para PostgreSQL) ya devuelve `version` desde siempre, pero nada comparaba esa versión contra la del binario local. Nuevo flag opt-in, `--target-url <url>`/`LINK_DOCTOR_TARGET_URL`: si está presente, `doctor` hace `GET <url>/health` y compara `version` contra `linkc::VERSION` local -- `[OK]` si coinciden, `[INFO]` (no falla el chequeo, solo lo hace visible) si difieren, `[ERROR]` si la URL no responde o no es un servidor `linkc`. Sin el flag, comportamiento idéntico a siempre, cero requests salientes.

**Deliberadamente NO resuelto en esta ronda -- el bloqueo real de iaacademy.** El pedido de fondo (aceptar `id: Uuid` -- o cualquier tipo no entero -- como clave primaria de una colección, delegando la generación al `DEFAULT` de la columna en vez de un autoincremento) sigue sin solución dentro del lenguaje. No es un gap chico: toca `insert`/`insert_returning_id` (necesita generar el valor client-side en vez de leer `RETURNING "id"`/`last_insert_rowid()`), el tipo de parámetro de `insert` (que hoy asume "sin id" porque el motor SIEMPRE lo genera), el emisor SQLite (`INTEGER PRIMARY KEY AUTOINCREMENT` no tiene equivalente sin una columna entera) y el propio checker (qué tipos son válidos como PK, y con qué default de generación cada uno). Es señal de que el lenguaje todavía no cubre una forma común y real de modelar datos (UUID como PK es habitual en microservicios/sistemas distribuidos) -- una lectura general de madurez, no un ticket puntual de iaacademy -- así que queda para su propia ronda de diseño, no una resolución apurada acá.

**Actualización (mismo día, §3.177): resuelto.** La ronda de diseño que este párrafo pedía se hizo el mismo 29/08/2026 -- `id: Uuid` ya es una PK soportada de punta a punta, generada del lado de la aplicación. `linkc introspect` contra la tabla `id uuid` de iaacademy ahora emite `id: Uuid` directo, sin advertencia (ver `map_pg_type_covers_the_common_scalar_types_without_a_warning` y las pruebas de `introspect.rs` -- el test que este párrafo citaba como "sigue generando `id: Int`" se reemplazó, ver §3.177).

**Sin cambio de código, señal de producto para otra ronda:** los 4 proyectos ya en el VPS (`myfinance`, `ignislove`, `porngit`, `segurma`) tienen su `.link` colocado a mano, sin git ni CI en el servidor -- funciona, pero sin trazabilidad repo↔producción. Ningún camino de despliegue recomendado está documentado todavía; `linkc pm2-config` (§3.121) cubre parte del hueco pero no lo cierra.

---

### 3.177 `id: Uuid` como PK alternativa — RESUELTO, cierra el bloqueo real de iaacademy que §3.176 dejó pendiente

§3.176 había dejado esto explícito como lo único que quedaba REALMENTE bloqueando la migración de iaacademy: sus tablas públicas `leads`/`posts`/`seo_pages` tienen `id uuid DEFAULT gen_random_uuid()`, y GRAMMAR.md §3.36/§3.59 solo aceptaba `id: Int`. Esta ronda cierra ese gap -- `id: Uuid` es ahora una PK de primera clase, con el mismo tratamiento que `id: Int` en todo el lenguaje: `find`/`applyPatch`/`delete`/`increment`/`insert`/`upsert`/`page`/`maxRow`/`minRow` funcionan igual, `insert` sigue tomando `Omit<T,"id">` (nunca cambia por nombre de campo, no por tipo), y `linkc introspect`/`linkc migrate --dry-run`/`--adopt-existing` reconocen la forma real de iaacademy sin fricción.

<!-- linkc:check -->
```rust
type Lead = { id: Uuid, email: String, score: Int }
type NewLead = { email: String, score: Int }
db { leads: Lead[] }
service Leads {
  rpc create(email: String, score: Int) -> Lead { db.leads.insert(NewLead { email: email, score: score }) }
  rpc get(id: Uuid) -> Lead? { db.leads.find(id) }
  rpc update(id: Uuid, patch: Patch<Lead>) -> Lead { db.leads.applyPatch(id, patch) }
  rpc remove(id: Uuid) -> Bool { db.leads.delete(id) }
}
```

**La PK se genera SIEMPRE del lado de la aplicación, nunca del motor.** `Db::call` ("insert", `runtime/db.rs`) genera un UUIDv4 real (`generate_uuid_v4`, la MISMA función que `crypto.uuid()` usa -- extraída a `runtime/mod.rs` para que las dos compartan un solo generador, nunca dos copias del layout de bytes que puedan desalinearse) ANTES de armar el `INSERT`, y lo manda como valor EXPLÍCITO -- nunca depende de `DEFAULT`/`RETURNING`/`last_insert_rowid()`. Es la decisión de diseño que hace posible adoptar una tabla existente sin tocarla: no importa si la columna real tiene `DEFAULT gen_random_uuid()` o ningún default en absoluto, c-script nunca lo necesita.

**DDL: `TEXT PRIMARY KEY NOT NULL` en SQLite, `UUID PRIMARY KEY` nativo en Postgres.** Los dos casan con lo que un campo `Uuid` normal (no-PK) ya usaba en SQLite (`ColumnPlan::kind` -> `ColumnKind::Text`) -- pero en Postgres, a propósito, la PK usa el tipo NATIVO `uuid`, distinto de cualquier otro campo `Uuid` (que sigue siendo `TEXT`, sin cambios): es lo único que permite adoptar una columna que YA es `uuid` nativo en una base real, el caso exacto de iaacademy. `NOT NULL` explícito en la rama SQLite -- quirk real y documentado del motor: un `PRIMARY KEY` sobre cualquier tipo que no sea el alias de `rowid` (`INTEGER PRIMARY KEY`) NO implica `NOT NULL` por sí solo, a diferencia del estándar SQL.

**Encodificación/decodificación binaria de `uuid` a mano, sin la dependencia `with-uuid-1`.** `postgres` (la crate, sin ese feature opcional) no sabe leer/escribir el formato binario nativo de `uuid` -- ni `String::to_sql`/`FromSql` lo cubren (esos son para `TEXT`/`VARCHAR`). El intento INICIAL de esta ronda fue agregar un cast SQL `::uuid` al placeholder (`$1::uuid` en vez de `$1`), asumiendo que forzaría al servidor a inferir el parámetro como texto -- **verificado FALSO contra Postgres real en CI**: el servidor sigue infiriendo el tipo desde la columna destino sin importar el cast, así que el mismatch de wire (36 bytes de texto UTF-8 donde el protocolo espera 16 bytes binarios) seguía pasando (`ERROR: incorrect binary data format in bind parameter 1`). El arreglo real: `Cell::to_sql` (`store.rs`) detecta cuándo el servidor pide el tipo `uuid` (`ty == postgres::types::Type::UUID`) y en ESE caso decodifica la forma canónica de 36 caracteres a sus 16 bytes crudos a mano (`uuid_string_to_binary`) en vez de mandar el texto tal cual -- la forma canónica ya está validada en el borde (`is_canonical_uuid`) antes de llegar hasta acá, así que el parseo es fijo y chico, no un formato arbitrario. La LECTURA tiene el mismo problema simétrico -- `ColumnKind::Uuid` (nuevo, distinto de `Text`) hace que `postgres_cell` decodifique esos mismos 16 bytes de vuelta a la forma canónica (`PgUuidText`), en vez de intentar leerlos como `String` (que fallaría, `String::accepts` no cubre el OID de `uuid`). SQLite nunca necesita nada de esto -- no tiene un tipo binario separado del texto, `ColumnKind::Uuid` se comporta ahí exactamente igual que `Text`.

**`pageAfter` RECHAZADO a propósito sobre una PK Uuid -- no "todavía no soportado".** Su garantía real ("nunca se salta una fila insertada durante la paginación") depende de que el id crezca en el MISMO orden que la inserción -- cierto para un autoincremento, falso para un UUIDv4 aleatorio: una fila insertada concurrentemente con id menor al cursor actual quedaría afuera de toda página futura del mismo pase, en silencio. El checker (`checker.rs::check_db_method`) lo rechaza con un mensaje que nombra el motivo, no un error genérico -- dejarlo pasar con orden lexicográfico habría sido "compila y corre" con una garantía documentada rota sin ningún aviso. `page` (offset) no tiene ese problema -- LIMIT/OFFSET no depende de que el orden sea cronológico, solo de que sea TOTAL -- así que sigue funcionando igual sobre cualquier PK.

**Límite honesto, deliberado**: `findWhere`/`countWhere`/`deleteWhere`/`upsert` empujan a SQL una condición `x.id == valor` (GRAMMAR.md §3.95/§3.108/§3.170) SOLO cuando `valor` es `Value::Int` (`leaf_condition_sql`, sin cambios esta ronda) -- una condición sobre el id de una colección con PK Uuid cae siempre al camino INTERPRETADO, nunca a un error: sigue siendo correcto, solo no empuja esa hoja puntual a SQL. Sin evidencia de demanda real más allá de lo que esta ronda ya resuelve (el caso de iaacademy es `find`/`insert`/`applyPatch`/`delete`, todos con SQL real) -- queda documentado, no una ronda propia todavía.

**`linkc introspect` (§3.66/§3.176) y `linkc migrate --dry-run`/`--adopt-existing` (§3.36/§3.67/§3.97) ya sabían de esto -- solo necesitaban la mitad de RUNTIME que faltaba.** `introspect` ahora emite `id: Uuid` directo (sin advertencia) para una PK `uuid` nativa; `validate_existing_id_column` (compartida por el connect real y `migrate --dry-run`, una sola fuente de verdad) acepta `uuid` cuando el `.link` declara `id: Uuid`, con el mismo criterio de "cualquier OTRO tipo real, rechazo con mensaje claro" que ya aplicaba para `id: Int`.

**Verificado**: 6 tests de checker (`checker.rs`) -- `id: Uuid` acepta como PK, `find` rechaza un `Int` contra una colección Uuid y viceversa, `applyPatch`/`delete`/`increment` aceptan un id `Uuid`, `pageAfter` rechazado con el mensaje que nombra el motivo, `insert` sigue omitiendo `id` de la forma insertable sin importar su tipo. 5 tests de runtime contra SQLite real (`runtime/mod.rs`, vía `invoke_rpc`) -- ciclo CRUD completo (insert/find/applyPatch/increment/delete, dos inserts nunca chocan de id), `upsert` en sus dos ramas (insert e in-place-update) preservando un id Uuid real, `page`/`maxRow`/`minRow` decodificando la columna id con el `ColumnKind` correcto (encontrado un bug real ahí: `find_where_conjunction`/`select_rows_page`/`top_row` tenían el mismo `ColumnKind::Int` hardcodeado que `select_rows` -- el test de `upsert` lo atrapó primero, vía su pushdown de predicado), y que reabrir un archivo SQLite ya escrito pasa `check_schema_matches` contra su propio schema Uuid. 3 tests contra un PostgreSQL real (`pg_integration.rs`) -- ciclo CRUD completo contra una tabla FRESCA (`UUID PRIMARY KEY` nativo, creada por `linkc serve`), el mismo ciclo con `--adopt-existing` contra una tabla preexistente con `id uuid DEFAULT gen_random_uuid()` armada a mano (el caso EXACTO de iaacademy, confirmando con SQL crudo por fuera de c-script que el id que quedó en la fila es el que la aplicación generó, no el `DEFAULT` de la columna) y `migrate --dry-run` reportando "sin cambios" en vez del rechazo de antes. Suite completa sin regresiones (1316 tests locales, +14 sobre v1.132.0; los 3 nuevos de `pg_integration.rs` corren en CI contra Postgres real).

**Segundo bug real, encontrado por CI, no localmente (v1.133.0 → v1.133.1): el primer intento de resolver el bind contra `uuid` nativo estaba mal.** La primera versión de esta ronda intentaba resolver el mismatch de wire con un cast SQL explícito (`$1::uuid`), asumiendo que forzaría a Postgres a inferir el parámetro como texto -- sin Postgres real corriendo localmente, esa hipótesis nunca se pudo probar antes de pushear. CI (que sí corre contra Postgres real) la refutó: `ERROR: incorrect binary data format in bind parameter 1` -- el servidor infiere el tipo del parámetro de la COLUMNA destino sin importar el cast. El arreglo real está descrito arriba (encodificación/decodificación binaria a mano, `uuid_string_to_binary`/`PgUuidText`) -- confirma, una vez más, que "sin Postgres local, todo lo que toque su wire binario se verifica en CI, nunca por inspección de código" (mismo criterio que ya dejaron GRAMMAR.md §3.58/§3.59 sobre los anchos de entero).

---

### 3.178 `@rate_limit` distribuido vía Postgres — RESUELTO

`@rate_limit` (§3.39) era un token bucket en memoria, por PROCESO -- con más de una instancia de `linkc serve`/`serve-all` detrás de un balanceador (el patrón real que corre hoy en producción: IgnisLove, varios procesos `serve-all`/pm2 compartiendo un único Postgres, PLAN.md §9.3), el límite se diluía: `N` réplicas dejaban pasar hasta `N × capacidad`, no `capacidad`, sin ningún error que lo señalara -- solo un contador en `/metrics` (`linkc_rate_limit_rejections_total`, §3.39/§3.149) para NOTAR el problema, nunca una solución. Esta ronda cierra ese gap para quien ya corre Postgres: el mismo bucket, compartido de verdad por todas las instancias que apuntan a la MISMA base.

**Sin flag nuevo, sin cambio de sintaxis -- `@rate_limit("N/ventana")` es exactamente el mismo de siempre.** Lo que cambia es DÓNDE vive el estado del bucket: una tabla interna, `_linkc_internal_rate_limits` (prefijo reservado, nunca colisiona con una colección declarada por el usuario), creada automáticamente al conectar contra Postgres -- invisible para `db {}`, `linkc introspect`, `linkc migrate --dry-run`/`linkc db inspect` (ninguno de los tres la lista ni la toca, no es una colección del programa). SQLite sigue exactamente igual que siempre -- un solo archivo/proceso ya tiene el estado exacto en memoria, no hay nada que distribuir.

```
service Sys {
  @rate_limit("5/2s")
  rpc ping() -> String { "pong" }
}
```

**Mismo algoritmo EXACTO que el `RateLimiter` en memoria** (`rate_limit.rs`): token bucket con refill CONTINUO (no ventanas fijas que resetean de golpe) -- `capacidad = N`, `refill_por_segundo = N / ventana`. La única diferencia real es dónde vive el estado.

**Un solo UPSERT atómico, mismo criterio que `increment()` (§3.105): nunca leer-y-después-escribir en dos pasos separados que puedan carrerear entre procesos distintos.** El refill/consumo se calcula DENTRO del propio `SET` de un `INSERT ... ON CONFLICT ("bucket_key") DO UPDATE`, referenciando `"_linkc_internal_rate_limits".tokens`/`.last_seen_ms` -- los valores REALES de la fila ya bloqueada por el propio UPSERT en el momento de escribir, nunca un valor leído por separado antes (que sí podría quedar desactualizado si otra instancia escribe en el medio). La cláusula `WHERE` sobre la acción `DO UPDATE` (sintaxis real de Postgres: si la condición es falsa, la fila conflictiva simplemente no se toca) es lo que hace que "no alcanzan los tokens" nunca escriba nada -- ni siquiera `last_seen_ms` avanza, así que el próximo intento sigue viendo el reloj real transcurrido y el refill se sigue acumulando sin este paso. `capacity`/`refill_per_sec` se reescriben en cada check EXITOSO, así que un redeploy con un `@rate_limit(...)` distinto converge solo, sin limpiar la tabla a mano.

**Degradación, nunca un servidor que no arranca.** `--adopt-existing` nunca ejecuta DDL, ni siquiera para esta tabla propia -- si no existe ya (un operador no la creó a mano), esta instancia simplemente usa el `RateLimiter` en memoria de siempre para todo, comportamiento IDÉNTICO al de antes de esta ronda. Fuera de ese modo, si la creación falla por cualquier motivo (un rol sin permiso de `CREATE TABLE`, poco común pero posible), el arranque de `linkc serve` NO aborta -- solo esta pieza se degrada, con una advertencia por stderr. Un error transitorio de conexión en un check puntual (`Backend::query` ya reintenta solo, §3.40) también degrada a "usá el limitador en memoria para ESTE check", nunca deja una request colgada ni la rechaza por un problema de infra ajeno.

**Límite honesto, deliberado**: la clave del bucket sigue siendo `(identidad de cliente, servicio, rpc)`, sin cambios -- dos `.link` DISTINTOS que declaren el mismo par (servicio, rpc) contra la MISMA base compartirían bucket por accidente, el mismo tipo de colisión por convención de nombre que ya existe para colecciones (§3.94). Sin evidencia de demanda real de un namespace explícito todavía.

**Verificado contra Postgres real** (`pg_integration.rs`) -- DOS instancias `linkc serve` reales, corriendo como procesos separados, apuntando a la MISMA base: 16 requests concurrentes repartidas entre las dos (8 cada una) contra `@rate_limit("5/60s")` admiten EXACTAMENTE 5 en total, no 5 por instancia (10) -- la prueba directa de que el bucket es de verdad compartido, no dos independientes. Refill real: agotar la capacidad, esperar más que la ventana completa, confirmar que vuelve a admitir. `--adopt-existing` sin la tabla interna preexistente: el servidor arranca y sigue rechazando al agotar la capacidad -- por proceso, no compartido -- confirmando además que la tabla nunca se creó.

**Bug real encontrado por CI, no localmente (v1.134.0 → v1.134.1): el primer intento admitía EXACTAMENTE el doble (10, no 5) -- un patrón demasiado limpio para ser ruido de timing.** El test de las dos instancias reveló que las DOS caían en silencio al `RateLimiter` en memoria -- el UPSERT distribuido fallaba en cada request con `incorrect binary data format in bind parameter 2`, tragado por el `Err(_) => None` de degradación. Causa real: `$2 - 1` con el literal entero `1` SIN TIPO hacía que Postgres infiriera `$2` como `integer`, no `double precision` -- la PRIMERA aparición de un parámetro fija su tipo para TODA la sentencia; encontrarlo después en un contexto `DOUBLE PRECISION` (la columna `capacity`) no lo corrige, solo inserta un cast implícito ahí -- pero el driver seguía mandando 8 bytes de `Cell::Float` (formato binario `float8`) contra un parámetro que el servidor esperaba como 4 bytes de `int4`. Arreglado con un cast explícito (`$2::double precision`, `$3::double precision`, `$4::bigint`) en CADA aparición, sin depender de en qué orden Postgres visite las distintas apariciones para inferir el tipo -- mismo espíritu que el cast `::uuid` de §3.177, pero acá SÍ funcionó porque el objetivo era distinto: fijar un tipo concreto sin ambigüedad, no fingir "unknown" ante el servidor. De paso, la degradación silenciosa que ocultó esto durante el desarrollo se volvió un `eprintln!` real -- una landmine que un operador podía no notar nunca (§3.153) ahora deja rastro.

---

### 3.179 `String` contra `uuid`/`inet`/`cidr` NATIVOS de Postgres — RESUELTO

Segundo reporte real de adopción de iaacademy en el mismo día (vía skynet-43), esta vez un bug de verdad, no de discoverability: `leads`/`posts`/`seo_pages`, ya migradas a `id: Int` (§3.176/§3.177), seguían rompiendo con `--adopt-existing` -- `find`/`findWhere`/`all` fallaban con `"error deserializing column N"` contra datos reales, aunque `linkc doctor`/`migrate --dry-run` pasaran limpios (esos dos nunca leen una fila real, solo metadata). Dos hipótesis descartadas en el camino antes de encontrar la real: un `DROP COLUMN` de la migración anterior dejando un hueco en `pg_attribute.attnum` (reproducido a mano, paso a paso, contra Postgres real -- NO reprodujo nada) y el orden de campos del `.link` (tampoco cambiaba el error). La causa real, aislada por skynet-43 con el DDL exacto de la tabla: dos columnas NATIVAS de Postgres sin mapeo binario real -- `source_ip inet` (mapeada a `String?`, exactamente como `linkc introspect` ya recomendaba) y `uuid uuid` (una columna legada, mapeada a `String` en vez de `Uuid` -- una decisión de modelado válida, GRAMMAR.md no exige usar `Uuid` para toda columna que se llame así).

**El mismo problema que UUID como PK (§3.177), generalizado: `uuid`/`inet`/`cidr` tienen formato binario propio, no texto UTF-8.** `postgres_string_cell` (`runtime/store.rs`, reemplaza el `try_get::<_, Option<String>>` directo que `ColumnKind::Text` usaba) prueba en orden -- mismo criterio que `postgres_int_cell`/`postgres_timestamp_cell`/`postgres_float_cell`: `String` primero (el caso normal, sin costo extra), después `PgUuidText` (reusa el MISMO decodificador que la PK `id: Uuid` ya tenía, GRAMMAR.md §3.177 -- un campo `String` normal contra una columna `uuid` nativa es el mismo problema, esté o no en la PK), y por último `PgInetText` (nuevo). La escritura (`Cell::to_sql`) gana los mismos dos casos, simétricamente -- `inet_string_to_binary` reusa `std::net::{Ipv4Addr,Ipv6Addr}` para el FORMATEO de texto (RFC 5952 correcto para IPv6 -- compresión de ceros -- gratis, sin reimplementarlo a mano) y solo escribe a mano el parseo del layout binario de Postgres en sí (`family`/`bits`/`is_cidr`/`longitud`/bytes de dirección -- protocolo fijo y documentado, mismo criterio de "sin dependencia nueva" que `PgTimestampMicros`/`PgNumeric`).

**`linkc introspect` (§3.66) sube `uuid` de `String` con advertencia a `Uuid` sin advertencia, e `inet`/`cidr` (antes en el catch-all genérico) a `String` sin advertencia** -- mapeos EXACTOS ahora, mismo criterio que `date`/`timestamp` (§3.91).

**Límite honesto, deliberado**: `cidr` comparte el mismo mecanismo binario que `inet` (Postgres los trata como el mismo formato de wire, la diferencia es de constraint -- `cidr` exige que los bits fuera de la máscara sean cero) -- c-script no distingue los dos como tipos separados, ni valida esa constraint del lado de la aplicación. Sin evidencia de demanda propia más allá de lo que ya resuelve `inet`.

**Verificado contra Postgres real** (`pg_integration.rs`) -- una tabla adoptada con `source_ip inet` real: lectura de una IP con valor y de una fila con `source_ip` NULL, más una escritura nueva confirmada con SQL crudo (`pg_typeof(source_ip) = 'inet'`, no un tipo forzado). Una tabla adoptada con una columna `uuid` nativa mapeada a `String` (no `Uuid`): lectura decodifica a la forma canónica de 36 caracteres. Más 8 tests unitarios locales (sin Postgres, la codificación/decodificación binaria en sí es lógica pura) sobre `inet_string_to_binary`/`PgInetText` -- IPv4 con y sin máscara, IPv6 con compresión de ceros (incluido `::1`), el layout de bytes exacto, y que un valor inválido se rechaza sin panic.

---

### 3.180 Compresión GZIP de la respuesta HTTP — RESUELTO

Segundo ítem de la ronda de "límites y fricciones" (junto con §3.178 rate limiting distribuido, ya resuelto arriba): un `rpc` que devuelve una lista larga o un `openapi.json`/contrato grande viajaba entero sin comprimir -- ancho de banda real desperdiciado, sobre todo cruzando el enlace (típicamente el más lento del camino) entre el backend y un browser o un cliente en otra región.

**Sin flag nuevo, sin anotación nueva -- transparente para el `.link` y para el cliente generado.** Negociación de contenido HTTP estándar (RFC 9110 §12.5.3): si la request trae `Accept-Encoding: gzip`, la respuesta viaja comprimida con el header `Content-Encoding: gzip`; si no, byte a byte igual que antes de esta ronda. `client.ts` (`fetch` del browser/Node) descomprime solo -- ningún cliente generado necesita cambio.

**Solo GZIP, no brotli.** `flate2` es la única dependencia nueva razonable acá; brotli necesitaría una segunda dependencia separada sin beneficio claro sobre gzip para el caso de uso de este proyecto (un backend que sirve JSON, no un CDN de assets estáticos). Alcance v0 deliberado -- sin evidencia de demanda real de un ratio de compresión mejor que justifique el peso extra.

**Umbral mínimo, no comprime cualquier cosa.** `GZIP_MIN_BODY_BYTES` (1024 bytes, mismo orden de magnitud que el `gzip_min_length` por default de nginx) -- un body chico (la mayoría de las respuestas de un CRUD típico) no se comprime aunque el cliente lo acepte: el propio overhead de GZIP (cabecera + checksum + tabla de Huffman) puede superar el ahorro real, y comprimir sin necesidad solo gasta CPU del lado del servidor. Un `stream` (SSE, §3.16) queda excluido de forma ESTRUCTURAL, no por un chequeo explícito: `write_stream`/`write_live_stream` escriben chunked transfer encoding a mano sobre el socket (ver el comentario en `runtime/server.rs` sobre por qué, sección "escrito a mano en vez de `tiny_http::Response`"), nunca pasan por `cors_response`/`cors_response_with_type` -- el único punto donde se decide comprimir.

**Implementación**: `cors_response_with_type` (el único punto de construcción de CUALQUIER respuesta no-stream, `runtime/server.rs`) revisa `Accept-Encoding` de la request (`accepts_gzip`) y, si el cliente lo acepta y el body supera el umbral, comprime con `flate2::write::GzEncoder` (nivel de compresión por default) antes de armar la respuesta -- `Response::from_data(bytes)` en vez de `Response::from_string(body)`. Los dos constructores de `tiny_http` 0.12 devuelven el MISMO tipo (`Response<Cursor<Vec<u8>>>`), así que ambas ramas (comprimida/sin comprimir) conviven en una sola variable sin duplicar el resto de la función -- headers de CORS, seguridad, `Location`, `Cache-Control`, todos idénticos a antes, se agregan DESPUÉS sin importar cuál rama corrió.

**`flate2`, segunda excepción real a "cero dependencias nuevas"** (la primera es `regex`, §3.73): a diferencia de UUID/HMAC-SHA256/el wire de `inet`/`timestamp`/`numeric` de Postgres (formatos binarios chicos y FIJOS, triviales de escanear a mano y ya hand-rolleados en este proyecto), GZIP/DEFLATE es un algoritmo de compresión real (LZ77 + Huffman) -- un encoder a mano, aunque técnicamente posible, es una superficie de bugs real (un stream corrupto silencioso es mucho peor que un 500 limpio) sin ningún beneficio sobre una implementación ya madura. Backend por default de `flate2` (`miniz_oxide`, Rust puro) -- sin `zlib-ng`/`cloudflare_zlib` ni ninguna dependencia C, mismo criterio de "sin tooling nuevo que instalar" que el resto del proyecto.

**Límite honesto, deliberado**: nivel de compresión fijo (el default de `flate2`, equivalente a nivel 6 de zlib -- buen balance velocidad/ratio), sin forma de configurarlo desde `linkc serve`. Sin soporte de `deflate` ni `br` (brotli) como alternativas -- un cliente que solo declara esas dos (sin `gzip`) recibe la respuesta sin comprimir, comportamiento correcto según la negociación de contenido (nunca manda un encoding que el cliente no pidió), aunque deja algo de ahorro posible sobre la mesa. Sin caché de la versión comprimida -- cada request que califica se recomprime desde cero; para el tamaño típico de una respuesta de este servidor (JSON, no archivos estáticos grandes) el costo de CPU es marginal, así que no se justificó la complejidad de cachear bytes comprimidos junto a (o en vez de) `@cache`/`@idempotent` (§3.140/§3.144).

**Verificado contra un `linkc serve` real** (`server_http.rs`, subprocess real + `TcpStream` real, mismo estilo que el resto del archivo): un `rpc` que devuelve un string de 2000 bytes con `Accept-Encoding: gzip` responde con `Content-Encoding: gzip` y un body que descomprime (`flate2::read::GzDecoder`) al JSON esperado byte a byte; el mismo `rpc` SIN ese header responde sin comprimir, sin el header `Content-Encoding`; un `rpc` que devuelve un string corto (bajo el umbral) con `Accept-Encoding: gzip` tampoco comprime -- las tres ramas de la lógica de negociación probadas contra el binario real, no solo unitariamente.

---

### 3.181 Camino de despliegue recomendado (git+CI) — RESUELTO, alcance acotado

Último ítem de la ronda de "límites y fricciones" que arrancó con el rate limiter distribuido (§3.178). Auditando qué falta para un camino git+CI recomendado apareció algo distinto de lo esperado: `linkc docker`/`linkc systemd`/`linkc pm2-config`/`linkc doctor`/`linkc migrate --dry-run` ya existían, todos piezas maduras -- el gap real no era herramienta de despliegue, era que NINGUNA de esas piezas estaba conectada en un pipeline recomendado, y que `docs/multi-service-deployment.md` (la guía que un operador leería primero) describía como "no existe todavía" tres cosas que en realidad ya habían enviado (`--host`, los generadores de systemd/pm2, `--restart-backoff`) -- deriva de documentación real, no solo un gap de features.

**`linkc new <nombre>` ahora scaffoldea `.github/workflows/deploy.yml`** en TODO proyecto nuevo (los tres templates -- minimal/nextjs/vite -- comparten el mismo archivo, es sobre el backend, no el frontend). Dos jobs con actitudes deliberadamente distintas:

- **`test-and-build`**: corre en TODO push a `main`, sin ningún secret -- `linkc test main.link` (comportamiento), `linkc build main.link gen` (regenera el contrato), y si existe `main.link.snap`, el MISMO chequeo de deriva de contrato que usa c-script consigo mismo contra su propio demo insignia (GRAMMAR.md §3.29). Da señal real desde el primer commit.
- **`deploy`**: apagado por default (`if: github.event_name == 'workflow_dispatch'`, solo corre por disparo manual) hasta que el operador configure 5 secrets documentados en el propio archivo y cambie esa línea a `if: github.ref == 'refs/heads/main'`. **Deliberado**: un proyecto recién scaffoldeado no tiene servidor real ni secrets todavía -- un workflow que intenta desplegar desde el primer push dejaría el badge de CI en rojo sin que ese rojo signifique nada real.

**El job `deploy`, cuando está activo, encadena piezas que YA EXISTÍAN, no agrega ninguna nueva**: `linkc doctor --db` (diagnóstico de solo lectura ANTES de tocar el servidor -- nunca DDL, un `doctor` en rojo frena el deploy sin haber arriesgado nada) → copiar el `.link` + reiniciar el servicio (`scp`/`ssh systemctl restart`, una variante concreta y fácil de reemplazar por `linkc docker`/PM2/lo que sea -- `linkc serve` re-migra el schema al arrancar, GRAMMAR.md §3.17, así que copiar y reiniciar alcanza) → `linkc doctor --target-url` (mismo diagnóstico, ahora comparando la versión local recién corrida contra la que reporta `/health` del servidor ya reiniciado -- un desfasaje queda como `[INFO]`, nunca bloquea el job, útil como confirmación visual de que el deploy actualizó el binario real).

**`docs/multi-service-deployment.md` corregido, no solo extendido**: tres afirmaciones "no existe todavía" que ya eran falsas -- `--host`/`--bind` (existe, GRAMMAR.md §3.81), un generador de systemd/pm2 (existen, `linkc systemd`/`linkc pm2-config`), backoff exponencial nativo (existe, `--restart-backoff`, GRAMMAR.md §3.92). Más grave: la premisa central de la guía ("no existe ningún modo workspace que sirva varios `.link` bajo un mismo proceso") tampoco era cierta -- `linkc serve-all` (GRAMMAR.md §3.92) hace exactamente eso desde hace rondas, sin que esta guía lo mencionara como alternativa en ningún lado salvo de pasada. Reescrito para presentar los dos caminos reales (`serve-all` un proceso para todos, o un `linkc serve`/unidad por servicio para aislamiento) en vez de uno solo presentado como si fuera el único posible.

**`docs/deploying-from-git.md` (nuevo)**: qué hace cada paso del workflow scaffoldeado y por qué, cómo activar el job `deploy`, la tabla de los 5 secrets. Referenciado desde la tabla resumen de `multi-service-deployment.md`.

**Límite honesto, deliberado**: el paso de despliegue en sí (`scp`/`ssh systemctl restart`) es UNA variante concreta, no una abstracción de "despliegue genérico" -- reemplazarla por Docker/Kubernetes/lo que sea es cosa del operador, el resto del workflow (tests, contrato, `doctor` antes/después) no cambia. Sin rollback automático -- si `doctor --target-url` muestra algo raro después de desplegar, el job simplemente termina; revertir sigue siendo manual, igual que cualquier despliegue por SSH simple. Ninguna herramienta nueva del lado de `linkc serve`/runtime -- alcance puramente de scaffolding + documentación, la pieza que faltaba no era código de servidor.

**Verificado**: 4 tests unitarios de `scaffold.rs` (incluido uno nuevo que confirma que los TRES templates scaffoldean el workflow, que referencia `main.link` de verdad, y que el job `deploy` queda apagado por default) más una corrida real de `linkc new` cuyo `.github/workflows/deploy.yml` resultante se validó con un parser YAML real (`yaml.safe_load`, no solo "compila el Rust que lo genera"). Suite completa sin regresiones.

---

### 3.182 Escritura de `Timestamp` contra `date`/`timestamp`/`timestamptz` NATIVOS de Postgres — RESUELTO

**Bug real de producción, severidad alta, reportado por skynet-43 (iaacademy) el mismo día que §3.181** -- silencioso, no un error: `insert`/`applyPatch` contra una columna `created_at timestamp with time zone` NATIVA (adoptada, no generada por `linkc build`) guardaba una fecha completamente distinta a la enviada, sin ningún error. Repro reportado: mandar `"2026-08-29T12:34:56.789Z"` terminaba guardado como `2000-01-21 16:40:06.896896` -- un salto de 26 años, sin ningún mensaje que lo señalara.

**Causa raíz, confirmada con aritmética antes de tocar código (no solo el síntoma):** `Cell::to_sql` (`runtime/store.rs`) nunca tuvo un caso para `ty == TIMESTAMP`/`TIMESTAMPTZ` -- un `Cell::Int(millis)` (la representación interna de `Type::Timestamp`, milisegundos desde 1970) caía al brazo genérico `_ => n.to_sql(ty, out)`, que serializa el i64 tal cual como `int8` (8 bytes crudos). Postgres interpreta esos MISMOS 8 bytes, para una columna `timestamp`/`timestamptz`, como microsegundos desde el epoch PROPIO de Postgres (2000-01-01) -- un formato binario del MISMO ANCHO, así que el servidor los acepta sin protestar, solo que con la semántica equivocada. Es el motivo exacto por el que este bug es más peligroso que el mismatch ya documentado de `numeric` (§3.103, "solo lectura"): `numeric` tiene un formato binario de ancho/forma DISTINTA, así que Postgres lo hubiera rechazado con un error claro -- acá, en cambio, el ancho coincide por pura coincidencia (los dos son enteros de 8 bytes), así que la escritura "funciona" y corrompe en silencio.

**La corrección** agrega dos casos nuevos a `Cell::to_sql`, simétricos a la lectura que §3.91 ya resolvía (`PgTimestampMicros`/`PgDateDays`, mismo módulo `timestamp.rs`): `pg_timestamp_micros_from_millis` (inversa EXACTA de `millis_from_pg_timestamp_micros`, para `TIMESTAMP`/`TIMESTAMPTZ`) y `pg_date_days_from_millis` (inversa de `millis_from_pg_date_days`, para `DATE` -- trunca cualquier componente de hora, mismo criterio que `timestamp::date` del propio Postgres). Mismo espíritu que el resto de este proyecto: un formato binario chico y documentado (8 y 4 bytes respectivamente, protocolo fijo) no amerita ninguna dependencia nueva.

**Límite honesto, deliberado**: el mismatch simétrico de `Float`/`numeric` (§3.103) sigue sin arreglar -- esta ronda solo cerró `Timestamp`, el reportado como bug real. La diferencia de riesgo justifica la diferencia de urgencia: `numeric` falla RUIDOSO (Postgres rechaza el formato), así que sigue siendo "no funciona todavía" en vez de "corrompe en silencio" -- documentado como tal desde §3.103, sin cambios acá.

**Verificado contra Postgres real** (`pg_integration.rs`): una tabla adoptada con columnas `date`/`timestamptz`/`timestamp` nativas, un `insert` real vía `linkc serve --adopt-existing`, y la prueba que de verdad importa -- leer la fila guardada con el cliente `postgres` CRUDO (`SELECT ...::text`, sin pasar por ningún decodificador propio de c-script) para confirmar que el AÑO real guardado es el correcto, no un artefacto de que lectura y escritura compartan el mismo bug compensándose entre sí. Más 5 tests unitarios locales sobre la aritmética de conversión (ida y vuelta exacta contra la lectura ya existente, el ancla pública conocida del epoch, y un test que documenta explícitamente el cálculo del bug -- "sin este fix, esto resolvería a enero de 2000").

---

### 3.183 `link.lock` como pin real de dependencias git + locking entre procesos — RESUELTO

Cierra los dos huecos reales que §2.1 dejaba abiertos en el package manager (`git+<url>#<rev>`, GRAMMAR.md §2.1): `link.lock` puramente informativo (nunca se leía para decidir qué commit usar) y sin ningún locking entre procesos concurrentes. Pedido explícito del usuario ("reforcemos git-as-registry" -- la decisión ya tomada en PLAN.md de NO construir un registro centralizado, ver el hilo completo en el propio §2.1) tras auditar qué le faltaba al mecanismo ya existente antes de considerar cerrado ese ítem.

**Bug real encontrado corriendo el código, no leyéndolo, ANTES de diseñar el pin:** una dependencia por RAMA (`git+<url>#main`, a diferencia de un tag o un commit SHA) quedaba silenciosamente CONGELADA en el commit del primer clone para siempre, contradiciendo la documentación existente ("se re-resuelve contra su HEAD real en cada build"). Dos causas, ambas en `gitdep::resolve`:
1. El chequeo "¿el rev ya resuelve localmente?" usaba `rev-parse --verify rev^{commit}` -- que para una RAMA ya resuelve contra `refs/heads/<rama>` LOCAL apenas se clona, así que el fetch de actualización nunca se disparaba en la práctica.
2. Incluso arreglando eso: `git fetch` por sí solo NUNCA mueve una rama LOCAL, solo su ref de SEGUIMIENTO remoto (`refs/remotes/origin/<rama>`) -- el `checkout <rev>` de después seguía resolviendo a la rama local vieja de todas formas (`gitrevisions(7)`: `refs/heads/` gana sobre `refs/remotes/` en la resolución por nombre corto).

Los dos confirmados con un repro real (clonar un repo git local, avanzar su rama `main`, resolver `main` de nuevo) ANTES de tocar código, no solo el síntoma. Fix: `is_full_commit_sha`/`refs/tags/<rev>` deciden cuándo confiar en el caché sin red (un SHA completo o un tag ya conocido son inmutables por definición/convención); cualquier otra cosa SIEMPRE fetchea, y el checkout final prefiere `refs/remotes/origin/<rev>` (recién actualizado) sobre `<rev>` a secas cuando ese ref de seguimiento existe.

**`link.lock` ahora es un pin real, mismo contrato que `Cargo.lock`/`package-lock.json`.** `modules::Loader` lee `link.lock` (perezoso, una sola vez por build, igual que ya hacía con `link.json`) y, si tiene una entrada `git_dependencies` para una dependencia cuyo `url`/`rev` siguen coincidiendo con lo que `link.json` pide AHORA, usa `gitdep::resolve_pinned` -- checkout directo al commit ya fijado, SIN volver a preguntarle nada al remoto sobre qué es "lo último" (ni siquiera el chequeo de arriba aplica: no hay ninguna ambigüedad de rama/tag que resolver, el pin YA ES el commit exacto). Solo toca la red si ese commit puntual no está todavía en el caché local (ej. un `.linkc/cache` que otra máquina nunca vio). Si `link.json` cambió el `url`/`rev` de esa dependencia desde que se grabó el pin, el pin queda OBSOLETO a propósito y se re-resuelve fresco -- mismo criterio que Cargo ante un `Cargo.toml` editado a mano.

**`linkc build --update-deps` es el único camino que avanza el pin.** Ignora cualquier entrada existente en `link.lock` para TODAS las dependencias git y fuerza una resolución fresca (con el fix de staleness de arriba ya aplicado, así que una rama pinneada de verdad avanza al commit real del remoto). `linkc serve`/`test`/`lsp`/`check` (vía `load_program_with_overlay`) siempre respetan el pin si existe -- ninguno de ellos escribe `link.lock`, y ninguno tiene un flag equivalente: correr `linkc build --update-deps` una vez y dejar que el resto de los comandos lean el `link.lock` ya actualizado es el flujo esperado, mismo patrón que `cargo update` seguido de `cargo build`/`cargo run` normal.

**Locking entre procesos concurrentes, resuelto con un mecanismo advisory, no un lock real de sistema operativo.** `CacheLock` (`gitdep.rs`) crea `<hash-de-la-url>.lock` de forma atómica (`create_new`) junto al directorio de caché de cada dependencia -- cualquier `resolve`/`resolve_pinned` concurrente que pise el mismo directorio espera (hasta 60s) en vez de correr en paralelo. Un lock más viejo que 120s se trata como abandonado (el proceso que lo tomó murió sin soltarlo) y se roba, en vez de bloquear para siempre. Deliberadamente NO un lock real de sistema operativo (`flock`/`LockFileEx`, que pediría FFI a mano en dos plataformas distintas o una dependencia nueva) -- un caso raro en la práctica no amerita ese costo, mismo criterio de "proporcional al riesgo real" que el resto del proyecto.

**Límite honesto, deliberado**: el registro centralizado en red (`npm`/`crates.io`-style) sigue explícitamente DESCARTADO -- esta ronda refuerza el modelo "git-as-registry" ya elegido (PLAN.md), no lo reemplaza. Sin auto-detección de si el pin quedó desactualizado respecto al remoto (ningún warning tipo "hay una versión nueva disponible") -- hay que correr `--update-deps` a mano para enterarse, mismo comportamiento que Cargo sin `cargo outdated` instalado.

**Verificado**: 6 tests nuevos en `gitdep.rs` (el repro exacto del bug de staleness antes/después del fix, `resolve_pinned` quedándose en el commit fijado aunque el remoto avance, fetch único cuando el commit fijado no está en caché local, y dos tests de `CacheLock` -- serializa acceso concurrente real con 8 hilos del sistema operativo, roba un lock abandonado en vez de bloquear para siempre) + 3 tests nuevos en `modules.rs` (el pin se mantiene tras un `load_program_full` repetido con la rama avanzada, `--update-deps` lo ignora y re-resuelve, un pin obsoleto -- `link.json` cambió el rev -- se ignora solo). Más una verificación manual de punta a punta contra el binario real (`linkc build` tres veces seguidas contra un repo git local: resuelve y pinnea, se queda pinneado con el remoto avanzado, `--update-deps` re-resuelve y el checker atrapa correctamente el tipo nuevo que el código local todavía no soporta). Suite completa sin regresiones.

### 3.184 `Decimal`: tipo numérico de precisión exacta (punto fijo, 4 decimales) — RESUELTO

PLAN.md §9.2 ítem 1: `Float` es una fuente de error de redondeo confirmada por adoptadores financieros reales -- columnas de dinero (`subtotal`/`descuento`/`total`/etc.) sufren la imprecisión binaria de IEEE754 tarde o temprano (`19.99 * 3` da `59.96999999999999...`, no `59.97`). Decisión de representación tomada explícitamente por el usuario tras ver el trade-off: **punto fijo, `i128` interno escalado ×10.000 (4 decimales fijos)**, no precisión variable estilo `numeric` nativo de Postgres.

**Construcción, sin sintaxis de literal nueva** -- mismo patrón que `Int64` (§3.28): un literal `19.99` sigue lexeando a `f64` sin cambios; `Decimal` se construye vía `.toDecimal()`, nunca un sufijo o prefijo de literal propio. `Int.toDecimal()` es exacto (`n * 10_000`); `Float.toDecimal()` redondea el f64 ya parseado al 4to decimal -- seguro en la práctica, porque la resolución de 4 decimales es mucho más gruesa que la precisión de f64 (~15-17 dígitos significativos) para cualquier magnitud financiera real. `Decimal.toFloat()`/`Decimal.toString()` para volver, siempre explícito. Deliberadamente SIN `.toInt()` -- truncaría en silencio.

**Aritmética**: `+`/`-` son suma/resta entera exacta sobre el valor escalado, `checked` (overflow da un `RuntimeError` limpio, nunca panic). `*`/`/` rescalan el resultado a 4 decimales con **redondeo half-up -- empate se aleja de cero** (`-2.5` redondea a `-3`, no a `-2`; mismo criterio que la mayoría del software financiero/comercial). `%` (`Rem`) queda **excluido a propósito**: el checker lo rechaza con un mensaje claro (`"Decimal no soporta '%'"`) en vez de dejarlo type-checkear sin semántica real en runtime. Comparaciones (`==`/`!=`/`<`/`<=`/`>`/`>=`) son comparación entera directa, exacta. `-` unario es `checked_neg`.

**Wire format**: string JSON con EXACTAMENTE 4 decimales siempre (`"1234.5600"`, nunca `"1234.56"` ni un `number` nativo -- un `number` de JS perdería exactitud en cualquier cliente). `contract.d.ts` tipa el campo como `string`, sin una clase `Decimal` de cliente propia (alcance mayor, fuera de v0). `openapi.json` emite `{"type":"string","format":"decimal"}` -- deliberadamente consistente con el wire real, a diferencia de `Int64` (que emite `format:"int64"` pese a viajar como string -- inconsistencia preexistente, no tocada esta ronda). Zod valida la forma exacta con un regex (signo opcional, dígitos, punto, exactamente 4 decimales).

**Almacenamiento SQLite**: `ColumnKind::Decimal` nueva, columna `INTEGER` con el valor ya escalado. Rango real ±~922.337.203.685.477,5807 (el de un i64 tras escalar ×10.000) -- de sobra para cualquier magnitud financiera real; un valor fuera de rango da un error claro al escribir, nunca un wrap silencioso.

**Almacenamiento Postgres**: `NUMERIC(38,4)` para una columna GENERADA por c-script. Una columna `numeric`/`decimal` YA EXISTENTE (adoptada vía `--adopt-existing`, el caso real de MyFinance) funciona igual -- un decodificador/codificador binario nuevo (`PgDecimal`/`decimal_scaled_to_pg_numeric_binary`, `runtime/store.rs`) extiende el formato que `PgNumeric` ya mapea para `Float` (§3.103: ndigits/weight/sign/dscale + dígitos base-10000), pero acumulando en `i128` en las dos direcciones, nunca tocando `f64`. Una columna adoptada con más de 4 decimales se redondea a 4 al leer, mismo criterio half-up.

**Agregaciones**: `sumBy`/`maxBy`/`minBy` soportados sobre un campo `Decimal`, con pushdown real a SQL (`SUM`/`MAX`/`MIN` nativos, exactos en los dos backends -- tanto sobre el `INTEGER` de SQLite como sobre el `NUMERIC` de Postgres). `maxRow`/`minRow` también aceptan `Decimal` como campo de orden (pura `ORDER BY`, sin cast ni agregación de por medio). `avgBy` queda EXCLUIDO a propósito -- ver límite honesto abajo.

**`@check(min/max/range, ...)`**: extendido a `Decimal`, mismo mecanismo que `Int`/`Int64`/`Float` ya tenían (§3.5).

<!-- linkc:check -->
```link
type Product = { id: Int, name: String, price: Decimal }
type NewProduct = { name: String, price: Decimal }
db { products: Product[] }

service Products {
  rpc create(name: String, price: Decimal) -> Product {
    db.products.insert(NewProduct { name: name, price: price })
  }
  rpc get(id: Int) -> Product? { db.products.find(id) }
  // 19.99 * 1.19 con Float arrastraría error binario -- acá es exacto.
  rpc priceWithTax(id: Int) -> Decimal? {
    match db.products.find(id) {
      p: Product => p.price * 1.19.toDecimal(),
      null => null,
    }
  }
  rpc totalCatalogValue() -> Decimal[] {
    db.products.sumBy(|p: Product| { p.name }, |p: Product| { p.price })
      .map(|g: {key: String, value: Decimal}| { g.value })
  }
}
```

**Bug real encontrado por CI, no localmente (v1.140.0 → v1.140.1): `match`/narrowing de un `Optional`/`Union` sobre un struct con un campo `Decimal` nunca matcheaba ningún arm.** `value_matches_type` (`runtime/mod.rs`, la función que un patrón `nombre: Tipo` usa en runtime para confirmar el shape real de un valor -- GRAMMAR.md §3.9) es un `match` sobre `Type` sin brazo para `Type::Decimal`, cayendo al fallback `_ => false`; eso hacía que CUALQUIER struct con un campo `Decimal` fallara `struct_matches_fields` para ESE campo, y por lo tanto el patrón entero. Alcanzado por un idiom común -- `match db.<c>.find(id) { fila: T => ..., null => ... }` -- sobre una colección con un campo `Decimal`: los dos tests nuevos que lo ejercitaban (uno de integración contra SQLite real que se salta el checker por diseño de su harness, así que compiló pero no corrió el camino real; uno contra Postgres real en CI, que sí pasa por `linkc build`/`linkc serve` de punta a punta) expusieron el mismo bug desde dos ángulos distintos. Arreglado con el mismo criterio que el resto de esta sección: un brazo explícito, `Type::Decimal => matches!(v, Value::Decimal(_))`, sin tocar el resto de la función.

**Límites honestos, deliberados**:
- Escala fija de 4 decimales, global -- no configurable por campo. Sin evidencia de demanda de otra escala todavía.
- **`avgBy` excluido sobre `Decimal` en v0.** Motivo: asimetría real de almacenamiento entre backends -- Postgres guarda el `NUMERIC` verdadero (`AVG` nativo, exacto), SQLite guarda el valor YA escalado ×10.000 como `INTEGER` (un `AVG()` de SQLite sobre esos enteros da un resultado de punto flotante que habría que reescalar y redondear sin acumular error -- ingeniería propia, no construida esta ronda). El checker rechaza `avgBy` sobre un campo `Decimal` con un mensaje claro, nunca un resultado silenciosamente incorrecto.
- `Float.toDecimal()` redondea el f64 ya parseado -- no es "el texto fuente exacto" para una magnitud patológicamente grande (mismo límite que `Int64` ya tiene por no tener sintaxis de literal dedicada).
- Rango real limitado por SQLite (±~922 billones tras escalar) -- Postgres `NUMERIC` no tiene ese límite, pero el límite del lenguaje es el más chico de los dos backends a propósito, mismo comportamiento en los dos motores.
- Escribir contra una columna `numeric(N,M)` adoptada con `M < 4` depende de la coerción de escala implícita del propio Postgres al guardar -- sin redondeo/validación propia de c-script en esa dirección (el redondeo propio solo cubre LEER una columna con más decimales que 4). Sin caso real reportado que lo justifique todavía.

**Verificado**: 6 tests nuevos en `checker.rs` (conversión ida y vuelta, receptor/argumentos inválidos en `.toDecimal()`, sin mezcla implícita con `Float`/`Int` en aritmética ni comparaciones, aritmética y comparaciones entre dos `Decimal`, `%` rechazado con mensaje claro, `@check(min/max/range)` sobre un campo `Decimal`). 13 tests nuevos en `runtime/mod.rs` (formateo/parseo del wire string, `div_round` con empates -- incluidos negativos -- y no-empates, aritmética checked de las 4 operaciones con overflow y división por cero limpios, construcción desde `Int`/`Float` con NaN/Infinity rechazados, más un test de integración de punta a punta contra SQLite real: insert/find/round-trip exacto/multiplicación calculada/`applyPatch`/`sumBy`). 8 tests nuevos en `runtime/store.rs` (ida y vuelta del codec binario `NUMERIC` de Postgres: un valor típico, cero con la convención de Postgres de cero dígitos, negativo, solo fracción, un entero que omite el dígito fraccionario cero, un entero grande multi-chunk, redondeo correcto al decodificar más de 4 decimales, NaN/Infinity rechazados limpio). 2 tests nuevos en `pg_integration.rs` contra Postgres real (columna `numeric(12,2)` ADOPTADA -- lectura y escritura confirmadas con SQL crudo `::text`, el caso real de MyFinance; una tabla GENERADA por c-script de punta a punta con `sumBy`/`maxRow`/`minRow` reales y el DDL confirmado como `NUMERIC(38,4)` real vía `information_schema`). Suite completa (1066 tests de biblioteca + toda la matriz de integración) sin regresiones.

---

### 3.185 `linkc db export`/`linkc db import` — RESUELTO PARCIAL

PLAN.md §9.7 ítem 2 pedía una suite completa (`inspect`/`shell`/`export`/`import`/`seed`) -- §3.175 ya había resuelto `inspect`; esta ronda cierra `export`/`import`, elegidos explícitamente por el usuario sobre `shell` (un REPL interactivo, mucho más difícil de verificar de forma no interactiva con la disciplina de este proyecto de correr el binario real). `seed` no necesita su propia pieza: importar contra un target vacío YA ES ese caso, mismo mecanismo -- así que esta ronda cierra 3 de los 4 ítems que quedaban. `shell` sigue pendiente, para una ronda aparte.

<!-- linkc:check -->
```link
type Item = { id: Int, name: String, price: Decimal }
type NewItem = { name: String, price: Decimal }
db { items: Item[] }
service Items {
  rpc add(name: String, price: Decimal) -> Item { db.items.insert(NewItem { name: name, price: price }) }
}
```

```
$ linkc db export app.link export.json --db app.db
linkc db export -- 'app.link' contra SQLite embebido en 'app.db' -> 'export.json'

  items  2 fila(s)

1 colección(es), 2 fila(s) en total exportadas

$ linkc db import app.link export.json --db staging.db
linkc db import -- 'export.json' contra SQLite embebido en 'staging.db'

  items  2 fila(s) importadas

1 colección(es), 2 fila(s) en total importadas
```

**`export`: lector propio, sin DDL, nunca `Db` completo.** `Db::new_with_options(..., adopt_existing=true)` -- la alternativa obvia para "conectar sin migrar" -- panickea apenas UNA colección declarada no tiene tabla física todavía (`check_schema_for_adoption`), exactamente el caso normal que §3.175 existe para tratar como estado normal, no error. Por eso `export` usa su propio lector (`src/db_admin.rs`, mismo espíritu que `inspect.rs`): SQLite abre `SQLITE_OPEN_READ_ONLY`, Postgres arma un `Backend` a mano (sin `Db`, sin hilo LISTEN/NOTIFY, sin tabla de rate-limit), y una tabla faltante es "0 filas", nunca un error. `select_rows` (el camino de `all()`) NO se reusa tal cual -- filtra `@softDelete`, correcto para un rpc normal, incorrecto para un backup/migración, que tiene que mover TODA fila física (mismo criterio que `db.tableStats()`/`db inspect`: verdad física, a propósito distinto de `count()`/`all()`).

**Decodificación de fila reusada, no duplicada.** El cuerpo de `Db::row_to_fields` se extrajo a una función libre, `pub(crate) fn decode_row(collection, cells, columns, id_kind, checker)` (`runtime/db.rs`) -- `Db::row_to_fields` quedó como wrapper de una línea sobre esto mismo. `export` la llama directo, sin necesitar un `Db`. Mismo movimiento para `id_column_kind` -> `id_column_kind_for(id_kind)`. `ColumnPlan`/`ColumnPlan::for_field`/`ColumnPlan::kind` pasaron de privado a `pub(crate)` -- mismo criterio ya usado para `sqlite_table_exists`/`existing_columns` al construir `inspect.rs`.

**Formato**: un solo JSON con TODAS las colecciones declaradas (arrays vacíos incluidos para las que no tienen tabla física todavía -- el archivo es una foto completa del `.link` actual, no solo lo que tenía datos). Cada fila = `value_to_json(&Value::Struct(decode_row(...)), &simple_enums)` -- la MISMA función que `db.<c>.all()` ya usa para responder por HTTP, byte-idéntica al wire real (Decimal como string de 4 decimales, Uuid canónico, Timestamp ISO-8601, todo gratis, sin encoding paralelo que pueda divergir). `linkc_version` es puramente informativo, nunca se compara contra el binario corriendo.

**`import`: conecta con `Db::new`/`connect_postgres_for_testing` NORMAL, nunca `adopt_existing` -- esto cubre "target vacío" (el caso `seed`) y "target ya servido antes" (cruce de entornos) con un solo camino.** `create_table_sql`/`create_postgres_table_sql` son `CREATE TABLE IF NOT EXISTS`; `check_schema_matches`/`alter_table_add_column_postgres` son no-ops reales contra un esquema que ya coincide. Un target vacío recibe su esquema y sigue derecho a los datos -- sin código separado para "seed".

**Escritura con id EXPLÍCITO -- mecanismo nuevo, solo Rust, nunca alcanzable desde `.link`.** `Db::import_row` (nuevo, `runtime/db.rs`) reusa `write_param`/`Cell`/`placeholder` de la rama `"insert"` existente, con dos diferencias: el id siempre lo trae el caller (nunca se genera) y no hay `RETURNING`/`last_insert_rowid()` que pedir. Decodificación CAMPO POR CAMPO, nunca vía `Type::Struct{name: Some(...)}` -- esto es lo que evita `@validate`/`@check` de nivel tipo a propósito (esa rama de `json_to_typed_value` solo dispara esos validadores cuando decodifica el struct NOMBRADO completo, el camino de borde para input de cliente no confiable). Exactamente equivalente, por construcción, a lo que `insertMany` ya prueba seguro (`Db::call(&coll, "insert", vec![Value::Struct(fields)])` con un struct ya armado). Las restricciones de BASE (`CHECK`/`UNIQUE` de la DDL) siguen aplicando siempre -- las impone el propio `INSERT`.

**Resincronización de secuencia post-import -- el gap real que un id explícito abre.** `Db::resync_id_sequence` (nuevo), corrido una vez por colección después de que todas sus filas aterrizaron, autocorrectivo (lee el `MAX("id")` físico). SQLite usa `INTEGER PRIMARY KEY AUTOINCREMENT`, que respeta `sqlite_sequence` incluso tras un DELETE -- sin resync, un `insert()` normal posterior podía chocar con un id importado; `UPDATE`/`INSERT` sobre `sqlite_sequence` según si ya había fila para esa tabla. Postgres usa `BIGSERIAL` -- `setval(pg_get_serial_sequence(...), max_id)`, resolviendo el nombre real de la secuencia en vez de hardcodear `"<tabla>_id_seq"`. Se salta para `IdKind::Uuid` (sin concepto de secuencia) y para una colección sin filas importadas. Corre DENTRO de la misma transacción que los inserts (`with_exclusive_connection`/`begin_transaction`/`commit_transaction`/`rollback_transaction`, mismas piezas que `transaction { }` usa).

**Choque de id -- falla todo y deshace, sin overwrite/skip en v0.** Un `INSERT` con un id que ya existe dispara la violación de PK del motor; todo el import se cancela y revierte -- mismo criterio de "fallar limpio y ruidoso" que `check_schema_for_adoption` ya establece. Una colección desconocida en el archivo (una key que el `.link` actual no declara) se valida ANTES de conectar siquiera con el target (no solo antes de escribir una fila) -- si el error saliera después de conectar, "nada se escribió" sería impreciso: `Db::new`/`connect_postgres_for_testing` ya corren su DDL idempotente al conectar.

**Límites honestos, deliberados**:
- Sin FK/referencias entre colecciones en el lenguaje -> `import` procesa cada colección de forma independiente, sin ningún orden de dependencia -- es responsabilidad del operador si su propio modelo de datos asume cierto orden.
- `@validate`/`@check` de nivel tipo del CUERPO de una colección se saltean a propósito en `import` (las restricciones de base -- `CHECK`/`UNIQUE` -- siguen activas siempre). Una restauración cruda de datos que ya eran válidos cuando se escribieron no debería bloquearse por un validador de flujo de trabajo específico de la app.
- Sin modo overwrite/skip ante un choque de id -- todo o nada. Un futuro `--overwrite`/`--skip-existing` queda aditivo, sin diseñar todavía.
- Una transacción para la corrida entera -- todo-o-nada por diseño, a costa de un candado exclusivo sostenido por toda la duración del import. Aceptable para una herramienta de CLI en lote; un límite real para un dataset gigantesco, sin commit por chunks.
- `linkc_version` en el archivo es puramente informativo, nunca se compara contra el binario corriendo.
- **`linkc db shell` sigue pendiente** -- PLAN.md §9.7 ítem 2, para una ronda aparte.

**Verificado**: 6 tests de CLI contra el binario real (`cli_db_export.rs` -- una `.db` inexistente exporta arrays vacíos, una base real poblada por `linkc serve` exporta filas físicas incluida una soft-deleted, una colección declarada pero nunca creada exporta vacío, el export calza byte a byte contra la respuesta RPC real de `all()`, casos de uso limpios) + 5 (`cli_db_import.rs` -- caso seed contra un target vacío con id original preservado y secuencia resincronizada confirmada vía un insert normal posterior, cruce de entornos idempotente sin perder filas previas, choque de id revierte TODO sin dejar nada a medias, colección desconocida no toca ni el archivo `.db`, caso de uso limpio) + 3 contra Postgres real en `pg_integration.rs` (round-trip completo con Decimal exacto, resync de secuencia real confirmado con un insert normal tras importar ids altos, colección con PK `Uuid` sin necesitar resync). Suite completa sin regresiones.

### 3.186 `builtin_args!`: fast-path para curar un builtin nuevo — RESUELTO (tooling interno, no una feature del lenguaje)

**Esta sección documenta el COMPILADOR, no el lenguaje -- un `.link` no cambia en absoluto acá: cero sintaxis nueva, cero tipo nuevo, cero builtin nuevo expuesto.** Se documenta igual, con el mismo criterio de rigor que cualquier otra sección, porque es una decisión de arquitectura real que afecta cómo crece la stdlib de acá en adelante.

PLAN.md §9.2 ítem 2, "Pilar 2" del roadmap de concurrencia (propuesto por skynet-d3 a nombre del usuario, 26/08/2026): "FFI seguro y tipado hacia crates de Rust", para no seguir construyendo cada primitiva nueva (HTTP, SMTP, S3, HMAC...) a mano una por una. Investigación previa (2 forks, código real antes de proponer nada) estableció que el pedido ORIGINAL -- exponer `crates.io` entero -- **no es viable con la arquitectura actual sin construir antes un sistema de macros/codegen completo** (`Value`/`Type` son enums Rust CERRADOS, matcheados exhaustivamente en el checker, el intérprete y los tres emisores de codegen; no hay `libloading`/`dlopen`/WASM-component en ningún lado) **y choca de frente con la política de "cero dependencias nuevas"** que este proyecto sostiene en 3 archivos distintos (solo `regex`/`flate2` tuvieron excepción, cada una justificada). Presentado esto al usuario, eligió explícitamente: no atacar FFI arbitrario, sino un fast-path para seguir curando builtins a mano, mucho más rápido de agregar -- sin exponer crates arbitrarios, sin tocar la filosofía actual.

**El problema real, medido**: cada builtin (`checker.rs::try_builtin_method`, ~74 arms entre `crypto`/`http`/`math`/`string`/`db`/`auth`/etc.) se define en DOS lugares que pueden desincronizarse a mano: un arm `(Type::X, "method") => {...}` en `checker.rs` (destructura N args exactos, un `check_expr` por cada uno, devuelve un `Type`) y un arm espejo `"method" => {...}` en `runtime/mod.rs::call_method` (la lógica real). El lado checker es **máximamente regular** -- los 51 arms de `crypto`/`http`/`math`/etc. siguen exactamente la misma forma, ninguno tiene aridad opcional/variable. El lado runtime NO es regular (20-30+ líneas de lógica real, llamadas a crates externos) -- no se puede generar, sigue escrito a mano.

```
macro_rules! builtin_args {
    ($self:ident, $args:ident, $env:ident, $qualified_name:literal, [$(($pname:ident, $pdesc:literal, $pty:expr)),+ $(,)?] -> $ret:expr) => {{
        let [$($pname),+] = $args else {
            let n = 0usize $(+ { let _ = stringify!($pname); 1 })+;
            let word = if n == 1 { "argumento" } else { "argumentos" };
            let desc = [$($pdesc),+].join(", ");
            return Err(err(format!("'{}' toma exactamente {n} {word} ({desc})", $qualified_name)));
        };
        $($self.check_expr($pname, &$pty, $env)?;)+
        Some($ret)
    }};
}
```

Uso -- reemplaza un arm de 5-7 líneas por 1:

```
(Type::Crypto, "hashPassword") => builtin_args!(
    self, args, env, "crypto.hashPassword",
    [(pwd, "password: String", Type::String)] -> Type::String
),
```

**Requiere al menos 1 argumento (repetición `+`, nunca `*`).** Un `+` fallaba con `*` -- encontrado en una revisión adversarial del propio diseño ANTES de escribir el código de verdad, no en el compilador: con `*`, un builtin de 0 args expandiría `[$($pdesc),*].join(", ")` a `[].join(", ")`, que no compila (`E0282`, un array vacío no tiene forma de inferir su tipo de elemento). Un builtin de 0 args (`crypto.uuid`, el único caso hoy) queda deliberadamente FUERA del alcance de este macro -- sigue con `expect_no_args` de siempre, que además tiene su propia frase ("no toma argumentos", distinta de "toma exactamente 0 argumentos ()") -- unificar los dos casos no vale la complejidad para un solo builtin existente.

**El lado runtime (`call_method`) NO se toca, a propósito.** No hay forma sensata de generar 20-30 líneas de lógica real variable (Argon2, HTTP, HMAC...) -- intentarlo escondería la lógica de negocio detrás de una capa sin aportar nada. Este macro solo ataca el lado que de verdad es mecánico: el checker nunca tiene lógica propia, solo tipa.

**Alcance v0: solo para builtins NUEVOS de acá en adelante, no un retrofit de los 74 existentes.** Retrofitear todo sería el cambio mecánico grande y riesgoso que el propio research desaconsejó. Como prueba de que el macro produce EXACTAMENTE el mismo comportamiento (mensaje de error incluido) antes de confiar en él para algo nuevo, se retrofitearon 2 arms existentes con argumentos de verdad -- `crypto.hashPassword` (1 arg) y `crypto.randomInt` (2 args, cubre la concordancia plural del mensaje que el de 1 arg no ejercita) -- verificado con tests nuevos que comparan el mensaje de error CARÁCTER A CARÁCTER contra el texto original (no existía cobertura de esto antes: los tests previos de estos dos builtins solo ejercitaban el camino feliz vía el intérprete, nunca el mensaje de aridad incorrecta del checker -- un hueco real, encontrado implementando esta ronda, cerrado de paso). Los otros 72 builtins quedan con su forma actual, sin urgencia de migrarlos.

**Definido a nivel de módulo, antes de `impl Checker`, no como item adentro del `impl`** -- sin precedente en este código de un `macro_rules!` a nivel de item de un `impl` (el único macro previo, `runtime/server.rs:549`, es local al cuerpo de una función), y evitarlo no cuesta nada.

**Límites honestos, deliberados**:
- No ataca FFI arbitrario -- decisión explícita del usuario, no un recorte de alcance no comunicado.
- Solo cubre el lado CHECKER (tipado). El lado runtime sigue 100% a mano.
- No hay ningún mecanismo automático que impida que alguien agregue un builtin nuevo sin usar el macro -- sigue siendo una convención (documentada en AGENTS.md), no algo forzado por el compilador. El macro hace el camino fácil más corto que el camino manual, pero no lo prohíbe.

**Verificado**: 4 tests nuevos en `checker.rs` (caso feliz + mensaje de aridad exacto, para `hashPassword` y `randomInt` cada uno) + los tests de comportamiento ya existentes de `crypto.hashPassword`/`crypto.randomInt` (camino feliz vía el intérprete) sin modificar, siguen pasando. Verificación manual contra el binario real: un programa `.link` con `crypto.randomInt(1)` (aridad incorrecta) da el mismo mensaje de error de siempre, palabra por palabra. Suite completa sin regresiones.

### 3.187 `String` contra `json`/`jsonb` NATIVOS de Postgres — RESUELTO

Bug real de producción, severidad alta, reportado por skynet-43 (iaacademy) el mismo día que §3.186: una columna `jsonb` NATIVA adoptada (`properties` en `data_user_events`, una tabla de analíticas), mapeada a `String?` -- exactamente la forma que `linkc introspect` ya recomienda para JSON sin tipo propio declarado (§3.66) -- fallaba SIEMPRE al escribir con `"error deserializing column N"`, la fila nunca se insertaba, CON o SIN valor (`null` fallaba igual). Confirmado en Postgres directo que la fila nunca llegaba a existir, no era solo un fallo al armar la respuesta. Impacto real: ~2-3 minutos con el endpoint público de analíticas del sitio devolviendo 500 a todo visitante, antes de revertir a SQL crudo.

**Mismo problema que UUID/inet (§3.177/§3.179), la siguiente columna nativa con formato binario propio.** La causa exacta, confirmada leyendo el código FUENTE de la crate `postgres-types` (no solo su documentación): `String::accepts` solo acepta `VARCHAR`/`TEXT`/`BPCHAR`/`NAME`/`UNKNOWN` -- ni `json` ni `jsonb` están en esa lista, así que un `Cell::Text` normal se rechaza antes de siquiera mirar los bytes, tanto al leer como al escribir. Explica por qué `null` fallaba igual que un valor real: el rechazo pasa por tipo de columna, nunca llega a importar qué valor se estaba mandando.

**Los dos formatos binarios reales, verificados contra el código fuente de `postgres-types` (`Json<T>::to_sql`/`from_sql`), no adivinados:** `json` es el texto UTF-8 crudo, sin envoltorio -- el mismo wire que `TEXT`. `jsonb` antepone UN byte de versión (`0x01`, la única versión que el protocolo define hoy) antes del mismo texto -- reparsear/reserializar internamente es cosa del SERVIDOR, el cliente solo manda texto JSON con ese prefijo. `PgJsonText` (`runtime/store.rs`) decodifica los dos casos (despacha por `ty`, quita el byte de versión solo para `jsonb`); `postgres_string_cell` lo prueba al final de la cadena existente (`String` -> `PgUuidText` -> `PgInetText` -> `PgJsonText`). La escritura (`Cell::to_sql`) gana los mismos dos casos, simétricamente -- antepone el byte de versión para `jsonb`, nada para `json`. Postgres mismo valida que el texto sea JSON bien formado al escribir -- sin validación propia de c-script en esa dirección, mismo criterio que el resto de este archivo (la base hace cumplir su propio tipo).

**Límite honesto, deliberado**: sin cambios a `linkc introspect` -- su advertencia existente para `json`/`jsonb` ("la forma real del JSON no se puede inferir de `information_schema`; declará un `type` propio si corresponde") sigue siendo consejo válido, es sobre MODELADO (¿`String` genérico o un `type` con la forma real?), no sobre si `String` funciona -- ahora sí funciona, de punta a punta.

**Verificado contra Postgres real** (`pg_integration.rs`): una tabla adoptada con `properties jsonb`, el repro exacto reportado -- lectura de una fila con contenido real y de una con `NULL`, escritura de un valor nuevo Y de `null` (los dos fallaban igual antes del fix), confirmado con un operador `jsonb` real (`properties->>'amount'`) que solo funciona si de verdad se guardó como `jsonb`, no forzado a otro tipo. Segundo test aparte para `json` (no `jsonb`) -- formato binario distinto, confirma que el fix distingue los dos casos. Más 6 tests unitarios locales (sin Postgres, la decodificación binaria en sí es lógica pura): el byte de versión de `jsonb` se quita correctamente, `json` no tiene ningún byte de más, una versión de encoding desconocida se rechaza limpio (no panic), un payload truncado (vacío) se rechaza limpio, y `accepts` solo es cierto para `json`/`jsonb`, nunca para `TEXT` plano.

## 4. Tabla de Mapeo c-script → TypeScript (exhaustiva)

| Construcción c-script | TypeScript emitido | Forma JSON en el cable | Nota |
|---|---|---|---|
| `Int`, `Float` | `number` | número | — |
| `Int64` | `bigint` | string en el wire (decimal, ej. `"9223372036854775807"`), revivido a `bigint` real del lado TS | mismo rango `i64` que `Int` -- el WIRE sigue siendo string (§3.30) para no perder precisión, pero `client.ts` ahora lo revive a `bigint` real (§3.156). `.toInt64()`/`.toInt()` para convertir; sin mezcla implícita con `Int` |
| `Timestamp` | `string` | string ISO-8601 de forma fija, ej. `"2026-08-08T14:30:00.000Z"` | milisegundos desde epoch UTC internamente -- ver §3.31. Obtenible con `now() -> Timestamp` (§3.32) o `dateFromParts(...) -> Timestamp` (§3.90). Solo comparable (`< <= > >= == !=`); sin aritmética |
| `now()` | `now(): Timestamp` | `"2026-08-15T12:00:00.000Z"` | función builtin de fecha y hora actual en UTC (§3.32) |
| `dateFromParts(year, month, day, hour, minute, second)` | `dateFromParts(year: number, month: number, day: number, hour: number, minute: number, second: number): Timestamp` | `"2026-07-01T00:00:00.000Z"` | función builtin que construye un `Timestamp` arbitrario a partir de sus componentes de calendario; `bad_request` (400) si la fecha no existe (§3.90) |
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
