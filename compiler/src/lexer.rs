use crate::token::{keyword_from_str, HtmlPart, Span, Token, TokenKind};

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
    let sanitized = check_version_pragma(source)?;
    Lexer::new(&sanitized).run()
}

/// GRAMMAR.md §3.263: `#linkc >= "X.Y.Z"` opcional, solo entre las líneas en
/// blanco/`//` del principio del archivo (antes del primer token real) --
/// mismo lugar donde ya viven los headers de licencia/descripción de la
/// mayoría de los `.link` reales. Si aparece y esta versión no la cumple,
/// falla ACÁ, antes de tokenizar nada más, con un mensaje de una línea --
/// nunca dejando que la incompatibilidad real se manifieda como un error de
/// runtime irrelevante más adelante (el incidente real que motiva esto:
/// GRAMMAR.md §3.263 mismo). Si la pragma está presente y se cumple, se
/// devuelve el source con esa línea reemplazada por espacios (mismo largo
/// exacto, así que línea/columna de TODO lo demás queda idéntica) -- el
/// lexer normal de acá abajo no sabe nada de pragmas, `#` en cualquier otro
/// lugar sigue siendo su error de "carácter inesperado" de siempre.
fn check_version_pragma(source: &str) -> Result<String, LexError> {
    let mut line_no = 0usize;
    let mut offset = 0usize;
    for raw_line in source.split_inclusive('\n') {
        line_no += 1;
        let line = raw_line.trim_end_matches('\n').trim_end_matches('\r');
        let trimmed = line.trim_start();
        let leading_ws = line.len() - trimmed.len();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            offset += raw_line.len();
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("#linkc") {
            let col = leading_ws + 1;
            let span = Span::new(offset + leading_ws, offset + line.len(), line_no, col);
            let required = parse_version_pragma(rest).ok_or_else(|| LexError {
                message: format!(
                    "pragma de versión mal formado -- se esperaba `#linkc >= \"X.Y.Z\"`, se encontró `{trimmed}`"
                ),
                span,
            })?;
            let current = parse_semver(crate::VERSION).unwrap_or((0, 0, 0));
            if current < required {
                return Err(LexError {
                    message: format!(
                        "este .link pide linkc >= {}.{}.{}, esta instalación es {} -- actualizá el binario o ajustá el pragma",
                        required.0, required.1, required.2, crate::VERSION
                    ),
                    span,
                });
            }
            let mut sanitized = String::with_capacity(source.len());
            sanitized.push_str(&source[..offset]);
            for ch in line.chars() {
                sanitized.push(if ch == '\t' { '\t' } else { ' ' });
            }
            sanitized.push_str(&source[offset + line.len()..]);
            return Ok(sanitized);
        }
        // Primera línea real (no blanco, no `//`, no `#linkc`): no hay
        // pragma -- el resto del archivo lo procesa el lexer normal, `#`
        // sueltos ahí abajo siguen siendo el error de "carácter inesperado"
        // de siempre.
        return Ok(source.to_string());
    }
    Ok(source.to_string())
}

/// `>= "X.Y.Z"` -> `(X, Y, Z)`. Solo `>=` -- es el único operador que el
/// caso real pide (GRAMMAR.md §3.263); `<`/`==`/`~`/`^` quedan fuera a
/// propósito hasta que un caso real los necesite.
fn parse_version_pragma(rest: &str) -> Option<(u32, u32, u32)> {
    let rest = rest.trim_start();
    let rest = rest.strip_prefix(">=")?;
    let rest = rest.trim();
    let rest = rest.strip_prefix('"')?.strip_suffix('"')?;
    parse_semver(rest)
}

