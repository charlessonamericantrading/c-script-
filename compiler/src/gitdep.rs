// Dependencias git reales (GRAMMAR.md §2.1, package manager) -- `git+<url>#<rev>`
// en `link.json` se resuelve clonando/actualizando un caché local y
// haciendo checkout del rev pedido, invocando el binario `git` real vía
// subproceso. Sin ningún cliente git en Rust: misma filosofía que el
// resto del proyecto (usar la herramienta real en vez de reimplementarla
// -- `rusqlite` hace lo mismo con SQLite en vez de un motor de storage
// propio, GRAMMAR.md §3.17).

use crate::lockfile::{hash_source, GitLockEntry};
use std::path::{Path, PathBuf};
use std::process::Command;

/// `git+<url>#<rev>` -- AMBOS obligatorios en v0. Sin un rev explícito,
/// "la última versión" no tiene un significado bien definido sin un
/// registro que ordene versiones (no hay ninguno, a propósito -- ver
/// PLAN.md §8.3), y resolver contra la rama default de cada remoto sería
/// una fuente de builds NO reproducibles desde el día 1 -- exactamente el
/// problema que un package manager existe para resolver, no para
/// reintroducir de vuelta.
pub fn parse_spec(spec: &str) -> Result<(&str, &str), String> {
    let Some(rest) = spec.strip_prefix("git+") else {
        return Err(format!("'{spec}' no es una dependencia git (falta el prefijo 'git+')"));
    };
    let Some((url, rev)) = rest.rsplit_once('#') else {
        return Err(format!(
            "'{spec}' necesita un rev explícito ('git+<url>#<tag-o-rama-o-commit>') -- v0 no resuelve \"la última versión\" sin un rev pedido"
        ));
    };
    if url.is_empty() || rev.is_empty() {
        return Err(format!("'{spec}' tiene una URL o un rev vacío"));
    }
    Ok((url, rev))
}

