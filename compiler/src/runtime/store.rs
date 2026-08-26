//! La capa que EJECUTA el SQL, con dos backends detrás de una sola interfaz.
//!
//! Todo lo difícil de `db { ... }` (GRAMMAR.md §3.17) vive en `ColumnPlan`
//! (runtime/db.rs): qué campo va a una columna nativa y cuál a JSON, y el caso
//! `campo?: T?` que necesita tres estados -- ausente / null / valor -- donde una
//! columna SQL solo tiene un bit de NULL. Esa lógica es del LENGUAJE, no del
//! motor, así que no se duplica por backend: acá abajo solo queda lo que de
//! verdad cambia entre SQLite y PostgreSQL.
//!
//! Lo que de verdad cambia son cuatro cosas, y ninguna más:
//!
//! 1. **Los placeholders**: `?` en SQLite, `$1`, `$2`... en PostgreSQL.
//! 2. **El id recién insertado**: `last_insert_rowid()` contra `RETURNING "id"`.
//! 3. **Los tipos**: SQLite guarda un Bool como INTEGER y un JSON como TEXT;
//!    PostgreSQL tiene BOOLEAN y JSONB nativos.
//! 4. **El DDL**: `INTEGER PRIMARY KEY AUTOINCREMENT` contra `BIGSERIAL`, y la
//!    migración por `ALTER TABLE ... ADD COLUMN IF NOT EXISTS`.
//!
//! `Cell` es el tipo de valor común: lo que se bindea a un parámetro y lo que
//! se lee de una fila, idéntico para los dos motores.

use std::cell::RefCell;

/// Un valor SQL, en el vocabulario del lenguaje y no en el de un motor.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Cell {
    Null,
    Int(i64),
    Float(f64),
    Text(String),
    Bool(bool),
    /// Una columna JSON: TEXT en SQLite, JSONB en PostgreSQL. El envoltorio es
    /// del motor; lo de adentro es el mismo `serde_json::Value` en los dos.
    Json(serde_json::Value),
}

/// Qué se espera leer de una columna. Sale de `ColumnPlan`, así que la fila
/// nunca se decodifica "adivinando" por lo que el motor haya devuelto.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ColumnKind {
    Int,
    /// Un campo `Type::Timestamp` (GRAMMAR.md §3.31/§3.91) -- distinto de
    /// `Int`/`Int64` porque del lado Postgres puede venir de UNA de dos
    /// formas físicas MUY distintas: `BIGINT` con milisegundos (la
    /// convención propia de c-script, lo que `linkc build` crea) o un
    /// `date`/`timestamp`/`timestamptz` NATIVO de Postgres (una tabla ya
    /// existente, adoptada) -- ver `postgres_timestamp_cell`. Del lado
    /// SQLite se comporta exactamente igual que `Int` (SQLite no tiene un
    /// tipo temporal nativo separado, así que no hay ambigüedad que
    /// resolver ahí).
    Timestamp,
    Float,
    Text,
    Bool,
    Json,
}

/// `ReentrantMutex`, no `std::sync::Mutex` -- GRAMMAR.md, Pilar 1 del
/// roadmap de concurrencia (26/08/2026, a partir del pedido de skynet-d3):
/// `linkc serve` pasa a un hilo por request, así que la conexión real
/// necesita ser SEGURA entre hilos -- pero `transaction { }` (§3.154)
/// sostiene el candado por TODA su duración (BEGIN + cuerpo + COMMIT/
/// ROLLBACK), y el cuerpo llama de vuelta a `insert`/`applyPatch`/`find`/
/// etc., que TAMBIÉN piden el candado para su propia operación -- con un
/// `Mutex` común eso sería el mismo hilo bloqueándose a sí mismo (deadlock
/// garantizado, no una condición de carrera rara). Reentrante: el mismo
/// hilo puede volver a pedirlo cuantas veces haga falta sin bloquearse,
/// otro hilo sí espera de verdad. `RefCell` adentro para Postgres porque
/// `postgres::Client` pide `&mut self` para consultar -- `ReentrantMutex`
/// solo da `&T` al lockear (nunca `&mut T`, sería inseguro si el mismo
/// hilo pudiera reentrar con dos `&mut` vivos a la vez), así que la
/// mutabilidad real todavía la da `RefCell` puertas adentro del candado ya
/// tomado. SQLite no lo necesita: los métodos de `rusqlite::Connection`
/// (`execute`/`query`/`prepare`) ya toman `&self`, la mutabilidad interna
/// la maneja la propia librería C de SQLite.
pub(crate) enum Backend {
    Sqlite(parking_lot::ReentrantMutex<rusqlite::Connection>),
    Postgres {
        client: parking_lot::ReentrantMutex<RefCell<postgres::Client>>,
        /// La URL de conexión original -- guardada para poder RECONECTAR
        /// (`with_reconnect`, abajo) con el mismo criterio de TLS que usó
        /// la conexión inicial (`db::connect_postgres_client`), sin
        /// duplicar esa lógica acá (GRAMMAR.md §3.40).
        url: String,
    },
}

