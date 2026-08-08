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
             | "return" | "if" | "else" | "while" | "true" | "false" | "null" ;
```

**Reservado pero fuera del v0 de la gramática:** `async`, `await`, `trait`, `impl` — el modelo de concurrencia y de polimorfismo ad-hoc se diseña en una iteración posterior (ver PLAN.md §4, Fase 1). `for`, `in`, `break`, `continue` — v0 de loops (§3.15) es solo `while`; ninguno de estos cuatro es todavía una palabra reservada de verdad (no aparecen en `keyword_from_str`, `compiler/src/token.rs`), esto es prosa preparatoria, no una reserva real.

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
member_decl  = [ annotation ] , ( rpc_decl | stream_decl ) ;
(* auth v0, §3.14 -- a lo sumo UNA por rpc/stream, nunca una lista *)
annotation   = "@authenticated" | "@requires" , "(" , identifier , "." , identifier , ")" ;
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
  { "dependencies": {
    "shapes": "./libs/shapes.link",
    "auth-lib": "git+https://github.com/usuario/auth-link.git#v1.2.0"
  } }
  ```
  **Dependencias `git+<url>#<rev>` — RESUELTO (auditoría post-push, `compiler/src/gitdep.rs`).** El valor de una dependencia en `link.json` puede ser, además de una ruta local, una URL git real con el prefijo `git+` -- `resolve_import_target` (`modules.rs`) detecta el prefijo y delega en `gitdep::resolve`, que clona/actualiza un caché local (`<raíz-del-proyecto>/.linkc/cache/<hash-de-la-url>/`, `hash_source` reusado de `link.lock` en vez de sumar una segunda función de hashing) invocando el binario `git` real vía subproceso -- sin ningún cliente git en Rust, misma filosofía que `rusqlite` con SQLite (§3.17). El punto de entrada DENTRO del checkout es `main.link` en la raíz, por convención (el mismo nombre que `linkc new` ya scaffoldea) -- no configurable en esta v0.

  **`#<rev>` es OBLIGATORIO, a propósito.** Sin un registro que ordene versiones (no hay ninguno, PLAN.md §8.3 lo descarta a propósito), "la última versión" no tiene un significado bien definido -- resolver contra la rama default de cada remoto sería una fuente de builds NO reproducibles desde el día 1, exactamente el problema que un package manager existe para resolver, no para reintroducir. `<rev>` acepta un tag, una rama, o ya un commit SHA -- `git checkout --detach <rev>` los trata igual.

  **Resolución: siempre fresca, nunca cacheada más allá de lo que el clon local ya tiene.** Si el rev pedido ya resuelve contra el clon existente (un tag/commit ya conocido de una resolución anterior), no hay ningún acceso de red -- si no, un `git fetch --all --tags` sobre el clon ya cacheado alcanza (no un re-clone). Un rev que es una RAMA (no un tag/commit fijo) se re-resuelve contra su HEAD real en cada build -- si la rama avanzó, el build lo sigue; para un pin duro e inmutable, usar un tag o un commit SHA directamente en `link.json`.

  **`link.lock` graba el commit resuelto -- informativo en v0, no un pin que se imponga por sí solo.** Un nuevo campo, `git_dependencies` (`{"nombre":{"url":...,"rev":...,"resolved":"<sha-completo>"}}`), registra exactamente qué commit se usó la última vez que se corrió `linkc build` -- útil para auditar qué versión real terminó en un build dado, sobre todo cuando `rev` es un tag/rama que puede moverse. A diferencia de un `Cargo.lock`/`package-lock.json` real, esto NO se lee para decidir qué commit usar en el PRÓXIMO build (que siempre re-resuelve `rev` fresco, ver arriba) -- es un registro de auditoría, no una fuente de verdad que compita con `link.json`. Convertirlo en un pin real (leer `resolved` si está presente y `rev` no cambió, en vez de re-resolver) es la extensión natural, no incluida en esta ronda.

  **Sin locking entre procesos concurrentes** -- dos `linkc build` corriendo a la vez sobre el mismo proyecto podrían pisarse el mismo clon cacheado. Límite de v0 conocido, no manejado (`Cargo` tampoco lo manejó bien en sus primeras versiones).

  **`link.lock` para archivos LOCALES -- RESUELTO, pero sigue sin ser un lockfile de versiones.** Con una dependencia por RUTA local no hay versión ni conflicto que "lockear" en el sentido de Cargo/npm — ese razonamiento original sigue valiendo para ESE caso. Lo que se agregó primero (`compiler/src/lockfile.rs`) es más angosto: `linkc build` calcula un hash SHA-256 de cada archivo `.link` tocado (`touched`, el mismo `Vec<PathBuf>` que ya devuelve `load_program`) y lo escribe en `link.lock` (JSON, `{"version":1,"entries":{"ruta":{"path":...,"hash":...}},"git_dependencies":{...}}`); en el PRÓXIMO `build`, si ya existe un `link.lock`, se compara antes de sobreescribirlo y cualquier archivo cuyo hash no matchea imprime una advertencia — detección de deriva entre builds para archivos locales, resolución+registro real (no un pin, ver arriba) para dependencias git. Rutas siempre relativas a la raíz del proyecto (nunca el `\\?\C:\...` que `fs::canonicalize` da en Windows) para que el archivo sea legible y portable entre máquinas -- el mismo problema de prefijo apareció de nuevo al pasarle una ruta de caché a `git clone` como argumento (git no lo entiende como argumento de línea de comandos, "Invalid argument"; `display_path`, la función que ya pelaba esto para texto legible, resultó ser exactamente la función correcta acá también, por una razón distinta y más dura que la estética original).

  Verificado con subprocesos reales: `gitdep::resolve` contra un repo git LOCAL como "remoto" (clon inicial, reutilización de caché sin red, fetch de un tag agregado después del clon inicial, checkout de un commit SHA directo) y `linkc build` de punta a punta (clona, resuelve el import, tipa, genera el contrato, y graba el commit real en `link.lock`). 371 tests, todos pasando.
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
stmt         = let_stmt | assign_stmt | expr_stmt | return_stmt | while_stmt ;
let_stmt     = "let" , [ "mut" ] , identifier , [ ":" , type_expr ] , "=" , expr , ";" ;
assign_stmt  = identifier , "=" , expr , ";" ;
return_stmt  = "return" , [ expr ] , ";" ;
expr_stmt    = expr , ";" ;
while_stmt   = "while" , or_expr , block ;          (* nunca produce un valor -- ver §3.15 *)

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
| `.length()` | `T[]` | `Int` | cantidad de elementos -- faltaba (solo existía para `String`) hasta que `login` (§3.14) necesitó "¿matcheó algún usuario?" |
| `.createSession(role: R)` | `auth` | `String` | ver §3.14 -- `R` debe ser un enum declarado |
| `.destroySession()` | `auth` | `Void` | ver §3.14 -- sin argumentos, opera sobre la sesión de la request actual |

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

**El chequeo de runtime tiene que coincidir con el argumento de solidez, o el análisis de arriba no vale nada.** `value_matches_type` (runtime/mod.rs) no solo chequea que un campo requerido esté PRESENTE -- chequea recursivamente que el VALOR guardado ahí tenga el tipo declarado. Es la única forma de que "campo compartido con tipos en conflicto" sea una distinción confiable: dos valores `{x: 5}` y `{x: "hola"}` comparten el nombre de campo `x`, pero el runtime nunca los confunde porque mira el `Value` real (`Value::Int` vs `Value::Str`), no la forma estática de dónde vino ese valor. `try_match_pattern` necesitó, por primera vez en este módulo, resolver un `TypeExpr` a su forma real -- hasta esta ronda nada en runtime/mod.rs lo hacía, solo el checker.

**Corrección (encontrada en un reparso posterior, texto desactualizado desde entonces):** el párrafo de arriba, en su versión original, decía que esto se resolvía con una tabla `Symbols` propia del runtime, construida una sola vez en `invoke_rpc`. Esa tabla existió (commit `4513b96`) pero tenía un bug real: devolvía `Type::Dynamic` para `Generic`/`Tuple`/`Map`, así que una unión con un miembro `Box<Int>` tipaba en el checker pero JAMÁS podía matchear en runtime -- exactamente el tipo de inconsistencia checker-vs-runtime que esta sección entera existe para evitar. Un commit posterior (`49d227f`, "Auditoría: el borde de red ahora es tipado de verdad") la eliminó y reusa el `&Checker` real (`Checker::build_symbols`, construido una vez en `invoke_rpc_with_sessions`) en su lugar -- el resolvedor de tipos verdadero, no una segunda implementación ad-hoc que podía (y de hecho llegó a) divergir del primero. Ese commit nunca actualizó este párrafo; el comportamiento visible que describe (narrowing funciona, ambigüedad se rechaza) siguió siendo correcto todo este tiempo -- solo el detalle de implementación había quedado desactualizado, y en la dirección de "se reemplazó por algo mejor", no de una regresión.

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

**Runtime: en memoria al principio, generalizado.** `runtime/db.rs`'s `Db` pasó de estar hardcodeado a una única colección `"users"` a un `HashMap` con una entrada por colección declarada. Se eliminó el hack que le ponía un default a `deletedAt` en `insert` — bajo la regla `Omit<T,"id">`, `deletedAt` (requerido, nullable) es un campo obligado del argumento; quien inserta pasa `deletedAt: null` explícito, consistente con "sin coerción implícita en ningún lado" (§3.7). **Actualización: RESUELTO.** El storage detrás ya no es en memoria -- ver §3.17: `Db` corre sobre SQLite real, con persistencia genuina entre reinicios de `linkc serve`.

### 3.13 Streaming real (SSE) para `stream` — RESUELTO, alcance `List<T>`

Antes, `Member::Rpc`/`Member::Stream` se colapsaban a lo mismo en todo el pipeline — pegarle a un `stream` por HTTP corría el cuerpo una vez y devolvía un solo JSON con 200, sin ningún indicio de que debía ser un stream. El stub del cliente generado ni siquiera lo intentaba (`throw new Error("streaming no implementado...")`).

**Alcance explícito, de entrada: repite una secuencia YA CALCULADA, no suscribe a eventos futuros.** El ejemplo de `PLAN.md` (`stream watch(id) -> User { db.users.subscribe(id) }`) implica suscribirse a cambios que todavía no pasaron — eso necesitaría una capa de pub-sub sobre `db` que no existe. Lo que sí es real y honesto: el cuerpo de un `stream` devuelve `List<T>` (una lista completa, ya en memoria) y el servidor la manda como eventos SSE genuinos en vez de un solo blob JSON — mejor time-to-first-byte del lado del cliente, y el wire protocol que `AsyncIterable<T>` promete de verdad. **Actualización: RESUELTO para un shape fijo.** El lenguaje ya tiene un constructo de loop (`while`, §3.15) y, sobre él, una capa real de pub-sub para `db` (§3.16) — un `stream` cuyo cuerpo es exactamente `while true { db.<coleccion>.subscribe() }` sí recibe eventos futuros de verdad, sin polling. Todo lo demás (un cuerpo con cualquier otra forma) sigue el camino `List<T>` descripto en esta sección, sin cambios.

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

### 3.14 Auth v0 (sesión opaca en memoria + roles) — RESUELTO

Hasta acá no existía NINGÚN mecanismo de guard/autorización en el lenguaje — cualquiera podía invocar cualquier `rpc`. Alcance elegido para v0, explícitamente: sesión opaca en memoria + roles, **sin JWT y sin ninguna dependencia nueva** (el proyecto sigue dependiendo solo de `tiny_http` + `serde_json`). Verificar contraseña/hash de credenciales queda **fuera de alcance a propósito** — es su propio problema de seguridad, no algo para meter de paso acá.

```
service Users {
  @authenticated
  rpc me() -> User { ... }

  @requires(Role.Admin)
  rpc update(id: Int, patch: Patch<User>) -> User { db.users.applyPatch(id, patch) }

  rpc list() -> User[] { ... }   // sin anotación = sin restricción, como siempre
}
```

`@authenticated` exige una sesión válida, cualquier rol. `@requires(Enum.Variante)` exige además que el rol de esa sesión sea exactamente esa variante. **A lo sumo una anotación por rpc/stream** (`RpcDecl.annotation: Option<Annotation>`, nunca una lista) y **un solo rol por `@requires`** (sin OR de roles) — límites deliberados de v0, no descuidos.

**`@requires(Role.Admin)` reusa el mecanismo de `Enum.Variante` que YA existía para nombrar una variante en un patrón de `match`** (`parse_pattern_atom`, `ident "." ident`, SIN llaves) — no se inventó una tercera sintaxis. Esto es a propósito ASIMÉTRICO con `Role.Admin {}` (que sí hace falta para *construir* un valor real, ej. al llamar `auth.createSession(Role.Admin {})`): una anotación nombra un TAG a comparar, una expresión construye un VALOR — dos reglas correctas por separado, pero que un usuario puede confundir la primera vez que las ve una al lado de la otra.

**El enum de `@requires`/`createSession` NO necesita ser "simple" (todas las variantes unitarias).** La comparación en runtime es solo por tag (`enum_name` + nombre de variante), nunca mira campos — así que `enum Role { Admin, Member, ServiceAccount { scopes: String[] } }` puede usar `@requires(Role.Admin)` sin problema, aunque `ServiceAccount` (una variante HERMANA) sí tenga datos.

**Dos builtins nuevos sobre el identificador `auth`** (mismo mecanismo que `db`: `Type::Auth`/`Value::Auth`, identificador especial resuelto en `synth_expr`/`eval_expr` DESPUÉS del lookup de variables locales — ver el hallazgo de abajo sobre por qué ese orden importa):
- `auth.createSession(role: R) -> String` — `R` debe sintetizar a un enum declarado; devuelve un token opaco.
- `auth.destroySession() -> Void` — **CERO argumentos**, a propósito (ver "hallazgo de seguridad" más abajo).

```
rpc login(email: String) -> String? {
  let matches = db.users.all().filter(|u: User| { u.email == email });
  if matches.length() > 0 { auth.createSession(matches[0].role) } else { null }
}

@authenticated
rpc logout() -> Void { auth.destroySession() }
```

