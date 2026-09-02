//! `linkc triggers` (GRAMMAR.md §3.225, PLAN.md §9.19 ítem 1): el DDL de
//! PostgreSQL que hace que un `stream` de `linkc serve` reaccione a
//! escrituras hechas por OTRO sistema sobre la misma base.
//!
//! Hasta esta ronda, LISTEN/NOTIFY (§3.44) solo propagaba escrituras hechas
//! por otra instancia de `linkc` -- el NOTIFY lo mandaba `Db::notify_remote`
//! después de cada escritura propia. Una fila insertada por Express+Drizzle
//! (el caso real del CRM Nexus: 9 servicios `.link` sirviendo streams sobre
//! tablas que sigue escribiendo el backend viejo) era invisible para
//! `db.<c>.subscribe()`, y el backend viejo tenía que "republicar" por HTTP
//! cada escritura -- frágil, y sin forma de avisar desde dentro de una
//! transacción.
//!
//! La solución vive en la base, no en linkc: un trigger `AFTER INSERT OR
//! UPDATE OR DELETE ... FOR EACH ROW` por colección que hace `pg_notify` en
//! el MISMO canal que ya escucha cada `linkc serve` (`link_stream_changes`),
//! con un payload MÍNIMO -- `{via: "trigger", collection, op, id}` -- que el
//! receptor resuelve releyendo la fila por id (GRAMMAR.md §3.225). NOTIFY se
//! entrega al COMMIT, así que una transacción externa avisa sola, y el
//! payload nunca lleva la fila, así que el límite de 8000 bytes de NOTIFY
//! (§3.44) no aplica a estos eventos (salvo `delete`, ver abajo).
//!
//! Cómo evita el doble evento cuando quien escribe es el propio linkc: cada
//! conexión de `linkc serve` fija `SET link.instance = '<id>'` al conectar
//! (`connect_postgres_client`), y el trigger copia
//! `current_setting('link.instance', true)` al payload. Una instancia que
//! recibe un payload de trigger con `instance` NO vacío sabe que lo escribió
//! un linkc -- que ya mandó su propio NOTIFY con el evento completo (o lo
//! publicó local si era ella misma) -- y lo descarta. Solo `instance` vacío
//! (Drizzle, psql, cualquier otro cliente) dispara la relectura.
//!
//! El DDL es IDEMPOTENTE a propósito (`CREATE OR REPLACE FUNCTION` + `DROP
//! TRIGGER IF EXISTS` + `CREATE TRIGGER`): se puede aplicar N veces, y
//! ensayar dentro de `BEGIN`/`ROLLBACK` antes de ejecutarlo de verdad -- el
//! protocolo de migraciones del CRM lo exige. `linkc` NUNCA lo aplica solo:
//! este subcomando lo IMPRIME, y quien administra la base decide cuándo.
//! Solo PostgreSQL -- SQLite no tiene NOTIFY, y en SQLite un solo proceso
//! ya ve todas las escrituras.

use crate::ast::{Item, Program};

/// Nombre fijo de la función de trigger, una por schema. Compartida entre
/// todas las tablas: el nombre de la tabla viaja en `TG_TABLE_NAME`.
pub const TRIGGER_FUNCTION: &str = "link_notify_change";

/// Canal de NOTIFY -- tiene que ser EXACTAMENTE el que escucha
/// `spawn_remote_listener` (`runtime/db.rs`, `REMOTE_CHANGE_CHANNEL`).
pub const CHANNEL: &str = "link_stream_changes";

/// Cota del payload por debajo del límite duro de NOTIFY (8000 bytes): un
/// `delete` intenta llevar la fila borrada (`row_to_json(OLD)`, ya no se
/// puede releer), y si no entra, se manda sin ella.
pub const MAX_PAYLOAD_BYTES: usize = 7900;
const _: () = assert!(MAX_PAYLOAD_BYTES < 8000, "el payload tiene que caber en el límite duro de NOTIFY");

