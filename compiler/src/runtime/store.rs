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
    Float,
    Text,
    Bool,
    Json,
}

pub(crate) enum Backend {
    Sqlite(rusqlite::Connection),
    Postgres {
        /// `RefCell` porque el cliente de `postgres` pide `&mut self` para
        /// consultar, y `Db` se comparte como `&Db` por todo el intérprete.
        /// Es seguro por la misma razón que ya vale para `SessionStore` y
        /// para `Db::subscribers`: el intérprete corre entero en el hilo
        /// principal (ver runtime/server.rs), una request a la vez.
        client: RefCell<postgres::Client>,
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
            Backend::Sqlite(conn) => conn.execute_batch(sql).map_err(|e| e.to_string()),
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
                row.try_get::<_, i64>(0).map_err(|e| e.to_string())
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
    client: &RefCell<postgres::Client>,
    url: &str,
    op: impl FnOnce(&mut postgres::Client) -> Result<T, postgres::Error>,
) -> Result<T, String> {
    let result = op(&mut client.borrow_mut());
    if let Err(e) = &result {
        if e.is_closed() {
            if let Ok(fresh) = super::db::connect_postgres_client(url) {
                *client.borrow_mut() = fresh;
            }
        }
    }
    result.map_err(|e| e.to_string())
}

fn sqlite_cell(row: &rusqlite::Row, i: usize, kind: ColumnKind) -> rusqlite::Result<Cell> {
    Ok(match kind {
        ColumnKind::Int => match row.get::<_, Option<i64>>(i)? {
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

fn postgres_cell(row: &postgres::Row, i: usize, kind: ColumnKind) -> Result<Cell, String> {
    Ok(match kind {
        ColumnKind::Int => match row.try_get::<_, Option<i64>>(i).map_err(|e| e.to_string())? {
            Some(n) => Cell::Int(n),
            None => Cell::Null,
        },
        ColumnKind::Float => match row.try_get::<_, Option<f64>>(i).map_err(|e| e.to_string())? {
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
            Cell::Int(n) => n.to_sql(ty, out),
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
