# SQLite vs PostgreSQL: cómo elegir

`linkc serve` habla los dos motores con el mismo `.link`, el mismo contrato
generado y los mismos `test` (GRAMMAR.md
[§3.17](../GRAMMAR.md#317-persistencia-real-db-sobre-sqlite--resuelto) y
[§3.36](../GRAMMAR.md#336-postgresql-en-runtime--resuelto-alcance-acotado)).
No hay una respuesta "mejor" en abstracto -- depende de tres preguntas.

## 1. ¿La base ya existe, o la crea `linkc`?

Si estás **adoptando** un sistema que ya tiene datos reales en una Postgres
que otro equipo administra, la respuesta casi siempre es PostgreSQL --
`linkc introspect` (GRAMMAR.md
[§3.66](../GRAMMAR.md#366-linkc-introspect-generar-un-link-desde-una-base-postgresql-existente--resuelto-alcance-acotado))
genera un `.link` de partida desde el schema real, y `--adopt-existing`
(GRAMMAR.md
[§3.67](../GRAMMAR.md#367---adopt-existing-adoptar-tablas-sin-auto-migrar--resuelto))
deja conectar sin que `linkc serve` intente crear o alterar ninguna tabla.
SQLite no tiene un camino de adopción -- es un archivo que `linkc` crea y
gestiona él mismo, no algo a lo que "te conectás" desde afuera.

Si el servicio es **nuevo** y no hay ninguna base preexistente, seguí a la
pregunta 2.

## 2. ¿Cuántos procesos van a escribir en la misma base?

**Un solo proceso `linkc serve`** (el caso común: un servicio, un puerto, un
archivo): SQLite alcanza y sobra. Es más simple operacionalmente -- un
archivo, sin credenciales de red, sin proceso de base de datos separado que
mantener vivo, y con las mismas garantías de durabilidad (WAL, `busy_timeout`)
que necesita un servicio de un solo proceso.

**Más de un proceso** que necesita ver los mismos datos -- varias réplicas
del mismo servicio detrás de un balanceador, o un `stream` que tiene que
avisar de una escritura que entró por OTRA instancia -- necesita PostgreSQL.
SQLite no tiene ningún mecanismo de notificación entre procesos; PostgreSQL
sí, vía LISTEN/NOTIFY (GRAMMAR.md
[§3.44](../GRAMMAR.md#344-postgresql-listennotify-stream-entre-varias-instancias--resuelto-alcance-acotado)),
que `linkc serve` ya usa automáticamente cuando el `--db` es Postgres, sin
configuración adicional.

## 3. ¿Qué tan importante es no tocar la base de producción mientras development?

Un patrón real y válido: usar SQLite en desarrollo/staging precisamente
**para evitar tocar la Postgres de producción** mientras se itera un
servicio nuevo, y migrar a PostgreSQL recién cuando el servicio necesita
correr contra datos reales o con más de una instancia. El `.link` no cambia
-- solo el flag `--db`.

## Tabla resumen

| Si... | Usá |
|---|---|
| Estás adoptando una base Postgres que ya existe, con datos reales | PostgreSQL (`linkc introspect` + `--adopt-existing`) |
| Es un servicio nuevo, un solo proceso, sin urgencia de escalar | SQLite (el default -- no hace falta ningún flag) |
| Necesitás más de una instancia del mismo servicio, o `stream` cross-instancia | PostgreSQL |
| Estás iterando en desarrollo y no querés tocar la Postgres de producción todavía | SQLite en dev, Postgres cuando el servicio esté listo |
| El servicio corre en un entorno serverless/efímero sin disco persistente | PostgreSQL (un archivo SQLite no sobrevive un contenedor que se recicla) |

## Lo que NO cambia según cuál elijas

El `.link`, los `rpc`, el contrato TypeScript generado, los `test`, y todo
el resto del lenguaje son exactamente los mismos -- un programa no se entera
de qué motor tiene atrás (GRAMMAR.md §3.36). Migrar de uno a otro más
adelante es cambiar un flag, no reescribir el servicio -- aunque migrar
DATOS ya existentes de un motor al otro sigue siendo trabajo manual, fuera
del alcance de `linkc` hoy.

## Diferencias reales que sí importan al elegir

| | SQLite | PostgreSQL |
|---|---|---|
| Qué crea `linkc serve` | El archivo, si no existe | Nada -- tiene que existir el servidor, `linkc` solo crea/migra tablas |
| Auto-migración de schema | Falla fuerte ante cualquier cambio que no sea agregar una columna opcional nueva (matriz completa: GRAMMAR.md §3.17) | No destructiva siempre -- agrega columnas nuevas (nullable), nunca falla al conectar por un cambio de tipo |
| `stream` entre instancias | No participa -- cada proceso solo ve sus propias escrituras | LISTEN/NOTIFY real, automático |
| Requiere red/credenciales | No -- un archivo local | Sí -- URL de conexión, TLS oportunista |
| Adopción de datos existentes | No aplica (SQLite es gestionado por `linkc`, no "adoptado") | `linkc introspect` + `--adopt-existing` |
