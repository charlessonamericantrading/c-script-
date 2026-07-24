# Especificación Formal: Gramática y Sistema de Tipos de **c-script**

> Complementa a [`PLAN.md`](./PLAN.md). Aquí se define, con precisión de implementación: la gramática léxica y sintáctica (EBNF), las reglas del type checker (bidireccional), la tabla de mapeo exhaustiva c-script→TypeScript, y la semántica de nullability y errores.
>
> Notación EBNF (estilo ISO/Wirth): `,` secuencia · `|` alternativa · `[x]` opcional (0 o 1) · `{x}` repetición (0 o más) · `"texto"` terminal literal.

---

## 1. Gramática Léxica

```ebnf
digit        = "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" ;
letter       = "a".."z" | "A".."Z" | "_" ;
identifier   = letter , { letter | digit } ;

int_lit      = digit , { digit } ;
float_lit    = digit , { digit } , "." , digit , { digit } ;
string_lit   = '"' , { string_char } , '"' ;
string_char  = ? cualquier carácter excepto '"' o '\' ? | escape_seq ;
escape_seq   = "\" , ( "n" | "t" | "\" | '"' | "u" , hex4 ) ;
bool_lit     = "true" | "false" ;

line_comment  = "//" , { ? cualquier carácter excepto newline ? } ;
block_comment = "/*" , { ? cualquier carácter ? } , "*/" ;

keyword      = "type" | "enum" | "service" | "rpc" | "stream" | "match"
             | "import" | "from" | "pub" | "const" | "fn" | "let" | "mut"
             | "return" | "if" | "else" | "true" | "false" | "null" ;
```

**Reservado pero fuera del v0 de la gramática:** `async`, `await`, `trait`, `impl` — el modelo de concurrencia y de polimorfismo ad-hoc se diseña en una iteración posterior (ver PLAN.md §4, Fase 1).

---

## 2. Gramática Sintáctica

### 2.1 Programa e ítems de nivel superior

```ebnf
program      = { item } ;
item         = import_decl | type_decl | enum_decl | service_decl | const_decl | fn_decl ;

import_decl  = "import" , "{" , ident_list , "}" , "from" , string_lit , ";" ;
ident_list   = identifier , { "," , identifier } ;

type_decl    = "type" , identifier , [ type_params ] , "=" , type_expr , [ ";" ] ;
type_params  = "<" , identifier , { "," , identifier } , ">" ;

enum_decl    = "enum" , identifier , [ type_params ] , "{" , variant_list , "}" ;
variant_list = variant , { "," , variant } , [ "," ] ;
variant      = identifier , [ "{" , field_list , "}" ] ;

field_list   = field , { "," , field } , [ "," ] ;
field        = identifier , [ "?" ] , ":" , type_expr ;

const_decl   = "const" , identifier , ":" , type_expr , "=" , expr , ";" ;

service_decl = "service" , identifier , "{" , { member_decl } , "}" ;
member_decl  = rpc_decl | stream_decl ;
rpc_decl     = "rpc" , identifier , "(" , [ param_list ] , ")" , "->" , type_expr , block ;
stream_decl  = "stream" , identifier , "(" , [ param_list ] , ")" , "->" , type_expr , block ;
param_list   = param , { "," , param } ;
param        = identifier , ":" , type_expr , [ "=" , expr ] ;

fn_decl      = "fn" , identifier , "(" , [ param_list ] , ")" , "->" , type_expr , block ;
```

**El `;` de `type_decl` es opcional.** Un `type X = { ... }` termina en `}`; exigir además un `;` es la misma incomodidad que Rust/Go evitan después de un `struct`. `const_decl`/`let_stmt`/`return_stmt` sí exigen `;` — su valor no siempre termina en `}` (`const MAX: Int = 100`) y v0 no tiene todavía operadores infijos que hicieran innecesaria la marca de fin de sentencia.

**`fn` — funciones libres, no expuestas como RPC.** A diferencia de `rpc`/`stream`, no vive dentro de un `service` y no entra al contrato `.d.ts` — es lógica interna del backend (p. ej. `validate` llamada desde un `rpc`). Misma forma que `rpc_decl` porque comparten `param_list`/`block`; la diferencia es de visibilidad, no de sintaxis.

**`import_decl` — RESUELTO (multi-archivo + package manager mínimo, `compiler/src/modules.rs`).** `import { X, Y } from "./otro.link";` ya resuelve de verdad: cada `.link` alcanzado se lexea/parsea, y sus ítems (menos los `Item::Import` ya resueltos) se funden en un solo `Program` antes de llegar al checker — que sigue viendo un único árbol, sin ningún concepto nuevo de "archivo".

```
type Point = { x: Int, y: Int }        // b.link

import { Point } from "./b.link";      // a.link
fn origin() -> Point { Point { x: 0, y: 0 } }
```

- **`from` relativo (`./`/`../`) vs. nombre pelado.** Un `from` que empieza con `./` o `../` es una ruta relativa al archivo que importa. Un nombre pelado (`import { X } from "shapes";`) se busca en `dependencies` de un `link.json` en el directorio del archivo de entrada — la raíz del proyecto, sin buscar hacia arriba en el árbol (eso es útil para monorepos, un caso más avanzado que v0 no necesita):
  ```json
  { "dependencies": { "shapes": "./libs/shapes.link" } }
  ```
  **Sin lockfile en v0**: con dependencias puramente por ruta no hay versión ni conflicto que "lockear" todavía — un lockfile acá sería aparentar robustez sobre un problema que no existe (la misma trampa que la fila `Int64` fantasma que se encontró y sacó de este documento en la auditoría final).
