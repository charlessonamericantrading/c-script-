// Tests de integración para el pool de lectores SQLite sobre WAL
// (PLAN.md §9.21 Fase 1 ítem 2, GRAMMAR.md §3.246).
// Verifica:
// 1. Configuración de pool de lectores (--db-pool-size y Db::new_with_pool_options)
// 2. Concurrencia real con lectores paralelos mientras un escritor muta datos
// 3. Comportamiento en transacciones (lecturas fijadas al escritor)
// 4. Fallback limpio en bases :memory:
// 5. Rechazo de valores inválidos en CLI (--db-pool-size 0)

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-sqlite-pool-{name}-{}-{}",
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

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

const PROGRAM_SRC: &str = r#"
type Task = {
  id: Int,
  title: String,
  done: Bool,
}

db {
  tasks: Task[],
}

service Tasks {
  rpc add(title: String) -> Task {
    db.tasks.insert(Task { id: 0, title: title, done: false })
  }

  rpc list() -> Task[] {
    db.tasks.all()
  }

  rpc countDone() -> Int {
    db.tasks.countWhere(|t: Task| { t.done })
  }

  rpc remove(id: Int) -> Bool {
    db.tasks.delete(id)
  }
}
"#;

#[test]
fn sqlite_pool_in_memory_fallback() {
    let tokens = linkc::lexer::tokenize(PROGRAM_SRC).unwrap();
    let program = linkc::parser::parse(tokens).unwrap();

    let db = linkc::runtime::db::Db::new_with_pool_options(&program, std::path::Path::new(":memory:"), false, Some(5));
    let (max_r, _) = db.sqlite_pool_info().expect("debe ser sqlite");
    assert_eq!(max_r, 5);

    // Operaciones normales funcionan en memoria
    let inserted = db.call(
        "tasks",
        "insert",
        vec![linkc::runtime::Value::Struct(vec![
            ("title".to_string(), linkc::runtime::Value::Str("Mem Task".to_string())),
            ("done".to_string(), linkc::runtime::Value::Bool(false)),
        ])],
    ).unwrap();

    let id = match inserted {
        linkc::runtime::Value::Struct(fields) => fields.into_iter().find(|(k, _)| k == "id").unwrap().1,
        _ => panic!("se esperaba struct"),
    };

    let all = db.call("tasks", "all", vec![]).unwrap();
    match all {
        linkc::runtime::Value::List(items) => assert_eq!(items.len(), 1),
        _ => panic!("se esperaba list"),
    }
    assert_eq!(id, linkc::runtime::Value::Int(1));
}

