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
item         = import_decl | type_decl | enum_decl | service_decl | const_decl | fn_decl | db_decl ;

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

db_decl      = "db" , "{" , field_list , "}" ;   (* "db" NO es keyword -- ver §3.12 *)
```

**El `;` de `type_decl` es opcional.** Un `type X = { ... }` termina en `}`; exigir además un `;` es la misma incomodidad que Rust/Go evitan después de un `struct`. `const_decl`/`let_stmt`/`return_stmt` sí exigen `;` — su valor no siempre termina en `}` (`const MAX: Int = 100`), así que hace falta una marca explícita de fin de sentencia.

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
             | closure_lit
             | identifier
             | int_lit | float_lit | string_lit | bool_lit | "null"
             | "(" , expr , ")" ;

(* Closure (§3.10). El cuerpo es SIEMPRE un block -- no hay "block como
   expresión" en el lenguaje, así que esto lo reusa tal cual. Mínimo 1
   parámetro: "||" lexea como un solo token (or lógico), no como dos "|".
   El tipo de un parámetro se parsea como postfix_type, NO type_expr: un
   "|" de nivel superior pertenece al cierre del closure, así que un tipo
   unión necesita paréntesis (|x: (Int | String)| { ... }). *)
closure_lit       = "|" , closure_param , { "," , closure_param } , [ "," ] , "|" , block ;
closure_param     = identifier , [ ":" , postfix_type ] ;

struct_or_variant_lit = identifier , [ "." , identifier ] , "{" , [ field_init_list ] , "}" ;
field_init_list        = field_init , { "," , field_init } ;
field_init             = identifier , ":" , expr ;

array_lit = "[" , [ expr , { "," , expr } , [ "," ] ] , "]" ;

(* misma ambigüedad y misma solución que en tipos (§2.2): (a) es agrupación,
   (a,) tupla de 1, (a,b) tupla de 2+ -- requiere ≥1 coma para NO ser Paren. *)
tuple_lit = "(" , expr , "," , [ expr , { "," , expr } ] , ")" ;

match_expr   = "match" , expr , "{" , { match_arm } , "}" ;
(* La coma SEPARA un arm-expr del siguiente, así que es opcional en el
   último (justo antes del "}"), igual que en Rust. Un arm cuyo cuerpo es
   un block nunca la lleva. *)
match_arm    = pattern , [ "if" , expr ] , "=>" , ( expr , [ "," ] | block ) ;

pattern      = pattern_atom , { "|" , pattern_atom } ;        (* or-pattern *)
pattern_atom = identifier                                    (* binding, incl. "_" *)
             | identifier , "." , identifier ,
               [ "{" , field_pattern_list , "}" ]             (* Enum.Variant { .. } *)
             | type_pattern
             | literal_pattern ;
(* Narrowing de una unión a su miembro concreto (§3.9). El tipo se parsea
   como postfix_type, no type_expr: un "|" que siga pertenece al or-pattern
   que lo rodea (i: Int | s: String son DOS alternativas, no un tipo unión). *)
type_pattern = identifier , ":" , postfix_type ;
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

### 3.8 Métodos builtin

`x.metodo()` sobre un valor que no es un struct/enum declarado no es acceso a un campo real — es azúcar reconocida por nombre y tipo del receptor, resuelta ANTES de intentar el `FieldAccess` genérico (que fallaría: `Int`/`Float`/`String`/`List` no son `Struct` ni `Dynamic`). Es el mismo mecanismo que ya resolvía `db.users.find(...)` (`checker.rs`/`runtime/mod.rs`, `BoundMethod`), generalizado.

| Método | Receptor | Resultado | Nota |
|---|---|---|---|
| `.toFloat()` | `Int` | `Float` | conversión exacta |
| `.toInt()` | `Float` | `Int` | trunca hacia cero (`3.9`→`3`, `-3.9`→`-3`), igual que `as` en Rust — no redondea |
| `.length()` | `String` | `Int` | cantidad de caracteres |
| `.contains(s: String)` | `String` | `Bool` | substring, no regex |
| `.take(n: Int)` | `T[]` | `T[]` | los primeros `n`; si la lista tiene menos, la devuelve entera (no falla) |
| `.filter(p: (T) -> Bool)` | `T[]` | `T[]` | ver §3.10 |
| `.map(f: (T) -> U)` | `T[]` | `U[]` | ver §3.10 |

No hay coerción implícita en ningún operador (§3.7) — `.toFloat()`/`.toInt()` son las únicas conversiones numéricas, y son siempre explícitas. `.length()`/`.contains()` son método, no propiedad (`x.length`, sin paréntesis) — consistencia con `.toFloat()`/`.toInt()` importó más acá que imitar la convención de propiedad de JS/TS.

### 3.9 Uniones de tipo (`A | B`) — RESUELTO (subtipado de flujo de valor Y narrowing)

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

**Por qué a veces aparece un paréntesis (`(A | B)[]`):** igual que `Optional`/tipo función dentro de un `List` (§2.2), un miembro que en TS se renderiza con `|` o `=>` en su nivel superior necesita paréntesis explícitos al aparecer dentro de otra construcción — `number | string[]` en TS significa `number | (string[])`, no `(number | string)[]`. El emisor ya aplicaba esta regla para `Optional`/`Function` (`render_type_atom`); ahora también protege a `Union`, en ambas direcciones: como elemento de un `List`, y como miembro de otra unión.

#### Narrowing: `match` con patrones `nombre: Tipo`

```
type Query = Int | String

