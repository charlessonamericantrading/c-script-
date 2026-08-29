# Desplegar muchos servicios `.link` en un mismo host

Escrito para el caso real: una migración de un monolito a N servicios
`c-script` (una migración real reportó 17), todos corriendo en el mismo
servidor. El README cubre bien UN servicio suelto; esta guía cubre el resto.

**Dos caminos, según cuánto aislamiento querés entre servicios**: `linkc
serve-all <directorio> --port-base N` (GRAMMAR.md §3.92) sirve TODOS los
`.link` de un directorio bajo un ÚNICO proceso -- cada uno en su propio
hilo, puerto `N`/`N+1`/`N+2`/... en orden alfabético, cada uno con su propio
archivo SQLite. Un solo proceso para supervisar, un solo binario que
actualizar -- la opción más simple si tus servicios no necesitan estar
completamente aislados entre sí (un panic o una fuga de memoria en un hilo
no debería tumbar el proceso entero en el uso normal, pero comparten
recursos del sistema operativo de todas formas). El resto de esta guía
también aplica sirviendo cada `.link` con su propio `linkc serve` --
procesos separados, más aislamiento (un `systemctl restart` de un servicio
nunca toca a los demás), a costa de un proceso/unidad por servicio en vez
de uno solo. **Límite honesto**: no hay hoy un puerto ÚNICO compartido por
varios servicios (ni con `serve-all` ni sin él) -- cada `.link` sigue
necesitando su propio puerto, el proxy de la sección 1 es lo que da una
sola cara pública hacia afuera.

## 1. Un puerto por servicio, un proxy adelante

```bash
linkc serve auth.link      8781 --db postgres://... --host 127.0.0.1
linkc serve billing.link   8782 --db postgres://... --host 127.0.0.1
linkc serve notifications.link 8783 --db postgres://... --host 127.0.0.1
# ...uno por servicio
```

Un reverse proxy (nginx, Caddy) resuelve el ruteo hacia afuera -- un dominio
o path público por servicio, sin que ninguno de los `linkc serve` necesite
saber de los demás. Mismo patrón ya documentado para `@route` +
proxy ([docs/routing.md](routing.md)), aplicado ahora a nivel de servicio
completo en vez de una sola ruta:

```nginx
server {
    listen 443 ssl;
    server_name api.miapp.com;

    location /auth/ {
        proxy_pass http://127.0.0.1:8781/;
    }
    location /billing/ {
        proxy_pass http://127.0.0.1:8782/;
    }
    location /notifications/ {
        proxy_pass http://127.0.0.1:8783/;
    }
}
```

**Por qué no exponer cada puerto directo a Internet**: `--host 127.0.0.1`
(o `LINK_HOST`, GRAMMAR.md §3.81) limita cada `linkc serve` a aceptar solo
conexiones locales -- el proxy corre en la misma máquina y les habla por
`127.0.0.1:puerto`, así que nada externo puede llegar a un servicio salteando
el proxy. Sumale un firewall a nivel de sistema operativo (`ufw`/`iptables`,
deny-by-default, solo el puerto del proxy abierto hacia afuera) como
defensa en profundidad -- `--host 127.0.0.1` ya cierra el gap del lado de la
aplicación, pero un firewall cubre cualquier otro proceso/puerto que termine
corriendo en el mismo host más adelante.

## 2. Supervisión de proceso

Con `linkc serve-all`, esto es UNA unidad/config, no una por servicio -- un
solo `systemctl restart miapp` (o una sola entrada de PM2) reinicia el
proceso que sirve TODOS los `.link` del directorio a la vez. El resto de
esta sección asume el otro camino (aislamiento por proceso, la sección 1
de arriba lo explica).

`linkc docker` ya genera un `Dockerfile` por servicio (`linkc docker
<archivo> -o Dockerfile`) -- la ruta más simple si ya usás contenedores: un
contenedor por servicio, orquestado con `docker compose` o lo que ya uses.

Sin contenedores, `linkc systemd <archivo> <puerto> [outdir]` y `linkc
pm2-config <archivo> <puerto> [-o <archivo>]` generan la unidad/config REAL
por servicio -- una por servicio, sin escribirla a mano:

```bash
linkc systemd auth.link 8781      # -> auth.service, listo para /etc/systemd/system/
linkc systemd billing.link 8782   # -> billing.service
linkc pm2-config auth.link 8781 -o ecosystem.auth.json
linkc pm2-config billing.link 8782 -o ecosystem.billing.json
```