impl Backend {
    pub(crate) fn is_postgres(&self) -> bool {
        matches!(self, Backend::Postgres { .. })
    }

    /// Sostiene el candado de la conexión por TODA la duración de `f` --
    /// para `transaction { }` (GRAMMAR.md §3.154), que necesita que
    /// `BEGIN`/el cuerpo entero/`COMMIT`-`ROLLBACK` corran como una sola
    /// sección exclusiva, sin que OTRO hilo (otra request) intercale una
    /// escritura suya a mitad de una transacción ajena en la MISMA
    /// conexión física. `f` llama de vuelta a operaciones normales
    /// (`execute`/`query`/etc.) que también piden este mismo candado --
    /// reentrante, así que el mismo hilo no se bloquea a sí mismo.
    pub(crate) fn with_exclusive<T>(&self, f: impl FnOnce() -> T) -> T {
        match self {
            Backend::Sqlite(conn) => {
                let _guard = conn.lock();
                f()
            }
            Backend::Postgres { client, .. } => {
                let _guard = client.lock();
                f()
            }
        }
    }

    /// El placeholder del parámetro `n` (1-based).
    pub(crate) fn placeholder(&self, n: usize) -> String {
        match self {
            Backend::Sqlite(_) => "?".to_string(),
            Backend::Postgres { .. } => format!("${n}"),
        }
    }

    pub(crate) fn execute(&self, sql: &str, params: &[Cell]) -> Result<usize, String> {
        match self {
            Backend::Sqlite(conn) => {
                let conn = conn.lock();
                let refs: Vec<&dyn rusqlite::ToSql> =
                    params.iter().map(|c| c as &dyn rusqlite::ToSql).collect();
                conn.execute(sql, refs.as_slice()).map_err(|e| e.to_string())
            }
            Backend::Postgres { client, url } => {
                let refs: Vec<&(dyn postgres::types::ToSql + Sync)> =
                    params.iter().map(|c| c as &(dyn postgres::types::ToSql + Sync)).collect();
                with_reconnect(client, url, |c| c.execute(sql, refs.as_slice())).map(|n| n as usize)
            }
        }
    }

    /// Ejecuta SQL sin parámetros ni filas de vuelta (DDL).
    pub(crate) fn execute_ddl(&self, sql: &str) -> Result<(), String> {
        match self {
            Backend::Sqlite(conn) => conn.lock().execute_batch(sql).map_err(|e| e.to_string()),
            Backend::Postgres { client, url } => with_reconnect(client, url, |c| c.batch_execute(sql)),
        }
    }

