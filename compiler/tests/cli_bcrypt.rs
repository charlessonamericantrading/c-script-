// bcrypt en `crypto.verifyPassword`/`crypto.isLegacyHash` (GRAMMAR.md
// §3.226, PLAN.md §9.19 ítem 2): un hash `$2a$`/`$2b$`/`$2y$` emitido por
// OTRA app (bcryptjs en el caso real del CRM) verifica, se reporta como
// legado, y `hashPassword` sigue emitiendo solo Argon2id. Los hashes se
// generan acá con la crate real (costo 4, el mínimo, para que el test no
// pague 100ms por hash) y se inyectan en un `.link` que corre con `linkc
// test` -- checker y runtime reales, no el harness que saltea el checker.

use std::path::PathBuf;
use std::process::Command;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-bcrypt-{name}-{}-{}",
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
fn a_bcrypt_hash_from_another_app_verifies_and_is_reported_as_legacy_in_all_three_prefixes() {
    let hash_2b = bcrypt::hash("contraseña-del-crm", 4).expect("hash bcrypt");
    assert!(hash_2b.starts_with("$2b$04$"), "{hash_2b}");
    // `$2a$` y `$2y$` son el MISMO algoritmo con otro prefijo (bcryptjs
    // viejo, PHP password_hash): reetiquetar el hash tiene que verificar
    // igual -- así llegan de una base real que mezcló generaciones.
    let hash_2a = format!("$2a${}", &hash_2b[4..]);
    let hash_2y = format!("$2y${}", &hash_2b[4..]);

    let program = format!(
        r#"
service Auth {{
  rpc check(pwd: String, hash: String) -> Bool {{ crypto.verifyPassword(pwd, hash) }}
  rpc legacy(hash: String) -> Bool {{ crypto.isLegacyHash(hash) }}
  rpc fresh(pwd: String) -> String {{ crypto.hashPassword(pwd) }}
}}

test "the three bcrypt prefixes verify the right password and reject the wrong one" {{
  assert(Auth.check("contraseña-del-crm", "{hash_2b}"), "2b ok");
  assert(Auth.check("contraseña-del-crm", "{hash_2a}"), "2a ok");
  assert(Auth.check("contraseña-del-crm", "{hash_2y}"), "2y ok");
  assert(!Auth.check("otra", "{hash_2b}"), "2b wrong password");
  assert(!Auth.check("", "{hash_2b}"), "empty password");
}}

test "bcrypt is legacy, argon2id is not, and hashPassword never emits bcrypt" {{
  assert(Auth.legacy("{hash_2b}"), "2b legacy");
  assert(Auth.legacy("{hash_2a}"), "2a legacy");
  assert(Auth.legacy("{hash_2y}"), "2y legacy");
  let fresh = Auth.fresh("contraseña-del-crm");
  assert(fresh.startsWith("$argon2id$"), fresh);
  assert(!Auth.legacy(fresh), "argon2id is current, not legacy");
  assert(Auth.check("contraseña-del-crm", fresh), "the re-hash verifies too");
}}

test "a malformed or truncated bcrypt-looking hash is simply false, never an error" {{
  assert(!Auth.check("x", "$2b$04$demasiado-corto"), "truncated");
  assert(!Auth.check("x", "$2x$04$abcdefghijklmnopqrstuuabcdefghijklmnopqrstuvwxyz012345678901"), "2x (crypt_blowfish bug variant) is not accepted");
  assert(!Auth.legacy("$2x$04$abc"), "2x is not legacy either");
}}
"#
    );
    let (ok, text) = run_link_tests(&program);
    assert!(ok, "{text}");
    assert!(text.contains("3 passed"), "{text}");
}
