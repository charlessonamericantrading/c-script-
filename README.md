*[Leer en español](README.es.md)*

<div align="center">
  <h1>⚡ Link (c-script)</h1>
  <p><strong>The compiled backend language designed for absolute End-to-End Type Safety with TypeScript.</strong></p>
  
  <p>
    <a href="https://github.com/charlessonamericantrading/c-script-/actions/workflows/ci.yml"><img src="https://github.com/charlessonamericantrading/c-script-/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
    <a href="#-testing--quality-assurance"><img src="https://img.shields.io/badge/tests-679-success.svg" alt="Tests" /></a>
    <a href="https://github.com/charlessonamericantrading/c-script-/releases"><img src="https://img.shields.io/badge/version-1.51.0-blue.svg" alt="Version" /></a>
    <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-purple.svg" alt="License" /></a>
  </p>
</div>

---

## 💡 Why Link?

Whenever you rename a field in your backend or database, your frontend shouldn't silently break in production. With **Link**, your frontend fails to compile (`tsc`) immediately during development.

```
┌─────────────────┐       linkc build        ┌─────────────────────────────────────────┐
│   main.link     │ ───────────────────────► │ 📄 contract.d.ts  (TypeScript types)     │
│                 │                          │ 🔌 client.ts      (Type-safe RPC client) │
│ • Structs/Enums │                          │ 🛡️ validators.ts  (Runtime validation)   │
│ • Typed DB      │                          │ ⚛️ hooks.ts       (React SSR/Streaming)  │
│ • Auth & RBAC   │                          │ 📜 openapi.json   (OpenAPI 3.1 spec)     │
│ • Streams (SSE) │                          │ 🗄️ schema.pg.sql  (PostgreSQL DDL)       │
└─────────────────┘                          └─────────────────────────────────────────┘
```

---

## 📊 Status — what works and what does not

This section is the ground truth. If any other section of this README disagrees with it,
this section wins. Verified on 2026-08-24 by running the compiler, not by reading it.

**Works today**, covered by 808 automated tests:

