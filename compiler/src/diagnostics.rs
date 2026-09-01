// Renderizado de diagnósticos estilo gcc/rustc (archivo:línea:columna +
// snippet + caret) -- sin agregar ninguna dependencia PARA ESTO puntual
// (`ariadne`/`codespan-reporting` quedan afuera a propósito, no hace falta
// una librería para un span de una sola línea a la vez). Prerrequisito real
// para un LSP (GRAMMAR.md/README "needs span-tracking... first"), no el
// LSP en sí -- ver PLAN.md para el resto de lo que falta.

use crate::token::Span;

const TAB_WIDTH: usize = 4;

/// Diagnóstico de una sola línea:
/// ```text
/// error: <mensaje>
///   --> archivo.link:3:8
///    |
///  3 | rpc f(x Int) -> Int { x }
///    |        ^^^
/// ```
/// Asume que `span` cae dentro de UNA sola línea -- garantizado hoy por
/// `LexError`/`ParseError` (ver el fix de los casos "sin cerrar" en
/// `lexer.rs`, que ancla el span en el delimitador de apertura en vez de en
/// EOF) -- pero clampea `col`/el ancho del subrayado al largo real de la
/// línea como defensa en profundidad, no como comportamiento esperado.
pub fn render_diagnostic(source: &str, file_label: &str, span: Span, message: &str, code: Option<&str>) -> String {
    let line_text = source.lines().nth(span.line.saturating_sub(1)).unwrap_or("");
    let line_chars: Vec<char> = line_text.chars().collect();

    let col0 = span.col.saturating_sub(1).min(line_chars.len());
    let max_width = line_chars.len().saturating_sub(col0).max(1);
    let width = span.end.saturating_sub(span.start).max(1).min(max_width);

    // Tabs se expanden a un ancho fijo TANTO en la línea impresa como en el
    // padding del caret -- si no, un tab antes del caret lo desalinearía en
    // cualquier terminal que expanda tabs a más de 1 columna.
    let rendered_line = expand_tabs(&line_chars);
    let padding: String = line_chars[..col0]
        .iter()
        .map(|&c| if c == '\t' { " ".repeat(TAB_WIDTH) } else { " ".to_string() })
        .collect();
    let caret = "^".repeat(width);

    let gutter = span.line.to_string();
    let margin = " ".repeat(gutter.len());

    // GRAMMAR.md §3.210: `error[L0001]: ...` en vez de `error: ...` cuando
    // el error tiene un código estable -- mismo formato exacto que
    // `rustc` usa para `error[E0308]: ...`, a propósito (un formato ya
    // conocido por cualquiera que haya visto un error de Rust, humano o
    // agente, en vez de inventar uno nuevo).
    let header = match code {
        Some(c) => format!("error[{c}]"),
        None => "error".to_string(),
    };

    format!(
        "{header}: {message}\n  --> {file_label}:{}:{}\n{margin} |\n{gutter} | {rendered_line}\n{margin} | {padding}{caret}",
        span.line, span.col
    )
}

fn expand_tabs(chars: &[char]) -> String {
    chars.iter().map(|&c| if c == '\t' { " ".repeat(TAB_WIDTH) } else { c.to_string() }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start: usize, end: usize, line: usize, col: usize) -> Span {
        Span::new(start, end, line, col)
    }

    #[test]
    fn renders_snippet_and_caret_at_the_right_column() {
        // col 7 (1-based) de "rpc f(x Int) -> Int { x }" es la 'x' del
        // parámetro -- r=1,p=2,c=3,' '=4,f=5,(=6,x=7.
        let source = "type User = {\nrpc f(x Int) -> Int { x }\n}";
        let out = render_diagnostic(source, "demo.link", span(4, 5, 2, 7), "se esperaba ':'", None);
        assert!(out.contains("demo.link:2:7"), "{out}");
        let mut lines = out.lines();
        let source_line = lines.nth(3).unwrap(); // "2 | rpc f(x Int) -> Int { x }"
        let caret_line = lines.next().unwrap();
        assert!(source_line.contains("rpc f(x Int) -> Int { x }"), "{out}");
        // El '^' tiene que caer exactamente en la misma columna de texto
        // donde está la 'x' en la línea de arriba -- comparando índices
        // dentro de sus respectivos textos después de "| ", no un string
        // armado a mano (evita que un espacio de formato mal contado en el
        // test se confunda con un bug real del renderer).
        let source_text = source_line.split_once("| ").unwrap().1;
        let caret_text = caret_line.split_once("| ").unwrap().1;
        let x_index = source_text.find('x').unwrap();
        let caret_index = caret_text.find('^').unwrap();
        assert_eq!(x_index, caret_index, "{out}");
    }

    #[test]
    fn tabs_before_the_span_are_expanded_consistently_in_both_lines() {
        let source = "\tbad";
        let out = render_diagnostic(source, "demo.link", span(1, 2, 1, 2), "carácter inesperado", None);
        let mut lines = out.lines();
        let source_line = lines.nth(3).unwrap(); // "1 | <tab expandido>bad"
        let caret_line = lines.next().unwrap();
        let source_prefix_len = source_line.split('|').nth(1).unwrap().len() - "bad".len();
        let caret_padding_len = caret_line.split('|').nth(1).unwrap().len() - 1; // -1 por el '^'
        assert_eq!(source_prefix_len, caret_padding_len, "{out}");
    }

    #[test]
    fn out_of_range_span_is_clamped_instead_of_panicking() {
        // Defensa en profundidad -- no debería pasar en la práctica (el fix
        // de lexer.rs lo garantiza), pero el renderer no debería panicar si
        // algún caso futuro no respeta el invariante de una sola línea.
        let out = render_diagnostic("x", "demo.link", span(0, 999, 1, 999), "mensaje", None);
        assert!(out.contains("demo.link:1:999"));
    }

    #[test]
    fn missing_line_number_does_not_panic() {
        let out = render_diagnostic("x", "demo.link", span(0, 1, 5, 1), "mensaje", None);
        assert!(out.contains("demo.link:5:1"));
    }

    #[test]
    fn a_coded_error_gets_the_rustc_style_error_bracket_code_header() {
        // GRAMMAR.md §3.210: mismo formato exacto que `error[E0308]: ...`
        // de rustc -- `None` (la mayoría de los errores) sigue sin bracket.
        let coded = render_diagnostic("x", "demo.link", span(0, 1, 1, 1), "mensaje", Some("L0001"));
        assert!(coded.starts_with("error[L0001]: mensaje\n"), "{coded}");
        let uncoded = render_diagnostic("x", "demo.link", span(0, 1, 1, 1), "mensaje", None);
        assert!(uncoded.starts_with("error: mensaje\n"), "{uncoded}");
    }
}
