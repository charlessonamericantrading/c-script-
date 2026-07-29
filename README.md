*[Leer en español](README.es.md)*

# c-script

A compiled backend language whose entire point is **end-to-end type safety with TypeScript**: rename a field in the backend, and the frontend fails to compile (`tsc`) instead of failing in production.

This repo is the **Phase 0 MVP** (see [PLAN.md](PLAN.md) §4, currently Spanish-only): it proves the core mechanism end-to-end. It is not a production-ready language — it's proof the idea works.

## What's here

| | |
|---|---|
| [`PLAN.md`](PLAN.md) | Proposal, phased roadmap, risk analysis *(Spanish)* |
| [`GRAMMAR.md`](GRAMMAR.md) | Formal spec: EBNF, type system, TypeScript mapping table *(Spanish)* |
| [`compiler/`](compiler/) | The compiler (`linkc`), in Rust — see `Cargo.toml` for the current dependency list (each one justified in [GRAMMAR.md](GRAMMAR.md) where it's introduced) |
| [`examples/users.link`](examples/users.link) | The example program: a user CRUD service |
| [`frontend/`](frontend/) | A real TypeScript frontend consuming the generated contract |
| [`gen/`](gen/) | Output of `linkc build` — `contract.d.ts` + `client.ts` + `validators.ts` (generated, don't hand-edit) |

## Try the killer feature yourself

```bash
cd compiler
cargo build

# 1. Generate the TypeScript contract from the backend
./target/debug/linkc build ../examples/users.link ../gen

# 2. Confirm the frontend typechecks clean
cd ../frontend && npm install && npx tsc --noEmit   # exit 0

# 3. Start the server and run the frontend for real
cd ../compiler && ./target/debug/linkc serve ../examples/users.link 8787 &
cd ../frontend && node src/main.ts                  # calls the real server, typed end-to-end
```

The server starts with an **empty** database — it creates one empty collection per `db { ... }` declaration in your program and nothing else. The demo's first run therefore creates its own user and then reads it back; a language runtime inventing rows you never wrote would be a lie about what your program does.

Now break something: in `examples/users.link`, rename `name` to `fullName` inside `type User`. Re-run `linkc build` and `npx tsc --noEmit` **without touching `frontend/src/main.ts`**. `tsc` fails on every line that used `.name` — exactly the blind spot c-script exists to eliminate (see [PLAN.md](PLAN.md) §3).

Starting a project from scratch is faster: `linkc new my-app` scaffolds a minimal `.link` file plus a matching `frontend/`; `linkc dev my-app/main.link my-app/gen` watches it (and anything it `import`s) and regenerates the contract on every save, instead of re-running `build` by hand.

## Why not just tRPC / Bun / Deno?

They solve adjacent but different problems:

- **tRPC, Encore.ts, Convex** give you E2E type safety by having *no language boundary at all* — the backend already is TypeScript. Clever, but only works if your whole stack is TS.
- **Bun, Deno** are faster, more modern JS/TS runtimes — but still JS/TS semantics under the hood, not systems-language performance.
- **Rust+ts-rs, Go+tygo, gRPC/protobuf, OpenAPI codegen** are the actual comparison set: a non-TS backend bridged to a TS frontend. They require a separate IDL/schema you keep in sync by hand, or give you types without a full RPC client. c-script's bet: the backend type declaration itself *is* the contract — no separate schema, automatic client, automatic wire validators.

## Status

Done (Phase 0): lexer, parser, bidirectional type checker (structural/nominal subtyping, `Result<T,E>` and `Patch<T>` as builtins, arithmetic/comparison/logical operators, `if/else`, assignment and mutability, arrays, tuples, explicit numeric conversion, `Map<K,V>`, string builtin methods), user-defined generics via monomorphization, union types (`A | B`) with value-flow subtyping AND narrowing back to a concrete member via `match` (`name: Type` patterns, reusing the same `:` that already means "declared type" everywhere else, with unions whose members can't be told apart at runtime rejected at compile time rather than silently mismatched), functions as first-class values — named references AND real lexical closures (`|params| { block }`, with real contravariant/covariant function subtyping) — plus higher-order `List` methods (`.map`/`.filter`), `match` exhaustiveness extended with literal patterns, or-patterns, and guards, `const` declarations, contract emitter, minimal interpreted runtime.

Done (Phase 1, partial): `linkc new`/`linkc dev` CLI, multi-file imports with a minimal path-based package manager (`link.lock` now records a SHA-256 per touched file and warns on drift between builds — still no version resolution or network registry, see GRAMMAR.md §2.1), and a v0 WASM target — the existing interpreter recompiled to `wasm32-wasip1`, proven by running a real RPC call inside `wasmtime` end to end (`compiler/src/bin/wasm_demo.rs`; see PLAN.md §2.4 for exactly what that does and doesn't prove).

Done (Phase 2, partial): runtime validators (`validators.ts` — the third emitter output planned since PLAN.md's first draft; every RPC response is checked against the declared contract before the client hands it back, throwing `LinkValidationError` on a mismatch instead of silently returning malformed data), a v0 typed `db` (`db { users: User[] }` replaces `Type::Dynamic` — `all/find/insert/applyPatch` are now checked against the real element type, still fully in-memory, no SQL driver), real SSE streaming for `stream` (genuine wire framing — `Transfer-Encoding: chunked` with a flush per event, not one buffered JSON blob — replaying an already-computed sequence; the generated client consumes it as a real `AsyncIterable<T>`, validating each event), and auth v0 (`@authenticated`/`@requires(Role.Admin)` decorators on a `rpc`/`stream`, backed by opaque in-memory sessions — no JWT, no new dependency, password/credential verification explicitly out of scope; see [GRAMMAR.md](GRAMMAR.md) §3.14 for the token-generation weakness two adversarial reviews caught and the fix).

Done (Phase 2, LSP prerequisite #1 of 3): tokens now carry a real column (not just line), two real position bugs in the lexer are fixed (error spans used to land one character late in `lex_punct`/`lex_string`/`lex_number`, and an unterminated string/block comment used to mix its opening line with an EOF position — both invisible until a real renderer existed to expose them), and a new `diagnostics` module renders lexer/parser errors as a gcc/rustc-style snippet with a caret, with no new dependency. A syntax error inside an *imported* file now also names that file (previously collapsed to a bare line number, losing which of several files had the problem).

Done (Phase 2, LSP prerequisite #2 of 3): the parser recovers from a syntax error instead of aborting on the first one — it now reports every independent error found in a single pass (at top-level-item granularity: one broken `service`/`type`/`fn`/etc. doesn't stop the others from being checked, though it's discarded whole rather than salvaging the well-formed members inside it). Caught during design review: an earlier version of the recovery step advanced one token unconditionally before resynchronizing, which silently swallowed the next real item's own error whenever a syntax error happened nested inside something (the common case — a missing closing brace); fixed by checking before advancing instead.

Done (Phase 2, LSP prerequisite #3 of 3): every `Expr`/`Stmt` node in the AST now carries its own real position (`Spanned<T>`, chosen as the most precise — and most expensive — of three considered granularities), and the type checker actually uses it: a *type* error (mismatched operand, missing struct field, an rpc signature that can't cross the wire, ...) now renders the same gcc/rustc-style snippet-with-caret that syntax errors already got in prerequisite #1, not just a bare message. Shipped as two rounds: a purely mechanical migration first (every existing test kept passing with zero behavior change, the signal that the ~155-site refactor across the parser/checker/runtime/codegen didn't alter anything), then the checker itself stamping and rendering those positions. `Span` still has no file identity, so a type error inside a multi-file program's *imported* file falls back to the old plain-text form rather than risk rendering a plausible-but-wrong snippet against the wrong file — real per-file provenance is follow-up work, not done here. The actual LSP protocol server (JSON-RPC over stdio, `textDocument/didOpen`, `publishDiagnostics`, completion, hover) is a separate, larger round that hasn't started. 269 tests, all passing.

Done (Phase 2): a `while` loop construct (`Stmt`, never `Expr`; no `for`/`break`/`continue`; a hard iteration cap since the single-threaded, timeout-free server would otherwise hang for every client on one infinite loop) and, built on it, real push for `stream`: a body that is exactly `while true { db.<collection>.subscribe() }` is recognized as one fixed syntactic shape at compile time — chosen over building a general coroutine/`yield` mechanism for arbitrary per-event logic — and intercepted before the interpreter ever runs it. A pub-sub registry on `Db` (bounded channel, non-blocking publish, lazy eviction of disconnected subscribers) delivers a snapshot followed by real live events over the same SSE wire format from the previous streaming round, with zero client codegen changes (the generated client already read indefinitely). Whole-collection only in v0 — no per-row `subscribe(id)`, no event filtering/transformation inside the stream body (the client can already filter by id for free). Verified end to end with the real generated client: snapshot delivery, a live event arriving over an already-open connection after a separate insert, and a disconnected subscriber pruned lazily on the next write without crashing or hanging the server. See [GRAMMAR.md](GRAMMAR.md) §3.15 (`while`) and §3.16 (pub-sub) for the full design, the concurrency argument for why no new lock was needed, and what's explicitly out of scope.

Done (Phase 2): `db { ... }` is now backed by real SQLite (`rusqlite`, `bundled` feature — no system SQLite, no external server process, same "just run it" ethos as the embedded `tiny_http`), replacing the purely in-memory store — data now survives a `linkc serve` restart. The SQL schema is derived automatically from the same `db { ... }` declaration, the same "one source of truth, everything else generated" principle behind `contract.d.ts`/`client.ts`/`validators.ts`: scalars and simple enums get real typed columns (so `find(id)` is now an indexed lookup instead of a linear scan), anything nested reuses the existing `Value`↔JSON conversion as a JSON column, and every nullable/optional-by-key combination — including the one that needs 3 states, `x?: T?` — round-trips losslessly. An incompatible schema on reopen fails loudly with an exact diff and a "delete the file" remedy rather than attempting to migrate. `rusqlite` compiles and runs correctly for the `wasm32-wasip1` demo target too (confirmed with a real spike, not assumed) — one backend serves both native and wasm, no target-specific fork. Verified against the real binary: insert over HTTP, kill the process, restart it, confirm the data survived without re-inserting. Breaks this project's own previously-documented "zero new dependencies" rule, consciously — see [GRAMMAR.md](GRAMMAR.md) §3.17 for the full design, the column-mapping table, and what's explicitly out of scope (real migrations, any engine other than SQLite — `delete` shipped in the next round, see below).

Done (Phase 2): real CRUD completion on `db` — `delete(id) -> Bool`, `deleteWhere(fn(T) -> Bool) -> Int`, `findWhere(fn(T) -> Bool) -> T[]`, same spirit as `List.filter` (§3.10) now over a persisted collection. The predicate is evaluated by the interpreter (`call_callable`, same interception point that already redirects `List::filter`/`.map`) because the SQL storage layer (`Db::call`) has no access to closures/environment at all — a structural reason, not an oversight, and the reason `Db::call`'s own dead `deleteWhere`/`findWhere` arms now return a clear error instead of quietly ignoring the predicate if ever reached directly (they aren't, in normal interpreter dispatch, but the function is `pub` and was reachable). `delete` now also publishes to `stream` subscribers (§3.16), so a live subscriber sees a deletion as an event, not just inserts. `id` gained `AUTOINCREMENT` since a real `delete` makes id-reuse-after-delete an actual possibility instead of a moot point. See [GRAMMAR.md](GRAMMAR.md) §3.18.

Done (Phase 2): the LSP protocol server itself (`linkc lsp` — JSON-RPC 2.0 over stdio, hand-rolled framing rather than the originally-planned `lsp-server`/`lsp-types` crates, which ended up with zero consumers and were removed from `Cargo.toml`). Diagnostics, hover, completion, and goto-definition all resolve against the real merged multi-file program (`modules::load_program_with_overlay` + `checker::check_program_full`) instead of an isolated buffer — fixing a real false-positive bug where any file using `import` reported its imported symbols as undeclared. Span-to-range conversion is now genuinely multi-line and UTF-16-aware (the CLI's own renderer gets away with assuming single-line spans; the LSP has the full document and does the real computation). Scope is deliberately Level 1 (diagnostics) + Level 2 (declaration-level hover/completion/goto-def, not position-sensitive) — see [GRAMMAR.md](GRAMMAR.md) §3.19 for the full design and what's explicitly deferred to a future Level 3. A minimal real VS Code client lives in `editors/vscode/`.

Done (Phase 1, the "evolution" the WASM row above named but hadn't started): `linkc wasm <file> <out.wasm>` emits real, direct WASM bytecode per function via `wasm-encoder` — no interpreter involved, distinct from (and much narrower than) the `wasm32-wasip1` interpreter-recompile above, which remains the actual production path. Scope is intentionally minimal: only `Int`/`Bool` params and return types (both map to `i64`), and a body that is exactly one final expression (integer/boolean arithmetic and comparisons) — no statements, no other types. Outside that subset, emission now fails with a clear, specific error instead of silently substituting a placeholder — an earlier version (from outside this session) replaced anything unsupported with `I64Const(0)` and dropped every statement in a block, so `linkc wasm`/`linkc build` reported success while producing wrong bytecode; `linkc build`'s own success message now only names `main.wasm` when it was actually written. See [GRAMMAR.md](GRAMMAR.md) §3.20 for exactly what's supported and why closing this gap for a real program is its own future round, not an incremental extension.

Not done yet: LSP Level 3 (position-sensitive completion after `x.`, hover of an arbitrary mid-body expression, goto-def of a type name in a signature — see GRAMMAR.md §3.19), and closing the WASM native-codegen gap for real programs (statements, non-scalar types — see GRAMMAR.md §3.20). See [GRAMMAR.md](GRAMMAR.md) §2.1 for what's still missing in the import/package-manager story (dependency version resolution, a network registry).

## License

MIT — see [LICENSE](LICENSE).