    /// Las filas, decodificadas posicionalmente según `kinds`. Una columna NULL
    /// siempre vuelve como `Cell::Null`, sea del tipo que sea -- distinguir
    /// "ausente" de "null" es trabajo de `ColumnPlan`, no de acá.
    pub(crate) fn query(
        &self,
        sql: &str,
        params: &[Cell],
        kinds: &[ColumnKind],
    ) -> Result<Vec<Vec<Cell>>, String> {
        match self {
            Backend::Sqlite(conn) => {
                let conn = conn.lock();
                let refs: Vec<&dyn rusqlite::ToSql> =
                    params.iter().map(|c| c as &dyn rusqlite::ToSql).collect();
                let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
                let rows = stmt
                    .query_map(refs.as_slice(), |row| {
                        let mut out = Vec::with_capacity(kinds.len());
                        for (i, kind) in kinds.iter().enumerate() {
                            out.push(sqlite_cell(row, i, *kind)?);
                        }
                        Ok(out)
                    })
                    .map_err(|e| e.to_string())?;
                rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| e.to_string())
            }
            Backend::Postgres { client, url } => {
                let refs: Vec<&(dyn postgres::types::ToSql + Sync)> =
                    params.iter().map(|c| c as &(dyn postgres::types::ToSql + Sync)).collect();
                let rows = with_reconnect(client, url, |c| c.query(sql, refs.as_slice()))?;
                rows.iter()
                    .map(|row| {
                        kinds
                            .iter()
                            .enumerate()
                            .map(|(i, kind)| postgres_cell(row, i, *kind))
                            .collect::<Result<Vec<_>, String>>()
                    })
                    .collect()
            }
        }
    }

    /// El INSERT y el id que la base le asignó a la fila, en una sola operación
    /// donde el motor lo permite. `sql` no debe traer `RETURNING`: lo agrega
    /// esta función cuando corresponde.
    pub(crate) fn insert_returning_id(&self, sql: &str, params: &[Cell]) -> Result<i64, String> {
        match self {
            Backend::Sqlite(conn) => {
                let conn = conn.lock();
                let refs: Vec<&dyn rusqlite::ToSql> =
                    params.iter().map(|c| c as &dyn rusqlite::ToSql).collect();
                conn.execute(sql, refs.as_slice()).map_err(|e| e.to_string())?;
                Ok(conn.last_insert_rowid())
            }
            // En PostgreSQL no hay `last_insert_rowid()`, y su equivalente
            // (`lastval()`) es por sesión: con una segunda conexión de por medio
            // devolvería el id de otra fila. `RETURNING` lo resuelve en la misma
            // sentencia, sin ventana de carrera posible.
            Backend::Postgres { client, url } => {
                let refs: Vec<&(dyn postgres::types::ToSql + Sync)> =
                    params.iter().map(|c| c as &(dyn postgres::types::ToSql + Sync)).collect();
                let returning = format!("{sql} RETURNING \"id\"");
                let row = with_reconnect(client, url, |c| c.query_one(&returning, refs.as_slice()))?;
                // `Row::get` (a diferencia de todo lo demás en este archivo)
                // PANICKEA si el valor no convierte al tipo pedido -- documentado
                // así en tokio-postgres. `Db::connect_postgres` ya rechaza al
                // conectar cualquier tabla preexistente cuyo "id" no sea entero
                // (ver validate_existing_id_column en db.rs), así que en el
                // camino normal esto nunca dispara -- pero como handle_rpc corre
                // sincrónico en el hilo principal del accept-loop (server.rs), un
                // panic acá tira abajo el servidor ENTERO, no solo esta request.
                // `try_get` es la variante que no panickea: defensa en
                // profundidad, no confiar en que la validación de arriba sea la
                // única puerta.
                //
                // `validate_existing_id_column` (db.rs) acepta "bigint",
                // "integer" Y "smallint" -- una tabla preexistente con "id"
                // `SERIAL`/`INTEGER GENERATED ALWAYS AS IDENTITY` (int4, no
                // int8) pasa esa validación al conectar. `try_get::<_, i64>`
                // exige que el OID de la columna sea EXACTAMENTE int8 --
                // contra un int4/int2 real devolvía error en CADA insert,
                // pese a que el connect había aceptado la tabla (el mismo
                // desacuerdo entre capas que GRAMMAR.md §3.9 viene
                // documentando desde v1.0). `postgres_int_cell` prueba los
                // tres anchos que esa validación ya reconoce como válidos.
                postgres_int_cell(&row, 0)?.ok_or_else(|| "\"id\" devuelto por RETURNING es NULL".to_string())
            }
        }
    }

    /// `NOTIFY <channel>, <payload>` (GRAMMAR.md §3.44) -- no-op para
    /// SQLite, que no tiene ningún mecanismo de notificación cross-proceso
    /// (y cross-instancia solo importa cuando hay más de una instancia
    /// compartiendo la base, que es justo el caso que Postgres cubre).
    /// `channel` es un literal fijo del propio compilador (nunca viene de
    /// afuera, así que interpolarlo en el SQL no es una inyección), pero
    /// `payload` SÍ es un parámetro bindeado -- via `pg_notify()`, la forma
    /// de función, en vez de la sentencia `NOTIFY canal, 'texto'`, que
    /// exigiría escapar el payload a mano dentro de un literal SQL.
    pub(crate) fn notify(&self, channel: &str, payload: &str) -> Result<(), String> {
        match self {
            Backend::Sqlite(_) => Ok(()),
            Backend::Postgres { client, url } => {
                with_reconnect(client, url, |c| c.execute("SELECT pg_notify($1, $2)", &[&channel, &payload])).map(|_| ())
            }
        }
    }
}

