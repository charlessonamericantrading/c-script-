// El compilador como librería -- lo que `main.rs` (el binario `linkc`) y
// `src/bin/wasm_demo.rs` (v0 de compilación a WASM, GRAMMAR.md/PLAN.md §2.4)
// consumen por igual. Un `src/bin/*.rs` compila como un crate genuinamente
// aparte aunque viva en el mismo paquete Cargo -- sin este `[lib]`, no
// tendría de dónde importar `ast`/`checker`/`runtime`/etc.

/// Versión exacta de ESTE binario -- `env!("CARGO_PKG_VERSION")` la toma de
/// `Cargo.toml` en tiempo de COMPILACIÓN, así que nunca puede desincronizarse
/// del binario real que la reporta (a diferencia de un string hardcodeado
/// aparte, que alguien podría olvidarse de actualizar en un release).
/// `linkc --version` (GRAMMAR.md §3.83) la imprime tal cual, y cada archivo
/// que `linkc build` genera (`contract.d.ts`/`client.ts`/`hooks.ts`/
/// `validators.ts`/`schemas.ts`/`openapi.json`) queda estampado con ella --
/// para saber con qué versión exacta del compilador se generó un `gen/`
/// dado, cuando conviven varias versiones en el tiempo (PLAN.md §9.7).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

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
pub mod systemd;
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
pub mod migrate;
pub mod scaffold;
pub mod token;
pub mod types;