- **Ciclos se rechazan con un error claro** (no un stack overflow silencioso ni un colgado): se detectan sobre la pila de imports que se está resolviendo en ese momento, no sobre "todo lo que ya se vio alguna vez" (eso rompería el caso diamante de abajo).
- **Sin re-exports, a propósito.** Un import se valida contra los ítems NATIVOS del archivo importado — nunca contra su cierre ya fusionado con SUS PROPIOS imports. Si A importa `X` de B, y B a su vez importa `X` de C (pero no declara `X` él mismo), el import de A **falla**: B nunca declaró `X` nativamente, así que no hay nada que A pueda "heredar" a través de B. Si hiciera falta lo contrario, hay que importar `X` directamente de C.
- **Namespaces cruzados.** `types`/`enums`/`fns`/`const`s son namespaces independientes (el checker los guarda en tablas separadas) — un import busca el nombre en los cuatro y alcanza con que matchee en uno; error solo si no matchea en ninguno. `service` queda afuera: no es algo que se referencie por nombre en ningún otro lado del lenguaje, así que "importar un service" no tiene un significado real todavía.
- **Sin visibilidad real (`pub`/privado).** El `Program` final que llega al checker es la unión plana de los ítems nativos de todo archivo alcanzado transitivamente — el import valida "¿existe ese nombre en ESE archivo puntual?" pero no oculta nada de los demás archivos del cierre entre sí (dos archivos no relacionados por ningún import, pero alcanzados por el mismo cierre transitivo, pueden verse los símbolos entre sí sin querer). Implementar visibilidad real necesitaría un scoping por archivo en el checker, que hoy no tiene ningún concepto de "de qué archivo vino este símbolo" — una extensión más grande, correctamente fuera de alcance acá.
- **Detección de colisiones, de paso.** Al construir esto se encontró que dos `type`/`enum`/`fn` con el mismo nombre en el mismo `Program` (antes, solo pasaba dentro de un único archivo; con imports, entre archivos) ganaban por orden de inserción, en silencio — `checker.rs::build_symbols` ahora rechaza el duplicado explícitamente.

### 2.2 Expresiones de tipo — y la trampa del postfix

```ebnf
type_expr     = union_type ;
union_type    = postfix_type , { "|" , postfix_type } ;
postfix_type  = primary_type , { type_postfix_op } ;
type_postfix_op = "?" | "[" "]" ;

primary_type  = identifier , [ type_args ]
              | "{" , field_list , "}"                     (* struct inline *)
              | "{" , type_expr , ":" , type_expr , "}"     (* map: {K: V} *)
              | "(" , type_expr , ")"                       (* agrupación *)
              | "(" , type_expr , "," , [ type_list ] , ")"  (* tupla, requiere ≥1 coma *)
              | "(" , [ type_list ] , ")" , "->" , type_expr (* tipo función *)
              ;
type_args     = "<" , type_expr , { "," , type_expr } , ">" ;
type_list     = type_expr , { "," , type_expr } ;
```

**El parser v0 no implementa la forma literal `{ type_expr : type_expr }` para mapas.** Es una ambigüedad real, no un detalle de implementación: `{ id: Int }` es sintácticamente idéntico tanto para "struct de un campo sin coma final" como para "map de `id` (tipo) a `Int`" — nada en la gramática los distingue sin recurrir a qué identificadores son "tipos conocidos", que no es información disponible en tiempo de parseo. Se resuelve con `Map<K, V>` (named type genérico ordinario, sin gramática especial) hasta que se justifique una sintaxis dedicada.

`★ Insight ─────────────────────────────────────`
Dos decisiones sutiles del diseño de gramática, no obvias hasta que las rompes:

1. **El orden de `?` y `[]` importa y por eso `postfix_type` es una lista, no dos campos fijos.** `T[]?` se parsea como `primary=T`, luego postfix `[]`, luego postfix `?` → `Optional(List(T))` ("array que puede ser null"). `T?[]` se parsea al revés → `List(Optional(T))` ("array de elementos que pueden ser null"). Son tipos completamente distintos y ambos son legítimos — la gramática tiene que permitir encadenarlos en cualquier orden, no fijar uno.
2. **`(A)` vs `(A, B)` es la misma ambigüedad clásica de Python con tuplas de un elemento.** Por eso exijo `,` obligatoria en `type_list` de la producción de tupla: `(A)` es agrupación pura, `(A,)` sería la tupla de un elemento (si algún día hace falta). Sin esta regla, `(Int)` sería ambiguo entre "el tipo Int entre paréntesis" y "una tupla de un Int".
`─────────────────────────────────────────────────`

### 2.3 Expresiones, sentencias y patrones (cuerpo de un `rpc`)

