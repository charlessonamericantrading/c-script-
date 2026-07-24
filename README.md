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
| [`gen/`](gen/) | Output of `linkc build` — `contract.d.ts` + `client.ts` (generated, don't hand-edit) |

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

Now break something: in `examples/users.link`, rename `name` to `fullName` inside `type User`. Re-run `linkc build` and `npx tsc --noEmit` **without touching `frontend/src/main.ts`**. `tsc` fails on every line that used `.name` — exactly the blind spot c-script exists to eliminate (see [PLAN.md](PLAN.md) §3).

## Why not just tRPC / Bun / Deno?

They solve adjacent but different problems:

- **tRPC, Encore.ts, Convex** give you E2E type safety by having *no language boundary at all* — the backend already is TypeScript. Clever, but only works if your whole stack is TS.
- **Bun, Deno** are faster, more modern JS/TS runtimes — but still JS/TS semantics under the hood, not systems-language performance.
- **Rust+ts-rs, Go+tygo, gRPC/protobuf, OpenAPI codegen** are the actual comparison set: a non-TS backend bridged to a TS frontend. They require a separate IDL/schema you keep in sync by hand, or give you types without a full RPC client. c-script's bet: the backend type declaration itself *is* the contract — no separate schema, automatic client, automatic wire validators.

## Status

Done (Phase 0): lexer, parser, bidirectional type checker (structural/nominal subtyping, `Result<T,E>` and `Patch<T>` as builtins, arithmetic/comparison/logical operators, `if/else`, assignment and mutability, arrays, tuples, explicit numeric conversion, `Map<K,V>`, string builtin methods), user-defined generics via monomorphization, union types (`A | B`) with value-flow subtyping, functions as first-class values (named references, with real contravariant/covariant function subtyping), `match` exhaustiveness extended with literal patterns, or-patterns, and guards, `const` declarations, contract emitter, minimal interpreted runtime. 133 tests, all passing.

Not done yet: a real compilation backend (a `.wasm`-emitting one — not Cranelift, which only targets native code; see PLAN.md §2.4), an LSP, a network-backed package registry, real streaming over WebSocket/SSE, a typed DB layer, anonymous function literals / true lexical closures, higher-order `List` methods (`.map`/`.filter`), narrowing a union-typed value back to a concrete member, and multi-file imports (the `import` syntax parses but has no effect yet). See [GRAMMAR.md](GRAMMAR.md) §2.1, §3.6, §3.9, §3.10 for exactly what each of those means and why.

## License

MIT — see [LICENSE](LICENSE).
