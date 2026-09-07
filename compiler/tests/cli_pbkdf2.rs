// `crypto.verifyPbkdf2Sha256` (GRAMMAR.md §3.284, PLAN.md §9.24 Fase 2 ítem
// D1): verificación de un hash `pbkdf2:<iter>:<salt>:<hashHex>` -- el formato
// que un boilerplate típico de Node produce con `crypto.pbkdf2Sync`, NO el
// formato propio de este lenguaje (`crypto.hashPassword` sigue siendo
// Argon2id). El fixture de abajo es un valor REAL generado con Node
// (`crypto.pbkdf2Sync('correct horse battery staple', salt, 10000, 64,
// 'sha256')`, node v24), no un valor auto-generado por este mismo binario --
// probar contra la implementación de referencia es lo único que confirma
// interoperabilidad real, no solo que el round-trip interno de este
// compilador es consistente consigo mismo.

use std::path::PathBuf;
use std::process::Command;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-pbkdf2-{name}-{}-{}",
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

const NODE_GENERATED_HASH: &str =
    "pbkdf2:10000:30023bbc3b460190607f4a0d0474a361:b1989d9ad7bd209329b930361d564437aa5c27f081c461e729d58485202a0df800549d88afb102dd646c2054acce56e240865ab8140e270c17bde23883b95e0b";

#[test]
fn verifies_a_real_node_crypto_pbkdf2sync_hash_and_rejects_the_wrong_password() {
    let program = format!(
        r#"
service Auth {{
  rpc check(pwd: String, stored: String) -> Bool {{ crypto.verifyPbkdf2Sha256(pwd, stored) }}
}}

test "a hash produced by node's crypto.pbkdf2Sync verifies the right password" {{
  assert(Auth.check("correct horse battery staple", "{NODE_GENERATED_HASH}"));
}}

test "the wrong password fails against the same real hash" {{
  assert(!Auth.check("wrong password", "{NODE_GENERATED_HASH}"));
  assert(!Auth.check("", "{NODE_GENERATED_HASH}"));
}}
"#
    );
    let (ok, text) = run_link_tests(&program);
    assert!(ok, "{text}");
    assert!(text.contains("2 passed"), "{text}");
}

#[test]
fn a_malformed_or_foreign_stored_value_is_simply_false_never_a_runtime_error() {
    let program = r#"
service Auth {
  rpc check(pwd: String, stored: String) -> Bool { crypto.verifyPbkdf2Sha256(pwd, stored) }
}

test "anything that isn't a well-formed pbkdf2:<iter>:<salt>:<hashHex> string is false, not an error" {
  assert(!Auth.check("x", ""));
  assert(!Auth.check("x", "not-pbkdf2-at-all"));
  assert(!Auth.check("x", "$argon2id$v=19$m=19456,t=2,p=1$abc$def"));
  assert(!Auth.check("x", "pbkdf2:notanumber:salt:aabbcc"));
  assert(!Auth.check("x", "pbkdf2:0:salt:aabbcc"));
  assert(!Auth.check("x", "pbkdf2:10:salt:not-hex-zz"));
  assert(!Auth.check("x", "pbkdf2:10:salt:a"));
  assert(!Auth.check("x", "pbkdf2:10:salt:"));
  assert(!Auth.check("x", "pbkdf2:10:salt"));
  assert(!Auth.check("x", "pbkdf2:10:salt:aabbcc:extra"));
}
"#;
    let (ok, text) = run_link_tests(program);
    assert!(ok, "{text}");
    assert!(text.contains("1 passed"), "{text}");
}

#[test]
fn the_derived_key_length_follows_whatever_the_stored_hash_hex_actually_has() {
    // Node boilerplates NO siempre usan el mismo dkLen -- algunos piden 32,
    // otros 64. La implementación tiene que derivar tantos bytes como
    // `hashHex` realmente tenga, no un largo fijo hardcodeado.
    let program = r#"
service Auth {
  rpc check(pwd: String, stored: String) -> Bool { crypto.verifyPbkdf2Sha256(pwd, stored) }
}

test "a 32-byte (64 hex char) derived key verifies correctly too" {
  // node -e "console.log('pbkdf2:1000:' + 'abc123' + ':' + require('crypto').pbkdf2Sync('hola', 'abc123', 1000, 32, 'sha256').toString('hex'))"
  assert(Auth.check("hola", "pbkdf2:1000:abc123:1d3f6577e5b5fefe99434a299249c15f9ec362d3b924713e8a42a1d81f90caae"));
  assert(!Auth.check("chau", "pbkdf2:1000:abc123:1d3f6577e5b5fefe99434a299249c15f9ec362d3b924713e8a42a1d81f90caae"));
}
"#;
    let (ok, text) = run_link_tests(program);
    assert!(ok, "{text}");
    assert!(text.contains("1 passed"), "{text}");
}