/// Repara la conexión SOLA cuando se cortó, pero nunca reintenta la
/// operación que la encontró cortada (GRAMMAR.md §3.40). `op` corre UNA
/// vez -- si el error es de conexión cerrada (`Error::is_closed`, ver
/// tokio-postgres), esta request sigue devolviendo ese error tal cual (no
/// hay forma de saber si el servidor ya había aplicado un INSERT/UPDATE
/// antes de que la conexión se cayera; reintentarlo a ciegas podría
/// duplicar una fila), pero la conexión se reemplaza por una nueva ANTES de
/// devolver el error -- así que la PRÓXIMA request (un reintento real del
/// cliente, o cualquier otro rpc) encuentra la base ya reconectada, en vez
/// de que el proceso entero quede sirviendo error tras error hasta un
/// reinicio manual (el comportamiento de antes de esta ronda).
///
/// Reconectar es best-effort: si TAMBIÉN falla, se deja el cliente viejo
/// como estaba -- la request siguiente vuelve a intentarlo sola, con la
/// misma lógica, sin ningún estado especial que limpiar.
fn with_reconnect<T>(
    client: &parking_lot::ReentrantMutex<RefCell<postgres::Client>>,
    url: &str,
    op: impl FnOnce(&mut postgres::Client) -> Result<T, postgres::Error>,
) -> Result<T, String> {
    let guard = client.lock();
    let result = op(&mut guard.borrow_mut());
    if let Err(e) = &result {
        if e.is_closed() {
            if let Ok(fresh) = super::db::connect_postgres_client(url) {
                *guard.borrow_mut() = fresh;
            }
        }
    }
    result.map_err(|e| describe_postgres_error(&e))
}

/// Bug real, encontrado verificando a mano el `@unique` COMPUESTO nuevo
/// (GRAMMAR.md §3.155) contra Postgres real -- pero preexistente, no
/// introducido por esa ronda: `postgres::Error::to_string()` para un error
/// devuelto por el SERVIDOR (`as_db_error()`, ej. una violación de
/// `UNIQUE`/`CHECK`) imprime solo la categoría genérica ("db error"), sin el
/// mensaje real que Postgres mandó -- el texto real vive en
/// `DbError::message()`, un nivel más adentro. `db::is_unique_violation`/
/// `is_check_violation` (GRAMMAR.md §3.80/§3.96) buscan un substring
/// EXACTO ("duplicate key value violates unique constraint"/"violates
/// check constraint") en ese texto para traducir a 400 -- contra "db
/// error" a secas, ninguno de los dos matcheaba nunca, así que TODA
/// violación de `@unique`/`@check` contra Postgres real caía como 500
/// genérico, nunca el 400 documentado. SQLite nunca tuvo este bug (su
/// propio `rusqlite::Error::to_string()` sí incluye el mensaje real).
fn describe_postgres_error(e: &postgres::Error) -> String {
    match e.as_db_error() {
        // Segunda mitad del mismo bug: el MENSAJE de un DbError viene
        // LOCALIZADO según `lc_messages` del servidor Postgres ("llave
        // duplicada viola restricción de unicidad" en un servidor en
        // español, no "duplicate key value..." -- encontrado en la propia
        // verificación manual de esta ronda) -- comparar por SUBSTRING de
        // ese mensaje (`is_unique_violation`/`is_check_violation`, arriba)
        // sería frágil ante cualquier locale que no sea inglés. El
        // SQLSTATE (`db_err.code()`) es la parte del protocolo que NUNCA
        // se traduce -- `23505`/`23514` significan lo mismo sin importar
        // el idioma del servidor. Se antepone la frase fija en inglés que
        // `is_unique_violation`/`is_check_violation` ya buscan (texto que
        // YO agrego acá, no algo que Postgres mandó) para que esas dos
        // funciones seguir funcionando SIN TOCARLAS -- el mensaje real
        // (posiblemente en otro idioma) se conserva igual, al lado.
        Some(db_err) => match *db_err.code() {
            postgres::error::SqlState::UNIQUE_VIOLATION => {
                format!("duplicate key value violates unique constraint -- {}", db_err.message())
            }
            postgres::error::SqlState::CHECK_VIOLATION => format!("violates check constraint -- {}", db_err.message()),
            _ => db_err.message().to_string(),
        },
        None => e.to_string(),
    }
}

