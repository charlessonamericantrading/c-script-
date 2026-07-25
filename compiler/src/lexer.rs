use crate::token::{keyword_from_str, Span, Token, TokenKind};

#[derive(Debug)]
pub struct LexError {
    pub message: String,
    pub span: Span,
}

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "error léxico en línea {}: {}", self.span.line, self.message)
    }
}

pub fn tokenize(source: &str) -> Result<Vec<Token>, LexError> {
    Lexer::new(source).run()
}

struct Lexer {
    chars: Vec<char>,
    pos: usize,
    line: usize,
}

impl Lexer {
    fn new(source: &str) -> Self {
        Lexer {
            chars: source.chars().collect(),
            pos: 0,
            line: 1,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let c = self.peek();
        if let Some(ch) = c {
            self.pos += 1;
            if ch == '\n' {
                self.line += 1;
            }
        }
        c
    }

    fn error(&self, message: impl Into<String>) -> LexError {
        LexError {
            message: message.into(),
            span: Span::new(self.pos, self.pos + 1, self.line),
        }
    }

    fn run(mut self) -> Result<Vec<Token>, LexError> {
        let mut tokens = Vec::new();
        loop {
            self.skip_trivia()?;
            let start = self.pos;
            let line = self.line;
            let Some(c) = self.peek() else {
                tokens.push(Token::new(TokenKind::Eof, Span::new(start, start, line)));
                break;
            };

            let kind = if c.is_ascii_digit() {
                self.lex_number()?
            } else if is_ident_start(c) {
                self.lex_ident_or_keyword()
            } else if c == '"' {
                self.lex_string()?
            } else {
                self.lex_punct()?
            };

            let end = self.pos;
            tokens.push(Token::new(kind, Span::new(start, end, line)));
        }
        Ok(tokens)
    }

    /// Saltea espacios en blanco, comentarios de línea (`//`) y de bloque (`/* */`).
    fn skip_trivia(&mut self) -> Result<(), LexError> {
        loop {
            match self.peek() {
                Some(c) if c.is_whitespace() => {
                    self.advance();
                }
                Some('/') if self.peek_at(1) == Some('/') => {
                    while let Some(c) = self.peek() {
                        if c == '\n' {
                            break;
                        }
                        self.advance();
                    }
                }
                Some('/') if self.peek_at(1) == Some('*') => {
                    let start_line = self.line;
                    self.advance();
                    self.advance();
                    loop {
                        match (self.peek(), self.peek_at(1)) {
                            (Some('*'), Some('/')) => {
                                self.advance();
                                self.advance();
                                break;
                            }
                            (Some(_), _) => {
                                self.advance();
                            }
                            (None, _) => {
                                return Err(LexError {
                                    message: "comentario de bloque sin cerrar".into(),
                                    span: Span::new(self.pos, self.pos, start_line),
                                });
                            }
                        }
                    }
                }
                _ => break,
            }
        }
        Ok(())
    }

    /// int_lit | float_lit (GRAMMAR.md §1). El '.' solo se consume como parte
    /// del número si el carácter siguiente es un dígito — así "42.foo" no
    /// se traga el punto que en realidad separa dos tokens distintos.
    fn lex_number(&mut self) -> Result<TokenKind, LexError> {
        let start = self.pos;
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            self.advance();
        }
        if self.peek() == Some('.') && matches!(self.peek_at(1), Some(c) if c.is_ascii_digit()) {
            self.advance(); // '.'
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.advance();
            }
            let text: String = self.chars[start..self.pos].iter().collect();
            let value: f64 = text
                .parse()
                .map_err(|_| self.error(format!("float inválido: {text}")))?;
            return Ok(TokenKind::Float(value));
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        let value: i64 = text
            .parse()
            .map_err(|_| self.error(format!("entero inválido: {text}")))?;
        Ok(TokenKind::Int(value))
    }

