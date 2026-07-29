// El compilador como librería -- lo que `main.rs` (el binario `linkc`) y
// `src/bin/wasm_demo.rs` (v0 de compilación a WASM, GRAMMAR.md/PLAN.md §2.4)
// consumen por igual. Un `src/bin/*.rs` compila como un crate genuinamente
// aparte aunque viva en el mismo paquete Cargo -- sin este `[lib]`, no
// tendría de dónde importar `ast`/`checker`/`runtime`/etc.

pub mod ast;
pub mod checker;
pub mod codegen;
pub mod diagnostics;
pub mod lexer;
pub mod lockfile;
pub mod modules;
pub mod lsp;
pub mod parser;
pub mod runtime;
pub mod scaffold;
pub mod token;
pub mod types;