Cada `.service` generado ya trae `ExecStart`, `WorkingDirectory`,
`Restart=on-failure`+`RestartSec`, la variable `LINK_DATABASE_URL` comentada
como referencia (nunca un valor real -- eso queda para el operador, no para
un archivo generado), y hardening mínimo (`NoNewPrivileges`,
`ProtectSystem=strict`, `ReadWritePaths`, `PrivateTmp`). Cada
`ecosystem.json` de PM2 trae `--restart-backoff 30s` ya incluido en `args`
y `autorestart: true` del lado de PM2 -- las dos capas son complementarias,
no redundantes: una reinicia el PROCESO (PM2/systemd), la otra espera antes
de reintentar la CONEXIÓN (ver el párrafo siguiente).

**Arranque en frío de muchos procesos a la vez**: si los N servicios
arrancan simultáneamente (ej. un reinicio del host), un bind de puerto que
falla momentáneamente (`Address already in use` durante un reinicio previo
todavía liberando el puerto) puede producir una ráfaga de reintentos --
`--restart-backoff <duración>`/`LINK_RESTART_BACKOFF` (GRAMMAR.md §3.92,
funciona en `linkc serve` y en `linkc serve-all`) agrega backoff
exponencial NATIVO ante ese fallo (dobla en cada intento consecutivo, techo
30s, se resetea tras 60s estable) -- ya viene incluido en el `ecosystem.json`
que genera `linkc pm2-config`; para una unidad systemd escrita por
`linkc systemd`, agregalo a mano al `ExecStart` si tu caso lo necesita.

## 3. Si varios servicios comparten una base de datos: cuidado con las colisiones de nombre

No hay hoy `--db-schema`/`--db-prefix` (PLAN.md §9.3) para separar
automáticamente las tablas de distintos `.link` sobre la misma base.
PostgreSQL en particular NUNCA valida el schema completo de una colección
(a diferencia de SQLite, que falla fuerte ante cualquier diferencia) -- dos
servicios que declaran una colección con el mismo nombre por coincidencia
(`events`, `users`, `sessions`, nombres genéricos de alto riesgo) pueden
terminar compartiendo sin querer la misma tabla física, con resultados que
van de "inofensivo" a "un `INSERT` de un servicio viola una constraint que
dejó el otro" -- comportamiento completo, verificado, en GRAMMAR.md
[§3.36](../GRAMMAR.md#336-postgresql-en-runtime--resuelto-alcance-acotado).

**Mientras no exista namespacing nativo:**
- Usá una base de datos POR servicio si es viable -- la separación más
  simple y la que menos sorpresas da.
- Si varios servicios comparten una base a propósito, revisá los nombres de
  colección de cada `.link` contra los de los demás antes de desplegar --
  hoy es una revisión manual, no algo que `linkc build` detecte todavía
  (PLAN.md §9.3.9 lo trackea).
- Si estás adoptando servicios sobre tablas que YA EXISTEN y no querés que
  ningún `.link` las cree o altere, `--adopt-existing`/`LINK_ADOPT_EXISTING`
  (GRAMMAR.md [§3.67](../GRAMMAR.md#367---adopt-existing-adoptar-tablas-sin-auto-migrar--resuelto))
  hace que ese servicio nunca ejecute DDL -- útil también como defensa
  extra contra que un servicio cree una tabla por accidente sobre datos de
  otro.

## 4. Cada servicio configura sus propios flags -- no hay configuración global

CORS ([§3.41](../GRAMMAR.md#341-cors-configurable-y-headers-de-seguridad--resuelto-alcance-acotado)),
rate limiting por cliente ([§3.39](../GRAMMAR.md#339-rate_limit201m-límite-de-requests-por-cliente--resuelto-alcance-acotado)),
y expiración de sesión ([§3.50](../GRAMMAR.md#350---session-ttl-expiración-real-de-sesión--resuelto))
son flags de `linkc serve`, por proceso -- cada servicio los declara los
suyos, sin heredar nada de los demás. Para N servicios con la misma
política (ej. el mismo allowlist de CORS en todos), hoy hay que repetir el
flag en cada comando/unidad -- no hay un archivo de configuración
compartido entre servicios todavía.

## Resumen

| Necesidad | Solución hoy |
|---|---|
| Ruteo público | Reverse proxy (nginx/Caddy) por path o subdominio |
| No exponer cada puerto directo | `--host 127.0.0.1` (GRAMMAR.md §3.81) + firewall de sistema operativo como defensa en profundidad |
| Un solo proceso para varios `.link` | `linkc serve-all <directorio> --port-base N` (GRAMMAR.md §3.92) |
| Supervisión de proceso | `linkc docker` (contenedores), o `linkc systemd`/`linkc pm2-config` (unidad/config generada, sin contenedores) |
| Muchos servicios, una sola base | Revisar colisiones de nombre a mano, o una base por servicio, o `--adopt-existing` |
| Política compartida (CORS/rate-limit/TTL) | Repetir el flag por servicio -- no hay configuración global todavía |
| Desplegar desde git (CI/CD) | [docs/deploying-from-git.md](deploying-from-git.md) -- el workflow que `linkc new` ya scaffoldea, un servicio a la vez |
