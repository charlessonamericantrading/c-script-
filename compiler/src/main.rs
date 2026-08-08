use linkc::ast::Program;
use linkc::{checker, codegen, diagnostics, modules, runtime, scaffold};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
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

/// Reporta los errores de `check_program_with_files`, con snippet+caret
/// cuando es seguro hacerlo. Antes de esta ronda (GRAMMAR.md §3.21, "Not
/// done yet"), `Span` no tenía identidad de archivo y el único gate posible
/// era `touched.len() == 1` -- cualquier programa con imports caía al
/// `Display` plano para TODOS sus errores, aunque el 100% de ellos
/// estuvieran en el archivo de entrada. Ahora `CheckError.file` (estampado
/// por `check_program_full` con `item_files`, ver checker.rs y
/// modules::load_program_with_overlay) le dice a cada error de qué archivo
/// real vino, así que el snippet se renderiza por error individual, no por
/// el programa entero -- un error sin `file` (no debería pasar cuando el
/// caller pasa `item_files` poblado, pero se maneja igual por si acaso)
/// cae al `Display` plano de siempre, nunca a una posición adivinada.
///
/// `single_file` cachea las lecturas de disco (varios errores del mismo
/// archivo no lo releen cada vez), poblado de forma perezosa a medida que
/// aparecen archivos nuevos en los errores.
///
/// Orden de reporte: `build_symbols` hace 4 scans secuenciales (types+enums,
/// fns, consts, db) que respetan el orden del archivo DENTRO de cada scan,
/// pero no ENTRE scans -- se ordena acá por posición antes de imprimir, con
/// los errores sin span (algunos de `build_symbols` no son "sobre" un nodo
/// puntual) consistentemente al final.
fn report_check_errors(mut errors: Vec<checker::CheckError>) {
    errors.sort_by_key(|e| match e.span {
        Some(s) => (0, s.line, s.col),
        None => (1, 0, 0),
    });
    let mut source_cache: std::collections::HashMap<PathBuf, Option<String>> = std::collections::HashMap::new();
    for e in &errors {
        let rendered = match (&e.file, e.span) {
            (Some(path), Some(span)) => {
                let source = source_cache.entry(path.clone()).or_insert_with(|| fs::read_to_string(path).ok());
                source.as_ref().map(|src| diagnostics::render_diagnostic(src, &display_path(path), span, &e.message))
            }
            _ => None,
        };
        match rendered {
            Some(snippet) => eprintln!("{snippet}"),
            None => eprintln!("{e}"),
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("build") => cmd_build(&args[2..]),
        Some("test") => cmd_test(&args[2..]),
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
            eprintln!("     linkc test <archivo.link> <archivo.snap> [--update]  (compara el contrato generado contra un snapshot commiteado; falla si cambió sin querer)");
            eprintln!("     linkc wasm <archivo.link> <out.wasm>   (emite binario WebAssembly nativo -- v0: solo Int/Bool y una expresión final)");
            eprintln!("     linkc dev <archivo.link> <outdir> [puerto]  (+ observa y reconstruye solo; con <puerto>, hot reload real de 'linkc serve')");
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
    let (program, _touched, item_files) = modules::load_program(Path::new(path)).map_err(|e| {
        report_load_error(&e);
        ExitCode::FAILURE
    })?;

    checker::Checker::check_program_with_files(&program, &item_files).map_err(|errors| {
        report_check_errors(errors);
        ExitCode::FAILURE
    })?;

    Ok(program)
}

fn cmd_check(path: &str) -> ExitCode {
    // `linkc <algo>` cae acá para cualquier <algo> que no sea un subcomando
    // conocido -- si además no parece un archivo real, es casi seguro un
    // subcomando mal escrito, no un archivo que el usuario quiere tipar.
    if !path.ends_with(".link") && !Path::new(path).exists() {
        eprintln!("'{path}' no es un subcomando conocido (build, serve, new, dev, lsp, wasm) ni un archivo .link existente");
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
    // `load_program_full`, no `load_program`: además del trío de
    // siempre, necesita `git_dependencies` (GRAMMAR.md §2.1, package
    // manager real) para grabar en `link.lock` más abajo -- `linkc
    // check`/`serve`/`wasm` (que sí usan `load_program` vía
    // `load_and_check`) no escriben ningún lockfile, así que no
    // necesitan este cuarto valor.
    let (program, touched, item_files, git_dependencies) = match modules::load_program_full(Path::new(path), &HashMap::new()) {
        Ok(quad) => quad,
        Err(e) => {
            report_load_error(&e);
            return BuildResult { ok: false, touched: vec![PathBuf::from(path)] };
        }
    };
    if let Err(errors) = checker::Checker::check_program_with_files(&program, &item_files) {
        report_check_errors(errors);
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


    let mut lock = linkc::lockfile::generate_lockfile(&touched, root);
    lock.git_dependencies = git_dependencies;
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

/// `linkc test` -- PLAN.md §5 ("tests de contrato, que el .d.ts generado no
/// rompa sin querer"). El snapshot es UN archivo de texto (no un directorio
/// dentro de `outdir`, que casi siempre está en `.gitignore` -- ver
/// `/gen/` -- y por lo tanto no sobreviviría entre commits, que es
/// justamente lo que un snapshot necesita para servir de algo) con los tres
/// outputs del emisor concatenados. Sin snapshot previo: lo crea y avisa
/// que hay que commitearlo -- esa primera corrida establece la base, no
/// hay "antes" con que compararla. Con snapshot previo que difiere: falla
/// (`ExitCode::FAILURE`) y muestra el diff en vez de sobreescribir en
/// silencio -- que el contrato haya cambiado de forma real (una ronda
/// legítima) o accidental (el bug que esta feature existe para atrapar) lo
/// decide una persona mirando el diff, nunca el comando solo. `--update`
/// es ese "sí, es a propósito" explícito.
fn cmd_test(args: &[String]) -> ExitCode {
    let (Some(path), Some(snap_path)) = (args.first(), args.get(1)) else {
        eprintln!("uso: linkc test <archivo.link> <archivo.snap> [--update]");
        return ExitCode::FAILURE;
    };
    let update = args.iter().any(|a| a == "--update");

    let program = match load_and_check(path) {
        Ok(p) => p,
        Err(code) => return code,
    };

    let contract = match codegen::ts_emit::emit_contract(&program) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error al emitir contract.d.ts: {e}");
            return ExitCode::FAILURE;
        }
    };
    let client = match codegen::ts_emit::emit_client(&program) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error al emitir client.ts: {e}");
            return ExitCode::FAILURE;
        }
    };
    let validators = match codegen::validators_emit::emit_validators(&program) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error al emitir validators.ts: {e}");
            return ExitCode::FAILURE;
        }
    };

    let current = format!(
        "=== contract.d.ts ===\n{contract}\n=== client.ts ===\n{client}\n=== validators.ts ===\n{validators}"
    );

    let snap_file = Path::new(snap_path);
    if !snap_file.exists() {
        if let Some(parent) = snap_file.parent() {
            if !parent.as_os_str().is_empty() {
                if let Err(e) = fs::create_dir_all(parent) {
                    eprintln!("no se pudo crear {}: {e}", parent.display());
                    return ExitCode::FAILURE;
                }
            }
        }
        if let Err(e) = fs::write(snap_file, &current) {
            eprintln!("no se pudo escribir {snap_path}: {e}");
            return ExitCode::FAILURE;
        }
        println!("snapshot creado en {snap_path} -- revisalo y commitealo a git (es la base contra la que se compara desde ahora)");
        return ExitCode::SUCCESS;
    }

    let previous = match fs::read_to_string(snap_file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("no se pudo leer {snap_path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    if previous == current {
        println!("OK: el contrato de {path} coincide con el snapshot ({snap_path})");
        return ExitCode::SUCCESS;
    }

    if update {
        if let Err(e) = fs::write(snap_file, &current) {
            eprintln!("no se pudo escribir {snap_path}: {e}");
            return ExitCode::FAILURE;
        }
        println!("snapshot actualizado en {snap_path}");
        return ExitCode::SUCCESS;
    }

    eprintln!("EL CONTRATO DE {path} CAMBIÓ respecto al snapshot ({snap_path}):");
    eprintln!("{}", diff_lines(&previous, &current));
    eprintln!("si el cambio es intencional: linkc test {path} {snap_path} --update");
    ExitCode::FAILURE
}