fn parse_semver(s: &str) -> Option<(u32, u32, u32)> {
    let mut parts = s.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

struct Lexer {
    chars: Vec<char>,
    pos: usize,
    line: usize,
    col: usize,
    /// Líneas `///` acumuladas desde el último token real, todavía sin
    /// asignar a ninguno (GRAMMAR.md §3.72). Se vuelca sobre el PRÓXIMO
    /// token real que se produzca (`run`) y se resetea ahí mismo -- así que
    /// un `///` en cualquier posición donde el parser no lo espere (media
    /// expresión, dentro de un `if`, etc.) simplemente queda pegado a un
    /// token que nadie lee ese campo, exactamente inocuo como un `//`
    /// normal hoy: CERO nuevos errores de sintaxis en programas existentes.
    pending_doc: Option<String>,
}

impl Lexer {
    fn new(source: &str) -> Self {
        Lexer {
            chars: source.chars().collect(),
            pos: 0,
            line: 1,
            col: 1,
            pending_doc: None,
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
            let token = self.next_token()?;
            let is_eof = matches!(token.kind, TokenKind::Eof);
            tokens.push(token);
            if is_eof {
                break;
            }
        }
        Ok(tokens)
    }

    /// Produce UN token -- extraído del loop de `run()` (antes en línea ahí
    /// mismo) para poder reusarlo al sub-tokenizar el interior de un
    /// `${...}` de un literal `html` (`lex_html_literal`, GRAMMAR.md
    /// §3.268): mismo lexer, sin ningún camino de tokenización aparte.
    fn next_token(&mut self) -> Result<Token, LexError> {
        self.skip_trivia()?;
        let start = self.pos;
        let line = self.line;
        let col = self.col;
        let Some(c) = self.peek() else {
            return Ok(Token::new(TokenKind::Eof, Span::new(start, start, line, col)));
        };

        let kind = if c.is_ascii_digit() {
            self.lex_number()?
        } else if is_ident_start(c) {
            let ident_kind = self.lex_ident_or_keyword();
            match &ident_kind {
                TokenKind::Ident(name) if name == "html" && self.peek() == Some('`') => self.lex_html_literal()?,
                _ => ident_kind,
            }
        } else if c == '"' {
            self.lex_string()?
        } else {
            self.lex_punct()?
        };

        let end = self.pos;
        let mut token = Token::new(kind, Span::new(start, end, line, col));
        token.leading_doc = self.pending_doc.take();
        Ok(token)
    }

    /// Literal `html\`...\`` (GRAMMAR.md §3.268) -- interpolado, multilínea
    /// (un `\n` crudo adentro se acepta tal cual, igual que cualquier otro
    /// carácter -- a diferencia de `lex_string`, que SOLO admite `\n` vía
    /// escape). Acá solo se separa texto de expresión -- el checker decide
    /// cómo se escapa cada `${...}` según su tipo (GRAMMAR.md §3.268),
    /// nunca el lexer. `self.peek() == Some('\`')` ya lo confirmó el
    /// caller; esta función consume la comilla de apertura.
    fn lex_html_literal(&mut self) -> Result<TokenKind, LexError> {
        let open_start = self.pos;
        let open_line = self.line;
        let open_col = self.col;
        self.advance(); // ` de apertura
        let mut parts = Vec::new();
        let mut text = String::new();
        loop {
            match self.peek() {
                None => return Err(self.error_span(open_start, open_start + 1, open_line, open_col, "literal 'html`...`' sin cerrar")),
                Some('`') => {
                    self.advance();
                    parts.push(HtmlPart::Text(std::mem::take(&mut text)));
                    break;
                }
                Some('\\') => {
                    let esc_start = self.pos;
                    let esc_line = self.line;
                    let esc_col = self.col;
                    self.advance();
                    match self.advance() {
                        Some('`') => text.push('`'),
                        Some('$') => text.push('$'),
                        Some('\\') => text.push('\\'),
                        Some('n') => text.push('\n'),
                        Some('t') => text.push('\t'),
                        Some(other) => {
                            return Err(self.error_span(
                                esc_start,
                                self.pos,
                                esc_line,
                                esc_col,
                                format!("secuencia de escape desconocida en 'html`...`': \\{other}"),
                            ))
                        }
                        None => return Err(self.error_span(open_start, open_start + 1, open_line, open_col, "literal 'html`...`' sin cerrar")),
                    }
                }
                Some('$') if self.peek_at(1) == Some('{') => {
                    parts.push(HtmlPart::Text(std::mem::take(&mut text)));
                    self.advance(); // $
                    self.advance(); // {
                    let expr_start = self.pos;
                    let expr_line = self.line;
                    let expr_col = self.col;
                    let mut expr_tokens = Vec::new();
                    let mut depth = 0i32;
                    loop {
                        let tok = self.next_token()?;
                        match &tok.kind {
                            TokenKind::Eof => {
                                return Err(self.error_span(
                                    expr_start,
                                    self.pos,
                                    expr_line,
                                    expr_col,
                                    "expresión '${...}' de un literal 'html' sin cerrar",
                                ))
                            }
                            TokenKind::LBrace => {
                                depth += 1;
                                expr_tokens.push(tok);
                            }
                            TokenKind::RBrace if depth == 0 => break,
                            TokenKind::RBrace => {
                                depth -= 1;
                                expr_tokens.push(tok);
                            }
                            _ => expr_tokens.push(tok),
                        }
                    }
                    if expr_tokens.is_empty() {
                        return Err(self.error_span(expr_start, self.pos, expr_line, expr_col, "'${}' vacío en un literal 'html' -- falta la expresión"));
                    }
                    parts.push(HtmlPart::Expr(expr_tokens));
                }
                Some(c) => {
                    text.push(c);
                    self.advance();
                }
            }
        }
        Ok(TokenKind::HtmlLit(parts))
    }

    /// Saltea espacios en blanco, comentarios de línea (`//`) y de bloque (`/* */`).
    /// `///` (exactamente 3 slashes, no 4+) es la excepción: además de
    /// saltearse como trivia normal, su texto se acumula en `pending_doc`
    /// (GRAMMAR.md §3.72) -- varias líneas `///` consecutivas (solo
    /// separadas por espacio en blanco, ninguna otra cosa en el medio) se
    /// unen con `\n` en un solo docstring.
    fn skip_trivia(&mut self) -> Result<(), LexError> {
        loop {
            match self.peek() {
                Some(c) if c.is_whitespace() => {
                    self.advance();
                }
                Some('/') if self.peek_at(1) == Some('/') && self.peek_at(2) == Some('/') && self.peek_at(3) != Some('/') => {
                    self.advance();
                    self.advance();
                    self.advance();
                    if self.peek() == Some(' ') {
                        self.advance(); // un solo espacio de indentación tras `///` no es parte del texto
                    }
                    let text_start = self.pos;
                    while let Some(c) = self.peek() {
                        if c == '\n' {
                            break;
                        }
                        self.advance();
                    }
                    let line_text: String = self.chars[text_start..self.pos].iter().collect();
                    match &mut self.pending_doc {
                        Some(acc) => {
                            acc.push('\n');
                            acc.push_str(&line_text);
                        }
                        None => self.pending_doc = Some(line_text),
                    }
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
            // GRAMMAR.md §3.271: `..` (rango de un `for i in a..b { }`) es un
            // token DISTINTO de `Dot`, nunca ambiguo con un número (un float
            // como `1.5` ya se consume ENTERO más arriba, en `lex_number`,
            // antes de que este método vea el `.`) ni con acceso posicional
            // a tupla (`t.0`, que sigue siendo un `Dot` seguido de un
            // `Int`).
            '.' => self.one_or_two('.', TokenKind::Dot, TokenKind::DotDot),
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

    // ---- `for`/`in`/`..` (GRAMMAR.md §3.271) ----

    #[test]
    fn for_in_and_dotdot_tokenize_as_their_own_kinds() {
        assert_eq!(
            kinds("for i in 0..n"),
            vec![
                TokenKind::For,
                TokenKind::Ident("i".into()),
                TokenKind::In,
                TokenKind::Int(0),
                TokenKind::DotDot,
                TokenKind::Ident("n".into()),
                TokenKind::Eof,
            ]
        );
    }

    /// `1.2` (un float) sigue lexeando como UN token -- `lex_number` ya
    /// consume el `.` seguido de dígito ANTES de que `lex_punct` vea nada;
    /// `1..2` en cambio nunca entra a `lex_number` para el `..` porque no
    /// hay un dígito inmediatamente después del primer punto.
    #[test]
    fn dotdot_never_swallows_a_float_literal() {
        assert_eq!(kinds("1.2"), vec![TokenKind::Float(1.2), TokenKind::Eof]);
        assert_eq!(
            kinds("1..2"),
            vec![TokenKind::Int(1), TokenKind::DotDot, TokenKind::Int(2), TokenKind::Eof]
        );
    }

    /// `t.0` (acceso posicional a tupla, GRAMMAR.md §2.2) sigue siendo un
    /// `Dot` simple seguido de un `Int` -- no hay forma de que colisione con
    /// `..` porque acá solo hay UN punto.
    #[test]
    fn single_dot_tuple_index_is_unaffected() {
        assert_eq!(
            kinds("t.0"),
            vec![TokenKind::Ident("t".into()), TokenKind::Dot, TokenKind::Int(0), TokenKind::Eof]
        );
    }

    /// `for`/`in` como palabra COMPLETA -- un identificador que solo
    /// CONTIENE esas letras (`login`, `format`, `interior`) nunca se
    /// confunde, porque `lex_ident_or_keyword` ya consumió el identificador
    /// ENTERO antes de consultar la tabla de palabras clave.
    #[test]
    fn for_and_in_do_not_collide_with_identifiers_that_contain_them() {
        assert_eq!(
            kinds("login format interior"),
            vec![
                TokenKind::Ident("login".into()),
                TokenKind::Ident("format".into()),
                TokenKind::Ident("interior".into()),
                TokenKind::Eof,
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

    /// `///` (GRAMMAR.md §3.72) se saltea como trivia igual que `//` -- el
    /// stream de `TokenKind` no cambia -- pero además queda capturado en
    /// `leading_doc` del PRÓXIMO token real.
    #[test]
    fn a_triple_slash_comment_is_skipped_like_a_normal_comment_but_captured_as_leading_doc() {
        let tokens = tokenize("/// crea un usuario nuevo\nrpc create() -> Int { 1 }").unwrap();
        assert_eq!(
            tokens.iter().map(|t| t.kind.clone()).collect::<Vec<_>>(),
            kinds("rpc create() -> Int { 1 }")
        );
        assert_eq!(tokens[0].leading_doc.as_deref(), Some("crea un usuario nuevo"));
        assert!(tokens[1..].iter().all(|t| t.leading_doc.is_none()));
    }

    /// Varias líneas `///` consecutivas se unen con `\n` en un solo docstring.
    #[test]
    fn consecutive_triple_slash_lines_join_with_newlines_into_one_docstring() {
        let tokens = tokenize("/// linea uno\n/// linea dos\nrpc f() -> Int { 1 }").unwrap();
        assert_eq!(tokens[0].leading_doc.as_deref(), Some("linea uno\nlinea dos"));
    }

    /// `////` (4+ slashes) NO es un docstring -- sigue siendo un separador
    /// visual común (`//// Sección ////`), tratado como comentario normal.
    #[test]
    fn four_or_more_slashes_is_not_a_docstring() {
        let tokens = tokenize("//// Sección ////\nrpc f() -> Int { 1 }").unwrap();
        assert!(tokens[0].leading_doc.is_none());
    }

    /// Un `///` sin nada detrás (línea vacía) produce un docstring vacío,
    /// no `None` -- distinción real: "documentado, pero en blanco" no es lo
    /// mismo que "sin documentar".
    #[test]
    fn an_empty_triple_slash_line_produces_an_empty_string_not_none() {
        let tokens = tokenize("///\nrpc f() -> Int { 1 }").unwrap();
        assert_eq!(tokens[0].leading_doc.as_deref(), Some(""));
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

    // ---- pragma `#linkc >= "X.Y.Z"` (GRAMMAR.md §3.263) ----

    #[test]
    fn version_pragma_satisfied_tokenizes_normally() {
        let source = "#linkc >= \"0.1.0\"\ntype User = { id: Int }";
        let tokens = tokenize(source).unwrap();
        assert_eq!(tokens.first().map(|t| &t.kind), Some(&TokenKind::Type));
    }

    #[test]
    fn version_pragma_after_leading_blank_and_comment_lines_is_still_recognized() {
        // Mismo lugar donde ya viven los headers de licencia/descripción.
        let source = "\n// segurma.link -- header real\n//\n#linkc >= \"0.1.0\"\ntype User = { id: Int }";
        assert!(tokenize(source).is_ok());
    }

    #[test]
    fn version_pragma_unsatisfied_fails_before_lexing_anything_else() {
        let source = "#linkc >= \"999.0.0\"\ntype User = { id: Int }";
        let err = tokenize(source).unwrap_err();
        assert!(err.message.contains("999.0.0"), "{err:?}");
        assert!(err.message.contains(crate::VERSION), "{err:?}");
    }

    #[test]
    fn version_pragma_malformed_gives_a_clear_error_not_a_lex_error_on_hash() {
        let source = "#linkc >= 1.0.0\ntype User = { id: Int }";
        let err = tokenize(source).unwrap_err();
        assert!(err.message.contains("mal formado"), "{err:?}");
    }

    #[test]
    fn version_pragma_error_span_points_at_the_pragma_line() {
        let source = "// header\n#linkc >= \"999.0.0\"\ntype User = { id: Int }";
        let err = tokenize(source).unwrap_err();
        assert_eq!(err.span.line, 2, "{err:?}");
        assert_eq!(err.span.col, 1, "{err:?}");
    }

    #[test]
    fn no_pragma_present_is_unaffected_and_bare_hash_later_still_errors_as_before() {
        // `#` en cualquier otro lado del archivo sigue siendo el error de
        // "carácter inesperado" de siempre -- el pragma solo se reconoce
        // entre las líneas en blanco/`//` del principio.
        let source = "type User = { id: Int }\n# esto no es un pragma";
        let err = tokenize(source).unwrap_err();
        assert!(!err.message.contains("linkc"), "{err:?}");
    }

    #[test]
    fn hash_after_real_code_on_first_line_is_not_treated_as_pragma() {
        let source = "type X = { id: Int } # no es un pragma";
        let err = tokenize(source).unwrap_err();
        assert!(!err.message.contains("mal formado"), "{err:?}");
    }

    // ---- literal `html`...`` (GRAMMAR.md §3.268) ----

    fn html_parts(source: &str) -> Vec<HtmlPart> {
        let tokens = tokenize(source).unwrap_or_else(|e| panic!("{e}"));
        match &tokens[0].kind {
            TokenKind::HtmlLit(parts) => parts.clone(),
            other => panic!("se esperaba HtmlLit, se encontró {other:?}"),
        }
    }

    #[test]
    fn html_literal_with_no_interpolation_is_a_single_text_part() {
        let parts = html_parts("html`<h1>Hola</h1>`");
        assert_eq!(parts, vec![HtmlPart::Text("<h1>Hola</h1>".into())]);
    }

    #[test]
    fn html_literal_supports_raw_multiline_text_without_any_escape() {
        // A diferencia de un string "...", un '\n' CRUDO adentro es válido tal cual.
        let parts = html_parts("html`<div>\n  <p>hola</p>\n</div>`");
        assert_eq!(parts, vec![HtmlPart::Text("<div>\n  <p>hola</p>\n</div>".into())]);
    }

    #[test]
    fn html_literal_splits_text_and_interpolated_expression() {
        let parts = html_parts("html`<h1>${name}</h1>`");
        assert_eq!(parts.len(), 3, "{parts:?}");
        assert_eq!(parts[0], HtmlPart::Text("<h1>".into()));
        match &parts[1] {
            HtmlPart::Expr(tokens) => assert_eq!(tokens.iter().map(|t| &t.kind).collect::<Vec<_>>(), vec![&TokenKind::Ident("name".into())]),
            other => panic!("{other:?}"),
        }
        assert_eq!(parts[2], HtmlPart::Text("</h1>".into()));
    }

    #[test]
    fn html_literal_interpolation_can_contain_a_full_expression_with_braces() {
        // El conteo de profundidad es a nivel de TOKEN (LBrace/RBrace), no de
        // carácter crudo -- un `{`/`}` de un literal de struct adentro de un
        // `${...}` no cierra la interpolación antes de tiempo.
        let parts = html_parts("html`${Point { x: 1, y: 2 }}`");
        assert_eq!(parts.len(), 3, "{parts:?}");
        assert_eq!(parts[0], HtmlPart::Text("".into()));
        match &parts[1] {
            HtmlPart::Expr(tokens) => {
                let kinds: Vec<&TokenKind> = tokens.iter().map(|t| &t.kind).collect();
                assert_eq!(
                    kinds,
                    vec![
                        &TokenKind::Ident("Point".into()),
                        &TokenKind::LBrace,
                        &TokenKind::Ident("x".into()),
                        &TokenKind::Colon,
                        &TokenKind::Int(1),
                        &TokenKind::Comma,
                        &TokenKind::Ident("y".into()),
                        &TokenKind::Colon,
                        &TokenKind::Int(2),
                        &TokenKind::RBrace,
                    ]
                );
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(parts[2], HtmlPart::Text("".into()));
    }

    #[test]
    fn html_literal_can_nest_another_html_literal_inside_an_interpolation() {
        let parts = html_parts("html`<div>${html`<span>x</span>`}</div>`");
        assert_eq!(parts.len(), 3, "{parts:?}");
        match &parts[1] {
            HtmlPart::Expr(tokens) => {
                assert_eq!(tokens.len(), 1);
                match &tokens[0].kind {
                    TokenKind::HtmlLit(inner) => assert_eq!(*inner, vec![HtmlPart::Text("<span>x</span>".into())]),
                    other => panic!("{other:?}"),
                }
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn html_literal_supports_escaped_backtick_dollar_and_backslash() {
        let parts = html_parts(r#"html`\`\$\\`"#);
        assert_eq!(parts, vec![HtmlPart::Text("`$\\".into())]);
    }

    #[test]
    fn html_literal_rejects_an_empty_interpolation() {
        let err = tokenize("html`${}`").unwrap_err();
        assert!(err.message.contains("vacío"), "{err:?}");
    }

    #[test]
    fn html_literal_rejects_being_unterminated() {
        let err = tokenize("html`<div>").unwrap_err();
        assert!(err.message.contains("sin cerrar"), "{err:?}");
    }

    #[test]
    fn html_literal_rejects_an_unterminated_interpolation() {
        let err = tokenize("html`${name").unwrap_err();
        assert!(err.message.contains("'${...}'") && err.message.contains("sin cerrar"), "{err:?}");
    }

    #[test]
    fn an_identifier_named_html_without_a_backtick_is_a_plain_identifier() {
        let kinds = kinds("html + 1");
        assert_eq!(kinds, vec![TokenKind::Ident("html".into()), TokenKind::Plus, TokenKind::Int(1), TokenKind::Eof]);
    }

    #[test]
    fn html_literal_error_span_points_at_the_opening_backtick() {
        let err = tokenize("let x = html`unterminated").unwrap_err();
        assert_eq!(err.span.col, 13, "{err:?}"); // 1-based: la ` de apertura
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