fn run_git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git").args(args).current_dir(dir).output().map_err(|e| {
        format!("no se pudo ejecutar 'git {}': {e} -- ¿está git instalado y en el PATH?", args.join(" "))
    })?;
    if !output.status.success() {
        return Err(format!(
            "'git {}' falló ({}): {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// El directorio de caché para `url`, DENTRO de `project_root` -- un clon
/// real por URL distinta (`hash_source`, la misma función SHA-256 que ya
/// usa `link.lock`, reusada acá en vez de sumar una segunda forma de
/// hashear). No es un caché GLOBAL (a diferencia de `~/.cargo/registry`)
/// a propósito: evita depender de localizar el directorio de caché del
/// usuario (`dirs`/`home`, una dependencia nueva que este v0 no necesita)
/// y evita que dos proyectos distintos compartan (y potencialmente
/// corrompan concurrentemente) el mismo clon -- el costo es re-clonar la
/// misma dependencia una vez por proyecto que la use, aceptable en v0.
fn cache_dir_for(project_root: &Path, url: &str) -> PathBuf {
    project_root.join(".linkc").join("cache").join(hash_source(url))
}

/// Clona (si hace falta), actualiza (si el rev pedido no está localmente
/// todavía) y hace checkout de `spec` (`git+<url>#<rev>`) dentro del
/// caché de `project_root`. Devuelve el directorio ya en el commit
/// correcto -- el punto de entrada dentro de él es convención de quien
/// llama (`modules.rs` asume `main.link` en la raíz, mismo nombre que
/// `linkc new` ya scaffoldea) -- más el `GitLockEntry` para grabar en
/// `link.lock`.
///
/// Sin locking entre procesos concurrentes (dos `linkc build` corriendo
/// a la vez sobre el mismo proyecto podrían pisarse el mismo clon) --
/// límite de v0 conocido, no manejado.
pub fn resolve(spec: &str, project_root: &Path) -> Result<(PathBuf, GitLockEntry), String> {
    let (url, rev) = parse_spec(spec)?;
    let checkout_dir = cache_dir_for(project_root, url);

    if !checkout_dir.exists() {
        let parent = checkout_dir.parent().expect("cache_dir_for siempre da un directorio con padre");
        std::fs::create_dir_all(parent).map_err(|e| format!("no se pudo crear '{}': {e}", parent.display()))?;
        // `display_path` (modules.rs) le pela el prefijo `\\?\` que
        // `fs::canonicalize` antepone en Windows -- ahí existe para
        // texto que un humano lee, pero acá hace falta por una razón
        // distinta y más dura: git (vía su capa MSYS/Cygwin) directamente
        // NO ENTIENDE ese prefijo como argumento de línea de comandos --
        // "fatal: could not create work tree dir ... Invalid argument"
        // en vez de un simple problema estético. Mismo string, dos
        // motivos.
        let checkout_str = crate::modules::display_path(&checkout_dir);
        run_git(parent, &["clone", url, &checkout_str])?;
    }

    // ¿el rev pedido ya resuelve localmente (un commit ya presente, o un
    // tag/rama ya conocidos de un fetch anterior)? Si no, un fetch
    // actualiza el clon existente antes de reintentar -- evita un fetch
    // de red en el caso común (mismo rev que la corrida anterior, ya
    // cacheado).
    if run_git(&checkout_dir, &["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")]).is_err() {
        run_git(&checkout_dir, &["fetch", "--all", "--tags", "--force"])?;
    }

    run_git(&checkout_dir, &["checkout", "--detach", "--force", rev])?;
    let resolved = run_git(&checkout_dir, &["rev-parse", "HEAD"])?;

    Ok((checkout_dir, GitLockEntry { url: url.to_string(), rev: rev.to_string(), resolved }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Un repo git real y LOCAL como "remoto" -- ningún test de este
    /// módulo toca la red. `git clone`/`fetch` contra una ruta local
    /// (en vez de una URL http(s)/ssh) es exactamente el mismo camino de
    /// código real de git, no un mock -- lo único distinto es el
    /// transporte.
    struct FixtureRemote(PathBuf);
    impl FixtureRemote {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("cscript-gitdep-fixture-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            run_git(&dir, &["init", "--quiet", "-b", "main"]).unwrap();
            run_git(&dir, &["config", "user.email", "test@example.com"]).unwrap();
            run_git(&dir, &["config", "user.name", "Test"]).unwrap();
            FixtureRemote(dir)
        }

        fn commit_file(&self, rel: &str, contents: &str, message: &str) -> String {
            fs::write(self.0.join(rel), contents).unwrap();
            run_git(&self.0, &["add", rel]).unwrap();
            run_git(&self.0, &["commit", "--quiet", "-m", message]).unwrap();
            run_git(&self.0, &["rev-parse", "HEAD"]).unwrap()
        }

        fn tag(&self, name: &str) {
            run_git(&self.0, &["tag", name]).unwrap();
        }

        fn url(&self) -> String {
            self.0.to_string_lossy().replace('\\', "/")
        }
    }
    impl Drop for FixtureRemote {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct TempProject(PathBuf);
    impl TempProject {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("cscript-gitdep-project-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            TempProject(dir)
        }
    }
    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn parse_spec_requires_the_git_plus_prefix() {
        assert!(parse_spec("./local.link").is_err());
        assert!(parse_spec("https://example.com/repo.git#v1").is_err(), "sin el prefijo 'git+', no es una dependencia git");
    }

    #[test]
    fn parse_spec_requires_an_explicit_rev() {
        let result = parse_spec("git+https://example.com/repo.git");
        assert!(result.is_err(), "sin '#rev', v0 no resuelve 'la última versión'");
    }

    #[test]
    fn parse_spec_splits_url_and_rev_on_the_last_hash() {
        let (url, rev) = parse_spec("git+https://example.com/repo.git#v1.2.0").unwrap();
        assert_eq!(url, "https://example.com/repo.git");
        assert_eq!(rev, "v1.2.0");
    }

    #[test]
    fn resolve_clones_and_checks_out_a_tagged_commit() {
        let remote = FixtureRemote::new("tag");
        remote.commit_file("main.link", "type Point = { x: Int }", "primero");
        let v1_sha = remote.commit_file("main.link", "type Point = { x: Int, y: Int }", "segundo");
        remote.tag("v1.0.0");

        let project = TempProject::new("tag");
        let spec = format!("git+{}#v1.0.0", remote.url());
        let (checkout_dir, lock) = resolve(&spec, &project.0).expect("debe resolver un tag real");

        assert_eq!(lock.rev, "v1.0.0");
        assert_eq!(lock.resolved, v1_sha, "el commit resuelto debe ser exactamente al que apunta el tag");
        let content = fs::read_to_string(checkout_dir.join("main.link")).unwrap();
        assert!(content.contains('y'), "debe tener el contenido del commit taggeado: {content}");
    }

    #[test]
    fn resolve_reuses_the_cache_on_a_second_call_without_reaching_the_network() {
        let remote = FixtureRemote::new("cache-reuse");
        remote.commit_file("main.link", "type Point = { x: Int }", "init");
        remote.tag("v1.0.0");

        let project = TempProject::new("cache-reuse");
        let spec = format!("git+{}#v1.0.0", remote.url());
        let (dir1, lock1) = resolve(&spec, &project.0).unwrap();

        // Borrar el remoto simula "sin red" -- si `resolve` intentara
        // clonar o fetchear de nuevo acá, fallaría; si reusa el caché ya
        // resuelto sin tocar el remoto, sigue funcionando igual.
        drop(remote);

        let (dir2, lock2) = resolve(&spec, &project.0).expect("la segunda resolución no debería necesitar red");
        assert_eq!(dir1, dir2);
        assert_eq!(lock1.resolved, lock2.resolved);
    }

    #[test]
    fn resolve_can_check_out_a_specific_commit_sha_directly() {
        let remote = FixtureRemote::new("commit-sha");
        let sha = remote.commit_file("main.link", "type Point = { x: Int }", "único commit");

        let project = TempProject::new("commit-sha");
        let spec = format!("git+{}#{sha}", remote.url());
        let (_, lock) = resolve(&spec, &project.0).expect("debe poder resolver un commit SHA exacto, no solo tags");
        assert_eq!(lock.resolved, sha);
    }

    #[test]
    fn resolve_fetches_when_a_new_tag_was_pushed_after_the_first_clone() {
        let remote = FixtureRemote::new("new-tag-after-clone");
        remote.commit_file("main.link", "type Point = { x: Int }", "v1");
        remote.tag("v1.0.0");

        let project = TempProject::new("new-tag-after-clone");
        let spec_v1 = format!("git+{}#v1.0.0", remote.url());
        resolve(&spec_v1, &project.0).expect("debe clonar y resolver v1.0.0");

        // v2.0.0 no existía todavía cuando se clonó -- resolve debe
        // fetchear el clon YA CACHEADO para encontrarlo, no fallar
        // asumiendo que el caché ya tiene todo lo que hará falta.
        let v2_sha = remote.commit_file("main.link", "type Point = { x: Int, y: Int, z: Int }", "v2");
        remote.tag("v2.0.0");

        let spec_v2 = format!("git+{}#v2.0.0", remote.url());
        let (_, lock) = resolve(&spec_v2, &project.0).expect("debe fetchear el tag nuevo sobre el clon ya cacheado");
        assert_eq!(lock.resolved, v2_sha);
    }

    /// Encontrado como gap real por un reparso: solo el camino feliz de
    /// `resolve` tenía cobertura -- un rev inexistente nunca se había
    /// probado. Importa que falle RUIDOSO (`Err`, mensaje real) en vez de,
    /// por ejemplo, quedarse silenciosamente en el HEAD de la rama default
    /// del clon (`checkout --force` sin `--detach` explícito ya haría
    /// justamente eso si el argumento fuera inválido de otra forma) --
    /// exactamente el tipo de "no reproducible" que `#<rev>` obligatorio
    /// existe para prevenir.
    #[test]
    fn resolve_fails_loudly_on_a_nonexistent_rev() {
        let remote = FixtureRemote::new("bad-rev");
        remote.commit_file("main.link", "type Point = { x: Int }", "único commit");

        let project = TempProject::new("bad-rev");
        let spec = format!("git+{}#no-existe-este-tag", remote.url());
        let result = resolve(&spec, &project.0);

        let err = result.expect_err("un rev que no existe en el remoto debe fallar, nunca resolver silenciosamente a otra cosa");
        assert!(!err.is_empty(), "el error debe traer un mensaje real, no una cadena vacía");
    }

    /// Mismo espíritu que arriba, pero para el clon en sí -- una URL que
    /// `git clone` no puede alcanzar (acá, un directorio local que
    /// directamente no existe, sin necesitar tocar la red para que el
    /// test sea determinista) debe fallar en el paso de CLONE, con un
    /// mensaje que lo diga, no colgarse ni devolver un directorio a medio
    /// clonar como si hubiera funcionado.
    #[test]
    fn resolve_fails_loudly_when_the_remote_is_unreachable() {
        let project = TempProject::new("unreachable-remote");
        let nonexistent_remote = project.0.join("no-existe-ningun-repo-aca");
        let spec = format!("git+{}#main", nonexistent_remote.to_string_lossy().replace('\\', "/"));

        let result = resolve(&spec, &project.0);

        let err = result.expect_err("un remoto inalcanzable debe fallar el clone, no resolver como si nada");
        assert!(!err.is_empty(), "el error debe traer un mensaje real, no una cadena vacía");
    }
}
