// Tests de integración para `db.<c>.with(selector)` -- carga eager de una
// relación `@ref` sin N+1 (PLAN.md §9.21 Fase 3 ítem 10, GRAMMAR.md §3.250).

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-with-{name}-{}-{}",
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

fn run_link_tests(source: &str) -> (bool, String) {
    let temp = TempDir::new("run");
    let src = temp.write("app.link", source);
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("test").arg(&src).output().expect("linkc test");
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    (out.status.success(), text)
}

const REQUIRED_PROGRAM: &str = r#"
type Author = {
  id: Int,
  name: String,
}

type Post = {
  id: Int,
  title: String,
  @ref(authors) authorId: Int,
}

db {
  authors: Author[],
  posts: Post[],
}

service Blog {
  rpc addAuthor(name: String) -> Author {
    db.authors.insert(Author { id: 0, name: name })
  }

  rpc addPost(title: String, authorId: Int) -> Post {
    db.posts.insert(Post { id: 0, title: title, authorId: authorId })
  }

  rpc listWithAuthor() -> { row: Post, related: Author }[] {
    db.posts.with(|p: Post| { p.authorId })
  }
}
"#;

const OPTIONAL_PROGRAM: &str = r#"
type Author = {
  id: Int,
  name: String,
}

type Post = {
  id: Int,
  title: String,
  @ref(authors, onDelete: SetNull) authorId: Int?,
}

db {
  authors: Author[],
  posts: Post[],
}

service Blog {
  rpc addAuthor(name: String) -> Author {
    db.authors.insert(Author { id: 0, name: name })
  }

  rpc addPost(title: String, authorId: Int?) -> Post {
    db.posts.insert(Post { id: 0, title: title, authorId: authorId })
  }

  rpc listWithAuthor() -> { row: Post, related: Author? }[] {
    db.posts.with(|p: Post| { p.authorId })
  }
}
"#;

#[test]
fn with_relation_full_cycle_through_real_binary() {
    let source = format!(
        "{REQUIRED_PROGRAM}\n{}",
        r#"
test "with carga la relacion sin N+1" {
  let a1 = Blog.addAuthor("Ada");
  let a2 = Blog.addAuthor("Alan");
  Blog.addPost("Post 1", a1.id);
  Blog.addPost("Post 2", a1.id);
  Blog.addPost("Post 3", a2.id);

  let joined = Blog.listWithAuthor();
  assert(joined.length() == 3, "tres posts con su autor");
  assert(joined[0].row.title == "Post 1", "row es el Post real");
  assert(joined[0].related.name == "Ada", "related es el Author real");
  assert(joined[1].related.name == "Ada", "segundo post tambien con Ada");
  assert(joined[2].related.name == "Alan", "tercer post con Alan");
  assert(joined[2].row.authorId == a2.id, "row conserva el campo FK original");
}
"#
    );
    let (ok, out) = run_link_tests(&source);
    assert!(ok, "test falló: {out}");
}

#[test]
fn with_relation_over_an_optional_ref_field_nulls_related_when_absent() {
    let source = format!(
        "{OPTIONAL_PROGRAM}\n{}",
        r#"
test "with sobre @ref opcional" {
  let a = Blog.addAuthor("Ada");
  Blog.addPost("con autor", a.id);
  Blog.addPost("sin autor", null);

  let joined = Blog.listWithAuthor();
  assert(joined.length() == 2, "dos posts");
  assert(joined[0].related.isSome(), "el primero tiene related");
  assert(joined[1].related.isNone(), "el segundo (sin authorId) no tiene related");
}
"#
    );
    let (ok, out) = run_link_tests(&source);
    assert!(ok, "test falló: {out}");
}

#[test]
fn with_relation_rejects_a_field_without_ref() {
    let bad_program = r#"
type Author = { id: Int, name: String }
type Post = { id: Int, title: String, @ref(authors) authorId: Int }
db { authors: Author[], posts: Post[] }
service Blog {
  rpc bad() -> Int[] {
    db.posts.with(|p: Post| { p.title })
  }
}
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: 'title' no tiene @ref: {out}");
    assert!(out.contains("no tiene '@ref(...)'"), "{out}");
}

