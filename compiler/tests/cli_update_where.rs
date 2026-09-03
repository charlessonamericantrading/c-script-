// Tests de integración para `updateWhere` atómico y masivo a SQL (PLAN.md §9.20 Fase 2.3).

use std::path::PathBuf;
use std::process::Command;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-update-where-{name}-{}-{}",
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

fn run_link_tests(source: &str) -> (bool, String) {
    let temp = TempDir::new("run");
    let src = temp.write("app.link", source);
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("test").arg(&src).output().expect("linkc test");
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    (out.status.success(), text)
}

#[test]
fn update_where_works_end_to_end_through_real_binary() {
    let program = r#"
type Task = {
  id: Int,
  title: String,
  status: String,
  priority: Int,
  @column("is_flagged") flagged: Bool,
}

type NewTask = {
  title: String,
  status: String,
  priority: Int,
  flagged: Bool,
}

type StatusPatch = {
  status: String,
  flagged: Bool,
}

type PriorityPatch = {
  priority: Int,
}

db {
  tasks: Task[],
}

service TaskService {
  rpc updateStatus(fromStatus: String, toStatus: String) -> Int {
    db.tasks.updateWhere(
      |t: Task| { t.status == fromStatus },
      StatusPatch { status: toStatus, flagged: true }
    )
  }

  rpc updateWire(fromStatus: String, patch: Patch<Task>) -> Int {
    db.tasks.updateWhere(
      |t: Task| { t.status == fromStatus },
      patch
    )
  }

  rpc updateHighPriorityNonPushable(minPriority: Int, newPriority: Int) -> Int {
    db.tasks.updateWhere(
      |t: Task| { t.priority >= minPriority && t.title.length() > 3 },
      PriorityPatch { priority: newPriority }
    )
  }
}

test "updateWhere atómico modifica filas y retorna el conteo exacto" {
  db.tasks.insert(NewTask { title: "Fix bug", status: "todo", priority: 1, flagged: false });
  db.tasks.insert(NewTask { title: "Write docs", status: "todo", priority: 2, flagged: false });
  db.tasks.insert(NewTask { title: "Deploy", status: "done", priority: 3, flagged: false });
  db.tasks.insert(NewTask { title: "Refactor", status: "todo", priority: 5, flagged: false });

  // 1. Actualización masiva con struct patch tipado en código
  let updated = TaskService.updateStatus("todo", "in_progress");
  assert(updated == 3, "actualiza exactamente las 3 tareas en todo");

  let inProgress = db.tasks.findWhere(|t: Task| { t.status == "in_progress" });
  assert(inProgress.length() == 3, "las 3 tareas tienen status in_progress");
  assert(inProgress[0].flagged == true, "aplica el alias @column is_flagged");
  assert(inProgress[1].flagged == true, "aplica el alias @column is_flagged en la segunda");
  assert(inProgress[2].flagged == true, "aplica el alias @column is_flagged en la tercera");

  let doneTasks = db.tasks.findWhere(|t: Task| { t.status == "done" });
  assert(doneTasks.length() == 1, "la tarea done no fue modificada");
  assert(doneTasks[0].flagged == false, "flagged de tarea done sigue false");

  // 2. Segunda ejecución sobre el estado original devuelve 0 filas afectadas
  let updatedAgain = TaskService.updateStatus("todo", "archived");
  assert(updatedAgain == 0, "0 filas afectadas porque ya no hay ninguna en todo");

  // 3. Fallback interpretado para predicados complejos con funciones de string
  let reprioritized = TaskService.updateHighPriorityNonPushable(2, 10);
  assert(reprioritized == 3, "actualiza Write docs, Deploy y Refactor");

  let highPri = db.tasks.findWhere(|t: Task| { t.priority == 10 });
  assert(highPri.length() == 3, "las 3 tareas tienen nueva prioridad 10");
}
"#;

    let (ok, output) = run_link_tests(program);
    assert!(ok, "test falló:\n{output}");
    assert!(output.contains("1 passed"), "{output}");
}