/// Solo las colecciones que algún `stream` observa con `db.<c>.subscribe()`
/// (la forma exacta que §3.16 reconoce, `ast::recognize_live_subscribe`) --
/// `--only-streams`, pedido del CRM Nexus: de 24 tablas declaradas solo 9
/// tenían un stream, y las de más escritura (sync de pedidos, imports) no
/// necesitan un trigger disparando en cada fila para nadie. Orden de
/// aparición, sin duplicados.
pub fn stream_collections(program: &Program) -> Vec<String> {
    let mut out = Vec::new();
    for item in &program.items {
        let Item::Service(s) = item else { continue };
        for m in &s.members {
            let crate::ast::Member::Stream(r) = m else { continue };
            if let Some(c) = crate::ast::recognize_live_subscribe(&r.body) {
                if !out.iter().any(|x| x == c) {
                    out.push(c.to_string());
                }
            }
        }
    }
    out
}

/// Todas las colecciones declaradas en `db { ... }` del programa (fusión de/// Todas las colecciones declaradas en `db { ... }` del programa (fusión de
/// varios bloques incluida, GRAMMAR.md §3.172), en orden de declaración.
pub fn collection_names(program: &Program) -> Vec<String> {
    let mut out = Vec::new();
    for item in &program.items {
        if let Item::Db(db) = item {
            for c in &db.collections {
                if !out.contains(&c.name) {
                    out.push(c.name.clone());
                }
            }
        }
    }
    out
}

fn qualified(schema: Option<&str>, name: &str) -> String {
    match schema {
        Some(s) => format!("\"{s}\".\"{name}\""),
        None => format!("\"{name}\""),
    }
}

/// El DDL completo para `program`: la función (una vez) + un trigger por
/// colección. `schema` es el de `--db-schema` (GRAMMAR.md §3.193); sin él,
/// todo va sin calificar (el `public` de siempre, vía `search_path`).
pub fn external_change_triggers_sql(program: &Program, schema: Option<&str>) -> String {
    external_change_triggers_sql_for(program, schema, &collection_names(program))
}