fn findByIdOrEmail(query: Int | String) -> User[] {
  match query {
    id: Int => db.users.all().filter(|u: User| { u.id == id }),
    email: String => db.users.all().filter(|u: User| { u.email == email }),
  }
}
```

`nombre: Tipo` reusa el `:` que ya significa "nombre tiene este tipo declarado" en todos lados (`let`, params, campos de struct) -- sin inventar puntuación nueva (`is`/`as`). Nuevo `Pattern::Type(String, TypeExpr)` en el AST, mismo orden nombre-primero que `Param`/`Field`/`FieldPattern`. El tipo se parsea con `parse_postfix_type` (NO `parse_type_expr`, que consumiría un `|` de nivel superior perteneciente al propio or-pattern que lo rodea) -- esa elección resuelve, sin lógica extra, tanto que un miembro `Optional<T>` sea narrowable (`u: User?`) como que `i: Int | s: String` funcione como or-pattern normal (el `|` queda para el loop de `parse_pattern`, no se lo come la anotación de tipo).

**Rechazo de ambigüedad en tiempo de COMPILACIÓN, no "primer match gana" en runtime.** Antes de mirar los arms siquiera, `check_exhaustive_union` rechaza una unión cuyos miembros no se puedan distinguir de forma demostrable -- es una propiedad de la unión en sí, no de cómo se la matchea. Un chequeo ingenuo de `is_subtype` mutuo entre cada par de miembros NO alcanza: `{x:Int,y:Int}` y `{x:Int,z:Int}` no son subtipo mutuo entre sí, pero un TERCER tipo más ancho (`{x:Int,y:Int,z:Int}`, construible por cualquier usuario vía subtipado estructural de ancho, GRAMMAR.md §3.2) satisface los campos requeridos de los DOS a la vez -- un valor de ese tercer tipo sería ambiguo para cualquier regla que solo mire nombres de campo. La condición real (`union_members_are_distinguishable`, checker.rs): dos miembros son distinguibles solo si existe al menos un campo REQUERIDO por ambos cuyos tipos declarados tengan discriminantes de `Value` mutuamente excluyentes (`Int` vs `String`, nunca los dos a la vez en el mismo valor real) -- si no comparten ningún campo así, incluyendo el caso de no compartir NINGÚN campo requerido, se rechazan como ambiguos (falla cerrado: si el análisis no puede probar que son distinguibles, es error, no "asumamos que está bien"). Siempre ambiguos, sin análisis fino: `Dynamic` emparejado con cualquier cosa; dos miembros `List(_)` (una lista vacía matchea cualquiera de los dos); dos miembros `Optional(_)` (`null` matchea ambos). Este chequeo corre SOLO dentro de un `match` -- el uso ya soportado de una unión como tipo de parámetro que solo acepta-y-pasa sin narrowear (`fn f(x: Int | String)`) sigue funcionando igual que siempre, sin verse afectado.

**El chequeo de runtime tiene que coincidir con el argumento de solidez, o el análisis de arriba no vale nada.** `value_matches_type` (runtime/mod.rs) no solo chequea que un campo requerido esté PRESENTE -- chequea recursivamente que el VALOR guardado ahí tenga el tipo declarado. Es la única forma de que "campo compartido con tipos en conflicto" sea una distinción confiable: dos valores `{x: 5}` y `{x: "hola"}` comparten el nombre de campo `x`, pero el runtime nunca los confunde porque mira el `Value` real (`Value::Int` vs `Value::Str`), no la forma estática de dónde vino ese valor. `try_match_pattern` necesitó, por primera vez en este módulo, una tabla de `type`/`enum` declarados (`Symbols`, construida una sola vez en `invoke_rpc`, mismo patrón que la tabla `fns` ya existente) -- hasta esta ronda nada en runtime/mod.rs necesitaba resolver un `TypeExpr` a su forma real; solo el checker lo hacía.

**Fuera de alcance, a propósito:** narrowing fuera de `match` (sin operador `is`/`typeof` standalone, sin narrowing vía `if`). Una unión con miembros ambiguos según el análisis de arriba sigue sin poder matchearse -- error claro apuntando a la alternativa de siempre: modelar la alternancia como `enum` en vez de una unión estructural.

### 3.10 Funciones como valores — RESUELTO (referencias Y closures reales, `.map`/`.filter`)

Una `fn` de nivel superior, referenciada por su nombre sin llamarla ahí mismo, es un valor de primera clase: se puede pasar como argumento, guardar en una variable, o recibir a través de un parámetro tipado `(A) -> B`. `Expr::Ident` para un nombre que no resuelve a una variable local cae al conjunto de `fn`s declaradas y sintetiza `Type::Function(params, ret)` (checker.rs) / produce un `Value::FnRef(nombre)` en runtime (runtime/mod.rs) — nunca captura nada, porque una `fn` de nivel superior no tiene ningún scope léxico exterior que capturar.

```
fn add_one(x: Int) -> Int { x + 1 }
fn apply_twice(f: (Int) -> Int, x: Int) -> Int { f(f(x)) }

