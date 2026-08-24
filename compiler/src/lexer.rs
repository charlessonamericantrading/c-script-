use crate::token::{keyword_from_str, Span, Token, TokenKind};

#[derive(Debug)]
pub struct LexError {
    pub message: String,
    pub span: Span,
}

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "error léxico en línea {}, columna {}: {}", self.span.line, self.span.col, self.message)
    }
}

pub fn tokenize(source: &str) -> Result<Vec<Token>, LexError> {
    Lexer::new(source).run()
}

struct Lexer {
    chars: Vec<char>,
    pos: usize,
    line: usize,
    col: usize,
}

impl Lexer {
    fn new(source: &str) -> Self {
        Lexer {
            chars: source.chars().collect(),
            pos: 0,
            line: 1,
            col: 1,
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
                self.col = 1;
            } else {
                self.col += 1;
            }
        }
        c
    }

    /// Span EXPLÍCITO -- sin "posición actual" implícita, a propósito. Bug
    /// real encontrado por review: el helper viejo asumía "el error está en
    /// self.pos", pero varios call sites ya habían consumido el/los
    /// carácter(es) problemáticos antes de construir el error (ej.
    /// `lex_punct` ya hace `self.advance()` antes de saber si el char anda
    /// mal). Obligar a cada call site a pasar su propio `start`/`line`/`col`
    /// (capturados ANTES de consumir) elimina la clase entera de bug.
    fn error_span(&self, start: usize, end: usize, line: usize, col: usize, message: impl Into<String>) -> LexError {
        LexError { message: message.into(), span: Span::new(start, end, line, col) }
    }

    fn run(mut self) -> Result<Vec<Token>, LexError> {
        let mut tokens = Vec::new();
        loop {
            self.skip_trivia()?;
            let start = self.pos;
            let line = self.line;
            let col = self.col;
            let Some(c) = self.peek() else {
                tokens.push(Token::new(TokenKind::Eof, Span::new(start, start, line, col)));
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
            tokens.push(Token::new(kind, Span::new(start, end, line, col)));
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
                    // Bug real (encontrado por review): antes, el span de
                    // "sin cerrar" usaba self.pos en EOF (potencialmente
                    // miles de chars después) con start_line de la apertura
                    // -- line y start/end podían quedar de líneas distintas.
                    // Acá el span queda anclado en la apertura, de un solo
                    // token de largo -- un renderer puede asumir con
                    // seguridad que cualquier LexError es de una sola línea.
                    let start = self.pos;
                    let start_line = self.line;
                    let start_col = self.col;
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
                                return Err(self.error_span(
                                    start,
                                    start + 2,
                                    start_line,
                                    start_col,
                                    "comentario de bloque sin cerrar",
                                ));
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
        let start_line = self.line;
        let start_col = self.col;
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            self.advance();
        }
        if self.peek() == Some('.') && matches!(self.peek_at(1), Some(c) if c.is_ascii_digit()) {
            self.advance(); // '.'
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.advance();
            }
            let text: String = self.chars[start..self.pos].iter().collect();
            let value: f64 = text.parse().map_err(|_| {
                // Bug real (review): antes este error usaba self.pos DESPUÉS
                // de consumir el literal completo -- el span de 1 char caía
                // justo después del número, no sobre él. Acá cubre el
                // literal entero (start..self.pos).
                self.error_span(start, self.pos, start_line, start_col, format!("float inválido: {text}"))
            })?;
            return Ok(TokenKind::Float(value));
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        let value: i64 = text.parse().map_err(|_| {
            self.error_span(start, self.pos, start_line, start_col, format!("entero inválido: {text}"))
        })?;
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
        // Posición de la comilla de APERTURA -- usada para "sin cerrar"
        // (Bug real de review: antes ese caso mezclaba esta línea con
        // self.pos en EOF, que puede estar a miles de chars/líneas de
        // distancia si el string sin cerrar cruza varias líneas, algo que
        // sí puede pasar: el brazo `Some(c) => value.push(c)` de abajo
        // acepta un '\n' crudo sin rechazarlo).
        let start = self.pos;
        let start_line = self.line;
        let start_col = self.col;
        self.advance(); // comilla inicial
        let mut value = String::new();
        loop {
            match self.advance() {
                Some('"') => break,
                Some('\\') => {
                    // Posición del '\' que acaba de consumirse -- usada para
                    // TODOS los errores de escape de acá abajo (Bug real de
                    // review: antes usaban self.pos ya avanzado más allá).
                    let esc_start = self.pos - 1;
                    let esc_line = self.line;
                    let esc_col = self.col - 1;
                    match self.advance() {
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
                                        return Err(self.error_span(
                                            esc_start,
                                            self.pos,
                                            esc_line,
                                            esc_col,
                                            "escape \\u incompleto, se esperaban 4 dígitos hex",
                                        ))
                                    }
                                }
                            }
                            let code = u32::from_str_radix(&hex, 16).map_err(|_| {
                                self.error_span(esc_start, self.pos, esc_line, esc_col, "escape \\u inválido")
                            })?;
                            let ch = char::from_u32(code).ok_or_else(|| {
                                self.error_span(esc_start, self.pos, esc_line, esc_col, "code point \\u inválido")
                            })?;
                            value.push(ch);
                        }
                        Some(other) => {
                            return Err(self.error_span(
                                esc_start,
                                self.pos,
                                esc_line,
                                esc_col,
                                format!("secuencia de escape desconocida: \\{other}"),
                            ))
                        }
                        None => {
                            return Err(self.error_span(start, start + 1, start_line, start_col, "string sin cerrar"))
                        }
                    }
                }
                Some(c) => value.push(c),
                None => return Err(self.error_span(start, start + 1, start_line, start_col, "string sin cerrar")),
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
        // Bug real (review): `self.advance()` de acá abajo YA consume `c`
        // antes de saber si termina siendo un error -- capturar la posición
        // ACÁ (antes de consumir) es lo que hace que los dos `error_span`
        // de este método apunten al carácter real, no a uno después.
        let start = self.pos;
        let start_line = self.line;
        let start_col = self.col;
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
            '?' => {
                if self.peek() == Some('?') {
                    self.advance();
                    TokenKind::QuestionQuestion
                } else {
                    TokenKind::Question
                }
            }
            '@' => TokenKind::At,
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
                    return Err(self.error_span(
                        start,
                        self.pos,
                        start_line,
                        start_col,
                        "'&' suelto no es válido (¿quisiste '&&'? no hay bitwise-and en v0)",
                    ));
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
            other => {
                return Err(self.error_span(
                    start,
                    self.pos,
                    start_line,
                    start_col,
                    format!("carácter inesperado: '{other}'"),
                ))
            }
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
    fn at_sign_lexes_as_its_own_token() {
        assert_eq!(
            kinds("@requires(Role.Admin)"),
            vec![
                TokenKind::At,
                TokenKind::Ident("requires".into()),
                TokenKind::LParen,
                TokenKind::Ident("Role".into()),
                TokenKind::Dot,
                TokenKind::Ident("Admin".into()),
                TokenKind::RParen,
                TokenKind::Eof,
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

    // ---- columna + los 2 bugs de span encontrados por el review ----

    #[test]
    fn column_tracks_position_within_a_line_and_resets_on_newline() {
        let tokens = tokenize("ab cd\nef").unwrap();
        // "ab"=col1, "cd"=col4, "ef" (después del \n)=col1
        assert_eq!(tokens[0].span.col, 1); // ab
        assert_eq!(tokens[1].span.col, 4); // cd
        assert_eq!(tokens[2].span.col, 1); // ef, línea 2
        assert_eq!(tokens[2].span.line, 2);
    }

    #[test]
    fn unexpected_character_error_points_at_the_character_itself() {
        // Bug real: antes, self.error() en lex_punct usaba self.pos DESPUÉS
        // de que `self.advance()` ya había consumido '#' -- el span caía un
        // char tarde. "ab #" -> '#' está en col 4 (1-based).
        let err = tokenize("ab #").unwrap_err();
        assert_eq!(err.span.col, 4, "{err:?}");
        assert_eq!(err.span.start, 3, "{err:?}");
        assert_eq!(err.span.end, 4, "{err:?}");
    }

    #[test]
    fn lone_ampersand_error_points_at_the_ampersand() {
        let err = tokenize("x & y").unwrap_err();
        assert_eq!(err.span.col, 3, "{err:?}"); // '&' en col 3
    }

    #[test]
    fn invalid_number_literal_error_spans_the_whole_literal_not_one_char_after() {
        // Bug real: antes, el span caía DESPUÉS del literal completo (en el
        // espacio/EOF que sigue), no sobre el número mismo. Un i64 fuera de
        // rango: se necesitan más de 19 dígitos para desbordar i64::MAX.
        let src = "99999999999999999999";
        let err = tokenize(src).unwrap_err();
        assert_eq!(err.span.start, 0, "{err:?}");
        assert_eq!(err.span.end, src.len(), "{err:?}"); // cubre el literal ENTERO
        assert_eq!(err.span.col, 1, "{err:?}");
    }

    #[test]
    fn unterminated_string_spans_only_the_opening_quote_not_eof() {
        // Bug real: antes, este caso usaba `line` de la apertura pero
        // start/end de EOF -- si el string sin cerrar cruza líneas, el span
        // terminaba con `line` y `start`/`end` de líneas totalmente
        // distintas. Acá el span queda anclado en la comilla de apertura.
        let src = "x\n\"sin cerrar\ny mas lineas\ny mas";
        let err = tokenize(src).unwrap_err();
        assert_eq!(err.span.line, 2, "{err:?}"); // la comilla abre en la línea 2
        assert_eq!(err.span.col, 1, "{err:?}");
        assert_eq!(err.span.end - err.span.start, 1, "span de 1 solo char (la comilla)"); // NO llega a EOF
    }

    #[test]
    fn unterminated_block_comment_spans_only_the_opening_delimiter() {
        let src = "x /* comentario\nque nunca\ncierra";
        let err = tokenize(src).unwrap_err();
        assert_eq!(err.span.line, 1, "{err:?}");
        assert_eq!(err.span.col, 3, "{err:?}"); // "x " ocupa 2 cols, "/*" arranca en la 3
        assert_eq!(err.span.end - err.span.start, 2, "span de 2 chars (/*), no hasta EOF");
    }

    #[test]
    fn incomplete_unicode_escape_error_points_at_the_backslash() {
        let src = r#""\u12""#; // le faltan 2 dígitos hex
        let err = tokenize(src).unwrap_err();
        assert_eq!(err.span.col, 2, "{err:?}"); // el '\' está en col 2 (después de la comilla en col 1)
    }
}
