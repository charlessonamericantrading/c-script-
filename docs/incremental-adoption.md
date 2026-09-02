# Adopción incremental: convivir con una base de código que no es c-script

Para el caso real de migrar un backend existente (Express/Fastify/NestJS,
o cualquier otro) servicio por servicio, no reescribiéndolo todo de una vez.
No hay ningún modo especial de "migración" en `linkc` -- esta guía es sobre
cómo combinar features que ya existen para que la convivencia sea segura.

## El patrón: un servicio nuevo a la vez, detrás del mismo gateway

1. Elegí UN servicio o endpoint del backend viejo -- preferentemente uno
   con lectura/escritura acotada, no el núcleo de todo el sistema.
2. Si ese servicio ya tiene su propia tabla en una base Postgres existente,
   generá el `.link` de partida con `linkc introspect` (GRAMMAR.md
   [§3.66](../GRAMMAR.md#366-linkc-introspect-generar-un-link-desde-una-base-postgresql-existente--resuelto-alcance-acotado))
   en vez de escribir cada `type`/`db {...}` a mano mirando el schema en
   otra ventana.
3. Arrancá el nuevo servicio con `--adopt-existing`/`LINK_ADOPT_EXISTING`
   (GRAMMAR.md [§3.67](../GRAMMAR.md#367---adopt-existing-adoptar-tablas-sin-auto-migrar--resuelto))
   MIENTRAS el backend viejo siga siendo dueño de esa tabla -- esto
   garantiza que `linkc serve` nunca va a ejecutar `CREATE TABLE`/`ALTER
   TABLE` sobre datos que el sistema viejo todavía gestiona activamente, ni
   por accidente.
4. Poné un reverse proxy (o el gateway que ya tengas) adelante de los dos
   backends, y mové SOLO las rutas de ese servicio hacia `linkc serve` --
   mismo patrón que [multi-service-deployment.md](multi-service-deployment.md),
   con el backend viejo como "un servicio más" detrás del mismo proxy.
5. Repetí con el siguiente servicio cuando el primero esté estable.

```nginx
server {
    listen 443 ssl;
    server_name api.miapp.com;

    # Ya migrado a c-script.
    location /billing/ {
        proxy_pass http://127.0.0.1:8782/;
    }

    # Todavía en el backend viejo -- sin tocar.
    location / {
        proxy_pass http://127.0.0.1:3000/;
    }
}
```

## Autenticación: no obligues a un usuario a loguearse de nuevo

Si el backend viejo ya emite sus propios tokens de sesión (JWT o cookie),
el servicio nuevo puede verificarlos directamente en vez de reimplementar
login desde cero -- `linkc serve --jwt-secret <secreto>` (GRAMMAR.md
[§3.64](../GRAMMAR.md#364-auth-externo-confiar-en-un-jwt-ya-emitido--resuelto-alcance-acotado-hs256))
verifica un JWT HS256 ya emitido por un sistema externo, junto con (nunca en
vez de) las sesiones nativas de Link. `@requires`/`@authenticated` funcionan
igual sin que el `rpc` necesite saber cuál de los dos autenticó la request.
Solo HS256 -- si el sistema viejo firma con RS256/JWKS (típico de un
proveedor de identidad completo tipo Auth0/Cognito), ese puente todavía no
existe (PLAN.md, fuera de esta ronda).

Si el frontend llama al nuevo servicio directo (no solo a través del backend
viejo), configurá el mismo allowlist de CORS que ya tiene el sistema
existente (`--cors-origin`, GRAMMAR.md
[§3.41](../GRAMMAR.md#341-cors-configurable-y-headers-de-seguridad--resuelto-alcance-acotado))
para que el navegador no rechace la llamada.

## Verificar que el servicio nuevo responde igual que el viejo, antes de cortar tráfico real

No hay ningún modo "shadow" nativo en `linkc serve` (correr en paralelo,
comparar respuestas automáticamente, sin servir tráfico real) -- es una
idea real pedida en un reporte de adopción, pero no una feature hoy. La
forma de conseguir el mismo resultado con lo que ya existe:

1. Corré el servicio nuevo en un puerto propio, sin ponerlo detrás del
   proxy todavía.
2. Escribí un script (fuera de `linkc`, en lo que ya uses para testing)
   que mande la MISMA request a los dos backends y compare las respuestas
   -- reusando datos de producción reales o un dataset de prueba
   representativo.
3. Recién cuando las respuestas coincidan de forma consistente, movés la
   ruta correspondiente en el proxy del backend viejo al nuevo.

`linkc test` (`test "..." { assert(...) }`, con una base de datos aislada
por test) sirve para fijar el comportamiento ESPERADO del servicio nuevo de
forma reproducible, pero no reemplaza comparar contra el sistema viejo con
datos reales -- son dos capas de verificación distintas, ambas necesarias.


## Escrituras que siguen haciéndose desde el backend viejo

Mientras dura la migración, el backend viejo sigue escribiendo en las mismas
tablas que un servicio `.link` sirve por `stream`. Sin nada más, esas
escrituras son invisibles para `db.<c>.subscribe()`: LISTEN/NOTIFY
(GRAMMAR.md §3.44) solo propaga lo que escribe otro `linkc serve`. No hagas
un "republish" por HTTP desde el backend viejo tras cada escritura -- es la
pieza más frágil que un reporte de adopción real llegó a construir. En su
lugar, `linkc triggers app.link` imprime el DDL de PostgreSQL (idempotente,
revisable, aplicable con tu herramienta de migraciones de siempre) que hace
que cada escritura de cualquier origen -- un ORM, `psql`, un job -- llegue a
los `stream` conectados, al COMMIT y sin tocar el código viejo. Ver
GRAMMAR.md §3.225.
## Rollback: el proxy es el interruptor

Como el backend viejo sigue existiendo mientras dura la migración, volver
atrás ante un problema es cambiar la ruta en el proxy de vuelta al puerto
viejo -- no un rollback de base de datos ni un redeploy del servicio nuevo.
Esto es la razón real para NO borrar/deprecar el código viejo hasta que el
servicio nuevo lleve un tiempo probado en producción, no solo cautela
genérica.

## Cuándo el servicio nuevo puede necesitar su PROPIA base

Si el servicio que estás migrando no comparte datos con el resto del
sistema (un servicio nuevo de cero, o uno que puede tener su propia tabla
sin que el backend viejo la toque), no hace falta `--adopt-existing` -- el
`.link` puede declarar su propio schema y dejar que `linkc serve` lo cree
y gestione, sin ninguna dependencia del sistema viejo. Reservá
`--adopt-existing` para el caso real de convivir sobre datos que otro
sistema sigue escribiendo.