fn sqlite_cell(row: &rusqlite::Row, i: usize, kind: ColumnKind) -> rusqlite::Result<Cell> {
    Ok(match kind {
        // SQLite no tiene un tipo temporal nativo separado -- una columna
        // `Timestamp` es siempre INTEGER con milisegundos, sin la
        // ambigüedad que sí existe del lado Postgres (ver `ColumnKind::Timestamp`).
        ColumnKind::Int | ColumnKind::Timestamp => match row.get::<_, Option<i64>>(i)? {
            Some(n) => Cell::Int(n),
            None => Cell::Null,
        },
        ColumnKind::Float => match row.get::<_, Option<f64>>(i)? {
            Some(f) => Cell::Float(f),
            None => Cell::Null,
        },
        ColumnKind::Text => match row.get::<_, Option<String>>(i)? {
            Some(s) => Cell::Text(s),
            None => Cell::Null,
        },
        // SQLite no tiene BOOLEAN: se guardó como 0/1 (ver `native_sql_type`).
        ColumnKind::Bool => match row.get::<_, Option<i64>>(i)? {
            Some(n) => Cell::Bool(n != 0),
            None => Cell::Null,
        },
        ColumnKind::Json => match row.get::<_, Option<String>>(i)? {
            Some(text) => Cell::Json(
                serde_json::from_str(&text)
                    .unwrap_or_else(|e| panic!("JSON guardado por nosotros mismos no puede ser inválido: {e}")),
            ),
            None => Cell::Null,
        },
    })
}

/// Lee una columna entera de Postgres tolerando los tres anchos que
/// `validate_existing_id_column` ya acepta para "id" de una tabla preexistente
/// (`bigint`/int8, `integer`/int4, `smallint`/int2, GRAMMAR.md §3.55) -- pero
/// se usa para CUALQUIER columna `ColumnKind::Int`, no solo "id": una tabla
/// adoptada de otro backend puede tener perfectamente un campo `Int` normal
/// guardado como `INTEGER` en vez de `BIGINT`. `try_get` exige que el OID de
/// la columna matchee EXACTO al tipo Rust pedido -- de ahí probar los tres en
/// orden en vez de uno solo.
fn postgres_int_cell(row: &postgres::Row, i: usize) -> Result<Option<i64>, String> {
    if let Ok(v) = row.try_get::<_, Option<i64>>(i) {
        return Ok(v);
    }
    if let Ok(v) = row.try_get::<_, Option<i32>>(i) {
        return Ok(v.map(i64::from));
    }
    row.try_get::<_, Option<i16>>(i).map(|v| v.map(i64::from)).map_err(|e| e.to_string())
}

/// Decodifica el binario CRUDO que Postgres manda para `timestamp`/
/// `timestamptz` (8 bytes, entero de 64 bits big-endian, microsegundos
/// desde su propio epoch 2000-01-01 -- IDÉNTICO para las dos variantes: la
/// diferencia "with/without time zone" es de FORMATEO en texto, nunca de
/// representación binaria) -- GRAMMAR.md §3.91. `postgres`/`postgres-types`
/// no ofrece esto sin sumar la dependencia `chrono`; se implementa a mano
/// en vez de eso, mismo espíritu que el algoritmo de calendario de Hinnant
/// en `runtime/timestamp.rs` -- un formato binario chico y documentado por
/// el propio protocolo de Postgres no amerita una dependencia nueva.
struct PgTimestampMicros(i64);

impl<'a> postgres::types::FromSql<'a> for PgTimestampMicros {
    fn from_sql(ty: &postgres::types::Type, raw: &'a [u8]) -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        let bytes: [u8; 8] = raw.try_into().map_err(|_| format!("'{ty}': se esperaban 8 bytes, llegaron {}", raw.len()))?;
        Ok(PgTimestampMicros(i64::from_be_bytes(bytes)))
    }