#[test]
fn sqlite_pool_wal_multireader_concurrent_access() {
    let temp = TempDir::new("wal-concurrency");
    let db_path = temp.0.join("app.db");

    let tokens = linkc::lexer::tokenize(PROGRAM_SRC).unwrap();
    let program = linkc::parser::parse(tokens).unwrap();

    let pool_size = 6;
    let db = Arc::new(linkc::runtime::db::Db::new_with_pool_options(&program, &db_path, false, Some(pool_size)));
    let (max_r, _) = db.sqlite_pool_info().expect("debe ser sqlite");
    assert_eq!(max_r, pool_size);

    // 1. Insertar filas iniciales
    for i in 0..10 {
        db.call(
            "tasks",
            "insert",
            vec![linkc::runtime::Value::Struct(vec![
                ("title".to_string(), linkc::runtime::Value::Str(format!("Task {i}"))),
                ("done".to_string(), linkc::runtime::Value::Bool(i % 2 == 0)),
            ])],
        ).unwrap();
    }

    // 2. Concurrencia: 8 hilos lectores simultáneos ejecutando all/countWhere
    // mientras 1 hilo escritor inserta nuevas filas
    let mut reader_handles = Vec::new();
    for _ in 0..8 {
        let db_clone = db.clone();
        reader_handles.push(std::thread::spawn(move || {
            for _ in 0..20 {
                let all = db_clone.call("tasks", "all", vec![]).unwrap();
                match all {
                    linkc::runtime::Value::List(items) => assert!(items.len() >= 10),
                    _ => panic!("se esperaba list"),
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }));
    }

    let writer_db = db.clone();
    let writer_handle = std::thread::spawn(move || {
        for i in 10..25 {
            writer_db.call(
                "tasks",
                "insert",
                vec![linkc::runtime::Value::Struct(vec![
                    ("title".to_string(), linkc::runtime::Value::Str(format!("Task {i}"))),
                    ("done".to_string(), linkc::runtime::Value::Bool(false)),
                ])],
            ).unwrap();
            std::thread::sleep(Duration::from_millis(6));
        }
    });

    for h in reader_handles {
        h.join().unwrap();
    }
    writer_handle.join().unwrap();

    // 3. Verificación final: 10 iniciales + 15 del escritor = 25 filas
    let final_all = db.call("tasks", "all", vec![]).unwrap();
    match final_all {
        linkc::runtime::Value::List(items) => assert_eq!(items.len(), 25),
        _ => panic!("se esperaba list"),
    }

    // El pool de lectores debe tener conexiones recicladas disponibles
    let (_, idle) = db.sqlite_pool_info().unwrap();
    assert!(idle > 0, "debe haber conexiones recicladas en el pool");
}

#[test]
fn sqlite_pool_transaction_isolation() {
    let temp = TempDir::new("wal-tx");
    let db_path = temp.0.join("app.db");

    let tokens = linkc::lexer::tokenize(PROGRAM_SRC).unwrap();
    let program = linkc::parser::parse(tokens).unwrap();

    let db = Arc::new(linkc::runtime::db::Db::new_with_pool_options(&program, &db_path, false, Some(4)));

    // Dentro de una transacción, las lecturas ven las escrituras no commiteadas
    let res = db.with_exclusive_connection(|| {
        db.call(
            "tasks",
            "insert",
            vec![linkc::runtime::Value::Struct(vec![
                ("title".to_string(), linkc::runtime::Value::Str("Tx Task".to_string())),
                ("done".to_string(), linkc::runtime::Value::Bool(false)),
            ])],
        ).unwrap();

        let in_tx = db.call("tasks", "all", vec![]).unwrap();
        match in_tx {
            linkc::runtime::Value::List(items) => items.len(),
            _ => 0,
        }
    });

    assert_eq!(res, 1, "la lectura dentro de la transacción debe ver la fila insertada");
}

#[test]
fn cli_serve_rejects_invalid_db_pool_size() {
    let temp = TempDir::new("cli-reject");
    let link_path = temp.write("app.link", PROGRAM_SRC);
    let port = free_port();

    // pool size 0 debe fallar de inmediato
    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&link_path)
        .arg(port.to_string())
        .arg("--db-pool-size")
        .arg("0")
        .output()
        .expect("ejecutar linkc serve");

    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--db-pool-size/LINK_DATABASE_POOL_SIZE: se esperaba un entero >= 1"),
        "stderr: {stderr}"
    );

    // pool size no numérico debe fallar
    let out2 = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&link_path)
        .arg(port.to_string())
        .arg("--db-pool-size")
        .arg("invalido")
        .output()
        .expect("ejecutar linkc serve");

    assert!(!out2.status.success());
    let stderr2 = String::from_utf8_lossy(&out2.stderr);
    assert!(
        stderr2.contains("--db-pool-size/LINK_DATABASE_POOL_SIZE: se esperaba un entero >= 1"),
        "stderr: {stderr2}"
    );
}

#[test]
fn cli_serve_accepts_db_pool_size_and_handles_requests() {
    let temp = TempDir::new("cli-serve");
    let link_path = temp.write("app.link", PROGRAM_SRC);
    let db_path = temp.0.join("app.db");
    let port = free_port();

    let mut child = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&link_path)
        .arg(port.to_string())
        .arg("--db")
        .arg(&db_path)
        .arg("--db-pool-size")
        .arg("4")
        .spawn()
        .expect("spawn linkc serve");

    // Esperar a que el servidor esté listo
    let mut ready = false;
    for _ in 0..100 {
        if ureq::get(&format!("http://127.0.0.1:{port}/live")).call().is_ok() {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    assert!(ready, "el servidor no levantó a tiempo");

    // Enviar peticiones concurrentes
    let mut handles = Vec::new();
    for i in 0..6 {
        handles.push(std::thread::spawn(move || {
            let body = format!(r#"{{"title":"Task {i}"}}"#);
            let resp = ureq::post(&format!("http://127.0.0.1:{port}/Tasks/add"))
                .set("Content-Type", "application/json")
                .send_string(&body);
            assert!(resp.is_ok(), "Tasks/add debe responder 200 OK");
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    // Listar tareas
    let list_resp = ureq::post(&format!("http://127.0.0.1:{port}/Tasks/list"))
        .set("Content-Type", "application/json")
        .send_string("{}")
        .expect("Tasks/list debe responder 200 OK")
        .into_string()
        .unwrap();

    let val: serde_json::Value = serde_json::from_str(&list_resp).unwrap();
    assert_eq!(val.as_array().unwrap().len(), 6);

    let _ = child.kill();
    let _ = child.wait();
}
