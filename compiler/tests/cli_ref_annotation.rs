// Tests de integración para `@ref(Coleccion, onDelete: ...)` -- foreign keys
// declarativas (PLAN.md §9.21 Fase 3 ítem 9, GRAMMAR.md §3.249).

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-ref-{name}-{}-{}",
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

const CASCADE_PROGRAM: &str = r#"
type Author = {
  id: Int,
  name: String,
}

type Post = {
  id: Int,
  title: String,
  @ref(authors, onDelete: Cascade) authorId: Int,
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

  rpc listPosts() -> Post[] {
    db.posts.all()
  }

  rpc removeAuthor(id: Int) -> Bool {
    db.authors.delete(id)
  }
}
"#;

// Mismo programa que CASCADE_PROGRAM pero sin `onDelete:` -- el default
// `NO ACTION` de SQL, para probar que un `delete` sobre un padre con hijos
// se bloquea en vez de dejar filas huérfanas.
const RESTRICT_PROGRAM: &str = r#"
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

  rpc removeAuthor(id: Int) -> Bool {
    db.authors.delete(id)
  }
}
"#;

const SET_NULL_PROGRAM: &str = r#"
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

  rpc getPost(id: Int) -> Post? {
    db.posts.find(id)
  }

  rpc removeAuthor(id: Int) -> Bool {
    db.authors.delete(id)
  }
}
"#;

#[test]
fn ref_annotation_full_crud_cycle_and_cascade_delete_through_real_binary() {
    let source = format!(
        "{CASCADE_PROGRAM}\n{}",
        r#"
test "@ref full CRUD cycle with cascade delete" {
  let author = Blog.addAuthor("Ada Lovelace");
  assert(author.id > 0, "author got an id");

  let post = Blog.addPost("Notes on the Analytical Engine", author.id);
  assert(post.authorId == author.id, "post references the real author id");

  let posts = Blog.listPosts();
  assert(posts.length() == 1, "one post exists before deleting the author");

  let removed = Blog.removeAuthor(author.id);
  assert(removed, "author deleted");

  let afterCascade = Blog.listPosts();
  assert(afterCascade.length() == 0, "onDelete: Cascade removed the dependent post too");
}
"#
    );
    let (ok, out) = run_link_tests(&source);
    assert!(ok, "test falló: {out}");
}

#[test]
fn ref_annotation_rejects_unknown_target_collection() {
    let bad_program = r#"
type Post = {
  id: Int,
  @ref(doesNotExist) authorId: Int,
}
db { posts: Post[] }
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: 'doesNotExist' no es una colección: {out}");
    assert!(out.contains("no es una colección declarada en 'db'"), "{out}");
}

#[test]
fn ref_annotation_rejects_field_type_that_does_not_match_target_pk() {
    let bad_program = r#"
type Author = {
  id: Int,
  name: String,
}
type Post = {
  id: Int,
  @ref(authors) authorId: String,
}
db {
  authors: Author[],
  posts: Post[],
}
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: el campo es String pero la PK de 'authors' es Int: {out}");
    assert!(out.contains("tiene que ser Int"), "{out}");
}

#[test]
fn ref_annotation_rejects_uuid_field_against_an_int_pk_target() {
    let bad_program = r#"
type Author = {
  id: Int,
  name: String,
}
type Post = {
  id: Int,
  @ref(authors) authorId: Uuid,
}
db {
  authors: Author[],
  posts: Post[],
}
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: Uuid contra una PK Int: {out}");
    assert!(out.contains("tiene que ser Int"), "{out}");
}

#[test]
fn ref_annotation_on_delete_set_null_requires_an_optional_field() {
    let bad_program = r#"
type Author = {
  id: Int,
  name: String,
}
type Post = {
  id: Int,
  @ref(authors, onDelete: SetNull) authorId: Int,
}
db {
  authors: Author[],
  posts: Post[],
}
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: SetNull sobre un campo no opcional: {out}");
    assert!(out.contains("SetNull") && out.contains("opcional"), "{out}");
}

#[test]
fn ref_annotation_rejects_repeated_annotation_on_the_same_field() {
    let bad_program = r#"
type Author = {
  id: Int,
  name: String,
}
type Post = {
  id: Int,
  @ref(authors) @ref(authors) authorId: Int,
}
db {
  authors: Author[],
  posts: Post[],
}
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: '@ref' repetido: {out}");
    assert!(out.contains("repetido"), "{out}");
}

#[test]
fn ref_annotation_rejects_double_optional_field() {
    let bad_program = r#"
type Author = {
  id: Int,
  name: String,
}
type Post = {
  id: Int,
  @ref(authors) authorId?: Int?,
}
db {
  authors: Author[],
  posts: Post[],
}
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: 'x?: T?' fuerza el envoltorio JSON: {out}");
    assert!(out.contains("opcional por clave Y nullable"), "{out}");
}

#[test]
fn ref_annotation_rejects_on_an_enum_variant_field() {
    let bad_program = r#"
type Author = {
  id: Int,
  name: String,
}
enum Event {
  PostCreated { @ref(authors) authorId: Int },
}
db {
  authors: Author[],
}
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: '@ref' sobre una variante de enum: {out}");
    assert!(out.contains("las variantes de enum se guardan como JSON"), "{out}");
}

