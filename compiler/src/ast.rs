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
    /// `Enum.Variante { campo: patrón, ... }` — la variante unitaria sin
    /// llaves (`Enum.Variante`) se representa con `fields: None`.
    Variant {
        enum_name: String,
        variant_name: String,
        fields: Option<Vec<FieldPattern>>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldPattern {
    pub name: String,
    /// El shorthand `x` (sin `: patrón`) se expande en el parser a
    /// `Pattern::Bind(x)`, así el resto del compilador no necesita conocer
    /// la abreviatura — ya llega desugared.
    pub pattern: Pattern,
}
