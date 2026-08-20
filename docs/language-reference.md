# Referencia del Lenguaje: **c-script**

`c-script` es un lenguaje backend compilado diseñado para garantizar **isomorfismo de tipos y Type-Safety de extremo a extremo con TypeScript**.

---

## 1. Tipos Primitivos y Construcción de Estructuras

| Tipo c-script | Mapeo TypeScript | Descripción |
|---|---|---|
| `Int` | `number` | Entero firmado de 64 bits |
| `Float` | `number` | Flotante de 64 bits |
| `String` | `string` | Cadena UTF-8 |
| `Bool` | `boolean` | Valor booleano (`true` / `false`) |
| `Void` | `void` | Retorno vacío para RPCs |
| `T[]` | `T[]` | Lista/Array de elementos tipo `T` |
| `Map<K, V>` | `Record<K, V>` | Diccionario clave-valor |
| `T?` | `T \| null` | Tipo opcional / nullable |
| `field?: T` | `field?: T` | Clave opcional en un struct |

### Declaración de Structs

<!-- linkc:part -->
```rust
type User = {
  id: Int,
  name: String,
  email: String,
  role: Role,
  bio?: String,
  deletedAt: String?,
}
```

---

## 2. Enums Tipados y Tipos Algebraic (ADT)

### Enum Simple (Unión de Cadenas)

<!-- linkc:part -->
```rust
enum Role { Admin, Member, Guest }
```
*Genera en TypeScript:* `export type Role = "Admin" | "Member" | "Guest";`

### Enum con Datos (Unión Discriminada)

<!-- linkc:part -->
```rust
enum Result<T, E> {
  Ok  { value: T },
  Err { error: E },
}
```
*Genera en TypeScript:*
```typescript
export type Result<T, E> =
  | { type: "Ok"; value: T }
  | { type: "Err"; error: E };
```

---

## 3. Base de Datos Declarativa (`db { ... }`)

Cada colección declarada dentro del bloque `db` compila internamente a una tabla SQLite persistente en disco.

<!-- linkc:part -->
```rust
db {
  users: User[],
}
```

### Métodos Integrados de Colecciones DB
- `db.users.all()`: Devuelve todos los registros.
- `db.users.find(id: Int)`: Busca un registro por ID (`User?`).
- `db.users.insert(record)`: Inserta un nuevo registro.
- `db.users.applyPatch(id: Int, patch: Patch<User>)`: Aplica una actualización parcial.
- `db.users.delete(id: Int)`: Elimina un registro por ID (`Bool`).
- `db.users.deleteWhere(fn)`: Elimina registros que cumplan con el predicado.
- `db.users.findWhere(fn)`: Filtra registros por predicado.

---

## 4. Servicios RPC y Streaming SSE

<!-- linkc:part -->
```rust
service Users {
  rpc list(limit: Int = 20) -> User[] {
    db.users.all().take(limit)
  }

  @requires(Role.Admin)
  rpc remove(id: Int) -> Bool {
    db.users.delete(id)
  }

  stream watchAll() -> User {
    db.users.all()
  }
}
```

---

## 5. Autenticación y Decoradores

- `@authenticated`: Exige que la petición HTTP adjunte una sesión activa válida.
- `@requires(Role.Admin)`: Verifica que la sesión pertenezca al rol especificado.

---

## 6. Content-Type y URLs amigables

Un rpc que devuelve `String` puede combinar estos dos decoradores, además de
los de auth de arriba:

- `@content_type("text/html; charset=utf-8")`: el cuerpo de la respuesta es
  ese `String` tal cual, sin las comillas de JSON -- HTML, XML, CSV, texto
  plano. Detalle: [GRAMMAR.md §3.35](../GRAMMAR.md#335-content_type-respuestas-que-no-son-json--resuelto-alcance-acotado).
- `@route("/blog/:slug")`: URL adicional, amigable para un crawler, que
  convive con la dirección `/Servicio/rpc` de siempre sin reemplazarla. El
  segmento final `:nombre` se bindea a un parámetro `String`/`Int` del rpc
  con ese mismo nombre. Detalle y el patrón de proxy para lo que no cubre:
  [GRAMMAR.md §3.37](../GRAMMAR.md#337-routeblogslug-urls-amigables-para-seo--resuelto-alcance-acotado)
  y [`docs/routing.md`](routing.md).
