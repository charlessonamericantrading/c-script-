# GitHub Copilot Instructions for c-script / Link (.link)

## Project Context
This repository develops **c-script** (also referred to as **Link**), a compiled backend language engineered for complete, compile-time End-to-End Type Safety with TypeScript frontends.
- Extension: `.link`
- Compiler CLI: `linkc` (Rust-based)
- Target Outputs: TypeScript contracts (`contract.d.ts`), typed RPC client (`client.ts`), runtime payload guards (`validators.ts`), React hooks (`hooks.ts`), OpenAPI 3.1 specification (`openapi.json`), PostgreSQL DDL migrations (`schema.pg.sql`).

## Core Language Syntax & Idioms

### Types
- Primitives: `Int`, `Float`, `Int64`, `String`, `Bool`, `Timestamp` (UTC milliseconds with `now()`), `Void`.
- Collections: `T[]` (list), `Map<K, V>`, Tuples `(A, B)`.
- Nullable: `T?` (maps to `T | null`). Optional struct fields: `field?: T`.
- Structs:
  ```rust
  type User = {
    id: Int,
    name: String,
    email: String,
    role: Role,
    created_at: Timestamp,
    avatar?: String,
  }
  ```
- Enums & Discriminated Unions:
  ```rust
  enum Role { Admin, Member, Guest }

  enum ApiResult<T, E> {
    Ok { data: T },
    Err { code: Int, message: E },
  }
  ```

### Database (`db { ... }`)
Built-in SQLite persistence with compile-time checked operations:
```rust
db {
  users: User[],
}
```
CRUD operations:
- `db.users.all()`
- `db.users.find(id)`
- `db.users.insert(user)`
- `db.users.applyPatch(id, partial)`
- `db.users.delete(id)`
- `db.users.findWhere(|u: User| -> Bool { ... })`
- `db.users.deleteWhere(|u: User| -> Bool { ... })`
- `db.users.subscribe()` (for reactive push streams)

### Services, RPCs, and Streaming
```rust
service UserService {
  rpc list() -> User[] {
    db.users.all()
  }

  @authenticated
  rpc me() -> User? {
    db.users.find(1)
  }

  @requires(Role.Admin)
  rpc delete(id: Int) -> Bool {
    db.users.delete(id)
  }

  stream watch() -> User {
    while true {
      db.users.subscribe()
    }
  }
}
```

### Integrated Tests
```rust
test "can insert and find user" {
  let created = UserService.create("Alice", "alice@example.com");
  assert(created.id > 0, "ID assigned");
}
```

## CLI Usage
- `linkc build <file.link> <outdir>`: Compile backend into TypeScript SDK.
- `linkc test <file.link>`: Run built-in behavioral tests.
- `linkc serve <file.link> [port]`: Launch HTTP API server with SQLite persistence.
- `linkc dev <file.link> <outdir> [port]`: Hot-reloading watcher.
