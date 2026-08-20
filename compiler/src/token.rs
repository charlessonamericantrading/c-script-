/// Posición de un token/error en el código fuente. `start`/`end` son índices
/// dentro del `Vec<char>` que arma el lexer -- NO son offsets de byte UTF-8
/// ni UTF-16 code units (eso haría falta recién en el borde de un protocolo
/// LSP real, que todavía no existe). `line`/`col` son 1-based (misma
/// convención para los dos) y describen SIEMPRE el INICIO (`start`) del
/// span -- no hay `end_line`/`end_col`. Un span que cubra varios tokens (ej.
/// un `Expr` completo, cuando el AST tenga spans) se arma como
/// `Span::new(primero.span.start, ultimo.span.end, primero.span.line, primero.span.col)`,
/// gratis con esta forma.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub line: usize,
    pub col: usize,
}

impl Span {
    pub fn new(start: usize, end: usize, line: usize, col: usize) -> Self {
        Span { start, end, line, col }
    }
}

impl TokenKind {
    /// Texto fuente de una palabra clave, o `None` si el token no lo es.
    ///
    /// Existe para las posiciones donde la gramatica pide un NOMBRE y una
    /// palabra clave no puede significar otra cosa -- declaracion de campo,
    /// campo de un literal de struct, campo de un patron y acceso `.campo`.
    /// Sin esto, un modelo con una columna llamada `service`, `type` o `from`
    /// -- nombres normales en un esquema real -- no se puede describir en
    /// Link, aunque en esas posiciones no haya ninguna ambiguedad.
    pub fn keyword_text(&self) -> Option<&'static str> {
        match self {
            TokenKind::Type => Some("type"),
            TokenKind::Enum => Some("enum"),
            TokenKind::Service => Some("service"),
            TokenKind::Rpc => Some("rpc"),
            TokenKind::Stream => Some("stream"),
            TokenKind::Match => Some("match"),
            TokenKind::Import => Some("import"),
            TokenKind::From => Some("from"),
            TokenKind::Pub => Some("pub"),
            TokenKind::Const => Some("const"),
            TokenKind::Fn => Some("fn"),
            TokenKind::Let => Some("let"),
            TokenKind::Mut => Some("mut"),
            TokenKind::Return => Some("return"),
            TokenKind::If => Some("if"),
            TokenKind::Else => Some("else"),
            TokenKind::While => Some("while"),
            TokenKind::Test => Some("test"),
            TokenKind::True => Some("true"),
            TokenKind::False => Some("false"),
            TokenKind::Null => Some("null"),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // Literales
    Int(i64),
    Float(f64),
    Str(String),
    Ident(String),

    // Palabras clave (GRAMMAR.md §1)
    Type,
    Enum,
    Service,
    Rpc,
    Stream,
    Match,
    Import,
    From,
    Pub,
    Const,
    Fn,
    Let,
    Mut,
    Return,
    If,
    Else,
    While,
    Test,
    True,
    False,
    Null,

    // Puntuación (GRAMMAR.md §2)
    LBrace,   // {
    RBrace,   // }
    LParen,   // (
    RParen,   // )
    LBracket, // [
    RBracket, // ]
    Comma,    // ,
    Semi,     // ;
    Colon,    // :
    Question, // ?
    At,       // @   (anotaciones sobre rpc/stream: @authenticated, @requires(...))
    Pipe,     // |   (union de tipos: A | B)
    Lt,       // <
    Gt,       // >
    Equals,   // =
    Arrow,    // ->
    FatArrow, // =>
    Dot,      // .

    // Operadores (GRAMMAR.md §2.3/§3.7)
    Plus,     // +
    Minus,    // -   (también unario)
    Star,     // *
    Slash,    // /
    Percent,  // %
    EqEq,     // ==
    NotEq,    // !=
    LtEq,     // <=
    GtEq,     // >=
    AmpAmp,   // &&
    PipePipe, // ||  (distinto de Pipe: '|' solo no es válido en v0)
    Bang,     // !

    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    pub fn new(kind: TokenKind, span: Span) -> Self {
        Token { kind, span }
    }
}

/// Reconoce las palabras clave de GRAMMAR.md §1. Devuelve `None` si `s` no es
/// una palabra reservada (y por lo tanto es un identificador normal).
pub fn keyword_from_str(s: &str) -> Option<TokenKind> {
    Some(match s {
        "type" => TokenKind::Type,
        "enum" => TokenKind::Enum,
        "service" => TokenKind::Service,
        "rpc" => TokenKind::Rpc,
        "stream" => TokenKind::Stream,
        "match" => TokenKind::Match,
        "import" => TokenKind::Import,
        "from" => TokenKind::From,
        "pub" => TokenKind::Pub,
        "const" => TokenKind::Const,
        "fn" => TokenKind::Fn,
        "let" => TokenKind::Let,
        "mut" => TokenKind::Mut,
        "return" => TokenKind::Return,
        "if" => TokenKind::If,
        "else" => TokenKind::Else,
        "while" => TokenKind::While,
        "test" => TokenKind::Test,
        "true" => TokenKind::True,
        "false" => TokenKind::False,
        "null" => TokenKind::Null,
        _ => return None,
    })
}