```ebnf
block        = "{" , { stmt } , [ expr ] , "}" ;
stmt         = let_stmt | assign_stmt | expr_stmt | return_stmt ;
let_stmt     = "let" , [ "mut" ] , identifier , [ ":" , type_expr ] , "=" , expr , ";" ;
assign_stmt  = identifier , "=" , expr , ";" ;
return_stmt  = "return" , [ expr ] , ";" ;
expr_stmt    = expr , ";" ;

expr         = match_expr | if_expr | or_expr ;

if_expr      = "if" , or_expr , block , "else" , ( if_expr | block ) ;

(* Precedence climbing estándar, de menor a mayor precedencia. Cada nivel
   solo delega al siguiente si no encuentra su propio operador — así `&&`
   liga más fuerte que `||`, `+` más fuerte que comparación, etc. *)
or_expr           = and_expr , { "||" , and_expr } ;
and_expr          = equality_expr , { "&&" , equality_expr } ;
equality_expr     = relational_expr , { ( "==" | "!=" ) , relational_expr } ;
relational_expr   = additive_expr , { ( "<" | "<=" | ">" | ">=" ) , additive_expr } ;
additive_expr     = multiplicative_expr , { ( "+" | "-" ) , multiplicative_expr } ;
multiplicative_expr = unary_expr , { ( "*" | "/" | "%" ) , unary_expr } ;
unary_expr        = ( "!" | "-" ) , unary_expr | postfix_expr ;

postfix_expr = primary_expr , { postfix_op } ;
postfix_op   = "." , identifier                   (* acceso a campo / método: db.users *)
             | "." , int_lit                       (* acceso posicional a tupla: t.0 *)
             | "(" , [ arg_list ] , ")"            (* llamada: f(x), o encadenada: db.users.find(id) *)
             | "[" , expr , "]" ;                  (* indexado: arr[i] *)
arg_list     = expr , { "," , expr } ;

primary_expr = struct_or_variant_lit
             | array_lit
             | tuple_lit
             | identifier
             | int_lit | float_lit | string_lit | bool_lit | "null"
             | "(" , expr , ")" ;

struct_or_variant_lit = identifier , [ "." , identifier ] , "{" , [ field_init_list ] , "}" ;
field_init_list        = field_init , { "," , field_init } ;
field_init             = identifier , ":" , expr ;

array_lit = "[" , [ expr , { "," , expr } , [ "," ] ] , "]" ;

(* misma ambigüedad y misma solución que en tipos (§2.2): (a) es agrupación,
   (a,) tupla de 1, (a,b) tupla de 2+ -- requiere ≥1 coma para NO ser Paren. *)
tuple_lit = "(" , expr , "," , [ expr , { "," , expr } ] , ")" ;

match_expr   = "match" , expr , "{" , { match_arm } , "}" ;
match_arm    = pattern , [ "if" , expr ] , "=>" , ( expr , "," | block ) ;

pattern      = pattern_atom , { "|" , pattern_atom } ;        (* or-pattern *)
pattern_atom = identifier                                    (* binding, incl. "_" *)
             | identifier , "." , identifier ,
               [ "{" , field_pattern_list , "}" ]             (* Enum.Variant { .. } *)
             | literal_pattern ;
literal_pattern = int_lit | "-" , int_lit | str_lit | "true" | "false" ;
field_pattern_list = field_pattern , { "," , field_pattern } ;
field_pattern       = identifier , [ ":" , pattern ] ;        (* shorthand: `x` ≡ `x: x` *)
```

**`[]` vacío solo en modo chequeo.** Sin elementos no hay de dónde sintetizar el tipo — `[]` únicamente es válido donde el contexto ya da un tipo esperado `T[]` (ej. `let xs: Int[] = [];`), igual que `Result.Ok`/`Result.Err` (§3.5). Un array no vacío sí sintetiza: se infiere del primer elemento y se chequea que el resto coincida.

**Indexar fuera de rango es un error de runtime, no `null`.** `arr[i]` con `i` fuera de rango falla en tiempo de ejecución en vez de devolver un valor nulo silencioso — la alternativa (devolver `T?` siempre, incluso cuando `T` no es nullable) ensuciaría el tipo de CADA acceso a un array por un caso excepcional. Es la misma decisión que Rust (panic) y distinta de la de JS (`undefined`).

**`t.0.1` no encadena — limitación conocida del lexer, no un error silencioso.** El lexer decide si `0.1` es un solo `float_lit` o dos `int_lit` separados por un `.` mirando únicamente los caracteres, sin saber que venía de un acceso posicional a tupla — así que `t.0.1` se lexea como `Ident("t")`, `Dot`, `Float(0.1)`, no como dos accesos encadenados. Rust tiene el mismo problema de fondo y lo resuelve con una regla especial en su lexer; acá, mientras tanto, la forma de acceder a una tupla anidada es `let inner = t.0; inner.1;`.

**Nota de implementación — lookahead de `struct_or_variant_lit`:** distinguir `Result.Ok { value: u }` (literal de variante) de `db.users` (acceso encadenado, sin `{` después) requiere que el parser mire hasta 2 tokens adelante antes de decidir. No es una ambigüedad del lenguaje — es la misma clase de decisión que "no struct literals en la condición de un `if`" en Rust: una regla del parser, no del árbol de derivación.

**`if` siempre exige `else`.** Es una expresión total: si `if` pudiera faltar el `else`, ¿qué tipo tendría la rama ausente? Rust resuelve esto dándole tipo `()` al `if` sin `else` y exigiendo que solo se use donde `()` es válido; acá se simplifica exigiendo `else` siempre. Un condicional de solo-efecto se escribe `if cond { ... } else { }` explícito.

**Mutabilidad — por qué `let mut` no alcanza sin `assign_stmt`.** Antes de `assign_stmt`, `mut` era una palabra reservada sin ningún efecto: se podía escribir `let mut x = 1`, pero no había ninguna sentencia que permitiera cambiar `x` después. El checker exige que el nombre a la izquierda de un `assign_stmt` ya exista en el scope **y** haya sido declarado con `mut` — asignar a un binding inmutable, o a un nombre que no existe, es un error de tipos (checker.rs), no algo que el parser rechace. `assign_stmt` solo cubre variables simples (`x = ...`) — todavía no hay mutación de campos (`obj.campo = ...`) ni de posiciones de array (`arr[i] = ...`).

Or-patterns, patrones de literales y guardas ya están resueltos — ver §3.3 para el algoritmo de exhaustividad extendido y sus límites de alcance explícitos (en particular, ninguna alternativa de un `p1 | p2` puede introducir bindings).

---

## 3. Sistema de Tipos

### 3.1 Juicios bidireccionales

Dos juicios, como en Rust/TS/Swift modernos:

- `Γ ⊢ e ⇒ T` — **síntesis**: a partir de `e`, se infiere `T`.
- `Γ ⊢ e ⇐ T` — **chequeo**: se verifica que `e` es válido contra un `T` ya conocido.

La regla que conecta ambos mundos:

```
Γ ⊢ e ⇒ T'      T' <: T
─────────────────────────  (Subsunción)
Γ ⊢ e ⇐ T
```

Reglas clave:

```
─────────────────────────  (Lit-Int)
Γ ⊢ n ⇒ Int

─────────────────────────  (Lit-Str)
Γ ⊢ "s" ⇒ String

x : T ∈ Γ
─────────────────────────  (Var)
Γ ⊢ x ⇒ T

f : (T1, .., Tn) -> T ∈ Γ      Γ ⊢ eᵢ ⇐ Tᵢ  (para cada i)
────────────────────────────────────────────────────────  (Call)
Γ ⊢ f(e1, .., en) ⇒ T

Γ ⊢ e ⇐ T
─────────────────────────  (Struct-Lit, modo chequeo — necesita T objetivo
Γ ⊢ Nombre{...} ⇐ T          para saber qué campos son válidos)
```

