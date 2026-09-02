// `db.<c>.orderBy(...)`/`orderByDesc(...)` encadenados con `.page()`/
// `.all()`, y `List<T>.sortBy`/`sortByDesc` en memoria (GRAMMAR.md §3.230,
// PLAN.md §9.19 ítem 5) -- contra el BINARIO real con `linkc test`:
// checker Y runtime, no el harness de runtime/mod.rs que saltea el checker.
// El caso real que lo motiva: "los 50 webhooks más NUEVOS de 15.000", que
// `.all().take(50)` (los más viejos) y `page` solo (orden por id) no daban.

use std::path::PathBuf;
use std::process::Command;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-order-by-{name}-{}-{}",
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

const PROGRAM: &str = r#"
type Event = { id: Int, kind: String, amount: Int, at: Int? }
type NewEvent = { kind: String, amount: Int, at: Int? }
db { events: Event[] }

service S {
  rpc add(kind: String, amount: Int, at: Int?) -> Event { db.events.insert(NewEvent { kind: kind, amount: amount, at: at }) }
  rpc newest(n: Int) -> String { db.events.orderByDesc(|e: Event| { e.at }).page(n, 0).map(|e: Event| { e.kind }).join(",") }
  rpc oldest() -> String { db.events.orderBy(|e: Event| { e.at }).all().map(|e: Event| { e.kind }).join(",") }
  rpc twoKeys() -> String { db.events.orderBy(|e: Event| { e.kind }).orderByDesc(|e: Event| { e.amount }).all().map(|e: Event| { e.kind }).join(",") }
  rpc mem() -> String { db.events.all().sortByDesc(|e: Event| { e.amount }).map(|e: Event| { e.kind }).join(",") }
}

test "orderBy pushes ORDER BY with nulls last, sortBy sorts in memory" {
  let e1 = S.add("a", 1, 100);
  let e2 = S.add("b", 5, null);
  let e3 = S.add("c", 3, 300);
  let e4 = S.add("d", 2, 200);
  assert(S.newest(2) == "c,d", "the two newest, never the null row first");
  assert(S.newest(10) == "c,d,a,b", "desc with the null last");
  assert(S.oldest() == "a,d,c,b", "asc with the null last");
  assert(S.twoKeys() == "a,b,c,d", "secondary key");
  assert(S.mem() == "b,c,d,a", "in-memory desc by amount");
}
"#;

#[test]
fn order_by_and_sort_by_work_end_to_end_through_the_real_binary() {
    let (ok, out) = run_link_tests(PROGRAM);
    assert!(ok, "{out}");
}

#[test]
fn ordering_by_a_list_field_is_a_compile_error_not_a_runtime_surprise() {
    let program = r#"
type Event = { id: Int, tags: String[] }
db { events: Event[] }
service S {
  rpc bad() -> Event[] { db.events.orderBy(|e: Event| { e.tags }).all() }
}
test "never runs" { assert(true, "unreachable"); }
"#;
    let (ok, out) = run_link_tests(program);
    assert!(!ok, "{out}");
    assert!(out.contains("solo se puede ordenar por"), "{out}");
}
