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
        Some("serve-all") => cmd_serve_all(&args[2..]),
        Some("migrate") => cmd_migrate(&args[2..]),
        Some("db") => cmd_db(&args[2..]),
        Some("doctor") => cmd_doctor(&args[2..]),
        Some("new") => cmd_new(&args[2..]),
        Some("dev") => cmd_dev(&args[2..]),
        Some("lsp") => cmd_lsp(),
        Some("wasm") => cmd_wasm(&args[2..]),
        Some("fmt") => cmd_fmt(&args[2..]),
        Some("lint") => cmd_lint(&args[2..]),
        Some("doc") => cmd_doc(&args[2..]),
        Some("docker") => cmd_docker(&args[2..]),
        Some("systemd") => cmd_systemd(&args[2..]),
        Some("pm2-config") => cmd_pm2_config(&args[2..]),
        Some("introspect") => cmd_introspect(&args[2..]),
        // `--help` es una peticion valida, no un error: va a stdout y sale 0.
        // Sin este brazo caia en `cmd_check("--help")`, que respondia con un
        // mensaje sobre archivos .link inexistentes.
        Some("--help") | Some("-h") | Some("help") => {
            print_usage(false);
            ExitCode::SUCCESS
        }
        // `linkc::VERSION` (GRAMMAR.md §3.83) es `env!("CARGO_PKG_VERSION")`
        // -- la MISMA constante que estampa el header de cada archivo que
        // `linkc build` genera, así que las dos nunca pueden desincronizarse
        // entre sí.
        Some("--version") | Some("-v") | Some("version") => {
            println!("linkc {}", linkc::VERSION);
            ExitCode::SUCCESS
        }
        Some(path) => cmd_check(path), // `linkc <archivo.link>` -- solo lex+parse+check
        None => {
            print_usage(true);
            ExitCode::FAILURE
        }
    }
}

/// Texto de uso. Se imprime en stdout con codigo 0 cuando lo pide el usuario
/// (`--help`) y en stderr con codigo 1 cuando `linkc` se invoca mal.
fn print_usage(to_stderr: bool) {
    let out = |line: &str| {
        if to_stderr {
            eprintln!("{line}");
        } else {
            println!("{line}");
        }
    };
    out(&format!("uso: linkc <subcomando> [opciones]"));
    out(&format!("subcomandos conocidos:"));
    out(&format!("     linkc new <nombre>                     (scaffoldea un proyecto nuevo)"));
    out(&format!("     linkc build <archivo.link> <outdir> [--diff <anterior>] [--update-deps]    (genera contratos TS, cliente, hooks, schemas Zod, OpenAPI, llms.txt y llms-full.txt; --diff compara el contract.d.ts nuevo contra uno guardado antes; --update-deps ignora el pin de link.lock y re-resuelve cada dependencia git contra su remoto real)"));
    out(&format!("     linkc test <archivo.link> [--filter <nombre>] [--db <url-postgres>]  (ejecuta pruebas de comportamiento integradas; --filter acota a las que CONTIENEN ese substring en el nombre; --db/LINK_TEST_DB corre contra PostgreSQL real en vez de SQLite :memory:, sin aislamiento entre tests -- solo contra una base de test dedicada, nunca producción)"));
    out(&format!("     linkc wasm <archivo.link> <out.wasm>   (compila a WebAssembly nativo)"));
    out(&format!("     linkc fmt <archivo.link> [--check]     (formatea el código fuente canónicamente)"));
    out(&format!("     linkc lint <archivo.link> [--fix]      (analiza calidad de código y detecta variables sin uso)"));
    out(&format!("     linkc doc <archivo.link> [outdir]      (genera documentación HTML estática interactiva)"));
    out(&format!("     linkc docker <archivo.link> [outdir]   (genera Dockerfile y docker-compose.yml de producción)"));
    out(&format!("     linkc systemd <archivo.link> <puerto> [outdir]   (genera una unidad systemd lista para /etc/systemd/system/)"));
    out(&format!("     linkc pm2-config <archivo.link> <puerto> [-o <archivo>]   (genera un ecosystem.json de PM2, default ./ecosystem.json)"));
    out(&format!("     linkc introspect <db-url> [> main.link] (genera un .link de partida leyendo el schema de una base PostgreSQL ya existente -- punto de partida para revisar a mano, no listo para producción sin mirarlo)"));
    out(&format!("     linkc migrate <archivo.link> --db <url-postgres> --dry-run (muestra el DDL exacto que 'linkc serve' ejecutaría al conectar a esa base, sin aplicar nada -- solo PostgreSQL, SQLite ya reporta el diff exacto al conectar de verdad)"));
    out(&format!("     linkc doctor <archivo.link> [--db <url|archivo>] [--target-url <url>] (diagnóstico de entorno antes de un despliegue: versión, que el archivo y sus imports resuelvan/tipen, permiso de escritura en su directorio, y conectividad de solo lectura a la base configurada -- --db/LINK_DATABASE_URL, mismo criterio que 'linkc serve'; con --target-url/LINK_DOCTOR_TARGET_URL, además compara la versión local contra la de un 'linkc serve' real corriendo ahí, vía /health)"));
    out(&format!("     linkc db inspect <archivo.link> [--db <url|archivo>] (lista cada colección declarada con su estado físico real -- existe o no, cuántas filas -- sin ejecutar ningún DDL; --db/LINK_DATABASE_URL, mismo criterio que 'linkc serve'/'linkc doctor')"));
    out(&format!("     linkc db export <archivo.link> <archivo.json> [--db <url|archivo>] (vuelca cada colección declarada a un archivo JSON, byte-idéntico al wire real -- sin ejecutar ningún DDL)"));
    out(&format!("     linkc db import <archivo.link> <archivo.json> [--db <url|archivo>] (escribe las filas de un archivo de 'db export' contra un target, preservando el id original de cada fila -- un target vacío ES el caso 'seed')"));
    out(&format!("     linkc db shell <archivo.link> [--db <url|archivo>] (REPL de solo lectura sobre stdin/stdout, una consulta SQL por línea -- SQLite abre de solo lectura, Postgres corre con default_transaction_read_only)"));
    out(&format!("     linkc dev <archivo.link> <outdir>      (observa y reconstruye automáticamente)"));
    out(&format!("     linkc serve <archivo.link> <puerto> [--db <url>] [--host <dirección>] [--cors-origin <origen>] [--session-ttl <duración>] [--argon2-memory-kib <N>] [--argon2-iterations <N>] [--encryption-key <clave-base64>] [--jwt-secret <secreto>] [--jwt-role-claim <nombre>] [--jwt-user-id-claim <nombre>] [--max-body-bytes <N>] [--http-timeout <duración>] [--trust-proxy] [--adopt-existing] [--restart-backoff <duración>]  (servidor HTTP; SQLite embebido, o PostgreSQL con --db/LINK_DATABASE_URL; escucha en todas las interfaces (0.0.0.0) por default, o solo en una dirección puntual vía --host/LINK_HOST, ej. '127.0.0.1'; CORS abierto por default, o allowlist con --cors-origin/LINK_CORS_ORIGINS; sesiones sin expiración por default, o con TTL vía --session-ttl/LINK_SESSION_TTL, ej. '7d'; costo de crypto.hashPassword al default de Argon2id, o configurable vía --argon2-memory-kib/LINK_ARGON2_MEMORY_KIB y --argon2-iterations/LINK_ARGON2_ITERATIONS; clave de @encrypted vía --encryption-key/LINK_ENCRYPTION_KEY (32 bytes en base64), obligatoria si el programa declara algún campo @encrypted; sin JWT externo por default, o verificando JWTs HS256 de un backend ya existente vía --jwt-secret/LINK_JWT_SECRET, con --jwt-role-claim/LINK_JWT_ROLE_CLAIM y --jwt-user-id-claim/LINK_JWT_USER_ID_CLAIM para elegir qué claims traen el rol y el id, default 'role'/'sub'; body de request acotado a 10 MiB por default, configurable vía --max-body-bytes/LINK_MAX_BODY_BYTES (bytes); llamadas http.* salientes con timeout de 30s por default, configurable vía --http-timeout/LINK_HTTP_TIMEOUT (ej. '10s'); @rate_limit identifica por remote_addr() por default, o por X-Forwarded-For con --trust-proxy/LINK_TRUST_PROXY (solo detrás de un proxy de confianza); crea/migra tablas por default, o --adopt-existing/LINK_ADOPT_EXISTING para asumir que ya existen y no tocar DDL; sin reintento nativo por default, o backoff exponencial ante un fallo de bind/conexión vía --restart-backoff/LINK_RESTART_BACKOFF, ej. '1s'; sin autenticación servidor-a-servidor por default, o exigir el header X-Service-Api-Key en toda request que no sea /health vía --service-api-key/LINK_SERVICE_API_KEY; log de texto por default, o JSON por línea vía --log-format/LINK_LOG_FORMAT; nivel de log 'info' por default -- las dos líneas por request de siempre --, o 'warn'/'error' para solo ver 4xx/5xx en producción con tráfico real, vía --log-level/LINK_LOG_LEVEL; sin Strict-Transport-Security por default -- linkc serve nunca termina TLS por sí solo --, o con el valor literal que se pase vía --hsts/LINK_HSTS, ej. 'max-age=63072000; includeSubDomains', SOLO si un proxy de confianza termina TLS delante)"));
    out(&format!("     linkc serve-all <directorio> --port-base <N> [--port-map-out <archivo.json>] [mismos flags globales que 'linkc serve', salvo --db]  (UN proceso sirve TODOS los .link de <directorio>, cada uno en su propio hilo y puerto N/N+1/N+2/... en orden alfabético; cada servicio conserva su propio archivo SQLite -- --db/LINK_DATABASE_URL compartido no está soportado; --port-map-out escribe {{\"nombre_archivo\": puerto, ...}} a un JSON antes de arrancar, para que un gateway externo lea la asignación real en vez de replicarla a mano)"));
    out(&format!("     linkc lsp                              (inicia el servidor Language Server Protocol)"));
    out(&format!("     linkc --version                        (imprime la versión exacta de este binario -- la misma que queda estampada en cada archivo que 'linkc build' genera)"));
}


