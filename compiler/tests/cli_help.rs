use std::process::Command;

fn linkc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_linkc"))
}

/// Todo subcomando despachado en `main` tiene que aparecer en el texto de
/// uso. La lista se desactualizó una vez -- el mensaje de subcomando
/// desconocido nombraba 6 de los 11 que existen -- y este test es lo que
/// impide que vuelva a pasar en silencio.
#[test]
fn help_lists_every_dispatched_subcommand() {
    let out = linkc().arg("--help").output().expect("no se pudo ejecutar linkc");
    assert!(out.status.success(), "--help debería salir con código 0");
    let text = String::from_utf8_lossy(&out.stdout);

    for sub in [
        "build", "test", "serve", "new", "dev", "lsp", "wasm", "fmt", "lint", "doc", "docker", "systemd",
    ] {
        assert!(
            text.contains(&format!("linkc {sub}")),
            "'{sub}' no aparece en `linkc --help`:\n{text}"
        );
    }
}

/// Pedir ayuda no es un error: va a stdout, no a stderr, y sale 0.
#[test]
fn help_goes_to_stdout_with_a_success_code() {
    for flag in ["--help", "-h", "help"] {
        let out = linkc().arg(flag).output().expect("no se pudo ejecutar linkc");
        assert!(out.status.success(), "'{flag}' debería salir con código 0");
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("uso: linkc"),
            "'{flag}' no imprimió el uso en stdout"
        );
        assert!(
            String::from_utf8_lossy(&out.stderr).is_empty(),
            "'{flag}' escribió en stderr"
        );
    }
}

/// Invocar mal la herramienta sí es un error: uso en stderr y código 1.
#[test]
fn no_arguments_prints_usage_to_stderr_and_fails() {
    let out = linkc().output().expect("no se pudo ejecutar linkc");
    assert!(!out.status.success(), "sin argumentos debería fallar");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("uso: linkc"),
        "no imprimió el uso en stderr"
    );
}

/// Un subcomando mal escrito redirige a `--help` en vez de a una lista
/// incrustada en el mensaje, que es lo que se desactualizaba.
#[test]
fn an_unknown_subcommand_points_at_help() {
    let out = linkc().arg("buidl").output().expect("no se pudo ejecutar linkc");
    assert!(!out.status.success());
    let text = String::from_utf8_lossy(&out.stderr);
    assert!(text.contains("linkc --help"), "mensaje inesperado: {text}");
}

/// `linkc --version`/`-v`/`version` (PLAN.md §9.7, GRAMMAR.md §3.83):
/// `env!("CARGO_PKG_VERSION")` acá (en este mismo `Cargo.toml`) tiene que
/// ser LITERALMENTE lo que el binario real imprime -- las dos lecturas
/// vienen del mismo archivo, así que una desincronización sería un bug de
/// verdad, no un test frágil.
#[test]
fn version_flag_prints_the_exact_crate_version_and_succeeds() {
    for flag in ["--version", "-v", "version"] {
        let out = linkc().arg(flag).output().expect("no se pudo ejecutar linkc");
        assert!(out.status.success(), "'{flag}' debería salir con código 0");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(stdout.trim(), format!("linkc {}", env!("CARGO_PKG_VERSION")), "'{flag}': {stdout}");
        assert!(String::from_utf8_lossy(&out.stderr).is_empty(), "'{flag}' escribió en stderr");
    }
}