**¿Por qué bidireccional y no solo inferencia (Hindley-Milner)?** Porque `rpc` declara su tipo de retorno explícitamente (`-> User`), lo cual da un "ancla" de tipo esperado en cada punto de entrada. Eso simplifica enormemente el checker: no hace falta unificación global, con propagar el tipo esperado hacia abajo (chequeo) y sintetizar hacia arriba en las hojas (literales, variables) alcanza. Es el mismo enfoque que TypeScript usa internamente para inferencia contextual.

### 3.2 Subtipado: estructural para `type`, nominal para `enum`

```
∀ (k: Tₖ) ∈ campos(T')  ∃ (k: Sₖ) ∈ campos(S)     Sₖ <: Tₖ
─────────────────────────────────────────────────────────  (Struct-Width-Depth)
S <: T'
```

- **`type` es estructural** (como TS): si `S` tiene al menos los campos de `T'` con tipos compatibles, `S <: T'`. Esto es necesario para que el mapeo a TS sea 1:1 — TS *solo* entiende structural typing para tipos objeto.
- **`enum` es nominal**: dos enums con las mismas variantes pero nombres distintos NO son intercambiables. Esto también refleja TS: una unión discriminada se distingue por el tipo declarado, no por su forma accidental.

### 3.3 Exhaustividad en `match` — RESUELTO (enum + literales, or-patterns, guardas)

Algoritmo base sobre un scrutinee **enum** (incluye `Result<T,E>` y enums genéricos instanciados):

```
cubierto := ∅
para cada arm SIN guard en match:      -- un arm CON guard nunca cuenta, ver más abajo
  si arm.pattern == "_" (o un bind con nombre):
    cubierto := todas_las_variantes(EnumType)
  si no:
    cubierto := cubierto ∪ variantes_de(arm.pattern)   -- un Or aporta la unión de sus alternativas

error si cubierto ≠ todas_las_variantes(EnumType)
```

Esto es lo que hace que el compilador de c-script, igual que Rust, **rechace un `match` que no cubre un nuevo variant** añadido a un enum. Es una propiedad valiosa por sí misma (no solo para el puente con TS): añadir un caso a `Result` rompe la compilación en *todos* los `match` que lo consumen, en el backend, no solo en el frontend.

**Extensión: `match` también acepta un scrutinee `Int`/`String`/`Bool`** (antes, `match` exigía un enum a secas — matchear un primitivo directamente no tenía ninguna forma de patrón que no fuera un bind, así que era, en los hechos, imposible de usar con más de un arm real). El algoritmo para este caso es distinto porque `Int`/`String` tienen un espacio de valores no enumerable:

```
error si NO hay un catch-all (bind sin guard) entre los arms
   Y  NO ( tipo == Bool  Y  'true' y 'false' están ambos cubiertos por un literal sin guard )
```

`Bool` es, en los hechos, un enum de dos variantes — es el único tipo no-enum donde un conjunto de literales, sin catch-all, alcanza para ser exhaustivo. `Int`/`String` **siempre** necesitan un arm final sin guard (`_ => ...` o un bind con nombre) — ningún conjunto finito de literales agota sus valores posibles.

```
fn describe(n: Int) -> String {
  match n {
    1 | 2 => "bajo",     // or-pattern: aporta {1, 2} a la cobertura
    -1    => "negativo", // literal negativo: un solo token de patrón, no unario aplicado a un patrón
    _     => "otro",     // catch-all obligatorio -- Int no es enumerable
  }
}
```

**Guardas (`pattern if cond => body`) nunca descartan exhaustividad por sí solas.** La condición podría ser `false` en runtime, así que un arm con guard —aunque su patrón sería, sin el guard, un catch-all o cubriría el último variant que faltaba— **no cuenta** para el algoritmo de arriba: sigue habiendo que cubrir ese caso con algún otro arm sin guard. En runtime, si el patrón matchea pero el guard da `false`, la búsqueda **continúa con el siguiente arm** (igual que Rust), no se trata como "sin match":

```
fn classify(n: Int) -> String {
  match n {
    x if x > 100 => "grande",
    x if x > 0   => "positivo chico",
    _            => "cero o negativo",
  }
}
```

El guard ve las variables que el propio patrón acaba de ligar — `Setting.Level { value } if value > 10 => ...` puede usar `value` en la condición — y debe sintetizar `Bool`, como cualquier condición (§3.7).

**Or-patterns (`p1 | p2 | ...`) — alcance v0: ninguna alternativa puede introducir bindings.** La regla completa de otros lenguajes (cada alternativa debe ligar exactamente las mismas variables, con el mismo tipo) es la parte cara de implementar or-patterns; acá se evita ese problema entero prohibiendo bindear del todo dentro de un `Or` — cubre el caso común (combinar variantes unitarias o literales que comparten cuerpo) sin esa complejidad:

```
enum Status { Active, Paused, Cancelled }
match s {
  Status.Active | Status.Paused => "en curso",   // ok: ninguna alternativa liga nada
  Status.Cancelled => "cancelado",
}

enum Shape { Circle { r: Int }, Square { r: Int } }
match sh {
  Shape.Circle { r } | Shape.Square { r } => r,  // ERROR: cada alternativa intenta ligar 'r'
}
```

**Deliberadamente fuera de alcance: literales `Float`, y matchear un `T?` directamente.** Sin patrón `Float`: comparar floats por igualdad exacta es una trampa conocida (`0.1 + 0.2 != 0.3`) — Rust llegó a la misma conclusión y terminó prohibiéndolo en sus propios patrones (antes era solo un warning). Sin `null` como patrón: eso requeriría que `match` acepte un scrutinee `Optional(T)`, una extensión relacionada pero distinta que queda para más adelante — hoy la forma de testear nullability sigue siendo `== null` / `!= null` dentro de un `if/else` (§3.7), que ya funciona porque `Null <: Optional(_)` (§3.4) hace que la comparación tipe.

