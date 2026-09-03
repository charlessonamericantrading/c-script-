// Tests de integración para `db.<c>.select(...)` empujado a SQL (PLAN.md §9.21 Fase 2 ítem 8, GRAMMAR.md §3.248).

use std::path::PathBuf;
use std::process::Command;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-select-projection-{name}-{}-{}",
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
fn select_projection_works_end_to_end_through_real_binary() {
    let program = r#"
type User = {
  id: Int,
  name: String,
  @column("user_email") email: String,
  score: Int,
  @softDelete deletedAt: Timestamp? = null,
}

type NewUser = {
  name: String,
  email: String,
  score: Int,
  deletedAt: Timestamp? = null,
}

type UserDTO = {
  id: Int,
  name: String,
}

type CustomProjection = {
  userId: Int,
  userEmail: String,
}

db {
  users: User[],
}

service UserService {
  rpc listNames() -> String[] {
    db.users.select(|u: User| { u.name })
  }

  rpc listIds() -> Int[] {
    db.users.select(|u: User| { u.id })
  }

  rpc listDTOs() -> UserDTO[] {
    db.users.select(|u: User| { UserDTO { id: u.id, name: u.name } })
  }

  rpc listCustom() -> CustomProjection[] {
    db.users.select(|u: User| { CustomProjection { userId: u.id, userEmail: u.email } })
  }

  rpc listAnonymous() -> { id: Int, email: String }[] {
    db.users.select(|u: User| { { id: u.id, email: u.email } })
  }

  rpc listOrderedDesc() -> UserDTO[] {
    db.users.orderByDesc(|u: User| { u.score }).select(|u: User| { UserDTO { id: u.id, name: u.name } })
  }

  rpc listUpperNamesCalculated() -> String[] {
    db.users.select(|u: User| { u.name.toUpper() })
  }

  rpc listDoubleScoresCalculated() -> Int[] {
    db.users.select(|u: User| { u.score * 2 })
  }
}

test "proyecciones parciales y escalares select funcionan con pushdown y fallback" {
  let u1 = db.users.insert(NewUser { name: "Alice", email: "alice@example.com", score: 80 });
  let u2 = db.users.insert(NewUser { name: "Bob", email: "bob@example.com", score: 95 });
  let u3 = db.users.insert(NewUser { name: "Charlie", email: "charlie@example.com", score: 70 });

  // 1. Proyección escalar directa (String[] e Int[])
  let names = UserService.listNames();
  assert(names.length() == 3, "trae 3 nombres");
  assert(names[0] == "Alice" && names[1] == "Bob" && names[2] == "Charlie");

  let ids = UserService.listIds();
  assert(ids.length() == 3, "trae 3 ids");
  assert(ids[0] == u1.id && ids[1] == u2.id && ids[2] == u3.id);

  // 2. Proyección a struct tipado DTO
  let dtos = UserService.listDTOs();
  assert(dtos.length() == 3, "trae 3 DTOs");
  assert(dtos[0].id == u1.id && dtos[0].name == "Alice");
  assert(dtos[1].id == u2.id && dtos[1].name == "Bob");

  // 3. Proyección con alias @column y campos destino renombrados
  let custom = UserService.listCustom();
  assert(custom.length() == 3, "trae 3 custom projections");
  assert(custom[0].userId == u1.id && custom[0].userEmail == "alice@example.com");

  // 4. Proyección con struct anónimo
  let anon = UserService.listAnonymous();
  assert(anon.length() == 3, "trae 3 anónimos");
  assert(anon[0].id == u1.id && anon[0].email == "alice@example.com");

  // 5. Proyección ordenada con orderByDesc
  let ordered = UserService.listOrderedDesc();
  assert(ordered.length() == 3, "ordenado por score descendente");
  assert(ordered[0].name == "Bob", "primer puesto Bob con 95");
  assert(ordered[1].name == "Alice", "segundo puesto Alice con 80");
  assert(ordered[2].name == "Charlie", "tercer puesto Charlie con 70");

  // 6. Fallback interpretado para expresiones calculadas
  let uppers = UserService.listUpperNamesCalculated();
  assert(uppers[0] == "ALICE" && uppers[1] == "BOB" && uppers[2] == "CHARLIE");

  let doubles = UserService.listDoubleScoresCalculated();
  assert(doubles[0] == 160 && doubles[1] == 190 && doubles[2] == 140);

  // 7. Respeto de @softDelete
  assert(db.users.delete(u2.id), "soft delete de Bob");
  let namesAfterDelete = UserService.listNames();
  assert(namesAfterDelete.length() == 2, "Bob fue filtrado por soft delete");
  assert(namesAfterDelete[0] == "Alice" && namesAfterDelete[1] == "Charlie");
}
"#;

    let (ok, output) = run_link_tests(program);
    assert!(ok, "test falló: {output}");
}
