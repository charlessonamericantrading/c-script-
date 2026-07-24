mod ast;
mod checker;
mod codegen;
mod lexer;
mod parser;
mod runtime;
mod scaffold;
mod token;
mod types;

use ast::Program;
use std::env;
use std::fs;
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("build") => cmd_build(&args[2..]),
        Some("serve") => cmd_serve(&args[2..]),
        Some("new") => cmd_new(&args[2..]),
        Some(path) => cmd_check(path), // `linkc <archivo.link>` -- solo lex+parse+check
        None => {
            eprintln!("uso: linkc <archivo.link>                  (lexea, parsea y tipa)");
            eprintln!("     linkc new <nombre>                     (crea un proyecto nuevo)");
            eprintln!("     linkc build <archivo.link> <outdir>    (+ emite contract.d.ts y client.ts)");
            eprintln!("     linkc serve <archivo.link> <puerto>    (+ sirve los rpc por HTTP)");
            ExitCode::FAILURE
        }
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
    let source = fs::read_to_string(path).map_err(|e| {
        eprintln!("no se pudo leer {path}: {e}");
        ExitCode::FAILURE
    })?;

    let tokens = lexer::tokenize(&source).map_err(|e| {
        eprintln!("{e}");
        ExitCode::FAILURE
    })?;

    let program = parser::parse(tokens).map_err(|e| {
        eprintln!("{e}");
        ExitCode::FAILURE
    })?;

    checker::Checker::check_program(&program).map_err(|errors| {
        for e in &errors {
            eprintln!("{e}");
        }
        ExitCode::FAILURE
    })?;

    Ok(program)
}

fn cmd_check(path: &str) -> ExitCode {
    // `linkc <algo>` cae acá para cualquier <algo> que no sea "build"/"serve"/
    // "new" -- si además no parece un archivo real, es casi seguro un
    // subcomando mal escrito, no un archivo que el usuario quiere tipar.
    if !path.ends_with(".link") && !Path::new(path).exists() {
        eprintln!("'{path}' no es un subcomando conocido (build, serve, new) ni un archivo .link existente");
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

fn cmd_build(args: &[String]) -> ExitCode {
    let (Some(path), Some(outdir)) = (args.first(), args.get(1)) else {
        eprintln!("uso: linkc build <archivo.link> <outdir>");
        return ExitCode::FAILURE;
    };

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

    if let Err(e) = fs::create_dir_all(outdir) {
        eprintln!("no se pudo crear {outdir}: {e}");
        return ExitCode::FAILURE;
    }
    let contract_path = format!("{outdir}/contract.d.ts");
    let client_path = format!("{outdir}/client.ts");
    if let Err(e) = fs::write(&contract_path, contract) {
        eprintln!("no se pudo escribir {contract_path}: {e}");
        return ExitCode::FAILURE;
    }
    if let Err(e) = fs::write(&client_path, client) {
        eprintln!("no se pudo escribir {client_path}: {e}");
        return ExitCode::FAILURE;
    }

    println!("OK: generado {contract_path} y {client_path}");
    ExitCode::SUCCESS
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

    runtime::server::serve(program, port);
    ExitCode::SUCCESS
}
