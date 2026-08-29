# Desplegar un servicio `.link` desde git (CI/CD)

`linkc new <nombre>` ya scaffoldea `.github/workflows/deploy.yml` en todo
proyecto nuevo (GRAMMAR.md §3.181) -- esta guía explica qué hace ese archivo,
por qué está diseñado así, y cómo activarlo de verdad. Para servir varios
`.link` desde el mismo host, ver
[docs/multi-service-deployment.md](multi-service-deployment.md); esta guía
es sobre UN servicio, el mismo shape que scaffoldea `linkc new`.

## Por qué el deploy viene apagado por default

Un proyecto recién creado con `linkc new` no tiene todavía un servidor real
ni los secrets de GitHub configurados -- si el workflow intentara desplegar
en cada push a `main` desde el primer commit, el badge de CI quedaría en
rojo hasta que alguien configure todo, sin que ese rojo signifique un bug
real. Por eso el archivo scaffoldeado tiene DOS jobs con actitudes distintas:

- **`test-and-build`** corre en TODO push a `main`, sin ningún secret. Da
  señal real desde el primer commit: tests de comportamiento (`linkc test`),
  regeneración del contrato (`linkc build`), y -- si existe
  `main.link.snap` -- el mismo chequeo de deriva de contrato que usa
  c-script consigo mismo contra su propio demo insignia.
- **`deploy`** solo corre por disparo manual (`workflow_dispatch`, el botón
  "Run workflow" en la pestaña Actions de GitHub) hasta que edites la línea
  `if:` de ese job -- ver el comentario en el propio `deploy.yml`.

## Activarlo

1. Generá `main.link.snap` una vez (opcional, pero recomendado) para
   activar el chequeo de deriva del contrato:
   ```bash
   linkc test main.link main.link.snap --update
   git add main.link.snap && git commit -m "snapshot inicial del contrato"
   ```
2. Elegí cómo vas a correr `linkc serve` en el servidor -- `linkc systemd
   main.link <puerto>` genera la unidad, `linkc pm2-config main.link
   <puerto>` genera el `ecosystem.json` si preferís PM2 (ver
   [docs/multi-service-deployment.md §2](multi-service-deployment.md#2-supervisión-de-proceso)).
   Copiá esa unidad al servidor UNA vez, a mano -- el workflow scaffoldeado
   despliega el `.link` y reinicia el servicio, no arma la unidad desde cero
   en cada corrida.
3. En tu repo de GitHub, `Settings -> Secrets and variables -> Actions`,
   agregá los 5 secrets que documenta el propio `deploy.yml`:

   | Secret | Qué es |
   |---|---|
   | `DATABASE_URL` | La misma URL que le pasarías a `linkc serve --db` |
   | `DEPLOY_HOST` | IP o dominio del servidor |
   | `DEPLOY_USER` | Usuario SSH con permiso de `systemctl restart` |
   | `DEPLOY_SSH_KEY` | Clave privada SSH (la pública ya en `~/.ssh/authorized_keys` de `DEPLOY_USER`) |
   | `DEPLOY_URL` | URL pública donde queda escuchando el servicio ya desplegado |

4. Cambiá el `if:` del job `deploy` de `github.event_name ==
   'workflow_dispatch'` a `github.ref == 'refs/heads/main'` -- a partir de
   ahí, cada push a `main` que pase `test-and-build` despliega de verdad.

## Qué hace cada paso del job `deploy`, y por qué

1. **`linkc doctor main.link --db "$DATABASE_URL"`** -- diagnóstico de
   entorno de SOLO LECTURA (versión, que el `.link` tipe, permiso de
   escritura, conectividad `SELECT 1`) ANTES de tocar el servidor. Nunca
   ejecuta DDL -- un `doctor` en rojo frena el deploy sin haber arriesgado
   nada todavía.
2. **Copiar `main.link` y reiniciar el servicio** -- `scp` + `ssh systemctl
   restart`, la variante más simple posible. `linkc serve` recompila/migra
   el schema al arrancar (auto-migración no destructiva, GRAMMAR.md §3.17),
   así que copiar el `.link` y reiniciar es suficiente -- no hace falta un
   paso de migración separado.
3. **`linkc doctor main.link --db "$DATABASE_URL" --target-url "$DEPLOY_URL"`**
   -- el mismo diagnóstico de arriba, pero ahora comparando además la
   versión LOCAL (la que acaba de correr en el runner de CI) contra la que
   reporta `/health` del servidor ya reiniciado. Un desfasaje de versión
   queda como `[INFO]` en el log del job, no como error -- útil para
   confirmar a simple vista que el deploy realmente actualizó el binario en
   producción, sin bloquear el job por un servidor que todavía está
   terminando de reiniciar.

## Límite honesto, deliberado

El paso de despliegue en sí (`scp`/`ssh systemctl restart`) es UNA variante
concreta, pensada para ser fácil de leer y de reemplazar -- no una
abstracción de "despliegue genérico". Si usás `linkc docker` o `linkc
pm2-config` en vez de systemd, o Kubernetes, o cualquier otra cosa, ese es
el único paso que necesitás cambiar; el resto del workflow (`doctor` antes y
después, los tests/contrato de `test-and-build`) aplica igual. Sin rollback
automático: si `doctor --target-url` muestra algo raro después de
desplegar, el job termina igual (no hay una forma de "deshacer" un
`systemctl restart` con la versión anterior desde acá) -- revertir es
manual, igual que con cualquier despliegue por SSH simple.
