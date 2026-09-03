// Tests de integración para la anotación `@column("nombre_sql")` (PLAN.md §9.20 Eje H).
// Valida que el nombre físico en la base de datos quede desacoplado del nombre lógico en el
// AST de c-script y el contrato de TypeScript generado.

use std::path::PathBuf;
use std::process::Command;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-col-alias-{name}-{}-{}",
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

const VALID_PROGRAM: &str = r#"
type Account = {
  @column("acc_id") id: Int,
  @column("full_name") userName: String,
  @column("mail_address") email: String,
  balance: Int,
}

type NewAccount = {
  @column("full_name") userName: String,
  @column("mail_address") email: String,
  balance: Int,
}

db {
  accounts: Account[],
}

service Accounts {
  rpc create(name: String, email: String, balance: Int) -> Account {
    db.accounts.insert(NewAccount {
      userName: name,
      email: email,
      balance: balance,
    })
  }

  rpc byId(id: Int) -> Account? {
    db.accounts.find(id)
  }

  rpc findByName(name: String) -> Account[] {
    db.accounts.findWhere(|a: Account| { a.userName == name })
  }

  rpc listByName() -> Account[] {
    db.accounts.orderBy(|a: Account| { a.userName }).all()
  }

  rpc update(id: Int, patch: Patch<Account>) -> Account {
    db.accounts.applyPatch(id, patch)
  }

  rpc remove(id: Int) -> Bool {
    db.accounts.delete(id)
  }
}

test "full CRUD cycle with @column physical aliases" {
  let a1 = Accounts.create("Bob", "bob@example.com", 100);
  let a2 = Accounts.create("Alice", "alice@example.com", 200);

  assert(a1.id == 1, "first id is 1");
  assert(a1.userName == "Bob", "userName mapped correctly");

  let found = Accounts.byId(1);
  assert(found.isSome(), "found by id");
  match found {
    a: Account => {
      assert(a.userName == "Bob", "userName preserved");
      assert(a.email == "bob@example.com", "email preserved");
    },
    null => assert(false, "unreachable"),
  }

  let foundWhere = Accounts.findByName("Alice");
  assert(foundWhere.length() == 1, "found by name through pushdown");
  assert(foundWhere[0].id == 2, "found Alice with id 2");

  let ordered = Accounts.listByName();
  assert(ordered.length() == 2, "2 accounts");
  assert(ordered[0].userName == "Alice", "Alice sorted first");

  let ok = Accounts.remove(1);
  assert(ok, "account deleted");
  assert(Accounts.listByName().length() == 1, "1 account remains");
}
"#;

#[test]
fn column_alias_works_end_to_end_through_real_binary() {
    let (ok, out) = run_link_tests(VALID_PROGRAM);
    assert!(ok, "test falló: {out}");
}

#[test]
fn column_alias_rejects_invalid_sql_identifier_characters() {
    let bad_program = r#"
type User = {
  id: Int,
  @column("user-name-invalid") name: String,
}
db { users: User[] }
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar por caracteres inválidos: {out}");
    assert!(out.contains("solo puede contener caracteres alfanuméricos"), "{out}");
}

#[test]
fn column_alias_rejects_duplicate_column_names_in_same_struct() {
    let bad_program = r#"
type User = {
  id: Int,
  @column("same_column") firstName: String,
  @column("same_column") lastName: String,
}
db { users: User[] }
test "dummy" { assert(true, "ok"); }
"#;
    let (ok, out) = run_link_tests(bad_program);
    assert!(!ok, "debería fallar por colisión de columnas físicas: {out}");
    assert!(out.contains("colisión de nombre de columna SQL"), "{out}");
}

#[test]
fn column_alias_preserves_typescript_field_names_in_codegen() {
    let temp = TempDir::new("build");
    let src = temp.write("app.link", VALID_PROGRAM);
    let gen_dir = temp.0.join("gen");
    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("build")
        .arg(&src)
        .arg(&gen_dir)
        .output()
        .expect("linkc build");
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "linkc build falló: {text}");

    let contract_path = gen_dir.join("contract.d.ts");
    let contract = std::fs::read_to_string(&contract_path).expect("leer contract.d.ts");

    // El contrato de TypeScript DEBE usar 'userName' y 'email', NUNCA 'full_name' o 'mail_address'
    assert!(contract.contains("userName: string;"), "contract debe contener userName: string;");
    assert!(contract.contains("email: string;"), "contract debe contener email: string;");
    assert!(!contract.contains("full_name: string;"), "contract NO debe filtrar el nombre físico full_name");
    assert!(!contract.contains("mail_address: string;"), "contract NO debe filtrar el nombre físico mail_address");
}
