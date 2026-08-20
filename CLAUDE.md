# c-script (Link)

Compiled backend language: one `.link` file is the single source of truth for
types, database schema, RPC services, auth rules and tests. `linkc build`
emits the TypeScript contract, a typed client, runtime validators, React
hooks, Zod schemas and OpenAPI from it.

**Read [AGENTS.md](AGENTS.md) before writing any code.** It has the repo map,
the real CLI commands, and the syntax mistakes that break almost every first
attempt. The two that matter most:

1. An enum variant used as a **value** needs braces — `Role.Member {}`, not
   `Role.Member`. In `@requires(Role.Admin)` and in a `match` pattern it has
   none.
2. A closure carries no return-type annotation: `|u: User| { u.active }`,
   never `|u: User| -> Bool { ... }`.

Every c-script code block in this repository's documentation is compiled by
the real binary in CI (`compiler/tests/docs_examples.rs`). If you add or edit
one, mark it `<!-- linkc:check -->`, `<!-- linkc:part -->` or
`<!-- linkc:fragment -->` — see AGENTS.md for what each means. An unmarked
block fails the build on purpose.