#[test]
fn ref_annotation_generated_postgres_ddl_includes_references_and_on_delete_as_a_second_pass() {
    let temp = TempDir::new("build");
    let src = temp.write("app.link", CASCADE_PROGRAM);
    let gen_dir = temp.0.join("gen");
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("build").arg(&src).arg(&gen_dir).output().expect("linkc build");
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "linkc build falló: {text}");

    let schema = std::fs::read_to_string(gen_dir.join("schema.postgres.sql")).expect("leer schema.postgres.sql");
    // La FK NUNCA va inline en el CREATE TABLE de "posts" -- "authors" podría
    // no existir todavía en ese punto (orden de HashMap sin garantías). Va
    // en una sentencia aparte, idempotente, después de que ambas tablas
    // existen -- por eso se busca "REFERENCES" solo dentro del bloque
    // CREATE TABLE de "posts", no en el archivo entero (el ALTER TABLE de
    // más abajo sí la usa, legítimamente).
    let create_posts_end = schema.find("CREATE TABLE IF NOT EXISTS \"posts\"").map(|i| i + schema[i..].find(");").unwrap()).unwrap();
    assert!(!schema[..create_posts_end].contains("REFERENCES"), "la FK no debe ir inline en el CREATE TABLE de posts: {schema}");
    assert!(schema.contains("ADD CONSTRAINT \"fk_posts_authorId\""), "falta el ALTER TABLE de la FK: {schema}");
    assert!(schema.contains("FOREIGN KEY (\"authorId\") REFERENCES \"authors\"(\"id\") ON DELETE CASCADE"), "{schema}");
    assert!(schema.contains("SELECT 1 FROM pg_constraint WHERE conname = 'fk_posts_authorId'"), "el ALTER TABLE debe ser idempotente: {schema}");
}

/// Espera a que `/live` responda, con timeout -- mismo criterio que
/// `cli_serve_accepts_db_pool_size_and_handles_requests` (cli_sqlite_wal_pool.rs).
fn wait_ready(port: u16) {
    for _ in 0..100 {
        if ureq::get(&format!("http://127.0.0.1:{port}/live")).call().is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("el servidor no levantó a tiempo");
}

fn rpc_status_and_body(port: u16, method: &str, body: &str) -> (u16, String) {
    match ureq::post(&format!("http://127.0.0.1:{port}/{method}")).set("Content-Type", "application/json").send_string(body) {
        Ok(r) => (r.status(), r.into_string().unwrap_or_default()),
        Err(ureq::Error::Status(status, r)) => (status, r.into_string().unwrap_or_default()),
        Err(e) => panic!("{method} falló de red: {e}"),
    }
}

#[test]
fn ref_annotation_insert_with_nonexistent_parent_is_a_400_not_a_500_over_real_sqlite() {
    let temp = TempDir::new("insert-bad-parent");
    let link_path = temp.write("app.link", CASCADE_PROGRAM);
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

    let (status, body) = rpc_status_and_body(port, "Blog/addPost", r#"{"title":"orphan","authorId":999999}"#);
    assert_eq!(status, 400, "un authorId inexistente tiene que ser 400, no 500 -- body: {body}");
    assert!(body.contains("@ref"), "el mensaje de error tiene que mencionar @ref (GRAMMAR.md §3.249): {body}");

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn ref_annotation_delete_blocked_by_child_without_cascade_is_a_400_not_a_500_over_real_sqlite() {
    let temp = TempDir::new("delete-restrict");
    let link_path = temp.write("app.link", RESTRICT_PROGRAM);
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

    let (status, author_body) = rpc_status_and_body(port, "Blog/addAuthor", r#"{"name":"Ada"}"#);
    assert_eq!(status, 200, "crear el autor debería funcionar: {author_body}");
    let author: serde_json::Value = serde_json::from_str(&author_body).unwrap();
    let author_id = author["id"].as_i64().unwrap();

    let (status, post_body) = rpc_status_and_body(port, "Blog/addPost", &format!(r#"{{"title":"x","authorId":{author_id}}}"#));
    assert_eq!(status, 200, "crear el post debería funcionar: {post_body}");

    // Sin `onDelete:`, el default `NO ACTION` bloquea el borrado del padre
    // mientras el hijo siga apuntándolo.
    let (status, delete_body) = rpc_status_and_body(port, "Blog/removeAuthor", &format!(r#"{{"id":{author_id}}}"#));
    assert_eq!(status, 400, "borrar un autor con posts dependientes tiene que ser 400, no 500 -- body: {delete_body}");
    assert!(delete_body.contains("@ref"), "el mensaje de error tiene que mencionar @ref (GRAMMAR.md §3.249): {delete_body}");

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn ref_annotation_on_delete_set_null_nulls_the_child_field_over_real_sqlite() {
    let temp = TempDir::new("set-null");
    let link_path = temp.write("app.link", SET_NULL_PROGRAM);
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

    let (_, author_body) = rpc_status_and_body(port, "Blog/addAuthor", r#"{"name":"Ada"}"#);
    let author: serde_json::Value = serde_json::from_str(&author_body).unwrap();
    let author_id = author["id"].as_i64().unwrap();

    let (_, post_body) = rpc_status_and_body(port, "Blog/addPost", &format!(r#"{{"title":"x","authorId":{author_id}}}"#));
    let post: serde_json::Value = serde_json::from_str(&post_body).unwrap();
    let post_id = post["id"].as_i64().unwrap();

    let (status, _) = rpc_status_and_body(port, "Blog/removeAuthor", &format!(r#"{{"id":{author_id}}}"#));
    assert_eq!(status, 200, "onDelete: SetNull permite borrar al padre");

    let (status, get_body) = rpc_status_and_body(port, "Blog/getPost", &format!(r#"{{"id":{post_id}}}"#));
    assert_eq!(status, 200, "{get_body}");
    let post_after: serde_json::Value = serde_json::from_str(&get_body).unwrap();
    assert_eq!(post_after["authorId"], serde_json::Value::Null, "SQLite puso NULL en authorId: {post_after}");

    let _ = child.kill();
    let _ = child.wait();
}