### 3.4 Nullability (`T?`) — RESUELTO (default aplicado)

Regla de subtipado (se deriva de "opcional es más permisivo"):

```
S <: T
─────────────  (Optional-Widen)
S <: T?
```

**Decisión:** el default recomendado en `PLAN.md` §8.3, aplicado sin pasar por el TODO del usuario (ver `examples/decision-nullability.ts` para el resultado).

| Sintaxis c-script | Significado | TypeScript | Wire (JSON) |
|---|---|---|---|
| `x: T` | requerido, nunca ausente ni null | `x: T` | clave siempre presente |
| `x: T?` | la clave siempre está; el **valor** puede ser null | `x: T \| null` | clave presente, valor `null` |
| `x?: T` | la **clave** puede no existir | `x?: T` | clave omitida si ausente |
| `x?: T?` | ambos a la vez | `x?: T \| null` | combinación de las dos anteriores |

**PATCH parcial — `Patch<T>`:** utilitario análogo a `Partial<T>` de TS. Vuelve **todos** los campos de `T` del tipo `?:` (clave omitible), preservando si además eran `T?` (nullable). Esto resuelve exactamente la pregunta que planteaba `decision-nullability.ts` ("¿cómo distingo *no lo toques* de *bórralo*?"):

- Campo **no nullable** en la base (`x: T`) → en el patch, `x?: T`: omitido = no tocar, presente = fijar. No se puede limpiar (nunca fue nullable).
- Campo **nullable** en la base (`x: T?`) → en el patch, `x?: T | null`: omitido = no tocar, `null` = limpiar, valor = fijar.
- Campo **opcional-al-crear** (`x?: T`) → en el patch, sigue `x?: T`: omitido = no tocar, presente = fijar. Si además necesitás poder limpiarlo, la base tiene que declararse `T?`, no solo `?:` — la distinción tiene consecuencias reales, no es solo estilo.

```typescript
// rpc update(id: Int, patch: Patch<User>) -> User
type PatchUser = {
  name?: string;         // bio no nullable en la base -> solo fijar u omitir
  bio?: string | null;   // si bio fuera T? en la base -> se puede limpiar con null
};
declare function updateUser(id: number, patch: PatchUser): Promise<User>;

updateUser(42, { name: "Ada" }); // no toca bio ni deletedAt
```

Esta convención sigue el mismo principio que **JSON Merge Patch (RFC 7386)** y el patrón habitual de inputs nullable en GraphQL — no es una invención ad-hoc, es la solución estándar a este problema exacto.

### 3.5 Manejo de errores en `rpc` — RESUELTO (default aplicado)

**Decisión:** `Result<T, E>`, con `E` siempre un `enum` (así la exhaustividad de `match`, §3.3, aplica también a los errores). Razón: TypeScript no tipa lo que se lanza (`catch (e)` siempre es `unknown`), así que una excepción rompe la tesis central del proyecto justo en el peor lugar. `Result<T,E>` es coherente con el resto del lenguaje (ya hay `enum` + `match` exhaustivo — un error no es más que otro ADT) y es la única opción que preserva "rompe en compilación" para errores, no solo para el happy path (comparativa completa con la alternativa de excepciones tipadas: ver `examples/decision-errors.ts`).

```
enum ValidationError {
  InvalidEmail { field: String },
  TooShort     { field: String, min: Int },
}

enum ValidateResult {
  Ok  { value: NewUser },
  Err { error: ValidationError },
}

fn validate(input: NewUser) -> ValidateResult {
  ValidateResult.Ok { value: input }   // placeholder; reglas reales en checker/runtime
}

service Users {
  rpc create(input: NewUser) -> Result<User, ValidationError> {
    match validate(input) {
      ValidateResult.Ok  { value: v } => Result.Ok  { value: db.users.insert(v) },
      ValidateResult.Err { error: e } => Result.Err { error: e },
    }
  }
}
```

Nótese el patrón `ValidateResult.Ok { value: v }`, no `Ok(v)`: los variants de c-script se declaran con campos nombrados (§3.5 arriba), así que su patrón es struct-style (§2.3), no posicional al estilo Rust `Some(x)`. Es una consecuencia directa de la gramática de patrones, no una elección nueva.

Mapeo a TS (reusa la regla general de enum-con-datos, §4):

```typescript
type ValidationError =
  | { type: "InvalidEmail"; field: string }
  | { type: "TooShort"; field: string; min: number };

type Result_User_ValidationError =
  | { type: "Ok"; value: User }
  | { type: "Err"; error: ValidationError };

declare function create(input: NewUser): Promise<Result_User_ValidationError>;
```

```typescript
const result = await usersClient.create(input);
if (result.type === "Ok") {
  console.log(result.value.id);
} else {
  switch (result.error.type) {           // exhaustivo, TS avisa si falta un caso
    case "InvalidEmail": /* ... */ break;
    case "TooShort":     /* ... */ break;
  }
}
```

**Errores de transporte vs errores de dominio:** el cliente generado **nunca** lanza (`throw`) para un error que el `rpc` declaró en su `Result<T,E>` — esos siempre vuelven como valor. El cliente **sí** puede lanzar `LinkTransportError` para fallos fuera del contrato de dominio (red caída, 5xx, timeout) — son excepcionales por definición, no algo que el backend predijo. Es la misma línea divisoria que separa `Result`/`Option` de `panic!` en Rust.

### 3.6 Genéricos definidos por el usuario — RESUELTO (monomorfización)

`type Box<T> = { value: T }` / `enum Option<T> { Some{value:T}, None }` ya funcionan: se instancian (`Box<Int>`), se construyen, se accede a sus campos, y se hace `match` exhaustivo sobre enums genéricos.

