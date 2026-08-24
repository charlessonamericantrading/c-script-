# Integrar un servicio `.link` desde afuera

[AGENTS.md](../AGENTS.md) está escrito para quien desarrolla o extiende un
`.link` -- esta guía es para el otro lado: una app (Node, Python, lo que
sea) que llama a uno o más servicios `linkc serve` ya generados, sin tocar
su código fuente. Si estás integrando servicios `c-script` desde una app
existente, esto es lo que importa.

## Qué te da `linkc build`, y qué usar de eso

```bash
linkc build servicio.link gen
```

genera, en `gen/`:

| Archivo | Para qué |
|---|---|
| `contract.d.ts` | Los tipos TypeScript del servicio -- fuente de verdad para lo que existe |
| `client.ts` | Cliente RPC tipado, ya armado para llamar al servicio real -- **usá este, no `fetch` a mano** |
| `validators.ts` | Valida cada respuesta en runtime contra su tipo declarado -- ya integrado en `client.ts` |
| `hooks.ts` | Hooks de React (si tu consumidor es un frontend React) |
| `schemas.ts` | Los mismos tipos como esquemas Zod, para validar en cualquier punto propio |
| `openapi.json` | Spec OpenAPI 3.1, útil si tu consumidor NO es TypeScript |

Si tu app consumidora es TypeScript, `client.ts` ya resuelve la llamada
HTTP, el parseo, y la validación de la respuesta -- no hay ninguna ventaja
en reimplementarlo a mano, y perdés la garantía de que un campo renombrado
en el servicio te rompa el build del lado consumidor en vez de fallar en
producción (la razón de ser de todo el proyecto). Si tu app consumidora NO
es TypeScript, `openapi.json` es el punto de partida para generar un
cliente en tu propio lenguaje con la herramienta que ya uses.

## Cómo vienen los errores

Toda respuesta de error -- sin excepción, incluida una que nunca llegó a
ejecutar tu `rpc` -- es JSON con la forma `{"error": "<mensaje>"}`. El
status HTTP dice la categoría:

| Status | Cuándo |
|---|---|
| `400` | La URL no tiene la forma `/Servicio/rpc`, o un argumento no matchea el tipo declarado |
| `401` | El rpc requiere autenticación (`@authenticated`/`@requires`) y no viniste con un token válido |
| `403` | Autenticado, pero con un rol que no tiene permiso para ese rpc |
| `404` | Ruta no encontrada (incluida una `@route` que no matchea nada) |
| `429` | `@rate_limit` del rpc, superado |
| `5xx` | Un error real del `rpc` (un `panic`, un error de base de datos, etc.) |

`client.ts` ya distingue estos casos por vos -- si tu app consumidora
resuelve la llamada a mano (sin `client.ts`), tratá cualquier cosa que no
sea 2xx como `{"error": string}`, nunca asumas que el body tiene la forma
de una respuesta exitosa.

## Autenticación: un solo header, dos orígenes posibles

Todo lo que autentica una request -- una sesión nativa de Link
(`auth.createSession`) o un JWT externo (`--jwt-secret`, GRAMMAR.md
[§3.64](../GRAMMAR.md#364-auth-externo-confiar-en-un-jwt-ya-emitido--resuelto-alcance-acotado-hs256))
-- viaja en el MISMO header:

```
Authorization: Bearer <token>
```

Si tu sistema ya emite sus propios JWT (HS256) y el servicio `.link` se
arrancó con `--jwt-secret` apuntando al mismo secreto, tu app consumidora
puede reusar el token que ya tiene -- no hace falta un login separado
contra el servicio `.link`. Ver
[docs/incremental-adoption.md](incremental-adoption.md) para el patrón
completo.

**Llamadas servidor-a-servidor**: hoy no existe un mecanismo de API keys
separado de las sesiones de usuario (PLAN.md §9.5) -- si tu app llama a un
`linkc serve` desde su propio backend (no en nombre de un usuario logueado),
las opciones reales hoy son: (a) un JWT propio con un rol de servicio, si
`--jwt-secret` está configurado, o (b) confiar en que la red no es
alcanzable desde afuera (mismo host, o una red privada) y no autenticar esa
llamada en absoluto -- la opción real que muchos equipos usan hoy, con sus
límites reales: cualquiera con acceso a esa red puede llamar sin
credenciales.

## Health check: `/`, `/health` y `/status` responden lo mismo

```json
{"status": "ok", "engine": "c-script", "version": "1.33.0", "services": ["Auth", "Billing"]}
```

Siempre `200` si el proceso está vivo -- **no verifica conectividad a la
base de datos ni a ningún servicio externo declarado** (PLAN.md §9.8, un
health check configurable queda para una ronda futura). Si tu orquestador
necesita saber que la base responde de verdad, no alcanza con pegarle a
este endpoint -- hoy hace falta un `rpc` propio que haga una lectura real
(`db.<c>.count()`, por ejemplo) y comprobar que responde 200. El campo
`version` es la versión real del binario `linkc` corriendo -- útil para
confirmar qué versión sirve cada servicio cuando conviven generados en
momentos distintos.

## CORS: solo hace falta si un navegador llama directo

Si tu app consumidora corre en un SERVIDOR (Node, un backend), CORS no
aplica -- es una restricción del navegador, no del protocolo HTTP. Si en
cambio un FRONTEND en el navegador llama directo al servicio `.link` (sin
pasar por tu propio backend como proxy), el servicio necesita
`--cors-origin <tu-origen>` (GRAMMAR.md
[§3.41](../GRAMMAR.md#341-cors-configurable-y-headers-de-seguridad--resuelto-alcance-acotado))
o las llamadas se van a rechazar en el navegador aunque el servidor
responda bien.

## Qué NO asumir

- **Que reintentar automáticamente es seguro.** No hay idempotency keys
  nativas (PLAN.md §9.3) -- reintentar un `create` a ciegas puede duplicar
  la fila. Si tu app consumidora reintenta, la comprobación de "¿ya se
  aplicó?" es responsabilidad tuya, no del servicio.
- **Que el servicio tiene un timeout de escritura lento.** `smtp`/`http`
  salientes desde dentro de un `rpc` son sincrónicos hoy (PLAN.md §9.4/§9.6)
  -- una llamada externa lenta hace lenta a la respuesta completa de ESE
  rpc, no algo que el servicio resuelva por vos con un timeout propio.
- **Que un 200 significa que TODO el batch se aplicó**, si mandaste varias
  operaciones -- no hay ningún endpoint de batch a nivel de transporte hoy
  (PLAN.md §9.3); cada llamada a un `rpc` es su propia unidad.
