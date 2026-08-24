use crate::lexer;
use crate::token::TokenKind;

pub fn format_source(src: &str) -> Result<String, String> {
    let tokens = lexer::tokenize(src).map_err(|e| e.message)?;
    // El lexer descarta los comentarios (no hay `TokenKind::Comment`), así que
    // formatear solo con la lista de tokens borraría toda la documentación del
    // archivo. Los `Span` son índices sobre ESTE vector de chars, así que el
    // texto original de cada comentario se recupera del hueco entre dos tokens.
    let chars: Vec<char> = src.chars().collect();
    let mut out = String::new();
    let mut indent_level = 0usize;
    let mut at_line_start = true;
    let mut prev_kind: Option<TokenKind> = None;
    let mut prev_end = 0usize;

    let mut i = 0;
    while i < tokens.len() {
        let tok = &tokens[i];
        let kind = &tok.kind;

        // Antes de imprimir el token: recuperar los comentarios que lo preceden.
        // Va ANTES del `RBrace` de abajo a propósito -- un comentario escrito
        // justo arriba de un `}` pertenece al bloque que se está cerrando, y
        // por eso se indenta con el nivel de adentro, todavía sin decrementar.
        emit_gap_comments(&chars, prev_end, tok.span.start, &mut out, &mut at_line_start, indent_level);
        prev_end = tok.span.end;

        if matches!(kind, TokenKind::Eof) {
            break;
        }

        // Manejo de cierre de bloque: reducir indentación antes de imprimir
        if matches!(kind, TokenKind::RBrace) {
            if indent_level > 0 {
                indent_level -= 1;
            }
            if !at_line_start {
                out.push('\n');
                at_line_start = true;
            }
        }

        // Una declaración de nivel superior que no termina en `}` ni en `;`
        // (p. ej. `type Ids = Int[]`) no dispara ningún salto de línea, y la
        // siguiente declaración quedaba pegada a ella en el mismo renglón.
        if indent_level == 0 && !at_line_start && starts_top_level_decl(kind) {
            out.push('\n');
            out.push('\n');
            at_line_start = true;
        }

        let starting_line = at_line_start;
        if at_line_start {
            out.push_str(&"  ".repeat(indent_level));
            at_line_start = false;
        }

        // Espaciado antes del token según token anterior. Se omite al abrir
        // línea: ahí el separador ya es la indentación, y un espacio extra
        // dejaría sangrías de ancho inconsistente.
        if !starting_line {
            if let Some(prev) = &prev_kind {
                if needs_space_before(prev, kind) {
                    out.push(' ');
                }
            }
        }

        // Renderizar el token
        match kind {
            TokenKind::Ident(name) => out.push_str(name),
            TokenKind::Int(n) => out.push_str(&n.to_string()),
            TokenKind::Float(f) => out.push_str(&f.to_string()),
            TokenKind::Str(s) => {
                out.push('"');
                out.push_str(&s.replace('\\', "\\\\").replace('"', "\\\""));
                out.push('"');
            }
            TokenKind::True => out.push_str("true"),
            TokenKind::False => out.push_str("false"),
            TokenKind::Null => out.push_str("null"),
            TokenKind::Let => out.push_str("let"),
            TokenKind::Mut => out.push_str("mut"),
            TokenKind::Const => out.push_str("const"),
            TokenKind::Type => out.push_str("type"),
            TokenKind::Enum => out.push_str("enum"),
            TokenKind::Service => out.push_str("service"),
            TokenKind::Rpc => out.push_str("rpc"),
            TokenKind::Stream => out.push_str("stream"),
            TokenKind::Fn => out.push_str("fn"),
            TokenKind::If => out.push_str("if"),
            TokenKind::Else => out.push_str("else"),
            TokenKind::While => out.push_str("while"),
            TokenKind::Match => out.push_str("match"),
            TokenKind::Return => out.push_str("return"),
            TokenKind::Test => out.push_str("test"),
            TokenKind::Import => out.push_str("import"),
            TokenKind::From => out.push_str("from"),
            TokenKind::Pub => out.push_str("pub"),
            TokenKind::At => out.push('@'),

            TokenKind::Plus => out.push('+'),
            TokenKind::Minus => out.push('-'),
            TokenKind::Star => out.push('*'),
            TokenKind::Slash => out.push('/'),
            TokenKind::Percent => out.push('%'),
            TokenKind::Equals => out.push('='),
            TokenKind::EqEq => out.push_str("=="),
            TokenKind::NotEq => out.push_str("!="),
            TokenKind::Lt => out.push('<'),
            TokenKind::LtEq => out.push_str("<="),
            TokenKind::Gt => out.push('>'),
            TokenKind::GtEq => out.push_str(">="),
            TokenKind::AmpAmp => out.push_str("&&"),
            TokenKind::PipePipe => out.push_str("||"),
            TokenKind::Pipe => out.push('|'),
            TokenKind::Bang => out.push('!'),
            TokenKind::Question => out.push('?'),
            TokenKind::QuestionQuestion => out.push_str("??"),
            TokenKind::Arrow => out.push_str("->"),
            TokenKind::FatArrow => out.push_str("=>"),
            TokenKind::Dot => out.push('.'),
            TokenKind::Comma => out.push(','),
            TokenKind::Colon => out.push(':'),
            TokenKind::Semi => out.push(';'),
            TokenKind::LParen => out.push('('),
            TokenKind::RParen => out.push(')'),
            TokenKind::LBracket => out.push('['),
            TokenKind::RBracket => out.push(']'),
            TokenKind::LBrace => {
                out.push('{');
                indent_level += 1;
            }
            TokenKind::RBrace => out.push('}'),
            TokenKind::Eof => {}
        }

        // Salto de línea después de ciertos tokens
        if matches!(kind, TokenKind::LBrace | TokenKind::Semi) {
            out.push('\n');
            at_line_start = true;
        } else if matches!(kind, TokenKind::RBrace) {
            out.push('\n');
            if indent_level == 0 {
                out.push('\n');
            }
            at_line_start = true;
        }

        prev_kind = Some(kind.clone());
        i += 1;
    }

    let trimmed = out.trim_end();
    let mut final_res = trimmed.to_string();
    final_res.push('\n');
    Ok(final_res)
}

