// `@hidden` (GRAMMAR.md §3.232, PLAN.md §9.19 ítem 7) contra el BINARIO
// real: `linkc build` emite `contract.d.ts`/`schemas.ts`/`openapi.json`
// SIN el campo, y `linkc test` (checker + runtime) lo sigue leyendo dentro
// del cuerpo del rpc. El caso real del CRM: `secret_key`/`auth_config`/
// `password_hash` viajaban por un stream que §3.16 obliga a devolver la
// fila entera.

use std::path::PathBuf;
use std::process::Command;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-hidden-{name}-{}-{}",
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

fn linkc(args: &[&std::ffi::OsStr]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).args(args).output().expect("ejecutar linkc");
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    (out.status.success(), text)
}

const PROGRAM: &str = r#"
type Account = { id: Int, name: String, @hidden secretKey: String, @hidden authConfig: String? }
type NewAccount = { name: String, secretKey: String, authConfig: String? }
db { accounts: Account[] }

service Accounts {
  rpc create(name: String, key: String) -> Account { db.accounts.insert(NewAccount { name: name, secretKey: key, authConfig: null }) }
  rpc all() -> Account[] { db.accounts.all() }
  rpc keyMatches(id: Int, key: String) -> Bool {
    db.accounts.all().filter(|a: Account| { a.id == id && a.secretKey == key }).length() == 1
  }
  stream live() -> Account { while true { db.accounts.subscribe() } }
}

test "the hidden field is readable inside the rpc body" {
  let a = Accounts.create("acme", "sk_live_123");
  assert(Accounts.keyMatches(a.id, "sk_live_123"), "the body reads secretKey");
  assert(!Accounts.keyMatches(a.id, "other"), "and compares it for real");
  assert(Accounts.all().length() == 1, "all() still returns the row");
}
"#;

#[test]
fn build_omits_hidden_fields_from_every_generated_artifact_and_test_still_reads_them() {
    let temp = TempDir::new("build");
    let src = temp.write("app.link", PROGRAM);
    let outdir = temp.0.join("gen");
    let (ok, out) = linkc(&[std::ffi::OsStr::new("build"), src.as_os_str(), outdir.as_os_str()]);
    assert!(ok, "{out}");
    // `NewAccount` (el type de ENTRADA, sin `@hidden`) sí lleva el campo
    // -- por eso cada aserción mira el bloque del type `Account`, no el
    // archivo entero.
    let block = |text: &str, start: &str, end: &str| -> String {
        let from = text.find(start).unwrap_or_else(|| panic!("falta '{start}' en:\n{text}"));
        let rest = &text[from..];
        let to = rest.find(end).unwrap_or_else(|| panic!("falta '{end}' tras '{start}' en:\n{text}"));
        rest[..to].to_string()
    };
    let contract = std::fs::read_to_string(outdir.join("contract.d.ts")).expect("contract.d.ts");
    let account = block(&contract, "export interface Account ", "}");
    assert!(account.contains("name: string"), "{account}");
    assert!(!account.contains("secretKey") && !account.contains("authConfig"), "contract.d.ts filtró un campo @hidden:\n{account}");
    let new_account = block(&contract, "export interface NewAccount ", "}");
    assert!(new_account.contains("secretKey"), "el type de entrada no cambia:\n{new_account}");

    let schemas = std::fs::read_to_string(outdir.join("schemas.ts")).expect("schemas.ts");
    let account = block(&schemas, "export const AccountSchema", "})");
    assert!(account.contains("name:"), "{account}");
    assert!(!account.contains("secretKey") && !account.contains("authConfig"), "schemas.ts filtró un campo @hidden:\n{account}");

    let openapi: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(outdir.join("openapi.json")).expect("openapi.json")).expect("openapi.json válido");
    let props = &openapi["components"]["schemas"]["Account"]["properties"];
    assert!(props.get("name").is_some(), "{props}");
    assert!(props.get("secretKey").is_none() && props.get("authConfig").is_none(), "openapi.json filtró un campo @hidden:\n{props}");
    let required = &openapi["components"]["schemas"]["Account"]["required"];
    assert!(!required.to_string().contains("secretKey"), "{required}");

    let (ok, out) = linkc(&[std::ffi::OsStr::new("test"), src.as_os_str()]);
    assert!(ok, "{out}");
}

#[test]
fn a_hidden_type_cannot_be_an_rpc_parameter() {
    let program = r#"
type Account = { id: Int, name: String, @hidden secretKey: String }
db { accounts: Account[] }
service Accounts {
  rpc echo(a: Account) -> String { a.name }
}
test "never runs" { assert(true, "unreachable"); }
"#;
    let temp = TempDir::new("param");
    let src = temp.write("app.link", program);
    let (ok, out) = linkc(&[std::ffi::OsStr::new("test"), src.as_os_str()]);
    assert!(!ok, "{out}");
    assert!(out.contains("un type con campos '@hidden'"), "{out}");
}
