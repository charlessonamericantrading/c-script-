// Generación real de bytes de PDF (GRAMMAR.md §3.201) a partir de una lista
// de `PdfBlockSpec` -- ver el comentario en `compiler/Cargo.toml` sobre por
// qué `pdf-writer` es la cuarta excepción real a "cero dependencias
// nuevas". Deliberadamente de alcance chico (v1): página A4 fija, márgenes
// fijos, una de las 14 fuentes estándar de PDF (Helvetica/Helvetica-Bold)
// SIN EMBEBER -- evita toda la complejidad de fuentes custom. Paginación
// automática vertical es el único trabajo de layout real de este módulo.
// Separado en dos pasadas: `layout` decide QUÉ va en cada página (sin tocar
// `pdf-writer` para nada), `render` convierte ESO en bytes de PDF reales --
// mantiene la aritmética de posición lejos de la API de bajo nivel.

use pdf_writer::{Content, Finish, Name, Pdf, Rect, Ref, Str};

const PAGE_WIDTH: f32 = 595.0; // A4, en puntos (72pt = 1 pulgada)
const PAGE_HEIGHT: f32 = 842.0;
const MARGIN: f32 = 50.0;
const LINE_HEIGHT_FACTOR: f32 = 1.4; // separación entre líneas, relativa al tamaño de fuente
const TABLE_FONT_SIZE: f32 = 10.0;
/// Ancho promedio de un glyph de Helvetica, en fracción del tamaño de
/// fuente -- Helvetica no es monoespaciada y `pdf-writer` no da métricas de
/// glyph reales, así que el wrap/truncado de texto es una APROXIMACIÓN
/// deliberada, no una medición pixel-perfect (límite honesto documentado en
/// GRAMMAR.md §3.201).
const AVG_CHAR_WIDTH_FACTOR: f32 = 0.5;

#[derive(Debug, Clone)]
pub enum PdfBlockSpec {
    Text { content: String, bold: bool, size: f32 },
    Table { headers: Vec<String>, rows: Vec<Vec<String>> },
}

struct DrawOp {
    x: f32,
    y: f32,
    bold: bool,
    size: f32,
    text: String,
}

/// Codifica un `&str` a los bytes que `pdf-writer` necesita para dibujarlo
/// con una fuente estándar sin embeber (`/Encoding /WinAnsiEncoding`) --
/// WinAnsiEncoding coincide con Latin-1 en el rango 0xA0-0xFF (donde caen
/// ñ/á/é/í/ó/ú/¿/¡, lo que necesita una factura real en español), más un
/// caso especial para el símbolo Euro (U+20AC -> 0x80, la única posición
/// donde WinAnsi diverge de Latin-1 que un documento financiero real
/// probablemente use). Cualquier otro caracter fuera de ese rango (CJK,
/// emoji, ...) se reemplaza por '?' -- límite v1 honesto, no soporte
/// Unicode completo (documentado en GRAMMAR.md §3.201).
fn encode_winansi(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| {
            let cp = c as u32;
            if cp == 0x20AC {
                0x80
            } else if cp < 0x100 {
                cp as u8
            } else {
                b'?'
            }
        })
        .collect()
}

fn truncate_to_width(s: &str, size: f32, max_width: f32) -> String {
    let max_chars = ((max_width / (size * AVG_CHAR_WIDTH_FACTOR)).floor().max(1.0)) as usize;
    s.chars().take(max_chars).collect()
}

