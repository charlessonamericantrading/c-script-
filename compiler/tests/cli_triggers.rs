// `linkc triggers <archivo.link> [--db-schema <nombre>]` (GRAMMAR.md §3.225,
// PLAN.md §9.19 ítem 1) contra el binario real: imprime el DDL idempotente
// de PostgreSQL y NO se conecta a nada -- el camino con base real
// (aplicar el DDL, escribir desde fuera, ver el push en un `stream`) vive
// en `pg_integration.rs`.

use std::path::PathBuf;
use std::process::Command;

const PROGRAM: &str = r#"
type Conversation = { id: Int, subject: String }
type Message = { id: Int, conversationId: Int, body: String }

db { conversations: Conversation[], messages: Message[] }

service Chat {
  stream watch() -> Message {
    while true {
      db.messages.subscribe()
    }
  }
}
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-triggers-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        );
        let path = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&path).expect("crear tempdir");
        Self(path)
    }

    fn write(&self, name: &str, content: &str) -> PathBuf {
        let p = self.0.join(name);
        std::fs::write(&p, content).expect("escribir archivo");
        p
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_linkc")).args(args).output().expect("ejecutar linkc")
}

#[test]
fn triggers_prints_idempotent_ddl_for_every_collection_without_touching_any_database() {
    let temp = TempDir::new("ddl");
    let src = temp.write("app.link", PROGRAM);
    let out = run(&["triggers", src.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

    assert_eq!(stdout.matches("CREATE OR REPLACE FUNCTION \"link_notify_change\"()").count(), 1, "{stdout}");
    for table in ["conversations", "messages"] {
        assert!(stdout.contains(&format!("DROP TRIGGER IF EXISTS \"link_notify_{table}\" ON \"{table}\";")), "{stdout}");
        assert!(
            stdout.contains(&format!("CREATE TRIGGER \"link_notify_{table}\" AFTER INSERT OR UPDATE OR DELETE ON \"{table}\"")),
            "{stdout}"
        );
    }
    // El canal y la marca que el lado receptor (`runtime/db.rs`) espera.
    assert!(stdout.contains("pg_notify('link_stream_changes', payload)"), "{stdout}");
    assert!(stdout.contains("'via', 'trigger'"), "{stdout}");
    assert!(stdout.contains("current_setting('link.instance', true)"), "{stdout}");
    // Ningún archivo SQLite ni nada más quedó en el directorio: solo texto.
    let leftovers: Vec<_> = std::fs::read_dir(&temp.0).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).collect();
    assert_eq!(leftovers, vec!["app.link"], "{leftovers:?}");
}

#[test]
fn a_db_schema_flag_qualifies_the_function_and_every_table() {
    let temp = TempDir::new("schema");
    let src = temp.write("app.link", PROGRAM);
    let out = run(&["triggers", src.to_str().unwrap(), "--db-schema", "crm"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("CREATE OR REPLACE FUNCTION \"crm\".\"link_notify_change\"()"), "{stdout}");
    assert!(stdout.contains("ON \"crm\".\"messages\""), "{stdout}");
    assert!(stdout.contains("EXECUTE FUNCTION \"crm\".\"link_notify_change\"()"), "{stdout}");
}

#[test]
fn a_program_that_does_not_type_check_fails_like_every_other_subcommand() {
    let temp = TempDir::new("broken");
    let src = temp.write("app.link", "fn f() -> Int { \"no\" }\n");
    let out = run(&["triggers", src.to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).is_empty(), "sin DDL parcial para un programa roto");
}

#[test]
fn only_streams_limits_the_ddl_to_collections_a_live_stream_observes() {
    let temp = TempDir::new("only-streams");
    let src = temp.write("app.link", PROGRAM);
    let out = run(&["triggers", src.to_str().unwrap(), "--only-streams"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    // Solo `messages` tiene un `stream` con `db.messages.subscribe()`;
    // `conversations` se declara pero nadie la observa.
    assert!(stdout.contains("CREATE TRIGGER \"link_notify_messages\""), "{stdout}");
    assert!(!stdout.contains("link_notify_conversations"), "sin stream, sin trigger: {stdout}");
    assert_eq!(stdout.matches("CREATE TRIGGER").count(), 1, "{stdout}");
}
