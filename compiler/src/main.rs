use linkc::ast::Program;
use linkc::{checker, codegen, diagnostics, modules, runtime, scaffold};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, SystemTime};

use modules::display_path;

/// `LoadError::Other` (IO, ciclos, etc.) se muestra como siempre. Para
/// `LoadError::Syntax`, usa `diagnostics::render_diagnostic` -- snippet +
/// caret en la línea/columna real -- en vez del `Display` plano de una
/// línea. Si el archivo no se puede releer por algún motivo (borrado entre
/// el error y este punto), cae de vuelta al `Display` plano en vez de
/// fallar de una forma más confusa.
fn report_load_error(e: &modules::LoadError) {
    let modules::LoadError::Syntax { path, errors } = e else {
        eprintln!("{e}");
        return;
    };
    let Ok(source) = fs::read_to_string(path) else {
        eprintln!("{e}");
        return;
    };
    let file_label = display_path(path);
    for (span, message) in errors {
        eprintln!("{}", diagnostics::render_diagnostic(&source, &file_label, *span, message));
    }
}

/// Reporta los errores de `check_program` (LSP prerrequisito 3/3), con
/// snippet+caret cuando es seguro hacerlo. `Span` no tiene identidad de
/// archivo (token.rs), y `modules::load_program` funde TODOS los archivos
/// importados transitivamente en un solo `Program` sin etiquetar de qué
/// archivo vino cada ítem -- así que releer ingenuamente el archivo de
/// ENTRADA para renderizar un error que en realidad vino de un archivo
/// IMPORTADO sería activamente engañoso (un snippet plausible, pero de la
/// línea equivocada). Gate de seguridad: solo se renderiza con snippet
/// cuando `touched.len() == 1` -- si el programa vino de más de un archivo,
/// cae al `Display` plano de siempre para TODOS los errores, no solo los
/// dudosos. Proveniencia real por archivo (para levantar este límite)
/// queda para una ronda futura, no bloqueante acá.
///
/// Orden de reporte: `build_symbols` hace 4 scans secuenciales (types+enums,
/// fns, consts, db) que respetan el orden del archivo DENTRO de cada scan,
/// pero no ENTRE scans -- se ordena acá por posición antes de imprimir, con
/// los errores sin span (algunos de `build_symbols` no son "sobre" un nodo
/// puntual) consistentemente al final.
fn report_check_errors(mut errors: Vec<checker::CheckError>, touched: &[PathBuf]) {
    errors.sort_by_key(|e| match e.span {
        Some(s) => (0, s.line, s.col),
        None => (1, 0, 0),
    });
    let single_file = match touched {
        [only] => fs::read_to_string(only).ok().map(|src| (display_path(only), src)),
        _ => None,
    };
    for e in &errors {
        match (&single_file, e.span) {
            (Some((label, source)), Some(span)) => {
                eprintln!("{}", diagnostics::render_diagnostic(source, label, span, &e.message));
            }
            _ => eprintln!("{e}"),
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("build") => cmd_build(&args[2..]),
        Some("serve") => cmd_serve(&args[2..]),
        Some("new") => cmd_new(&args[2..]),
        Some("dev") => cmd_dev(&args[2..]),
        Some("lsp") => cmd_lsp(),
        Some("wasm") => cmd_wasm(&args[2..]),
        Some(path) => cmd_check(path), // `linkc <archivo.link>` -- solo lex+parse+check
        None => {
            eprintln!("uso: linkc <subcomando> [opciones]");
            eprintln!("subcomandos conocidos:");
            eprintln!("     linkc new <nombre>                     (scaffoldea un proyecto nuevo)");
            eprintln!("     linkc build <archivo.link> <outdir>    (genera contract.d.ts, client.ts, validators.ts, link.lock, y main.wasm si el programa entra en el subconjunto soportado -- ver 'linkc wasm')");
            eprintln!("     linkc wasm <archivo.link> <out.wasm>   (emite binario WebAssembly nativo -- v0: solo Int/Bool y una expresión final)");
            eprintln!("     linkc dev <archivo.link> <outdir>      (+ observa y reconstruye solo)");
            eprintln!("     linkc serve <archivo.link> <puerto>    (+ sirve los rpc por HTTP)");
            eprintln!("     linkc lsp                              (inicia el servidor Language Server Protocol)");
            ExitCode::FAILURE
        }
    }
}

fn cmd_wasm(args: &[String]) -> ExitCode {
    let (Some(path), Some(out_path)) = (args.first(), args.get(1)) else {
        eprintln!("uso: linkc wasm <archivo.link> <outfile.wasm>");
        return ExitCode::FAILURE;
    };

    let program = match load_and_check(path) {
        Ok(p) => p,
        Err(code) => return code,
    };

    let bytes = match codegen::wasm_emit::emit_wasm(&program) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error al emitir bytecode WASM: {e}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(e) = fs::write(out_path, bytes) {
        eprintln!("no se pudo escribir {out_path}: {e}");
        return ExitCode::FAILURE;
    }

    println!("OK: binario WebAssembly emitido en {out_path}");
    ExitCode::SUCCESS
}


fn cmd_lsp() -> ExitCode {
    let mut server = linkc::lsp::LspServer::new();
    if let Err(e) = server.run_stdio() {
        eprintln!("Error en el servidor LSP: {e}");
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}


fn cmd_new(args: &[String]) -> ExitCode {
    let Some(name) = args.first() else {
        eprintln!("uso: linkc new <nombre>");
        return ExitCode::FAILURE;
    };
    scaffold::new_project(name)
}

fn load_and_check(path: &str) -> Result<Program, ExitCode> {
    let (program, touched) = modules::load_program(Path::new(path)).map_err(|e| {
        report_load_error(&e);
        ExitCode::FAILURE
    })?;

    checker::Checker::check_program(&program).map_err(|errors| {
        report_check_errors(errors, &touched);
        ExitCode::FAILURE
    })?;

    Ok(program)
}

fn cmd_check(path: &str) -> ExitCode {
    // `linkc <algo>` cae acá para cualquier <algo> que no sea un subcomando
    // conocido -- si además no parece un archivo real, es casi seguro un
    // subcomando mal escrito, no un archivo que el usuario quiere tipar.
    if !path.ends_with(".link") && !Path::new(path).exists() {
        eprintln!("'{path}' no es un subcomando conocido (build, serve, new, dev) ni un archivo .link existente");
        return ExitCode::FAILURE;
    }
    match load_and_check(path) {
        Ok(program) => {
            println!("OK: {path} tipa correctamente ({} ítems)", program.items.len());
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}

/// Resultado de un build: si tuvo éxito, y qué archivos físicos se
/// tocaron. `link dev` necesita `touched` INCLUSO cuando `ok` es falso --
/// si el error es de tipos (no de carga), ya sabemos qué archivos observar
/// para el próximo intento; si falló la carga misma, al menos queda el
/// archivo de entrada.
struct BuildResult {
    ok: bool,
    touched: Vec<PathBuf>,
}

fn build_once(path: &str, outdir: &str) -> BuildResult {
    let (program, touched) = match modules::load_program(Path::new(path)) {
        Ok(pair) => pair,
        Err(e) => {
            report_load_error(&e);
            return BuildResult { ok: false, touched: vec![PathBuf::from(path)] };
        }
    };
    if let Err(errors) = checker::Checker::check_program(&program) {
        report_check_errors(errors, &touched);
        return BuildResult { ok: false, touched };
    }

    let contract = match codegen::ts_emit::emit_contract(&program) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error al emitir contract.d.ts: {e}");
            return BuildResult { ok: false, touched };
        }
    };
    let client = match codegen::ts_emit::emit_client(&program) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error al emitir client.ts: {e}");
            return BuildResult { ok: false, touched };
        }
    };
    let validators = match codegen::validators_emit::emit_validators(&program) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error al emitir validators.ts: {e}");
            return BuildResult { ok: false, touched };
        }
    };

    if let Err(e) = fs::create_dir_all(outdir) {
        eprintln!("no se pudo crear {outdir}: {e}");
        return BuildResult { ok: false, touched };
    }
    let contract_path = format!("{outdir}/contract.d.ts");
    let client_path = format!("{outdir}/client.ts");
    let validators_path = format!("{outdir}/validators.ts");
    if let Err(e) = fs::write(&contract_path, contract) {
        eprintln!("no se pudo escribir {contract_path}: {e}");
        return BuildResult { ok: false, touched };
    }
    if let Err(e) = fs::write(&client_path, client) {
        eprintln!("no se pudo escribir {client_path}: {e}");
        return BuildResult { ok: false, touched };
    }
    if let Err(e) = fs::write(&validators_path, validators) {
        eprintln!("no se pudo escribir {validators_path}: {e}");
        return BuildResult { ok: false, touched };
    }

    let wasm_path = format!("{outdir}/main.wasm");
    match codegen::wasm_emit::emit_wasm(&program) {
        Ok(wasm_bytes) => {
            if let Err(e) = fs::write(&wasm_path, wasm_bytes) {
                eprintln!("advertencia: no se pudo escribir {wasm_path}: {e}");
            }
            println!("OK: generado {contract_path}, {client_path}, {validators_path} y {wasm_path}");
        }
        Err(e) => {
            println!("OK: generado {contract_path}, {client_path}, {validators_path}");
            eprintln!(
                "advertencia: no se generó {wasm_path} -- el codegen wasm nativo (v0, solo Int/Bool y expresión final) no soporta este programa: {e}"
            );
        }
    }

    let root = Path::new(path).parent().unwrap_or_else(|| Path::new("."));
    let lock_path = root.join("link.lock");

    if lock_path.exists() {
        if let Ok(existing_lock) = linkc::lockfile::read_lockfile(&lock_path) {
            if let Err(mismatches) = linkc::lockfile::verify_lockfile(&existing_lock, root) {
                for m in mismatches {
                    eprintln!("ADVERTENCIA [link.lock]: {m}");
                }
            }
        }
    }


    let lock = linkc::lockfile::generate_lockfile(&touched, root);
    if let Err(e) = linkc::lockfile::write_lockfile(&lock, &lock_path) {
        eprintln!("advertencia: no se pudo escribir link.lock: {e}");
    }

    BuildResult { ok: true, touched }
}