/// Parte `s` en líneas que entran en `max_width` (en puntos, al tamaño
/// `size`), por palabra -- una palabra individual más larga que el ancho
/// disponible se trunca a ese ancho en vez de partirse en varias líneas
/// (límite v1 adicional, mismo espíritu que el truncado de celda).
fn wrap_to_width(s: &str, size: f32, max_width: f32) -> Vec<String> {
    let max_chars = ((max_width / (size * AVG_CHAR_WIDTH_FACTOR)).floor().max(1.0)) as usize;
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in s.split_whitespace() {
        let word: String = if word.chars().count() > max_chars {
            word.chars().take(max_chars).collect()
        } else {
            word.to_string()
        };
        let candidate_len = if current.is_empty() {
            word.chars().count()
        } else {
            current.chars().count() + 1 + word.chars().count()
        };
        if candidate_len <= max_chars {
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(&word);
        } else {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            current = word;
        }
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

fn ensure_room(pages: &mut Vec<Vec<DrawOp>>, y: &mut f32, size: f32) {
    if *y - size * LINE_HEIGHT_FACTOR < MARGIN {
        pages.push(Vec::new());
        *y = PAGE_HEIGHT - MARGIN;
    }
}

fn layout(blocks: &[PdfBlockSpec]) -> Vec<Vec<DrawOp>> {
    let usable_width = PAGE_WIDTH - 2.0 * MARGIN;
    let mut pages: Vec<Vec<DrawOp>> = vec![Vec::new()];
    let mut y = PAGE_HEIGHT - MARGIN;

    for block in blocks {
        match block {
            PdfBlockSpec::Text { content, bold, size } => {
                for line in wrap_to_width(content, *size, usable_width) {
                    ensure_room(&mut pages, &mut y, *size);
                    pages.last_mut().unwrap().push(DrawOp { x: MARGIN, y, bold: *bold, size: *size, text: line });
                    y -= size * LINE_HEIGHT_FACTOR;
                }
            }
            PdfBlockSpec::Table { headers, rows } => {
                let cols = if !headers.is_empty() {
                    headers.len()
                } else {
                    rows.first().map(|r| r.len()).unwrap_or(1)
                }
                .max(1);
                let col_width = usable_width / cols as f32;

                if !headers.is_empty() {
                    ensure_room(&mut pages, &mut y, TABLE_FONT_SIZE);
                    for (i, h) in headers.iter().enumerate() {
                        let text = truncate_to_width(h, TABLE_FONT_SIZE, col_width);
                        pages.last_mut().unwrap().push(DrawOp {
                            x: MARGIN + i as f32 * col_width,
                            y,
                            bold: true,
                            size: TABLE_FONT_SIZE,
                            text,
                        });
                    }
                    y -= TABLE_FONT_SIZE * LINE_HEIGHT_FACTOR;
                }
                for row in rows {
                    ensure_room(&mut pages, &mut y, TABLE_FONT_SIZE);
                    for (i, cell) in row.iter().enumerate() {
                        let text = truncate_to_width(cell, TABLE_FONT_SIZE, col_width);
                        pages.last_mut().unwrap().push(DrawOp {
                            x: MARGIN + i as f32 * col_width,
                            y,
                            bold: false,
                            size: TABLE_FONT_SIZE,
                            text,
                        });
                    }
                    y -= TABLE_FONT_SIZE * LINE_HEIGHT_FACTOR;
                }
            }
        }
    }
    pages
}

fn render(pages: &[Vec<DrawOp>]) -> Vec<u8> {
    let mut next_id: i32 = 0;
    let mut alloc = || {
        next_id += 1;
        Ref::new(next_id)
    };

    let catalog_id = alloc();
    let page_tree_id = alloc();
    let font_regular_id = alloc();
    let font_bold_id = alloc();

    let mut pdf = Pdf::new();
    pdf.catalog(catalog_id).pages(page_tree_id);
    pdf.type1_font(font_regular_id).base_font(Name(b"Helvetica")).encoding_predefined(Name(b"WinAnsiEncoding"));
    pdf.type1_font(font_bold_id).base_font(Name(b"Helvetica-Bold")).encoding_predefined(Name(b"WinAnsiEncoding"));

    let mut page_ids = Vec::new();
    for ops in pages {
        let mut content = Content::new();
        for op in ops {
            let font_name = if op.bold { Name(b"F2") } else { Name(b"F1") };
            let bytes = encode_winansi(&op.text);
            content.begin_text();
            content.set_font(font_name, op.size);
            content.next_line(op.x, op.y);
            content.show(Str(&bytes));
            content.end_text();
        }
        let content_id = alloc();
        pdf.stream(content_id, &content.finish());

        let page_id = alloc();
        page_ids.push(page_id);
        let mut page = pdf.page(page_id);
        page.media_box(Rect::new(0.0, 0.0, PAGE_WIDTH, PAGE_HEIGHT));
        page.parent(page_tree_id);
        page.contents(content_id);
        page.resources().fonts().pair(Name(b"F1"), font_regular_id).pair(Name(b"F2"), font_bold_id);
        page.finish();
    }

    pdf.pages(page_tree_id).kids(page_ids.iter().copied()).count(page_ids.len() as i32);

    pdf.finish()
}

/// Punto de entrada de `pdf.build(blocks: PdfBlock[]) -> String`
/// (`call_method`, `Value::Pdf`). Devuelve los bytes crudos del PDF -- el
/// caller los codifica a base64, mismo criterio que cualquier adjunto
/// binario en este proyecto (GRAMMAR.md §3.141).
pub fn build(blocks: &[PdfBlockSpec]) -> Result<Vec<u8>, String> {
    for block in blocks {
        if let PdfBlockSpec::Table { headers, rows } = block {
            let expected = if !headers.is_empty() {
                headers.len()
            } else {
                rows.first().map(|r| r.len()).unwrap_or(0)
            };
            for (i, row) in rows.iter().enumerate() {
                if row.len() != expected {
                    return Err(format!(
                        "pdf.build: la fila {i} de una tabla tiene {} columna(s), se esperaban {expected}",
                        row.len()
                    ));
                }
            }
        }
    }
    let pages = layout(blocks);
    Ok(render(&pages))
}