fn use_it() -> Int { apply_twice(add_one, 5) } // 7
```

**Subtipado de tipos función — contravariante en parámetros, covariante en el retorno** (regla estándar): una función que acepta MENOS de lo estrictamente necesario (parámetro declarado más angosto) o devuelve MÁS de lo prometido (retorno más ancho) sirve donde se espera la firma original.

```
S <: T          (para cada parámetro, EN SENTIDO INVERSO: T_param <: S_param)
S_ret <: T_ret  (el retorno, en el mismo sentido que todo lo demás)
──────────────────────────────────────────────────────────────────  (Function-Sub)
(S_params) -> S_ret  <:  (T_params) -> T_ret
```

Esa comparación de params vive en su propia función con nombre (`types::params_accept`), no repetida inline en cada lugar que la necesita -- ver más abajo por qué eso importó de verdad, no solo por prolijidad.

#### Closures: `|params| { block }`

```
list.filter(|u: User| { u.active })      // predicado -- siempre List<T>
list.map(|u: User| { u.name })           // transforma -- puede cambiar List<T> a List<U>

// captura 'total' del scope que lo rodea, y lo MUTA -- de ahí el `mut`
let mut total = 0;
let sumar = |x: Int| { total = total + x; x };
```

Estilo Rust, delimitado por `|`. El cuerpo es SIEMPRE un bloque con llaves -- nunca una expresión suelta (`|x| x + 1` no se soporta; hace falta `|x| { x + 1 }`) porque el lenguaje no tiene ningún concepto de "bloque como expresión general" y esto reutiliza `Block` tal cual en vez de inventarlo. Cada parámetro es `nombre (: tipo)?` -- la anotación es opcional cuando el closure se chequea (⇐) contra un `Type::Function` ya conocido (el callback de `.filter`/`.map`, o un `let` con el tipo declarado), y obligatoria cuando no hay ningún contexto del que inferirla (`synth_expr`, ej. `let f = |x| {...}` sin anotar el `let`).

**Dos límites de alcance reales, no arbitrarios:**
- **Closures de 0 parámetros no se soportan** (`||`). `||` lexea como un único token (`PipePipe`, distinto de `Pipe`), y ninguno de los dos consumidores nuevos (`.map`/`.filter`) necesita un closure sin parámetros -- no hay infraestructura sin un caso de uso real que la ejercite.
- **Un tipo unión en la anotación de un parámetro necesita paréntesis**: `|x: (Int | String)| { ... }`, no `|x: Int | String| { ... }`. La anotación se parsea con `parse_postfix_type` (no `parse_type_expr`, que consume `|` en loop para uniones y se comería el `|` de CIERRE del closure).

**Bug real de subtipado encontrado por un review de diseño antes de escribir código, no en producción:** al chequear un closure con un parámetro ANOTADO contra un `Type::Function` esperado, la dirección correcta es `is_subtype(esperado, anotación)` -- contravariante, igual que `Function-Sub` de arriba --, NUNCA `is_subtype(anotación, esperado)`. Al revés, un closure como `points.filter(|p: WidePoint| ...)` sobre una `List<NarrowPoint>` (donde `WidePoint` tiene MÁS campos que `NarrowPoint`) se aceptaría por error, y su cuerpo podría leer un campo que el dato real nunca tuvo -- crash en runtime, no error de compilación. La dirección correcta está aislada en `types::params_accept` (la misma función que usa `is_subtype`'s regla `Function`) precisamente para que no se pueda invertir por accidente una segunda vez.

**`.filter(pred)` y `.map(f)` -- por qué el checker los trata distinto.** `.filter` siempre devuelve `Bool`: el tipo esperado del callback (`(T) -> Bool`) se conoce ENTERO de entrada, así que se chequea (⇐) igual que cualquier otro argumento de tipo función. `.map` es distinto: el tipo de retorno del callback (`U`) es exactamente lo que no se sabe de entrada -- se SINTETIZA (`synth_callback_result`) en vez de chequearse contra algo fijo, ligando el parámetro del closure al tipo de elemento real de la lista y sintetizando el cuerpo. Ambos aceptan tanto un closure literal como una `fn` con nombre ya declarada (`xs.map(double)`) -- dos caminos de código distintos dentro de `synth_callback_result`, no un caso especial para cada forma.

**`return` dentro de un closure sin tipo de retorno conocido por contexto es un error, no una inconsistencia silenciosa.** `check_block` (la función que ya chequea cualquier bloque) usa el mismo `expected` tanto para la cola del bloque como para cualquier `Stmt::Return` anidado -- y hoy tiene un bug preexistente, real pero nunca ejercitado (`return` no se usaba en ningún `.link` ni test antes de esta ronda): un `if`/`match` en posición de sentencia (no cola) se chequea contra `Type::Void` sin importar el `expected` real del bloque que lo contiene, así que un `return` ahí adentro se compara contra `Void` en vez del retorno real. Ese bug queda **fuera de alcance de esta ronda** (es ortogonal, se documenta acá, no se arregla). Para no heredarlo de otra forma, la síntesis del cuerpo de un closure (`synth_block`, nueva) rechaza de entrada, con un error claro, cualquier `return` alcanzable desde el bloque que recorre -- incluso dentro de un `if`/`match` no-cola.

**Captura léxica real, no solo una referencia.** `Value::Closure` guarda el `Env` (`Rc<RefCell<Value>>` por variable) del momento en que se construyó -- clonar ese `Env` al llamar el closure clona los punteros `Rc`, no las celdas, así que una mutación posterior de una variable capturada (vía `Assign` en el scope exterior) SÍ es visible adentro del closure, y viceversa (mismo mecanismo que ya usan los bloques de `if` anidados).

**Hallazgo real, no pedido, encontrado por el mismo review: un closure recursivo arma un ciclo de `Rc`.** El patrón `let mut f: (Int)->Int = |x|{x}; f = |x|{ ... f(x-1) ... };` (necesario para escribir recursión desde un closure -- el lenguaje no tiene otra forma) tipa bien, y en runtime el segundo closure captura un `Env` que contiene la MISMA celda que `f` está a punto de sobreescribir: un ciclo real, no hipotético. Dos defensas, independientes y ambas baratas:
1. El checker rechaza `==`/`!=` cuando alguno de los dos operandos es (o contiene recursivamente, en un campo/elemento/miembro de unión) un tipo función -- comparar closures no tiene un significado útil de todos modos.
2. `Value` deja de derivar `PartialEq`/`Debug` -- se implementan a mano para que `Value::Closure` nunca recurse dentro de su `Env` capturado (nunca son iguales entre sí; su `Debug` solo imprime los nombres de parámetros). Defensa en profundidad para cualquier OTRO código (mensajes de error, tests) que compare/imprima un `Value` arbitrario sin saber que puede ser autorreferencial.

**Consecuencia real sobre el streaming (§3.13): `Value` dejó de ser `Send`.** `Value::Closure` guarda un `Env` con `Rc<RefCell<Value>>` -- ni `Rc` ni `RefCell` son `Send`/`Sync`, así que agregar este variant hizo que `Value` (y por lo tanto `Db`, que guarda `Vec<Value>`) dejaran de poder cruzar el borde de un hilo. El diseño original de streaming (Fase 2) corría `invoke_rpc` DENTRO del hilo spawneado para la conexión -- eso ya no compila. Arreglado moviendo `invoke_rpc` de vuelta al hilo PRINCIPAL (igual que cualquier `rpc` normal) y dejando que el hilo spawneado reciba solo el resultado YA CONVERTIDO a `serde_json::Value` (sin ningún `Rc` adentro, `Send` de sobra) -- el hilo aparte pasa a encargarse únicamente de la escritura de bytes SSE, que es lo único que de verdad necesitaba correr aparte (un cliente lento leyendo no debe bloquear al servidor de aceptar otras conexiones). Diseño más ajustado que el original, no solo un parche.

**Nada de esto cruza el wire.** Un valor de tipo función (`FnRef` o `Closure`) sigue siendo "solo campo de tipo local" en la tabla de mapeo (§4) -- `.map`/`.filter` y los closures solo existen DENTRO de cuerpos de `fn`/`rpc`, nunca como parte de un tipo declarado que el emisor tenga que traducir a TypeScript, así que `ts_emit.rs` no necesitó ningún cambio.

**Fuera de alcance, a propósito:** `.reduce()` y otros combinadores de orden superior, parámetros `mut` en un closure (ningún `fn`/`rpc` los tiene tampoco hoy), closures de 0 parámetros, y capturar por valor en vez de por referencia compartida.

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

**Efecto secundario real: construir esto expuso un bug de serialización preexistente.** `Value::Variant` (runtime/mod.rs) siempre serializaba como `{ type: "..." }`, sin importar si el enum era simple (`Role`, todo unit) o un ADT (`ValidationError`) — nadie lo había notado porque nada construía un enum simple vía la sintaxis del lenguaje (`Role.Member {}`) antes de esta sesión; los datos sembrados a mano en `db.rs` usaban directamente un string de Rust, sin pasar por acá. `validators.ts` es justo lo bastante estricto como para haberlo atrapado apenas se ejercitó de punta a punta: `isRole` exige un string plano, no un objeto. Arreglado dándole a `Value::Variant` también el nombre del ENUM (no solo el de la variante), para que el runtime pueda replicar exactamente el mismo chequeo `all_unit` que ya usa `emit_enum_decl` (ts_emit.rs) al serializar — la variante ganadora no alcanza para decidirlo sola: un ADT puede tener una variante sin campos propios (ej. `enum Wrapped { Has{value:Int}, Empty }`) que igual debe serializar como `{type:"Empty"}`, no como un string suelto.

### 3.12 "DB tipada" v0 (`db { ... }`) — RESUELTO

`db` dejó de ser `Type::Dynamic` (cualquier `db.lo-que-sea.como-sea(...)` tipaba, y solo fallaba en runtime). Un nuevo ítem de nivel superior declara la forma real:

```
db {
  users: User[],
  posts: Post[],
}
```

**`db` no es palabra reservada.** Se reconoce por texto ("db" seguido de `{`) solo en posición de ítem de nivel superior — en cualquier otro lado (`let db = 5;`, un parámetro, un campo) sigue siendo un identificador común. De hecho, esto arregló un bug real: antes, el string mágico `"db"` se chequeaba ANTES del lookup de variables (tanto en el checker como en runtime/mod.rs), así que un `let db = ...` de un usuario quedaba sombreado en silencio por el builtin. Ahora el lookup de variables va primero.

**Cada colección necesita un campo `id: Int`.** No es un capricho — es lo que hace posible que `insert` pida `Omit<T, "id">` (los campos de T menos `id`, un utility type nativo de TS, sin sintaxis nueva) en vez de T completo. Sin esta regla, `insert` habría exigido el struct entero — y **habría roto el propio demo insignia**, donde la forma de creación (`NewUser`) es deliberadamente un subconjunto de `User` (sin `id`, `role`, `deletedAt`). El checker lo exige al procesar `db { ... }`, con un error claro si falta.

```
type User = { id: Int, name: String, email: String, role: Role, bio?: String, deletedAt: String? }
db { users: User[] }

// insert pide Omit<User, "id"> -- NO el User completo. Como el lenguaje no
// tiene sintaxis de struct literal anónimo (siempre hace falta un nombre
// declarado, ver struct_or_variant_lit §2.3), la forma completa de creación
// se modela con un `type` propio, estructuralmente idéntico a Omit<User,"id">:
type NewUserRecord = { name: String, email: String, role: Role, bio?: String, deletedAt: String? }
fn makeUser(input: NewUser) -> NewUserRecord {
  NewUserRecord { name: input.name, email: input.email, role: Role.Member {}, deletedAt: null }
}
// db.users.insert(makeUser(input)) -- NewUserRecord <: Omit<User,"id"> por subtipado estructural
```

**Métodos:** `all() -> T[]`, `find(id: Int) -> T?`, `insert(x: Omit<T,"id">) -> T`, `applyPatch(id: Int, p: Patch<T>) -> T` — resueltos contra el tipo de elemento de verdad (`Type::DbCollection`, checker.rs). Un nombre de colección o de método desconocido ya es un error del checker (`db.usres.fnid(1)`, con AMBOS typo'd, se rechaza en tiempo de chequeo), no algo que se descubre recién en runtime.

**Runtime: en memoria, generalizado — sigue sin ser Postgres real.** `runtime/db.rs`'s `Db` pasó de estar hardcodeado a una única colección `"users"` a `HashMap<String, Mutex<Vec<Value>>>`, una entrada por colección declarada. `Db::new(&program)` arranca cada colección vacía (uso real); `Db::seeded()` se mantiene aparte, como conveniencia para tests/demo, sembrando los mismos dos usuarios de siempre. Se eliminó el hack que le ponía un default a `deletedAt` en `insert` — bajo la regla `Omit<T,"id">`, `deletedAt` (requerido, nullable) es un campo obligado del argumento; quien inserta pasa `deletedAt: null` explícito, consistente con "sin coerción implícita en ningún lado" (§3.7).

**Fuera de alcance, a propósito:** ningún driver SQL real (Postgres, etc.) — eso sigue siendo Fase 2 "Beta" en la tabla de `PLAN.md` §4. Esto es la forma más chica y honesta de darle a `db` un tipo REAL sin la infraestructura de una base de datos de verdad detrás.

### 3.13 Streaming real (SSE) para `stream` — RESUELTO, alcance `List<T>`

Antes, `Member::Rpc`/`Member::Stream` se colapsaban a lo mismo en todo el pipeline — pegarle a un `stream` por HTTP corría el cuerpo una vez y devolvía un solo JSON con 200, sin ningún indicio de que debía ser un stream. El stub del cliente generado ni siquiera lo intentaba (`throw new Error("streaming no implementado...")`).

**Alcance explícito, de entrada: repite una secuencia YA CALCULADA, no suscribe a eventos futuros.** El ejemplo de `PLAN.md` (`stream watch(id) -> User { db.users.subscribe(id) }`) implica suscribirse a cambios que todavía no pasaron — eso necesitaría una capa de pub-sub sobre `db` que no existe. Un generador perezoso tampoco es posible hoy: el lenguaje no tiene NINGÚN constructo de loop (`token.rs` no tiene `for`/`while`/`loop`), así que "generador real" está bloqueado por algo más grande que esta ronda. Lo que sí es real y honesto: el cuerpo de un `stream` devuelve `List<T>` (una lista completa, ya en memoria) y el servidor la manda como eventos SSE genuinos en vez de un solo blob JSON — mejor time-to-first-byte del lado del cliente, y el wire protocol que `AsyncIterable<T>` promete de verdad.

```
// La firma declara el ELEMENTO (igual que un rpc normal) -- el cuerpo
// tiene que devolver la secuencia completa (List<User>, no User suelto).
stream watchAll() -> User {
  db.users.all()
}
```

**Checker: `check_rpc` chequea el cuerpo contra `List<T>`, no contra `T`, cuando `is_stream`.** La firma (`return_type`) sigue resolviendo a `T` sin ningún caso especial — eso es lo que ya usan `emit_service_interface` (`AsyncIterable<T>`) y el validador de cada evento (mismo `isX` que un rpc normal usa para su único valor de retorno). Solo el chequeo del CUERPO envuelve el tipo esperado en `Type::List` antes de llamar a `check_block`.

**Runtime: `invoke_rpc` no distingue Rpc/Stream al evaluar** (siempre hizo `Member::Rpc(r) | Member::Stream(r)` en el lookup) — el resultado ya es el `Vec<Value>`/array JSON completo que `server.rs` necesita. Lo único nuevo es `is_stream_member(program, service, rpc) -> bool`, una función APARTE (no un cambio a la firma de `invoke_rpc`) que le permite a `server.rs` decidir el framing ANTES de invocar, sin forzar a los ~30 call sites de test existentes (todos `.unwrap()` un solo `Value`) a desestructurar una tupla que no les interesa.

**`server.rs`: solo la ESCRITURA de eventos corre en un hilo aparte, no el cómputo.** `invoke_rpc` siempre corre en el loop principal (igual que cualquier `rpc` normal); el hilo spawneado recibe el resultado YA CONVERTIDO a `Vec<serde_json::Value>` y solo se encarga de mandar los bytes SSE al cliente, así una escritura lenta (cliente que lee despacio) no bloquea al servidor de aceptar el resto de las conexiones. **Revisado durante la ronda de closures (§3.10):** el diseño original hacía correr `invoke_rpc` DENTRO del hilo spawneado (con `Arc<Program>`/`Arc<Db>` compartidos) — eso dejó de compilar en cuanto `Value` ganó el variant `Closure` (guarda un `Env` con `Rc<RefCell<Value>>`, ni `Send` ni `Sync`), porque `Db` guarda `Vec<Value>` y ya no podía cruzar el borde del hilo. La corrección de arriba (cómputo en el hilo principal, solo escritura aparte) resultó, además, un diseño más ajustado que el original: lo único que de verdad necesitaba correr aparte era la escritura, no el cómputo.

**Hallazgo real, no anticipado por el plan: `tiny_http::Response` + `request.respond()` NO sirve para streaming.** Confirmado con un spike aislado antes de tocar código de producción (no solo lectura de fuente): `request.rs::respond_impl` solo llama `writer.flush()` UNA vez, al final, sobre un `BufWriter::with_capacity(1024, ...)` (`client.rs`) que envuelve el socket real. Un `Read` que produce datos de a poco con sleeps en el medio NO llega incrementalmente al cliente por ese camino — todo el body sale junto, recién al cerrar la respuesta. La solución: `request.into_writer()` (acceso directo al mismo `BufWriter`, pero bajo control manual) + un `flush()` explícito después de cada evento — `BufWriter::flush()` ignora su capacity interno y fuerza lo acumulado al socket en el momento.

**Segundo hallazgo real, encontrado recién al probar con el `client.ts` GENERADO (no con un cliente crudo): `Connection: close` sin `Content-Length` ni `Transfer-Encoding` no alcanza.** Es válido por RFC 7230 §3.3.3 regla 7 ("el body termina cuando se cierra la conexión"), y un cliente TCP crudo lo respeta bien — pero `fetch()` nativo de Node (sobre `undici`) no lo trata como señal confiable de fin de body bajo HTTP/1.1: el stream llegaba completo pero el `for await` nunca veía `done: true`, colgado esperando más datos indefinidamente. Fix: `Transfer-Encoding: chunked` real, con el framing (`{tamaño-hex}\r\n{datos}\r\n`, terminado en `0\r\n\r\n`) escrito a mano en `server.rs` — bypasseando también `chunked_transfer::Encoder` (vive adentro de `Response::raw_print`, el mismo camino que ya se bypasseaba por el hallazgo anterior). Es la señal que todo cliente HTTP/1.1 sabe reconocer sin ambigüedad, a diferencia de depender del cierre de conexión.

**Desconexión de cliente a mitad de stream: no cuelga el hilo.** Confirmado con el mismo spike: el próximo `write()` después de que el cliente cierra la conexión falla de inmediato con `BrokenPipe`/`ConnectionAborted`/`ConnectionReset` (según la plataforma) — nunca se queda esperando. `write_stream` corta el loop apenas ve ese error, sin nada más que limpiar (la lista ya estaba completa en memoria de entrada).

**Cliente generado: `fetch()` + parseo manual del framing SSE, no `EventSource`.** `EventSource` es GET-only y sin body — pero el resto del contrato ya asume POST+JSON body para argumentos (igual que cualquier otro rpc), y un `stream` puede tener parámetros. En cambio, `async *m(): AsyncIterable<T>` lee `res.body` (un `ReadableStream` nativo de `fetch`) a mano: acumula en un buffer, corta en `\n\n`, valida cada `data: ...` con el mismo `isX` que un rpc normal, y hace `yield` recién si pasa. Cero dependencias nuevas (`TextDecoder`/`ReadableStream` son nativos de Node y del browser).

**De paso: un log mínimo de request-id.** Un `AtomicU64` incremental (`server.rs`) y dos líneas por request (inicio + status/resultado) — lo mínimo que el cambio a multi-hilo hace necesario para poder correlacionar logs concurrentes, no una iniciativa de observabilidad aparte.

---

## 4. Tabla de Mapeo c-script → TypeScript (exhaustiva)

| Construcción c-script | TypeScript emitido | Forma JSON en el cable | Nota |
|---|---|---|---|
| `Int`, `Float` | `number` | número | — |
| `String` | `string` | string | — |
| `Bool` | `boolean` | bool | — |
| `Void` | `void` | `null` en el cuerpo | Solo válido como retorno COMPLETO de un `rpc` -- como campo o parámetro es un error del checker (§4.1) |
| `T[]` | `T[]` | array | — |
| `Map<K, V>` | `Record<K, V>` | objeto | `K` limitado a `String`/`Int` (claves JSON); `{K: V}` como literal de tipo NO se parsea, ver §2.2 |
| `(A, B)` | `[A, B]` | array de longitud fija | tupla, ver §2.2 sobre ambigüedad de paréntesis |
| `(A) -> B` | `(arg0: A) => B` | — | solo dentro del backend; usarlo en la firma de un `rpc` (o en un tipo que esa firma alcance) es un error del checker (§4.1) |
| `A \| B` | `A \| B` | valor tal cual, con la forma de cualquiera de los miembros | subtipado de flujo de valor Y narrowing vía `match` — resuelto en §3.9 |
| `type X = {...}` | `interface X {...}` (structural) | objeto | subtipado estructural, §3.2 |
| `type X<T> = {...}` | `interface X<T> {...}` | objeto | monomorfizado en el backend, genérico en TS, §3.6 |
| `enum E { A, B }` | `type E = "A" \| "B"` | string | enum simple = unión de literales |
| `enum` con datos (ADT) | unión discriminada con tag fijo `type` (no configurable en v0) | objeto con campo `type` | ver ejemplo `Result` en PLAN.md §2.2 |
| `x: T?` (campo) | `x: T \| null` | clave presente, valor `null` | resuelto en §3.4 |
| `x?: T` (campo) | `x?: T` (clave ausente = `undefined`) | clave omitida | resuelto en §3.4 |
| `Patch<T>` | todos los campos `?:`, preserva nullability de cada uno | — | utilitario análogo a `Partial<T>`, resuelto en §3.4 |
| `rpc f(x: T = v)` | parámetro con default → opcional en la firma TS del cliente | — | `f(x?: T)` en el cliente si se omite |
| `rpc f(...) -> Result<T, E>` | `{type:"Ok",value:T} \| {type:"Err",error:E}` | objeto con tag `type` | resuelto en §3.5 — nunca lanza para errores declarados |
| `stream f(...) -> T` | `AsyncIterable<T>` | eventos SSE reales (`data: ...\n\n`), uno por `T` serializado, sobre chunked transfer | resuelto en §3.13 -- repite una lista ya calculada, no suscribe a eventos futuros |
| `service S { ... }` | `interface SClient { ... }` + instancia concreta generada | — | el cliente real es un thin wrapper sobre `fetch`/WS |
| `const X: T = v` | `export const X: T = v` **en `client.ts`**, no en `contract.d.ts` | — | un `.d.ts` es ambiental y TS rechaza inicializadores ahí (TS1039); un `const` es un valor, así que vive en el módulo real |

### 4.1 Qué puede aparecer en la firma de un `rpc`

Todo lo que aparece en la firma de un `rpc`/`stream` viaja de verdad por la red, así que tiene que ser expresable como JSON. Dos tipos de la tabla de arriba NO lo son, y el checker los rechaza en esa posición:

- **Tipos función** (`(A) -> B`) en cualquier lado de la firma, incluso anidados dentro de un struct que la firma alcance. Dentro del backend siguen siendo válidos (pasar una `fn` a otra, §3.10) -- lo que no puede es cruzar.
- **`Void`** en cualquier posición que no sea el retorno COMPLETO de un `rpc`. Como campo de struct o parámetro no significa nada.

Esta regla existía como afirmación en la tabla desde el principio, pero nada la hacía cumplir: hasta la auditoría, un `type T = { h: (Int) -> String }` usado como retorno tipaba, emitía `h: (arg0: number) => string` al contrato, y generaba un validador con `typeof x.h === "function"` -- una condición que ningún payload JSON puede satisfacer, así que el cliente rechazaba SIEMPRE la respuesta. Un error de compilación claro es mejor que un contrato imposible de cumplir.

### 4.2 Validación en los dos extremos

El contrato no es solo una promesa de tipos en tiempo de compilación: los dos extremos lo verifican en runtime, con errores de categorías distintas.

| Dirección | Quién valida | Qué pasa si no matchea |
|---|---|---|
| Respuesta (servidor → cliente) | `validators.ts`, llamado desde `client.ts` (§3.11) | `LinkValidationError` en el cliente |
| Petición (cliente → servidor) | el servidor, contra el tipo declarado de cada parámetro | HTTP **400** con la ruta exacta del campo que falló |

La segunda mitad faltaba por completo hasta la auditoría: el servidor convertía el JSON entrante con una función puramente sintáctica, sin mirar ningún tipo. Las consecuencias reales están documentadas en el commit que lo arregló; la más visible era que un enum recibido por el wire nunca llegaba a ser un enum de verdad adentro, así que `match` sobre cualquier parámetro de tipo enum fallaba siempre. Un campo de más en la petición se acepta (subtipado de ancho, §3.2) pero se descarta: el valor que entra al backend tiene EXACTAMENTE la forma declarada.

---

## 5. Estado

`T?` (§3.4) y el manejo de errores (§3.5) quedaron resueltos con los defaults recomendados en `PLAN.md` §8.3 — ver `examples/decision-nullability.ts` y `examples/decision-errors.ts` para el resultado aplicado. Son reemplazables: si el criterio real termina siendo otro, es un cambio acotado a esas dos secciones y al emisor, no un rediseño del lenguaje.

El compilador está construido y vive en `compiler/` (Rust; las únicas dependencias son `tiny_http`/`serde_json`, para el runtime del demo). Para el estado real y actualizado de qué está hecho y qué no, ver la sección "Estado" del [README](README.md) — este documento describe el LENGUAJE, no el avance del proyecto. Cada gap de diseño que se fue cerrando tiene su propia sección `§3.X — RESUELTO` acá arriba, incluyendo lo que quedó deliberadamente afuera y por qué.