**Cómo se resuelve una instanciación.** `resolve_type` arma un *subst* (`type_param -> tipo concreto`, ej. `{"T": Int}`) y lo aplica recursivamente al cuerpo de la declaración — es monomorfización real, no type erasure: `Box<Int>` y `Box<String>` son dos tipos concretos distintos para el checker, tal como se recomendaba acá antes de implementarlo. La instanciación queda **opaca** (`Type::Generic(nombre, args)`, sin expandir) hasta que hace falta la forma real — field access, construcción, match — el mismo patrón que ya usaban `Result<T,E>`/`Patch<T>`/`Map<K,V>`.

**Construcción: solo en modo chequeo, igual que `Result`.** `Box { value: 5 }` no trae los argumentos de tipo en su sintaxis (no hay `Box<Int> { value: 5 }`) — así que, igual que `Result.Ok`, necesita un tipo esperado ya instanciado viniendo del contexto (anotación de `let`, tipo de retorno declarado, etc.). Sintetizar `Box { value: 5 }` sin ese contexto es un error explícito: no hay de dónde sacar el argumento de tipo.

**Decisión: la comparación de un genérico ya instanciado es NOMINAL, no estructural.** `Box<Int>` y un struct suelto `{ value: Int }` con la misma forma **no** son intercambiables, aunque `type` sin genéricos sí es estructural (§3.2). Es una simplificación deliberada: sostener estructural-a-través-de-un-genérico exigiría que `is_subtype` pudiera "ver a través" de una instanciación opaca, lo cual necesita acceso a las tablas de símbolos que hoy no tiene (es una función libre, sin ese contexto) — y en la práctica varios lenguajes con tipado estructural (la propia TypeScript incluida, en varios casos con genéricos) tampoco garantizan esa equivalencia en general.

**La declaración se emite como genérico real de TypeScript, no monomorfizada.** A diferencia del checker (que sí monomorfiza internamente), el `.d.ts` emite `export interface Box<T> { value: T; }` **una sola vez** — TypeScript ya tiene genéricos nativos, así que no hace falta (ni conviene) generar una interface por cada instanciación usada. Una referencia a `Box<Int>` en una firma se emite como `Box<number>`, dejando que el propio `tsc` haga la instanciación.

### 3.7 Operadores e `if/else`

Sin coerción implícita — a diferencia de JS, `1 + "1"` es un error de tipos, no `"11"`. Cada operador exige que ambos operandos ya tengan el tipo correcto (vía la regla de Subsunción de §3.1); si hace falta convertir, es explícito (`.toFloat()`/`.toInt()`, §3.8), nunca automático.

| Operador | Regla | Resultado |
|---|---|---|
| `+` | ambos `Int`, ambos `Float`, o ambos `String` (concatenación) | mismo tipo que los operandos |
| `- * /` `%` | ambos operandos `Int`, o ambos `Float` (no mezclados) | mismo tipo que los operandos |
| `- ` unario | operando `Int` o `Float` | mismo tipo |
| `== !=` | operandos de tipos mutuamente compatibles (mismo primitivo, o mismo enum nominal) | `Bool` |
| `< <= > >=` | ambos operandos `Int`, o ambos `Float` | `Bool` |
| `&& \|\| !` | operando(s) `Bool` | `Bool` |

`if cond { A } else { B }` es de **modo chequeo**, igual que `match` (§3.1): no tiene un tipo propio que sintetizar, necesita el tipo esperado del contexto para verificar que `cond ⇐ Bool` y que tanto `A` como `B` chequean contra ese mismo tipo esperado. Es la misma familia de regla que `match` — control de flujo condicional siempre se chequea top-down, nunca se infiere bottom-up.

### 3.8 Métodos builtin sobre primitivos

`x.metodo()` sobre un valor primitivo (no un struct/enum declarado) no es acceso a un campo real — es azúcar reconocida por nombre y tipo del receptor, resuelta ANTES de intentar el `FieldAccess` genérico (que fallaría: `Int`/`Float`/`String` no son `Struct` ni `Dynamic`). Es el mismo mecanismo que ya resolvía `db.users.find(...)` (`checker.rs`/`runtime/mod.rs`, `BoundMethod`), generalizado a primitivos.

| Método | Receptor | Resultado | Nota |
|---|---|---|---|
| `.toFloat()` | `Int` | `Float` | conversión exacta |
| `.toInt()` | `Float` | `Int` | trunca hacia cero (`3.9`→`3`, `-3.9`→`-3`), igual que `as` en Rust — no redondea |
| `.length()` | `String` | `Int` | cantidad de caracteres |
| `.contains(s: String)` | `String` | `Bool` | substring, no regex |

No hay coerción implícita en ningún operador (§3.7) — estas son las únicas conversiones numéricas, y son siempre explícitas. `.length()`/`.contains()` son método, no propiedad (`x.length`, sin paréntesis) — consistencia con `.toFloat()`/`.toInt()` importó más acá que imitar la convención de propiedad de JS/TS.

### 3.9 Uniones de tipo (`A | B`) — RESUELTO (subtipado de flujo de valor, sin angosto)

`x: Int | String` ya se resuelve, se acepta como tipo de parámetro/campo, y se emite como la unión nativa de TypeScript (`number | string`). La gramática (§2.2) ya traía `union_type` desde el principio; lo que faltaba era que el checker supiera qué hacer con un `TypeExpr::Union` en vez de devolver un error fijo.

**Regla de subtipado — dos direcciones, no una:**

```
S <: Tᵢ   para algún i ∈ 1..n
──────────────────────────────  (Union-Intro, "a la derecha")
S <: (T₁ | ... | Tₙ)

∀i ∈ 1..n.  Sᵢ <: T
──────────────────────────────  (Union-Elim, "a la izquierda")
(S₁ | ... | Sₙ) <: T
```

La primera es la que cubre el caso real más común: un valor concreto (`Int`) fluye hacia un parámetro/campo tipado como unión con solo encajar en UNO de los miembros. La segunda es la que hace que una unión sea, a su vez, subtipo de otra unión más ancha (`Int | String <: Int | String | Bool`) — cada miembro de la izquierda tiene que encajar en algo de la derecha.