    fn lex_ident_or_keyword(&mut self) -> TokenKind {
        let start = self.pos;
        while matches!(self.peek(), Some(c) if is_ident_continue(c)) {
            self.advance();
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        keyword_from_str(&text).unwrap_or(TokenKind::Ident(text))
    }

    /// string_lit con escapes \n \t \\ \" \uXXXX (GRAMMAR.md §1).
    fn lex_string(&mut self) -> Result<TokenKind, LexError> {
        let line = self.line;
        self.advance(); // comilla inicial
        let mut value = String::new();
        loop {
            match self.advance() {
                Some('"') => break,
                Some('\\') => match self.advance() {
                    Some('n') => value.push('\n'),
                    Some('t') => value.push('\t'),
                    Some('\\') => value.push('\\'),
                    Some('"') => value.push('"'),
                    Some('u') => {
                        let mut hex = String::new();
                        for _ in 0..4 {
                            match self.advance() {
                                Some(c) if c.is_ascii_hexdigit() => hex.push(c),
                                _ => {
                                    return Err(
                                        self.error("escape \\u incompleto, se esperaban 4 dígitos hex")
                                    )
                                }
                            }
                        }
                        let code = u32::from_str_radix(&hex, 16)
                            .map_err(|_| self.error("escape \\u inválido"))?;
                        let ch = char::from_u32(code)
                            .ok_or_else(|| self.error("code point \\u inválido"))?;
                        value.push(ch);
                    }
                    Some(other) => {
                        return Err(self.error(format!("secuencia de escape desconocida: \\{other}")))
                    }
                    None => {
                        return Err(LexError {
                            message: "string sin cerrar".into(),
                            span: Span::new(self.pos, self.pos, line),
                        })
                    }
                },
                Some(c) => value.push(c),
                None => {
                    return Err(LexError {
                        message: "string sin cerrar".into(),
                        span: Span::new(self.pos, self.pos, line),
                    })
                }
            }
        }
        Ok(TokenKind::Str(value))
    }

    /// Puntuación y operadores de GRAMMAR.md §2/§3.7. `-` ahora es un
    /// operador válido standalone (resta/unario) además de parte de `->` —
    /// se distingue con un carácter de lookahead, igual que `=`/`=>` y
    /// `<`/`<=`. `|` (unión de tipos) y `||` (or lógico) son tokens
    /// distintos: no hay bitwise-or en v0, así que un `|` seguido de otro
    /// `|` siempre es `PipePipe`.
    fn lex_punct(&mut self) -> Result<TokenKind, LexError> {
        let c = self.advance().unwrap();
        Ok(match c {
            '{' => TokenKind::LBrace,
            '}' => TokenKind::RBrace,
            '(' => TokenKind::LParen,
            ')' => TokenKind::RParen,
            '[' => TokenKind::LBracket,
            ']' => TokenKind::RBracket,
            ',' => TokenKind::Comma,
            ';' => TokenKind::Semi,
            ':' => TokenKind::Colon,
            '?' => TokenKind::Question,
            '.' => TokenKind::Dot,
            '+' => TokenKind::Plus,
            '*' => TokenKind::Star,
            '/' => TokenKind::Slash,
            '%' => TokenKind::Percent,
            '<' => self.one_or_two('=', TokenKind::Lt, TokenKind::LtEq),
            '>' => self.one_or_two('=', TokenKind::Gt, TokenKind::GtEq),
            '=' => {
                if self.peek() == Some('=') {
                    self.advance();
                    TokenKind::EqEq
                } else if self.peek() == Some('>') {
                    self.advance();
                    TokenKind::FatArrow
                } else {
                    TokenKind::Equals
                }
            }
            '!' => self.one_or_two('=', TokenKind::Bang, TokenKind::NotEq),
            '-' => {
                if self.peek() == Some('>') {
                    self.advance();
                    TokenKind::Arrow
                } else {
                    TokenKind::Minus
                }
            }
            '&' => {
                if self.peek() == Some('&') {
                    self.advance();
                    TokenKind::AmpAmp
                } else {
                    return Err(self.error("'&' suelto no es válido (¿quisiste '&&'? no hay bitwise-and en v0)"));
                }
            }
            '|' => {
                if self.peek() == Some('|') {
                    self.advance();
                    TokenKind::PipePipe
                } else {
                    TokenKind::Pipe
                }
            }
            other => return Err(self.error(format!("carácter inesperado: '{other}'"))),
        })
    }

    /// Consume `second` si sigue inmediatamente, devolviendo `two`; si no,
    /// devuelve `one` sin consumir nada más. Evita repetir el mismo if/else
    /// para cada par `X`/`X=`.
    fn one_or_two(&mut self, second: char, one: TokenKind, two: TokenKind) -> TokenKind {
        if self.peek() == Some(second) {
            self.advance();
            two
        } else {
            one
        }
    }
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_ident_continue(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<TokenKind> {
        tokenize(source)
            .unwrap_or_else(|e| panic!("{e}"))
            .into_iter()
            .map(|t| t.kind)
            .collect()
    }

    #[test]
    fn keywords_and_identifiers() {
        assert_eq!(
            kinds("type User rpc"),
            vec![
                TokenKind::Type,
                TokenKind::Ident("User".into()),
                TokenKind::Rpc,
                TokenKind::Eof
            ]
        );
    }

    #[test]
    // 3.14 es un literal de prueba del lexer, no una aproximación de PI --
    // clippy::approx_constant no puede distinguirlo.
    #[allow(clippy::approx_constant)]
    fn int_and_float_literals() {
        assert_eq!(kinds("42"), vec![TokenKind::Int(42), TokenKind::Eof]);
        assert_eq!(kinds("3.14"), vec![TokenKind::Float(3.14), TokenKind::Eof]);
        // '.' no seguido de dígito no se consume como parte del número
        assert_eq!(
            kinds("42.foo"),
            vec![
                TokenKind::Int(42),
                TokenKind::Dot,
                TokenKind::Ident("foo".into()),
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn string_literal_with_escapes() {
        assert_eq!(
            kinds(r#""hola\nmundo""#),
            vec![TokenKind::Str("hola\nmundo".into()), TokenKind::Eof]
        );
        assert_eq!(
            kinds(r#""é""#),
            vec![TokenKind::Str("é".into()), TokenKind::Eof]
        );
    }

    #[test]
    fn line_and_block_comments_are_skipped() {
        assert_eq!(
            kinds("type // esto es un comentario\nEnum"),
            vec![
                TokenKind::Type,
                TokenKind::Ident("Enum".into()),
                TokenKind::Eof
            ]
        );
        assert_eq!(
            kinds("type /* bloque \n multilinea */ Enum"),
            vec![
                TokenKind::Type,
                TokenKind::Ident("Enum".into()),
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn arrow_and_fat_arrow_vs_bare_equals() {
        assert_eq!(kinds("->"), vec![TokenKind::Arrow, TokenKind::Eof]);
        assert_eq!(kinds("=>"), vec![TokenKind::FatArrow, TokenKind::Eof]);
        assert_eq!(kinds("="), vec![TokenKind::Equals, TokenKind::Eof]);
    }

    #[test]
    fn minus_is_now_a_valid_standalone_operator() {
        // Antes de GRAMMAR.md §3.7, '-' suelto era error léxico. Ahora es
        // resta/unario -- el lookahead sigue distinguiéndolo de '->'.
        assert_eq!(
            kinds("3 - 4"),
            vec![TokenKind::Int(3), TokenKind::Minus, TokenKind::Int(4), TokenKind::Eof]
        );
        assert_eq!(kinds("->"), vec![TokenKind::Arrow, TokenKind::Eof]);
    }

    #[test]
    fn two_char_operators_vs_their_one_char_prefix() {
        assert_eq!(kinds("=="), vec![TokenKind::EqEq, TokenKind::Eof]);
        assert_eq!(kinds("!="), vec![TokenKind::NotEq, TokenKind::Eof]);
        assert_eq!(kinds("!"), vec![TokenKind::Bang, TokenKind::Eof]);
        assert_eq!(kinds("<="), vec![TokenKind::LtEq, TokenKind::Eof]);
        assert_eq!(kinds("<"), vec![TokenKind::Lt, TokenKind::Eof]);
        assert_eq!(kinds(">="), vec![TokenKind::GtEq, TokenKind::Eof]);
        assert_eq!(kinds(">"), vec![TokenKind::Gt, TokenKind::Eof]);
        assert_eq!(kinds("&&"), vec![TokenKind::AmpAmp, TokenKind::Eof]);
        assert_eq!(kinds("||"), vec![TokenKind::PipePipe, TokenKind::Eof]);
        // '|' solo sigue siendo el de uniones de tipo (A | B), no bitwise-or
        assert_eq!(kinds("|"), vec![TokenKind::Pipe, TokenKind::Eof]);
    }

    #[test]
    fn bare_ampersand_is_a_lex_error() {
        // No hay bitwise-and en v0 -- solo '&&' es válido.
        assert!(tokenize("&").is_err());
    }

    #[test]
    fn arithmetic_expression_tokenizes() {
        assert_eq!(
            kinds("a + b * c - d / e % f"),
            vec![
                TokenKind::Ident("a".into()),
                TokenKind::Plus,
                TokenKind::Ident("b".into()),
                TokenKind::Star,
                TokenKind::Ident("c".into()),
                TokenKind::Minus,
                TokenKind::Ident("d".into()),
                TokenKind::Slash,
                TokenKind::Ident("e".into()),
                TokenKind::Percent,
                TokenKind::Ident("f".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn postfix_nullable_and_list_tokens_lex_identically_regardless_of_order() {
        // La diferencia entre T[]? y T?[] la resuelve el PARSER (GRAMMAR.md §2.2),
        // no el lexer: acá solo verificamos que ambos órdenes de postfix tokenizan.
        assert_eq!(
            kinds("T[]?"),
            vec![
                TokenKind::Ident("T".into()),
                TokenKind::LBracket,
                TokenKind::RBracket,
                TokenKind::Question,
                TokenKind::Eof
            ]
        );
        assert_eq!(
            kinds("T?[]"),
            vec![
                TokenKind::Ident("T".into()),
                TokenKind::Question,
                TokenKind::LBracket,
                TokenKind::RBracket,
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn full_users_service_snippet() {
        let source = r#"
type User = {
  id: Int,
  name: String,
  bio?: String,
}

enum Role { Admin, Member, Guest }

service Users {
  rpc getById(id: Int) -> User? {
    db.users.find(id)
  }
}
"#;
        let kinds = kinds(source);
        assert_eq!(kinds.first(), Some(&TokenKind::Type));
        assert_eq!(kinds.last(), Some(&TokenKind::Eof));
        assert!(kinds.contains(&TokenKind::Service));
        assert!(kinds.contains(&TokenKind::Rpc));
        assert!(kinds.contains(&TokenKind::Arrow));
        assert!(kinds.contains(&TokenKind::Question));
        assert!(kinds.contains(&TokenKind::Ident("db".into())));
    }
}
