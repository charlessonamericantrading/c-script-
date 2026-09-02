// El bloque `ai { alias: "spec" }` (GRAMMAR.md §3.234, PLAN.md §9.20 Eje G
// ítem 2) contra el BINARIO real: `linkc serve` se niega a ARRANCAR si un
// modelo declarado no existe en esta máquina (con la lista completa de los
// que faltan), `linkc doctor` lo reporta como [ERROR], y con `--models-dir`
// apuntando a un directorio donde el .gguf sí está, `doctor` da [OK] por
// alias. Ningún test carga un modelo real (eso es del ítem 3, contra un
// GGUF de verdad): acá solo se resuelve la existencia del archivo.

use std::path::PathBuf;
use std::process::Command;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-ai-block-{name}-{}-{}",
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
ai { router: "modelo-que-no-existe:latest", coder: "./coder.gguf" }
type Ping = { id: Int, n: Int }
db { pings: Ping[] }
service S { rpc ping() -> Int { 1 } }
"#;

#[test]
fn serve_refuses_to_start_when_a_declared_model_is_missing_and_lists_every_missing_alias() {
    let temp = TempDir::new("serve");
    let src = temp.write("app.link", PROGRAM);
    // Sin --restart-backoff el arranque se intenta UNA vez: un modelo que
    // falta es un error inmediato, no un reintento infinito.
    let (ok, out) = linkc(&[std::ffi::OsStr::new("serve"), src.as_os_str(), std::ffi::OsStr::new("39877")]);
    assert!(!ok, "{out}");
    assert!(out.contains("no encontrados"), "{out}");
    assert!(out.contains("router") && out.contains("coder"), "tiene que listar TODOS los que faltan: {out}");
    assert!(out.contains("§3.234"), "{out}");
}

#[test]
fn doctor_reports_missing_models_as_errors_and_resolved_ones_per_alias() {
    let temp = TempDir::new("doctor");
    let src = temp.write("app.link", PROGRAM);
    let (ok, out) = linkc(&[std::ffi::OsStr::new("doctor"), src.as_os_str()]);
    assert!(!ok, "{out}");
    assert!(out.contains("[ERROR] modelos de 'ai { }' no encontrados"), "{out}");

    // Con los dos modelos presentes en --models-dir (el alias de Ollama como
    // <nombre-tag>.gguf, la ruta relativa tal cual), doctor pasa en verde.
    let models = temp.0.join("models");
    std::fs::create_dir_all(&models).unwrap();
    std::fs::write(models.join("modelo-que-no-existe-latest.gguf"), b"stub").unwrap();
    std::fs::write(models.join("coder.gguf"), b"stub").unwrap();
    let (ok, out) = linkc(&[std::ffi::OsStr::new("doctor"), src.as_os_str(), std::ffi::OsStr::new("--models-dir"), models.as_os_str()]);
    assert!(ok, "{out}");
    assert!(out.contains("[OK]    modelo 'router':") && out.contains("[OK]    modelo 'coder':"), "{out}");
}

#[test]
fn a_program_without_an_ai_block_is_untouched_and_ai_is_still_an_identifier() {
    let temp = TempDir::new("plain");
    let src = temp.write("app.link", "type Ai = { id: Int, ai: Int }\ndb { ai: Ai[] }\nservice S { rpc ai() -> Int { db.ai.all().length() } }\ntest \"ai\" { assert(S.ai() == 0, \"ai\"); }\n");
    let (ok, out) = linkc(&[std::ffi::OsStr::new("test"), src.as_os_str()]);
    assert!(ok, "{out}");
    let (ok, out) = linkc(&[std::ffi::OsStr::new("doctor"), src.as_os_str()]);
    assert!(ok, "{out}");
    assert!(!out.contains("modelo '"), "{out}");
}