/// Diff línea a línea vía LCS (programación dinámica) -- correcto de
/// verdad (a diferencia de una comparación posición-a-posición, que
/// "arrastra" como distinta cada línea siguiente a una sola inserción),
/// sin sumar una dependencia nueva -- mismo espíritu que el SHA-256
/// hand-rolled de `lockfile.rs`: un algoritmo chico, estable y
/// autocontenido no necesita un crate aparte. `n*m` es memoria de la
/// tabla LCS: trivial para un contrato generado (cientos de líneas), así
/// que la guarda de tamaño de abajo es para no colgarse si algún día esto
/// se aplica a un archivo gigante, no una ruta esperada hoy.
fn diff_lines(old: &str, new: &str) -> String {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let n = old_lines.len();
    let m = new_lines.len();

    if n.saturating_mul(m) > 4_000_000 {
        return format!(
            "(archivos demasiado grandes para diff en línea -- {n} vs {m} líneas; comparalos con tu herramienta de diff preferida)"
        );
    }

    let mut lcs = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if old_lines[i] == new_lines[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }

    let mut out = String::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if old_lines[i] == new_lines[j] {
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            out.push_str(&format!("- {}\n", old_lines[i]));
            i += 1;
        } else {
            out.push_str(&format!("+ {}\n", new_lines[j]));
            j += 1;
        }
    }
    while i < n {
        out.push_str(&format!("- {}\n", old_lines[i]));
        i += 1;
    }
    while j < m {
        out.push_str(&format!("+ {}\n", new_lines[j]));
        j += 1;
    }
    out
}