```
type Event = { payload: Int | String }

fn accept(x: Int | String) -> Void {}

fn f() -> Void {
  accept(1);        // Int <: Int | String -- ok (Union-Intro)
  accept("hola");   // String <: Int | String -- ok (Union-Intro)
}
```

Emitido:

```typescript
export interface Event {
  payload: number | string;
}
```

**Lo que NO está implementado: angostar (narrowing).** Una vez que un valor entra a una unión, no hay forma de recuperar el miembro concreto después: no existe ningún operador `is`/`typeof`, y `match` (§3.3) solo opera sobre `enum` declarados — no sobre un `TypeExpr::Union` crudo. No hay ninguna construcción sintáctica en la que "angostar" pudiera engancharse todavía. En la práctica, `x: Int | String` sirve para **aceptar** cualquiera de los dos, pero dentro del cuerpo de la función no hay forma de preguntar cuál de los dos es. Es una limitación real y documentada, no un error silencioso: si hace falta distinguir casos hoy, la alternativa es modelar la alternancia como `enum` (que sí tiene `match` exhaustivo, §3.3) en vez de una unión estructural.

**Por qué a veces aparece un paréntesis (`(A | B)[]`):** igual que `Optional`/tipo función dentro de un `List` (§2.2), un miembro que en TS se renderiza con `|` o `=>` en su nivel superior necesita paréntesis explícitos al aparecer dentro de otra construcción — `number | string[]` en TS significa `number | (string[])`, no `(number | string)[]`. El emisor ya aplicaba esta regla para `Optional`/`Function` (`render_type_atom`); ahora también protege a `Union`, en ambas direcciones: como elemento de un `List`, y como miembro de otra unión.

### 3.10 Funciones como valores — RESUELTO (referencias, no closures)

Una `fn` de nivel superior, referenciada por su nombre sin llamarla ahí mismo, ya es un valor de primera clase: se puede pasar como argumento, guardar en una variable, o recibir a través de un parámetro tipado `(A) -> B`. `Expr::Ident` para un nombre que no resuelve a una variable local cae al conjunto de `fn`s declaradas y sintetiza `Type::Function(params, ret)` (checker.rs) / produce un `Value::FnRef(nombre)` en runtime (runtime/mod.rs) — nunca captura nada, porque una `fn` de nivel superior no tiene ningún scope léxico exterior que capturar.

```
fn add_one(x: Int) -> Int { x + 1 }
fn apply_twice(f: (Int) -> Int, x: Int) -> Int { f(f(x)) }

fn use_it() -> Int { apply_twice(add_one, 5) } // 7
```

**Subtipado de tipos función — contravariante en parámetros, covariante en el retorno** (regla estándar): una función que acepta MENOS de lo estrictamente necesario (parámetro declarado más angosto) o devuelve MÁS de lo prometido (retorno más ancho) sirve donde se espera la firma original. Antes de esto, dos tipos función solo se consideraban compatibles por igualdad estructural exacta (misma lista de params, mismo retorno, sin aprovechar el subtipado ya definido para structs/`Optional`/unión):

```
S <: T          (para cada parámetro, EN SENTIDO INVERSO: T_param <: S_param)
S_ret <: T_ret  (el retorno, en el mismo sentido que todo lo demás)
──────────────────────────────────────────────────────────────────  (Function-Sub)
(S_params) -> S_ret  <:  (T_params) -> T_ret
```

**Lo que NO está implementado, y por qué:**
- **No hay literales de función anónima** (`fn(x) { x + 1 }`, `|x| x + 1`, o equivalente): `fn` solo existe como ítem de nivel superior (§2.1) — no hay ninguna expresión que produzca una función nueva ahí mismo, en medio de otro código.
- **No hay closures léxicos de verdad** (capturar una variable del scope que la rodea): consecuencia directa del punto anterior — una `fn` de nivel superior no tiene "scope que la rodea" para capturar nada. Lo que existe es una referencia a una función ya declarada (equivalente a un `fn` pointer de Rust/C), no un closure (`Fn`/`FnMut`/`FnOnce`).
- **No hay métodos de orden superior sobre `List`** (`.map()`, `.filter()`, `.reduce()`) — serían el consumidor más natural de "función como valor", pero `List` hoy solo tiene indexado (§2.3) y no participa de los métodos builtin de §3.8 (esos son sobre primitivos: `Int`/`Float`/`String`).
- Ninguno de los tres cruza el wire — un valor de tipo función ya estaba documentado como "solo campo de tipo local" en la tabla de mapeo (§4) desde antes de esta sección.

Es una decisión de alcance real, no una omisión: para un lenguaje cuyo propósito central es un contrato RPC tipado (PLAN.md §1), el consumidor principal de closures completos serían combinadores sobre colecciones que todavía no existen. Implementar closures de verdad (captura por valor vs. por referencia, análisis de qué variables escapan, su equivalente al emitir TypeScript) antes de tener ese consumidor sería construir infraestructura sin un caso de uso real que la ejercite todavía — mismo criterio que ya se aplicó al posponer WASM/LSP/gestor de paquetes a Fase 1+ (PLAN.md §4).

### 3.11 Validadores runtime (`validators.ts`) — RESUELTO

Planeado desde el documento original (`PLAN.md` §3.1: *"[4b] Emisor de contrato → .d.ts + cliente TS + validadores"*, *"esto es lo que hace la seguridad real en el borde, no solo compile-time"*) pero nunca construido hasta ahora — `compiler/src/codegen/ts_emit.rs` solo emitía `.d.ts` y `client.ts`. `linkc build`/`linkc dev` ahora generan un tercer archivo, `validators.ts` (`compiler/src/codegen/validators_emit.rs`), y `client.ts` valida cada respuesta contra él antes de devolverla.

```typescript
async getById(id: number): Promise<User | null> {
  const res = await fetch(...);
  if (!res.ok) throw new LinkTransportError(`HTTP ${res.status}`);
  const json: unknown = await res.json();
  if (!(json === null || isUser(json))) throw new LinkValidationError("getById", json);
  return json as User | null;
}
```