- `linkc build` / `serve` / `test` / `dev` / `lint` / `doc` / `docker` / `lsp` / `new`
- `--http-timeout <duration>`/`LINK_HTTP_TIMEOUT` for `linkc serve`: caps how long any outbound `http.*` call can take — 30s by default. Until now, `http.get`/`post`/`getWithHeaders`/etc. had no read/write timeout at all (`ureq` only defaults a 30s *connect* timeout); against this single-threaded interpreter, a slow or hanging remote server blocked the entire process forever — not even `/health` responded meanwhile. Same precedence and duration format (`Ns`/`Nm`/`Nh`/`Nd`) as `--session-ttl`; a timed-out call surfaces as an ordinary runtime error, never a panic or a hang
- `--max-body-bytes <N>`/`LINK_MAX_BODY_BYTES` for `linkc serve`: caps how many bytes of body any request can send — 10 MiB by default. Until now the server read a request's entire body into memory with no limit at all, a real memory-exhaustion vector. The read is bounded with `Read::take(max_body_bytes + 1)` and rejected with `413 Payload Too Large` *before* it's read in full — auth, rate limiting, and JSON parsing never get a chance to compete for memory with a body already known to be too large. Process-wide, not per-rpc; a rejected body's remaining bytes aren't drained (a client that reuses the same connection anyway gets a clean 400 on its next attempt, never a hang or a leak)
- `linkc --version`/`-v`/`version` prints the exact compiler version (`env!("CARGO_PKG_VERSION")`, taken from `Cargo.toml` at compile time) — the same constant stamps the header of every generated TypeScript file (`contract.d.ts`/`client.ts`/`hooks.ts`/`validators.ts`/`schemas.ts`) and, since JSON has no comments, an `x-generated-by` vendor extension in `openapi.json` (never `info.version`, which is the documented API's own version, a separate concern). Purely informational — nothing checks a stale `gen/`'s stamped version against the binary serving or rebuilding it
- `linkc test <file> --filter <name>`: runs only the `test "..." { ... }` blocks whose name CONTAINS that substring (case-sensitive, same rule as `cargo test <substring>`) — a filter matching nothing runs zero tests and still succeeds. Only applies to the integrated test runner, never to contract snapshot testing (`linkc test <file> <snap>`), which has no test names to filter — combining the two is a clean usage error, not a silently-ignored flag
- `--host <address>`/`LINK_HOST` for `linkc serve`: binds to `0.0.0.0` (all interfaces) by default, same as before — or to one specific address (`127.0.0.1`, for a process that only needs local connections) so the OS firewall isn't the only thing standing between it and the rest of the network. Passed straight to the underlying bind call, no extra resolution or validation beyond rejecting an empty `--host ""` — an address that doesn't belong to any local interface fails to start, naming the exact address, never silently falling back to `0.0.0.0`
- Declarative single-field indexes: `@index`/`@unique` on a struct field — neither requires a particular field type. The index is created for real on startup in both backends (`CREATE [UNIQUE] INDEX IF NOT EXISTS`, idempotent, deterministic name), and `linkc build` emits the same statement in the static Postgres DDL. A `@unique` violation on `insert`/`applyPatch` (and on `upsert`'s update path) is translated to a 400, not a generic 500 — matched against the exact message SQLite/Postgres return for that specific violation. `--adopt-existing` never runs this DDL either, same rule as the rest of the schema. Composite (multi-field) indexes/constraints aren't supported yet — only single-field
- `linkc build --diff <file>`: compares the freshly generated `contract.d.ts` against a saved copy (typically `git show <rev>:path > file` before the build) — for reviewing exactly what changed in the public contract on a PR. Reuses the same LCS diff `linkc test` already had for showing why a snapshot changed. Purely informational, never fails the build — an unreadable comparison file just prints a warning to stderr
- Native soft-delete: `@softDelete` on a `Timestamp?` field turns `delete(id)` into an idempotent `UPDATE` (sets the field to `now()`, `AND "<field>" IS NULL` in the WHERE so a second call is a no-op returning `false`, never a real `DELETE`). Every read that returns a list or a count — `all()`, `page()`, `pageAfter()`, `count()`, the `*By` aggregates, and `findWhere`/`deleteWhere` (which reuse `all()` internally, no extra code needed) — filters it out automatically. `find(id)` deliberately does not filter — a soft-deleted row stays reachable by direct id lookup, same tradeoff Django/Rails make, and needed so `insert`/`applyPatch`'s own post-write re-fetch never explodes if a patch happens to touch that field
- Automatic `createdAt`/`updatedAt`: no magic field names — `createdAt: Timestamp = now()` (an existing default composed with the existing `now()` builtin) already covers "set once at creation." `@autoUpdate` on a `Timestamp` field (only) is the one new piece — it forces that field to `now()` on every `applyPatch`/`upsert`-update, even when the patch doesn't mention it, while a field without the annotation is never touched automatically
- `db.<c>.insertMany(items) -> T[]`: each item goes through the same real `insert` (one autocommit SQL statement per row), in order — saves the N sequential HTTP round trips from the client for a backfill, not the cost of N inserts against the database itself. No wrapping transaction: if item 3 of 5 fails, the first two stay inserted
- `db.<c>.upsert(matchFn, insertValue, updateFn)`: update-in-place-or-insert without hand-rolling find+delete+reinsert (which doesn't even preserve the row's id across a real autoincrement). `matchFn` scans the whole collection in the interpreter (same limit as `findWhere`/`deleteWhere` — not pushed to SQL yet); on a match, `updateFn` receives the full existing row and its result is applied onto that SAME id. `updateFn` returns a full `Omit<T,"id">` value, not a partial `Patch<T>` — deliberately, since `Patch<T>` has no literal syntax and couldn't be constructed inside a function body at all
- Default values on struct fields: `status: String = "pending"` — same syntax and mechanism as a function/rpc param default. A field with a default can be omitted from a `Struct { ... }` literal without becoming `Optional` — it stays the same declared type. Filled in by the interpreter on construction, evaluated fresh every time (`token: Uuid = crypto.uuid()` gives a different value per literal, not a cached one). Propagated as an optional field to `contract.d.ts`/`schemas.ts` (Zod), and out of `required` in `openapi.json` (plus a literal `"default"` value when it's a simple constant). No access to sibling fields in the same literal, and no support on a generic `type` yet
- `@validate(email)` / `@validate(regex, "...")` on a `String`/`String?` field, enforced for real in four places — the actual server (400 on a bad value, checked both when an rpc receives the whole struct as a param and when it's built inline from loose params inside the body — a real `curl` test against a running server is what caught that second path was missing at first), `openapi.json` (`format`/`pattern`, standard JSON Schema keywords), `schemas.ts`/Zod (`.email()` / `.regex(new RegExp(...))`, correctly chained before `.nullable()` on an optional field), and an informative JSDoc comment in `contract.d.ts`. A malformed regex pattern is a compile error, never a first-request surprise. The one real gotcha: `@validate` is tied to the exact declaration it's written on — the "New*" (`Omit<T,"id">`) shape used everywhere for `insert` is a separate type, so the annotation has to be repeated there too. `validators.ts`'s hand-written `isX()` type guards don't enforce it yet — everything else does
- `///` docstrings on an rpc/stream, propagated as `description` in the generated `openapi.json` and as a multi-line JSDoc block in `contract.d.ts` — purely additive: `///` was already valid anywhere (same trivia as `//`), so no existing program stops compiling; the parser only reads the captured text right above a `rpc`/`stream` (through an `@annotation` in between, if any). Combines with `@deprecated` on the same rpc into one field/block instead of one overwriting the other
- `@deprecated("usa X en su lugar")` on a struct field or an rpc/stream — purely informational, no effect on runtime or on structural subtyping (a struct is still the same type whether or not a field carries it). Propagated as a JSDoc `/** @deprecated ... */` comment right above the field/method in the generated `contract.d.ts`, and as native `deprecated: true` + `description` (Operation Object / JSON Schema 2020-12 keywords, no proprietary `x-*` extension) in `openapi.json`. On a field it's the ONLY annotation accepted there — any other name (`@authenticated`, etc.) is a syntax error at that position, not silently ignored
- Real narrowing of `T?` inside an rpc body: `match x { v: T => v.field, null => ... }` binds `v` to the real `T` (not `T?`) in that branch — reuses the same exhaustive pattern-matching machinery already used for union narrowing, so a missing `null` or a missing value arm is a compile error, not a runtime surprise. `if x != null { x.field }` still doesn't narrow — that stays deliberate — but `match` does. `x ?? default` covers the common "give me a default" case (chains left-to-right: `a ?? b ?? c`), and `x.isSome()`/`x.isNone()` cover "just need to know if there's a value," both without needing a full `match`
- Native `Uuid` type: validates the canonical `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` form at every boundary a value can cross — decoding an incoming request, `validators.ts`, and the generated Zod schema, all with the exact same check so the three can never disagree. A separate type from `String`, no implicit mixing (same rule as `Int64` vs `Int`) — `crypto.uuid()` returns `Uuid`, and `.toString()` is the explicit way down to a plain string
- `linkc introspect <postgres-url>` generates a starting `.link` (types + `db {...}`) from an existing PostgreSQL database's schema — for adopting a system with real data instead of writing every field by hand. A starting point to review, not production-ready as-is: any column it can't map with confidence (`jsonb`, `uuid`, a native `timestamp`/`timestamptz`) still gets a valid type (`String`) plus a warning on stderr, never silently dropped. PostgreSQL only, no `service` generated
- Embedded SQLite with real persistence across restarts, and non-destructive auto-migrations
- Live push over Server-Sent Events (`stream` + `db.<c>.subscribe()`)
- Declarative auth: `@authenticated`, `@requires(Role.Admin)` (or `@requires(Role.Admin | Role.Agent)` for any of several roles, all from the same enum), session tokens from the OS CSPRNG. `linkc serve --session-ttl 7d` (or `LINK_SESSION_TTL`) makes sessions expire on their own — unset, they still live until `destroySession()` or a process restart, as before. `auth.currentRole() -> String?` reads which role authenticated the current request from inside an rpc body — lets a `Role.Admin | Role.Agent` endpoint behave differently per role, not just allow/deny; works with no auth annotation at all too, `null` if there's no valid session. `auth.createSessionWithId(role, userId)` associates the user id in the session, and `auth.currentUserId() -> Int?` inspects it from inside any rpc body (`null` if no session or created without id). `auth.destroyAllSessions(userId: Int) -> Int` revokes every session of a given user at once (password change, an admin ban) and returns how many were closed — unlike `destroySession()` (which only ever acts on the current request's own session, precisely so nobody can revoke someone else's by guessing a token), this one takes an explicit `userId`, same reasoning as `createSessionWithId`: a user id is an application key, not a guessable secret. Gating who's allowed to call it (typically `@requires(Role.Admin)`) is up to whoever writes the `.link`
- External auth: `linkc serve --jwt-secret <secret>` (or `LINK_JWT_SECRET`) verifies an HS256 JWT already issued by an existing backend — alongside, never instead of, Link's own sessions. `@requires`/`@authenticated`/`auth.currentRole()`/`auth.currentUserId()` all work the same regardless of which one authenticated the request. `--jwt-role-claim`/`--jwt-user-id-claim` (default `role`/`sub`) pick which claims carry the role and user id; `sub` accepts a JSON number or a digit string (real OIDC convention). Only HS256 — any other `alg`, including `"none"`, is rejected before checking a signature at all
- PostgreSQL as the runtime database: `linkc serve app.link 8787 --db postgres://user:pass@host/db` (or `LINK_DATABASE_URL`), with non-destructive auto-migration (a new column always lands nullable, even a required one — a pre-existing row with `NULL` there now fails that one read with a clean error naming the row and field, never a silent `null` sent to a typed client or a process crash), opportunistic TLS (pure-rustls, no OpenSSL — connects to managed providers like Supabase/Neon/RDS that require it), automatic reconnection after a dropped connection, and LISTEN/NOTIFY so a `stream` connected to one `linkc serve` instance sees a write that came in through another instance against the same database. Same program, same generated contract — SQLite remains the default. The generated `schema.postgres.sql` never requires `CREATE EXTENSION` for anything — verified applying it as a real Postgres role with no superuser/createrole privileges, the kind you actually get on a managed provider
- Adopting an existing database without touching it: `linkc serve --adopt-existing` (or `LINK_ADOPT_EXISTING`) makes every declared collection assume its table already exists — never runs `CREATE TABLE` or `ALTER TABLE`, not even the usual non-destructive kind, only read-only checks that each declared column is actually there. For a database role with no DDL permission (common in production), or a SQLite/Postgres table that already carries columns this program doesn't model (which it now simply ignores instead of refusing to start)
- Non-JSON responses: `@content_type("text/html; charset=utf-8")` on an rpc returning `String` sends that body verbatim — HTML pages, XML sitemaps, CSV — and stacks with `@requires(Role.Admin)` for pages behind auth. `"...".escapeHtml()` sanitizes untrusted data before it goes into a page (not automatic — you call it where you interpolate). `response.setStatus(code)` picks the success-path HTTP status (e.g. a branded 404 page for an `@route` that found nothing, or 201 on a plain JSON `create`) — transport errors still always come back as JSON, unchanged
- Friendly URLs: `@route("/blog/:slug")` gives an rpc a clean, crawlable GET path alongside (never instead of) its normal `/Service/rpc` address — the generated client keeps using the latter. Any number of `:param` segments, in any position (`/blog/:category/:slug`), bound by name; a more specific route (more fixed segments) deterministically wins over a fully dynamic one that would also match. A trailing catch-all segment (`:name*`) captures the rest of the path, joined by `/`. Any rpc param NOT named in the path is read from the query string instead (`String`/`Int` required, `String?`/`Int?` optional — `null` if absent) — a filter like `?page=2` no longer needs a separate rpc; `body` is still never read, on purpose, since the whole point is a plain GET a crawler can open
- Verifying third-party webhooks: `env.get(name)`, `request.rawBody()` / `request.header(name)`, and `crypto.hmacSha256(secret, message)` give an rpc everything it needs to check a Stripe/GitHub/etc. signature before trusting a callback
- Calling third-party APIs: `http.get(url)` / `http.post(url, body)`, plus `http.getWithHeaders(url, headers)` / `http.postWithHeaders(url, body, headers)` for calls that need `Authorization` or any other header — `headers` is any `{name: String, value: String}[]` you declare, no built-in type required. Response is the body as `String`; a non-2xx becomes a normal runtime error, not a panic. When the status code or response headers matter (e.g. retry only on 429), `http.getWithStatus(url, headers)` / `http.postWithStatus(url, body, headers)` return `{status: Int, headers: {name: String, value: String}[], body: String}` instead — same structural-type principle, a 4xx/5xx is data, not an error
- Real pagination: `db.<c>.page(limit, offset)` pushes `LIMIT`/`OFFSET` into the actual SQL query (SQLite and Postgres both) instead of fetching the whole table and slicing in memory — same row order as `all()`, so pages never overlap or skip a row. `db.<c>.pageAfter(cursor, limit)` is a cursor-based alternative for sequential/infinite-scroll pagination — the cursor is the last-seen `id` (`null` for the first page), stable under concurrent inserts unlike `OFFSET`, which counts rows from the start on every call
- Real aggregation: `db.<c>.sumBy(groupSelector, valueSelector)` / `countBy(groupSelector)` / `avgBy` / `maxBy` / `minBy` push a `GROUP BY` into actual SQL — MRR by plan, counts by status — instead of pulling every row into memory. Selectors must be a bare field access (`|o: Order| { o.planId }`); group-by is `String`/`Int`/`Int64`/`Bool`/`enum` (no date truncation yet), the aggregated field must be `Int`/`Int64`/`Float` — `Int64` stays `Int64` in the result, never silently narrowed to `Int`. Grouping by an `enum` field returns the real enum as the key, not a string
- Per-client rate limiting: `@rate_limit("20/1m")` caps an rpc to N requests per time window per `(client IP, service, rpc)`, 429 on exceeding, token bucket with continuous refill
- Sending email: `smtp.send(to, subject, body)` — connection (`LINK_SMTP_URL`) and sender (`LINK_SMTP_FROM`) come from the process environment, never from rpc arguments. TLS via pure-rustls, same stack as the PostgreSQL driver. `smtp.sendToMany(to: String[], subject, body)` sends one message with one `RCPT TO` per recipient; `smtp.sendHtml(to: String[], subject, html)` sends an HTML body (`Content-Type: text/html`) to one or many recipients — `send` itself is unchanged
- Configurable CORS and fixed security headers: `--cors-origin <origin>` (repeatable, or `LINK_CORS_ORIGINS`) switches from open `*` to a real allowlist (exact match, echoed literal + `Vary: Origin`); every response — including errors and `stream` SSE — carries `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`
- `linkc fmt`, `linkc --help`, and the TypeScript client emitter for multi-service files all work correctly now
- Real password hashing: `crypto.hashPassword` is Argon2id (RFC 9106) with a random per-password salt, in PHC format; `verifyPassword` compares in constant time and still accepts hashes written by the previous version so existing users are not locked out
- Numeric randomness and constant-time comparison for user code: `crypto.randomInt(min, max)` gives a uniform `Int` in that inclusive range from the OS CSPRNG (rejection-sampled against modulo bias) — enough for a real OTP, unlike `randomToken`'s hex alphabet; `crypto.timingSafeEqual(a, b)` exposes the same constant-time comparison `verifyPassword` already used internally, for comparing a webhook secret or API key without a timing side-channel
- `.toString()` on `Int`/`Int64`/`Float`/`Bool` — explicit conversion, never automatic (same principle as `toInt64()`); `Bool` didn't have a single method before this. `response.setStatus` inside a `stream` is now a compile error instead of a silent no-op. `@route` supports a trailing catch-all segment (`:name*`) that captures zero or more remaining path segments joined by `/`, for variable-depth routes (docs, a CMS) — always `String`, never `Int`, and always the route's last segment
- Configurable password-hashing cost: `linkc serve --argon2-memory-kib <N> --argon2-iterations <N>` (or `LINK_ARGON2_MEMORY_KIB`/`LINK_ARGON2_ITERATIONS`) raises `crypto.hashPassword`'s Argon2id cost above the crate default; unset, behavior is unchanged. `crypto.isLegacyHash(hash: String) -> Bool` tells a caller whether a stored hash is the pre-Argon2id legacy format, for proactive rehashing on login instead of eyeballing the prefix. A PostgreSQL table with a preexisting 32- or 16-bit autoincrement id (`SERIAL`/`IDENTITY`, not just `BIGSERIAL`) no longer fails on its first insert — connecting already accepted it, reading the id column now does too
- Generated TypeScript contract, typed client, runtime validators, React hooks, Zod schemas, OpenAPI 3.1

**Does not work yet** — do not plan around these:

| Limitation | Detail |
|---|---|
| `@rate_limit` is per-process, in-memory | No persistence across restarts, no coordination across replicas if the same `.link` runs on more than one process; the client IP comes from the real TCP connection, never `X-Forwarded-For` (no trusted-proxy config yet, so behind a proxy this limits by the proxy's IP). |
| `request.rawBody()` needs a JSON body | Argument parsing runs before any rpc, regardless of how many parameters it declares, so a non-JSON body (form-encoded, XML) never reaches the rpc — a JSON webhook payload with extra fields works fine. |
| No CSP or HSTS | CSP depends on each page's actual content (no way to get a safe default without it); HSTS only makes sense over a connection that's already HTTPS, which `linkc serve` itself never is — both belong at the reverse proxy that terminates TLS in front of it. CORS allowlist entries are exact-match only, no wildcard subdomains. |
| `smtp` has no attachments, cc/bcc, or async send | `smtp.send`/`sendToMany`/`sendHtml` cover plain-text and HTML bodies to one or many recipients (as of this round), but none of the three take an attachment or a cc/bcc list, and all three are synchronous — a slow relay makes the whole (single-threaded) server slow for that request. |
| `--session-ttl` cleans up lazily | An expired session is only removed from memory the next time its token is used — one created and never touched again stays in memory until the process restarts. |
| Full user struct is not auto-loaded into session | `auth.currentRole()` and `auth.currentUserId()` expose the authenticated role and numeric user id, but loading the full `User` struct into memory is done explicitly via `db.users.find(uid)`. |
| Aggregation (`sumBy`/etc.) has no date bucketing | Can't `GROUP BY` a truncated date (monthly cohorts, for example) — grouping by a bare `Timestamp` field isn't accepted, and there's no truncation method to bucket one first. `Int64` support landed — see Works today. |
| Cross-instance `stream` push (LISTEN/NOTIFY) has real limits | A changed row over 8000 bytes (Postgres's own NOTIFY payload cap) doesn't propagate to other instances — it still publishes locally where it was written. NOTIFY is best-effort with no retry queue; an idle server can take up to 200ms to notice a remote change; each instance opens one extra Postgres connection just for LISTEN; SQLite doesn't participate at all. |
| No npm package | `link-lang` is not on the npm registry yet. GitHub releases work — see Installation below. |
| `linkc wasm` | Deliberately frozen at integer/boolean scalar functions; the production path is `wasm32-wasip1`. |
| The web playground compiles one file only | Runs the real lexer/parser/checker/codegen (compiled to `wasm32-unknown-unknown`), but bypasses the module loader: no `import` across files, and no `test` execution (that needs the native interpreter). |

## ⚡ Quick Start (10 Seconds)

### 1. Installation

#### 📦 Linux / macOS (Automatic 1-Line Installer)
```bash
curl -fsSL https://raw.githubusercontent.com/charlessonamericantrading/c-script-/master/install.sh | sh
```

#### 🪟 Windows (PowerShell)
```powershell
irm https://raw.githubusercontent.com/charlessonamericantrading/c-script-/master/install.ps1 | iex
```

#### 🌐 via NPM / npx

> **Not published yet.** `link-lang` is not on the npm registry. Use one of the installers
> above (they download the real prebuilt binary from
> [GitHub Releases](https://github.com/charlessonamericantrading/c-script-/releases)), or
> build from source:

```bash
git clone https://github.com/charlessonamericantrading/c-script-.git
cd c-script-/compiler
cargo build --release        # target/release/linkc
```

---

## 🤖 Built for Cursor & AI Agents (Grok, Claude, GPT)

Link ships the language rules in the format each tool reads, and **every example in those
rules is compiled by the real binary on every CI run** — which is what actually reduces
hallucination: what the agent reads is what the compiler accepts, not a promise.

- **[`AGENTS.md`](AGENTS.md)**: what Claude Code and Codex read first — repository map, real commands, project conventions, and the list of what is knowingly broken so an agent does not report it as a new finding.
- **[`llms.txt`](llms.txt) & [`llms-full.txt`](llms-full.txt)**: the condensed language reference, including the syntax mistakes every LLM makes (enum variants need braces as values, closures take no return type, `T?` can't be dereferenced via `if` — `match`/`??`/`.isSome()` narrow it instead).
- **`CLAUDE.md`, `.cursorrules`, `.cursor/rules/c-script.mdc`, `.windsurfrules`, `.github/copilot-instructions.md`**: the same rules in each tool's own format — `CLAUDE.md` is what Claude Code auto-loads on open; it's a thin pointer into `AGENTS.md`, not a duplicate.

Every c-script example in those files is compiled by the real binary on every CI run
(`compiler/tests/docs_examples.rs`), so what an agent reads here is what the compiler
actually accepts.
- **Install Editor Extension in 1-Click**:
  ```bash
  # For Cursor
  cursor --install-extension editors/vscode/c-script-vscode-1.0.0.vsix

  # For VS Code
  code --install-extension editors/vscode/c-script-vscode-1.0.0.vsix
  ```

---

## 🚀 Scaffold Your First App

Create a fullstack project with **Next.js 14**, **Vite+React**, or **Minimal Backend**:

```bash
# Next.js 14 App Router + Link Backend
linkc new my-app --template nextjs

# React + Vite Single Page Application
linkc new my-app --template vite

# Minimal Backend
linkc new my-app --template minimal
```

Then build and run:

```bash
cd my-app
linkc build main.link gen    # Generates typed contracts, client & OpenAPI
linkc serve main.link 3000   # Starts HTTP server with auto-migrating database
```

---

## 🧠 Language at a Glance

<!-- linkc:check -->
```rust
// 1. Data Models & Enums
type User = {
  id: Int,
  name: String,
  email: String,
  role: Role,
  created_at: Timestamp,
}

enum Role {
  Admin,
  Member,
}

// 2. Typed Database with Auto-Generated SQLite Schemas
db {
  users: User[],
}

// 3. RPC Services with RBAC Access Control & Live SSE Streams
service UserService {
  @requires(Role.Admin)
  rpc create(name: String, email: String) -> User {
    let new_user = db.users.insert(User {
      id: 0,
      name: name,
      email: email,
      role: Role.Member {},
      created_at: now(),
    });
    new_user
  }

  rpc list() -> User[] {
    db.users.all()
  }

  // 4. Real-time Streaming Push Endpoint (SSE)
  stream watchUsers() -> User {
    while true {
      db.users.subscribe()
    }
  }
}

// 5. Integrated Behavioral Tests
test "user creation and count" {
  let count = db.users.all().length();
  assert(count >= 0, "non-negative user count");
}
```

---

## 🛠️ Unified Tooling Suite

Link comes out-of-the-box with all the developer tooling you need:

| Command | Description |
|---|---|
| `linkc new <name> [--template nextjs\|vite\|minimal]` | Scaffolds a fullstack or minimal project |
| `linkc build <file.link> <outdir>` | Generates TS contracts, client, validators, React hooks, Zod & OpenAPI |
| `linkc serve <file.link> <port>` | Runs production HTTP server with embedded SQLite and SSE streaming |
| `linkc dev <file.link> <outdir> [port]` | Live hot-reloading dev mode with automatic server restart |
| `linkc test <file.link>` | Runs built-in behavioral tests in clean sandbox |
| `linkc fmt <file.link> [--check]` | Formats source code according to canonical rules |
| `linkc lint <file.link> [--fix]` | Analyzes code quality and auto-fixes warnings |
| `linkc doc <file.link> [outdir]` | Generates responsive, interactive HTML documentation |
| `linkc docker <file.link> [outdir]` | Generates production multi-stage Dockerfile & docker-compose.yml |
| `linkc wasm <file.link> <out.wasm>` | Compiles pure functions to standard WebAssembly |
| `linkc lsp` | Launches Language Server Protocol for VS Code / Cursor / Neovim |

---

## 🌐 Interactive Web Playground

[`playground/index.html`](playground/index.html) runs the real `linkc` lexer, parser, type
checker and code generators in your browser — compiled to `wasm32-unknown-unknown` via the
[`playground-wasm`](compiler/playground-wasm) crate, not a canned demo. It compiles a single
file (no cross-file `import`) and does not execute `test` (that needs the native interpreter —
run `linkc test` locally for that). To try it locally:

```bash
cd playground && python3 -m http.server 8000   # any static file server works
# open http://localhost:8000/ -- opening index.html directly via file:// will NOT work,
# browsers block fetch() of local files, and the wasm module loads via fetch()
```

To rebuild `playground/pkg/` after changing the compiler:

```bash
cd compiler/playground-wasm
cargo build --target wasm32-unknown-unknown --release
wasm-bindgen --target web --out-dir ../../playground/pkg --out-name playground_wasm \
  target/wasm32-unknown-unknown/release/playground_wasm.wasm
```

---

## 🧪 Testing & Quality Assurance

Link is verified by **679 automated unit, integration and CLI tests**, including tests that
spawn the real binary as a subprocess, drive a real HTTP server, and compile every c-script
example published in this repository's documentation:

```bash
cd compiler
cargo test
```

---

## 📄 License

MIT License — Copyright (c) 2026 Charlesson UK Consulting Group LTD. See [LICENSE](LICENSE).
