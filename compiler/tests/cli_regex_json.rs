// `String.matchAll`/`.match` + `json.tryParse` (GRAMMAR.md §3.290, PLAN.md
// §9.24 Fase 2 ítem F3): el caso real que los motiva -- extraer bloques
// `<script type="application/ld+json">...</script>` de un HTML con regex, y
// parsear el contenido de cada uno sin que un documento externo mal formado
// tumbe toda la request (`json.parse` sí aborta; `json.tryParse` es la
// variante `try*` puntual para este caso, GRAMMAR.md §3.114).
//
// Contra el BINARIO real vía `linkc test` -- incluye el bug real de
// subtipado que escribir este test encontró (`Optional(Dynamic)` no
// subtipaba en `Optional(Row)`, corregido en `types.rs`).

use std::path::PathBuf;
use std::process::Command;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-regex-json-{name}-{}-{}",
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
fn match_all_extracts_ld_json_script_blocks_the_real_motivating_case() {
    let program = r#"
service S {
  rpc extractBlocks(html: String) -> String[] {
    html.matchAll("<script type=\"application/ld\\+json\">.*?</script>")
  }
}

test "extrae los dos bloques ld+json, ignora el resto del HTML" {
  let html = "<html><script type=\"application/ld+json\">{\"a\":1}</script><p>x</p><script type=\"application/ld+json\">{\"b\":2}</script></html>";
  let blocks = S.extractBlocks(html);
  assert(blocks.length() == 2, "cantidad de bloques: " + blocks.length().toString());
  assert(blocks[0].contains("\"a\":1"));
  assert(blocks[1].contains("\"b\":2"));
}

test "sin ninguna coincidencia, matchAll da una lista vacia, no null ni error" {
  let blocks = S.extractBlocks("<html><p>sin scripts aca</p></html>");
  assert(blocks.length() == 0);
}
"#;
    let (ok, text) = run_link_tests(program);
    assert!(ok, "{text}");
    assert!(text.contains("2 passed"), "{text}");
}

#[test]
fn match_returns_the_first_occurrence_or_null() {
    let program = r#"
service S {
  rpc firstNumber(s: String) -> String? {
    s.match("[0-9]+\\.[0-9]+")
  }
}

test "devuelve la primera coincidencia" {
  assert(S.firstNumber("precio: 42.50 EUR, antes 39.99") == "42.50");
}

test "sin coincidencia, devuelve null" {
  assert(S.firstNumber("sin numeros decimales aca") == null);
}
"#;
    let (ok, text) = run_link_tests(program);
    assert!(ok, "{text}");
    assert!(text.contains("2 passed"), "{text}");
}

#[test]
fn an_invalid_regex_pattern_is_a_clean_runtime_error_not_a_panic() {
    let temp = TempDir::new("badpattern");
    let src = temp.write(
        "app.link",
        r#"
service S {
  rpc bad(s: String) -> String? { s.match("(unclosed") }
}
"#,
    );
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("build").arg(&src).arg(temp.0.join("gen")).output().expect("linkc build");
    assert!(out.status.success(), "un patron invalido tipa bien -- 'pattern' es un String de runtime, no un literal validado en compilacion: {}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn json_try_parse_returns_the_value_or_null_never_aborts_on_malformed_json() {
    let program = r#"
type Payload = { x: Int }

service S {
  rpc parseOrNull(s: String) -> Payload? {
    json.tryParse(s)
  }
}

test "JSON valido da el struct" {
  let out = S.parseOrNull("{\"x\": 42}");
  assert(out != null);
}

test "JSON invalido da null, no aborta la request" {
  let out = S.parseOrNull("esto no es json valido {{{");
  assert(out == null);
}
"#;
    let (ok, text) = run_link_tests(program);
    assert!(ok, "{text}");
    assert!(text.contains("2 passed"), "{text}");
}

#[test]
fn a_realistic_schema_validator_pipeline_combining_match_all_and_try_parse() {
    // El caso real completo de F3, PLAN.md §9.24: extraer bloques ld+json de
    // un HTML, parsear cada uno con tryParse (algunos pueden venir mal
    // formados de un proveedor externo), y contar cuántos parsearon bien --
    // sin que un solo bloque roto tumbe el resto del pipeline.
    let program = r#"
service S {
  rpc countValidLdJsonBlocks(html: String) -> Int {
    let blocks = html.matchAll("<script type=\"application/ld\\+json\">.*?</script>");
    let valid = blocks.filter(| b: String | {
      let inner = b.replace("<script type=\"application/ld+json\">", "").replace("</script>", "");
      json.tryParse(inner) != null
    });
    valid.length()
  }
}

test "cuenta solo los bloques que parsean, ignora el roto" {
  let html = "<script type=\"application/ld+json\">{\"a\":1}</script><script type=\"application/ld+json\">esto no es json</script><script type=\"application/ld+json\">{\"b\":2}</script>";
  assert(S.countValidLdJsonBlocks(html) == 2);
}
"#;
    let (ok, text) = run_link_tests(program);
    assert!(ok, "{text}");
    assert!(text.contains("1 passed"), "{text}");
}