fn needs_space_before(prev: &TokenKind, curr: &TokenKind) -> bool {
    if matches!(
        curr,
        TokenKind::Comma
            | TokenKind::Semi
            | TokenKind::RParen
            | TokenKind::RBracket
            | TokenKind::Dot
            | TokenKind::Question
    ) {
        return false;
    }

    if matches!(prev, TokenKind::Dot | TokenKind::Bang | TokenKind::LParen | TokenKind::LBracket | TokenKind::At) {
        return false;
    }

    if matches!(
        prev,
        TokenKind::Comma
            | TokenKind::Colon
            | TokenKind::Arrow
            | TokenKind::FatArrow
            | TokenKind::Let
            | TokenKind::Mut
            | TokenKind::Const
            | TokenKind::Type
            | TokenKind::Enum
            | TokenKind::Service
            | TokenKind::Rpc
            | TokenKind::Stream
            | TokenKind::Fn
            | TokenKind::If
            | TokenKind::Else
            | TokenKind::While
            | TokenKind::Match
            | TokenKind::Return
            | TokenKind::Test
            | TokenKind::Import
            | TokenKind::From
            | TokenKind::Pub
    ) {
        return true;
    }

    if matches!(
        curr,
        TokenKind::Plus
            | TokenKind::Minus
            | TokenKind::Star
            | TokenKind::Slash
            | TokenKind::Percent
            | TokenKind::Equals
            | TokenKind::EqEq
            | TokenKind::NotEq
            | TokenKind::Lt
            | TokenKind::LtEq
            | TokenKind::Gt
            | TokenKind::GtEq
            | TokenKind::AmpAmp
            | TokenKind::PipePipe
            | TokenKind::QuestionQuestion
            | TokenKind::Pipe
            | TokenKind::Arrow
            | TokenKind::FatArrow
            | TokenKind::LBrace
    ) {
        return true;
    }

    if matches!(
        prev,
        TokenKind::Plus
            | TokenKind::Minus
            | TokenKind::Star
            | TokenKind::Slash
            | TokenKind::Percent
            | TokenKind::Equals
            | TokenKind::EqEq
            | TokenKind::NotEq
            | TokenKind::Lt
            | TokenKind::LtEq
            | TokenKind::Gt
            | TokenKind::GtEq
            | TokenKind::AmpAmp
            | TokenKind::PipePipe
            | TokenKind::QuestionQuestion
            | TokenKind::Pipe
    ) {
        return true;
    }

    // Dos tokens "de palabra" seguidos SIEMPRE necesitan un separador. Antes
    // esta regla solo cubría `Ident` + `Ident`, y por eso el identificador de
    // una anotación quedaba pegado a la palabra clave siguiente: `@authenticated`
    // seguido de `rpc` se emitía como `@authenticatedrpc`, que ya no parsea.
    // `rpc`/`stream`/`fn` no son `Ident`, son sus propios `TokenKind`.
    if is_word_like(prev) && is_word_like(curr) {
        return true;
    }

    // Un cierre de paréntesis/corchete seguido de una palabra también se
    // pegaría: `@requires(Role.Admin)` + `rpc` daba `@requires(Role.Admin)rpc`,
    // y `type Ids = Int[]` + `type` daba `Int[]type`.
    if matches!(prev, TokenKind::RParen | TokenKind::RBracket) && is_word_like(curr) {
        return true;
    }

    false
}