    fn accepts(ty: &postgres::types::Type) -> bool {
        matches!(*ty, postgres::types::Type::TIMESTAMP | postgres::types::Type::TIMESTAMPTZ)
    }
}

/// Como `PgTimestampMicros`, para `date` nativo -- 4 bytes, entero de 32
/// bits big-endian, DÍAS (no microsegundos) desde el mismo epoch 2000-01-01.
struct PgDateDays(i32);

impl<'a> postgres::types::FromSql<'a> for PgDateDays {
    fn from_sql(ty: &postgres::types::Type, raw: &'a [u8]) -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        let bytes: [u8; 4] = raw.try_into().map_err(|_| format!("'{ty}': se esperaban 4 bytes, llegaron {}", raw.len()))?;
        Ok(PgDateDays(i32::from_be_bytes(bytes)))
    }

    fn accepts(ty: &postgres::types::Type) -> bool {
        matches!(*ty, postgres::types::Type::DATE)
    }
}

/// Decodifica el binario CRUDO que Postgres manda para `numeric` -- GRAMMAR.md
/// §3.103. A diferencia de `float4`/`float8` (IEEE754 de ancho fijo, que
/// `postgres-types` ya sabe leer como `f64`), `numeric` es un formato de
/// PRECISIÓN ARBITRARIA propio del protocolo: `int16 ndigits`, `int16
/// weight`, `int16 sign` (`0x0000` positivo, `0x4000` negativo, `0xC000`
/// `NaN`), `int16 dscale` (escala de display, no hace falta para el valor),
/// y `ndigits` dígitos de BASE 10000 (no base 10), cada uno un `int16`. El
/// valor es `signo * Σ dígito[i] * 10000^(weight - i)`. Mismo espíritu que
/// `PgTimestampMicros`/`PgDateDays` arriba: un formato chico y documentado
/// por el propio protocolo no amerita sumar una dependencia nueva
/// (`rust_decimal` u otra) solo para esto -- y como el destino declarado en
/// c-script es `Float` (`f64`), no un tipo decimal propio, decodificar
/// directo a `f64` no pierde nada que `Float` ya no perdiera de por sí.
struct PgNumeric(f64);

impl<'a> postgres::types::FromSql<'a> for PgNumeric {
    fn from_sql(ty: &postgres::types::Type, raw: &'a [u8]) -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        if raw.len() < 8 {
            return Err(format!("'{ty}': numeric truncado, se esperaban al menos 8 bytes de cabecera, llegaron {}", raw.len()).into());
        }
        let ndigits = u16::from_be_bytes([raw[0], raw[1]]) as usize;
        let weight = i16::from_be_bytes([raw[2], raw[3]]) as i32;
        let sign = u16::from_be_bytes([raw[4], raw[5]]);
        // `dscale` (raw[6..8]) es la escala de DISPLAY (cuántos decimales
        // mostraría Postgres) -- no afecta el valor numérico en sí, así que
        // no hace falta leerla para convertir a `f64`.
        let expected_len = 8 + ndigits * 2;
        if raw.len() < expected_len {
            return Err(format!("'{ty}': numeric truncado, se esperaban {expected_len} bytes, llegaron {}", raw.len()).into());
        }
        if sign != 0x0000 && sign != 0x4000 {
            // `0xC000` (NaN) y `0xD000`/`0xF000` (Infinity/-Infinity, Postgres
            // 14+) no tienen un `Float` real que los represente sin perder la
            // distinción "no es un número" -- se rechaza en vez de adivinar
            // (ej. devolver 0.0 en silencio).
            return Err(format!("'{ty}': numeric con signo 0x{sign:04x} (NaN/Infinity) no se puede representar como Float").into());
        }
        let mut value = 0f64;
        for i in 0..ndigits {
            let offset = 8 + i * 2;
            let digit = i16::from_be_bytes([raw[offset], raw[offset + 1]]) as f64;
            value += digit * 10f64.powi((weight - i as i32) * 4);
        }
        if sign == 0x4000 {
            value = -value;
        }
        Ok(PgNumeric(value))
    }

    fn accepts(ty: &postgres::types::Type) -> bool {
        matches!(*ty, postgres::types::Type::NUMERIC)
    }
}