/// Snapshot de mtimes de una lista de archivos, en el mismo orden -- `None`
/// si un archivo dejó de existir (se borró, o el path quedó viejo tras un
/// rename). Comparar dos snapshots detecta tanto "un archivo cambió" como
/// "la lista de archivos a observar cambió de tamaño" (ej. una import
/// nueva) en una sola comparación de `Vec`.
fn snapshot_mtimes(paths: &[PathBuf]) -> Vec<Option<SystemTime>> {
    paths.iter().map(|p| fs::metadata(p).and_then(|m| m.modified()).ok()).collect()
}

/// Lanza `linkc serve <path> <port>` como PROCESO HIJO real (reinvoca el
/// propio binario vía `env::current_exe()`) -- reusa `cmd_serve`/
/// `runtime::server::serve` TAL CUAL, sin ningún cambio, en vez de
/// intentar un hot-swap del `Program` DENTRO del proceso servidor ya
/// corriendo. Un restart de proceso es más simple de razonar y más
/// robusto que un swap en memoria (que necesitaría tocar el modelo de
/// threading que `runtime/server.rs` ya documenta con cuidado --
/// `Value::Closure`/`Rc` no cruzan un borde de hilo, GRAMMAR.md §3.13) --
/// el costo es perder las conexiones `stream` abiertas en cada reload, un
/// trade-off razonable para modo desarrollo, no para producción.
fn spawn_serve_child(exe: &Path, path: &str, port: u16) -> Option<std::process::Child> {
    match Command::new(exe).arg("serve").arg(path).arg(port.to_string()).spawn() {
        Ok(child) => {
            println!("linkc dev: sirviendo en http://localhost:{port} (PID {})", child.id());
            Some(child)
        }
        Err(e) => {
            eprintln!("linkc dev: no se pudo iniciar 'linkc serve': {e}");
            None
        }
    }
}

