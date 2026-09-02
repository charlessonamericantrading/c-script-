// `sumBy`/`countBy` con una clave de agrupación NULLABLE (`channel:
// String?`) -- GRAMMAR.md §3.231, PLAN.md §9.19 ítem 6 -- contra el
// BINARIO real con `linkc test`: checker Y runtime. El caso real: el
// desglose por canal/negocio del CRM sobre columnas adoptadas que son
// nullable, que §3.52 rechazaba de plano.

use std::path::PathBuf;
use std::process::Command;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-group-null-{name}-{}-{}",
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
type Sale = { id: Int, channel: String?, cents: Int }
type NewSale = { channel: String?, cents: Int }
type ByChannel = { key: String?, value: Int }
db { sales: Sale[] }

service S {
  rpc add(channel: String?, cents: Int) -> Sale { db.sales.insert(NewSale { channel: channel, cents: cents }) }
  rpc totals() -> ByChannel[] { db.sales.sumBy(|s: Sale| { s.channel }, |s: Sale| { s.cents }) }
  rpc counts() -> ByChannel[] { db.sales.countBy(|s: Sale| { s.channel }) }
  rpc unattributed() -> Int {
    db.sales.sumBy(|s: Sale| { s.channel }, |s: Sale| { s.cents })
      .filter(|g: ByChannel| { g.key == null })
      .map(|g: ByChannel| { g.value })
      .sum()
  }
  rpc attributed() -> Int {
    db.sales.sumBy(|s: Sale| { s.channel }, |s: Sale| { s.cents })
      .filter(|g: ByChannel| { g.key != null })
      .map(|g: ByChannel| { g.value })
      .sum()
  }
}

test "a nullable group key makes the null rows one more group" {
  let a = S.add("web", 100);
  let b = S.add(null, 30);
  let c = S.add("web", 50);
  let d = S.add(null, 20);
  let e = S.add("shop", 5);
  assert(S.totals().length() == 3, "web, shop and the null group");
  assert(S.counts().length() == 3, "countBy has the same three groups");
  assert(S.unattributed() == 50, "the null group sums only its own rows");
  assert(S.attributed() == 155, "the rest is untouched");
}
"#;

#[test]
fn a_nullable_group_key_groups_the_null_rows_together_end_to_end() {
    let (ok, out) = run_link_tests(PROGRAM);
    assert!(ok, "{out}");
}

#[test]
fn a_key_optional_field_is_still_rejected_with_a_message_that_names_the_fix() {
    let program = r#"
type Sale = { id: Int, channel?: String, cents: Int }
type ByChannel = { key: String?, value: Int }
db { sales: Sale[] }
service S {
  rpc totals() -> ByChannel[] { db.sales.sumBy(|s: Sale| { s.channel }, |s: Sale| { s.cents }) }
}
test "never runs" { assert(true, "unreachable"); }
"#;
    let (ok, out) = run_link_tests(program);
    assert!(!ok, "{out}");
    assert!(out.contains("opcional por clave"), "{out}");
}