/// Un token es "de palabra" si se renderiza como una secuencia de caracteres
/// alfanuméricos: identificadores, literales y palabras clave. Dos de ellos
/// pegados se leerían como un único token distinto.
fn is_word_like(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Ident(_)
            | TokenKind::Int(_)
            | TokenKind::Float(_)
            | TokenKind::True
            | TokenKind::False
            | TokenKind::Null
            | TokenKind::Let
            | TokenKind::Mut
            | TokenKind::Const
            | TokenKind::Type
            | TokenKind::Enum
            | TokenKind::Service
            | TokenKind::Rpc
            | TokenKind::Stream
            | TokenKind::Fn
            | TokenKind::If
            | TokenKind::Else
            | TokenKind::While
            | TokenKind::Match
            | TokenKind::Return
            | TokenKind::Test
            | TokenKind::Import
            | TokenKind::From
            | TokenKind::Pub
    )
}

/// Tokens que solo pueden aparecer abriendo una declaración de nivel superior.
/// `db` es un `Ident` y no una palabra clave, pero en columna cero tampoco
/// puede ser otra cosa: ahí no hay expresiones donde un identificador suelto
/// sea válido.
fn starts_top_level_decl(kind: &TokenKind) -> bool {
    match kind {
        TokenKind::Type
        | TokenKind::Enum
        | TokenKind::Service
        | TokenKind::Fn
        | TokenKind::Test
        | TokenKind::Import
        | TokenKind::Const
        | TokenKind::Pub => true,
        TokenKind::Ident(name) => name == "db",
        _ => false,
    }
}

/// Re-emite los comentarios que quedaron en el hueco `[from, to)` del fuente
/// original. `own_line` distingue el comentario que estaba en su propia línea
/// del que iba al final de una línea de código, para no moverlo de lugar.
fn emit_gap_comments(
    chars: &[char],
    from: usize,
    to: usize,
    out: &mut String,
    at_line_start: &mut bool,
    indent_level: usize,
) {
    if from >= to || to > chars.len() {
        return;
    }
    let gap = &chars[from..to];
    let mut j = 0usize;
    let mut saw_newline = false;

    while j < gap.len() {
        let c = gap[j];
        if c == '\n' {
            saw_newline = true;
            j += 1;
        } else if c.is_whitespace() {
            j += 1;
        } else if c == '/' && gap.get(j + 1) == Some(&'/') {
            let start = j;
            while j < gap.len() && gap[j] != '\n' {
                j += 1;
            }
            let text: String = gap[start..j].iter().collect();
            push_comment(out, text.trim_end(), saw_newline, at_line_start, indent_level);
            saw_newline = false;
        } else if c == '/' && gap.get(j + 1) == Some(&'*') {
            let start = j;
            j += 2;
            while j + 1 < gap.len() && !(gap[j] == '*' && gap[j + 1] == '/') {
                j += 1;
            }
            j = (j + 2).min(gap.len());
            let text: String = gap[start..j].iter().collect();
            push_comment(out, &text, saw_newline, at_line_start, indent_level);
            saw_newline = false;
        } else {
            // Fuera de comentarios, un hueco entre tokens solo puede tener
            // espacios: cualquier otra cosa sería un token que el lexer emitió.
            j += 1;
        }
    }
}

