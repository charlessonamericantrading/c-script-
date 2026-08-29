# AGENTS.md — instructions for coding agents working in this repository

Read this before writing any code. It is the shortest path from "I have the repo URL" to
"I can produce c-script that actually compiles".

This file is written in English because agents arrive here from every tool. The
authoritative specification (`GRAMMAR.md`) and the engineering plan (`PLAN.md`) are
written in Spanish, as are all source comments and every compiler error message. Keep
writing them in Spanish — matching the surrounding code matters more than a consistent
repo language.

## What this repository is

A compiled backend language and its toolchain, written in Rust. One `.link` file declares
your types, database, RPC services, auth rules and tests; `linkc build` emits a TypeScript
contract, a typed client, runtime validators, React hooks, Zod schemas and OpenAPI from
it. The value proposition is that renaming a backend field breaks `tsc` in the frontend
immediately rather than breaking users in production.

Naming, because it trips up everyone: the repository is `c-script-`, the language is
**c-script**, the README brands it **Link**, files end in `.link`, and the compiler binary
is `linkc`. They are one project.

## Repository map

| Path | What lives there |
|---|---|
| `compiler/src/lexer.rs`, `parser.rs`, `ast.rs` | Front end: source text → AST |
| `compiler/src/checker.rs` | Bidirectional type checker (the largest file; start here for type errors) |
| `compiler/src/codegen/` | TypeScript, Zod, validators, OpenAPI and WASM emitters |
| `compiler/src/runtime/` | Interpreter (`mod.rs`), HTTP server, database layer (`db.rs` + `store.rs`, SQLite and PostgreSQL), sessions |
| `compiler/src/lsp.rs` | Language server (`linkc lsp`) |
| `compiler/tests/` | Integration tests that spawn the real binary as a subprocess |
| `examples/users.link` | The reference program. CI compiles it, snapshots its contract, serves it and drives it from a real client |
| `GRAMMAR.md` | The specification, and the honest limits of every feature. Has a table of contents |
| `llms.txt` | Condensed language reference plus the mistakes LLMs make. Read it before writing `.link` code |
| `CLAUDE.md` | What Claude Code auto-loads on open. A thin pointer into this file — keep it short, don't duplicate |
| `docs/consuming-services.md` | This file is for developing the compiler. If you're instead integrating an already-generated `.link` service from another app, that guide is the one to read, not this one |

## Build, test, run

```bash
cd compiler
cargo build --release        # produces target/release/linkc
cargo test                   # unit + integration; spawns real LSP and HTTP subprocesses
```

```bash
linkc                        # lists subcommands (also: linkc --help)
linkc build examples/users.link gen
linkc test examples/users.link                                    # runs test "..." { } blocks
linkc test examples/users.link examples/users.link.snap --update  # regenerates the contract SNAPSHOT
linkc serve examples/users.link 8787
```

`linkc test <file>` and `linkc test <file> <file>.snap [--update]` are two different
checks sharing one subcommand — the snapshot form needs BOTH positional arguments. Running
`linkc test <file> --update` (no `.snap` path) silently runs only the behavior tests and
touches no snapshot file at all — no error, no "nothing to update" message, it just does not
do what `--update` implies. This has actually shipped a stale `.snap` to CI once; if a
release step is supposed to refresh a snapshot, verify the file's content actually changed
(`git diff`), don't trust the command's exit code alone.

The full end-to-end path that CI enforces, and the one to reproduce before claiming
anything works: build the contract, type-check the frontend against it, then run the real
client against a real server.

## Rules of this codebase

- **Run it; do not reason about it.** This project's real bugs have consistently been
  places where two layers disagree — the checker accepting what the runtime cannot do, the
  documentation describing what the parser rejects. A change is verified when the binary
  ran, not when the code looks right.
- **Comments explain why, not what**, and they record decisions and their limits. The
  existing comments are long on purpose; match that, and update them when the code beneath
  them changes.
- **Documentation examples are tests.** Every c-script block in `README.md`, `llms.txt`,
  the agent rule files and this file is compiled by `compiler/tests/docs_examples.rs`
  using the real binary. Mark any block you add with `<!-- linkc:check -->` (a complete
  program), `<!-- linkc:part -->` (one chapter of a program the file builds up across
  several blocks) or `<!-- linkc:fragment -->` (a snippet that cannot compile). An
  unmarked block fails the test on purpose.
- **Do not add a feature claim to the README without a test that exercises it.** Several
  claims in this repository's history turned out to be aspirational; that is the failure
  mode to avoid.

## Writing c-script: the mistakes that cost the most time

Full list in [`llms.txt`](llms.txt). The three that break almost every first attempt:

1. An enum variant used as a **value** needs braces — `Role.Member {}`. In an annotation
   (`@requires(Role.Admin)`) and in a `match` pattern (`Role.Admin => ...`) it has none.
2. Closures carry no return type: `|u: User| { u.active }`, never `|u: User| -> Bool {...}`.
3. `T?` still can't be dereferenced via `if`. `if x != null { x.name }` is an error — there's
   no narrowing through `if`, deliberately. Use `match x { v: T => v.name, null => ... }`
   instead — that narrows for real (GRAMMAR.md §3.69). For the common "give me a default"
   case, `x ?? default` is shorter; `x.isSome()`/`x.isNone()` cover "just need to know if
   there's a value."

A complete, verified program:

<!-- linkc:check -->
```rust
type Note = {
  id: Int,
  body: String,
  pinned: Bool,
  createdAt: Timestamp,
}

type NewNote = {
  body: String,
  pinned: Bool,
  createdAt: Timestamp,
}

enum Role { Admin, Member }

db {
  notes: Note[],
}

service Notes {
  rpc list() -> Note[] {
    db.notes.all()
  }

  rpc pinned() -> Note[] {
    db.notes.findWhere(|n: Note| { n.pinned })
  }

  @authenticated
  rpc create(body: String) -> Note {
    db.notes.insert(NewNote {
      body: body,
      pinned: false,
      createdAt: now(),
    })
  }

  @requires(Role.Admin)
  rpc remove(id: Int) -> Bool {
    db.notes.delete(id)
  }
}

test "a created note is listed and is not pinned" {
  let n = Notes.create("first");
  assert(n.id > 0, "insert assigns the id");
  assert(Notes.list().length() == 1, "it is listed");
  assert(Notes.pinned().length() == 0, "it is not pinned yet");
}
```

## Known broken, do not re-report as new findings

None right now. Check `gh pr list` before reporting a bug — it may already be tracked in
an open PR. This section stays here as the place to look, and to add to, when something is.