/// Una columna `Timestamp` (GRAMMAR.md §3.31/§3.91) contra Postgres puede
/// ser físicamente UNA de dos cosas MUY distintas -- se prueban en orden,
/// mismo criterio que `postgres_int_cell` con los tres anchos de entero:
///
/// 1. `BIGINT` con milisegundos-desde-1970 -- la convención propia de
///    c-script, lo que `linkc build` crea para una tabla nueva.
/// 2. `timestamp`/`timestamptz`/`date` NATIVO de Postgres -- una tabla YA
///    EXISTENTE, adoptada (`--adopt-existing`/`linkc introspect`), que
///    nunca pasó por `linkc build`.
///
/// Encontrado auditando un reporte de adopción real (MyFinance): antes de
/// esta ronda, una columna `date`/`timestamp` nativa declarada como
/// `Timestamp` fallaba al leer la primera fila real -- ninguno de los tres
/// anchos de entero de `postgres_int_cell` matchea el OID de esos tipos
/// (`try_get` exige que el OID de la columna matchee EXACTO al tipo Rust
/// pedido), así que la única alternativa documentada era declarar el campo
/// como `String` -- que a su vez fallaba igual, por el motivo opuesto (el
/// wire binario de un `timestamp` tampoco es texto UTF-8 válido).
fn postgres_timestamp_cell(row: &postgres::Row, i: usize) -> Result<Option<i64>, String> {
    if let Ok(v) = row.try_get::<_, Option<i64>>(i) {
        return Ok(v);
    }
    if let Ok(v) = row.try_get::<_, Option<PgTimestampMicros>>(i) {
        return Ok(v.map(|PgTimestampMicros(micros)| super::timestamp::millis_from_pg_timestamp_micros(micros)));
    }
    row.try_get::<_, Option<PgDateDays>>(i)
        .map(|v| v.map(|PgDateDays(days)| super::timestamp::millis_from_pg_date_days(days)))
        .map_err(|e| e.to_string())
}

/// Una columna `Float` (GRAMMAR.md §3.103) contra Postgres puede ser
/// físicamente `float4`/`float8` (la convención propia de c-script, lo que
/// `linkc build` crea) O `numeric` NATIVO (una tabla ya existente, adoptada
/// -- el caso normal para cualquier columna de DINERO, donde `numeric` es
/// el tipo correcto y `float8` NO, por el error de redondeo binario que
/// justamente evita). Mismo criterio de "probar en orden" que
/// `postgres_int_cell`/`postgres_timestamp_cell`: `try_get` exige que el
/// OID matchee EXACTO, así que antes de esta ronda una columna `numeric`
/// declarada `Float` fallaba al leer la primera fila real -- "'{ty}' no
/// implementa FromSql para numeric", con `String` fallando igual por el
/// motivo opuesto (el wire binario de `numeric` tampoco es texto UTF-8).
fn postgres_float_cell(row: &postgres::Row, i: usize) -> Result<Option<f64>, String> {
    if let Ok(v) = row.try_get::<_, Option<f64>>(i) {
        return Ok(v);
    }
    row.try_get::<_, Option<PgNumeric>>(i).map(|v| v.map(|PgNumeric(f)| f)).map_err(|e| e.to_string())
}

fn postgres_cell(row: &postgres::Row, i: usize, kind: ColumnKind) -> Result<Cell, String> {
    Ok(match kind {
        ColumnKind::Int => match postgres_int_cell(row, i)? {
            Some(n) => Cell::Int(n),
            None => Cell::Null,
        },
        ColumnKind::Timestamp => match postgres_timestamp_cell(row, i)? {
            Some(n) => Cell::Int(n),
            None => Cell::Null,
        },
        ColumnKind::Float => match postgres_float_cell(row, i)? {
            Some(f) => Cell::Float(f),
            None => Cell::Null,
        },
        ColumnKind::Text => match row.try_get::<_, Option<String>>(i).map_err(|e| e.to_string())? {
            Some(s) => Cell::Text(s),
            None => Cell::Null,
        },
        ColumnKind::Bool => match row.try_get::<_, Option<bool>>(i).map_err(|e| e.to_string())? {
            Some(b) => Cell::Bool(b),
            None => Cell::Null,
        },
        ColumnKind::Json => {
            match row.try_get::<_, Option<serde_json::Value>>(i).map_err(|e| e.to_string())? {
                Some(v) => Cell::Json(v),
                None => Cell::Null,
            }
        }
    })
}