/// Termina el hijo por su PID exacto (`Child::kill`, nunca un kill por
/// nombre de imagen -- si el usuario tiene OTRO `linkc serve` corriendo
/// aparte, no debe verse afectado) y espera a que salga antes de devolver
/// el control, para no arrancar el próximo hijo mientras el puerto
/// todavía está ocupado por el anterior.
fn kill_serve_child(mut child: std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// `linkc dev <archivo.link> <outdir> [puerto]` -- el `[puerto]` es
/// hot reload real (GRAMMAR.md §2.1, auditoría post-push), opcional y
/// retrocompatible: sin él, comportamiento idéntico a antes de esta
/// ronda (observa y reconstruye, sin servidor). Con él, cada rebuild
/// EXITOSO reinicia un `linkc serve` hijo con el programa actualizado.
///
/// Un rebuild FALLIDO (error de sintaxis/tipos mientras se edita) NUNCA
/// tira abajo el servidor -- el hijo de la última versión válida sigue
/// sirviendo tal cual hasta que el próximo rebuild exitoso lo reemplace,
/// mismo criterio que un dev server de frontend (Vite/webpack) que sigue
/// sirviendo el último build bueno en vez de caerse por un typo a medio
/// escribir.
fn cmd_dev(args: &[String]) -> ExitCode {
    let (Some(path), Some(outdir)) = (args.first(), args.get(1)) else {
        eprintln!("uso: linkc dev <archivo.link> <outdir> [puerto]");
        return ExitCode::FAILURE;
    };
    let serve_port: Option<u16> = match args.get(2) {
        None => None,
        Some(s) => match s.parse() {
            Ok(p) => Some(p),
            Err(_) => {
                eprintln!("puerto inválido: '{s}'");
                return ExitCode::FAILURE;
            }
        },
    };
    // Se resuelve UNA vez, no en cada restart -- `current_exe()` es una
    // syscall real (lee el link simbólico /proc/self/exe en Linux, el
    // equivalente en cada plataforma), sin necesidad de repetirla por
    // cada reload.
    let exe = serve_port.and_then(|_| env::current_exe().ok());
    if serve_port.is_some() && exe.is_none() {
        eprintln!("linkc dev: no se pudo resolver la ruta del propio binario -- hot reload del servidor deshabilitado, solo se observará y reconstruirá");
    }

    // Sin manejo de señales explícito para limpiar el hijo al salir --
    // `Command::spawn()` sin `CREATE_NEW_PROCESS_GROUP` deja al hijo en el
    // mismo grupo de proceso/consola que este padre en ambas plataformas,
    // así que un Ctrl+C real en una terminal interactiva ya le llega
    // TAMBIÉN al hijo (ese es el camino verificado manualmente). Un kill
    // programático dirigido SOLO al PID de este proceso padre (no un
    // Ctrl+C real) es el caso que sí puede dejar al hijo huérfano
    // sirviendo el puerto -- límite de v0 conocido, no manejado (mismo
    // tipo de limitación que `gitdep::resolve` ya documenta para el
    // locking entre procesos).
    println!("linkc dev: observando '{path}' y sus imports (Ctrl+C para detener)");
    let mut result = build_once(path, outdir);
    let mut mtimes = snapshot_mtimes(&result.touched);
    let mut server_child = match (&exe, serve_port) {
        (Some(exe), Some(port)) if result.ok => spawn_serve_child(exe, path, port),
        _ => None,
    };

    loop {
        std::thread::sleep(Duration::from_millis(400));
        let current = snapshot_mtimes(&result.touched);
        if current != mtimes {
            println!("cambio detectado, reconstruyendo...");
            result = build_once(path, outdir);
            mtimes = snapshot_mtimes(&result.touched);
            if !result.ok {
                if server_child.is_some() {
                    eprintln!("linkc dev: el rebuild falló -- el servidor sigue sirviendo la última versión válida");
                }
                continue;
            }
            if let (Some(exe), Some(port)) = (&exe, serve_port) {
                if let Some(child) = server_child.take() {
                    kill_serve_child(child);
                }
                server_child = spawn_serve_child(exe, path, port);
            }
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
