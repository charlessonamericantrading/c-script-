// Árbol de sintaxis abstracta — refleja 1:1 las producciones de GRAMMAR.md §2.
//
// Cada `Expr`/`Stmt` viaja envuelto en `Spanned<T>` (definido debajo), y una
// decena de tipos de declaración (`TypeDecl`, `FnDecl`, `RpcDecl`, `Block`,
// `MatchArm`, etc.) cargan además un campo `span: Span` plano -- lo que hace
// falta para que un error del type checker señale una posición real del
// código fuente, no solo los de sintaxis (que ya podían antes, vía el span
// de cada `Token`). `Pattern`/`TypeExpr`/`Param`/`Field`/`Variant` quedan
// deliberadamente SIN span propio por ahora.

use crate::token::Span;

/// Envoltorio genérico que asocia un `Span` a un nodo del AST. La igualdad
/// (implementada A MANO, no derivada) ignora el span a propósito: dos
/// expresiones son "la misma" si representan el mismo árbol, sin importar de
/// dónde del archivo vinieron. Esto es lo que deja a los sitios que ya
/// comparan `Expr`/`Stmt` a mano (tests) sin cambios de comportamiento, y es
/// lo correcto de cara a un LSP real futuro (reanálisis incremental: "¿esta
/// declaración cambió?").
#[derive(Debug, Clone)]
pub struct Spanned<T> {
    pub node: T,
    pub span: Span,
}

impl<T: PartialEq> PartialEq for Spanned<T> {
    fn eq(&self, other: &Self) -> bool {
        self.node == other.node
    }
}

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
    Test(TestDecl),
}

/// Bloque de test integrado `test "nombre" { ... }` (PLAN.md §5, Eje 2)
#[derive(Debug, Clone, PartialEq)]
pub struct TestDecl {
    pub name: String,
    pub name_span: Option<Span>,
    pub body: Block,
    pub span: Span,
}

/// `db { users: User[], posts: Post[] }` (GRAMMAR.md §2.1) -- "DB tipada"
/// v0. `db` NO es palabra reservada (así una variable/campo llamado `db`
/// sigue siendo válido en cualquier otro lado) -- se reconoce por texto
/// ("db" seguido de `{`) solo en posición de ítem de nivel superior, igual
/// que un contextual keyword. Reusa `field_list`/`type_expr` tal cual --
/// sin gramática nueva para el tipo de cada colección, el checker exige
/// que cada uno resuelva a un struct con un campo `id: Int` (no la
/// gramática, ver checker.rs).
#[derive(Debug, Clone)]
pub struct DbDecl {
    pub collections: Vec<Field>,
    pub span: Span,
}

impl PartialEq for DbDecl {
    fn eq(&self, other: &Self) -> bool {
        self.collections == other.collections
    }
}

#[derive(Debug, Clone)]
pub struct ImportDecl {
    pub names: Vec<String>,
    pub from: String,
    pub span: Span,
}

impl PartialEq for ImportDecl {
    fn eq(&self, other: &Self) -> bool {
        self.names == other.names && self.from == other.from
    }
}

#[derive(Debug, Clone)]
pub struct TypeDecl {
    pub name: String,
    pub type_params: Vec<String>,
    pub ty: TypeExpr,
    pub span: Span,
}

impl PartialEq for TypeDecl {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.type_params == other.type_params && self.ty == other.ty
    }
}

#[derive(Debug, Clone)]
pub struct EnumDecl {
    pub name: String,
    pub type_params: Vec<String>,
    pub variants: Vec<Variant>,
    pub span: Span,
}

impl PartialEq for EnumDecl {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.type_params == other.type_params && self.variants == other.variants
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    pub name: String,
    /// `None` = variante unitaria (`Admin`). `Some(vec![])` es sintácticamente
    /// posible (`Foo {}`) aunque poco útil; se acepta sin caso especial.
    pub fields: Option<Vec<Field>>,
}