/// Igual que `external_change_triggers_sql` pero para una lista explícita de
/// colecciones (la de `stream_collections` con `--only-streams`).
pub fn external_change_triggers_sql_for(program: &Program, schema: Option<&str>, collections: &[String]) -> String {
    let _ = program;
    let function = qualified(schema, TRIGGER_FUNCTION);
    let mut out = String::new();
    out.push_str("-- 'linkc triggers' (GRAMMAR.md §3.225): hace que un `stream` de `linkc serve` reaccione a\n");
    out.push_str("-- escrituras hechas por OTRO sistema (un ORM, psql, un job) sobre estas tablas.\n");
    out.push_str("-- Idempotente: se puede aplicar N veces y ensayar con BEGIN/ROLLBACK. `linkc` nunca lo\n");
    out.push_str("-- aplica solo -- revisalo y aplicalo con el mecanismo de migraciones que ya uses.\n");
    out.push_str("-- Requiere: columna `id` como clave primaria en cada tabla (la que `db { }` declara).\n\n");
    out.push_str(&format!(
        "CREATE OR REPLACE FUNCTION {function}() RETURNS trigger LANGUAGE plpgsql AS $link$\n\
DECLARE\n\
  payload text;\n\
  writer text := coalesce(current_setting('link.instance', true), '');\n\
  sent bigint := (extract(epoch from clock_timestamp()) * 1000)::bigint;\n\
BEGIN\n\
  IF TG_OP = 'DELETE' THEN\n\
    payload := json_build_object('via', 'trigger', 'instance', writer, 'collection', TG_TABLE_NAME,\n\
                                 'op', 'delete', 'id', OLD.id, 'sent_at_ms', sent, 'row', row_to_json(OLD))::text;\n\
    IF octet_length(payload) > {MAX_PAYLOAD_BYTES} THEN\n\
      payload := json_build_object('via', 'trigger', 'instance', writer, 'collection', TG_TABLE_NAME,\n\
                                   'op', 'delete', 'id', OLD.id, 'sent_at_ms', sent)::text;\n\
    END IF;\n\
  ELSE\n\
    payload := json_build_object('via', 'trigger', 'instance', writer, 'collection', TG_TABLE_NAME,\n\
                                 'op', lower(TG_OP), 'id', NEW.id, 'sent_at_ms', sent)::text;\n\
  END IF;\n\
  PERFORM pg_notify('{CHANNEL}', payload);\n\
  RETURN NULL;\n\
END;\n\
$link$;\n\n"
    ));
    for name in collections {
        let table = qualified(schema, name);
        let trigger = format!("link_notify_{name}");
        out.push_str(&format!("DROP TRIGGER IF EXISTS \"{trigger}\" ON {table};\n"));
        out.push_str(&format!(
            "CREATE TRIGGER \"{trigger}\" AFTER INSERT OR UPDATE OR DELETE ON {table}\n  FOR EACH ROW EXECUTE FUNCTION {function}();\n\n"
        ));
    }
    if collections.is_empty() {
        out.push_str("-- Ninguna colección a la que enganchar: el programa no declara ninguna en `db { }` (o, con --only-streams, ningún `stream` observa una con db.<c>.subscribe()).\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::parse;

    fn program(src: &str) -> Program {
        parse(tokenize(src).expect("lexer")).expect("parser")
    }

    #[test]
    fn emits_one_idempotent_trigger_per_collection_and_the_shared_function_once() {
        let p = program("type A = { id: Int, x: Int }\ntype B = { id: Int, y: String }\ndb { as: A[], bs: B[] }\n");
        let sql = external_change_triggers_sql(&p, None);
        assert_eq!(sql.matches("CREATE OR REPLACE FUNCTION \"link_notify_change\"()").count(), 1, "{sql}");
        assert!(sql.contains("DROP TRIGGER IF EXISTS \"link_notify_as\" ON \"as\";"), "{sql}");
        assert!(sql.contains("CREATE TRIGGER \"link_notify_bs\" AFTER INSERT OR UPDATE OR DELETE ON \"bs\""), "{sql}");
        assert!(sql.contains("pg_notify('link_stream_changes', payload)"), "{sql}");
        assert!(sql.contains("current_setting('link.instance', true)"), "el trigger tiene que copiar la instancia escritora: {sql}");
        assert!(sql.contains("'via', 'trigger'"), "{sql}");
        assert_eq!(sql.matches("CREATE TRIGGER").count(), 2);
    }

    #[test]
    fn a_schema_qualifies_the_function_and_every_table() {
        let p = program("type A = { id: Int }\ndb { as: A[] }\n");
        let sql = external_change_triggers_sql(&p, Some("crm"));
        assert!(sql.contains("CREATE OR REPLACE FUNCTION \"crm\".\"link_notify_change\"()"), "{sql}");
        assert!(sql.contains("ON \"crm\".\"as\""), "{sql}");
        assert!(sql.contains("EXECUTE FUNCTION \"crm\".\"link_notify_change\"()"), "{sql}");
    }

    #[test]
    fn stream_collections_lists_only_what_a_live_stream_subscribes_to() {
        let p = program(
            "type A = { id: Int }\ntype B = { id: Int }\ntype C = { id: Int }\ndb { as: A[], bs: B[], cs: C[] }\n\
             service S {\n  stream wa() -> A { while true { db.as.subscribe() } }\n  stream wc() -> C { while true { db.cs.subscribe() } }\n  stream wa2() -> A { while true { db.as.subscribe() } }\n  rpc all() -> B[] { db.bs.all() }\n}\n",
        );
        assert_eq!(stream_collections(&p), vec!["as".to_string(), "cs".to_string()]);
        let sql = external_change_triggers_sql_for(&p, None, &stream_collections(&p));
        assert!(sql.contains("ON \"as\""), "{sql}");
        assert!(sql.contains("ON \"cs\""), "{sql}");
        assert!(!sql.contains("ON \"bs\""), "sin stream, sin trigger: {sql}");
    }

    #[test]
    fn a_program_without_collections_says_so_instead_of_emitting_nothing() {
        let p = program("fn f() -> Int { 1 }\n");
        let sql = external_change_triggers_sql(&p, None);
        assert!(sql.contains("no declara ninguna en `db { }`"), "{sql}");
        assert!(!sql.contains("CREATE TRIGGER"));
    }

    #[test]
    fn the_channel_and_payload_cap_match_the_listener_side() {
        // Si alguien renombra el canal en `db.rs`, este test es el que avisa.
        assert_eq!(CHANNEL, "link_stream_changes");
    }
}
