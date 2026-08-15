use crate::lexer;
use crate::token::TokenKind;

pub fn format_source(src: &str) -> Result<String, String> {
    let tokens = lexer::tokenize(src).map_err(|e| e.message)?;
    let mut out = String::new();
    let mut indent_level = 0usize;
    let mut at_line_start = true;
    let mut prev_kind: Option<TokenKind> = None;

    let mut i = 0;
    while i < tokens.len() {
        let tok = &tokens[i];
        let kind = &tok.kind;

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

        if at_line_start {
            out.push_str(&"  ".repeat(indent_level));
            at_line_start = false;
        }

        // Espaciado antes del token según token anterior
        if let Some(prev) = &prev_kind {
            if needs_space_before(prev, kind) {
                out.push(' ');
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
            | TokenKind::Pipe
    ) {
        return true;
    }

    if matches!(prev, TokenKind::Ident(_)) && matches!(curr, TokenKind::Ident(_)) {
        return true;
    }

    false
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

    #[test]
    fn test_format_service_and_test_blocks() {
        let raw = "service S{rpc ping()->Int{1}}test \"ping works\"{assert(S.ping()==1);}";
        let formatted = format_source(raw).unwrap();
        assert!(formatted.contains("service S {\n  rpc ping() -> Int {\n    1\n  }\n}"));
        assert!(formatted.contains("test \"ping works\" {\n  assert(S.ping() == 1);\n}"));
    }
}
