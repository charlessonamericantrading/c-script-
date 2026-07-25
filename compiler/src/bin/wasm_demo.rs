// v0 de "compilar a WASM" (GRAMMAR.md/PLAN.md §2.4): en vez de un codegen
// nuevo desde cero, se recompila el intérprete YA EXISTENTE para
// `wasm32-wasip1` y se le da un programa c-script (embebido acá como texto,
// vía include_str!) para ejecutar -- el mismo camino que siguieron
// RustPython y Boa para su primera versión real en wasm.
//
// No-objetivo explícito: sin networking, sin servidor. `invoke_rpc` no
// depende de sockets para nada, así que probar "una llamada RPC real
// corre dentro de un sandbox wasm" es mucho más chico de lo que parece --
// no hace falta ningún adaptador WASI-HTTP para esto (eso, si hace falta
// algún día, es un trabajo aparte: tiny_http asume sockets de SO reales y
// no corre tal cual dentro de ningún sandbox wasm).
//
// Build + verificación:
//   rustup target add wasm32-wasip1
//   cargo install wasmtime-cli
//   cargo build --target wasm32-wasip1 --bin wasm_demo
//   cargo run --target wasm32-wasip1 --bin wasm_demo   (usa el runner de .cargo/config.toml)

use linkc::checker::Checker;
use linkc::lexer::tokenize;
use linkc::parser::parse;
use linkc::runtime::db::Db;
use linkc::runtime::invoke_rpc;

const SOURCE: &str = include_str!("../../../examples/users.link");

fn main() {
    let tokens = tokenize(SOURCE).unwrap_or_else(|e| panic!("{e}"));
    let program = parse(tokens).unwrap_or_else(|errors| {
        for e in &errors {
            eprintln!("{e}");
        }
        std::process::exit(1);
    });
    if let Err(errors) = Checker::check_program(&program) {
        for e in &errors {
            eprintln!("{e}");
        }
        std::process::exit(1);
    }

    let db = Db::seeded();
    let args = serde_json::json!({ "id": 1 });
    match invoke_rpc(&program, "Users", "getById", &args, &db) {
        Ok(result) => println!("{result}"),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}