**La decisión de autorización (401/403) vive en `server.rs`, no en el intérprete.** `runtime/mod.rs` solo recibe `sessions: &SessionStore` (para que los dos builtins de arriba funcionen) y `current_token: Option<&str>` (para que `destroySession()` sepa cuál es "la propia" sesión) — ninguno de los dos es una decisión, son datos ya resueltos por el caller. El gate real (`server.rs::check_auth_gate`) corre ANTES de `parse_args`/`json_to_typed_value`, usando solo `program` (para mirar la anotación vía `required_auth`, hermana de `is_stream_member`) + `sessions` (para resolver el token a un rol) — nunca construye ningún `Value` del intérprete. Corre para `rpc` Y `stream` por igual (ambos pasan por el mismo punto en `serve()`). `invoke_rpc` (la firma pública de siempre, ~70 call sites — tests + `wasm_demo.rs`) queda intacta como wrapper de una línea sobre `invoke_rpc_with_sessions`, que es la que de verdad recibe `sessions`/`current_token`.

**401 vs. 403, y qué NO se revela.** Sin token, o token que no resuelve a ninguna sesión → 401 genérico ("se requiere autenticación"), sin distinguir los dos casos (no ayuda a ningún cliente legítimo, y sí le da a un atacante una forma barata de validar el formato de un guess). Sesión válida pero rol incorrecto → 403, con un mensaje genérico que **no nombra el rol exigido** — a diferencia del nombre del rpc (ya público vía `client.ts`/`contract.d.ts`), qué rol hace falta para cada operación es política interna del servidor; regalarla le daría a cualquiera con un token de bajo privilegio un mapeo completo endpoint→rol gratis.

**Hallazgo de seguridad central de esta ronda: el generador de tokens original estaba roto, no solo "no revisado".** La primera versión generaba el token con `RandomState::new().build_hasher().finish()`, llamado dos veces, asumiendo ~128 bits frescos por token. Dos revisores adversariales en paralelo llegaron, cada uno por su cuenta, a la misma causa raíz: `std` cachea las keys `(k0,k1)` de `RandomState` **por hilo** — la primera vez que se pide en un hilo dado, lee del SO; cada llamada SUBSIGUIENTE en ESE MISMO hilo solo incrementa `k0` en 1, `k1` nunca cambia. Como el intérprete corre siempre en el hilo principal (single-threaded por diseño, §3.13), esto no daba "un secreto nuevo por token" sino **un único secreto de 128 bits fijado una vez al arrancar el proceso**, reusado con un contador chico encima — insuficiente para lo único que hace segura a una sesión bearer ("poseer el string ES la sesión"). Fix real, sin agregar ninguna dependencia: un hilo RECIÉN CREADO nunca inicializó ese cache thread-local, así que su PRIMER `RandomState::new()` sí pega contra el RNG real del SO (`BCryptGenRandom`/`ProcessPrng` en Windows). `SessionStore::fresh_128_bits` (`runtime/session.rs`) spawnea un hilo descartable y, DENTRO de él, deriva 2 hashes de 64 bits de la MISMA `RandomState` (sin volver a llamar `::new()`, que reincidiría en el problema). **Esto sigue sin ser un CSPRNG auditado** — alcanza para v0/demo; una implementación real necesitaría el crate `rand`/`getrandom`.

**Segundo hallazgo real: `destroySession(token)` como parámetro ordinario es una vulnerabilidad, no un detalle de API.** La propuesta original tomaba el token a destruir como argumento, simétrico a `createSession`. Un revisor adversarial lo marcó como el hallazgo más concreto de su ronda: **cualquiera que conozca o adivine el token de otra sesión podría destruirla sin poseerla ni haber pasado ningún chequeo de `@requires`** — un primitivo de "logout ajeno"/DoS dirigido, sin ningún segundo factor (a diferencia de RFC 7009, revocación OAuth, que sí exige credenciales del client que revoca). Fix: `destroySession()` sin argumentos, operando implícitamente sobre `current_token` — la sesión que ya autenticó la request actual. Por eso `logout` necesita `@authenticated`: sin sesión válida no hay nada que destruir, y sin la anotación el intérprete no sabría cuál token es "el propio".

**Bug preexistente encontrado de paso, no introducido por esta ronda: `eval_expr` no respetaba el orden de shadowing que `synth_expr` (checker) ya respetaba.** Al agregar el identificador especial `"auth"` a `eval_expr::Ident`, se encontró que esa función chequeaba `if name == "db"` **ANTES** de `env.get(name)` — al revés que el checker, que hace `env` primero desde que se corrigió el mismo bug para "DB tipada" (con un comentario explícito documentándolo). Consecuencia real, sin tocar nada de esta ronda: `fn f(db: Int) -> Int { db + 1 }` tipaba perfecto y **crasheaba en runtime**, porque `eval_expr` devolvía `Value::Db` ignorando el parámetro real. El único test relacionado solo verificaba que tipara, nunca lo ejecutaba. Corregido en el mismo lugar que hacía falta tocar para `auth`, con un test de runtime nuevo (antes no existía ninguno que ejecutara este caso).

**Otro hallazgo de paso: `const` no estaba restringido a literales fuera de `linkc build`.** `check_const` aceptaba cualquier expresión que tipara — la restricción real de forma-literal vivía solo en `ts_emit.rs::render_const_value`, o sea que `linkc serve` (que nunca llama a los emisores) nunca la exigía. Ya era una rareza inocua con `db` (`const X: User[] = db.users.all();` "funciona" en `serve`, releyendo la colección en cada uso). Con `auth.createSession(...)` deja de ser inocuo: un `const` así crearía una sesión Admin nueva cada vez que se lo referencia (los `const` no se memoizan en runtime), sin que nadie la pidiera ni forma de limpiarla. `check_const` ahora exige la misma forma-literal en `check_program` (por lo tanto en `serve` también), cerrando el agujero para los dos casos con una sola regla.

**CORS: `Access-Control-Allow-Headers` no dejaba pasar `Authorization`.** Confirmado por los dos reviews como necesario para que la feature sea alcanzable en absoluto: sin agregarlo, el preflight `OPTIONS` de cualquier browser real rechaza la request ANTES de que salga — ni siquiera es que el servidor la rechace, el browser no la intenta. Un solo cambio (`"Content-Type, Authorization"`) cubre `rpc` y `stream` por igual. `Access-Control-Allow-Origin: *` + un header `Authorization` manual no es el caso que la spec de CORS prohíbe combinar con `*` (eso aplica a `credentials: 'include'`/cookies, que este cliente nunca usa).

**Cliente generado: `token` es estado MUTABLE de instancia, no un parámetro por-llamada.** `{Service}ClientImpl` gana `private token: string | null` + `setToken(token)`, parte de la interfaz pública (`{Service}Client`) para que algo tipado como tal también pueda llamarlo. `push_fetch_call` adjunta `Authorization: Bearer ${token}` en TODO rpc si hay token seteado (el servidor decide caso por caso si lo exige). Correcto para "una instancia de cliente = un usuario/sesión activa" (mismo patrón que la mayoría de SDKs generados reales) — pero una instancia COMPARTIDA entre requests concurrentes de usuarios DISTINTOS (ej. un backend-for-frontend Node reusando un cliente módulo-level) puede pisarse el token entre requests. Documentado como límite v0 explícito; la alternativa (token por-llamada) cambiaría la forma pública de TODOS los métodos generados, no solo los protegidos.

**Fuera de alcance, a propósito:** verificación de contraseña/credenciales; expiración de sesión (vive hasta `destroySession()` o hasta reiniciar el proceso — el lenguaje ya tiene un `while`, §3.15, pero sigue sin ningún temporizador/reloj, así que expresar "expirá en N minutos" sigue sin ser posible); múltiples roles por `@requires` o múltiples anotaciones por rpc; exponer la identidad del caller dentro de un cuerpo (`ctx.user`/similar — solo el ROL viaja en la sesión, nunca una referencia al `User` completo); un CSPRNG auditado (ver el hallazgo de arriba).

---

### 3.15 Constructo de loop: `while` — RESUELTO, alcance acotado

Hasta acá el lenguaje no tenía NINGÚN constructo de loop — la única forma de repetir algo era recursión (una `fn` con nombre llamándose a sí misma, o un closure reasignado vía `mut` que se referencia a sí mismo, que además arma un ciclo real de `Rc`, ver §3.10). Elegido para v0, explícitamente: **`while` únicamente, `Stmt` (nunca `Expr`), sin `break`/`continue`, con una cota dura de iteraciones.**

```
fn sum(xs: Int[]) -> Int {
  let mut total = 0;
  let mut i = 0;
  while i < xs.length() {
    total = total + xs[i];
    i = i + 1;
  }
  total
}
```

**`while` NUNCA es una expresión.** `if`/`match` sí lo son porque necesitan unificar un valor entre ramas — eso exigiría diseñar `break <valor>`, un tipo para "el loop que nunca hace `break`" (el lenguaje no tiene ningún tipo `Never`/bottom) y unificación de tipos entre N sitios de `break`. Nada de eso hace falta para agregar sin recursión: el patrón es mutar un `let mut` declarado ANTES del loop, y usar un valor de cola DESPUÉS de él — el `while` en sí corre por puro efecto, se chequea contra `Type::Void` (mismo tratamiento que un `if`/`match` en posición de sentencia).

**Sin `for`, a propósito.** No existe ningún concepto de rango/iterador en el lenguaje (`.take`/`.filter`/`.map`/`.length` siguen siendo los únicos métodos de `List`, sin `.reduce()`/`.forEach()`); todo lo que `for` daría ya es expresable con `while` + indexado manual (`arr[i]`, que ya existía). Agregarlo antes de que `while` se haya usado en programas reales sería azúcar prematuro — mismo criterio que ya dejó afuera closures de 0 parámetros y roles múltiples en `@requires`.

**Sin `break`/`continue`, a propósito.** Implementarlos bien primero necesita resolver el hallazgo de abajo (un `break` anidado dentro de un `if`/`match` fallaría en silencio por la misma razón estructural que `return` ya falla) — deferido a una ronda futura si el uso real lo pide; la recursión sigue disponible mientras tanto para loops con salida temprana.

**`return` dentro de un cuerpo de `while` se RECHAZA explícitamente en el checker — no es una limitación caprichosa, evita heredar un bug real y ya existente.** Encontrado leyendo el código vecino al diseñar esto, no introducido por esta ronda: un `return` anidado dentro de un `if`/`match` usado COMO SENTENCIA (no cola) no solo tipa mal hoy (se chequea contra `Void` en vez del tipo real de retorno, por cómo `check_stmt` trata `if`/`match`-como-sentencia) sino que en RUNTIME es un no-op silencioso — `eval_block` descarta el valor que produce ese `if`/`match` (incluido cualquier `return` de adentro, que solo corta el `eval_block` INTERNO de esa rama, no el que la contiene) y sigue con la sentencia siguiente como si nada. Ya es explotable hoy con un `return;` desnudo en una función `Void`. En vez de reescribir el mecanismo de señalización de control de flujo entero (un cambio mucho más grande y riesgoso que agregar un loop), `while` simplemente no deja usar `return` en su cuerpo — sacá el valor final con una variable `mut` declarada antes del loop y un tail después, como en el ejemplo de arriba. El bug preexistente en `if`/`match`-como-sentencia queda documentado pero sin arreglar, fuera de alcance de esta ronda.

**Cota dura de iteraciones (`MAX_WHILE_ITERATIONS = 1_000_000`, `runtime/mod.rs`) — no opcional, agregada en la MISMA ronda que el loop.** El servidor (`server.rs::serve`) es un loop estrictamente single-threaded sin timeout ni scheduling cooperativo: un `while true { }` (o cualquier condición que el programa nunca vuelve falsa) congelaría PARA SIEMPRE el único hilo que atiende TODAS las requests, no solo la que lo disparó. Esto no es un límite v0 "honesto" en el mismo espíritu que otros (ej. "sin CSPRNG auditado") — es un footgun nuevo que la propia feature introduce, y este proyecto ya encontró y arregló footguns reales de ese calibre por review adversarial (el generador de tokens y `destroySession`, §3.14). La cota es deliberadamente generosa y NO configurable: un backstop contra el bug/loop-infinito más común, no un sistema fino de cuotas de recursos. Se cuenta una vez por invocación de rpc/fn (un `Cell<u64>` enhebrado por todo el árbol de evaluación, incluidos loops anidados y loops dentro de una fn/closure llamada desde el cuerpo), así que partir un loop grande en muchos chicos no lo esquiva.

**Fuera de alcance, a propósito:** `for`, `break`/`continue`, `while` como expresión con `break <valor>`; el bug preexistente de `return` dentro de `if`/`match`-como-sentencia (documentado arriba, no arreglado); límite de profundidad de recursión (preexistente, no empeorado por esta ronda — barato de cerrar reusando el mismo `Cell<u64>` si hace falta más adelante).

### 3.16 Push real: pub-sub sobre `db` para `stream` — RESUELTO, alcance acotado (shape fijo)

Con `while` ya resuelto (§3.15), el segundo bloqueo que §3.13 dejaba pendiente para push real era la falta total de una capa de pub-sub sobre `db`. Elegido para v0, explícitamente (vía pregunta directa, no un default silencioso): en vez de un mecanismo general de corutinas/`yield` para lógica arbitraria por evento, el diseño reconoce en tiempo de compilación UN ÚNICO shape sintáctico fijo como cuerpo de un `stream` "en vivo":

```
stream watchItems() -> Item {
  while true {
    db.items.subscribe()
  }
}
```

Cualquier otra forma (otro método, argumentos, sentencias de más, otra condición) NO dispara push real — cae al camino `List<T>` de §3.13, o directamente no tipa, nunca a una ejecución silenciosamente distinta de lo que el código sugiere.

