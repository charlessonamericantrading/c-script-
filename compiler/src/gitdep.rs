// Dependencias git reales (GRAMMAR.md §2.1, package manager) -- `git+<url>#<rev>`
// en `link.json` se resuelve clonando/actualizando un caché local y
// haciendo checkout del rev pedido, invocando el binario `git` real vía
// subproceso. Sin ningún cliente git en Rust: misma filosofía que el
// resto del proyecto (usar la herramienta real en vez de reimplementarla
// -- `rusqlite` hace lo mismo con SQLite en vez de un motor de storage
// propio, GRAMMAR.md §3.17).

use crate::lockfile::{hash_source, GitLockEntry};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

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

/// Exclusión mutua ENTRE PROCESOS sobre el directorio de caché de una
/// dependencia puntual (GRAMMAR.md §2.1) -- dos `linkc build`/`serve`
/// corriendo a la vez sobre el mismo proyecto podían pisarse el mismo
/// clon (un `fetch` de uno interrumpido por el `checkout --force` del
/// otro, por ejemplo). Advisory, basado en un archivo (`<hash>.lock`
/// junto al directorio de caché) -- no un lock real de sistema operativo
/// (`flock`/`LockFileEx`, que pediría FFI a mano en dos plataformas
/// distintas o una dependencia nueva) para un caso que en la práctica es
/// raro; proporcional al riesgo real, no al máximo rigor posible.
const CACHE_LOCK_STALE_AFTER: Duration = Duration::from_secs(120);
const CACHE_LOCK_POLL_INTERVAL: Duration = Duration::from_millis(100);
const CACHE_LOCK_WAIT_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug)]
struct CacheLock {
    path: PathBuf,
}

impl CacheLock {
    /// Bloquea hasta tomar el lock (creando `<cache_dir>.lock` de forma
    /// atómica, `create_new`) o hasta `CACHE_LOCK_WAIT_TIMEOUT` -- lo que
    /// pase primero. Un lock más viejo que `CACHE_LOCK_STALE_AFTER` se
    /// considera abandonado (el proceso que lo tomó murió sin soltarlo --
    /// un panic, un `kill -9`) y se borra para reintentar, en vez de
    /// bloquear para siempre.
    fn acquire(cache_dir: &Path) -> Result<Self, String> {
        let lock_path = cache_dir.with_extension("lock");
        // El propio directorio de caché puede no existir todavía (primera
        // vez que se resuelve esta URL) -- el lock vive junto a él
        // (sibling, no adentro), pero su PADRE (`.linkc/cache/`) sí tiene
        // que existir para poder crear el archivo del lock.
        if let Some(parent) = lock_path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("no se pudo crear '{}': {e}", parent.display()))?;
        }
        let start = Instant::now();
        loop {
            match OpenOptions::new().write(true).create_new(true).open(&lock_path) {
                Ok(mut f) => {
                    let _ = write!(f, "{}", std::process::id());
                    return Ok(CacheLock { path: lock_path });
                }
                // CUALQUIER error al crear el archivo se trata como "lock
                // ocupado, reintentar" hasta el timeout -- no solo
                // `AlreadyExists` (GRAMMAR.md §3.227). En Windows, un lock que
                // otro hilo/proceso acaba de `remove_file` queda un instante en
                // "pending delete" y `create_new` sobre ese nombre devuelve
                // ACCESS_DENIED o SHARING_VIOLATION (según la versión del SO y
                // de la stdlib llegan como `PermissionDenied` o como
                // `Uncategorized`), no "ya existe": el lock sigue OCUPADO (se
                // está soltando), no roto. Sin esto, la carrera entre el `Drop`
                // de un holder y el `acquire` del siguiente terminaba en un
                // error DEFINITIVO en vez de un reintento -- visto en CI
                // (windows-latest, v1.184.0) y reproducido en local (1 de 5
                // corridas), nunca en Linux, donde unlink es atómico. Un error
                // persistente (permisos de verdad, disco lleno) sigue saliendo:
                // por el timeout de abajo, con el texto del último error.
                Err(last_error) => {
                    if let Ok(meta) = fs::metadata(&lock_path) {
                        if let Ok(modified) = meta.modified() {
                            if let Ok(age) = modified.elapsed() {
                                if age > CACHE_LOCK_STALE_AFTER {
                                    let _ = fs::remove_file(&lock_path);
                                    continue;
                                }
                            }
                        }
                    }
                    if start.elapsed() > CACHE_LOCK_WAIT_TIMEOUT {
                        return Err(format!(
                            "no se pudo tomar el lock de caché '{}' tras {}s (último error: {last_error}) -- otro 'linkc' parece \
                             estar resolviendo la misma dependencia; si no hay ningún otro proceso corriendo, borrá ese archivo a mano",
                            lock_path.display(),
                            CACHE_LOCK_WAIT_TIMEOUT.as_secs()
                        ));
                    }
                    std::thread::sleep(CACHE_LOCK_POLL_INTERVAL);
                }
            }
        }
    }
}