/// `name_span` cubre SOLO el identificador del nombre del campo (GRAMMAR.md
/// §3.22, cierra el límite que §3.21 dejó documentado) -- mismo criterio
/// que `TypeExpr::Named`'s propio `Span`: es lo que le permite al LSP
/// distinguir "el cursor está sobre el NOMBRE de un campo" de "está sobre
/// un USO de tipo", que antes eran indistinguibles y hacían que un campo
/// homónimo a un `type`/`enum` existente (`type Point = {...}; type Shape =
/// { Point: Int }`) saltara mal al pedir goto-def sobre el nombre de campo.
/// `PartialEq` es manual (no derive) para IGNORAR `name_span`, mismo motivo
/// que `TypeExpr` ya no deriva `PartialEq`: dos `Field` estructuralmente
/// iguales en offsets distintos deben seguir siendo `==` (lo usa, entre
/// otros, `TypeExpr::Struct`'s propio `PartialEq` al comparar `Vec<Field>`).
#[derive(Debug, Clone)]
pub struct Field {
    pub name: String,
    pub optional: bool, // el `?` ANTES de `:` (x?: T) — distinto de Optional(T) en TypeExpr
    pub ty: TypeExpr,
    pub name_span: Span,
}

impl PartialEq for Field {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.optional == other.optional && self.ty == other.ty
    }
}

#[derive(Debug, Clone)]
pub struct ConstDecl {
    pub name: String,
    pub ty: TypeExpr,
    pub value: Spanned<Expr>,
    pub span: Span,
}

impl PartialEq for ConstDecl {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.ty == other.ty && self.value == other.value
    }
}

#[derive(Debug, Clone)]
pub struct ServiceDecl {
    pub name: String,
    pub members: Vec<Member>,
    pub span: Span,
}

impl PartialEq for ServiceDecl {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.members == other.members
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Member {
    Rpc(RpcDecl),
    Stream(RpcDecl),
}

/// El span cubre la FIRMA (desde el nombre hasta el return type, incluyendo
/// la `@annotation` si hay una -- ver `parse_member`/`parse_rpc_like` en
/// parser.rs), NO el cuerpo: `body` (un `Block`) ya carga su propio span
/// preciso por statement/expresión, así que un error DENTRO del cuerpo nunca
/// necesita caer de vuelta al span de todo el rpc.
#[derive(Debug, Clone)]
pub struct RpcDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: TypeExpr,
    pub body: Block,
    pub annotation: Option<Annotation>,
    pub span: Span,
}

impl PartialEq for RpcDecl {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.params == other.params
            && self.return_type == other.return_type
            && self.body == other.body
            && self.annotation == other.annotation
    }
}

/// Auth v0 (GRAMMAR.md §3.14): a lo sumo UNA anotación por rpc/stream --
/// nunca una lista. `@requires` implica autenticado (además del rol); no hay
/// forma de pedir "cualquiera de estos N roles" en v0.
#[derive(Debug, Clone, PartialEq)]
pub enum Annotation {
    Authenticated,
    Requires { enum_name: String, variant_name: String },
}

/// `name_span`: mismo criterio y mismo motivo que `Field::name_span` (ver
/// su doc) -- distingue el NOMBRE de un parámetro de un USO de tipo para
/// goto-def (GRAMMAR.md §3.22). `PartialEq` manual por la misma razón:
/// ignora `name_span` para que dos `Param` estructuralmente iguales en
/// offsets distintos sigan siendo `==` (lo usa `FnDecl`/`RpcDecl`'s propio
/// `PartialEq` al comparar `Vec<Param>`).
#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub ty: TypeExpr,
    pub default: Option<Spanned<Expr>>,
    pub name_span: Span,
}

impl PartialEq for Param {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.ty == other.ty && self.default == other.default
    }
}

/// Mismo criterio que `RpcDecl`: el span cubre la firma, no `body`.
#[derive(Debug, Clone)]
pub struct FnDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: TypeExpr,
    pub body: Block,
    pub span: Span,
}

impl PartialEq for FnDecl {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.params == other.params
            && self.return_type == other.return_type
            && self.body == other.body
    }
}

