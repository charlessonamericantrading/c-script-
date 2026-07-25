#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub line: usize,
}

impl Span {
    pub fn new(start: usize, end: usize, line: usize) -> Self {
        Span { start, end, line }
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
        "true" => TokenKind::True,
        "false" => TokenKind::False,
        "null" => TokenKind::Null,
        _ => return None,
    })
}