fn push_comment(
    out: &mut String,
    text: &str,
    own_line: bool,
    at_line_start: &mut bool,
    indent_level: usize,
) {
    if own_line || *at_line_start {
        if !*at_line_start {
            out.push('\n');
        }
        out.push_str(&"  ".repeat(indent_level));
    } else {
        out.push(' ');
    }
    out.push_str(text);
    out.push('\n');
    *at_line_start = true;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_basic_function_and_types() {
        let raw = "fn add(a:Int,b:Int)->Int{let sum=a+b;sum}\ntype Point={x:Int,y:Int}";
        let formatted = format_source(raw).unwrap();
        assert!(formatted.contains("fn add(a: Int, b: Int) -> Int {\n  let sum = a + b;\n  sum\n}\n\ntype Point = {"));
    }

    /// Regresión: `needs_space_before` solo separaba `Ident` + `Ident`, así que
    /// el nombre de la anotación se fusionaba con la palabra clave siguiente y
    /// el archivo formateado dejaba de parsear (`@authenticatedrpc`).
    #[test]
    fn annotations_keep_a_separator_from_the_keyword_that_follows() {
        let raw = "service S{
@authenticated
rpc a()->Int{1}
@requires(R.Admin)
rpc b()->Int{2}
@authenticated
stream c()->Int{while true{db.x.subscribe()}}
}";
        let formatted = format_source(raw).unwrap();
        assert!(!formatted.contains("@authenticatedrpc"), "anotación fusionada con rpc: {formatted}");
        assert!(!formatted.contains("@authenticatedstream"), "anotación fusionada con stream: {formatted}");
        assert!(!formatted.contains(")rpc"), "cierre de @requires fusionado con rpc: {formatted}");
        assert!(formatted.contains("@authenticated rpc a()"));
        assert!(formatted.contains("@requires(R.Admin) rpc b()"));
        assert!(formatted.contains("@authenticated stream c()"));
    }

    /// Regresión: el lexer descarta los comentarios, así que formatear
    /// reescribía el archivo sin ninguno de ellos -- una pérdida silenciosa.
    #[test]
    fn comments_survive_formatting_in_every_position() {
        let raw = "// cabecera
type T={
  // sobre el campo
  a: Int, // al final
}
";
        let formatted = format_source(raw).unwrap();
        assert!(formatted.contains("// cabecera"), "se perdió el comentario de cabecera: {formatted}");
        assert!(formatted.contains("// sobre el campo"), "se perdió el comentario de línea propia: {formatted}");
        assert!(formatted.contains("// al final"), "se perdió el comentario final de línea: {formatted}");
        assert!(formatted.contains("a: Int, // al final"), "el comentario final cambió de línea: {formatted}");
    }

    /// Un comentario justo antes de `}` pertenece al bloque que se cierra, y
    /// tiene que quedar con la indentación de adentro, no la de afuera.
    #[test]
    fn a_comment_before_a_closing_brace_keeps_the_inner_indentation() {
        let raw = "service S{
rpc a()->Int{
// nota
1
}
}";
        let formatted = format_source(raw).unwrap();
        assert!(formatted.contains("    // nota"), "indentación incorrecta: {formatted}");
    }

    /// Dos declaraciones seguidas donde la primera no cierra con `}` ni `;`
    /// terminaban en el mismo renglón (`type Ids = Int[]type Item = {`).
    #[test]
    fn consecutive_top_level_declarations_do_not_share_a_line() {
        let raw = "type Ids=Int[]
type Names=String[]
db{items: Ids}";
        let formatted = format_source(raw).unwrap();
        assert!(!formatted.contains("Int[] type"), "declaraciones en la misma línea: {formatted}");
        assert!(formatted.contains("type Ids = Int[]

type Names = String[]"), "{formatted}");
        assert!(formatted.contains("

db {"), "{formatted}");
    }

    /// Formatear tiene que ser un punto fijo: la segunda pasada no puede
    /// mover nada, o `--check` en CI daría falsos positivos eternos.
    #[test]
    fn formatting_is_idempotent() {
        let raw = "// cabecera
type T={a: Int, // fin
}
service S{
@authenticated
rpc a()->Int{
// nota
1
}
}";
        let once = format_source(raw).unwrap();
        let twice = format_source(&once).unwrap();
        assert_eq!(once, twice, "la segunda pasada cambió el resultado");
    }

    #[test]
    fn test_format_service_and_test_blocks() {
        let raw = "service S{rpc ping()->Int{1}}test \"ping works\"{assert(S.ping()==1);}";
        let formatted = format_source(raw).unwrap();
        assert!(formatted.contains("service S {\n  rpc ping() -> Int {\n    1\n  }\n}"));
        assert!(formatted.contains("test \"ping works\" {\n  assert(S.ping() == 1);\n}"));
    }
}