**Por qué un shape fijo alcanza, en vez de corutinas de verdad.** El caso de uso real (anunciar mutaciones de `db` para siempre) no tiene ningún estado que se acarree entre iteraciones — cada vuelta hace exactamente lo mismo, "¿cuál es la próxima fila?". Bajo esa condición, "suspender el intérprete a mitad del loop y reanudarlo después" y "no dejar que el intérprete corra el loop en absoluto, y resolver todo con un registro de suscriptores en Rust puro" son observacionalmente idénticos — no hay nada que una corutina real preservaría que este atajo no dé gratis. Por eso `server.rs` intercepta el shape reconocido ANTES de invocar `invoke_rpc_with_sessions`: el cuerpo de un `stream` "en vivo" nunca llega a `eval_block`.

**El reconocedor vive en `ast.rs`, no en el checker ni en el runtime.** `recognize_live_subscribe(body: &Block) -> Option<&str>` es sintáctico puro (sin tipos): devuelve el nombre de la colección si el cuerpo es exactamente ese `while true { db.<col>.subscribe() }`, o `None` para cualquier otra cosa. Vivir en `ast.rs` es lo que le permite tanto a `checker.rs` (`check_rpc`, para tipar) como a `runtime/mod.rs`/`server.rs` (`live_subscribe_collection`, para interceptar en tiempo de request) llamarlo sin que ninguno de los dos dependa del otro.

**Hueco de TOCTOU cerrado a propósito, no dejado abierto.** Si `check_db_method` le diera a `"subscribe"` una firma normal y libremente componible (como `all`/`find`), entonces `rpc getOne() -> User { db.users.subscribe() }` -- fuera del shape reconocido -- tipiaría bien sin tener ningún comportamiento sensato en runtime. Fix: el brazo `"subscribe"` de `check_db_method` SIEMPRE falla, con un mensaje que apunta al shape exacto que sí funciona. La única forma de que `subscribe()` tipe en todo el programa es a través de `check_rpc` reconociendo el shape completo primero -- nunca a través del camino genérico de métodos de `db`.

**`Db` gana un registro de suscriptores; `subscribe()` hace snapshot+registro en una sola llamada sincrónica.** `Db::subscribe(collection)` devuelve `(snapshot, Receiver)`: `snapshot` es el estado actual de la colección ya serializado a JSON (mismo `value_to_json` que cualquier respuesta normal), y `Receiver` es el lado de lectura de un `mpsc::sync_channel(1024)` recién registrado. Las dos partes (sacar la foto, registrarse) son las dos líneas de UNA sola llamada, sin ningún punto de suspensión entre ellas -- y la única otra cosa que podría "colarse" (una mutación, vía `insert`/`applyPatch`) solo pasa dentro de `Db::call`, en el mismo único hilo del servidor. Como el servidor entero procesa una request a la vez, no hay forma de que una mutación se intercale entre esas dos líneas: el single-threading del servidor ES el lock del pub-sub, no algo aparte que hubo que agregar. (Si `Db` alguna vez dejara de ser single-threaded, este argumento hay que revisarlo primero -- probablemente invirtiendo el orden a "registrarse, después sacar la foto, después descartar duplicados".)

**`publish()` nunca bloquea, y un suscriptor lento o muerto no puede tirar abajo al servidor.** Cada `insert`/`applyPatch` exitoso llama `publish(collection, &row)` justo antes de devolver -- convierte la fila a JSON una vez y hace `try_send` (nunca bloqueante) a cada suscriptor de esa colección, podando (`retain`) cualquiera que devuelva `Full` (buffer de 1024 lleno, cliente no lee lo bastante rápido) o `Disconnected` (el hilo que escribía ya terminó). Un canal ilimitado hubiera sido un vector real de agotamiento de memoria; la política elegida es simple y explícita: mejor perder eventos para un suscriptor atascado que crecer sin límite.

**La limpieza de un suscriptor desconectado es LAZY, a propósito -- no eager.** Nada en el servidor nota activamente que un socket se cerró; lo que pasa es que el hilo escritor de ESE stream (`write_live_stream`, spawneado por `server.rs`, nunca el hilo principal) intenta escribir el próximo evento que le llega por su `Receiver`, ese `write()` falla con `BrokenPipe`/`ConnectionReset` igual que en §3.13, el hilo loguea `"cliente desconectado de un stream en vivo tras N eventos"` y termina -- recién en la SIGUIENTE mutación a esa colección, `publish()` encuentra el `SyncSender` ya cerrado (`Disconnected`) y lo poda del registro con `retain`. Entre la desconexión real y esa próxima mutación, el suscriptor muerto sigue ocupando una entrada -- aceptado a propósito: una limpieza eager reabriría la misma pregunta de `Send`/`Sync` que todo este diseño evita (ver §3.10 sobre por qué `Value`, y por lo tanto `Db`, están confinados a un hilo).

**Suscripción a la colección ENTERA, no por fila.** `subscribe(id: Int)` (recibir solo los cambios de una fila puntual) queda deliberadamente afuera de v0 -- whole-collection es un superset estrictamente más simple de reconocer (el shape fijo no necesita validar ningún argumento) y el cliente ya puede filtrar por `id` del lado TS sin ningún cambio de protocolo, gratis.

**Verificado end-to-end con el `client.ts` generado de verdad, no con una llamada cruda.** Se ejecutó el flujo completo con el cliente TAL COMO lo genera `linkc build` (ningún cambio de codegen hizo falta -- confirma la premisa del diseño en §3.13, "el cliente ya lee de forma indefinida"): insertar una fila ANTES de abrir el stream y confirmar que el primer evento recibido es esa foto inicial; insertar una SEGUNDA fila mediante una request separada mientras el stream seguía abierto y confirmar que llega como evento nuevo por la MISMA conexión, sin que se corte; terminar el proceso cliente abruptamente (sin cerrar el stream de forma prolija) e insertar una tercera fila, confirmando que el hilo principal sigue respondiendo de inmediato (el stream muerto se poda recién ahí, con el log esperado) -- nada se cuelga ni crashea del lado del servidor.

**Fuera de alcance de esta ronda, a propósito:**
- Filtrado/transformación por evento DENTRO del cuerpo de un stream -- exigiría reentrada real del intérprete (insegura sin corutinas) o cómputo en el momento del `publish`; ninguna de las dos entra en el shape fijo de esta ronda.
- Suscripción por fila (`subscribe(id)`) -- ver arriba.
- `delete` sobre `db` (no existe hoy) y qué significaría publicar una fila "eliminada".
- Re-autorización de una conexión en vivo de larga duración si la sesión que la abrió se revoca después -- `@authenticated`/`@requires` se valida una sola vez, al abrir: una conexión de horas de duración amplía ese hueco respecto de un rpc normal de vida corta.
- Limpieza EAGER de suscriptores desconectados (ver arriba) -- lazy es la política elegida para no reabrir la pregunta de `Send`/`Sync`.

### 3.17 Persistencia real: `db` sobre SQLite — RESUELTO

Con auth v0, los 3 prerrequisitos de LSP, y push real + loop (§3.15/§3.16) ya cerrados, el pendiente elegido fue "DB real con SQL": `db { ... }` (§3.12) era real a nivel de TIPOS desde esa ronda, pero el storage detrás seguía siendo un `HashMap<String, Mutex<Vec<Value>>>` puramente en memoria -- cada reinicio de `linkc serve` empezaba con todo vacío. Esta ronda le da persistencia genuina, manteniendo el mismo contrato público (`all/find/insert/applyPatch`, más `subscribe` del §3.16) sin ningún cambio en checker.rs ni en los ~50 call sites de test existentes.

**`rusqlite` (SQLite embebido, feature `bundled`), no Postgres.** El servidor es deliberadamente single-threaded y sin ningún runtime async (`Value::Closure` guarda un `Env` con `Rc<RefCell<Value>>>`, ni `Send` ni `Sync` -- confirmado desde la ronda de closures, §3.10) -- un driver async (`sqlx`, `tokio-postgres`) exigiría traer `tokio` entero, un cambio de arquitectura mucho más grande que esta ronda. `rusqlite` es sync-only por diseño, embebido (sin proceso de servidor externo corriendo aparte), y `bundled` compila su propio SQLite sin necesitar uno instalado en el sistema -- coherente con que `linkc serve` siga arrancando solo, mismo espíritu que ya tiene `tiny_http`. Postgres se descartó explícitamente por necesitar un servidor externo corriendo, rompiendo ese mismo espíritu.

**El schema SQL se DERIVA de `db { ... }`, nunca se escribe a mano** -- mismo principio que ya rige contract.d.ts/client.ts/validators.ts (todos generados de la misma fuente de verdad). Por cada colección, `Db::new` corre `CREATE TABLE IF NOT EXISTS` con un mapeo fijo:

| Campo c-script | Columna SQLite | Round-trip |
|---|---|---|
| `id: Int` | `INTEGER PRIMARY KEY AUTOINCREMENT` | ver justificación abajo -- y §3.18 para por qué pasó a llevar `AUTOINCREMENT` |
| `x: Int/Float/String/Bool` (requerido) | `INTEGER`/`REAL`/`TEXT`/`INTEGER` `NOT NULL` | directo |
| `x: EnumSimple` (requerido) | `TEXT NOT NULL` | nombre de variante en texto plano (`"Admin"`), no envuelto en JSON |
| `x: T?` (nullable, la clave SIEMPRE está) | columna nullable de `T` | SQL `NULL` ⇄ `Value::Null` |
| `x?: T` (opcional-por-clave, `T` no opcional) | columna nullable de `T` | SQL `NULL` ⇄ clave AUSENTE del `Value::Struct` |
| `x?: T?` (ambos a la vez, §3.4) | `TEXT`, siempre | único caso con 3 estados reales -- ver abajo |
| Struct / enum ADT / List / Tuple / Map / Generic / Union / Result / Patch | `TEXT` | `value_to_json`/`json_to_typed_value` reusados tal cual, cero formato nuevo |

**`id` como `INTEGER PRIMARY KEY AUTOINCREMENT` (revisado en §3.18).** En SQLite, `INTEGER PRIMARY KEY` es alias del rowid: insertar sin especificarlo autoasigna `max(rowid)+1`. En la ronda original de esta sección, `AUTOINCREMENT` se dejó afuera a propósito porque su única garantía adicional ("nunca reusar un id después de un borrado") era irrelevante -- no existía ningún método `delete` en todo el lenguaje, así que un id reusado no podía pasar por construcción. §3.18 agregó `delete`, lo que vuelve real esa situación (insertar tras borrar el último row reusaría su id sin `AUTOINCREMENT`) -- el mapeo de la tabla de arriba ya refleja el fix.

**`x?: T?` necesita 3 estados; una columna SQL solo tiene un bit de NULL.** Este es el único caso que se fuerza a `TEXT` (envuelto en JSON) aunque `T` sea nativo, específicamente para ganar un tercer estado: SQL `NULL` = clave ausente; el texto `"null"` (el JSON de `Value::Null`) = clave presente con valor null; cualquier otro texto = clave presente con un valor real. Sale gratis de `value_to_json`/`json_to_typed_value` sin ningún código especial -- `value_to_json(Value::Null)` YA serializa a `"null"`.

**Schema incompatible entre corridas: falla fuerte, nunca migra.** Al abrir, después del `CREATE TABLE IF NOT EXISTS`, se compara vía `PRAGMA table_info` el schema real de la tabla contra el que el programa actual declara (como conjunto, no por posición). Cualquier diferencia -- una columna de más, de menos, o de otro tipo -- hace panic ANTES de aceptar ninguna request, nombrando el archivo, la colección, y el diff exacto (esperado vs. encontrado), terminando en "borrá el archivo y volvé a intentar". Simétrico a propósito: incluso un cambio puramente aditivo falla igual, en vez de auto-`ALTER TABLE` solo para ese caso -- mezclar "esto se auto-arregla, esto no" es más sorpresa y más código que un único criterio parejo, y sigue el mismo criterio de v0 que el resto de esta sección (§3.12: "la forma más chica y honesta"). Detalle real: `id INTEGER PRIMARY KEY` reporta `notnull=0` en `PRAGMA table_info` aunque nunca pueda ser NULL de verdad -- la comparación trata esto como un caso especial, o cualquier reinicio detectaría un mismatch falso desde el primer arranque.

**El argumento de concurrencia de §3.16 se mantiene sin cambios.** Un `SELECT`/`INSERT` de `rusqlite` es una llamada de Rust sincrónica normal, sin `.await`, que corre entera en el hilo que la llama -- ni distinto de clonar un `Vec` en ese sentido. El single-threading del servidor sigue siendo el lock de `Db::subscribe`; lo único que cambia es cuánto tarda cada llamada (I/O real de disco), no si algo puede colarse en el medio. Se activa `PRAGMA busy_timeout` y, si el archivo lo permite (no aplica a `:memory:`), `journal_mode=WAL` -- higiene operativa que permite inspeccionar el archivo con `sqlite3` mientras el servidor sigue corriendo, sin cambiar ningún argumento de corrección.

**Verificado con un spike real ANTES de escribir el resto del código, no asumido de la documentación de `rusqlite`.** El riesgo real de esta ronda era `wasm32-wasip1` (el target del demo de `bin/wasm_demo.rs`): compilar el C de SQLite bundleado para WASI necesita un compilador C que apunte ahí (`wasi-sdk`), y no hay ninguna garantía de que "simplemente funcione". Investigación previa (release notes reales de `rusqlite`, no solo la crate en general) encontró evidencia concreta de soporte activo para `wasm32-wasip1 + bundled` desde la v0.33 -- confirmado corriendo el spike de verdad: **un único backend `rusqlite` sirve tanto para `linkc serve` (nativo) como para el demo wasm**, sin ningún fork `#[cfg(target_arch = "wasm32")]` en el código de la aplicación. `Db::seeded()`/tests usan `Connection::open(":memory:")` -- mismo código que un archivo real, SQLite trata ese string como su propio caso especial. Hallazgo real del spike, no anticipado: las variables de entorno `CC_wasm32_wasip1`/`AR_wasm32_wasip1` tienen que usar rutas con formato Windows (`C:/...`), NO el formato POSIX de Git Bash (`/c/...`) -- este segundo formato "parece" funcionar en una invocación directa de `clang` desde Bash (la conversión automática de rutas de MSYS lo arregla en ese caso puntual) pero falla en silencio cuando cargo lee la variable de entorno y spawnea `clang` él mismo (ese camino nunca pasa por la conversión de MSYS), con un error de `stdio.h no encontrado` que no tiene nada que ver con la causa real.