impl Drop for CacheLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// `true` si `rev` ya es un commit SHA completo (40 hex) -- inmutable por
/// definición, a diferencia de un tag (movible en teoría, aunque mala
/// práctica moverlo) o una rama (movible por diseño). Usado para decidir
/// cuándo un fetch es realmente necesario, ver `resolve`.
fn is_full_commit_sha(rev: &str) -> bool {
    rev.len() == 40 && rev.chars().all(|c| c.is_ascii_hexdigit())
}

/// Clona (si hace falta), actualiza (si hace falta) y hace checkout de
/// `spec` (`git+<url>#<rev>`) dentro del caché de `project_root`. Devuelve
/// el directorio ya en el commit correcto -- el punto de entrada dentro de
/// él es convención de quien llama (`modules.rs` asume `main.link` en la
/// raíz, mismo nombre que `linkc new` ya scaffoldea) -- más el
/// `GitLockEntry` para grabar en `link.lock`.
///
/// Resolución FRESCA -- siempre le pregunta al remoto qué es lo más
/// reciente para `rev` (salvo que `rev` ya sea un commit SHA completo, o
/// un tag que ya resuelve en el caché local -- ninguno de los dos puede
/// haber cambiado). Este es el camino que corre la PRIMERA vez que se ve
/// una dependencia, o cuando `link.json` cambió su `rev`, o bajo
/// `--update-deps` -- para builds repetidos que ya tienen un pin en
/// `link.lock`, `modules.rs` llama a `resolve_pinned` en cambio, que NUNCA
/// vuelve a preguntarle nada al remoto sobre qué es "lo último".
pub fn resolve(spec: &str, project_root: &Path) -> Result<(PathBuf, GitLockEntry), String> {
    let (url, rev) = parse_spec(spec)?;
    let checkout_dir = cache_dir_for(project_root, url);
    let _lock = CacheLock::acquire(&checkout_dir)?;

    if !checkout_dir.exists() {
        clone_into(url, &checkout_dir)?;
    }

    // Bug real encontrado corriendo esto, no leyéndolo (GRAMMAR.md §2.1):
    // antes de este fix, `rev-parse --verify rev^{commit}` ya resolvía
    // localmente para una RAMA apenas clonada (el clone deja
    // `refs/heads/<rama>` local, apuntando al commit de ESE momento) --
    // así que una dependencia por rama quedaba congelada en el commit del
    // PRIMER clone para siempre, sin importar cuánto avanzara el remoto,
    // aunque la documentación afirmara "se re-resuelve contra su HEAD real
    // en cada build". Un commit SHA completo es inmutable por definición
    // (nunca hace falta red si ya está local); un TAG que ya resuelve
    // contra `refs/tags/<rev>` específicamente se trata igual (moverlo ya
    // publicado es mala práctica, no algo que este resolver defienda
    // activamente) -- CUALQUIER OTRA COSA (una rama, o un rev que ni
    // siquiera se conoce localmente todavía) SIEMPRE fetchea: es la única
    // forma real de saber si el remoto avanzó.
    let trust_local_cache = is_full_commit_sha(rev)
        || run_git(&checkout_dir, &["rev-parse", "--verify", "--quiet", &format!("refs/tags/{rev}")]).is_ok();
    let already_resolves = run_git(&checkout_dir, &["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")]).is_ok();
    if !(trust_local_cache && already_resolves) {
        run_git(&checkout_dir, &["fetch", "--all", "--tags", "--force"])?;
    }

    // Segunda mitad del mismo bug: `git fetch` por sí solo NUNCA mueve una
    // rama LOCAL (`refs/heads/<rama>`, la que `checkout <rev>` resuelve
    // primero por las reglas de `gitrevisions(7)`) -- solo actualiza el
    // ref de SEGUIMIENTO remoto (`refs/remotes/origin/<rama>`). Sin este
    // paso, el fetch de arriba traía el commit nuevo al caché, pero el
    // checkout de abajo seguía resolviendo a la rama local vieja de todas
    // formas -- confirmado a mano contra un repo real antes de este fix.
    // Si existe un ref de seguimiento remoto para `rev`, ESE es el que
    // manda (recién actualizado); si no (un tag, un SHA), `rev` tal cual
    // sigue siendo correcto -- los tags fetcheados van directo a
    // `refs/tags/`, sin este problema de dos copias.
    let checkout_target = if run_git(&checkout_dir, &["rev-parse", "--verify", "--quiet", &format!("refs/remotes/origin/{rev}")]).is_ok() {
        format!("refs/remotes/origin/{rev}")
    } else {
        rev.to_string()
    };
    run_git(&checkout_dir, &["checkout", "--detach", "--force", &checkout_target])?;
    let resolved = run_git(&checkout_dir, &["rev-parse", "HEAD"])?;

    Ok((checkout_dir, GitLockEntry { url: url.to_string(), rev: rev.to_string(), resolved }))
}

