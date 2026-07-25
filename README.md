*[Leer en español](README.es.md)*

# c-script

A compiled backend language whose entire point is **end-to-end type safety with TypeScript**: rename a field in the backend, and the frontend fails to compile (`tsc`) instead of failing in production.

This repo is the **Phase 0 MVP** (see [PLAN.md](PLAN.md) §4, currently Spanish-only): it proves the core mechanism end-to-end. It is not a production-ready language — it's proof the idea works.

## What's here

| | |
|---|---|
| [`PLAN.md`](PLAN.md) | Proposal, phased roadmap, risk analysis *(Spanish)* |
| [`GRAMMAR.md`](GRAMMAR.md) | Formal spec: EBNF, type system, TypeScript mapping table *(Spanish)* |
| [`compiler/`](compiler/) | The compiler (`linkc`), in Rust, no external deps beyond `tiny_http`/`serde_json` for the demo runtime |
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

Done (Phase 1, partial): `linkc new`/`linkc dev` CLI, multi-file imports with a minimal path-based package manager (no lockfile, no network registry yet — see GRAMMAR.md §2.1), and a v0 WASM target — the existing interpreter recompiled to `wasm32-wasip1`, proven by running a real RPC call inside `wasmtime` end to end (`compiler/src/bin/wasm_demo.rs`; see PLAN.md §2.4 for exactly what that does and doesn't prove).

Done (Phase 2, partial): runtime validators (`validators.ts` — the third emitter output planned since PLAN.md's first draft; every RPC response is checked against the declared contract before the client hands it back, throwing `LinkValidationError` on a mismatch instead of silently returning malformed data), a v0 typed `db` (`db { users: User[] }` replaces `Type::Dynamic` — `all/find/insert/applyPatch` are now checked against the real element type, still fully in-memory, no SQL driver), real SSE streaming for `stream` (genuine wire framing — `Transfer-Encoding: chunked` with a flush per event, not one buffered JSON blob — replaying an already-computed sequence; the generated client consumes it as a real `AsyncIterable<T>`, validating each event), and auth v0 (`@authenticated`/`@requires(Role.Admin)` decorators on a `rpc`/`stream`, backed by opaque in-memory sessions — no JWT, no new dependency, password/credential verification explicitly out of scope; see [GRAMMAR.md](GRAMMAR.md) §3.14 for the token-generation weakness two adversarial reviews caught and the fix).

Done (Phase 2, LSP prerequisite #1 of 3): tokens now carry a real column (not just line), two real position bugs in the lexer are fixed (error spans used to land one character late in `lex_punct`/`lex_string`/`lex_number`, and an unterminated string/block comment used to mix its opening line with an EOF position — both invisible until a real renderer existed to expose them), and a new `diagnostics` module renders lexer/parser errors as a gcc/rustc-style snippet with a caret, with no new dependency. A syntax error inside an *imported* file now also names that file (previously collapsed to a bare line number, losing which of several files had the problem). 258 tests, all passing.

Not done yet: an LSP still needs two more things before the protocol server itself makes sense — real recovery in the parser (today it's strictly fail-fast: the first syntax error aborts the whole parse, so you fix one typo only to hit the next one-by-one instead of seeing them all at once) and spans threaded into the AST/checker (a *type* error still has no position at all, only syntax errors do). Also still missing: a `.wasm`-emitting codegen backend (today's WASM target recompiles the tree-walking interpreter rather than generating native wasm instructions), real push (WebSocket, or long-lived SSE) so a `stream` can announce FUTURE events — today it only replays an already-computed sequence; subscribing to changes would need a pub-sub layer over `db` that doesn't exist, and a genuinely lazy generator would additionally need a loop construct, which the language still doesn't have (recursion through a named `fn` or a self-referencing closure works today, but there's no `for`/`while` syntax) —, and a real SQL-backed DB. See [GRAMMAR.md](GRAMMAR.md) §2.1 (imports/package manager), §3.12 (`db`) and §3.13 (streaming) for exactly what each of those means and why.

## License

MIT — see [LICENSE](LICENSE).
