// El compilador como librería -- lo que `main.rs` (el binario `linkc`) y
// `src/bin/wasm_demo.rs` (v0 de compilación a WASM, GRAMMAR.md/PLAN.md §2.4)
// consumen por igual. Un `src/bin/*.rs` compila como un crate genuinamente
// aparte aunque viva en el mismo paquete Cargo -- sin este `[lib]`, no
// tendría de dónde importar `ast`/`checker`/`runtime`/etc.

pub mod ast;
pub mod checker;
pub mod codegen;
pub mod diagnostics;
pub mod gitdep;
pub mod lexer;
pub mod lockfile;
pub mod modules;
pub mod fmt;
pub mod lint;
pub mod doc;
pub mod docker;
pub mod lsp;
pub mod parser;
pub mod rate_limit;
pub mod route;
// Detrás de un feature (default-on, así que `cargo build` normal no cambia)
// porque es el único módulo con dependencias nativas (rusqlite/postgres/
// tiny_http/argon2/lettre) -- excluirlo es lo que permite compilar
// lexer/parser/checker/codegen solos a `wasm32-unknown-unknown` para el
// playground web, sin tocar el binario `linkc` normal.
#[cfg(feature = "runtime")]
pub mod runtime;
// Mismo motivo que `runtime` arriba: `linkc introspect` habla PostgreSQL de
// verdad (crate `postgres`), así que no puede vivir en el build wasm32 del
// playground.
#[cfg(feature = "runtime")]
pub mod introspect;
pub mod scaffold;
pub mod token;
pub mod types;