impl rusqlite::ToSql for Cell {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        use rusqlite::types::{ToSqlOutput, Value as SqlValue};
        Ok(match self {
            Cell::Null => ToSqlOutput::Owned(SqlValue::Null),
            Cell::Int(n) => ToSqlOutput::Owned(SqlValue::Integer(*n)),
            Cell::Float(f) => ToSqlOutput::Owned(SqlValue::Real(*f)),
            Cell::Text(s) => ToSqlOutput::Owned(SqlValue::Text(s.clone())),
            Cell::Bool(b) => ToSqlOutput::Owned(SqlValue::Integer(i64::from(*b))),
            Cell::Json(v) => ToSqlOutput::Owned(SqlValue::Text(
                serde_json::to_string(v).expect("serializar a JSON no puede fallar"),
            )),
        })
    }
}

impl postgres::types::ToSql for Cell {
    fn to_sql(
        &self,
        ty: &postgres::types::Type,
        out: &mut postgres::types::private::BytesMut,
    ) -> Result<postgres::types::IsNull, Box<dyn std::error::Error + Sync + Send>> {
        match self {
            Cell::Null => Ok(postgres::types::IsNull::Yes),
            // GRAMMAR.md §3.104: `i64::to_sql` (la impl de `postgres-types`
            // para el tipo Rust `i64`, vía su macro `simple_to!`) IGNORA el
            // `ty` que se le pasa -- siempre serializa 8 bytes de `int8`, sin
            // importar qué ancho pidió el servidor. Contra una tabla
            // PREEXISTENTE con "id" `SERIAL`/`SMALLINT` (int4/int2, §3.59) el
            // servidor infiere `$1` como ESE ancho en `WHERE "id" = $1` --
            // mandar 8 bytes ahí corrompe el protocolo binario ("db error"
            // genérico, sin detalle útil). El lado de LECTURA
            // (`postgres_int_cell`, arriba) ya prueba los tres anchos; acá,
            // en escritura, el ancho lo dice el propio `ty` que el servidor
            // manda -- no hace falta probar, alcanza con despachar por él.
            // Un valor que no entra en el ancho pedido (im probable para
            // "id", posible para otro campo `Int` normal) es un error claro
            // en vez de un truncado silencioso.
            Cell::Int(n) => match *ty {
                postgres::types::Type::INT2 => {
                    let n16: i16 = (*n).try_into().map_err(|_| format!("{n} no entra en un entero de 16 bits (columna '{ty}')"))?;
                    n16.to_sql(ty, out)
                }
                postgres::types::Type::INT4 => {
                    let n32: i32 = (*n).try_into().map_err(|_| format!("{n} no entra en un entero de 32 bits (columna '{ty}')"))?;
                    n32.to_sql(ty, out)
                }
                _ => n.to_sql(ty, out),
            },
            Cell::Float(f) => f.to_sql(ty, out),
            Cell::Text(s) => s.to_sql(ty, out),
            Cell::Bool(b) => b.to_sql(ty, out),
            Cell::Json(v) => v.to_sql(ty, out),
        }
    }

    // `accepts` decide por TIPO de Rust, y acá un mismo `Cell` puede ir a
    // columnas distintas según el campo, así que la decisión se delega al
    // motor: `to_sql_checked!` valida el par (valor, tipo de columna) en cada
    // bindeo y devuelve un error claro si no encaja, en vez de mandar bytes que
    // el servidor interpretaría mal.
    fn accepts(_ty: &postgres::types::Type) -> bool {
        true
    }

    postgres::types::to_sql_checked!();
}

/// Invariante de arquitectura para la concurrencia real por hilos (Pilar 1
/// del roadmap propuesto a partir del pedido de skynet-d3, 26/08/2026):
/// compartir `Db` entre hilos vía `Arc<Db>` con `Mutex` adentro depende de
/// que estos dos tipos sean `Send` -- si una futura versión de `rusqlite`/
/// `postgres` alguna vez dejara de serlo, este test falla en COMPILACIÓN
/// (no en runtime), la señal más barata posible de que la arquitectura
/// entera necesita revisarse antes de seguir.
#[cfg(test)]
mod send_probe {
    fn assert_send<T: Send>() {}
    #[test]
    fn connection_types_are_send() {
        assert_send::<rusqlite::Connection>();
        assert_send::<postgres::Client>();
    }
}