**Costo real, a propósito no escondido: rompe la política de "cero dependencias nuevas" documentada en 3 lugares del proyecto** (`session.rs`, `diagnostics.rs`, `codegen/validators_emit.rs`). Elegir esta feature de la lista de pendientes ya implicaba aceptar infraestructura real de DB (§3.12 ya lo enmarcaba así). Efecto colateral nuevo para cualquiera que compile el binario nativo: `rusqlite` con `bundled` necesita un compilador C disponible (ya presente en este entorno vía el toolchain GNU activo; una instalación MSVC sin Build Tools lo necesitaría de cero) -- mismo tipo de requisito que crates como `openssl-sys`/`ring` ya piden en el ecosistema Rust en general.

**Verificado de punta a punta contra el binario real, no solo con tests unitarios:** insertar un usuario por HTTP real, matar el proceso de `linkc serve`, volver a levantarlo con el mismo comando, y confirmar que el usuario sigue ahí sin haberlo vuelto a insertar (con el efecto colateral esperado y correcto: el segundo usuario insertado después del reinicio ya NO se vuelve `Admin`, porque `examples/users.link`'s regla de bootstrap mira si la colección está vacía, y ahora "vacía" significa de verdad "nunca tuvo datos", no "desde el último reinicio"). Por separado, cambiar el schema de una colección y apuntar al mismo archivo confirma el panic esperado, con el mensaje y el diff exactos, antes de aceptar ninguna conexión.

**Fuera de alcance, a propósito:**
- `delete`/`deleteWhere`/`findWhere` -- no existían al escribir esta sección; agregados en §3.18, que también corrige el mapeo de `id` de arriba.
- Migraciones reales tipo `ALTER TABLE` -- ver arriba, falla fuerte en vez de auto-migrar.
- Índices más allá de `id` -- no hace falta ninguno hoy porque el lenguaje no tiene ningún mecanismo de query además de `find(id)`/`all()`/`findWhere` + `.filter()` del lado interpretado.
- Acceso concurrente desde múltiples procesos `linkc serve` al mismo archivo -- `busy_timeout`/WAL mitigan, no se verifica exhaustivamente.
- Cualquier motor que no sea SQLite (Postgres/MySQL) -- decisión explícita arriba, no una limitación técnica de último momento.

### 3.18 CRUD real: `delete`/`deleteWhere`/`findWhere` sobre `db` — RESUELTO

§3.17 persistió el CRUD que ya existía (`all/find/insert/applyPatch`) pero no agregaba superficie nueva. Esta ronda sí: `delete(id: Int) -> Bool` (borra por id, `false` si no existía), `deleteWhere(fn(T) -> Bool) -> Int` (borra cada fila que matchea, devuelve cuántas) y `findWhere(fn(T) -> Bool) -> T[]` (mismo predicado, sin borrar) -- mismo espíritu que `.filter()` de `List` (§3.10), ahora también sobre una colección de `db`.

**Dónde vive de verdad la evaluación del predicado -- y por qué no puede vivir en `Db::call`.** `deleteWhere`/`findWhere` reciben un closure de usuario (`fn(T) -> Bool`) que hay que invocar una vez por fila. `Db::call` (en `runtime/db.rs`) es la capa que sabe hablar SQL, pero no tiene acceso a `call_callable` ni al `Env`/`fns`/sesiones que evaluar un closure necesita -- esa información vive en el intérprete (`runtime/mod.rs`), no en la capa de storage. Por eso la implementación real intercepta ambos métodos en `call_method` (mismo punto que ya redirigía `List::filter`/`List::map` a su propia lógica) *antes* de que la llamada llegue a `Db::call`: trae todas las filas con `all`, evalúa el predicado real fila por fila con `call_callable`, y para `deleteWhere` borra cada fila que matcheó a través del `delete` ya persistente (así que también publica, ver abajo). `Db::call` conserva sus propios brazos `"deleteWhere"`/`"findWhere"`, pero ahora devuelven un error explícito en vez de intentar algo -- son inalcanzables desde el intérprete normal (que siempre pasa por `call_method` primero), pero como `Db::call` es `pub fn`, quedan invocables directo (tests, LSP, código futuro); antes de esta ronda, esos dos brazos existían con la implementación INGENUA e incorrecta (`deleteWhere` ignoraba el predicado y borraba TODAS las filas; `findWhere` ignoraba el suyo y devolvía TODAS) -- exactamente el tipo de resultado que parece válido y no lo es, ahora reemplazado por un error claro que nombra el problema.

**`delete` ahora publica a los suscriptores.** Antes de esta ronda, `delete` quitaba la fila de SQLite pero nunca llamaba a `Db::publish` -- un `stream` con `while true { db.<col>.subscribe() }` (§3.16) nunca se enteraba de un borrado, solo de inserts. Ahora publica la fila borrada igual que `insert`/`applyPatch` ya hacían, así que un suscriptor ve el borrado como un evento más sobre el mismo wire SSE, sin ningún cambio de protocolo.

**`id` gana `AUTOINCREMENT`** (tabla de columnas en §3.17) -- con `delete` real, insertar después de borrar la última fila reusaría su id bajo el `INTEGER PRIMARY KEY` liso de antes; `AUTOINCREMENT` cierra esa ventana.

**Fuera de alcance, a propósito:**
- `deleteWhere`/`findWhere` traen SIEMPRE la colección entera a memoria antes de filtrar (vía `all`) -- correcto para el volumen de datos de v0, no pensado para una tabla grande; no hay traducción de predicado a `WHERE` de SQL.
- Sin transacción envolvente en `deleteWhere`: cada borrado es su propio `DELETE` -- una falla a mitad de camino deja borrado un prefijo, no ninguna o todas.

### 3.19 Protocolo LSP real (`linkc lsp`) — RESUELTO, Nivel 1+2

Los 3 prerrequisitos (spans+columna real, recuperación de errores del parser, spans en todo el AST/checker -- ver los tres "Done" de LSP en README.md) dejaban listo el terreno; esta ronda escribe el servidor en sí. `linkc lsp` habla JSON-RPC 2.0 sobre stdio con framing `Content-Length` estándar, y responde `initialize` anunciando `textDocumentSync: Full`, `hoverProvider`, `completionProvider` y `definitionProvider`.

**Diagnósticos con imports resueltos de verdad, no un buffer aislado.** `didOpen`/`didChange`/`didSave` arman un overlay en memoria (`HashMap<PathBuf, String>`, ruta canonicalizada como clave) con TODOS los documentos actualmente abiertos -- no solo el que cambió, porque un archivo importado puede estar abierto en otra pestaña -- y re-chequean a través de `modules::load_program_with_overlay` (el `Program` fusionado, siguiendo `import` de verdad) más `checker::Checker::check_program_full`. Antes de esta conexión, cada request re-tokenizaba/re-parseaba el buffer aislado con `lexer::tokenize`+`parser::parse` directo, así que cualquier archivo con `import` daba "no declarado" en falso -- el símbolo importado nunca se resolvía. Cuando `uri` no corresponde a un archivo real en disco (un buffer `untitled:` nunca guardado, fuera de alcance en v0), cae de vuelta al chequeo aislado de antes en vez de no publicar nada.

**Atribución multi-archivo: igual de honesta que la CLI, mejor en un caso.** Un `LoadError::Syntax{path, errors}` ya tiene identidad de archivo real incluso cruzando imports (se captura antes del merge) -- si `path` no es el documento abierto, el mensaje lo nombra explícitamente en vez de fingir que el error está en el buffer actual. Un `CheckError`, en cambio, no tiene identidad de archivo tras el merge -- mismo gate que `main.rs::report_check_errors` ya usaba (`touched.len() == 1`): con un solo archivo en el cierre transitivo, rango preciso; con más de uno, todos los mensajes se publican igual (nunca esconder que algo está mal) anclados en una posición degradada que aclara la imprecisión, en vez de arriesgar una heurística que adivine mal el archivo.

**`span_to_range`: multi-línea y UTF-16 de verdad, no una suposición heredada.** `diagnostics.rs` (el renderer de la CLI) asume que un span nunca cruza una línea porque ahí solo hay UNA línea ya extraída para trabajar -- una suposición razonable ahí, pero el LSP tiene el documento COMPLETO disponible y puede hacerlo bien: cuenta saltos de línea reales entre `span.start` y `span.end` para la línea de fin, y sobre cada char usa `char::len_utf16()` para la columna en unidades UTF-16 (lo que el wire de LSP pide, no un conteo crudo de chars) -- necesario porque los spans de declaración (`TypeDecl`/`FnDecl`/`ServiceDecl`, los que hover usa) son rutinariamente multi-línea en código real.

**Hover/completion/goto-def -- Nivel 2: a nivel de declaración, no sensible a posición.** Hover reconoce palabras clave/tipos builtin y, si el cursor cae sobre un nombre declarado (`type`/`enum`/`service`/`rpc`/`fn`/`const`/colección de `db`), muestra un resumen de ESA declaración -- ahora resuelta contra el `Program` fusionado, así que también funciona sobre un símbolo usado pero declarado en otro archivo. Completion da una lista plana (palabras clave + nombres de nivel superior, más el listado de colecciones tras `db.`) igual en cualquier posición del cursor -- deliberadamente no sensible a posición. Goto-definition busca una referencia en posición de valor por nombre sobre el mismo `Program` fusionado. Explícitamente fuera de alcance, documentado, no escondido: completion sensible a posición después de `x.` (necesitaría reconstruir el `Env` de tipos en el punto exacto del cursor, una tercera función de recorrido paralela a `check_expr`/`synth_expr`, Nivel 3, ronda futura), hover de una expresión arbitraria en medio de un body, goto-def de un nombre de TIPO escrito en una firma (`TypeExpr` no tiene span propio), documentos `untitled:`, sync incremental, multi-root workspaces, `$/cancelRequest`.

**Transporte hand-rolled, no `lsp-server`/`lsp-types`.** La investigación previa a esta ronda había elegido esos dos crates (mantenidos por rust-analyzer) para el framing/dispatch -- en la implementación real terminó siendo un loop propio, chico, sobre `io::stdin()`/`io::stdout()` (parseo de `Content-Length`, un `match` sobre `method`), sin ninguna dependencia nueva. Funciona y está cubierto por tests reales; los dos crates se sacaron de `Cargo.toml` por quedar sin ningún consumidor, en vez de dejarlos declarados sin usar. Documentado como divergencia consciente del plan original, no como algo pendiente de corregir -- si en el futuro hace falta algo que el loop propio no cubre bien (p. ej. `$/cancelRequest` real), migrar a `lsp-server` sigue siendo una opción.

**Aislamiento de errores: `catch_unwind` alrededor de cada re-chequeo.** `linkc lsp` es un proceso de LARGA VIDA de un solo hilo -- un panic sin capturar dentro de `load_program_with_overlay`/`check_program_full` (hoy sin ningún caso conocido alcanzable desde texto inválido, pero un checker que sigue creciendo puede introducir uno) terminaría el proceso entero, tirando abajo el servidor para TODOS los documentos abiertos por un solo archivo problemático. `compute_diagnostics_for`/`full_program_for` envuelven su lógica en `std::panic::catch_unwind` -- un panic capturado se loggea a stderr (el canal de Output de un cliente LSP real, VS Code incluido) y degrada a un único diagnóstico (o a `None`, cayendo al chequeo aislado del buffer) en vez de propagar. `&LspServer` no tiene mutabilidad interior, así que es `UnwindSafe` sin necesitar `AssertUnwindSafe` -- verificado con un test que fuerza un panic sintético con el mismo patrón exacto de captura, dado que no existe hoy un input real que dispare uno.

**Bug real, encontrado en un reparso general (no en uso real): un framing corrupto dejaba el server colgado en silencio, para siempre.** `run_stdio` parsea los headers línea por línea buscando `Content-Length`; si faltaba o no era numérico, el código original hacía `continue` de vuelta al tope del loop -- pero los bytes del BODY de ese mensaje mal formado nunca se leían, así que quedaban sin consumir en el stream. La próxima vuelta del loop de headers los interpretaba como si fueran líneas de header (nunca lo son, así que nunca encuentra un `Content-Length` válido tampoco) -- un desync PERMANENTE: el server dejaba de responder a TODO lo que viniera después de ese único mensaje roto, sin ningún error, indistinguible de un proceso colgado desde el lado del editor. No hay forma confiable de "resincronizar" sin saber cuántos bytes saltar -- ese largo es exactamente el dato que falta o es inválido -- así que el fix trata esto como lo que es: un error fatal de conexión, no una condición recuperable. `run_stdio` ahora devuelve `Err` ahí mismo (`cmd_lsp`, `main.rs`, ya traducía cualquier `Err` de `run_stdio` a un mensaje en stderr + código de salida distinto de cero -- no hizo falta tocar esa parte). Verificado con un test que manda un `Content-Length` no numérico a mano contra el binario real y espera la salida del proceso con un timeout propio (`compiler/tests/lsp_stdio.rs`) -- necesario porque un `child.wait()` sin cota hubiera colgado el TEST también si el bug hubiera seguido ahí.

**Verificado en dos capas, no solo in-process.** Los tests unitarios de `lsp.rs` llaman `handle_message` directo, sin ningún proceso de por medio. `compiler/tests/lsp_stdio.rs` agrega una segunda capa que sí importa: spawnea el binario `linkc` compilado de verdad (`env!("CARGO_BIN_EXE_linkc")`) con el arg `lsp`, escribe bytes con framing `Content-Length` real a su stdin, y lee la respuesta de vuelta de su stdout -- cubriendo el buffering real de pipes de sistema operativo (particularmente en Windows) que una llamada a función in-process no puede. Incluye el mismo caso de import válido entre dos archivos reales que `lsp.rs` ya prueba in-process, ahora también contra el binario.

Cliente de referencia real en `editors/vscode/` (extensión mínima que spawnea `linkc lsp` y conecta un `LanguageClient` para archivos `.link`).

### 3.20 Codegen WASM nativo v0 (`linkc wasm`) — RESUELTO, alcance mínimo

Distinto del target WASM que ya existía (`compiler/src/bin/wasm_demo.rs`, que recompila el intérprete ENTERO a `wasm32-wasip1` -- sigue siendo el camino real/de producción): `linkc wasm <archivo.link> <salida.wasm>` (y, como efecto colateral best-effort de `linkc build`, un `main.wasm` junto a `contract.d.ts`/`client.ts`/`validators.ts`) genera bytecode WASM DIRECTO por función vía `wasm-encoder`, sin pasar por el intérprete en absoluto -- el experimento de codegen nativo que la fila de Fase 1 en PLAN.md §4 nombraba como evolución futura.

**Alcance real: aritmética/comparación entera sobre `Int`/`Bool`, una sola expresión final, nada más.** Todo parámetro y tipo de retorno tiene que ser `Int` o `Bool` (ambos representados como `i64`, `Bool` como 0/1) -- cualquier otro tipo (`String`, `Float`, un struct, un enum, `T?`, `T[]`, `Map<K,V>`, ...) no tiene representación en este esquema. El cuerpo de la función tiene que ser exactamente una expresión final (`Int`/`Bool` literal, un identificador que sea un parámetro, `+ - * / % == != < > <= >=` sobre esas, y paréntesis) -- nunca una sentencia (`let`/asignación/`if` como sentencia/`while`).

**Fuera de ese subconjunto, siempre falla explícito -- nunca antes.** La primera versión de este codegen (de una sesión externa al repo) reemplazaba silenciosamente CUALQUIER construcción no soportada por `I64Const(0)`, e ignoraba por completo las sentencias de un bloque (`emit_block` solo miraba la cola) -- `linkc wasm`/`linkc build` reportaban éxito mientras el `.wasm` generado calculaba otra cosa. Ahora `emit_expr`/`emit_block` devuelven `Result`, y cualquier construcción fuera del subconjunto de arriba (un parámetro `String`, un operador lógico `&&`/`||`, una sentencia `let` en el cuerpo, una llamada, un `match`, ...) hace fallar la emisión con un mensaje que nombra la función y el problema exacto. En `linkc build`, esto es una ADVERTENCIA, no un fallo del build entero: `contract.d.ts`/`client.ts`/`validators.ts` son las salidas de las que el resto del proyecto depende; `main.wasm` es un artefacto secundario best-effort, y casi ningún programa real (empezando por `examples/users.link`, que usa `String`/structs/`db`) cae dentro del subconjunto soportado hoy -- el mensaje de éxito de `linkc build` solo nombra `main.wasm` cuando de verdad se escribió.

**Fuera de alcance, a propósito -- v0 mínimo, no un backend de codegen general:** cualquier sentencia dentro de un cuerpo (locals, control de flujo compilado a bloques/loops/branches WASM); `String`/`Float`/structs/enums/`Optional`/`List`/`Map`/`Union`/`Result`/`Patch`; llamadas entre funciones dentro del módulo emitido; `db`/sesiones/streaming (no tienen sentido fuera del intérprete). Cerrar esta brecha de verdad (soportar un programa real como `users.link`) es una ronda propia, del tamaño aproximado de esta, no una extensión incremental.

**Decisión de roadmap (auditoría post-push): congelado a propósito, no una brecha a cerrar.** `wasm32-wasip1` (recompilar el intérprete entero, `compiler/src/bin/wasm_demo.rs`) es y sigue siendo el ÚNICO camino real de producción -- ya corre un programa REAL (`Users.getById` de punta a punta dentro de `wasmtime`, PLAN.md §2.4), mientras que cerrar la brecha de `linkc wasm` hasta soportar algo comparable (statements, `String`/structs/`db`, llamadas entre funciones) es, en la práctica, escribir un backend de codegen nativo completo desde cero -- meses de trabajo, no una ronda más. `linkc wasm` se queda tal como está: un experimento honesto, correctamente acotado y con tests, sin plan de extenderlo -- no se retira (el código y sus tests siguen siendo correctos para lo que documentan soportar) pero tampoco se lo trata como el Fase 1 "codegen directo vía `wasm-encoder`" pendiente de crecer que PLAN.md §4 todavía sugería. Si en el futuro hace falta codegen nativo de verdad (no vía intérprete), la recomendación sigue siendo `cranelift-jit`/`cranelift-object` (PLAN.md §2.4) sobre expandir esto -- son herramientas hechas para ESO, `wasm-encoder` es autoría de bajo nivel pensada para emitir bytecode ya decidido, no un framework de compilación con manejo de locals/control de flujo/calling conventions.

---

### 3.21 LSP Nivel 3 (Ronda 1/3): goto-definición de un nombre de tipo en una firma — RESUELTO

§3.19 dejó 3 gaps documentados como "Nivel 3, fuera de alcance a propósito": completion sensible al tipo real del receptor tras `x.`, hover de una expresión arbitraria en medio de un body, y goto-def de un nombre de TIPO escrito en una firma (ej. `Point` en `fn origin() -> Point`) -- bloqueado porque `TypeExpr`/`Param`/`Field` no tenían span propio. Esta ronda resuelve el tercero. Investigado con 2 agentes de Plan en paralelo (Nivel 3 mínimo vs. completo) que coincidieron en que los 3 ítems NO son una sola ronda: comparten una utilidad de conversión de posición y un principio (reusar el checker/AST existente en vez de duplicar lógica), pero el ítem de goto-def vive enteramente en la capa sintáctica (`ast.rs`+`parser.rs`, sin `Checker`) mientras los otros dos necesitan reconstruir el `Env` de tipos del checker -- una traversal genuinamente más cara, y su propia ronda futura (orden recomendado: hover antes que completion, porque completion termina reusando la misma máquina que hover construye).

**`TypeExpr::Named` gana un tercer campo, `Span` -- las otras 7 variantes no.** Verificado con grep en todo el crate: `TypeExpr::Named` se construye en 2 sitios de producción (`parser.rs::parse_primary_type`, y una construcción sintética en `checker.rs::synth_struct_lit` sin texto fuente real) y se destructura en 2 (`checker.rs::resolve_named_type_subst`, `codegen/wasm_emit.rs::wasm_scalar_type`) -- nada parecido a los ~155 sitios que la migración a `Spanned<Expr>`/`Spanned<Stmt>` tocó en su momento (Ronda A). La razón de fondo: de las 8 variantes de `TypeExpr`, solo `Named` corresponde a un identificador ESCRITO al que alguien pediría saltar -- `Struct`/`Map`/`Tuple`/`Function`/`Optional`/`List`/`Union` son combinadores sintácticos sin nombre propio (el `Int` dentro de `Int[]` ya es su propio `Named` anidado), así que no se repitió el patrón `Spanned<T>` para todo el enum. `TypeExpr` sacó `PartialEq` del derive y lo reimplementa a mano ignorando el span en el brazo de `Named` (mismo criterio que `Spanned<T>` ya resolvía) -- si no, dos `Named("Int", vec![])` en offsets distintos hubieran dejado de ser `==`, rompiendo en silencio los tests existentes que comparan `TypeExpr` por igualdad.

**La búsqueda es puramente sintáctica -- sin `Checker`, sin `Env`.** `find_named_type_in_program` (`compiler/src/lsp.rs`) recorre, para cada ítem del programa, exactamente los mismos lugares donde un `Field`/`Param`/`return_type` puede aparecer -- `type`/`db`/variantes de `enum` (sus `Field`s), y `Param`s + `return_type` de `fn`/`rpc`/`stream` -- exactamente los spans de FIRMA que `FnDecl.span`/`RpcDecl.span` ya cubrían desde el prerrequisito 3/3 original del LSP (firma completa, nunca el body), así que esta búsqueda nunca se solapa con hover/completion de Nivel 2 ni con lo que Nivel 3 (ítems 1/2) construirá más adelante dentro de un body. `find_named_type_at` es exhaustiva sobre las 8 variantes de `TypeExpr` -- sin brazo `_` a propósito, para que agregar una variante nueva rompa la compilación acá en vez de que la búsqueda la ignore en silencio -- y prioriza los `args` de un genérico antes que el propio nombre, para que el cursor en `Line` dentro de `Box<Line>` resuelva a `Line`, no al `Box` que lo envuelve.

**Integración en `get_definition`: autoritativa cuando dispara, nunca cuando no.** Si el offset del cursor cae dentro de un `TypeExpr::Named`, la respuesta viene de esta búsqueda -- la declaración `type`/`enum` encontrada, o `None` si el nombre es un builtin (`Int`/`String`/...) o un parámetro de tipo genérico sin declaración propia -- y NUNCA cae al loop viejo de coincidencia-por-palabra de Nivel 2. Esto evita un falso positivo real y concreto: un tipo builtin usado en una firma (`fn f() -> Int`), si además existe un `const`/`fn`/`service` con el mismo nombre en otro namespace (`const Int: Bool = true;`), el loop viejo saltaría (mal) a ESE otro ítem por pura coincidencia de texto -- la búsqueda nueva, al confirmar que el offset SÍ es un uso de tipo, responde `None` ella misma en vez de dejar que el loop viejo adivine.

**Límite honesto, encontrado escribiendo los tests de esta misma ronda, no anticipado en el diseño original: esto NO protege contra un `Field`/`Param` cuyo NOMBRE (no su tipo) coincide textualmente con una declaración `type`/`enum` existente** (ej. `type Point = {...}; type Shape = { Point: Int }` -- pedir goto-def sobre el nombre de CAMPO `Point` sigue cayendo al loop viejo, que salta al `type Point`, porque el cursor ahí no está dentro de ningún `TypeExpr::Named` en absoluto). La causa es la misma que ya limita a Nivel 2: `Field`/`Param` no tienen span propio, así que no hay forma de que la búsqueda nueva sepa "este offset es un NOMBRE de campo, no un tipo" sin agregarles uno -- fuera de alcance de esta ronda a propósito (agregar el span de `Named` alcanzaba para el objetivo principal; agregar spans a `Field`/`Param` sería una extensión aparte, del mismo tamaño que esta, no una consecuencia gratis).

**Bug real encontrado en la investigación, corregido como corequisito (no un cuarto ítem de Nivel 3, un defecto de Nivel 2 que esta ronda expuso al agregar el primer test cross-file de `get_definition`):** `full_program_for` fusiona todo el cierre transitivo de imports en un solo `Program`, descartando `touched` -- así que un símbolo resuelto vía un archivo IMPORTADO tenía un `Span` con offsets de ESE otro archivo, pero `get_definition` devolvía `"uri": uri` (el documento ABIERTO) con el rango calculado sobre `source` (el texto del documento abierto). Resultado: el editor no navegaba a ningún lado útil. Mismo gate que `compute_diagnostics_for_inner` ya usa para diagnósticos (`touched.len() <= 1`): con más de un archivo tocado, `get_definition` devuelve `None` para CUALQUIERA de las dos búsquedas (nunca arriesga una posición que puede estar en el archivo equivocado) en vez de intentar el caso general, que necesitaría que `modules.rs` etiquete cada `Item` con su archivo de origen -- cambio genuinamente más grande, no pedido acá.

**Fuera de alcance, a propósito:**
- Anotaciones de tipo DENTRO de un cuerpo (`let x: Point`, tipo de un parámetro de closure) -- mismo tipo de búsqueda aplicada desde otro punto de partida sería la extensión natural, no incluida acá.
- El límite de `Field`/`Param` sin span descrito arriba.
- Los ítems 1 (completion sensible a `x.`) y 2 (hover de expresión arbitraria) de Nivel 3 -- rondas futuras separadas; el orden recomendado es hover antes que completion (completion termina siendo un superconjunto de la misma máquina de "reconstruir `Env` + ubicar nodo" que hover necesita construir primero) y ninguno de los dos depende de esta ronda.

---

### 3.22 Identidad de archivo en `Span` — RESUELTO

§3.21 documentó como bug real (no un ítem de Nivel 3 en sí) que `get_definition` cruzando archivos se negaba en bloque (`touched.len() <= 1`) porque un `Span` del `Program` fusionado no decía de qué archivo real venía -- podía ser de un `import`. Esta ronda cierra ese gap de fondo, con el mismo mecanismo sirviendo a LSP y CLI a la vez.

**`item_files: Vec<PathBuf>`, un archivo por ÍTEM, no por span individual.** `modules::load_program_with_overlay` ahora devuelve una tercera pieza junto a `Program`/`touched`: un `Vec<PathBuf>` del mismo largo y orden que `Program.items`, poblado en `Loader::visit` en el mismo `push` que ya llenaba `merged`. La idea central: un `Item` (`type`/`enum`/`fn`/`service`/`const`) nunca se parte entre dos archivos, así que CUALQUIER `Span` anidado a cualquier profundidad dentro de ese ítem -- firma, body, una sub-expresión -- pertenece al mismo archivo que el ítem completo. No hace falta razonar sobre RANGOS de offsets (ambiguos entre archivos: el offset 200 puede existir válidamente en dos archivos de 500 bytes cada uno) ni requiere ningún cambio en `ast.rs`/`parser.rs` -- alcanza con trackear el archivo por ítem, en el único lugar (`modules.rs`) que ya sabía cuál era.

**El checker estampa el archivo en el mismo punto donde ya estampaba el span.** `checker::CheckError` gana `file: Option<PathBuf>` (mismo patrón que `span: Option<Span>`, pero sin la semántica "primer stamp gana" de `with_span` -- el archivo es constante para todo el subárbol de un ítem, nunca "más específico" según la profundidad). `check_program_full` gana un segundo parámetro, `item_files: &[PathBuf]`, y en cada uno de sus 5 puntos de entrada (`Item::Fn`, los 3 chequeos por `rpc`/`stream` de `Item::Service`, `Item::Const`) ahora itera con `.enumerate()` y estampa `item_files[index]` junto al span. `check_program` (pública, sin `Checker` ni archivos) pasa `&[]` -- `item_files.get(i)` da `None` para cualquier índice, así que los ~113 call sites de error existentes y los tests que arman un `Program` a mano quedan bit-a-bit iguales. `check_program_with_files` es la nueva fachada pública para callers que SÍ tienen `item_files` (no pueden llamar a `check_program_full` directo: es `pub(crate)` de la librería, invisible desde el crate binario aunque compartan paquete Cargo).

**LSP: goto-definición cruza archivos de verdad.** `get_definition`/`get_definition_inner` ganan `item_files: &[PathBuf]` y `overlay: &HashMap<PathBuf, String>`. Un `respond(index, span)` interno resuelve, para el ítem encontrado, si su archivo real coincide con el documento abierto (`uri`, camino rápido de siempre) o es OTRO archivo -- en ese caso arma la respuesta con el `uri` y el rango calculados sobre el archivo REAL (leído del overlay del editor o de disco), en vez de negarse. `item_files` vacío (el buffer aislado de un test o un documento sin resolver vía `modules.rs`) preserva el comportamiento exacto de antes: todo pertenece a `uri`/`source`. El bug concreto que esto arregla, confirmado con un test de subproceso real contra el binario: goto-def sobre un tipo importado ahora devuelve `{uri: "file:///.../b.link", range: {line: 0, ...}}` en vez de `null`.

**LSP: diagnósticos de tipos ya no degradan el programa ENTERO por tocar más de un archivo.** Antes, `touched.len() > 1` hacía caer TODOS los errores de un programa a un único diagnóstico en posición (0,0) con un mensaje genérico ("podría estar en uno de los N archivos importados") -- aunque el 100% de los errores estuviera en el documento abierto. Ahora cada `CheckError.file` se compara contra el documento que disparó el chequeo: si coincide, snippet con rango real de siempre; si no (vino de un `import`), el protocolo LSP no da forma de apuntar una posición de OTRO archivo dentro de la respuesta de `publishDiagnostics` para `uri` -- así que se nombra el archivo real en el mensaje (mismo criterio que ya usan los errores de sintaxis de `LoadError::Syntax`) en vez de esconder cuál de los N archivos era. Publicar diagnósticos para MÚLTIPLES uris en una sola notificación (el arreglo completo) queda fuera de esta ronda -- ver "Fuera de alcance" abajo.

**CLI: el mismo mecanismo, sin la limitación de "una sola uri".** `main.rs::report_check_errors` no tiene el problema de protocolo del LSP (escribe a stderr, no responde a un `uri` puntual), así que acá el arreglo es completo: CUALQUIER error, en CUALQUIER archivo tocado, ahora sale con su snippet real -- confirmado con `linkc <archivo>` de verdad, tanto para un error en el archivo de entrada como en uno importado (`compiler/tests/cli_multifile_diagnostics.rs`, subproceso real). `report_check_errors` cachea las lecturas de disco por archivo (`source_cache`) en vez de releer el mismo archivo por cada error que le pertenece.

**Fuera de alcance, a propósito:**
- Publicar `publishDiagnostics` para múltiples URIs desde un solo re-chequeo (lo que daría rango real también para un error en un archivo importado, no solo el mensaje nombrándolo) -- requiere trackear y limpiar diagnósticos de archivos que dejan de estar en el cierre transitivo entre un chequeo y el siguiente; una ronda propia, más grande que "adjuntar identidad de archivo".
- Los ítems 1 y 2 de Nivel 3 (§3.21) siguen sin empezar -- no dependen de esta ronda.
- El límite de `Field`/`Param` sin span (§3.21) sigue igual -- resuelto en la siguiente ronda (§3.23), no acá.

---

### 3.23 `Field`/`Param` ganan `name_span` — RESUELTO

§3.21 dejó documentado, como límite honesto encontrado escribiendo sus propios tests: un campo o parámetro cuyo NOMBRE coincide textualmente con una declaración `type`/`enum` existente (`type Point = {...}; type Shape = { Point: Int }`) seguía cayendo al loop viejo de coincidencia-por-palabra al pedir goto-def sobre el nombre de CAMPO `Point` -- saltaba (mal) a `type Point`, porque el cursor ahí no caía dentro de ningún `TypeExpr::Named`. Esta ronda lo cierra.

**Un solo campo nuevo por struct, verificado con grep: exactamente 2 sitios de producción.** `Field` y `Param` ganan `name_span: Span`, cubriendo SOLO el identificador del nombre (mismo criterio que `TypeExpr::Named`, §3.21). A diferencia de la migración `Spanned<Expr>`/`Spanned<Stmt>` (~155 sitios) o incluso `TypeExpr::Named` (~4 sitios), acá hubo literalmente UN sitio de producción real por tipo (`parser.rs::parse_field`, `parser.rs::parse_param`) -- `ClosureParam` (parámetros de un closure `|params| {...}`) queda deliberadamente afuera, es un tercer tipo de parámetro distinto y el bug reportado nunca lo mencionaba. Capturar el span es `let name_span = self.span();` inmediatamente ANTES de `self.eat_ident()` -- mismo patrón que `parse_primary_type` ya usaba para `TypeExpr::Named`, válido porque `eat_ident` no saltea nada antes del identificador.

**`PartialEq` manual, mismo motivo que `TypeExpr` ya tenía.** `Field`/`Param` sacaron `PartialEq` del derive automático (que hubiera empezado a comparar `name_span`, rompiendo en silencio cualquier comparación estructural existente -- ej. `TypeExpr::Struct`'s propio `PartialEq` compara `Vec<Field>` elemento a elemento, y `FnDecl`/`RpcDecl` hacen lo mismo con `Vec<Param>`) y lo reimplementan a mano ignorando `name_span`, exactamente el mismo patrón que `TypeExpr::Named` ya resolvía para su propio span. Cero tests rotos por esto -- confirmado corriendo la suite completa antes y después: los tests existentes ya construían programas parseando texto fuente real, no armando `Field`/`Param` a mano, así que el nuevo campo no tocó ningún sitio de construcción fuera de los 2 de arriba.

**LSP: `is_field_or_param_name_at` + `field_name_at_in_type`, mismo criterio de exhaustividad que `find_named_type_at`.** Dos funciones nuevas en `lsp.rs`, sin brazo `_` en ninguna (agregar una variante de `TypeExpr` rompe la compilación acá, no se ignora en silencio): `field_name_at_in_type` recorre las 8 variantes buscando un `TypeExpr::Struct` en cualquier profundidad (un genérico puede envolver un struct inline, `Box<{ n: Int }>`) y chequea el `name_span` de sus campos; `is_field_or_param_name_at` aplica eso sobre exactamente los mismos lugares que `find_named_type_in_program` ya recorre (`Field` de `type`/`db`/variantes de `enum`, `Param` de `fn`/`rpc`/`stream`). En `get_definition_inner`, corre como un SEGUNDO gate autoritativo, inmediatamente después del de `TypeExpr::Named` (§3.21) y antes del loop viejo: si el offset cae sobre el nombre de un campo/parámetro, responde `None` directamente -- un nombre de campo no es una referencia a otro símbolo (a diferencia de su TIPO, que el primer gate ya resuelve), así que no hay ninguna declaración a la que saltar.

**Verificado que el gate nuevo no es sobre-amplio.** Además del caso que arregla (`test_goto_def_on_a_field_name_that_collides_with_an_existing_type_name_does_not_jump`, `test_goto_def_on_a_param_name_that_collides_with_an_existing_type_name_does_not_jump`), un test cubre la contraparte exacta con el MISMO código: pedir goto-def sobre el TIPO de un campo cuyo nombre coincide con ese mismo tipo (`type Marker = {...}; type Shape = { Marker: Marker }`, cursor sobre el segundo `Marker`) sigue resolviendo a `type Marker` como siempre -- el gate nuevo distingue nombre de campo vs. uso de tipo en vez de tragarse ambos. 345 tests, todos pasando.

**Fuera de alcance, a propósito:**
- `ClosureParam` (parámetros de closure) no ganó `name_span` -- el bug reportado en §3.21 solo mencionaba `Field`/`Param`; si aparece el mismo problema ahí, es una extensión de tamaño similar, no una consecuencia gratis de esta ronda.
- Los ítems 1 y 2 de Nivel 3 (§3.21: hover de expresión arbitraria, completion sensible a `x.`) siguen sin empezar -- resueltos/en curso en §3.24 y §3.25 respectivamente.
- Publicar `publishDiagnostics` multi-URI (§3.22) sigue sin empezar.

---

### 3.24 Hover de expresión arbitraria — RESUELTO, LSP Nivel 3 ronda 2/3

§3.21 dejó esta ronda como "la más cara" del Nivel 3 -- reconstruir el `Env` del checker en vez de una búsqueda puramente sintáctica como las rondas anteriores (§3.21, §3.23). Se investigó primero un diseño alternativo (reimplementar en `lsp.rs` el recorrido de scoping -- params, `let`, bloques de `if`/`match`/closures -- para reconstruir el `Env` activo en un offset "desde afuera") y se descartó: hubiera duplicado ~150-300 líneas de reglas que YA viven en `check_stmt`/`check_block`/`bind_pattern`, con el riesgo real de que diverjan con el tiempo (dos fuentes de verdad para las mismas reglas de scoping). El diseño elegido reusa el checker de verdad, sin reimplementar nada de scoping.

**El "probe" vive en los DOS puntos de entrada unificados de expresión, no en cada `synth_*`/`check_*` interno.** Absolutamente toda expresión del programa pasa por `synth_expr` (modo síntesis, ⇒) o por `check_expr` (modo chequeo, ⇐) en algún momento -- son los dos wrappers públicos que `synth_expr_inner`/`check_expr_inner` (y los ~15 `synth_*`/`check_*` especializados que delegan en ellos) nunca bypasean. Instrumentar esos DOS puntos (no los ~15 internos) alcanza para cubrir el árbol completo: `Checker` gana `hover_target: Option<usize>` (el offset a buscar, `None` en cualquier chequeo normal) y `hover_result: RefCell<Option<(ancho_del_span, Type)>>` (interior mutability porque el checker entero opera con `&self`, nunca `&mut self` -- agregarlo hubiera tocado los ~40 call sites de `check_expr`/`synth_expr`). `synth_expr` guarda el tipo SINTETIZADO cuando su span contiene el offset; `check_expr` guarda `expected` (no hay tipo sintetizado propio en modo chequeo -- pero si el chequeo tuvo éxito, `expected` es por construcción un tipo válido para esa expresión, ej. un `if`/`match`/closure).

**Bug real evitado ANTES de implementarlo, analizando el orden de recursión:** la primera versión de este diseño consideraba "última escritura gana" (sobreescribir sin guardas) -- INCORRECTO. Un nodo padre (ej. `x > 5`) siempre tiene un span que CONTIENE al de sus hijos (`x`, `5`), y el padre termina de procesarse DESPUÉS de que sus hijos ya retornaron (la recursión entra a los hijos antes de que el padre calcule su propio resultado) -- así que "última escritura" se hubiera quedado con el nodo MÁS EXTERNO que contiene el offset, no el más específico: hoverear sobre `x` en `x > 5` hubiera mostrado `Bool` (el tipo de toda la comparación), no `Int` (el tipo real de `x`). `probe_hover` en cambio compara ANCHOS de span -- solo reemplaza el resultado guardado si el nuevo span es más angosto que el mejor visto hasta ahora, sin importar el orden cronológico. Fijado con un test que prueba exactamente este caso (`checker::tests::hover_on_a_param_reference_inside_a_comparison_gives_the_param_type_not_the_comparisons_bool`) antes de dar la ronda por terminada.

**`hover_type_at(program, offset) -> Option<Type>`, el único punto de entrada nuevo (`pub(crate)`).** Encuentra qué `fn`/`rpc`/`stream` tiene un `body.span` (`Block.span`, prerrequisito 3/3 del LSP -- ninguna esta ronda necesitó agregar ningún span nuevo) que contiene `offset`, y llama a `check_fn`/`check_rpc` TAL CUAL sobre ese ítem -- ni siquiera necesita saber cómo se arman los bindings de parámetros, esas funciones ya lo hacen. El resultado real (`Ok`/`Err`) se descarta a propósito: lo único que importa es el efecto colateral sobre `hover_result` vía las llamadas a `synth_expr`/`check_expr` que ese chequeo dispara por su cuenta.

**`lsp::get_hover` reestructurado para no depender de estar sobre un identificador.** Antes, la función entera arrancaba con `let word = get_word_at_pos(...)?;` -- un `?` que cortaba TODO el hover (palabras clave, nombres de declaración, y ahora expresiones) apenas el cursor caía sobre un operador, un literal, o cualquier posición sin una palabra reconocible. La lógica de palabras clave/declaración (Nivel 1/2, sin cambios de comportamiento) se extrajo a `get_hover_for_word`: si no da resultado (incluyendo el caso "no hay ninguna palabra ahí"), `get_hover` sigue con el hover de expresión, que solo necesita un OFFSET, no una palabra -- así que hoverear sobre `>` en `x > 5`, o sobre un literal `5`, ahora también puede resolver, no solo sobre identificadores.

**El tipo se renderiza con `ts_emit::render_type`, el MISMO renderer que emite el `.d.ts` real** (no un volcado de `Debug` de Rust) -- mismo criterio en los dos lugares para lo que un tipo "se ve": `Int` se muestra `number`, un struct declarado muestra su nombre real (`Point`, no una forma anónima), coherente con "el contrato es el código" (PLAN.md §2.1).

**Límite honesto, documentado en el propio código de `hover_type_at`:** `check_fn`/`check_rpc` paran en el PRIMER error dentro de un body -- el checker no tiene recuperación de errores a nivel de SENTENCIA (el parser sí tiene, pero a nivel de ÍTEM completo, prerrequisito 2/3). Si el body tiene un error de tipos ANTES de la expresión que se está hovereando, esa expresión nunca se llega a chequear y esto devuelve `None` -- ausente, no una respuesta incorrecta, pero sí un hueco real. Cerrarlo necesitaría recuperación de errores a nivel de sentencia en el checker, una extensión propia y más grande que esta ronda (test que fija este límite: `checker::tests::hover_stops_at_an_earlier_error_in_the_same_body`).

Verificado con 6 tests directos sobre `hover_type_at` en `checker.rs` (incluyendo el caso decisivo de más arriba), 3 tests sobre `get_hover` en `lsp.rs`, y un test de subproceso real contra el binario (`compiler/tests/lsp_stdio.rs`). 355 tests, todos pasando.

**Fuera de alcance, a propósito:**
- Statement-level error recovery en el checker (el límite documentado arriba) -- una extensión propia, más grande que esta ronda.
- Hover sobre el NOMBRE de un parámetro en la FIRMA (antes del body) -- sigue sin cambios (Nivel 2, coincidencia por palabra, no llega a activar `hover_type_at` porque esa posición está fuera de `body.span`).
- El ítem 3 de Nivel 3 (completion sensible a `x.`, §3.21/§3.25) -- ronda separada, reutiliza esta misma máquina.

---

### 3.25 Completion sensible al tipo del receptor — RESUELTO, LSP Nivel 3 ronda 3/3 (último ítem)

Cierra el Nivel 3 del LSP completo (§3.19 → §3.21 → §3.24 → acá). Antes de esta ronda, `x.` (cualquier receptor) ofrecía SIEMPRE la misma lista fija de los ~15 métodos builtin posibles (de colección, de lista, de string, conversión numérica), todos mezclados, sin mirar el tipo real de `x` -- Nivel 2, no Nivel 3 (§3.19 ya lo documentaba así). Esta ronda reusa `hover_type_at` (§3.24) tal cual: el "tipo del receptor" es exactamente lo mismo que "el tipo de la expresión bajo el cursor" que el hover ya sabía calcular.

**El problema específico de completion (no de hover): el buffer con un `.` colgante casi nunca parsea.** Mientras se escribe "x.", el resto del archivo puede estar perfecto, pero un `.` sin nada después (o un identificador incompleto) es un error de sintaxis real -- y como el parser no tiene recuperación a nivel de sentencia (§3.19), el `fn`/`rpc` que se está editando se cae ENTERO del `Program`, justo el que hace falta para tipar el receptor. Se resuelve con un parche quirúrgico: `receiver_type_before_dot` reemplaza el rango `[offset_del_punto, offset_del_cursor)` por espacios (mismo largo exacto, nunca toca un `\n`) y re-parsea esa COPIA de forma aislada -- todo lo anterior al punto y todo lo posterior al cursor queda byte a byte idéntico al original, así que el receptor y el resto del archivo parsean normal. `char_offset_from_char_position` (nueva, hermana de `char_offset_from_utf16_position` pero contando CARACTERES en vez de unidades UTF-16, la convención que `get_word_at_pos`/`get_line_prefix_at_pos` ya usaban) convierte la longitud del prefijo recortado de vuelta a un offset absoluto sin mezclar las dos convenciones.

**Bug real encontrado escribiendo los tests de esta ronda, corregido en el mecanismo de §3.24 (no algo nuevo de completion en sí):** el tail de un body se chequea en modo ⇐ (`check_expr`) contra el tipo de retorno declarado -- si NO matchea (ej. una función que declara `-> Int` pero su body es en realidad una `List<Int>`), el chequeo falla, pero la SÍNTESIS de esa misma expresión sí había tenido éxito (literalmente lo que el mensaje de error reporta: "se esperaba Int, se encontró List(Int)"). Antes, el probe de `check_expr` solo grababa un tipo en el caso ÉXITO (`expected`, ver §3.24) -- así que un chequeo fallido perdía el tipo real sintetizado por completo, aunque `synth_expr_inner` lo hubiera calculado correctamente unas líneas antes. Arreglado: si el chequeo falla, `check_expr` reintenta una síntesis best-effort del MISMO nodo antes de rendirse -- gateado por `hover_target` (no corre nunca en un chequeo normal) y correcto incluso si es redundante con la síntesis que ya corrió adentro (re-probar sub-expresiones ya grabadas con el mismo ancho nunca las pisa, ver `probe_hover` §3.24). Sin este fix, completion sobre un receptor cuya firma envolvente tuviera CUALQUIER inconsistencia de tipos (algo que pasa todo el tiempo mientras se escribe código a medias) hubiera vuelto a caer en la lista genérica -- justo el escenario más común en la práctica.

**`completions_for_receiver_type`, un match directo sobre `Type`:** `DbCollection` (all/find/insert/applyPatch/delete/deleteWhere/findWhere/subscribe), `List` (length/take/map/filter), `String` (length/contains), `Int`/`Float` (conversión al otro), `Auth` (createSession/destroySession, con las firmas reales de `check_auth_method`) -- y, capacidad NUEVA que ningún tipo de receptor tenía antes, `Struct { fields, .. }` ofrece los NOMBRES DE CAMPO reales como completion (`p.` sobre `p: Point` ahora sugiere `x`/`y`, no solo métodos builtin genéricos). `Type::Db` (el identificador `db` a secas) devuelve `None` a propósito -- ya tenía su propio manejo por texto (`prefix.ends_with("db.")`, listar nombres de colección), que necesita el `Program` completo, no solo el `Type` aislado; no se tocó para no arriesgar esa lógica ya proband. Cualquier tipo no cubierto, o cualquier fallo en la cadena completa (parche → re-parse → `hover_type_at`), cae a la lista genérica de siempre -- esta ronda solo AGREGA precisión, nunca resta lo que ya había.

Verificado con 5 tests directos sobre `get_completions` en `lsp.rs` (lista/string/struct/colección específica/fallback), 1 test directo sobre el fix del checker en `checker.rs`, y 1 test de subproceso real contra el binario (`compiler/tests/lsp_stdio.rs`, campos de un struct real). 362 tests, todos pasando.

**Fuera de alcance, a propósito:**
- El parche-y-reparse es AISLADO (`parser::parse` directo, no `modules::load_program_with_overlay`) -- si el tipo del receptor depende de un `type`/`enum` de un archivo IMPORTADO, cae al fallback genérico en vez de resolverlo. Reconstruir el overlay completo del `LspServer` acá necesitaría que esta función deje de ser libre y pase a ser un método de instancia -- no pedido en esta ronda.
- Completion de un campo ESPECÍFICO dentro de un `Type::Generic` instanciado (`Box<Point>.`) -- `completions_for_receiver_type` no tiene un brazo para `Generic`, cae al fallback genérico (necesitaría `expand_generic_struct`, ya `pub(crate)`, pero no conectado acá).
- Nada de esto reemplaza el filtrado del lado del CLIENTE (VS Code, etc.) sobre lo que el usuario ya tipeó después del `.` -- sigue asumiendo el trigger character estándar (un `.` recién tipeado, sin texto parcial todavía), mismo alcance que la lista genérica ya tenía antes de esta ronda.

Con esto, el Nivel 3 del LSP completo queda resuelto: los 3 ítems que §3.19/§3.21 dejaban pendientes (goto-def de tipo en firma, hover de expresión arbitraria, completion sensible a tipo) están hechos.

---

### 3.26 Observabilidad: tracing estructurado por RPC — RESUELTO, v0

PLAN.md §4 (Fase 2) la nombraba junto al package manager como pendiente. `runtime/server.rs` ya tenía un `req_id` incremental (agregado como prerrequisito parcial para poder correlacionar líneas de log entre el hilo principal y los hilos de escritura de `stream`) -- esta ronda es lo que faltaba encima de eso.

**Una línea por request COMPLETADA, formato `clave=valor` -- greppable sin parsear JSON.** `log_done(req_id, method, status, start, extra)` es el único punto de emisión: `[req {id}] method={service}.{rpc} status={code} duration_ms={ms}` (+ `{extra}` si no está vacío). Mismo espíritu que el formato de texto de `tracing`/los logs de Heroku -- no se suma la dependencia `tracing` para esto, `println!` con un formato consistente ya alcanza para un v0 (agregar salida JSON estructurada, o niveles de log configurables, sería la extensión natural si hiciera falta después).

**Tres piezas nuevas sobre el log de "request recibida" que ya existía:**
- **Duración real** (`duration_ms`), un `Instant::now()` capturado al entrar y restado en cada punto de salida -- incluyendo los hilos de escritura de `stream`/`stream` en vivo (`start` se les pasa junto con `req_id`), así que la duración de un stream cubre el envío completo, no solo el cómputo inicial en el hilo principal.
- **El método real** (`method=Users.create`, no la ruta cruda) -- ya se conocía en cada rama existente (`service_name`/`rpc_name` de `parse_path`), esta ronda solo lo agrega al log de salida. `None` (`method=-`) para los pocos casos que nunca llegan a resolverlo (un 404 por URL mal formada).
- **El mensaje de error en la propia línea de log**, no solo el código de status. Antes, un 401/400/500 solo mostraba el número -- para saber QUÉ pasó había que inspeccionar la respuesta por otro lado (un `curl -v`, ver el cliente generado fallar). Ahora `error="..."` va en la misma línea. Para el camino de `handle_rpc` (la mayoría de los rpc), el body de error es `{"error": "<mensaje>"}` -- se extrae el mensaje real en vez de loguear el JSON completo escapado adentro de otro string (`error="{\"error\":\"...\"}"`, técnicamente correcto pero feo de leer); si el body no tiene esa forma exacta por algún motivo, cae al body crudo en vez de esconder la falla.

**Los casos de desconexión de un `stream` (antes texto libre) ahora usan el mismo formato**, con campos propios (`client_disconnected=true stage=snapshot sent=N`, etc.) en vez de una oración armada a mano -- consistente con el resto, aunque conceptualmente no sean "un error" (la respuesta 200 ya se había mandado; es el cliente el que se fue).

Verificado con un servidor real: los 4 casos (éxito, 404 por ruta desconocida, 401 por auth, 500 por servicio desconocido) dan líneas limpias y completas -- confirmado leyendo el stdout real del proceso, no solo por inspección de código. El demo insignia completo (`frontend/src/main.ts` contra un servidor real) también se corrió de punta a punta para confirmar que el refactor de logging no cambió ningún comportamiento funcional. 371 tests, todos pasando (esta ronda no agregó tests nuevos -- el logging en sí no es una superficie que este proyecto testee con asserts, mismo criterio que ya regía para el `req_id`/formato de log anteriores; se verificó leyendo stdout real, el mismo método que ya usaba la auditoría original de este mismo módulo).

**Fuera de alcance, a propósito:**
- Salida estructurada en JSON (para ingestión por un colector de logs real) -- el formato `clave=valor` alcanza para un v0 de un solo proceso sin infraestructura de observabilidad detrás.
- Niveles de log configurables (`--verbose`/`RUST_LOG`) -- hoy todo sale siempre, sin flag para silenciar ni para pedir más detalle.
- Métricas agregadas (percentiles de latencia, tasa de error) -- esto es tracing por request individual, no una capa de métricas encima.

---

### 3.27 Hot reload real en `linkc dev` — RESUELTO, v0

PLAN.md §4 (Fase 2) lo nombraba junto a `LSP completo`/`package manager`/`observabilidad`. Antes, `linkc dev <archivo> <outdir>` observaba y reconstruía el contrato (`contract.d.ts`/`client.ts`/`validators.ts`/`link.lock`) pero nunca tocaba un servidor -- correr el backend en paralelo seguía siendo `linkc serve` aparte, sin ninguna conexión entre los dos.

**`linkc dev <archivo> <outdir> [puerto]` -- el `[puerto]` es opcional y retrocompatible.** Sin él, comportamiento IDÉNTICO a antes de esta ronda. Con él, cada rebuild EXITOSO reinicia un `linkc serve` HIJO real con el programa actualizado.

**Restart de proceso, no hot-swap en memoria -- decisión deliberada.** `spawn_serve_child` reinvoca el propio binario (`env::current_exe()`) con `serve <archivo> <puerto>`, reusando `cmd_serve`/`runtime::server::serve` TAL CUAL, sin ningún cambio. La alternativa (mutar el `Program` de un servidor YA CORRIENDO) hubiera necesitado tocar el modelo de threading que `runtime/server.rs` ya documenta con cuidado (`Value::Closure`/`Rc` no cruzan un borde de hilo, GRAMMAR.md §3.13) -- un restart de proceso es más simple de razonar y más robusto, al costo de perder las conexiones `stream` abiertas en cada reload (aceptable en modo desarrollo, no sería aceptable en producción, pero esto es explícitamente `linkc dev`, nunca `linkc serve` en frío).

**Un rebuild FALLIDO nunca tira abajo el servidor.** Si el rebuild que sigue a un cambio de archivo falla (error de sintaxis/tipos mientras se edita), el hijo de la ÚLTIMA versión válida sigue sirviendo sin tocarse -- mismo criterio que un dev server de frontend real (Vite/webpack) que sigue sirviendo el último build bueno en vez de caerse por un typo a medio escribir. Solo un rebuild EXITOSO mata al hijo viejo (`kill_serve_child`, por su PID exacto -- nunca un kill por nombre de imagen, para no afectar otro `linkc serve` que el usuario tenga corriendo aparte) y levanta uno nuevo.

**Persistencia de datos entre reloads: gratis, por diseño ya existente.** `db_path` se deriva de `<archivo>.db` (GRAMMAR.md §3.17) -- el mismo archivo en cada restart, así que los datos de la sesión de desarrollo sobreviven un hot reload sin ningún código nuevo. Si el reload además cambia la FORMA de `db { ... }`, el hijo nuevo falla fuerte con el mismo diff-y-remedio de siempre (§3.17) al reabrir un schema incompatible -- comportamiento heredado, no una brecha nueva de esta ronda.

**Límite honesto sobre limpieza al salir.** Sin manejo de señales explícito: `Command::spawn()` sin `CREATE_NEW_PROCESS_GROUP` deja al hijo en el mismo grupo de proceso/consola que el padre en ambas plataformas, así que un Ctrl+C real en una terminal interactiva le llega TAMBIÉN al hijo -- el camino verificado manualmente. Un kill programático dirigido SOLO al PID del proceso padre (no un Ctrl+C real desde una terminal) es el caso que sí puede dejar al hijo huérfano sirviendo el puerto -- límite de v0 conocido, no manejado, mismo tipo de limitación que `gitdep::resolve` ya documenta para el locking entre procesos (§2.1).

**Verificado manualmente de punta a punta contra el binario real** (no un test automatizado -- ver por qué abajo): `linkc dev` con un archivo mínimo (`service Ping { rpc version() -> Int { 1 } }`) y puerto, confirmando en cada paso contra el servidor real vía `curl`: (1) arranque inicial sirviendo `1`; (2) al editar el archivo a `2`, detección del cambio, rebuild, kill del PID viejo, spawn de un PID nuevo, y `curl` devolviendo `2`; (3) al introducir un error de tipos, el rebuild falla con el snippet real de siempre y el servidor del PID anterior sigue sirviendo `2` sin interrupción. 371 tests automatizados (sin cambios de este archivo -- ver la nota de alcance de tests abajo), todos pasando.

**Por qué sin test automatizado:** `cmd_dev` es un loop infinito e interactivo (nunca antes tuvo cobertura automatizada, ni siquiera para su comportamiento de observar-y-reconstruir previo a esta ronda) que ahora además administra un PROCESO HIJO -- un test de subproceso real necesitaría descubrir el PID del hijo (parseando stdout) para limpiarlo aparte, porque matar solo al PID del padre programáticamente (a diferencia de un Ctrl+C real) no arrastra al hijo (ver el límite de arriba). Se prefirió verificación manual real y exhaustiva (3 escenarios, contra el binario real) antes que forzar un harness de test alrededor de una herramienta pensada para uso interactivo en primer plano.

**Fuera de alcance, a propósito:**
- Preservar conexiones `stream` abiertas a través de un reload -- se cortan y el cliente debe reconectar, igual que cualquier restart de servidor.
- Limpieza del hijo ante un kill programático del padre (el límite de arriba).
- Debounce de múltiples cambios de archivo muy seguidos -- cada mtime distinto dispara su propio rebuild+restart, como ya hacía el `linkc dev` sin servidor.

---

### 3.28 Fase 3 (PLAN.md §4): política de estabilidad de sintaxis, y por qué source maps NO se persigue todavía

Última pieza del backlog de la auditoría post-push que arrancó con la Ronda 0. PLAN.md §4 nombra, para Fase 3 (1.0, producción, "+6–12 meses, 4–6+ personas"), dos entregables puntuales: "estabilidad de sintaxis" y "debugging con source maps". Ninguno de los dos es una feature que se pueda "implementar" como las anteriores -- son, respectivamente, una DECISIÓN de política y una decisión de NO-hacer, y esta sección las deja explícitas en vez de dejarlas flotando sin resolución.

**Congelar la sintaxis ahora sería prematuro -- decisión consciente, no un olvido.** El propio [README](README.md) sigue abriendo con "This repo is the **Phase 0 MVP** ... It is not a production-ready language". Comprometerse a una sintaxis inmutable ANTES de que exista un solo usuario externo real usándola sería fijar en piedra decisiones (§2.3 nullability, §3.5 manejo de errores, y cada `RESUELTO` de la sección 3 de arriba) que todavía no pasaron la prueba de fuego de un caso de uso ajeno -- exactamente el tipo de compromiso prematuro que PLAN.md §7 ya identifica como uno de los riesgos principales de un lenguaje nuevo.

**Política aplicada en su lugar, efectiva desde esta ronda:** mientras la versión declarada en `compiler/Cargo.toml` sea `0.x` (hoy `0.1.0`), un cambio de sintaxis que rompa un `.link` existente se documenta en el `CHANGELOG` de su propio commit (mismo criterio que esta auditoría entera ya viene aplicando: cada ronda que cambió comportamiento lo dice explícitamente en README/GRAMMAR, nunca en silencio) pero NO requiere ningún proceso de deprecación ni compatibilidad hacia atrás. Recién en `1.0.0` esta libertad se cierra: un cambio incompatible pasa a requerir una migración documentada (o un nuevo mecanismo de edición/versión de lenguaje, al estilo `edition` de Rust, si para entonces hay motivo real de necesitarlo -- decisión que le corresponde a esa ronda futura, no a esta). Esto no es una promesa nueva inventada acá: es simplemente hacer explícito lo que SemVer ya dice sobre una versión `0.x`, para que quede escrito una vez en vez de asumido.

**Source maps: valor genuinamente incierto con la arquitectura actual, no simplemente "no hubo tiempo".** La razón habitual para pedir source maps es mapear código GENERADO (JS transpilado, minificado) de vuelta al fuente original durante una sesión de debugging. Acá:
- La lógica de negocio real (el cuerpo de cada `rpc`/`fn`) corre en el INTÉRPRETE de Rust (`runtime/mod.rs`), nunca se transpila a JS/TS -- no hay ningún paso de compilación de ESE código para el que un source map tenga sentido. Un error de runtime ahí ya sale con la posición real en el `.link` fuente (`diagnostics.rs`, GRAMMAR.md prerrequisitos 1-3 del LSP), sin necesitar ningún mapeo.
- Lo único que SÍ se genera hacia TS (`contract.d.ts`/`client.ts`/`validators.ts`, `ts_emit.rs`/`validators_emit.rs`) es deliberadamente FINO -- interfaces y un cliente RPC que arma un `fetch()`, sin lógica propia que alguien necesite pisar con un breakpoint y "step into" hacia el `.link` original. Pisar un breakpoint DENTRO de `client.ts` ya te deja en TypeScript legible, generado pero no ofuscado ni minificado -- el caso de uso que un source map resuelve (código irreconocible) no se da acá.
- El único lugar donde HOY se emite bytecode de verdad no legible por un humano es `linkc wasm` (§3.20) -- explícitamente congelado esta misma auditoría, alcance mínimo, no el camino de producción.

Dado esto, la recomendación es NO perseguir source maps como una ronda propia hasta que la arquitectura cambie de forma que los vuelva necesarios (ej. si algún día existe un compilador real hacia JS del CUERPO de un `rpc`, no solo del cliente) -- perseguirlos ahora sería construir infraestructura para un problema que este diseño concreto no tiene todavía. Si aparece un caso real y concreto de "no puedo debuggear X" que un source map resolvería, esa necesidad puntual es la que debería disparar la ronda, no esta lista de tareas.

**Con esto, el backlog completo de la auditoría post-push queda resuelto o explícitamente decidido: nada se dejó flotando sin una razón escrita.** Ver la sección "Estado" del [README](README.md) para el resumen de qué se hizo en cada ronda.

### 3.29 `linkc test`: contrato contra un snapshot commiteado (PLAN.md §5, "tests de contrato")

PLAN.md §5 nombra, en la lista de herramientas de ecosistema, "Testing: runner integrado + tests de contrato (que el `.d.ts` generado no rompa sin querer)" -- el único ítem de esa lista que seguía sin una v0 real (CLI, LSP, package manager, debugging/observabilidad y las integraciones vía interop nativa ya estaban resueltos, ver "Estado" del [README](README.md)).

**Qué hace.** `linkc test <archivo.link> <archivo.snap> [--update]` genera el mismo trío que `linkc build` (`contract.d.ts`, `client.ts`, `validators.ts`, vía los mismos emisores) y lo compara contra un snapshot de texto plano. Sin snapshot previo, lo crea y avisa que hay que commitearlo -- esa corrida establece la base. Con un snapshot que matchea, sale OK sin tocar nada. Con un snapshot que difiere, falla (`ExitCode::FAILURE`) y muestra el diff línea a línea; `--update` acepta el contrato nuevo como la base siguiente.

**Por qué un archivo de texto commiteado, y no algo dentro de `outdir`.** `outdir` (`gen/` en este repo) está en `.gitignore` -- se regenera en cada build, nunca sobrevive entre commits. Un snapshot necesita sobrevivir precisamente para servir de algo: comparar el contrato de HOY contra el de la ÚLTIMA VEZ QUE ALGUIEN LO REVISÓ, no contra el de hace un segundo. Por eso el snapshot vive en una ruta separada que el usuario elige (en este repo, `examples/users.link.snap`, sibling del `.link`, fuera de `gen/`) y se commitea a git como cualquier otro archivo fuente.

**Por qué falla en vez de sobreescribir.** Que el contrato haya cambiado puede ser una ronda legítima (agregar un campo, un rpc nuevo) o el bug exacto que esta feature existe para atrapar (un rename accidental, un tipo que cambió de forma sin que nadie se diera cuenta). Un comando que sobreescribe solo no distingue los dos casos -- por eso `--update` es un paso explícito, no el default, y CI (`.github/workflows/ci.yml`) corre `linkc test` SIN `--update`: un PR que cambia el contrato del demo insignia sin commitear el `.snap` actualizado falla la build, con el diff real en el log.

**El diff es un LCS real (programación dinámica), no una comparación posición-a-posición.** Una comparación ingenua línea-por-línea (línea 5 vieja vs línea 5 nueva, línea 6 vs línea 6, ...) marca como "distinta" cada línea después de una sola inserción o borrado, aunque el resto del archivo sea idéntico -- inútil para revisar un cambio real. El algoritmo hand-rolled (mismo espíritu que el SHA-256 de `lockfile.rs`: chico, estable, autocontenido, sin depender de un crate nuevo para algo bien entendido) encuentra la subsecuencia común más larga entre las líneas viejas y nuevas y reporta inserciones/borrados reales. Guarda de tamaño: la tabla LCS es O(n×m) en memoria -- trivial para un contrato de cientos de líneas, así que por encima de ~2000×2000 líneas el comando se rehúsa a construir la tabla completa y devuelve un mensaje en vez de arriesgar un uso de memoria sin cota -- no una ruta esperada hoy, pero tampoco silenciosa si algún día pasa.

**Límites honestos de v0.** Es "tests de contrato", no el "runner integrado" completo que PLAN.md §5 nombra en la misma línea -- no hay forma de escribir assertions sobre el COMPORTAMIENTO de un `rpc` (ej. "`Users.create` con este input devuelve este output") dentro de un `.link`, solo sobre la FORMA del contrato que genera. Escribir un framework de tests real embebido en el lenguaje (sintaxis `test { }`, aserciones, un runner que invoque rpcs contra una `db` de prueba) es una feature de lenguaje nueva -- semanas, no una ronda -- y queda fuera de esta ronda a propósito, mismo criterio que ya se aplicó al no perseguir source maps sin un caso concreto (§3.28). Si aparece la necesidad real de testear comportamiento (no solo forma), esa necesidad puntual debería disparar esa ronda, no esta lista.

Verificado con tests de integración reales contra el binario compilado (`compiler/tests/cli_test_snapshot.rs`): primera corrida crea el snapshot, corrida sin cambios matchea, un cambio real (rename de campo) falla mostrando el campo nuevo en el diff, y `--update` acepta el cambio y vuelve a matchear después. Dogfooded sobre el propio demo insignia: `examples/users.link.snap` está commiteado y CI lo verifica en cada push/PR (ver el paso nuevo en `.github/workflows/ci.yml`).

**Bug real, encontrado por CI en el primer push, no en revisión local: falso positivo de "cambió" en `windows-latest`.** El primer commit de esta ronda pasó local y en `ubuntu-latest`, pero falló en `windows-latest` -- `linkc test` reportaba que el contrato del demo insignia había cambiado, con un diff VACÍO (contradictorio: "cambió" pero sin mostrar qué). Causa real: este repo tiene `core.autocrlf=true`, así que el checkout en un runner Windows convierte `examples/users.link.snap` (commiteado en LF -- `linkc` nunca escribe `\r\n`) a CRLF en el disco del runner; la comparación `previous == current` es sobre los bytes crudos, así que un `\r` de más alcanza para que nunca matcheen, en TODA corrida sobre ese checkout. El diff vacío fue una segunda capa del mismo bug: `diff_lines` opera sobre `str::lines()`, que sí ignora `\r\n` vs `\n` al partir líneas -- así que el diff, corriendo sobre las mismas dos cadenas, no encontraba ninguna línea distinta, aunque la comparación de arriba ya hubiera decidido que sí. Fix real en `cmd_test` (`main.rs`): normalizar `\r\n` → `\n` tanto en el snapshot leído como en el contrato recién generado ANTES de cualquier comparación -- así la corrección del comando no depende de `core.autocrlf` de la máquina que lo corre, ni de que `.gitattributes` esté bien configurado (que también se agregó, fijando `*.snap` como LF, pero como higiene del diff commiteado, no como el fix en sí). Test de regresión que reproduce el bug a mano (reescribe el snapshot con CRLF sin depender de ninguna configuración real de git, para ser determinista en cualquier máquina) en `cli_test_snapshot.rs`. 376 tests, todos pasando -- verificado de nuevo en CI real en ambos sistemas operativos después del fix, no solo local, ya que el bug NUNCA se reprodujo localmente en primer lugar.

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
| `stream f(...) -> T` | `AsyncIterable<T>` | eventos SSE reales (`data: ...\n\n`), uno por `T` serializado, sobre chunked transfer | §3.13: cuerpo genérico, repite una lista ya calculada. §3.16: cuerpo `while true { db.<col>.subscribe() }`, push real de eventos futuros |
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

El compilador está construido y vive en `compiler/` (Rust; dependencias en `Cargo.toml` — `tiny_http`/`serde_json`/`serde` para el runtime del demo, más `wasm-encoder` §3.20 y `rusqlite` §3.17, agregadas a propósito y documentadas donde se justifican, no un descuido). Para el estado real y actualizado de qué está hecho y qué no, ver la sección "Estado" del [README](README.md) — este documento describe el LENGUAJE, no el avance del proyecto. Cada gap de diseño que se fue cerrando tiene su propia sección `§3.X — RESUELTO` acá arriba, incluyendo lo que quedó deliberadamente afuera y por qué.