#[derive(Debug, Clone)]
pub enum TypeExpr {
    /// `identifier [type_args]` — incluye tipos primitivos (Int, String, ...),
    /// nombres de type/enum declarados, y genéricos instanciados (Result<A,B>).
    /// El `Span` cubre solo el identificador (no los `type_args`) -- es lo
    /// que permite al LSP resolver goto-definición de un nombre de tipo
    /// escrito en una firma (GRAMMAR.md §3.21). Ninguna otra variante lleva
    /// span propio a propósito: son combinadores sintácticos sin un
    /// identificador escrito al que alguien pediría saltar (el `Int` dentro
    /// de `Int[]` ya es su propio `Named` anidado).
    Named(String, Vec<TypeExpr>, Span),
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

/// A mano, no derivado: el `Span` de `Named` se ignora a propósito (mismo
/// criterio que `Spanned<T>`, arriba) -- dos `TypeExpr` que describen el
/// mismo tipo siguen siendo iguales sin importar en qué offset del archivo
/// se haya escrito cada uno.
impl PartialEq for TypeExpr {
    fn eq(&self, other: &Self) -> bool {
        use TypeExpr::*;
        match (self, other) {
            (Named(n1, a1, _), Named(n2, a2, _)) => n1 == n2 && a1 == a2,
            (Struct(f1), Struct(f2)) => f1 == f2,
            (Map(k1, v1), Map(k2, v2)) => k1 == k2 && v1 == v2,
            (Tuple(t1), Tuple(t2)) => t1 == t2,
            (Function(p1, r1), Function(p2, r2)) => p1 == p2 && r1 == r2,
            (Optional(a), Optional(b)) => a == b,
            (List(a), List(b)) => a == b,
            (Union(a), Union(b)) => a == b,
            _ => false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Block {
    pub stmts: Vec<Spanned<Stmt>>,
    pub tail: Option<Box<Spanned<Expr>>>,
    pub span: Span,
}

impl PartialEq for Block {
    fn eq(&self, other: &Self) -> bool {
        self.stmts == other.stmts && self.tail == other.tail
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Let {
        name: String,
        mutable: bool,
        ty: Option<TypeExpr>,
        value: Spanned<Expr>,
    },
    Return(Option<Spanned<Expr>>),
    Expr(Spanned<Expr>),
    /// `x = expr;` -- solo variables simples (no `obj.field = ...` ni
    /// `arr[i] = ...` todavía). El checker exige que `x` haya sido
    /// declarada con `mut` (GRAMMAR.md §2.3).
    Assign {
        name: String,
        value: Spanned<Expr>,
    },
    /// `while cond { body }` (GRAMMAR.md §3.15) -- a propósito NUNCA un
    /// `Expr`: no hay `break <valor>` en v0 (ni tipo `Never`/bottom para lo
    /// que valdría un loop que nunca hace break), así que un loop no tiene
    /// ningún valor que producir. `cond` se re-evalúa en cada iteración;
    /// `body` corre por efecto solamente -- se chequea contra `Type::Void`,
    /// igual que un `if`/`match` en posición de sentencia (checker.rs::
    /// check_stmt). Un `return` alcanzable desde `body` se RECHAZA
    /// explícitamente en v0 (reusa `block_has_return`) en vez de heredar en
    /// silencio el bug ya existente de `if`/`match` como sentencia (un
    /// `return` ahí es un no-op silencioso en runtime, no solo mal tipado --
    /// ver la nota extensa en checker.rs::check_stmt).
    While {
        cond: Spanned<Expr>,
        body: Block,
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
        base: Box<Spanned<Expr>>,
        field: String,
    },
    Call {
        callee: Box<Spanned<Expr>>,
        args: Vec<Spanned<Expr>>,
    },
    /// `[e1, e2, ...]` -- vacío (`[]`) solo es válido en modo chequeo,
    /// ver checker.rs (no se puede sintetizar un elemento de la nada).
    ArrayLit(Vec<Spanned<Expr>>),
    /// `base[index]` -- postfix, ver GRAMMAR.md §2.3.
    Index {
        base: Box<Spanned<Expr>>,
        index: Box<Spanned<Expr>>,
    },
    /// `(e1, e2, ...)` -- distinto de `Paren` (agrupación) por la misma
    /// regla de la coma obligatoria que ya usa el nivel de tipos (§2.2).
    TupleLit(Vec<Spanned<Expr>>),
    /// `base.0`, `base.1`, ... -- acceso posicional. Un solo nivel: `t.0.1`
    /// NO encadena (ver nota del lexer en GRAMMAR.md §2.3), es una
    /// limitación conocida, no un error silencioso.
    TupleIndex {
        base: Box<Spanned<Expr>>,
        index: usize,
    },
    /// `Nombre { campos }` o `Enum.Variante { campos }` (GRAMMAR.md §2.3 struct_or_variant_lit).
    StructLit {
        name: String,
        variant: Option<String>,
        fields: Vec<(String, Spanned<Expr>)>,
    },
    Match {
        scrutinee: Box<Spanned<Expr>>,
        arms: Vec<MatchArm>,
    },
    If {
        cond: Box<Spanned<Expr>>,
        then_block: Block,
        else_block: Block,
    },
    Binary {
        op: BinaryOp,
        left: Box<Spanned<Expr>>,
        right: Box<Spanned<Expr>>,
    },
    Unary {
        op: UnaryOp,
        operand: Box<Spanned<Expr>>,
    },
    Paren(Box<Spanned<Expr>>),
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

#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pattern: Pattern,
    /// `pattern if guard => body` (GRAMMAR.md §3.3). Un arm con guard NUNCA
    /// descarta exhaustividad por sí solo -- la condición podría ser falsa
    /// en runtime, así que el checker lo trata como si no cubriera nada.
    pub guard: Option<Spanned<Expr>>,
    pub body: MatchArmBody,
    pub span: Span,
}

impl PartialEq for MatchArm {
    fn eq(&self, other: &Self) -> bool {
        self.pattern == other.pattern && self.guard == other.guard && self.body == other.body
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum MatchArmBody {
    Expr(Spanned<Expr>),
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

/// Reconoce el ÚNICO shape de cuerpo de un `stream` que dispara push real
/// v0 (GRAMMAR.md §3.16): `while true { db.<coleccion>.subscribe() }`,
/// nada más -- ni una sentencia antes, ni un tail después (`while` nunca
/// produce uno, ver `Stmt::While` arriba, así que el cuerpo entero tiene
/// que ser esa única sentencia). Cualquier otra forma (otro método,
/// argumentos, sentencias de más, una condición que no sea el literal
/// `true`) devuelve `None` -- ese `stream` sigue el camino de siempre
/// (`List<T>` ya calculada).
///
/// Vive acá, no en checker.rs ni en runtime, para que ambos lo llamen sin
/// que ninguno dependa del otro: `checker.rs::check_rpc` lo usa para
/// decidir si valida este shape especial en vez del cuerpo normal, y
/// `runtime::live_subscribe_collection` (sibling de `is_stream_member`)
/// lo usa para que `server.rs` decida el routing ANTES de invocar
/// `invoke_rpc_with_sessions` -- ese cuerpo nunca llega a `eval_block`.
pub fn recognize_live_subscribe(body: &Block) -> Option<&str> {
    let [stmt] = body.stmts.as_slice() else { return None };
    if body.tail.is_some() {
        return None;
    }
    let Stmt::While { cond, body: loop_body } = &stmt.node else { return None };
    if !matches!(cond.node, Expr::Bool(true)) {
        return None;
    }
    if !loop_body.stmts.is_empty() {
        return None;
    }
    let Expr::Call { callee, args } = &loop_body.tail.as_ref()?.node else { return None };
    if !args.is_empty() {
        return None;
    }
    let Expr::FieldAccess { base, field } = &callee.node else { return None };
    if field != "subscribe" {
        return None;
    }
    let Expr::FieldAccess { base: db_ident, field: collection } = &base.node else { return None };
    matches!(&db_ident.node, Expr::Ident(n) if n == "db").then(|| collection.as_str())
}
