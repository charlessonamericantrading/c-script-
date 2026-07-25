// Árbol de sintaxis abstracta — refleja 1:1 las producciones de GRAMMAR.md §2.
//
// No se trackean Spans por nodo en esta iteración (deliberado, no descuido):
// los errores de parseo ya referencian el span del token donde fallaron, que
// alcanza para el MVP. Si hace falta señalar errores del type checker sobre
// un nodo ya construido, se añade entonces — no antes.

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Import(ImportDecl),
    Type(TypeDecl),
    Enum(EnumDecl),
    Service(ServiceDecl),
    Const(ConstDecl),
    Fn(FnDecl),
    Db(DbDecl),
}

/// `db { users: User[], posts: Post[] }` (GRAMMAR.md §2.1) -- "DB tipada"
/// v0. `db` NO es palabra reservada (así una variable/campo llamado `db`
/// sigue siendo válido en cualquier otro lado) -- se reconoce por texto
/// ("db" seguido de `{`) solo en posición de ítem de nivel superior, igual
/// que un contextual keyword. Reusa `field_list`/`type_expr` tal cual --
/// sin gramática nueva para el tipo de cada colección, el checker exige
/// que cada uno resuelva a un struct con un campo `id: Int` (no la
/// gramática, ver checker.rs).
#[derive(Debug, Clone, PartialEq)]
pub struct DbDecl {
    pub collections: Vec<Field>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportDecl {
    pub names: Vec<String>,
    pub from: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeDecl {
    pub name: String,
    pub type_params: Vec<String>,
    pub ty: TypeExpr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumDecl {
    pub name: String,
    pub type_params: Vec<String>,
    pub variants: Vec<Variant>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    pub name: String,
    /// `None` = variante unitaria (`Admin`). `Some(vec![])` es sintácticamente
    /// posible (`Foo {}`) aunque poco útil; se acepta sin caso especial.
    pub fields: Option<Vec<Field>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: String,
    pub optional: bool, // el `?` ANTES de `:` (x?: T) — distinto de Optional(T) en TypeExpr
    pub ty: TypeExpr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConstDecl {
    pub name: String,
    pub ty: TypeExpr,
    pub value: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ServiceDecl {
    pub name: String,
    pub members: Vec<Member>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Member {
    Rpc(RpcDecl),
    Stream(RpcDecl),
}

#[derive(Debug, Clone, PartialEq)]
pub struct RpcDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: TypeExpr,
    pub body: Block,
    pub annotation: Option<Annotation>,
}

/// Auth v0 (GRAMMAR.md §3.14): a lo sumo UNA anotación por rpc/stream --
/// nunca una lista. `@requires` implica autenticado (además del rol); no hay
/// forma de pedir "cualquiera de estos N roles" en v0.
#[derive(Debug, Clone, PartialEq)]
pub enum Annotation {
    Authenticated,
    Requires { enum_name: String, variant_name: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub ty: TypeExpr,
    pub default: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FnDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: TypeExpr,
    pub body: Block,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeExpr {
    /// `identifier [type_args]` — incluye tipos primitivos (Int, String, ...),
    /// nombres de type/enum declarados, y genéricos instanciados (Result<A,B>).
    Named(String, Vec<TypeExpr>),
    /// `{ field_list }`
    Struct(Vec<Field>),
    /// `{ K: V }`
    Map(Box<TypeExpr>, Box<TypeExpr>),
    /// `(A, B, ...)` — requiere ≥2 elementos tras desambiguar con agrupación (§2.2)
    Tuple(Vec<TypeExpr>),
    /// `(A, B) -> C`
    Function(Vec<TypeExpr>, Box<TypeExpr>),
    /// postfix `?`
    Optional(Box<TypeExpr>),
    /// postfix `[]`
    List(Box<TypeExpr>),
    /// `A | B | C`
    Union(Vec<TypeExpr>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub tail: Option<Box<Expr>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Let {
        name: String,
        mutable: bool,
        ty: Option<TypeExpr>,
        value: Expr,
    },
    Return(Option<Expr>),
    Expr(Expr),
    /// `x = expr;` -- solo variables simples (no `obj.field = ...` ni
    /// `arr[i] = ...` todavía). El checker exige que `x` haya sido
    /// declarada con `mut` (GRAMMAR.md §2.3).
    Assign {
        name: String,
        value: Expr,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Null,
    Ident(String),
    FieldAccess {
        base: Box<Expr>,
        field: String,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    /// `[e1, e2, ...]` -- vacío (`[]`) solo es válido en modo chequeo,
    /// ver checker.rs (no se puede sintetizar un elemento de la nada).
    ArrayLit(Vec<Expr>),
    /// `base[index]` -- postfix, ver GRAMMAR.md §2.3.
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
    },
    /// `(e1, e2, ...)` -- distinto de `Paren` (agrupación) por la misma
    /// regla de la coma obligatoria que ya usa el nivel de tipos (§2.2).
    TupleLit(Vec<Expr>),
    /// `base.0`, `base.1`, ... -- acceso posicional. Un solo nivel: `t.0.1`
    /// NO encadena (ver nota del lexer en GRAMMAR.md §2.3), es una
    /// limitación conocida, no un error silencioso.
    TupleIndex {
        base: Box<Expr>,
        index: usize,
    },
    /// `Nombre { campos }` o `Enum.Variante { campos }` (GRAMMAR.md §2.3 struct_or_variant_lit).
    StructLit {
        name: String,
        variant: Option<String>,
        fields: Vec<(String, Expr)>,
    },
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    If {
        cond: Box<Expr>,
        then_block: Block,
        else_block: Block,
    },
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
    },
    Paren(Box<Expr>),
    /// `|params| { block }` (GRAMMAR.md §3.10) -- SIEMPRE un bloque con
    /// llaves, nunca una expresión suelta (no existe "bloque como
    /// expresión general" en el lenguaje; esto reusa `Block` tal cual, sin
    /// inventar ese concepto). Sin params implica `||`, que lexea como un
    /// solo token `PipePipe` distinto de `Pipe` -- alcance v0 deliberado:
    /// closures de 0 parámetros no tienen consumidor real todavía
    /// (`.map`/`.filter` siempre pasan 1 argumento), así que no se soportan.
    Closure {
        params: Vec<ClosureParam>,
        body: Block,
    },
}

/// `nombre` o `nombre: Tipo` dentro de `|...|`. Sin anotación solo es válido
/// cuando el closure se chequea (⇐) contra un `Type::Function` ya conocido
/// (ej. el callback de `.filter`/`.map`) -- `checker.rs::synth_expr` exige
/// que TODOS estén anotados si el closure se sintetiza sin ese contexto.
#[derive(Debug, Clone, PartialEq)]
pub struct ClosureParam {
    pub name: String,
    pub ty: Option<TypeExpr>,
}

/// GRAMMAR.md §3.7 — sin coerción implícita entre variantes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    pub pattern: Pattern,
    /// `pattern if guard => body` (GRAMMAR.md §3.3). Un arm con guard NUNCA
    /// descarta exhaustividad por sí solo -- la condición podría ser falsa
    /// en runtime, así que el checker lo trata como si no cubriera nada.
    pub guard: Option<Expr>,
    pub body: MatchArmBody,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MatchArmBody {
    Expr(Expr),
    Block(Block),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    /// binding simple, incluye `_` (wildcard)
    Bind(String),
    /// `1`, `"texto"`, `true`/`false` (GRAMMAR.md §3.3) -- deliberadamente
    /// SIN Float (comparar floats por igualdad exacta es la misma trampa
    /// que Rust terminó prohibiendo en sus propios patrones) y sin `null`
    /// (matchear un `T?` directamente queda para cuando exista ese diseño;
    /// hoy la forma de testear nullability es `== null` en un `if`, §3.7).
    Literal(LiteralPattern),
    /// `Enum.Variante { campo: patrón, ... }` — la variante unitaria sin
    /// llaves (`Enum.Variante`) se representa con `fields: None`.
    Variant {
        enum_name: String,
        variant_name: String,
        fields: Option<Vec<FieldPattern>>,
    },
    /// `P1 | P2 | ...` (GRAMMAR.md §3.3). Alcance v0: ninguna alternativa
    /// puede introducir bindings -- exigir que las N alternativas liguen
    /// exactamente las mismas variables del mismo tipo (la regla real de
    /// Rust) es la parte cara de or-patterns; acá se prohíbe bindear del
    /// todo dentro de un `Or` (el checker lo rechaza explícitamente), que
    /// cubre el caso común (combinar variantes/literales con un mismo
    /// cuerpo) sin la complejidad de reconciliar bindings entre ramas.
    Or(Vec<Pattern>),
    /// `nombre: Tipo` -- narrowing de un valor de tipo unión a su miembro
    /// concreto (GRAMMAR.md §3.9), ej. `i: Int` o `u: User`. Reusa el `:`
    /// que ya significa "nombre tiene este tipo declarado" en todos lados
    /// (let, params, campos de struct) en vez de inventar puntuación nueva
    /// (`is`/`as`). Nombre primero, tipo segundo -- mismo orden que
    /// `Param`/`Field`/`FieldPattern`. El tipo se parsea como un tipo
    /// POSTFIJO (parser.rs::parse_postfix_type), nunca uno con `|` de nivel
    /// superior -- así un `|` que sigue queda para el propio or-pattern que
    /// lo rodea (`i: Int | s: String`), no se lo come la anotación.
    Type(String, TypeExpr),
}

#[derive(Debug, Clone, PartialEq)]
pub enum LiteralPattern {
    Int(i64),
    Str(String),
    Bool(bool),
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldPattern {
    pub name: String,
    /// El shorthand `x` (sin `: patrón`) se expande en el parser a
    /// `Pattern::Bind(x)`, así el resto del compilador no necesita conocer
    /// la abreviatura — ya llega desugared.
    pub pattern: Pattern,
}
