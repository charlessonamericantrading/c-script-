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
