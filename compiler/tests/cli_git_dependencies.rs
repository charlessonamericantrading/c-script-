// Test de integración: `linkc build` como SUBPROCESO REAL con una
// dependencia `git+<url>#<rev>` real en `link.json` (GRAMMAR.md §2.1,
// package manager) -- un repo git LOCAL hace de "remoto" (mismo camino de
// código real de git que un remoto http(s)/ssh, sin tocar la red) y se
// verifica el pipeline completo: `linkc build` clona/resuelve la
// dependencia, el import type-checkea contra ella, y `link.lock` graba el
// commit SHA real resuelto.

use std::io::Write;
use std::process::{Command, Stdio};

fn run_git(dir: &std::path::Path, args: &[&str]) {
    let status = Command::new("git").args(args).current_dir(dir).status().expect("no se pudo ejecutar git");
    assert!(status.success(), "git {args:?} falló en {}", dir.display());
}

struct TempDir(std::path::PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("cscript-cli-git-dep-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
    fn write(&self, rel: &str, contents: &str) -> std::path::PathBuf {
        let path = self.0.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        path
    }
    fn url(&self) -> String {
        self.0.to_string_lossy().replace('\\', "/")
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn linkc_build_resolves_a_real_git_dependency_and_records_it_in_link_lock() {
    let remote = TempDir::new("remote");
    run_git(&remote.0, &["init", "--quiet", "-b", "main"]);
    run_git(&remote.0, &["config", "user.email", "test@example.com"]);
    run_git(&remote.0, &["config", "user.name", "Test"]);
    remote.write("main.link", "type Circle = { r: Int }\n");
    run_git(&remote.0, &["add", "main.link"]);
    run_git(&remote.0, &["commit", "--quiet", "-m", "init"]);
    run_git(&remote.0, &["tag", "v1.0.0"]);
    let remote_sha_output = Command::new("git").args(["rev-parse", "HEAD"]).current_dir(&remote.0).output().unwrap();
    let remote_sha = String::from_utf8_lossy(&remote_sha_output.stdout).trim().to_string();

    let project = TempDir::new("project");
    project.write("link.json", &format!(r#"{{ "dependencies": {{ "shapes": "git+{}#v1.0.0" }} }}"#, remote.url()));
    let entry = project.write(
        "a.link",
        "import { Circle } from \"shapes\";\nfn unit_circle() -> Circle { Circle { r: 1 } }\n",
    );
    let outdir = project.0.join("gen");

    let output = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("build")
        .arg(&entry)
        .arg(&outdir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("no se pudo iniciar 'linkc build'");
    assert!(
        output.status.success(),
        "linkc build debe resolver la dependencia git y tipar bien -- stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(outdir.join("contract.d.ts").exists(), "el build debe haber generado el contrato real");

    let lock_content = std::fs::read_to_string(project.0.join("link.lock")).expect("link.lock debe existir");
    let lock: linkc::lockfile::Lockfile = serde_json::from_str(&lock_content).expect("link.lock debe ser JSON válido");
    let git_entry = lock.git_dependencies.get("shapes").expect("link.lock debe registrar la dependencia git 'shapes'");
    assert_eq!(git_entry.rev, "v1.0.0");
    assert_eq!(git_entry.resolved, remote_sha, "el commit resuelto debe ser exactamente el HEAD real del fixture");
}
