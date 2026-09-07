// `linkc self-install <versión> [--dir <ruta>]` (GRAMMAR.md §3.266): baja el
// binario de un release real de GitHub para este SO/arch, verifica su
// SHA256 contra SHA256SUMS.txt del MISMO release, y lo deja ejecutable en
// `--dir` -- sin tocar ningún binario ya instalado en otra ruta. El
// incidente real que lo motiva: el binario COMPARTIDO de una VPS con más
// proyectos quedó 15 versiones atrás en silencio hasta romper en
// producción (PLAN.md §9.23 ítem 1).
//
// A diferencia de `cli_smtp.rs`/`cli_http.rs` (que evitan la red real con un
// servidor de mentira local), acá no hay forma de probar el camino real sin
// tocar GitHub -- el propósito ENTERO del comando es bajar de ahí. Se prueba
// contra un release real y ya publicado (v1.211.0, el mismo que agregó esta
// característica) en vez de mockear la descarga.

use std::path::PathBuf;
use std::process::Command;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "linkc-self-install-test-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn linkc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_linkc"))
}

#[test]
fn self_install_without_a_version_argument_is_a_clean_usage_error() {
    let out = linkc().arg("self-install").output().expect("ejecutar linkc self-install");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("uso: linkc self-install"), "stderr: {stderr}");
    assert!(!stderr.contains("panicked"), "stderr: {stderr}");
}

#[test]
fn self_install_downloads_verifies_and_installs_a_real_release() {
    let dir = TempDir::new("real-release");
    let out = linkc().arg("self-install").arg("1.211.0").arg("--dir").arg(&dir.0).output().expect("ejecutar linkc self-install");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stdout: {stdout}\nstderr: {stderr}");
    assert!(stdout.contains("checksum SHA256 verificado"), "stdout: {stdout}");
    assert!(stdout.contains("linkc 1.211.0"), "el binario instalado tiene que reportar exactamente la versión pedida: {stdout}");

    let bin_name = if cfg!(windows) { "linkc.exe" } else { "linkc" };
    let installed = dir.0.join(bin_name);
    assert!(installed.is_file(), "el binario tiene que quedar en --dir: {}", installed.display());

    // El binario instalado responde de verdad a --version por sí solo, no
    // solo durante la instalación -- confirma que quedó completo y
    // ejecutable, no un archivo a medio copiar.
    let version_out = Command::new(&installed).arg("--version").output().expect("ejecutar el binario recién instalado");
    assert!(version_out.status.success());
    assert!(String::from_utf8_lossy(&version_out.stdout).contains("linkc 1.211.0"));
}

#[test]
fn self_install_accepts_a_version_with_a_leading_v() {
    let dir = TempDir::new("leading-v");
    let out = linkc().arg("self-install").arg("v1.211.0").arg("--dir").arg(&dir.0).output().expect("ejecutar linkc self-install");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "'v1.211.0' con 'v' inicial tiene que aceptarse igual que '1.211.0': stdout: {stdout}");
}

#[test]
fn self_install_against_a_nonexistent_version_fails_cleanly_not_with_a_panic() {
    let dir = TempDir::new("nonexistent");
    let out = linkc().arg("self-install").arg("999.999.999").arg("--dir").arg(&dir.0).output().expect("ejecutar linkc self-install");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("panicked"), "una versión inexistente es un error normal, no un panic: {stderr}");
    assert!(!dir.0.join(if cfg!(windows) { "linkc.exe" } else { "linkc" }).exists(), "no debería haber instalado nada");
}