#[test]
fn with_relation_rejects_a_derived_expression_selector() {
    let bad_program = r#"
type Author = { id: Int, name: String }
type Post = { id: Int, title: String, @ref(authors) authorId: Int }
db { authors: Author[], posts: Post[] }
service Blog {
  rpc bad() -> Int[] {
    db.posts.with(|p: Post| { p.authorId + 1 })
  }
}
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: expresión derivada, no field access simple: {out}");
    assert!(out.contains("acceso de campo simple"), "{out}");
}

#[test]
fn with_relation_rejects_a_nonexistent_field() {
    let bad_program = r#"
type Author = { id: Int, name: String }
type Post = { id: Int, title: String, @ref(authors) authorId: Int }
db { authors: Author[], posts: Post[] }
service Blog {
  rpc bad() -> Int[] {
    db.posts.with(|p: Post| { p.doesNotExist })
  }
}
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: campo inexistente: {out}");
    assert!(out.contains("no es un campo de este struct"), "{out}");
}

#[test]
fn with_relation_rejects_more_than_one_argument() {
    let bad_program = r#"
type Author = { id: Int, name: String }
type Post = { id: Int, title: String, @ref(authors) authorId: Int }
db { authors: Author[], posts: Post[] }
service Blog {
  rpc bad() -> Int[] {
    db.posts.with(|p: Post| { p.authorId }, 1)
  }
}
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: 'with' toma exactamente 1 argumento: {out}");
    assert!(out.contains("toma exactamente 1 argumento"), "{out}");
}

#[test]
fn with_relation_generated_typescript_contract_emits_the_wrapper_type() {
    let temp = TempDir::new("build");
    let src = temp.write("app.link", REQUIRED_PROGRAM);
    let gen_dir = temp.0.join("gen");
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("build").arg(&src).arg(&gen_dir).output().expect("linkc build");
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "linkc build falló: {text}");

    let contract = std::fs::read_to_string(gen_dir.join("contract.d.ts")).expect("leer contract.d.ts");
    assert!(
        contract.contains("Promise<{ row: Post; related: Author }[]>"),
        "el contrato tiene que emitir el wrapper inline {{ row, related }}: {contract}"
    );
}

/// Espera a que `/live` responda, con timeout.
fn wait_ready(port: u16) {
    for _ in 0..100 {
        if ureq::get(&format!("http://127.0.0.1:{port}/live")).call().is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("el servidor no levantó a tiempo");
}

fn rpc(port: u16, method: &str, body: &str) -> serde_json::Value {
    let text = ureq::post(&format!("http://127.0.0.1:{port}/{method}"))
        .set("Content-Type", "application/json")
        .send_string(body)
        .unwrap_or_else(|e| panic!("{method} falló: {e}"))
        .into_string()
        .expect("leer el body");
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{method} no devolvió JSON ({e}): {text}"))
}

#[test]
fn with_relation_over_real_http_returns_the_joined_shape() {
    let temp = TempDir::new("http");
    let link_path = temp.write("app.link", REQUIRED_PROGRAM);
    let db_path = temp.0.join("app.db");
    let port = free_port();

    let mut child = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&link_path)
        .arg(port.to_string())
        .arg("--db")
        .arg(&db_path)
        .spawn()
        .expect("spawn linkc serve");
    wait_ready(port);

    let author = rpc(port, "Blog/addAuthor", r#"{"name":"Ada Lovelace"}"#);
    let author_id = author["id"].as_i64().unwrap();
    rpc(port, "Blog/addPost", &format!(r#"{{"title":"Notes","authorId":{author_id}}}"#));

    let joined = rpc(port, "Blog/listWithAuthor", "{}");
    let items = joined.as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["row"]["title"], "Notes");
    assert_eq!(items[0]["related"]["name"], "Ada Lovelace");
    assert_eq!(items[0]["related"]["id"], author_id);

    let _ = child.kill();
    let _ = child.wait();
}
