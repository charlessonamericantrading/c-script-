// Tests de integración para `@tenant`/`@tenant(claim: "nombre")` -- multi-tenancy
// declarativo con aislamiento total (PLAN.md §9.21 Fase 4 ítem 12, GRAMMAR.md §3.253).
//
// El aislamiento en sí (filtro automático en cada lectura, autopoblado en insert,
// bloqueo de escritura del campo, WHERE con tenant en delete/increment/applyPatch)
// se prueba contra un servidor real con JWTs firmados a mano en
// `compiler/tests/server_http.rs` -- un bloque `test { }` NUNCA lleva token
// (`linkc test` no abre ningún socket HTTP), así que cualquier operación real
// sobre una colección `@tenant` dentro de un `test { }` daría 400 "sin tenant
// resuelto". Este archivo se queda con lo que SÍ es chequeable en compilación:
// los rechazos del checker/parser, y que el shape insertable excluye el campo.

use std::path::PathBuf;
use std::process::Command;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-tenant-{name}-{}-{}",
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
fn tenant_annotation_insert_type_checks_with_the_tenant_field_present_but_the_runtime_ignores_it() {
    // Un literal `Note { ... }` con nombre se chequea PRIMERO contra su
    // propio tipo NOMINAL completo (todo campo requerido, "id" incluido, tiene
    // que estar presente) -- por eso todo `insert` de este repo escribe
    // `id: 0` aunque "id" esté excluido del shape insertable (`omit_id_field`).
    // El campo `@tenant` sigue la MISMA regla: tiene que estar en el literal
    // para que compile, pero el runtime SIEMPRE lo autopuebla con el claim
    // resuelto de la request (probado en server_http.rs) -- subtipado de
    // ancho (GRAMMAR.md §3.2/§4.2) es lo que permite que ese valor "de más"
    // (según el shape insertable, que sí lo excluye) se acepte y se descarte.
    let source = r#"
type Note = {
  id: Int,
  @tenant orgId: String,
  text: String,
}
db { notes: Note[] }
service Notes {
  rpc add(text: String) -> Note {
    db.notes.insert(Note { id: 0, orgId: "ignored-by-runtime", text: text })
  }
}
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(source);
    assert!(ok, "un literal que SÍ menciona 'orgId' también debería tipar (campo extra tolerado): {out}");
}

#[test]
fn tenant_annotation_rejects_unsupported_type() {
    let bad_program = r#"
type Note = {
  id: Int,
  @tenant orgId: Bool,
  text: String,
}
db { notes: Note[] }
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: '@tenant' sobre un Bool: {out}");
    assert!(out.contains("tiene que ser Int, Uuid o String"), "{out}");
}

#[test]
fn tenant_annotation_rejects_an_optional_field() {
    let bad_program = r#"
type Note = {
  id: Int,
  @tenant orgId: String?,
  text: String,
}
db { notes: Note[] }
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: '@tenant' sobre un campo opcional: {out}");
    assert!(out.contains("tiene que ser requerido"), "{out}");
}

#[test]
fn tenant_annotation_rejects_more_than_one_field_in_the_same_struct() {
    let bad_program = r#"
type Note = {
  id: Int,
  @tenant orgId: String,
  @tenant teamId: String,
  text: String,
}
db { notes: Note[] }
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: dos campos '@tenant' en el mismo struct: {out}");
    assert!(out.contains("más de un campo"), "{out}");
}

#[test]
fn tenant_annotation_rejects_combination_with_encrypted() {
    let bad_program = r#"
type Note = {
  id: Int,
  @tenant @encrypted orgId: String,
  text: String,
}
db { notes: Note[] }
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: '@tenant' junto con '@encrypted': {out}");
    assert!(out.contains("incompatible con '@encrypted'"), "{out}");
}

#[test]
fn tenant_annotation_rejects_on_an_enum_variant_field() {
    let bad_program = r#"
type Note = {
  id: Int,
  text: String,
}
enum Event {
  Created { @tenant orgId: String },
}
db { notes: Note[] }
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: '@tenant' sobre una variante de enum: {out}");
    assert!(out.contains("las variantes de enum se guardan como JSON"), "{out}");
}

#[test]
fn tenant_annotation_rejects_repeated_on_the_same_field() {
    let bad_program = r#"
type Note = {
  id: Int,
  @tenant @tenant orgId: String,
  text: String,
}
db { notes: Note[] }
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: '@tenant' repetido sobre el mismo campo: {out}");
    assert!(out.contains("repetido"), "{out}");
}

#[test]
fn tenant_annotation_parenthesized_form_rejects_a_keyword_other_than_claim() {
    let bad_program = r#"
type Note = {
  id: Int,
  @tenant(name: "orgId") orgId: String,
  text: String,
}
db { notes: Note[] }
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: '@tenant(name: ...)' -- solo 'claim:' es válido: {out}");
    assert!(out.contains("solo acepta 'claim: \"nombre\"'"), "{out}");
}

#[test]
fn tenant_annotation_parenthesized_form_rejects_an_empty_claim_name() {
    let bad_program = r#"
type Note = {
  id: Int,
  @tenant(claim: "") orgId: String,
  text: String,
}
db { notes: Note[] }
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar: '@tenant(claim: \"\")': {out}");
    assert!(out.contains("no puede estar vacío"), "{out}");
}