/// Resuelve una dependencia ya FIJADA por un `link.lock` existente (mismo
/// `url`/`rev` que la última vez, GRAMMAR.md §2.1) -- en vez de volver a
/// preguntarle al remoto "¿a qué resuelve `rev` AHORA?", hace checkout
/// DIRECTO al commit exacto ya resuelto antes (`pinned_commit`). Un
/// `linkc build` repetido sobre el mismo `link.json` (sin `--update-deps`)
/// es reproducible byte a byte -- una rama que avanzó en el remoto NO se
/// sigue hasta que alguien lo pide explícitamente, el mismo contrato que
/// cualquier lockfile real promete (`Cargo.lock`/`package-lock.json`).
///
/// Solo toca la red si el commit fijado no está YA en el caché local (ej.
/// un `.linkc/cache` recién clonado en otra máquina que nunca lo vio) --
/// como mucho un fetch, y nunca vuelve a intentar RESOLVER `rev` contra el
/// remoto: el pin YA ES el commit exacto, no hay ninguna ambigüedad de
/// rama/tag que resolver, así que el bug de `resolve` que el comentario de
/// arriba describe ni siquiera puede aplicar acá.
pub fn resolve_pinned(url: &str, pinned_commit: &str, project_root: &Path) -> Result<PathBuf, String> {
    let checkout_dir = cache_dir_for(project_root, url);
    let _lock = CacheLock::acquire(&checkout_dir)?;

    if !checkout_dir.exists() {
        clone_into(url, &checkout_dir)?;
    }
    if run_git(&checkout_dir, &["rev-parse", "--verify", "--quiet", &format!("{pinned_commit}^{{commit}}")]).is_err() {
        run_git(&checkout_dir, &["fetch", "--all", "--tags", "--force"])?;
    }
    run_git(&checkout_dir, &["checkout", "--detach", "--force", pinned_commit])?;
    Ok(checkout_dir)
}