**Generación por tipo concreto alcanzado, no por declaración con nombre.** Recorre las mismas firmas de rpc ya resueltas que usa `emit_client`, y genera una función `isX(x: unknown): x is X` por cada tipo con identidad propia (struct con nombre, enum, `Result<T,E>`, `Patch<T>`, instanciación de un genérico) que aparece en ellas — nunca para `Box<T>` abstracto (opaco, GRAMMAR.md §3.6), solo para instanciaciones concretas como `Box<Int>` que ya llegan resueltas. Un tipo estructural (Optional/List/Tuple/Union/Map/struct anónimo) no tiene función propia — se valida inline, igual que `render_type`/`render_type_atom` (ts_emit.rs) tratan esa misma división entre "tiene nombre" y "se renderiza en el lugar".

**Predicados a mano, no Zod/typia.** Consumir `validators.ts` no debería exigirle al usuario instalar nada — mismo criterio de cero dependencias nuevas que el resto del compilador (`tiny_http` + `serde_json` siguen siendo las únicas, y son del lado Rust, no del TS generado).

**`Patch<T>` tiene su propio validador, no delega en el de `T`.** Igual que `render_type` vuelve cada campo `?:` para `Patch<T>` (`Partial<T>`, §3.4), su validador vuelve cada campo `=== undefined || <chequeo>` — incluidos los que en `T` eran requeridos. Validar un patch contra el validador de `T` a secas rechazaría de forma incorrecta un patch parcial válido.

**Tercera categoría de error, `LinkValidationError`.** Ni un error de dominio declarado (`Result<T,E>`, siempre vuelve como valor) ni un fallo de transporte (`LinkTransportError`, red/5xx/timeout) — "el servidor respondió 200 pero el payload no matchea el contrato" es su propio modo de falla, con su propia clase, consistente con la línea divisoria que ya traza §3.5 entre las otras dos.

**Límite real: solo valida lo que efectivamente cruza el wire.** Un `type`/`enum` que ningún `rpc` usa como parámetro o retorno no genera validador — no hay ningún valor real en runtime que necesite chequear su forma. Si se agrega un `rpc` nuevo que lo referencia, el próximo `linkc build`/`linkc dev` lo agrega solo.

---

## 4. Tabla de Mapeo c-script → TypeScript (exhaustiva)

| Construcción c-script | TypeScript emitido | Forma JSON en el cable | Nota |
|---|---|---|---|
| `Int`, `Float` | `number` | número | — |
| `String` | `string` | string | — |
| `Bool` | `boolean` | bool | — |
| `Void` | `void` | — (sin cuerpo) | Solo válido como retorno de `rpc` |
| `T[]` | `T[]` | array | — |
| `Map<K, V>` | `Record<K, V>` | objeto | `K` limitado a `String`/`Int` (claves JSON); `{K: V}` como literal de tipo NO se parsea, ver §2.2 |
| `(A, B)` | `[A, B]` | array de longitud fija | tupla, ver §2.2 sobre ambigüedad de paréntesis |
| `(A) -> B` | `(a: A) => B` | — | solo como campo de tipo función local; no cruza el wire |
| `A \| B` | `A \| B` | valor tal cual, con la forma de cualquiera de los miembros | subtipado de flujo de valor, sin angosto — resuelto en §3.9 |
| `type X = {...}` | `interface X {...}` (structural) | objeto | subtipado estructural, §3.2 |
| `type X<T> = {...}` | `interface X<T> {...}` | objeto | monomorfizado en el backend, genérico en TS, §3.6 |
| `enum E { A, B }` | `type E = "A" \| "B"` | string | enum simple = unión de literales |
| `enum` con datos (ADT) | unión discriminada con tag fijo `type` (no configurable en v0) | objeto con campo `type` | ver ejemplo `Result` en PLAN.md §2.2 |
| `x: T?` (campo) | `x: T \| null` | clave presente, valor `null` | resuelto en §3.4 |
| `x?: T` (campo) | `x?: T` (clave ausente = `undefined`) | clave omitida | resuelto en §3.4 |
| `Patch<T>` | todos los campos `?:`, preserva nullability de cada uno | — | utilitario análogo a `Partial<T>`, resuelto en §3.4 |
| `rpc f(x: T = v)` | parámetro con default → opcional en la firma TS del cliente | — | `f(x?: T)` en el cliente si se omite |
| `rpc f(...) -> Result<T, E>` | `{type:"Ok",value:T} \| {type:"Err",error:E}` | objeto con tag `type` | resuelto en §3.5 — nunca lanza para errores declarados |
| `stream f(...) -> T` | `AsyncIterable<T>` | eventos SSE/WS, uno por `T` serializado | runtime detallado en Fase 1 (PLAN.md §4) |
| `service S { ... }` | `interface SClient { ... }` + instancia concreta generada | — | el cliente real es un thin wrapper sobre `fetch`/WS |
| `const X: T = v` | `export const X: T = v` | — | solo tipos serializables (mismo universo que campos de struct) |

---

## 5. Estado y próximos pasos

`T?` (§3.4) y el manejo de errores (§3.5) quedaron resueltos con los defaults recomendados en `PLAN.md` §8.3 — ver `examples/decision-nullability.ts` y `examples/decision-errors.ts` para el resultado aplicado. Son reemplazables: si el criterio real termina siendo otro, es un cambio acotado a esas dos secciones y al emisor, no un rediseño del lenguaje.

Con el sistema de tipos sin huecos, el siguiente entregable es la implementación real (MVP Fase 0 de `PLAN.md` §4): lexer → parser → type checker → emisor `.d.ts`/`client.ts` → runtime mínimo → demo E2E donde cambiar un tipo en el backend rompe `tsc` en el frontend sin tocarlo. Ese trabajo vive en `compiler/` (Rust, sin dependencias externas para no depender de acceso a red del sandbox).