fn cmd_build(args: &[String]) -> ExitCode {
    let (Some(path), Some(outdir)) = (args.first(), args.get(1)) else {
        eprintln!("uso: linkc build <archivo.link> <outdir>");
        return ExitCode::FAILURE;
    };
    if build_once(path, outdir).ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Snapshot de mtimes de una lista de archivos, en el mismo orden -- `None`
/// si un archivo dejó de existir (se borró, o el path quedó viejo tras un
/// rename). Comparar dos snapshots detecta tanto "un archivo cambió" como
/// "la lista de archivos a observar cambió de tamaño" (ej. una import
/// nueva) en una sola comparación de `Vec`.
fn snapshot_mtimes(paths: &[PathBuf]) -> Vec<Option<SystemTime>> {
    paths.iter().map(|p| fs::metadata(p).and_then(|m| m.modified()).ok()).collect()
}

fn cmd_dev(args: &[String]) -> ExitCode {
    let (Some(path), Some(outdir)) = (args.first(), args.get(1)) else {
        eprintln!("uso: linkc dev <archivo.link> <outdir>");
        return ExitCode::FAILURE;
    };

    println!("linkc dev: observando '{path}' y sus imports (Ctrl+C para detener)");
    let mut result = build_once(path, outdir);
    let mut mtimes = snapshot_mtimes(&result.touched);

    loop {
        std::thread::sleep(Duration::from_millis(400));
        let current = snapshot_mtimes(&result.touched);
        if current != mtimes {
            println!("cambio detectado, reconstruyendo...");
            result = build_once(path, outdir);
            mtimes = snapshot_mtimes(&result.touched);
        }
    }
}

fn cmd_serve(args: &[String]) -> ExitCode {
    let (Some(path), Some(port_str)) = (args.first(), args.get(1)) else {
        eprintln!("uso: linkc serve <archivo.link> <puerto>");
        return ExitCode::FAILURE;
    };
    let Ok(port) = port_str.parse::<u16>() else {
        eprintln!("puerto inválido: '{port_str}'");
        return ExitCode::FAILURE;
    };

    let program = match load_and_check(path) {
        Ok(p) => p,
        Err(code) => return code,
    };

    // `myapp.link` -> `myapp.db`, al lado del archivo fuente (GRAMMAR.md
    // §3.17) -- mismo patrón que `build_once` ya usa para nombrar
    // contract.d.ts/client.ts/validators.ts, sin ninguna flag de CLI nueva.
    let db_path = Path::new(path).with_extension("db");
    runtime::server::serve(program, port, db_path);
    ExitCode::SUCCESS
}