fn cmd_fmt(args: &[String]) -> ExitCode {
    let Some(path) = args.first() else {
        eprintln!("uso: linkc fmt <archivo.link> [--check]");
        return ExitCode::FAILURE;
    };
    let check_mode = args.iter().any(|a| a == "--check");
    let content = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("no se pudo leer {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let formatted = match linkc::fmt::format_source(&content) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("error de sintaxis al formatear {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    if check_mode {
        if content == formatted {
            println!("OK: {path} está correctamente formateado");
            ExitCode::SUCCESS
        } else {
            eprintln!("FALLO: {path} requiere formateo (ejecuta 'linkc fmt {path}')");
            ExitCode::FAILURE
        }
    } else {
        if content != formatted {
            if let Err(e) = fs::write(path, formatted) {
                eprintln!("no se pudo escribir {path}: {e}");
                return ExitCode::FAILURE;
            }
            println!("formateado {path}");
        } else {
            println!("{path} ya está formateado");
        }
        ExitCode::SUCCESS
    }
}

fn cmd_lint(args: &[String]) -> ExitCode {
    let Some(path) = args.iter().find(|a| !a.starts_with("--")) else {
        eprintln!("uso: linkc lint <archivo.link> [--fix]");
        return ExitCode::FAILURE;
    };
    let fix_mode = args.iter().any(|a| a == "--fix");
    let program = match load_and_check(path) {
        Ok(p) => p,
        Err(code) => return code,
    };
    let warnings = linkc::lint::lint_program(&program);
    if warnings.is_empty() {
        println!("OK: {path} pasó el análisis de lint sin advertencias");
        ExitCode::SUCCESS
    } else {
        if fix_mode {
            if let Ok(content) = fs::read_to_string(path) {
                let fixed = linkc::lint::fix_source(&content, &warnings);
                if fixed != content {
                    let _ = fs::write(path, fixed);
                    println!("lint --fix: correcciones automáticas aplicadas en {path}");
                }
            }
        }
        println!("lint: {} advertencia(s) en {path}:", warnings.len());
        for w in &warnings {
            println!("  [{}] {}:{}:{}: {}", w.rule, path, w.line, w.col, w.message);
        }
        ExitCode::SUCCESS
    }
}

fn cmd_doc(args: &[String]) -> ExitCode {
    let Some(path) = args.first() else {
        eprintln!("uso: linkc doc <archivo.link> [outdir]");
        return ExitCode::FAILURE;
    };
    let program = match load_and_check(path) {
        Ok(p) => p,
        Err(code) => return code,
    };
    let html = linkc::doc::generate_html(&program, path);
    let out_path = if let Some(dir) = args.get(1) {
        let p = Path::new(dir);
        let _ = fs::create_dir_all(p);
        p.join("index.html")
    } else {
        Path::new(path).with_extension("html")
    };
    if let Err(e) = fs::write(&out_path, html) {
        eprintln!("no se pudo escribir documentación en {}: {e}", out_path.display());
        return ExitCode::FAILURE;
    }
    println!("documentación HTML generada en {}", out_path.display());
    ExitCode::SUCCESS
}

fn cmd_docker(args: &[String]) -> ExitCode {
    let Some(path) = args.first() else {
        eprintln!("uso: linkc docker <archivo.link> [outdir]");
        return ExitCode::FAILURE;
    };
    let out_dir = args.get(1).map(Path::new).unwrap_or_else(|| Path::new("."));
    match linkc::docker::generate_docker_files(path, out_dir) {
        Ok(files) => {
            println!("archivos de contenedor generados exitosamente en {}:", out_dir.display());
            for f in files {
                println!("  - {}", f.display());
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error al generar configuración Docker: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `linkc systemd <archivo.link> <puerto> [outdir]` (GRAMMAR.md §3.120,
/// PLAN.md §9.7) -- a diferencia de `linkc docker`, el puerto es un
/// argumento REQUERIDO (mismo motivo que `linkc serve`: no hay un puerto
/// por default que la unidad pudiera asumir).
fn cmd_systemd(args: &[String]) -> ExitCode {
    let (Some(path), Some(port_str)) = (args.first(), args.get(1)) else {
        eprintln!("uso: linkc systemd <archivo.link> <puerto> [outdir]");
        return ExitCode::FAILURE;
    };
    let Ok(port) = port_str.parse::<u16>() else {
        eprintln!("puerto inválido: '{port_str}'");
        return ExitCode::FAILURE;
    };
    let out_dir = args.get(2).map(Path::new).unwrap_or_else(|| Path::new("."));
    match linkc::systemd::generate_systemd_unit(path, port, out_dir) {
        Ok(unit_path) => {
            println!("unidad systemd generada exitosamente: {}", unit_path.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error al generar la unidad systemd: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `linkc pm2-config <archivo.link> <puerto> [-o <archivo>]` (GRAMMAR.md
/// §3.121, PLAN.md §9.7) -- `-o` toma un VALOR (mismo criterio de filtrado
/// que `--diff` en `cmd_build`, extraerlo antes de tratar el resto como
/// posicional), default `./ecosystem.json` si se omite.
fn cmd_pm2_config(args: &[String]) -> ExitCode {
    let mut positional = Vec::new();
    let mut out_path: Option<&str> = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "-o" {
            let Some(value) = args.get(i + 1) else {
                eprintln!("uso: linkc pm2-config <archivo.link> <puerto> [-o <archivo>]");
                return ExitCode::FAILURE;
            };
            out_path = Some(value);
            i += 2;
        } else {
            positional.push(args[i].as_str());
            i += 1;
        }
    }
    let (Some(path), Some(port_str)) = (positional.first(), positional.get(1)) else {
        eprintln!("uso: linkc pm2-config <archivo.link> <puerto> [-o <archivo>]");
        return ExitCode::FAILURE;
    };
    let Ok(port) = port_str.parse::<u16>() else {
        eprintln!("puerto inválido: '{port_str}'");
        return ExitCode::FAILURE;
    };
    let out_path = Path::new(out_path.unwrap_or("ecosystem.json"));
    match linkc::pm2::generate_pm2_config(path, port, out_path) {
        Ok(generated) => {
            println!("configuración PM2 generada exitosamente: {}", generated.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error al generar la configuración PM2: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `linkc introspect <db-url>` (GRAMMAR.md §3.66): el `.link` generado va a
/// STDOUT (para `> main.link`, o para revisarlo en la terminal antes de
/// guardarlo); las advertencias -- columnas que necesitan revisión manual --
/// van a STDERR, así que un `> main.link` no se las lleva puestas adentro
/// del archivo por error.
fn cmd_introspect(args: &[String]) -> ExitCode {
    let Some(url) = args.first() else {
        eprintln!("uso: linkc introspect <db-url>");
        return ExitCode::FAILURE;
    };
    match linkc::introspect::generate_link_from_postgres(url) {
        Ok((content, warnings)) => {
            println!("{content}");
            if !warnings.is_empty() {
                eprintln!("linkc introspect: {} advertencia(s) -- revisar antes de usar en producción:", warnings.len());
                for w in &warnings {
                    eprintln!("  - {w}");
                }
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error al introspeccionar '{url}': {e}");
            ExitCode::FAILURE
        }
    }
}

/// GRAMMAR.md §3.97: solo soporta `--dry-run` en esta ronda -- aplicar de
/// verdad ya pasa automáticamente al conectar con `linkc serve`/
/// `serve-all`, así que un `linkc migrate` sin `--dry-run` no tiene todavía
/// un comportamiento propio que no sea ambiguo con eso. Rechazado
/// explícito, con el motivo, en vez de hacer algo inesperado en silencio.
/// Solo PostgreSQL: SQLite ya reporta el diff exacto al conectar de verdad
/// (`check_schema_matches`, GRAMMAR.md §3.17), antes de tocar nada.
fn cmd_migrate(args: &[String]) -> ExitCode {
    let Some(path) = args.first() else {
        eprintln!("uso: linkc migrate <archivo.link> --db <url-postgres> --dry-run");
        return ExitCode::FAILURE;
    };
    if !args.iter().any(|a| a == "--dry-run") {
        eprintln!(
            "uso: linkc migrate <archivo.link> --db <url-postgres> --dry-run -- esta ronda solo soporta \
             --dry-run (mostrar el DDL sin aplicarlo). Aplicar de verdad ya pasa automáticamente al conectar \
             con 'linkc serve'/'linkc serve-all', que es intencional, no un olvido."
        );
        return ExitCode::FAILURE;
    }
    let url = match read_flag_or_env(args, "--db", "LINK_DATABASE_URL") {
        Ok(Some(u)) => u,
        Ok(None) => {
            eprintln!("uso: linkc migrate <archivo.link> --db <url-postgres> --dry-run (o LINK_DATABASE_URL)");
            return ExitCode::FAILURE;
        }
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };
    if !(url.starts_with("postgres://") || url.starts_with("postgresql://")) {
        eprintln!(
            "'linkc migrate --dry-run' solo aplica a PostgreSQL -- SQLite ya reporta el diff exacto al \
             conectar de verdad ('linkc serve'), antes de tocar nada, sin necesitar un modo aparte."
        );
        return ExitCode::FAILURE;
    }

    let program = match load_and_check(path) {
        Ok(p) => p,
        Err(code) => return code,
    };

    match linkc::migrate::dry_run_postgres(&program, &url) {
        Ok(report) => {
            println!("{report}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Enmascara `usuario:contraseña@` en una URL de conexión antes de
/// imprimirla -- `linkc doctor` muestra QUÉ base está configurada
/// (diagnóstico útil), pero nunca la credencial en sí, ni siquiera en la
/// terminal local (un log/CI que capture stdout no debería terminar con un
/// secreto adentro). Si la URL no tiene `://` o no tiene `@` antes del host,
/// se devuelve tal cual -- no hay nada que enmascarar.
fn redact_url_credentials(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else { return url.to_string() };
    let after_scheme = &url[scheme_end + 3..];
    let Some(at) = after_scheme.find('@') else { return url.to_string() };
    format!("{}://***@{}", &url[..scheme_end], &after_scheme[at + 1..])
}

/// `linkc doctor <archivo.link> [--db <url|archivo>]` (GRAMMAR.md §3.100,
/// originalmente PLAN.md §9.7.1): diagnóstico de entorno antes de un
/// despliegue, pensado para correr en CI o a mano justo antes de `linkc
/// serve`/`serve-all`. Interpretación deliberada de "PATH" del ítem
/// original: `linkc` es un binario estático sin ningún otro ejecutable de
/// sistema del que depender, así que revisar la variable de entorno `PATH`
/// no daría ninguna señal real -- lo que SÍ importa antes de un despliegue
/// es que el `.link` de entrada y todos sus `import` resuelvan y tipen, que
/// es lo que este chequeo verifica en su lugar.
///
/// Cada chequeo es independiente entre sí (uno que falla no cancela los
/// demás) -- el reporte completo importa más que salir rápido en el primer
/// error, mismo criterio que `linkc migrate --dry-run` reportando por
/// colección en vez de abortar en la primera. Código de salida: `FAILURE`
/// si algún chequeo real dio error (archivo inválido, sin permiso de
/// escritura, base inalcanzable); un `--db` de Postgres es el único chequeo
/// que hace una conexión de red de verdad, y lo hace de SOLO LECTURA (`SELECT
/// 1` vía `check_postgres_connectivity`) -- nunca crea ni altera tablas,
/// para eso ya está `linkc migrate --dry-run`.
fn cmd_doctor(args: &[String]) -> ExitCode {
    let Some(path) = args.first() else {
        eprintln!("uso: linkc doctor <archivo.link> [--db <url|archivo>] [--target-url <url>]");
        return ExitCode::FAILURE;
    };
    println!("linkc doctor -- diagnóstico de entorno para '{path}'\n");

    let mut ok_count = 0usize;
    let mut err_count = 0usize;

    println!("[OK]    versión de linkc: {}", linkc::VERSION);
    ok_count += 1;

    match load_and_check(path) {
        Ok(_) => {
            println!("[OK]    '{path}' existe, resuelve sus imports, parsea y tipa correctamente");
            ok_count += 1;
        }
        Err(_) => {
            println!("[ERROR] '{path}' no se pudo cargar -- ver los errores impresos arriba");
            err_count += 1;
        }
    }

    let check_dir = Path::new(path).parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    let probe = check_dir.join(".linkc_doctor_check");
    match fs::write(&probe, b"linkc doctor") {
        Ok(()) => {
            let _ = fs::remove_file(&probe);
            println!("[OK]    permiso de escritura en '{}'", display_path(check_dir));
            ok_count += 1;
        }
        Err(e) => {
            println!("[ERROR] sin permiso de escritura en '{}': {e}", display_path(check_dir));
            err_count += 1;
        }
    }

    match resolve_db_source(path, args) {
        Ok(runtime::server::DbSource::SqliteFile(db_path)) => {
            println!(
                "[OK]    base de datos: SQLite embebido en '{}' (sin --db/LINK_DATABASE_URL configurada)",
                display_path(&db_path)
            );
            ok_count += 1;
        }
        Ok(runtime::server::DbSource::Postgres(url)) => {
            println!("[INFO]  base de datos: PostgreSQL configurada ({})", redact_url_credentials(&url));
            match runtime::db::check_postgres_connectivity(&url) {
                Ok(()) => {
                    println!("[OK]    conectividad a PostgreSQL: conectó y respondió (solo lectura, no se tocó ningún schema)");
                    ok_count += 1;
                }
                Err(e) => {
                    println!("[ERROR] conectividad a PostgreSQL: {e}");
                    err_count += 1;
                }
            }
        }
        Err(msg) => {
            println!("[ERROR] configuración de base de datos inválida: {msg}");
            err_count += 1;
        }
    }

    // `--target-url`/`LINK_DOCTOR_TARGET_URL`, opt-in: compara la versión
    // LOCAL contra la de un `linkc serve` real ya corriendo (vía `/health`,
    // que ya devuelve `version` desde siempre). Reporte de adopción real
    // (iaacademy, vía la sesión skynet-43, 2026-08-29): el PC de desarrollo
    // puede quedar en una versión vieja mientras el VPS de producción sigue
    // avanzando -- compilar/testear local contra un binario más viejo que
    // el de producción es una deriva silenciosa, sin ningún error que la
    // señale. Sin el flag: comportamiento IDÉNTICO a siempre, cero
    // requests salientes.
    match read_flag_or_env(args, "--target-url", "LINK_DOCTOR_TARGET_URL") {
        Ok(Some(target)) => {
            let health_url = format!("{}/health", target.trim_end_matches('/'));
            match ureq::get(&health_url).timeout(Duration::from_secs(5)).call() {
                Ok(resp) => match serde_json::from_str::<serde_json::Value>(&resp.into_string().unwrap_or_default()) {
                    Ok(json) => match json.get("version").and_then(|v| v.as_str()) {
                        Some(remote) if remote == linkc::VERSION => {
                            println!("[OK]    versión remota ({health_url}): {remote}, igual a la local");
                            ok_count += 1;
                        }
                        Some(remote) => {
                            println!(
                                "[INFO]  versión remota ({health_url}): {remote}, distinta de la local ({}) -- \
                                 compilar/testear acá contra un binario más viejo o más nuevo que el que corre \
                                 ahí puede esconder una regresión o una feature que ese lado todavía no tiene",
                                linkc::VERSION
                            );
                            ok_count += 1;
                        }
                        None => {
                            println!(
                                "[ERROR] versión remota ({health_url}): la respuesta no trae 'version' -- ¿es un servidor linkc?"
                            );
                            err_count += 1;
                        }
                    },
                    Err(e) => {
                        println!("[ERROR] versión remota ({health_url}): la respuesta no es JSON válido: {e}");
                        err_count += 1;
                    }
                },
                Err(e) => {
                    println!("[ERROR] versión remota ({health_url}): no se pudo conectar/responder: {e}");
                    err_count += 1;
                }
            }
        }
        Ok(None) => {}
        Err(msg) => {
            println!("[ERROR] {msg}");
            err_count += 1;
        }
    }

    println!("\n{ok_count} OK, {err_count} error(es)");
    if err_count > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// `linkc db <sub-subcomando>` (GRAMMAR.md §3.175/§3.185, PLAN.md §9.7 ítem
/// 2) -- `inspect`/`export`/`import` existen; `shell`/`seed` quedan para
/// rondas futuras (`seed` en rigor no necesita su propia pieza: importar
/// contra un target vacío YA ES ese caso, mismo mecanismo).
fn cmd_db(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("inspect") => cmd_db_inspect(&args[1..]),
        Some("export") => cmd_db_export(&args[1..]),
        Some("import") => cmd_db_import(&args[1..]),
        Some("shell") => cmd_db_shell(&args[1..]),
        _ => {
            eprintln!(
                "uso: linkc db inspect <archivo.link> [--db <url|archivo>]\n     linkc db export <archivo.link> <archivo.json> [--db <url|archivo>]\n     linkc db import <archivo.link> <archivo.json> [--db <url|archivo>]\n     linkc db shell <archivo.link> [--db <url|archivo>]"
            );
            ExitCode::FAILURE
        }
    }
}

/// `linkc db inspect <archivo.link> [--db <url|archivo>]` -- lista cada
/// colección declarada con su estado FÍSICO real (existe o no, filas),
/// SIN ejecutar ningún DDL -- a diferencia de `linkc serve`, que crea/migra
/// tablas al conectar. Mismo espíritu de solo-lectura que `linkc doctor`/
/// `linkc migrate --dry-run` (de hecho reusa `resolve_db_source`, el mismo
/// resolvedor de `--db`/`LINK_DATABASE_URL` que esos dos y que `serve`).
fn cmd_db_inspect(args: &[String]) -> ExitCode {
    let Some(path) = args.first() else {
        eprintln!("uso: linkc db inspect <archivo.link> [--db <url|archivo>]");
        return ExitCode::FAILURE;
    };
    let program = match load_and_check(path) {
        Ok(p) => p,
        Err(code) => return code,
    };
    let source = match resolve_db_source(path, args) {
        Ok(s) => s,
        Err(msg) => {
            eprintln!("configuración de base de datos inválida: {msg}");
            return ExitCode::FAILURE;
        }
    };
    let (label, result) = match &source {
        runtime::server::DbSource::SqliteFile(db_path) => {
            (format!("SQLite embebido en '{}'", display_path(db_path)), linkc::inspect::inspect_sqlite(&program, db_path))
        }
        runtime::server::DbSource::Postgres(url) => {
            (format!("PostgreSQL ({})", redact_url_credentials(url)), linkc::inspect::inspect_postgres(&program, url))
        }
    };
    let statuses = match result {
        Ok(s) => s,
        Err(msg) => {
            eprintln!("no se pudo inspeccionar la base: {msg}");
            return ExitCode::FAILURE;
        }
    };
    println!("linkc db inspect -- '{path}' contra {label}\n");
    if statuses.is_empty() {
        println!("(este programa no declara ninguna colección en 'db {{ ... }}')");
        return ExitCode::SUCCESS;
    }
    let name_width = statuses.iter().map(|s| s.name.len()).max().unwrap_or(0).max(10);
    let mut total_rows: i64 = 0;
    let mut missing = 0usize;
    for s in &statuses {
        let status = match s.row_count {
            Some(n) => {
                total_rows += n;
                format!("{n} fila(s)")
            }
            None => {
                missing += 1;
                "no existe todavía".to_string()
            }
        };
        println!("  {:<name_width$}  {} columna(s) declaradas  {}", s.name, s.declared_columns, status);
    }
    println!("\n{} colección(es) declaradas, {} sin crear todavía, {total_rows} fila(s) en total", statuses.len(), missing);
    ExitCode::SUCCESS
}

/// `linkc db export <archivo.link> <archivo.json> [--db <url|archivo>]` --
/// vuelca cada colección declarada a un solo archivo JSON, byte-idéntico
/// al wire real (GRAMMAR.md §3.185). Nunca ejecuta DDL, mismo espíritu de
/// solo lectura que `db inspect`.
fn cmd_db_export(args: &[String]) -> ExitCode {
    let (Some(path), Some(out_path)) = (args.first(), args.get(1)) else {
        eprintln!("uso: linkc db export <archivo.link> <archivo.json> [--db <url|archivo>]");
        return ExitCode::FAILURE;
    };
    let program = match load_and_check(path) {
        Ok(p) => p,
        Err(code) => return code,
    };
    let source = match resolve_db_source(path, args) {
        Ok(s) => s,
        Err(msg) => {
            eprintln!("configuración de base de datos inválida: {msg}");
            return ExitCode::FAILURE;
        }
    };
    let (label, result) = match &source {
        runtime::server::DbSource::SqliteFile(db_path) => {
            (format!("SQLite embebido en '{}'", display_path(db_path)), linkc::db_admin::export_sqlite(&program, db_path))
        }
        runtime::server::DbSource::Postgres(url) => {
            (format!("PostgreSQL ({})", redact_url_credentials(url)), linkc::db_admin::export_postgres(&program, url))
        }
    };
    let file = match result {
        Ok(f) => f,
        Err(msg) => {
            eprintln!("no se pudo exportar la base: {msg}");
            return ExitCode::FAILURE;
        }
    };
    let json = match serde_json::to_string_pretty(&file) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("no se pudo serializar el export: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = std::fs::write(out_path, json) {
        eprintln!("no se pudo escribir '{out_path}': {e}");
        return ExitCode::FAILURE;
    }
    let mut names: Vec<&String> = file.collections.keys().collect();
    names.sort();
    let total_rows: usize = file.collections.values().map(|rows| rows.len()).sum();
    println!("linkc db export -- '{path}' contra {label} -> '{out_path}'\n");
    for name in &names {
        println!("  {name}  {} fila(s)", file.collections[*name].len());
    }
    println!("\n{} colección(es), {total_rows} fila(s) en total exportadas", names.len());
    ExitCode::SUCCESS
}

/// `linkc db import <archivo.link> <archivo.json> [--db <url|archivo>]` --
/// lee un archivo generado por `db export` y escribe sus filas contra un
/// target SQLite o PostgreSQL, PRESERVANDO el id original de cada fila
/// (GRAMMAR.md §3.185). Un target vacío ES el caso "seed" -- mismo camino,
/// sin flag aparte. Un choque de id (o cualquier otro error) cancela y
/// revierte TODO el import, nunca deja datos a medias.
fn cmd_db_import(args: &[String]) -> ExitCode {
    let (Some(path), Some(in_path)) = (args.first(), args.get(1)) else {
        eprintln!("uso: linkc db import <archivo.link> <archivo.json> [--db <url|archivo>]");
        return ExitCode::FAILURE;
    };
    let program = match load_and_check(path) {
        Ok(p) => p,
        Err(code) => return code,
    };
    let raw = match std::fs::read_to_string(in_path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("no se pudo leer '{in_path}': {e}");
            return ExitCode::FAILURE;
        }
    };
    let file: linkc::db_admin::ExportFile = match serde_json::from_str(&raw) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("'{in_path}' no es un archivo de export válido: {e}");
            return ExitCode::FAILURE;
        }
    };
    let source = match resolve_db_source(path, args) {
        Ok(s) => s,
        Err(msg) => {
            eprintln!("configuración de base de datos inválida: {msg}");
            return ExitCode::FAILURE;
        }
    };
    let (label, result) = match &source {
        runtime::server::DbSource::SqliteFile(db_path) => {
            (format!("SQLite embebido en '{}'", display_path(db_path)), linkc::db_admin::import_sqlite(&program, db_path, &file))
        }
        runtime::server::DbSource::Postgres(url) => {
            (format!("PostgreSQL ({})", redact_url_credentials(url)), linkc::db_admin::import_postgres(&program, url, &file))
        }
    };
    let report = match result {
        Ok(r) => r,
        Err(msg) => {
            eprintln!("no se pudo importar: {msg}");
            return ExitCode::FAILURE;
        }
    };
    let total_rows: usize = report.iter().map(|(_, n)| n).sum();
    println!("linkc db import -- '{in_path}' contra {label}\n");
    for (name, n) in &report {
        println!("  {name}  {n} fila(s) importadas");
    }
    println!("\n{} colección(es), {total_rows} fila(s) en total importadas", report.len());
    ExitCode::SUCCESS
}

/// `linkc db shell <archivo.link> [--db <url|archivo>]` -- REPL de SOLO
/// LECTURA sobre stdin/stdout, línea por línea (GRAMMAR.md §3.189, cierra
/// PLAN.md §9.7 ítem 2, la suite de administración de datos). A diferencia
/// de `inspect`/`export`/`import`, no necesita `load_and_check` -- el shell
/// acepta SQL suelto, sin ninguna semántica de `.link` de por medio; el
/// archivo de entrada solo le da a `resolve_db_source` el nombre por
/// default de la base (`<archivo>.db`) si no hay `--db`/`LINK_DATABASE_URL`.
fn cmd_db_shell(args: &[String]) -> ExitCode {
    let Some(path) = args.first() else {
        eprintln!("uso: linkc db shell <archivo.link> [--db <url|archivo>]");
        return ExitCode::FAILURE;
    };
    let source = match resolve_db_source(path, args) {
        Ok(s) => s,
        Err(msg) => {
            eprintln!("configuración de base de datos inválida: {msg}");
            return ExitCode::FAILURE;
        }
    };
    let result = match &source {
        runtime::server::DbSource::SqliteFile(db_path) => linkc::db_admin::run_shell_sqlite(db_path),
        runtime::server::DbSource::Postgres(url) => linkc::db_admin::run_shell_postgres(url),
    };
    if let Err(msg) = result {
        eprintln!("no se pudo iniciar el shell: {msg}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
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
    let Some(name) = args.iter().find(|a| !a.starts_with("--")) else {
        eprintln!("uso: linkc new <nombre> [--template nextjs|vite|minimal]");
        return ExitCode::FAILURE;
    };
    let mut template = scaffold::Template::Minimal;
    if let Some(pos) = args.iter().position(|a| a == "--template") {
        if let Some(t_str) = args.get(pos + 1) {
            if let Some(t) = scaffold::Template::parse(t_str) {
                template = t;
            } else {
                eprintln!("plantilla desconocida '{t_str}'. Opciones válidas: nextjs, vite, minimal");
                return ExitCode::FAILURE;
            }
        }
    }
    scaffold::new_project_with_template(name, template)
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
        eprintln!("'{path}' no es un subcomando conocido ni un archivo .link existente -- `linkc --help` lista los subcomandos");
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

fn build_once(path: &str, outdir: &str, update_deps: bool) -> BuildResult {
    // `load_program_full`, no `load_program`: además del trío de
    // siempre, necesita `git_dependencies` (GRAMMAR.md §2.1, package
    // manager real) para grabar en `link.lock` más abajo -- `linkc
    // check`/`serve`/`wasm` (que sí usan `load_program` vía
    // `load_and_check`) no escriben ningún lockfile, así que no
    // necesitan este cuarto valor. `update_deps` (GRAMMAR.md §3.183):
    // `linkc dev` siempre pasa `false` -- un rebuild automático por watch
    // respeta el pin existente, igual que un `linkc build` normal; solo
    // `linkc build --update-deps` explícito puede pedir una resolución
    // fresca.
    let (program, touched, item_files, git_dependencies) = match modules::load_program_full(Path::new(path), &HashMap::new(), update_deps) {
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
    let hooks = match codegen::ts_emit::emit_hooks(&program) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error al emitir hooks.ts: {e}");
            return BuildResult { ok: false, touched };
        }
    };
    let schemas = match codegen::zod_emit::emit_zod_schemas(&program) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error al emitir schemas.ts: {e}");
            return BuildResult { ok: false, touched };
        }
    };
    let openapi = match codegen::openapi_emit::emit_openapi_json(&program, display_path(Path::new(path)).as_str()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error al emitir openapi.json: {e}");
            return BuildResult { ok: false, touched };
        }
    };
    let llms_txt = match codegen::llms_txt_emit::emit_llms_txt(&program, display_path(Path::new(path)).as_str()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error al emitir llms.txt: {e}");
            return BuildResult { ok: false, touched };
        }
    };
    let llms_txt_full = match codegen::llms_txt_emit::emit_llms_txt_full(&program, display_path(Path::new(path)).as_str()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error al emitir llms-full.txt: {e}");
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
    let hooks_path = format!("{outdir}/hooks.ts");
    let schemas_path = format!("{outdir}/schemas.ts");
    let openapi_path = format!("{outdir}/openapi.json");
    let llms_txt_path = format!("{outdir}/llms.txt");
    let llms_txt_full_path = format!("{outdir}/llms-full.txt");
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
    if let Err(e) = fs::write(&hooks_path, hooks) {
        eprintln!("no se pudo escribir {hooks_path}: {e}");
        return BuildResult { ok: false, touched };
    }
    if let Err(e) = fs::write(&schemas_path, schemas) {
        eprintln!("no se pudo escribir {schemas_path}: {e}");
        return BuildResult { ok: false, touched };
    }
    if let Err(e) = fs::write(&openapi_path, openapi) {
        eprintln!("no se pudo escribir {openapi_path}: {e}");
        return BuildResult { ok: false, touched };
    }
    if let Err(e) = fs::write(&llms_txt_path, llms_txt) {
        eprintln!("no se pudo escribir {llms_txt_path}: {e}");
        return BuildResult { ok: false, touched };
    }
    if let Err(e) = fs::write(&llms_txt_full_path, llms_txt_full) {
        eprintln!("no se pudo escribir {llms_txt_full_path}: {e}");
        return BuildResult { ok: false, touched };
    }

    let wasm_path = format!("{outdir}/main.wasm");
    match codegen::wasm_emit::emit_wasm(&program) {
        Ok(wasm_bytes) => {
            if let Err(e) = fs::write(&wasm_path, wasm_bytes) {
                eprintln!("advertencia: no se pudo escribir {wasm_path}: {e}");
            }
            println!("OK: generado {contract_path}, {client_path}, {validators_path}, {hooks_path}, {schemas_path}, {openapi_path}, {llms_txt_path}, {llms_txt_full_path} y {wasm_path}");
        }
        Err(e) => {
            println!("OK: generado {contract_path}, {client_path}, {validators_path}, {hooks_path}, {schemas_path}, {openapi_path}, {llms_txt_path}, {llms_txt_full_path}");
            eprintln!(
                "advertencia: no se generó {wasm_path} -- el codegen wasm nativo (solo funciones/escalares) no soporta este programa: {e}"
            );
        }
    }

    if let Ok(pg_ddl) = linkc::codegen::postgres_emit::generate_postgres_ddl(&program) {
        let pg_sql_path = format!("{outdir}/schema.postgres.sql");
        let _ = fs::write(&pg_sql_path, pg_ddl);
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



/// `--diff <contract.d.ts anterior>` (GRAMMAR.md §3.79) se extrae ACÁ, no
/// adentro de `build_once` -- toma un VALOR (no un flag suelto como
/// `--update` de `linkc test`), así que hay que consumir los dos tokens
/// juntos antes de tratar el resto como posicional (`path`/`outdir`), mismo
/// criterio de filtrado que `cmd_test` ya usa para `snap_path`/`--update`.
/// `--update-deps` (GRAMMAR.md §3.183) es un flag suelto, sin valor -- mismo
/// criterio que `--update` de `cmd_test`, filtrado sin consumir un token extra.
fn cmd_build(args: &[String]) -> ExitCode {
    let mut positional = Vec::new();
    let mut diff_against: Option<&str> = None;
    let mut update_deps = false;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--diff" {
            let Some(value) = args.get(i + 1) else {
                eprintln!("uso: linkc build <archivo.link> <outdir> [--diff <contract.d.ts anterior>] [--update-deps]");
                return ExitCode::FAILURE;
            };
            diff_against = Some(value);
            i += 2;
        } else if args[i] == "--update-deps" {
            update_deps = true;
            i += 1;
        } else {
            positional.push(args[i].as_str());
            i += 1;
        }
    }
    let (Some(path), Some(outdir)) = (positional.first(), positional.get(1)) else {
        eprintln!("uso: linkc build <archivo.link> <outdir> [--diff <contract.d.ts anterior>] [--update-deps]");
        return ExitCode::FAILURE;
    };
    let result = build_once(path, outdir, update_deps);
    if !result.ok {
        return ExitCode::FAILURE;
    }
    if let Some(prev_path) = diff_against {
        print_build_diff(prev_path, outdir);
    }
    ExitCode::SUCCESS
}

/// `linkc build --diff <archivo-anterior>` (PLAN.md §9.3, GRAMMAR.md §3.79):
/// compara el `contract.d.ts` RECIÉN generado contra una copia anterior
/// guardada aparte (ej. `git show HEAD~5:gen/contract.d.ts > /tmp/viejo.d.ts`),
/// para revisión de PR -- "¿qué cambió en el contrato público entre estas
/// dos versiones del `.link`?". Reusa `diff_lines`, el mismo LCS que ya usa
/// `linkc test` -- no reimplementa nada, solo lo llama desde otro lugar.
/// Puramente informativo: a diferencia de `linkc test` (que SÍ falla si el
/// snapshot no matchea, porque ahí un cambio sin querer es justo lo que se
/// busca atrapar), acá no hay "correcto"/"incorrecto" -- el build ya tuvo
/// éxito antes de llegar acá, esto solo muestra qué cambió para que una
/// persona lo revise.
fn print_build_diff(prev_path: &str, outdir: &str) {
    let current_path = format!("{outdir}/contract.d.ts");
    let current = match fs::read_to_string(&current_path) {
        Ok(s) => s.replace("\r\n", "\n"),
        Err(e) => {
            eprintln!("--diff: no se pudo leer '{current_path}' (recién generado): {e}");
            return;
        }
    };
    // Mismo `.replace("\r\n", "\n")` que `run_snapshot_test` ya aplica, y
    // mismo motivo: el archivo de comparación pudo pasar por un checkout de
    // git con `core.autocrlf=true`, y la corrección de este comando no
    // debería depender de esa configuración ajena.
    let previous = match fs::read_to_string(prev_path) {
        Ok(s) => s.replace("\r\n", "\n"),
        Err(e) => {
            eprintln!("--diff: no se pudo leer '{prev_path}': {e}");
            return;
        }
    };
    if previous == current {
        println!("--diff: el contrato no cambió respecto a '{prev_path}'");
        return;
    }
    println!("--diff: el contrato cambió respecto a '{prev_path}':");
    println!("{}", diff_lines(&previous, &current));
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
    let Some(path) = args.first() else {
        eprintln!("uso: linkc test <archivo.link> [archivo.snap] [--update] [--filter <nombre>] [--db <url-postgres>]");
        return ExitCode::FAILURE;
    };

    let snap_path = args.get(1).filter(|a| !a.starts_with("--"));
    let update = args.iter().any(|a| a == "--update");
    let filter = match extract_flag_value(args, "--filter") {
        Ok(v) => v,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };
    // GRAMMAR.md §3.99: `--db <url-postgres>` corre los bloques `test
    // "..." { ... }` contra una base Postgres REAL en vez de SQLite
    // `:memory:` -- necesario para reproducir un bug del wire binario de
    // Postgres (§3.91), invisible contra SQLite porque los dos backends
    // emiten SQL distinto para el mismo `.link`. Sin el flag: comportamiento
    // IDÉNTICO al de siempre.
    let db_url = match read_flag_or_env(args, "--db", "LINK_TEST_DB") {
        Ok(v) => v,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };
    // `--filter` solo tiene sentido contra los bloques `test "..." { ... }`
    // integrados -- el testing de CONTRATO (`linkc test <archivo> <snap>`)
    // no tiene nombres que filtrar, así que combinarlos es un uso confuso
    // que se rechaza acá en vez de ignorar `--filter` en silencio. `--db`
    // comparte el mismo motivo: el testing de contrato no toca ninguna base.
    if snap_path.is_some() && filter.is_some() {
        eprintln!(
            "--filter solo aplica a los bloques 'test \"...\" {{ ... }}' integrados, no al testing de contrato ('linkc test <archivo> <snap>')"
        );
        return ExitCode::FAILURE;
    }
    if snap_path.is_some() && db_url.is_some() {
        eprintln!(
            "--db solo aplica a los bloques 'test \"...\" {{ ... }}' integrados, no al testing de contrato ('linkc test <archivo> <snap>')"
        );
        return ExitCode::FAILURE;
    }

    let program = match load_and_check(path) {
        Ok(p) => p,
        Err(code) => return code,
    };

    // Si se especificó un archivo snapshot, ejecutamos snapshot testing de contratos
    if let Some(snap_path) = snap_path {
        return run_snapshot_test(&program, path, snap_path, update);
    }

    // Si solo se pasó el archivo .link, ejecutamos los bloques test integrados
    // -- `--filter <nombre>` (PLAN.md §9.7, GRAMMAR.md §3.82) los acota a los
    // que CONTIENEN ese substring en el nombre, mismo criterio que
    // `cargo test <substring>`.
    let result = match db_url {
        None => runtime::run_program_tests_filtered(&program, filter.as_deref()),
        Some(url) => {
            if !(url.starts_with("postgres://") || url.starts_with("postgresql://")) {
                eprintln!(
                    "'--db' en 'linkc test' solo acepta una URL de PostgreSQL -- SQLite ':memory:' ya es el default sin el flag"
                );
                return ExitCode::FAILURE;
            }
            let adopt_existing = resolve_adopt_existing(args);
            match runtime::db::Db::connect_postgres_for_testing(&program, &url, adopt_existing) {
                Ok(db) => runtime::run_program_tests_against_db(&program, filter.as_deref(), &db),
                Err(e) => {
                    eprintln!("error: {e}");
                    return ExitCode::FAILURE;
                }
            }
        }
    };
    match result {
        Ok(summary) => {
            match &filter {
                Some(f) => println!("running {} tests (filtro: '{f}')", summary.total),
                None => println!("running {} tests", summary.total),
            }
            for (name, err) in &summary.failed {
                println!("test \"{name}\" ... FAILED: {err}");
            }
            let passed_count = summary.passed;
            let failed_count = summary.failed.len();
            if summary.failed.is_empty() {
                if summary.total > 0 {
                    println!("\ntest result: ok. {passed_count} passed; 0 failed\n");
                } else {
                    println!("\ntest result: ok. 0 tests run\n");
                }
                ExitCode::SUCCESS
            } else {
                println!("\ntest result: FAILED. {passed_count} passed; {failed_count} failed\n");
                ExitCode::FAILURE
            }
        }
        Err(e) => {
            eprintln!("error de ejecución: {}", e.message);
            ExitCode::FAILURE
        }
    }
}

fn run_snapshot_test(program: &Program, path: &str, snap_path: &str, update: bool) -> ExitCode {
    let contract = match codegen::ts_emit::emit_contract(program) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error al emitir contract.d.ts: {e}");
            return ExitCode::FAILURE;
        }
    };
    let client = match codegen::ts_emit::emit_client(program) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error al emitir client.ts: {e}");
            return ExitCode::FAILURE;
        }
    };
    let validators = match codegen::validators_emit::emit_validators(program) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error al emitir validators.ts: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Normalizado a LF puro -- ver el mismo `.replace` sobre `previous` más
    // abajo para el porqué (checkout de Windows con `core.autocrlf=true`
    // convierte el `.snap` commiteado a CRLF; sin esto, la comparación de
    // abajo falla con un "cambió" falso en TODA corrida sobre ese checkout,
    // el bug real que rompió CI en windows-latest, GRAMMAR.md §3.29).
    let current = format!(
        "=== contract.d.ts ===\n{contract}\n=== client.ts ===\n{client}\n=== validators.ts ===\n{validators}"
    )
    .replace("\r\n", "\n");

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

    // `.replace("\r\n", "\n")`: un checkout de git puede haber convertido
    // el `.snap` commiteado (siempre LF -- ver `current` arriba) a CRLF
    // según `core.autocrlf`/`.gitattributes` de la máquina que lo clonó.
    // Sin esto, la comparación de abajo depende de una configuración de
    // git ajena a este comando para ser correcta -- exactamente el tipo de
    // supuesto frágil que ya rompió CI una vez (ver el fixture de este
    // mismo bug en cli_test_snapshot.rs). `.gitattributes` fija `*.snap`
    // como LF para que el archivo commiteado no le muestre un diff de
    // solo-EOL a nadie, pero la corrección de este comando no depende de
    // eso -- funciona igual si alguien lo abre y resave en CRLF a mano.
    let previous = match fs::read_to_string(snap_file) {
        Ok(s) => s.replace("\r\n", "\n"),
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
    // sirviendo el puerto -- límite de v0 conocido, no manejado.
    println!("linkc dev: observando '{path}' y sus imports (Ctrl+C para detener)");
    let mut result = build_once(path, outdir, false);
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
            result = build_once(path, outdir, false);
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
        eprintln!(
            "uso: linkc serve <archivo.link> <puerto> [--db <url|archivo>] [--host <dirección>] [--cors-origin <origen>] [--session-ttl <duración>] [--max-body-bytes <N>] [--http-timeout <duración>] [--trust-proxy] [--adopt-existing] [--restart-backoff <duración>] [--service-api-key <clave>] [--log-format text|json] [--log-level debug|info|warn|error] [--hsts <valor>]"
        );
        return ExitCode::FAILURE;
    };
    let Ok(port) = port_str.parse::<u16>() else {
        eprintln!("puerto inválido: '{port_str}'");
        return ExitCode::FAILURE;
    };

    let host = match resolve_host(args) {
        Ok(h) => h,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };

    let source = match resolve_db_source(path, args) {
        Ok(s) => s,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };

    let cors = match resolve_cors_origins(args) {
        Ok(origins) => match origins {
            Some(list) => runtime::server::CorsConfig::Allowlist(list),
            None => runtime::server::CorsConfig::Any,
        },
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };

    let session_ttl = match resolve_session_ttl(args) {
        Ok(ttl) => ttl,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };

    let argon2_params = match resolve_argon2_params(args) {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };

    // GRAMMAR.md §3.191: solo se extrae el string acá -- decodificarlo
    // (base64, exactamente 32 bytes) y confirmar que hace falta si el
    // programa declara algún campo `@encrypted` vive en `serve()`
    // (`runtime/server.rs`), que sí puede llamar a `encryption::
    // parse_encryption_key` (`pub(crate)`, no alcanzable desde este
    // binario -- separado de la librería `linkc` a nivel de crate).
    let encryption_key = match read_flag_or_env(args, "--encryption-key", "LINK_ENCRYPTION_KEY") {
        Ok(k) => k,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };

    let jwt_config = match resolve_jwt_config(args) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };

    let adopt_existing = resolve_adopt_existing(args);

    let max_body_bytes = match resolve_max_body_bytes(args) {
        Ok(n) => n,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };

    let http_timeout = match resolve_http_timeout(args) {
        Ok(d) => d,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };

    let trust_proxy = resolve_trust_proxy(args);

    let restart_backoff = match resolve_restart_backoff(args) {
        Ok(b) => b,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };
    let service_api_key = match resolve_service_api_key(args) {
        Ok(k) => k,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };
    let log_format = match resolve_log_format(args) {
        Ok(f) => f,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };
    let log_level = match resolve_log_level(args) {
        Ok(l) => l,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };
    let log = runtime::server::LogConfig { format: log_format, level: log_level };
    let hsts = match resolve_hsts(args) {
        Ok(h) => h,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };

    let program = match load_and_check(path) {
        Ok(p) => p,
        Err(code) => return code,
    };

    let attempt = || {
        runtime::server::serve(
            &program,
            &host,
            port,
            source.clone(),
            cors.clone(),
            session_ttl,
            argon2_params.clone(),
            encryption_key.clone(),
            jwt_config.clone(),
            adopt_existing,
            max_body_bytes,
            http_timeout,
            trust_proxy,
            service_api_key.clone(),
            log,
            hsts.clone(),
        )
    };
    match run_serve_with_backoff(attempt, restart_backoff, path) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("{msg}");
            ExitCode::FAILURE
        }
    }
}

/// Techo del backoff exponencial (GRAMMAR.md §3.92) -- ningún reintento
/// espera más que esto, sin importar cuántos fallos consecutivos lleve.
const MAX_RESTART_BACKOFF: Duration = Duration::from_secs(30);

/// Cuánto tiempo corriendo SIN fallar hace falta para considerar que un
/// servicio ya está sano de nuevo y resetear el backoff a la base -- así
/// una racha vieja de fallos (ej. un arranque en frío con varios puertos
/// disputados) no sigue penalizando a un servicio que ya lleva rato andando
/// bien. Mismo espíritu que el "restart window" de systemd/pm2, un número
/// razonable, no exhaustivamente investigado -- mismo criterio que
/// `DEFAULT_MAX_BODY_BYTES` arriba.
const MIN_UPTIME_TO_RESET_BACKOFF: Duration = Duration::from_secs(60);

/// Reintenta `attempt` (normalmente un cierre que compila+arranca UN
/// servicio, ver `cmd_serve`/`cmd_serve_all`) ante un fallo RECUPERABLE de
/// `runtime::server::serve` (bind de puerto ocupado, Postgres caído al
/// arrancar) -- GRAMMAR.md §3.92. El incidente real que lo motiva: un
/// arranque en frío con varios procesos (pm2, en el caso reportado)
/// compitiendo por bindear sus puertos casi al mismo tiempo, donde alguno
/// pierde la carrera la primera vez -- hoy mitigado desde AFUERA del
/// lenguaje (`--restart-delay` fijo de pm2); esto lo hace nativo, con
/// backoff exponencial en vez de una espera fija siempre igual.
///
/// `backoff_base` es `None` sin `--restart-backoff`/`LINK_RESTART_BACKOFF`:
/// UN solo intento, comportamiento IDÉNTICO al de siempre -- el fallo se
/// devuelve tal cual, sin reintento nativo (delega en quien orqueste el
/// proceso, como ya hacía). Con `Some(base)`: cada fallo duplica la espera
/// (con techo `MAX_RESTART_BACKOFF`), reseteada a `base` después de
/// `MIN_UPTIME_TO_RESET_BACKOFF` de funcionamiento estable. `label` va en
/// cada línea de log -- imprescindible en `serve-all`, donde varios
/// servicios comparten un mismo stdout/stderr de proceso.
fn run_serve_with_backoff(attempt: impl Fn() -> Result<(), String>, backoff_base: Option<Duration>, label: &str) -> Result<(), String> {
    let Some(base) = backoff_base else {
        return attempt();
    };
    let mut delay = base;
    loop {
        let started = std::time::Instant::now();
        let err = match attempt() {
            Ok(()) => return Ok(()),
            Err(e) => e,
        };
        if started.elapsed() >= MIN_UPTIME_TO_RESET_BACKOFF {
            delay = base;
        }
        eprintln!("[{label}] {err}");
        eprintln!("[{label}] reintentando en {delay:?}...");
        std::thread::sleep(delay);
        delay = (delay * 2).min(MAX_RESTART_BACKOFF);
    }
}

/// `--restart-backoff <duración>`/`LINK_RESTART_BACKOFF` (GRAMMAR.md
/// §3.92): duración BASE del backoff exponencial de `run_serve_with_backoff`
/// ante un fallo recuperable de `serve` (bind de puerto ocupado, Postgres
/// caído al arrancar). Mismo formato que `--session-ttl`/`--http-timeout`
/// (`parse_duration`, granularidad de 1 segundo -- sin milisegundos, ver
/// GRAMMAR.md §3.92 "Límites honestos"). Sin el flag/env var: `None`, un
/// solo intento -- comportamiento IDÉNTICO al de siempre.
fn resolve_restart_backoff(args: &[String]) -> Result<Option<Duration>, String> {
    let raw = read_flag_or_env(args, "--restart-backoff", "LINK_RESTART_BACKOFF")?;
    let Some(raw) = raw else {
        return Ok(None);
    };
    parse_duration(&raw).map(Some)
}

/// `--service-api-key <clave>`/`LINK_SERVICE_API_KEY` (GRAMMAR.md §3.93):
/// secreto compartido que autentica al LLAMADOR (típicamente un gateway
/// servidor-a-servidor) antes de aceptar CUALQUIER request que no sea
/// `/health`/`/`/`/status` -- una capa distinta y ANTERIOR a `@requires`/JWT
/// (que autentican a un USUARIO final, no a quién está haciendo la llamada
/// de red). Sin el flag/env var: `None`, sin este chequeo -- comportamiento
/// IDÉNTICO al de siempre.
fn resolve_service_api_key(args: &[String]) -> Result<Option<String>, String> {
    // AUDIT-2026-08-27.md #13: `read_flag_or_env` ya filtra un valor de ENV
    // VAR vacío (así queda `None`, "sin esta capa"), pero un valor vacío que
    // llega por FLAG pasaba tal cual -- `--service-api-key ""` activaba la
    // capa entera con un secreto vacío (`constant_time_eq` contra `""` es
    // válido, así que un caller mandando un header `X-Service-Api-Key: `
    // vacío pasaría). No se toca `read_flag_or_env` en sí: otros flags
    // (`--host`, por ejemplo) SÍ tienen un contrato deliberado de "un valor
    // vacío es un error explícito", no "tratalo como ausente" -- el filtro
    // va acá, puntual, para este secreto.
    Ok(read_flag_or_env(args, "--service-api-key", "LINK_SERVICE_API_KEY")?.filter(|v| !v.trim().is_empty()))
}

/// `--hsts <valor>`/`LINK_HSTS` (GRAMMAR.md §3.143): el valor LITERAL del
/// header `Strict-Transport-Security` a mandar en toda respuesta -- mismo
/// criterio que `@cache_control("...")` (§3.113): texto crudo, sin parsear
/// ninguna gramática interna (`max-age=N`, `includeSubDomains`, `preload`
/// son responsabilidad de HTTP, no de c-script). Sin el flag/env var:
/// `None`, sin este header -- comportamiento IDÉNTICO al de siempre.
/// `linkc serve` nunca termina TLS por sí solo, así que esto es un opt-in
/// explícito para cuando el operador SABE que un proxy de confianza
/// termina TLS delante (mismo espíritu que `--trust-proxy`).
fn resolve_hsts(args: &[String]) -> Result<Option<String>, String> {
    read_flag_or_env(args, "--hsts", "LINK_HSTS")
}

/// GRAMMAR.md §3.92: UN proceso sirviendo TODOS los `.link` de un
/// directorio, cada uno en su propio hilo del sistema operativo y su propio
/// puerto (`--port-base N`, N+0/N+1/N+2/... en orden alfabético de nombre
/// de archivo -- determinístico, pero cambia si se agrega/renombra un
/// archivo, ver "Límites honestos"). El caso real que lo motiva: 13-17
/// procesos `pm2` separados (uno por `.link`), cada uno con su propio
/// puerto y su propio archivo SQLite -- 13-17 líneas de deploy script para
/// lo que podría ser una sola. `serve-all` colapsa el conteo de PROCESOS a
/// uno, sin tocar el aislamiento de datos: cada servicio sigue con su
/// propio archivo SQLite (`<archivo>.db` al lado del `.link`, el mismo
/// default que `linkc serve` sin `--db`) -- por eso `--db`/
/// `LINK_DATABASE_URL` compartido NO está soportado acá, ver más abajo.
///
/// Todos los `.link` se compilan ANTES de arrancar ningún hilo -- un
/// workspace a medio arrancar (12 de 13 servicios sanos, uno ni siquiera
/// compiló) es peor que no arrancar nada; un error de tipos en cualquiera
/// de los archivos aborta TODO el comando, con el mismo reporte de error de
/// siempre.
/// GRAMMAR.md §3.153: con `--port-registry <archivo.json>`, el puerto de
/// cada servicio se fija por NOMBRE de archivo (sin `.link`) en vez de por
/// posición alfabética -- agregar, quitar o renombrar OTRO `.link` en la
/// carpeta ya no corre el puerto de los que ya estaban asignados. El
/// archivo tiene la misma forma que `--port-map-out` (`{"nombre": puerto,
/// ...}`): si ya existe, se lee primero -- cada nombre ya presente ahí
/// conserva su puerto tal cual, sin importar el orden alfabético actual de
/// los archivos descubiertos. Un nombre nuevo recibe el próximo puerto
/// libre desde `--port-base`, saltando cualquiera ya usado por otro nombre
/// (incluidos los de servicios que ya no están, ver abajo).
///
/// Un nombre que ya NO tiene `.link` correspondiente (borrado o renombrado)
/// queda igual en el registro devuelto -- su puerto nunca se reasigna a
/// otro servicio en un arranque futuro, a propósito: un gateway externo
/// puede seguir teniendo ESE puerto hardcodeado apuntando a lo que ya no
/// existe, y reasignarlo en silencio a un servicio distinto sería el mismo
/// incidente de colisión que este flag existe para evitar, solo que al
/// revés. Limpiar una entrada obsoleta del archivo es una decisión manual
/// del operador, nunca automática acá.
fn resolve_stable_ports(link_files: &[PathBuf], port_base: u16, registry_path: &str) -> Result<(Vec<u16>, serde_json::Map<String, serde_json::Value>), String> {
    let mut registry: serde_json::Map<String, serde_json::Value> = if Path::new(registry_path).exists() {
        let text = fs::read_to_string(registry_path).map_err(|e| format!("no se pudo leer --port-registry '{registry_path}': {e}"))?;
        match serde_json::from_str(&text).map_err(|e| format!("--port-registry '{registry_path}' no es JSON válido: {e}"))? {
            serde_json::Value::Object(map) => map,
            _ => return Err(format!("--port-registry '{registry_path}' debe ser un objeto JSON {{\"nombre\": puerto, ...}}")),
        }
    } else {
        serde_json::Map::new()
    };

    let mut taken: std::collections::HashSet<u16> = registry.values().filter_map(|v| v.as_u64()).filter_map(|p| u16::try_from(p).ok()).collect();

    let mut ports = Vec::with_capacity(link_files.len());
    for path in link_files {
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string();
        let existing = registry.get(&name).and_then(|v| v.as_u64()).and_then(|p| u16::try_from(p).ok());
        let port = match existing {
            Some(p) => p,
            None => {
                let mut candidate = port_base;
                while taken.contains(&candidate) {
                    candidate = candidate
                        .checked_add(1)
                        .ok_or_else(|| format!("--port-registry '{registry_path}': no quedan puertos libres desde --port-base {port_base} para el nuevo servicio '{name}'"))?;
                }
                taken.insert(candidate);
                registry.insert(name.clone(), serde_json::json!(candidate));
                candidate
            }
        };
        ports.push(port);
    }
    Ok((ports, registry))
}

fn cmd_serve_all(args: &[String]) -> ExitCode {
    let Some(dir) = args.first() else {
        eprintln!(
            "uso: linkc serve-all <directorio> --port-base <N> [--port-map-out <archivo.json>] [--port-registry <archivo.json>] [--host <dirección>] [--cors-origin <origen>] [--session-ttl <duración>] [--argon2-memory-kib <N>] [--argon2-iterations <N>] [--encryption-key <clave-base64>] [--jwt-secret <secreto>] [--jwt-role-claim <nombre>] [--jwt-user-id-claim <nombre>] [--max-body-bytes <N>] [--http-timeout <duración>] [--trust-proxy] [--adopt-existing] [--restart-backoff <duración>] [--service-api-key <clave>] [--service-api-key-exempt <nombre1,nombre2,...>] [--log-format text|json] [--log-level debug|info|warn|error] [--hsts <valor>]"
        );
        return ExitCode::FAILURE;
    };

    // `--db`/`LINK_DATABASE_URL` compartido entre servicios de distinto
    // schema es exactamente el escenario de colisión de nombre de tabla que
    // motivó §3.93 (detección de colisión) -- sin esa red de seguridad
    // todavía, rechazarlo acá de entrada es más honesto que aceptarlo y
    // arriesgar una tabla de un servicio pisando la de otro en silencio.
    if args.iter().any(|a| a == "--db") || std::env::var("LINK_DATABASE_URL").ok().filter(|v| !v.trim().is_empty()).is_some() {
        eprintln!(
            "linkc serve-all no soporta --db/LINK_DATABASE_URL compartido entre servicios -- cada .link usa su propio \
             archivo SQLite ('<archivo>.db' al lado del .link, igual que 'linkc serve' sin --db). Apuntar varios \
             servicios a la MISMA base todavía no está soportado (--db-schema/--db-prefix, sin implementar)."
        );
        return ExitCode::FAILURE;
    }

    let dir_path = Path::new(dir);
    if !dir_path.is_dir() {
        eprintln!("'{dir}' no es un directorio");
        return ExitCode::FAILURE;
    }

    let port_base: u16 = match extract_flag_value(args, "--port-base") {
        Ok(Some(v)) => match v.parse() {
            Ok(n) => n,
            Err(_) => {
                eprintln!("--port-base: '{v}' no es un puerto válido (0-65535)");
                return ExitCode::FAILURE;
            }
        },
        Ok(None) => {
            eprintln!("uso: linkc serve-all <directorio> --port-base <N> (falta --port-base)");
            return ExitCode::FAILURE;
        }
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };

    let port_map_out = match extract_flag_value(args, "--port-map-out") {
        Ok(v) => v,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };

    let port_registry = match extract_flag_value(args, "--port-registry") {
        Ok(v) => v,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };

    let mut link_files: Vec<PathBuf> = match fs::read_dir(dir_path) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("link"))
            .collect(),
        Err(e) => {
            eprintln!("no se pudo leer '{dir}': {e}");
            return ExitCode::FAILURE;
        }
    };
    // Orden alfabético: determinístico entre corridas (mismo directorio,
    // mismos archivos -> misma asignación de puerto), pero NO estable ante
    // agregar/quitar/renombrar un archivo -- ver "Límites honestos" en
    // GRAMMAR.md §3.92. Se imprime la asignación exacta más abajo para que
    // quede documentado en cada arranque, no solo en la documentación.
    link_files.sort();
    if link_files.is_empty() {
        eprintln!("'{dir}' no tiene ningún archivo .link");
        return ExitCode::FAILURE;
    }

    let host = match resolve_host(args) {
        Ok(h) => h,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };
    let cors = match resolve_cors_origins(args) {
        Ok(origins) => match origins {
            Some(list) => runtime::server::CorsConfig::Allowlist(list),
            None => runtime::server::CorsConfig::Any,
        },
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };
    let session_ttl = match resolve_session_ttl(args) {
        Ok(ttl) => ttl,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };
    let argon2_params = match resolve_argon2_params(args) {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };
    let encryption_key = match read_flag_or_env(args, "--encryption-key", "LINK_ENCRYPTION_KEY") {
        Ok(k) => k,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };
    let jwt_config = match resolve_jwt_config(args) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };
    let adopt_existing = resolve_adopt_existing(args);
    let max_body_bytes = match resolve_max_body_bytes(args) {
        Ok(n) => n,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };
    let http_timeout = match resolve_http_timeout(args) {
        Ok(d) => d,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };
    let trust_proxy = resolve_trust_proxy(args);
    let restart_backoff = match resolve_restart_backoff(args) {
        Ok(b) => b,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };
    let service_api_key = match resolve_service_api_key(args) {
        Ok(k) => k,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };
    let service_api_key_exempt: std::collections::HashSet<String> = match extract_flag_value(args, "--service-api-key-exempt") {
        Ok(Some(v)) => v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        Ok(None) => std::collections::HashSet::new(),
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };
    if !service_api_key_exempt.is_empty() && service_api_key.is_none() {
        eprintln!("--service-api-key-exempt no tiene sentido sin --service-api-key/LINK_SERVICE_API_KEY -- no hay ningún chequeo del que eximir a nadie");
        return ExitCode::FAILURE;
    }
    // Un nombre en --service-api-key-exempt que no corresponde a NINGÚN
    // .link descubierto es casi seguro un typo -- silenciarlo dejaría a
    // quien lo escribió creyendo que ese servicio quedó exento cuando en
    // realidad sigue protegido (o, al revés, ningún nombre real quedó
    // exento sin que nadie lo note). Falla acá, antes de arrancar nada,
    // nombrando exactamente qué nombres no matchean.
    let known_names: std::collections::HashSet<String> =
        link_files.iter().filter_map(|p| p.file_stem()).filter_map(|s| s.to_str()).map(|s| s.to_string()).collect();
    let unknown_exempt: Vec<&String> = service_api_key_exempt.iter().filter(|name| !known_names.contains(*name)).collect();
    if !unknown_exempt.is_empty() {
        let mut unknown_sorted: Vec<&str> = unknown_exempt.iter().map(|s| s.as_str()).collect();
        unknown_sorted.sort();
        let mut known_sorted: Vec<&str> = known_names.iter().map(|s| s.as_str()).collect();
        known_sorted.sort();
        eprintln!(
            "--service-api-key-exempt nombra un servicio que no existe en '{dir}': [{}]. Servicios reales encontrados: [{}].",
            unknown_sorted.join(", "),
            known_sorted.join(", "),
        );
        return ExitCode::FAILURE;
    }
    let log_format = match resolve_log_format(args) {
        Ok(f) => f,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };
    let log_level = match resolve_log_level(args) {
        Ok(l) => l,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };
    let log = runtime::server::LogConfig { format: log_format, level: log_level };
    let hsts = match resolve_hsts(args) {
        Ok(h) => h,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };

    let (ports, updated_registry): (Vec<u16>, Option<serde_json::Map<String, serde_json::Value>>) = match &port_registry {
        Some(path) => match resolve_stable_ports(&link_files, port_base, path) {
            Ok((ports, registry)) => (ports, Some(registry)),
            Err(msg) => {
                eprintln!("{msg}");
                return ExitCode::FAILURE;
            }
        },
        None => {
            let mut ports = Vec::with_capacity(link_files.len());
            for i in 0..link_files.len() {
                let Some(port) = port_base.checked_add(i as u16) else {
                    eprintln!("--port-base {port_base}: no alcanzan los puertos para {} archivos .link (se pasaría de 65535)", link_files.len());
                    return ExitCode::FAILURE;
                };
                ports.push(port);
            }
            (ports, None)
        }
    };

    let mut services: Vec<(PathBuf, u16, Program)> = Vec::with_capacity(link_files.len());
    for (path, port) in link_files.iter().zip(ports.iter()) {
        let path_str = path.to_string_lossy().to_string();
        let program = match load_and_check(&path_str) {
            Ok(p) => p,
            Err(code) => return code,
        };
        services.push((path.clone(), *port, program));
    }

    println!("linkc serve-all: {} servicio(s) en un proceso (datos en SQLite separado por servicio)", services.len());
    for (path, port, _) in &services {
        println!("  {:<40} -> http://localhost:{port}", path.display());
    }
    if !service_api_key_exempt.is_empty() {
        let mut exempt_sorted: Vec<&str> = service_api_key_exempt.iter().map(|s| s.as_str()).collect();
        exempt_sorted.sort();
        println!("  exentos de --service-api-key: {}", exempt_sorted.join(", "));
    }

    // GRAMMAR.md §3.107: la asignación real (orden alfabético de los
    // `.link` descubiertos, ver el comentario más arriba) queda escrita acá
    // ANTES de arrancar cualquier servicio -- un gateway/proxy externo (el
    // caso real: IgnisLove hardcodeaba a mano un mapa nombre→puerto que
    // tenía que actualizarse cada vez que se agregaba/quitaba/renombraba un
    // `.link`) puede leer este archivo en vez de replicar la regla de
    // asignación por su cuenta. Clave = nombre de archivo SIN `.link` (el
    // nombre que un router externo usaría para identificar el servicio),
    // valor = puerto. Escribirlo es lo último antes de servir: si falla,
    // mejor no arrancar nada a que el gateway arranque leyendo un mapeo
    // viejo o inexistente.
    if let Some(out_path) = &port_map_out {
        let mut map = serde_json::Map::new();
        for (path, port, _) in &services {
            let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
            map.insert(name.to_string(), serde_json::json!(port));
        }
        let json = serde_json::to_string_pretty(&map).expect("serializar el mapeo de puertos no puede fallar");
        if let Err(e) = fs::write(out_path, json) {
            eprintln!("no se pudo escribir --port-map-out en '{out_path}': {e}");
            return ExitCode::FAILURE;
        }
        println!("mapeo de puertos escrito en {out_path}");
    }

    // GRAMMAR.md §3.153: a diferencia de `--port-map-out` (arriba, siempre
    // sobreescribe con la asignación de ESTA corrida), `--port-registry` ya
    // leyó el archivo existente dentro de `resolve_stable_ports` -- lo que
    // se escribe acá es esa MISMA estructura, con los nombres nuevos ya
    // insertados y las entradas de servicios que ya no están (borrados o
    // renombrados) todavía presentes, apuntando a su puerto de siempre.
    if let Some(registry) = &updated_registry {
        let out_path = port_registry.as_deref().expect("updated_registry solo es Some junto con port_registry");
        let json = serde_json::to_string_pretty(registry).expect("serializar el registro de puertos no puede fallar");
        if let Err(e) = fs::write(out_path, json) {
            eprintln!("no se pudo escribir --port-registry en '{out_path}': {e}");
            return ExitCode::FAILURE;
        }
        println!("registro de puertos actualizado en {out_path}");
    }

    let handles: Vec<std::thread::JoinHandle<bool>> = services
        .into_iter()
        .map(|(path, port, program)| {
            let host = host.clone();
            let cors = cors.clone();
            let jwt_config = jwt_config.clone();
            let argon2_params = argon2_params.clone();
            let encryption_key = encryption_key.clone();
            // GRAMMAR.md §3.93/§3.153: `--service-api-key` es un flag GLOBAL
            // a la corrida entera de `serve-all` -- pero el chequeo en sí se
            // aplica POR HILO (cada servicio corre su propio
            // `runtime::server::serve`), así que un nombre presente en
            // `--service-api-key-exempt` puede pasar `None` acá mismo, sin
            // tocar el resto de los servicios que sí lo exigen.
            let is_exempt = path.file_stem().and_then(|s| s.to_str()).is_some_and(|name| service_api_key_exempt.contains(name));
            let service_api_key = if is_exempt { None } else { service_api_key.clone() };
            let hsts = hsts.clone();
            let label = path.to_string_lossy().to_string();
            std::thread::spawn(move || {
                let source = runtime::server::DbSource::SqliteFile(path.with_extension("db"));
                let attempt = || {
                    runtime::server::serve(
                        &program,
                        &host,
                        port,
                        source.clone(),
                        cors.clone(),
                        session_ttl,
                        argon2_params.clone(),
                        encryption_key.clone(),
                        jwt_config.clone(),
                        adopt_existing,
                        max_body_bytes,
                        http_timeout,
                        trust_proxy,
                        service_api_key.clone(),
                        log,
                        hsts.clone(),
                    )
                };
                match run_serve_with_backoff(attempt, restart_backoff, &label) {
                    Ok(()) => true,
                    Err(msg) => {
                        eprintln!("[{label}] servicio caído, no se reintenta más (los demás servicios siguen corriendo): {msg}");
                        false
                    }
                }
            })
        })
        .collect();

    // Join secuencial: cualquier hilo sano bloquea acá para siempre (el
    // caso normal), así que el proceso sigue vivo mientras QUEDE al menos
    // uno andando -- el código de salida solo importa en el caso raro (o de
    // test) en que TODOS terminan.
    let mut all_ok = true;
    for h in handles {
        match h.join() {
            Ok(ok) => all_ok &= ok,
            Err(_) => all_ok = false,
        }
    }
    if all_ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// En qué dirección escucha el servidor (GRAMMAR.md §3.81), mismo orden de
/// precedencia que el resto de los `resolve_*` de este archivo:
///
/// 1. `--host <dirección>` en la línea de comandos.
/// 2. La variable de entorno `LINK_HOST`.
/// 3. Ninguno de los dos: `"0.0.0.0"`, el comportamiento de siempre --
///    escucha en todas las interfaces, sin romper a nadie que no pida esto
///    explícitamente.
///
/// Solo valida que no venga vacío (`--host ""`) -- cualquier otra forma
/// inválida (una IP mal armada, un hostname que no resuelve) la rechaza
/// `tiny_http::Server::http` al bindear, con un mensaje que ya incluye el
/// valor exacto que se le pasó (ver `runtime::server::serve`).
fn resolve_host(args: &[String]) -> Result<String, String> {
    let host = read_flag_or_env(args, "--host", "LINK_HOST")?.unwrap_or_else(|| "0.0.0.0".to_string());
    if host.trim().is_empty() {
        return Err("uso: --host <dirección> (no puede ser vacío, ej. '127.0.0.1')".to_string());
    }
    Ok(host)
}

/// Default de `--max-body-bytes`/`LINK_MAX_BODY_BYTES` (GRAMMAR.md §3.85):
/// 10 MiB -- generoso para un body JSON real (incluido uno con algún campo
/// `String` grande en base64), acotado para no dejar el proceso entero
/// expuesto a agotamiento de memoria por un solo body sin límite. Número
/// razonable, no exhaustivamente investigado -- mismo criterio que
/// `LIVE_STREAM_BUFFER` en `runtime/db.rs`.
const DEFAULT_MAX_BODY_BYTES: u64 = 10 * 1024 * 1024;

/// Cuántos bytes de BODY acepta como máximo cualquier request a
/// `linkc serve` (GRAMMAR.md §3.85), mismo orden de precedencia que el resto
/// de los `resolve_*` de este archivo:
///
/// 1. `--max-body-bytes <N>` en la línea de comandos (bytes, un entero
///    plano -- sin sufijos de unidad, mismo criterio que
///    `--argon2-memory-kib`).
/// 2. La variable de entorno `LINK_MAX_BODY_BYTES`.
/// 3. Ninguno de los dos: `DEFAULT_MAX_BODY_BYTES`.
fn resolve_max_body_bytes(args: &[String]) -> Result<u64, String> {
    let raw = read_flag_or_env(args, "--max-body-bytes", "LINK_MAX_BODY_BYTES")?;
    let Some(raw) = raw else {
        return Ok(DEFAULT_MAX_BODY_BYTES);
    };
    raw.parse::<u64>().map_err(|_| format!("--max-body-bytes/LINK_MAX_BODY_BYTES: '{raw}' no es un entero positivo (bytes)"))
}

/// Cuánto puede tardar cualquier llamada saliente (`http.get`/`post`/
/// `getWithHeaders`/etc., GRAMMAR.md §3.86) antes de abortar con un error de
/// runtime, mismo orden de precedencia que el resto de los `resolve_*`:
///
/// 1. `--http-timeout <duración>` en la línea de comandos -- mismo formato
///    `Ns`/`Nm`/`Nh`/`Nd` que `--session-ttl` (`parse_duration`, arriba).
/// 2. La variable de entorno `LINK_HTTP_TIMEOUT`.
/// 3. Ninguno de los dos: 30 segundos -- el mismo número que `ureq` (la
///    crate) ya usa como timeout de CONEXIÓN por default; lo que faltaba
///    era el de lectura/escritura, que por default es "nunca" (sin esto,
///    una request saliente a un servidor colgado bloqueaba para siempre --
///    antes de GRAMMAR.md §3.158/v1.114.0 eso colgaba el proceso entero de
///    un solo hilo; hoy solo cuelga el hilo de ESA request, salvo dentro de
///    un `transaction{}`, donde sigue bloqueando a las demás porque
///    sostiene el candado de la conexión).
fn resolve_http_timeout(args: &[String]) -> Result<std::time::Duration, String> {
    let raw = read_flag_or_env(args, "--http-timeout", "LINK_HTTP_TIMEOUT")?;
    let Some(raw) = raw else {
        return Ok(std::time::Duration::from_secs(30));
    };
    parse_duration(&raw)
}

/// Adopción de tabla existente (`--adopt-existing`/`LINK_ADOPT_EXISTING`,
/// GRAMMAR.md §3.67): un flag booleano, no un valor -- por eso no reusa
/// `read_flag_or_env` (pensado para `--flag <valor>`). Presente en la línea
/// de comandos (sin importar qué venga después, si algo viene) o la env var
/// puesta a cualquier valor no vacío: `true`. Ninguno de los dos: `false`,
/// el comportamiento de siempre (`linkc serve` crea/migra tablas).
fn resolve_adopt_existing(args: &[String]) -> bool {
    args.iter().any(|a| a == "--adopt-existing") || std::env::var("LINK_ADOPT_EXISTING").ok().filter(|v| !v.trim().is_empty()).is_some()
}

/// `--trust-proxy`/`LINK_TRUST_PROXY` (GRAMMAR.md §3.89): igual que
/// `--adopt-existing`, un flag booleano de presencia, no un valor.
/// `false` por default -- `@rate_limit` sigue identificando al cliente por
/// `remote_addr()` (la conexión TCP real) a menos que se pida
/// explícitamente confiar en `X-Forwarded-For`, un header que cualquier
/// cliente directo puede mandar con el valor que quiera.
fn resolve_trust_proxy(args: &[String]) -> bool {
    args.iter().any(|a| a == "--trust-proxy") || std::env::var("LINK_TRUST_PROXY").ok().filter(|v| !v.trim().is_empty()).is_some()
}

/// `--log-format`/`LINK_LOG_FORMAT` (GRAMMAR.md §3.122): `text` (default,
/// el comportamiento de siempre) o `json` -- cualquier otro valor es un
/// error claro en vez de caer en silencio al default.
fn resolve_log_format(args: &[String]) -> Result<runtime::server::LogFormat, String> {
    let raw = read_flag_or_env(args, "--log-format", "LINK_LOG_FORMAT")?;
    match raw.as_deref() {
        None | Some("text") => Ok(runtime::server::LogFormat::Text),
        Some("json") => Ok(runtime::server::LogFormat::Json),
        Some(other) => Err(format!("--log-format/LINK_LOG_FORMAT: '{other}' inválido (se esperaba 'text' o 'json')")),
    }
}

/// `--log-level`/`LINK_LOG_LEVEL` (GRAMMAR.md §3.122): `info` (default, el
/// comportamiento de siempre -- las dos líneas por request, recibida y
/// completada, se siguen imprimiendo SIEMPRE) o `warn`/`error` para reducir
/// el volumen en producción con tráfico real, mostrando solo las requests
/// que terminaron en 4xx/5xx respectivamente. `debug` acá es un sinónimo de
/// `info` -- no hay todavía ninguna línea de nivel `Debug` propio, existe
/// para que la jerarquía completa sea un valor válido desde el principio.
fn resolve_log_level(args: &[String]) -> Result<runtime::server::LogLevel, String> {
    use runtime::server::LogLevel;
    let raw = read_flag_or_env(args, "--log-level", "LINK_LOG_LEVEL")?;
    match raw.as_deref() {
        None | Some("info") | Some("debug") => Ok(LogLevel::Info),
        Some("warn") => Ok(LogLevel::Warn),
        Some("error") => Ok(LogLevel::Error),
        Some(other) => Err(format!("--log-level/LINK_LOG_LEVEL: '{other}' inválido (se esperaba 'debug', 'info', 'warn' o 'error')")),
    }
}

/// Cuánto vive una sesión antes de expirar sola (GRAMMAR.md §3.50), en orden
/// de precedencia (mismo criterio que `resolve_db_source`/
/// `resolve_cors_origins`, arriba):
///
/// 1. `--session-ttl <duración>` en la línea de comandos.
/// 2. La variable de entorno `LINK_SESSION_TTL`.
/// 3. Ninguno de los dos: `None`, que sigue significando "nunca expira sola"
///    -- el comportamiento de siempre (vive hasta `destroySession()` o
///    reiniciar el proceso), sin romper a nadie que no pida esto
///    explícitamente.
fn resolve_session_ttl(args: &[String]) -> Result<Option<std::time::Duration>, String> {
    let flag = match args.iter().position(|a| a == "--session-ttl") {
        Some(i) => match args.get(i + 1) {
            Some(v) => Some(v.clone()),
            None => return Err("uso: --session-ttl <duración> (falta el valor, ej. '7d')".to_string()),
        },
        None => None,
    };
    let value = flag.or_else(|| std::env::var("LINK_SESSION_TTL").ok().filter(|v| !v.trim().is_empty()));
    let Some(value) = value else {
        return Ok(None);
    };
    parse_duration(&value).map(Some)
}

/// Costo de `crypto.hashPassword` (GRAMMAR.md §3.55): `--argon2-memory-kib`/
/// `LINK_ARGON2_MEMORY_KIB` y `--argon2-iterations`/`LINK_ARGON2_ITERATIONS`,
/// mismo orden de precedencia que `resolve_session_ttl`. Ninguno de los dos
/// puesto: el default de la crate (`Params::default()`, ~19 MiB / 2
/// iteraciones), el comportamiento de siempre. La paralelización (`p_cost`)
/// queda deliberadamente fuera de esta ronda -- el intérprete es de un solo
/// hilo, así que no hay ningún hilo extra que se beneficie de subirla.
fn resolve_argon2_params(args: &[String]) -> Result<argon2::Params, String> {
    let read = |flag: &str, env: &str| -> Result<Option<u32>, String> {
        let from_flag = match args.iter().position(|a| a == flag) {
            Some(i) => match args.get(i + 1) {
                Some(v) => Some(v.clone()),
                None => return Err(format!("uso: {flag} <número> (falta el valor)")),
            },
            None => None,
        };
        let raw = from_flag.or_else(|| std::env::var(env).ok().filter(|v| !v.trim().is_empty()));
        match raw {
            Some(v) => v.parse::<u32>().map(Some).map_err(|_| format!("{flag}/{env}: '{v}' no es un entero positivo")),
            None => Ok(None),
        }
    };
    let memory_kib = read("--argon2-memory-kib", "LINK_ARGON2_MEMORY_KIB")?.unwrap_or(argon2::Params::DEFAULT_M_COST);
    let iterations = read("--argon2-iterations", "LINK_ARGON2_ITERATIONS")?.unwrap_or(argon2::Params::DEFAULT_T_COST);
    argon2::Params::new(memory_kib, iterations, argon2::Params::DEFAULT_P_COST, None)
        .map_err(|e| format!("parámetros de Argon2id inválidos (memoria={memory_kib}KiB, iteraciones={iterations}): {e}"))
}

/// `--<flag> <valor>` en CUALQUIER posición de `args` (no solo el principio
/// -- a diferencia del parseo posicional de `cmd_build`/`--diff`, acá no
/// hace falta separar "lo que queda" porque el resto de `cmd_test` ya sabe
/// ignorar cualquier token que empiece con `--`), sin variable de entorno
/// asociada -- a diferencia de `read_flag_or_env`, que SIEMPRE la exige.
fn extract_flag_value(args: &[String], flag: &str) -> Result<Option<String>, String> {
    match args.iter().position(|a| a == flag) {
        Some(i) => match args.get(i + 1) {
            Some(v) => Ok(Some(v.clone())),
            None => Err(format!("uso: {flag} <valor> (falta el valor)")),
        },
        None => Ok(None),
    }
}

/// `--<flag> <valor>` si está, si no la variable de entorno `env`, si no
/// `None` -- mismo orden de precedencia que el resto de `resolve_*` de este
/// archivo.
fn read_flag_or_env(args: &[String], flag: &str, env: &str) -> Result<Option<String>, String> {
    let from_flag = match args.iter().position(|a| a == flag) {
        Some(i) => match args.get(i + 1) {
            Some(v) => Some(v.clone()),
            None => return Err(format!("uso: {flag} <valor> (falta el valor)")),
        },
        None => None,
    };
    Ok(from_flag.or_else(|| std::env::var(env).ok().filter(|v| !v.trim().is_empty())))
}

/// Auth externo (GRAMMAR.md §3.64): verificar JWTs HS256 emitidos por un
/// backend ya existente, además de -- nunca en vez de -- las sesiones
/// propias de este lenguaje. `--jwt-secret`/`LINK_JWT_SECRET` es el único
/// flag que de verdad importa: sin él, `None` entero -- el comportamiento es
/// IDÉNTICO al de antes de esta ronda, cero JWT se intenta verificar nunca.
/// `--jwt-role-claim`/`--jwt-user-id-claim` (o sus env vars) solo tienen
/// sentido si `--jwt-secret` está, y tienen default (`"role"`/`"sub"`, este
/// último por convención de OIDC) para el caso común.
fn resolve_jwt_config(args: &[String]) -> Result<Option<(String, String, String)>, String> {
    // AUDIT-2026-08-27.md #13: mismo motivo que `resolve_service_api_key` --
    // `--jwt-secret ""` (flag con valor vacío explícito) activaba la
    // verificación de JWT con un secreto vacío (`Hmac::<Sha256>::new_from_
    // slice(b"")` es una clave válida, aunque degenerada), cuando la
    // intención de "flag vacío" es casi seguro "no configuré nada" -- mismo
    // criterio que ya aplica del lado de la env var.
    let Some(secret) = read_flag_or_env(args, "--jwt-secret", "LINK_JWT_SECRET")?.filter(|v| !v.trim().is_empty()) else {
        return Ok(None);
    };
    let role_claim = read_flag_or_env(args, "--jwt-role-claim", "LINK_JWT_ROLE_CLAIM")?.unwrap_or_else(|| "role".to_string());
    let user_id_claim = read_flag_or_env(args, "--jwt-user-id-claim", "LINK_JWT_USER_ID_CLAIM")?.unwrap_or_else(|| "sub".to_string());
    Ok(Some((secret, role_claim, user_id_claim)))
}

/// "Ns"/"Nm"/"Nh"/"Nd" (segundos/minutos/horas/días) -- mismo espíritu que
/// `RateLimitSpec::parse` (`rate_limit.rs`, "N/Nm") pero CON días: la escala
/// típica de una sesión (horas a semanas) los necesita de verdad, a
/// diferencia de una ventana de rate limit, donde "N por día" es un caso
/// raro y por eso ese parser no los tiene.
fn parse_duration(raw: &str) -> Result<std::time::Duration, String> {
    let invalid = || format!("duración inválida: '{raw}' (se esperaba 'Ns', 'Nm', 'Nh' o 'Nd', ej. '7d' -- 7 días)");
    if raw.is_empty() {
        return Err(invalid());
    }
    let (num_str, unit) = raw.split_at(raw.len() - 1);
    let num: u64 = num_str.parse().map_err(|_| invalid())?;
    if num == 0 {
        return Err(invalid());
    }
    match unit {
        "s" => Ok(std::time::Duration::from_secs(num)),
        "m" => Ok(std::time::Duration::from_secs(num * 60)),
        "h" => Ok(std::time::Duration::from_secs(num * 3600)),
        "d" => Ok(std::time::Duration::from_secs(num * 86400)),
        _ => Err(invalid()),
    }
}

/// Qué orígenes puede pedir CORS, en orden de precedencia (mismo criterio
/// que `resolve_db_source`, arriba):
///
/// 1. `--cors-origin <origen>`, repetible -- una vez por origen permitido.
/// 2. La variable de entorno `LINK_CORS_ORIGINS`, orígenes separados por
///    coma (para un contenedor, donde no siempre se controla el comando).
/// 3. Ninguno de los dos: `None`, que sigue significando "cualquier
///    origen" -- el comportamiento de siempre, sin romper a nadie que no
///    pida esto explícitamente (GRAMMAR.md §3.41).
fn resolve_cors_origins(args: &[String]) -> Result<Option<Vec<String>>, String> {
    let mut from_flags: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--cors-origin" {
            match args.get(i + 1) {
                Some(v) => from_flags.push(v.clone()),
                None => return Err("uso: --cors-origin <origen> (falta el valor)".to_string()),
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    if !from_flags.is_empty() {
        return Ok(Some(from_flags));
    }
    Ok(std::env::var("LINK_CORS_ORIGINS")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()))
}

/// De dónde saca los datos `linkc serve`, en orden de precedencia:
///
/// 1. `--db <url|archivo>` en la línea de comandos.
/// 2. La variable de entorno `LINK_DATABASE_URL` (la que usa un contenedor,
///    donde no siempre se controla el comando).
/// 3. El default de siempre: `myapp.link` -> `myapp.db` al lado del fuente
///    (GRAMMAR.md §3.17).
///
/// Un valor que empieza con `postgres://` o `postgresql://` es PostgreSQL;
/// cualquier otro es la ruta de un archivo SQLite.
fn resolve_db_source(path: &str, args: &[String]) -> Result<runtime::server::DbSource, String> {
    let flag = match args.iter().position(|a| a == "--db") {
        Some(i) => match args.get(i + 1) {
            Some(v) => Some(v.clone()),
            None => return Err("uso: --db <url|archivo> (falta el valor)".to_string()),
        },
        None => None,
    };
    let value = flag.or_else(|| std::env::var("LINK_DATABASE_URL").ok().filter(|v| !v.trim().is_empty()));

    let Some(value) = value else {
        return Ok(runtime::server::DbSource::SqliteFile(Path::new(path).with_extension("db")));
    };
    if value.starts_with("postgres://") || value.starts_with("postgresql://") {
        Ok(runtime::server::DbSource::Postgres(value))
    } else {
        Ok(runtime::server::DbSource::SqliteFile(PathBuf::from(value)))
    }
}
