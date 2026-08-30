# link-lang

Instalador npm del compilador oficial de **c-script** (nombre del lenguaje: **Link**) — un lenguaje backend compilado donde un único archivo `.link` es la fuente de verdad para tipos, schema de base de datos, servicios RPC, autenticación y tests. `linkc build` emite el contrato TypeScript, un cliente tipado, validadores en runtime, hooks de React y OpenAPI a partir de ese archivo.

Este paquete **no contiene el compilador en sí** (`linkc` está escrito en Rust) — es un instalador liviano: la primera vez que se ejecuta, descarga el binario precompilado correcto para tu plataforma desde los [releases de GitHub](https://github.com/charlessonamericantrading/c-script-/releases) y lo deja cacheado en `~/.c-script/bin` para las próximas corridas.

## Instalación

```
npx linkc --version
```

o, para tenerlo disponible como comando global:

```
npm install -g link-lang
linkc --version
```

Plataformas soportadas: Windows (x64), Linux (x64) y macOS (Intel y Apple Silicon).

## Uso

```
linkc new mi-app
linkc build mi-app/app.link mi-app/gen
linkc serve mi-app/app.link 3000
```

Documentación completa del lenguaje, la CLI y todos los comandos: [GRAMMAR.md](https://github.com/charlessonamericantrading/c-script-/blob/master/GRAMMAR.md) y [README.md](https://github.com/charlessonamericantrading/c-script-/blob/master/README.md) en el repositorio.

## Cómo resuelve el binario

En orden, la primera opción que encuentra:

1. Un binario ya compilado dentro de este mismo repositorio (`compiler/target/release/`) -- solo aplica si estás desarrollando el compilador mismo.
2. Un binario ya descargado antes, cacheado en `~/.c-script/bin/`.
3. `linkc` en el `PATH` del sistema (por ejemplo, instalado con `cargo install`).
4. Si ninguna de las anteriores existe, descarga el binario correcto para tu plataforma desde el último release publicado en GitHub, verifica su checksum SHA-256 contra el que el propio release publica, y lo cachea en `~/.c-script/bin/` para la próxima vez.

## Licencia

MIT © Charlesson UK Consulting Group LTD