fn clone_into(url: &str, checkout_dir: &Path) -> Result<(), String> {
    let parent = checkout_dir.parent().expect("cache_dir_for siempre da un directorio con padre");
    fs::create_dir_all(parent).map_err(|e| format!("no se pudo crear '{}': {e}", parent.display()))?;
    // `display_path` (modules.rs) le pela el prefijo `\\?\` que
    // `fs::canonicalize` antepone en Windows -- ahí existe para texto que
    // un humano lee, pero acá hace falta por una razón distinta y más
    // dura: git (vía su capa MSYS/Cygwin) directamente NO ENTIENDE ese
    // prefijo como argumento de línea de comandos -- "fatal: could not
    // create work tree dir ... Invalid argument" en vez de un simple
    // problema estético. Mismo string, dos motivos.
    let checkout_str = crate::modules::display_path(checkout_dir);
    run_git(parent, &["clone", url, &checkout_str])?;
    Ok(())
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

    /// El bug real que motivó el fix de `resolve` (GRAMMAR.md §2.1): antes,
    /// una dependencia por RAMA quedaba congelada en el commit del primer
    /// clone para siempre, porque `refs/heads/<rama>` ya resolvía
    /// localmente después de clonar y el chequeo viejo nunca fetcheaba de
    /// nuevo. Repro exacto: clonar con `rev = "main"`, avanzar el remoto,
    /// resolver "main" DE NUEVO -- debe traer el commit nuevo, no el viejo.
    #[test]
    fn resolve_follows_a_branch_that_advanced_on_the_remote_after_the_first_clone() {
        let remote = FixtureRemote::new("branch-advances");
        remote.commit_file("main.link", "type Point = { x: Int }", "v1");

        let project = TempProject::new("branch-advances");
        let spec = format!("git+{}#main", remote.url());
        let (_, lock1) = resolve(&spec, &project.0).expect("debe clonar y resolver 'main'");

        let v2_sha = remote.commit_file("main.link", "type Point = { x: Int, y: Int }", "v2 -- el remoto avanzó");

        let (checkout_dir, lock2) = resolve(&spec, &project.0).expect("debe volver a resolver 'main'");
        assert_eq!(
            lock2.resolved, v2_sha,
            "una dependencia por rama tiene que seguir al remoto que avanzó, no quedarse congelada en el commit del primer clone \
             (bug real: lock1={:?} era el commit viejo)",
            lock1.resolved
        );
        let content = fs::read_to_string(checkout_dir.join("main.link")).unwrap();
        assert!(content.contains("y: Int"), "el checkout tiene que reflejar el contenido del commit nuevo: {content}");
    }

    /// Contraparte del test de arriba: un TAG ya conocido localmente, o un
    /// commit SHA completo, NO deben disparar un fetch -- son inmutables
    /// (o tratados como tales), así que `resolve` puede confiar en el
    /// caché sin tocar la red. Verificado indirectamente por
    /// `resolve_reuses_the_cache_on_a_second_call_without_reaching_the_network`
    /// (con un tag, borra el remoto entero) y
    /// `resolve_can_check_out_a_specific_commit_sha_directly` (con un SHA) --
    /// ambos siguen pasando después de este fix, lo que confirma que el
    /// fetch-siempre nuevo quedó acotado a ramas/revs desconocidos, no a
    /// TODO rev.
    #[test]
    fn resolve_does_not_refetch_for_an_already_known_tag_or_full_sha() {
        assert!(is_full_commit_sha("a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2"), "40 hex chars debe contar como SHA completo");
        assert!(!is_full_commit_sha("v1.0.0"), "un tag no es un SHA completo");
        assert!(!is_full_commit_sha("a1b2c3d"), "un SHA corto (abreviado) no cuenta -- podría ser ambiguo");
    }

    /// `resolve_pinned` (el camino que toma un build repetido con
    /// `link.lock` ya escrito, GRAMMAR.md §2.1): hace checkout DIRECTO al
    /// commit fijado, sin resolver `rev` de nuevo -- así que aunque el
    /// remoto avance, el pin se queda exactamente donde estaba hasta que
    /// alguien pida `--update-deps` explícitamente.
    #[test]
    fn resolve_pinned_stays_on_the_locked_commit_even_after_the_remote_advances() {
        let remote = FixtureRemote::new("pinned-stays");
        let v1_sha = remote.commit_file("main.link", "type Point = { x: Int }", "v1");

        let project = TempProject::new("pinned-stays");
        let (_, lock) = resolve(&format!("git+{}#main", remote.url()), &project.0).expect("debe resolver 'main' la primera vez");
        assert_eq!(lock.resolved, v1_sha);

        remote.commit_file("main.link", "type Point = { x: Int, y: Int }", "v2 -- el remoto avanzó");

        let checkout_dir = resolve_pinned(&remote.url(), &lock.resolved, &project.0).expect("debe resolver el pin sin tocar el remoto para 'rev'");
        let content = fs::read_to_string(checkout_dir.join("main.link")).unwrap();
        assert!(!content.contains("y: Int"), "el pin tiene que quedarse en v1 aunque el remoto ya tenga v2: {content}");
    }

    /// `resolve_pinned` contra un commit que el caché local NO tiene
    /// todavía (ej. un `.linkc/cache` recién clonado en otra máquina) --
    /// tiene que fetchear UNA vez para materializarlo, no fallar
    /// asumiendo que ya está.
    #[test]
    fn resolve_pinned_fetches_once_when_the_pinned_commit_is_not_in_the_local_cache_yet() {
        let remote = FixtureRemote::new("pinned-fetch-once");
        let v1_sha = remote.commit_file("main.link", "type Point = { x: Int }", "v1");
        let v2_sha = remote.commit_file("main.link", "type Point = { x: Int, y: Int }", "v2");

        let project = TempProject::new("pinned-fetch-once");
        // Clona y se queda en v1 -- v2 existe en el remoto pero el caché
        // local todavía no lo vio.
        resolve(&format!("git+{}#{v1_sha}", remote.url()), &project.0).expect("debe clonar y resolver v1");

        let checkout_dir = resolve_pinned(&remote.url(), &v2_sha, &project.0).expect("debe fetchear una vez para traer v2 al caché local");
        let content = fs::read_to_string(checkout_dir.join("main.link")).unwrap();
        assert!(content.contains("y: Int"), "debe haber materializado v2: {content}");
    }

    /// El lock de caché (`CacheLock`) serializa dos resoluciones
    /// CONCURRENTES de la MISMA dependencia -- sin él, dos hilos podrían
    /// pisarse el mismo `git clone`/`checkout` a la vez. No prueba que el
    /// resultado final sea correcto bajo carrera (`resolve` en sí ya lo es,
    /// una vez serializado) -- prueba que el lock REALMENTE excluye, con
    /// un contador compartido que solo puede incrementarse de a uno bajo
    /// el lock si la exclusión funciona.
    #[test]
    fn cache_lock_serializes_concurrent_acquisitions() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let dir = std::env::temp_dir().join(format!("cscript-gitdep-lock-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let cache_dir = dir.join("fake-cache-dir");

        let concurrent_holders = Arc::new(AtomicUsize::new(0));
        let max_concurrent_holders = Arc::new(AtomicUsize::new(0));

        std::thread::scope(|scope| {
            for _ in 0..8 {
                let cache_dir = cache_dir.clone();
                let concurrent_holders = Arc::clone(&concurrent_holders);
                let max_concurrent_holders = Arc::clone(&max_concurrent_holders);
                scope.spawn(move || {
                    let _lock = CacheLock::acquire(&cache_dir).expect("debe poder tomar el lock, eventualmente");
                    let now = concurrent_holders.fetch_add(1, Ordering::SeqCst) + 1;
                    max_concurrent_holders.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(20));
                    concurrent_holders.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });

        assert_eq!(max_concurrent_holders.load(Ordering::SeqCst), 1, "nunca debería haber más de UN hilo sosteniendo el lock a la vez");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Un lock más viejo que `CACHE_LOCK_STALE_AFTER` se trata como
    /// abandonado (el proceso que lo tomó murió sin soltarlo) -- se borra y
    /// se reintenta, en vez de bloquear para siempre.
    #[test]
    fn cache_lock_steals_a_stale_lock_instead_of_blocking_forever() {
        let dir = std::env::temp_dir().join(format!("cscript-gitdep-stale-lock-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let cache_dir = dir.join("fake-cache-dir");
        let lock_path = cache_dir.with_extension("lock");

        fs::write(&lock_path, "99999999").unwrap();
        // Retrocede el mtime del lock más allá del umbral de "abandonado"
        // -- sin esto, el test dependería de esperar CACHE_LOCK_STALE_AFTER
        // de verdad (120s), demasiado lento para un test.
        let stale_time = std::time::SystemTime::now() - CACHE_LOCK_STALE_AFTER - Duration::from_secs(1);
        // `File::open` (solo lectura) no alcanza para `set_modified` en
        // Windows ("Acceso denegado") -- necesita el handle abierto con
        // permiso de escritura, aunque no se escriba ningún byte acá.
        let file = OpenOptions::new().write(true).open(&lock_path).unwrap();
        file.set_modified(stale_time).unwrap();

        let acquired = CacheLock::acquire(&cache_dir);
        assert!(acquired.is_ok(), "un lock viejo abandonado tiene que poder robarse, no bloquear para siempre: {acquired:?}");

        let _ = fs::remove_dir_all(&dir);
    }
}
