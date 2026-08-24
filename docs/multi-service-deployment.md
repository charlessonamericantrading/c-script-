# Desplegar muchos servicios `.link` en un mismo host

Escrito para el caso real: una migración de un monolito a N servicios
`c-script` (una migración real reportó 17), todos corriendo en el mismo
servidor. El README cubre bien UN servicio suelto; esta guía cubre el resto.

**Límite honesto por adelantado**: hoy no existe ningún modo "workspace" que
sirva varios `.link` bajo un mismo proceso o puerto (PLAN.md §9.7) -- cada
`linkc serve` es un proceso y un puerto, uno por servicio. Esta guía es sobre
cómo operar bien ESE modelo con las herramientas que ya existen, no una
promesa de que el modelo vaya a cambiar.

## 1. Un puerto por servicio, un proxy adelante

```bash
linkc serve auth.link      8781 --db postgres://...
linkc serve billing.link   8782 --db postgres://...
linkc serve notifications.link 8783 --db postgres://...
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

**Por qué no exponer cada puerto directo a Internet**: hoy `linkc serve`
siempre escucha en `0.0.0.0` -- no hay un flag `--host`/`--bind` todavía
(PLAN.md §9.7) para limitarlo a `127.0.0.1`. Un firewall a nivel de sistema
operativo (`ufw`/`iptables`, deny-by-default, solo el puerto del proxy
abierto hacia afuera) es hoy la capa real que evita que cada servicio quede
expuesto directo -- no algo opcional, es la única defensa hasta que ese flag
exista.

## 2. Un proceso supervisor por servicio

`linkc docker` ya genera un `Dockerfile` por servicio (`linkc docker
<archivo> -o Dockerfile`) -- la ruta más simple si ya usás contenedores: un
contenedor por servicio, orquestado con `docker compose` o lo que ya uses.

Sin contenedores, `pm2` o `systemd` funcionan igual de bien -- no hay
todavía un generador oficial de ninguno de los dos (`linkc systemd`/`linkc
pm2-config`, PLAN.md §9.7), así que la unidad se escribe a mano, una por
servicio:

```ini
# /etc/systemd/system/miapp-auth.service
[Unit]
Description=miapp auth service
After=network.target

[Service]
ExecStart=/usr/local/bin/linkc serve /opt/miapp/auth.link 8781
Environment=LINK_DATABASE_URL=postgres://...
Restart=on-failure
RestartSec=2

[Install]
WantedBy=multi-user.target
```

```json
// ecosystem.config.json, para pm2
{
  "apps": [
    { "name": "auth", "script": "linkc", "args": "serve auth.link 8781", "env": { "LINK_DATABASE_URL": "postgres://..." } },
    { "name": "billing", "script": "linkc", "args": "serve billing.link 8782", "env": { "LINK_DATABASE_URL": "postgres://..." } }
  ]
}
```

**Arranque en frío de muchos procesos a la vez**: si los N servicios
arrancan simultáneamente (ej. un reinicio del host), un bind de puerto que
falla momentáneamente (`Address already in use` durante un reinicio previo
todavía liberando el puerto) puede producir una ráfaga de reintentos --
hoy no hay backoff exponencial nativo (`--restart-backoff`, PLAN.md §9.7),
así que `RestartSec`/`--restart-delay` del supervisor (arriba, ya puesto en
los dos ejemplos) es la mitigación real mientras tanto.

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
| No exponer cada puerto directo | Firewall de sistema operativo -- no hay `--host`/`--bind` todavía |
| Supervisión de proceso | `linkc docker` (con contenedores) o una unidad systemd/pm2 escrita a mano (sin contenedores) |
| Muchos servicios, una sola base | Revisar colisiones de nombre a mano, o una base por servicio, o `--adopt-existing` |
| Política compartida (CORS/rate-limit/TTL) | Repetir el flag por servicio -- no hay configuración global todavía |
