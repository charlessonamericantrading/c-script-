use crate::ast::{self, Item, Program, TypeExpr};
use crate::checker::Checker;
use crate::codegen::ts_emit::render_type;
use crate::lexer;
use crate::modules;
use crate::parser;
use crate::token::Span;
use crate::types::Type;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};

pub struct LspServer {
    documents: HashMap<String, String>,
}

/// El `Program` fusionado de un archivo + su cierre transitivo de imports,
/// junto con la identidad de archivo que hace falta para no arriesgar una
/// posición en el archivo equivocado (GRAMMAR.md §3.21) -- ver
/// `LspServer::full_program_loaded`.
struct LoadedProgram {
    program: Program,
    /// Mismo largo y orden que `program.items` -- `item_files[i]` es el
    /// archivo canonicalizado del que vino `program.items[i]`. Ver
    /// `modules::load_program_with_overlay` para el porqué de esta forma.
    item_files: Vec<PathBuf>,
}

impl Default for LspServer {
    fn default() -> Self {
        Self::new()
    }
}

impl LspServer {
    pub fn new() -> Self {
        Self {
            documents: HashMap::new(),
        }
    }

    /// Overlay en memoria de TODOS los documentos abiertos (no solo el que
    /// disparó el request -- un archivo importado puede estar abierto en
    /// otra pestaña), con la ruta canonicalizada como clave -- misma
    /// convención que `modules.rs` usa internamente, para que
    /// `load_program_with_overlay` encuentre el buffer editado en vez de
    /// caer al contenido (posiblemente desactualizado) de disco.
    fn build_overlay(&self) -> HashMap<PathBuf, String> {
        let mut overlay = HashMap::new();
        for (uri, text) in &self.documents {
            if let Some(path) = uri_to_path(uri) {
                if let Ok(canon) = std::fs::canonicalize(&path) {
                    overlay.insert(canon, text.clone());
                }
            }
        }
        overlay
    }

    /// El `Program` fusionado (siguiendo imports de verdad) para el
    /// archivo en `uri`, si `uri` corresponde a un archivo real en disco.
    /// `None` si no (URI que no es `file://`, un buffer nunca guardado, o
    /// un error de carga/sintaxis en el cierre transitivo) -- los
    /// callers deben caer al chequeo aislado del buffer solo en ese caso.
    ///
    /// Envuelto en `catch_unwind` (igual que `compute_diagnostics_for` --
    /// ver ese comentario para el razonamiento completo): un panic acá NO
    /// debe tirar abajo el proceso entero de `linkc lsp`, solo degradar
    /// ESTE request puntual a "no hay programa completo disponible" (los
    /// callers -- hover/completion/goto-def -- ya saben caer al buffer
    /// aislado cuando esto da `None`, mismo camino que un archivo sin
    /// imports).
    fn full_program_for(&self, uri: &str) -> Option<Program> {
        self.full_program_loaded(uri).map(|lp| lp.program)
    }

    /// Igual que `full_program_for`, pero además expone, por ítem, de qué
    /// archivo real vino cada uno (`item_files` -- GRAMMAR.md §3.21, "Not
    /// done yet", resuelto en esta ronda vía
    /// `modules::load_program_with_overlay`). Antes, con más de un archivo
    /// en el cierre transitivo, goto-definición se negaba en bloque porque
    /// un `Span` no decía de qué archivo venía -- ahora `item_files`
    /// alcanza para resolver a qué archivo real apuntar sin arriesgar una
    /// posición en el archivo equivocado.
    fn full_program_loaded(&self, uri: &str) -> Option<LoadedProgram> {
        match std::panic::catch_unwind(|| self.full_program_for_inner(uri)) {
            Ok(result) => result,
            Err(_) => {
                eprintln!(
                    "linkc lsp: panic interno al cargar el programa completo de '{uri}' -- degradando a chequeo aislado del buffer, el servidor sigue corriendo"
                );
                None
            }
        }
    }

    fn full_program_for_inner(&self, uri: &str) -> Option<LoadedProgram> {
        let entry_path = uri_to_path(uri)?;
        let entry_canon = std::fs::canonicalize(&entry_path).ok()?;
        let overlay = self.build_overlay();
        modules::load_program_with_overlay(&entry_canon, &overlay)
            .ok()
            .map(|(program, _touched, item_files)| LoadedProgram { program, item_files })
    }

    /// Diagnósticos para `uri`, soportando `import` de verdad. Si `uri` no
    /// tiene un archivo real en disco (fuera de alcance en v0 -- ver
    /// GRAMMAR.md, protocolo LSP), cae al chequeo aislado del buffer solo.
    ///
    /// Envuelto en `catch_unwind`: `linkc lsp` es un proceso de LARGA VIDA
    /// que corre en un solo hilo (`run_stdio`) -- un panic sin capturar acá
    /// (dentro de `load_program_with_overlay`/`check_program_full`, hoy sin
    /// ningún panic conocido alcanzable desde texto inválido, pero un
    /// checker que sigue creciendo puede introducir uno) se propagaría a
    /// través de `handle_message` y terminaría el proceso entero -- un
    /// documento roto matando el servidor para TODOS los documentos
    /// abiertos, no solo el problemático. `&LspServer` no tiene mutabilidad
    /// interior (`documents` es un `HashMap` liso), así que es
    /// `UnwindSafe` sin necesitar `AssertUnwindSafe`.
    fn compute_diagnostics_for(&self, uri: &str) -> Vec<Value> {
        match std::panic::catch_unwind(|| self.compute_diagnostics_for_inner(uri)) {
            Ok(diags) => diags,
            Err(_) => {
                eprintln!(
                    "linkc lsp: panic interno al re-chequear '{uri}' -- ver el mensaje de panic arriba para el detalle; el servidor sigue corriendo"
                );
                vec![zero_diagnostic(format!(
                    "error interno del servidor LSP al chequear este documento -- ver stderr del proceso 'linkc lsp' para el detalle ({uri})"
                ))]
            }
        }
    }

    fn compute_diagnostics_for_inner(&self, uri: &str) -> Vec<Value> {
        let standalone = || compute_diagnostics_standalone(self.documents.get(uri).map(String::as_str).unwrap_or(""));

        let Some(entry_path) = uri_to_path(uri) else {
            return standalone();
        };
        let Ok(entry_canon) = std::fs::canonicalize(&entry_path) else {
            return standalone();
        };
        let overlay = self.build_overlay();

        match modules::load_program_with_overlay(&entry_canon, &overlay) {
            Err(modules::LoadError::Syntax { path, errors }) => {
                let source = overlay
                    .get(&path)
                    .cloned()
                    .or_else(|| std::fs::read_to_string(&path).ok())
                    .unwrap_or_default();
                errors
                    .into_iter()
                    .map(|(span, message)| {
                        // Un error de sintaxis en un archivo IMPORTADO ya
                        // tiene identidad de archivo real (a diferencia de
                        // un CheckError, ver abajo) -- se nombra en el
                        // mensaje aunque el rango se dibuje sobre ESTE
                        // documento (`uri`), porque `path` puede no ser el
                        // documento actualmente abierto.
                        let message =
                            if path == entry_canon { message } else { format!("(en '{}') {message}", modules::display_path(&path)) };
                        json!({
                            "range": span_to_range(&source, span),
                            "severity": 1,
                            "source": "c-script",
                            "message": message,
                        })
                    })
                    .collect()
            }
            Err(modules::LoadError::Other(message)) => vec![zero_diagnostic(message)],
            Ok((program, _touched, item_files)) => {
                let (_, errors) = Checker::check_program_full(&program, &item_files);
                // Antes de esta ronda (GRAMMAR.md §3.21, "Not done yet"),
                // un CheckError no tenía identidad de archivo tras el
                // merge, así que CUALQUIER programa con más de un archivo
                // degradaba TODOS sus errores a una posición cero -- aunque
                // el 100% de ellos estuviera en el documento abierto. Ahora
                // `e.file` (estampado por `check_program_full` a partir de
                // `item_files`) dice de qué archivo real vino cada error
                // INDIVIDUAL: si coincide con `entry_canon` (el documento
                // que disparó este chequeo), el span se convierte con
                // precisión total sobre SU contenido; si no (vino de un
                // `import`), el protocolo LSP no da forma de apuntar una
                // posición de OTRO archivo dentro de la respuesta de
                // `uri` -- se nombra el archivo real en el mensaje (mismo
                // criterio que `LoadError::Syntax` ya usa arriba) en vez
                // de esconder cuál de los N archivos importados era.
                let source = self.documents.get(uri).cloned().unwrap_or_default();
                errors
                    .into_iter()
                    .map(|e| {
                        let in_open_doc = e.file.as_deref() == Some(entry_canon.as_path());
                        let range = match (in_open_doc, e.span) {
                            (true, Some(span)) => span_to_range(&source, span),
                            _ => zero_range(),
                        };
                        let message = match (&e.file, in_open_doc) {
                            (Some(file), false) => format!("(en '{}') {}", modules::display_path(file), e.message),
                            _ => e.message,
                        };
                        json!({ "range": range, "severity": 1, "source": "c-script", "message": message })
                    })
                    .collect()
            }
        }
    }

    /// Inicia el bucle principal de comunicación JSON-RPC 2.0 sobre stdin/stdout.
    pub fn run_stdio(&mut self) -> io::Result<()> {
        let stdin = io::stdin();
        let mut handle = stdin.lock();
        let mut stdout = io::stdout();

        loop {
            let mut content_length: Option<usize> = None;
            loop {
                let mut header_line = String::new();
                if handle.read_line(&mut header_line)? == 0 {
                    return Ok(()); // EOF
                }
                let trimmed = header_line.trim();
                if trimmed.is_empty() {
                    break;
                }
                if let Some((key, val)) = trimmed.split_once(':') {
                    if key.trim().eq_ignore_ascii_case("content-length") {
                        if let Ok(len) = val.trim().parse::<usize>() {
                            content_length = Some(len);
                        }
                    }
                }
            }

            // Bug real encontrado en un reparso (no en uso real -- ver
            // GRAMMAR.md §3.19 para la historia): un `continue` acá volvía
            // al tope del loop SIN leer los bytes del body del mensaje mal
            // formado, que seguían sin consumir en el stream. La próxima
            // vuelta del loop de headers los interpretaba como si fueran
            // líneas de header (nunca lo son, así que nunca encuentra un
            // Content-Length válido) -- un desync PERMANENTE y silencioso:
            // el server dejaba de responder a TODO lo que viniera después,
            // sin ningún error visible, indistinguible de un server
            // colgado desde el lado del cliente/editor. No hay forma
            // confiable de "resincronizar" sin saber cuántos bytes saltar
            // -- ese largo es exactamente el dato que falta o es inválido
            // -- así que la conexión termina con un error real en vez de
            // fingir que puede seguir. `cmd_lsp` (main.rs) ya traduce este
            // `Err` a un mensaje en stderr + código de salida distinto de
            // cero.
            let Some(len) = content_length else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "framing LSP corrupto: encabezado Content-Length faltante o no numérico -- sin ese largo no hay forma confiable de saber dónde empieza el próximo mensaje",
                ));
            };

            let mut body_buf = vec![0u8; len];
            handle.read_exact(&mut body_buf)?;

            let req: Value = match serde_json::from_slice(&body_buf) {
                Ok(val) => val,
                Err(_) => continue,
            };

            if let Some(resp) = self.handle_message(&req) {
                send_payload(&mut stdout, &resp)?;
            }
        }
    }

    /// Procesa un mensaje JSON-RPC entrante y devuelve la respuesta (si corresponde).
    pub fn handle_message(&mut self, req: &Value) -> Option<Value> {
        let id = req.get("id");
        let Some(method) = req.get("method").and_then(|m| m.as_str()) else {
            // Gap real encontrado en un reparso: esto antes era
            // `req.get("method")?.as_str()?`, que devolvía `None` sin
            // distinguir "es una notificación, no hay nada que
            // responder" de "es un REQUEST (tiene 'id') que un cliente
            // real está esperando responder". A diferencia del bug de
            // framing de `run_stdio` (que desincroniza la conexión
            // entera), esto no rompe nada más allá de ESTE id puntual --
            // pero silencio ahí sigue siendo un cliente esperando para
            // siempre una respuesta que nunca llega. JSON-RPC 2.0 pide
            // un error explícito para un request inválido, no silencio.
            return id.cloned().map(|id_val| {
                json!({
                    "jsonrpc": "2.0",
                    "id": id_val,
                    "error": { "code": -32600, "message": "Invalid Request: falta 'method' o no es un string" }
                })
            });
        };

        match method {
            "initialize" => {
                let id_val = id?.clone();
                Some(json!({
                    "jsonrpc": "2.0",
                    "id": id_val,
                    "result": {
                        "capabilities": {
                            "textDocumentSync": 1, // Full Sync
                            "hoverProvider": true,
                            "completionProvider": {
                                "triggerCharacters": [".", ":", "@", " "]
                            },
                            "definitionProvider": true,
                            "documentFormattingProvider": true
                        },
                        "serverInfo": {
                            "name": "linkc-lsp",
                            "version": "0.1.0"
                        }
                    }
                }))
            }
            "initialized" => None,
            "shutdown" => {
                let id_val = id?.clone();
                Some(json!({
                    "jsonrpc": "2.0",
                    "id": id_val,
                    "result": serde_json::Value::Null
                }))
            }
            "exit" => None,
            "textDocument/didOpen" => {
                let params = req.get("params")?;
                let doc = params.get("textDocument")?;
                let uri = doc.get("uri")?.as_str()?.to_string();
                let text = doc.get("text")?.as_str()?.to_string();

                self.documents.insert(uri.clone(), text);
                let diagnostics = self.compute_diagnostics_for(&uri);

                Some(json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/publishDiagnostics",
                    "params": {
                        "uri": uri,
                        "diagnostics": diagnostics
                    }
                }))
            }
            "textDocument/didChange" => {
                let params = req.get("params")?;
                let uri = params.get("textDocument")?.get("uri")?.as_str()?.to_string();
                let changes = params.get("contentChanges")?.as_array()?;
                if let Some(first_change) = changes.first() {
                    if let Some(text) = first_change.get("text").and_then(|t| t.as_str()) {
                        self.documents.insert(uri.clone(), text.to_string());
                        let diagnostics = self.compute_diagnostics_for(&uri);

                        return Some(json!({
                            "jsonrpc": "2.0",
                            "method": "textDocument/publishDiagnostics",
                            "params": {
                                "uri": uri,
                                "diagnostics": diagnostics
                            }
                        }));
                    }
                }
                None
            }
            "textDocument/didSave" => {
                let params = req.get("params")?;
                let uri = params.get("textDocument")?.get("uri")?.as_str()?.to_string();
                if self.documents.contains_key(&uri) {
                    let diagnostics = self.compute_diagnostics_for(&uri);
                    return Some(json!({
                        "jsonrpc": "2.0",
                        "method": "textDocument/publishDiagnostics",
                        "params": {
                            "uri": uri,
                            "diagnostics": diagnostics
                        }
                    }));
                }
                None
            }
            "textDocument/didClose" => {
                if let Some(params) = req.get("params") {
                    if let Some(uri) = params.get("textDocument").and_then(|d| d.get("uri")).and_then(|u| u.as_str()) {
                        self.documents.remove(uri);
                    }
                }
                None
            }
            "textDocument/hover" => {
                let id_val = id?.clone();
                let params = req.get("params")?;
                let uri = params.get("textDocument")?.get("uri")?.as_str()?;
                let pos = params.get("position")?;
                let line = pos.get("line")?.as_u64()? as usize;
                let character = pos.get("character")?.as_u64()? as usize;

                let full_program = self.full_program_for(uri);
                let result = if let Some(source) = self.documents.get(uri) {
                    get_hover(source, line, character, full_program.as_ref()).unwrap_or(serde_json::Value::Null)
                } else {
                    serde_json::Value::Null
                };

                Some(json!({
                    "jsonrpc": "2.0",
                    "id": id_val,
                    "result": result
                }))
            }
            "textDocument/completion" => {
                let id_val = id?.clone();
                let params = req.get("params")?;
                let uri = params.get("textDocument")?.get("uri")?.as_str()?;
                let pos = params.get("position")?;
                let line = pos.get("line")?.as_u64()? as usize;
                let character = pos.get("character")?.as_u64()? as usize;

                let full_program = self.full_program_for(uri);
                let items = if let Some(source) = self.documents.get(uri) {
                    get_completions(source, line, character, full_program.as_ref())
                } else {
                    Vec::new()
                };

                Some(json!({
                    "jsonrpc": "2.0",
                    "id": id_val,
                    "result": items
                }))
            }
            "textDocument/definition" => {
                let id_val = id?.clone();
                let params = req.get("params")?;
                let uri = params.get("textDocument")?.get("uri")?.as_str()?;
                let pos = params.get("position")?;
                let line = pos.get("line")?.as_u64()? as usize;
                let character = pos.get("character")?.as_u64()? as usize;

                let loaded = self.full_program_loaded(uri);
                let full_program = loaded.as_ref().map(|lp| &lp.program);
                let no_files: Vec<PathBuf> = Vec::new();
                let item_files = loaded.as_ref().map(|lp| &lp.item_files).unwrap_or(&no_files);
                let overlay = self.build_overlay();
                let loc = if let Some(source) = self.documents.get(uri) {
                    get_definition(uri, source, line, character, full_program, item_files, &overlay).unwrap_or(serde_json::Value::Null)
                } else {
                    serde_json::Value::Null
                };

                Some(json!({
                    "jsonrpc": "2.0",
                    "id": id_val,
                    "result": loc
                }))
            }
            "textDocument/formatting" => {
                let id_val = id?.clone();
                let params = req.get("params")?;
                let uri = params.get("textDocument")?.get("uri")?.as_str()?;
                let edits = if let Some(source) = self.documents.get(uri) {
                    match crate::fmt::format_source(source) {
                        Ok(formatted) => {
                            let line_count = source.lines().count();
                            json!([{
                                "range": {
                                    "start": { "line": 0, "character": 0 },
                                    "end": { "line": line_count + 1, "character": 0 }
                                },
                                "newText": formatted
                            }])
                        }
                        Err(_) => json!([]),
                    }
                } else {
                    json!([])
                };

                Some(json!({
                    "jsonrpc": "2.0",
                    "id": id_val,
                    "result": edits
                }))
            }
            _ => None,
        }
    }
}

pub fn send_payload<W: Write>(writer: &mut W, payload: &Value) -> io::Result<()> {
    let body = serde_json::to_string(payload)?;
    write!(writer, "Content-Length: {}\r\n\r\n{}", body.len(), body)?;
    writer.flush()
}

/// Convierte un URI `file://...` a una ruta real del sistema de archivos.
/// `None` para cualquier otro scheme (ej. `untitled:`, sin archivo real en
/// disco -- fuera de alcance en v0). Decodifica percent-encoding (un
/// espacio en el nombre de una carpeta ya viene como `%20`) y, en Windows,
/// saca la barra de más que un URI `file:///C:/...` trae antes de la letra
/// de unidad (si no, `PathBuf::from` vería `/C:/...`, no `C:/...`).
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let decoded = percent_decode(rest);
    let trimmed = if decoded.len() >= 3 && decoded.as_bytes()[0] == b'/' && decoded.as_bytes()[2] == b':' {
        decoded[1..].to_string()
    } else {
        decoded
    };
    Some(PathBuf::from(trimmed))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Inversa de `uri_to_path` -- antepone `file://` sin duplicar la barra
/// inicial (Unix ya la trae; en Windows se agrega una antes de la letra de
/// unidad, dando la forma estándar `file:///C:/...`).
pub fn path_to_uri(path: &Path) -> String {
    let mut s = path.to_string_lossy().replace('\\', "/");
    if !s.starts_with('/') {
        s = format!("/{s}");
    }
    format!("file://{s}")
}

/// Char-offset (de `span.start`/`.end`) -> `Position` de LSP (línea
/// 0-indexada, columna en UNIDADES UTF-16 -- lo que pide el wire de LSP,
/// no un conteo de chars crudo). Cuenta saltos de línea REALES desde el
/// principio de `chars` hasta `idx`, así que a diferencia de
/// `diagnostics::render_diagnostic` (que solo ve una línea ya extraída y
/// asume que el span nunca la cruza) esto da la línea de fin correcta
/// para un span multi-línea de verdad -- rutina para `TypeDecl`/`FnDecl`/
/// `ServiceDecl` con más de un campo/parámetro.
fn utf16_position(chars: &[char], idx: usize) -> (u64, u64) {
    let mut line = 0u64;
    let mut col_units = 0u64;
    for &c in &chars[..idx] {
        if c == '\n' {
            line += 1;
            col_units = 0;
        } else {
            col_units += c.len_utf16() as u64;
        }
    }
    (line, col_units)
}

/// Inversa de `utf16_position`: línea (0-indexada) + columna en unidades
/// UTF-16 (lo que el wire de LSP manda, `Position.line`/`.character`) ->
/// offset de char absoluto en `source`. Hace falta para comparar la
/// posición del cursor contra un `Span` (offsets de char, ver token.rs) --
/// a diferencia de `get_word_at_pos`/`get_line_prefix_at_pos`, que operan
/// sobre una sola línea aislada y nunca calculan un offset del archivo
/// completo. Si `target_col` cae más allá del fin real de la línea, o
/// `target_line` más allá del fin del archivo, clampea al límite más
/// cercano en vez de panicar -- mismo criterio de "defensa en profundidad,
/// no comportamiento esperado" que ya usa `render_diagnostic`.
fn char_offset_from_utf16_position(source: &str, target_line: usize, target_col: usize) -> usize {
    let chars: Vec<char> = source.chars().collect();
    let mut line = 0usize;
    let mut col_units = 0usize;
    for (idx, &c) in chars.iter().enumerate() {
        if line == target_line && col_units >= target_col {
            return idx;
        }
        if c == '\n' {
            if line == target_line {
                return idx; // la columna pedida cae más allá del fin de esta línea
            }
            line += 1;
            col_units = 0;
        } else {
            col_units += c.len_utf16();
        }
    }
    chars.len()
}

/// Convierte un `Span` (offsets de caracteres del lexer, sin ninguna
/// posición de fin propia) a un `Range` de LSP con inicio Y FIN reales,
/// usando el texto COMPLETO de `source` -- ver `utf16_position`.
pub fn span_to_range(source: &str, span: Span) -> Value {
    let chars: Vec<char> = source.chars().collect();
    let start = span.start.min(chars.len());
    let end = span.end.min(chars.len()).max(start);

    let (start_line, start_char) = utf16_position(&chars, start);
    let (end_line, end_char) = utf16_position(&chars, end);

    json!({
        "start": { "line": start_line, "character": start_char },
        "end": { "line": end_line, "character": end_char }
    })
}

fn zero_range() -> Value {
    json!({ "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } })
}

fn zero_diagnostic(message: String) -> Value {
    json!({ "range": zero_range(), "severity": 1, "source": "c-script", "message": message })
}

/// Chequeo aislado del buffer solo, sin resolver ningún `import` -- lo que
/// `compute_diagnostics_for` usaba SIEMPRE antes de esta ronda, y lo que
/// sigue usando como fallback cuando `uri` no tiene un archivo real en
/// disco (ver `compute_diagnostics_for`).
pub fn compute_diagnostics_standalone(source: &str) -> Vec<Value> {
    let tokens = match lexer::tokenize(source) {
        Ok(t) => t,
        Err(err) => {
            return vec![json!({
                "range": span_to_range(source, err.span),
                "severity": 1,
                "source": "c-script",
                "message": err.message,
            })];
        }
    };

    let program = match parser::parse(tokens) {
        Ok(p) => p,
        Err(parse_errors) => {
            return parse_errors
                .into_iter()
                .map(|e| {
                    json!({
                        "range": span_to_range(source, e.span),
                        "severity": 1,
                        "source": "c-script",
                        "message": e.message,
                    })
                })
                .collect();
        }
    };

    let mut diags = Vec::new();
    if let Err(check_errors) = Checker::check_program(&program) {
        for err in check_errors {
            let range = match err.span {
                Some(span) => span_to_range(source, span),
                None => zero_range(),
            };
            diags.push(json!({
                "range": range,
                "severity": 1,
                "source": "c-script",
                "message": err.message,
            }));
        }
    }

    diags
}

pub fn get_word_at_pos(source: &str, line0: usize, col0: usize) -> Option<String> {
    let line_str = source.lines().nth(line0)?;
    let chars: Vec<char> = line_str.chars().collect();
    if col0 >= chars.len() {
        return None;
    }
    let is_word_char = |c: char| c.is_alphanumeric() || c == '_';
    if !is_word_char(chars[col0]) {
        return None;
    }
    let mut start = col0;
    while start > 0 && is_word_char(chars[start - 1]) {
        start -= 1;
    }
    let mut end = col0;
    while end < chars.len() && is_word_char(chars[end]) {
        end += 1;
    }
    Some(chars[start..end].iter().collect())
}

pub fn get_line_prefix_at_pos(source: &str, line0: usize, col0: usize) -> String {
    if let Some(line_str) = source.lines().nth(line0) {
        let chars: Vec<char> = line_str.chars().collect();
        let end = col0.min(chars.len());
        chars[..end].iter().collect()
    } else {
        String::new()
    }
}

/// Resuelve el `Program` a inspeccionar: el fusionado-con-imports si
/// `full_program` vino de una carga real en disco, si no cae a
/// tokenize+parse aislado del buffer (mismo comportamiento que antes de
/// esta ronda -- necesario para los tests que usan URIs ficticios y para
/// un buffer sin archivo real en disco todavía).
fn resolve_program<'a>(source: &str, full_program: Option<&'a Program>, owned: &'a mut Option<Program>) -> Option<&'a Program> {
    if let Some(p) = full_program {
        return Some(p);
    }
    *owned = lexer::tokenize(source).ok().and_then(|toks| parser::parse(toks).ok());
    owned.as_ref()
}

pub fn get_hover(source: &str, line0: usize, col0: usize, full_program: Option<&Program>) -> Option<Value> {
    if let Some(word) = get_word_at_pos(source, line0, col0) {
        if let Some(hover) = get_hover_for_word(&word, source, full_program) {
            return Some(hover);
        }
    }

    // Nivel 3, ronda 2/3 (GRAMMAR.md §3.24): hover de una expresión
    // arbitraria dentro de un body -- a diferencia de todo lo de arriba
    // (nombres de declaración, palabras clave), esto NO depende de estar
    // sobre un identificador (`get_word_at_pos` no engancha operadores ni
    // literales) -- por eso corre INCONDICIONALMENTE acá abajo, no dentro
    // del `if let Some(word)`. `hover_type_at` hace todo el trabajo real
    // (reusa `check_fn`/`check_rpc` tal cual); acá solo se resuelve el
    // offset y se renderiza el `Type` encontrado con el mismo `render_type`
    // que ya usa el emisor del contrato real (`ts_emit.rs`) -- mismo
    // criterio en los dos lugares para lo que un tipo "se ve" en TS.
    let mut owned = None;
    let program = resolve_program(source, full_program, &mut owned)?;
    let offset = char_offset_from_utf16_position(source, line0, col0);
    let ty = Checker::hover_type_at(program, offset)?;
    Some(json!({
        "contents": {
            "kind": "markdown",
            "value": format!("```typescript\n{}\n```", render_type(&ty))
        }
    }))
}

/// Nivel 1/2 del LSP (palabras clave, builtins, hover a nivel de
/// declaración) -- extraído de `get_hover` sin cambiar NINGÚN
/// comportamiento existente, solo para poder correr el hover de Nivel 3
/// (expresión arbitraria) incluso cuando `get_word_at_pos` no engancha
/// nada (operadores, literales) sin duplicar esta lógica.
fn get_hover_for_word(word: &str, source: &str, full_program: Option<&Program>) -> Option<Value> {
    let builtin_hover = match word {
        "service" => Some("Keyword `service`\n\nDefines a service exposing RPC and stream endpoints."),
        "rpc" => Some("Keyword `rpc`\n\nDefines a Remote Procedure Call endpoint."),
        "stream" => Some("Keyword `stream`\n\nDefines a Server-Sent Events (SSE) streaming endpoint."),
        "type" => Some("Keyword `type`\n\nDefines a structural type/record."),
        "enum" => Some("Keyword `enum`\n\nDefines an enumeration or Algebraic Data Type (ADT)."),
        "db" => Some("Keyword `db`\n\nDefines persistent database collections."),
        "match" => Some("Keyword `match`\n\nPattern matching expression."),
        "fn" => Some("Keyword `fn`\n\nDefines a function."),
        "const" => Some("Keyword `const`\n\nDefines a compile-time constant."),
        "let" => Some("Keyword `let`\n\nBinds a variable in local scope."),
        "mut" => Some("Keyword `mut`\n\nMarks a variable binding as mutable."),
        "while" => Some("Keyword `while`\n\nLoop construct."),
        "if" => Some("Keyword `if`\n\nConditional branch."),
        "else" => Some("Keyword `else`\n\nConditional fallback branch."),
        "import" => Some("Keyword `import`\n\nImports symbols from another module."),
        "from" => Some("Keyword `from`\n\nSpecifies the source module for imports."),
        "pub" => Some("Keyword `pub`\n\nExposes a module item."),
        "Int" => Some("Builtin Type `Int`\n\n64-bit signed integer."),
        "Int64" => Some("Builtin Type `Int64`\n\nSame 64-bit range as `Int`, but serialized as a string on the wire (and typed as `string` in TS) to avoid precision loss above 2^53. Convert with `.toInt64()`/`.toInt()`."),
        "Timestamp" => Some("Builtin Type `Timestamp`\n\nUTC instant, serialized as a fixed-shape ISO-8601 string (`YYYY-MM-DDTHH:mm:ss.sssZ`) on the wire and typed as `string` in TS. Comparable (`< <= > >= == !=`) but no arithmetic; not constructible from source in v0 (arrives as an rpc param or from `db`)."),
        "Float" => Some("Builtin Type `Float`\n\n64-bit floating point number."),
        "String" => Some("Builtin Type `String`\n\nUTF-8 string."),
        "Bool" => Some("Builtin Type `Bool`\n\nBoolean type (`true` or `false`)."),
        "Void" => Some("Builtin Type `Void`\n\nEmpty return type for RPCs."),
        "Result" => Some("Builtin Type `Result<T, E>`\n\nResult of an operation (`Result.Ok` or `Result.Err`)."),
        "Patch" => Some("Builtin Type `Patch<T>`\n\nPartial update shape for type `T`."),
        "authenticated" => Some("Annotation `@authenticated`\n\nRequires a valid authenticated session."),
        "requires" => Some("Annotation `@requires(Role)`\n\nRequires a session with the specified Role."),
        _ => None,
    };

    if let Some(hover_str) = builtin_hover {
        return Some(json!({
            "contents": {
                "kind": "markdown",
                "value": format!("```c-script\n{}\n```", hover_str)
            }
        }));
    }

    let mut owned = None;
    let program = resolve_program(source, full_program, &mut owned)?;

    for item in &program.items {
        match item {
            Item::Type(t) if t.name == word => {
                let params = if t.type_params.is_empty() {
                    "".to_string()
                } else {
                    format!("<{}>", t.type_params.join(", "))
                };
                return Some(json!({
                    "contents": {
                        "kind": "markdown",
                        "value": format!("```c-script\ntype {}{}\n```", t.name, params)
                    }
                }));
            }
            Item::Enum(e) if e.name == word => {
                let variant_names: Vec<String> = e.variants.iter().map(|v| v.name.clone()).collect();
                return Some(json!({
                    "contents": {
                        "kind": "markdown",
                        "value": format!("```c-script\nenum {} {{ {} }}\n```", e.name, variant_names.join(", "))
                    }
                }));
            }
            Item::Service(s) => {
                if s.name == word {
                    return Some(json!({
                        "contents": {
                            "kind": "markdown",
                            "value": format!("```c-script\nservice {}\n```", s.name)
                        }
                    }));
                }
                for member in &s.members {
                    let rpc = match member {
                        ast::Member::Rpc(r) | ast::Member::Stream(r) => r,
                    };
                    if rpc.name == word {
                        let kind = match member {
                            ast::Member::Rpc(_) => "rpc",
                            ast::Member::Stream(_) => "stream",
                        };
                        let params_str = rpc
                            .params
                            .iter()
                            .map(|p| format!("{}: {:?}", p.name, p.ty))
                            .collect::<Vec<_>>()
                            .join(", ");
                        return Some(json!({
                            "contents": {
                                "kind": "markdown",
                                "value": format!("```c-script\n{} {}({}) -> {:?}\n```", kind, rpc.name, params_str, rpc.return_type)
                            }
                        }));
                    }
                }
            }
            Item::Fn(f) if f.name == word => {
                let params_str = f
                    .params
                    .iter()
                    .map(|p| format!("{}: {:?}", p.name, p.ty))
                    .collect::<Vec<_>>()
                    .join(", ");
                return Some(json!({
                    "contents": {
                        "kind": "markdown",
                        "value": format!("```c-script\nfn {}({}) -> {:?}\n```", f.name, params_str, f.return_type)
                    }
                }));
            }

            Item::Const(c) if c.name == word => {
                return Some(json!({
                    "contents": {
                        "kind": "markdown",
                        "value": format!("```c-script\nconst {}\n```", c.name)
                    }
                }));
            }
            Item::Db(db) => {
                for col in &db.collections {
                    if col.name == word {
                        return Some(json!({
                            "contents": {
                                "kind": "markdown",
                                "value": format!("```c-script\ndb.{}\n```", col.name)
                            }
                        }));
                    }
                }
            }
            _ => {}
        }
    }

    None
}

/// Busca el `TypeExpr::Named` cuyo span cubre `offset`, recorriendo
/// exhaustivamente las 8 variantes de `TypeExpr` (sin brazo `_`, a
/// propósito -- una variante nueva de `TypeExpr` rompe la compilación acá
/// en vez de que esta búsqueda la ignore en silencio). Mira primero los
/// `args` de un genérico, para que el cursor en `Line` dentro de
/// `List<Line>` resuelva a `Line`, nunca al `List` que lo envuelve.
fn find_named_type_at(texpr: &TypeExpr, offset: usize) -> Option<(String, Span)> {
    match texpr {
        TypeExpr::Named(name, args, span) => {
            for arg in args {
                if let Some(found) = find_named_type_at(arg, offset) {
                    return Some(found);
                }
            }
            if offset >= span.start && offset < span.end {
                Some((name.clone(), *span))
            } else {
                None
            }
        }
        TypeExpr::Struct(fields) => fields.iter().find_map(|f| find_named_type_at(&f.ty, offset)),
        TypeExpr::Map(k, v) => find_named_type_at(k, offset).or_else(|| find_named_type_at(v, offset)),
        TypeExpr::Tuple(items) => items.iter().find_map(|t| find_named_type_at(t, offset)),
        TypeExpr::Function(params, ret) => {
            params.iter().find_map(|p| find_named_type_at(p, offset)).or_else(|| find_named_type_at(ret, offset))
        }
        TypeExpr::Optional(inner) | TypeExpr::List(inner) => find_named_type_at(inner, offset),
        TypeExpr::Union(members) => members.iter().find_map(|m| find_named_type_at(m, offset)),
    }
}

/// Aplica `find_named_type_at` sobre todas las firmas del programa --
/// `Field`s de `type`/`db`/variantes de `enum`, `Param`s + `return_type`
/// de `fn`/`rpc`/`stream`, y el `ty` de un `const`. Exactamente los mismos
/// spans de FIRMA que `FnDecl.span`/`RpcDecl.span` ya cubren hoy (firma
/// completa, nunca el body) -- por eso esta búsqueda nunca se solapa con
/// hover/completion de Nivel 2 (que solo miran nombres de declaración) ni
/// con una futura búsqueda dentro de un body (Nivel 3, ítems 1/2,
/// GRAMMAR.md §3.19).
fn find_named_type_in_program(program: &Program, offset: usize) -> Option<(String, Span)> {
    for item in &program.items {
        let found = match item {
            Item::Type(t) => find_named_type_at(&t.ty, offset),
            Item::Enum(e) => e.variants.iter().find_map(|v| {
                v.fields.as_ref().and_then(|fields| fields.iter().find_map(|f| find_named_type_at(&f.ty, offset)))
            }),
            Item::Service(s) => s.members.iter().find_map(|member| {
                let rpc = match member {
                    ast::Member::Rpc(r) | ast::Member::Stream(r) => r,
                };
                rpc.params
                    .iter()
                    .find_map(|p| find_named_type_at(&p.ty, offset))
                    .or_else(|| find_named_type_at(&rpc.return_type, offset))
            }),
            Item::Fn(f) => f
                .params
                .iter()
                .find_map(|p| find_named_type_at(&p.ty, offset))
                .or_else(|| find_named_type_at(&f.return_type, offset)),
            Item::Const(c) => find_named_type_at(&c.ty, offset),
            Item::Db(d) => d.collections.iter().find_map(|f| find_named_type_at(&f.ty, offset)),
            Item::Import(_) | Item::Test(_) => None,
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

/// ¿Hay un `Field` con `name_span` en `offset`, en cualquier parte de
/// `texpr`? Recorre las 8 variantes de `TypeExpr` sin brazo `_` (mismo
/// criterio que `find_named_type_at`: agregar una variante nueva rompe la
/// compilación acá en vez de que esta búsqueda la ignore en silencio).
/// `Named` recursa en sus `args` (un genérico puede envolver un struct
/// inline, ej. `Box<{ n: Int }>`) pero un `Named` no tiene campos propios.
fn field_name_at_in_type(texpr: &TypeExpr, offset: usize) -> bool {
    let in_span = |span: Span| offset >= span.start && offset < span.end;
    match texpr {
        TypeExpr::Named(_, args, _) => args.iter().any(|a| field_name_at_in_type(a, offset)),
        TypeExpr::Struct(fields) => fields.iter().any(|f| in_span(f.name_span) || field_name_at_in_type(&f.ty, offset)),
        TypeExpr::Map(k, v) => field_name_at_in_type(k, offset) || field_name_at_in_type(v, offset),
        TypeExpr::Tuple(items) => items.iter().any(|t| field_name_at_in_type(t, offset)),
        TypeExpr::Function(params, ret) => {
            params.iter().any(|p| field_name_at_in_type(p, offset)) || field_name_at_in_type(ret, offset)
        }
        TypeExpr::Optional(inner) | TypeExpr::List(inner) => field_name_at_in_type(inner, offset),
        TypeExpr::Union(members) => members.iter().any(|m| field_name_at_in_type(m, offset)),
    }
}

/// ¿El offset cae sobre el NOMBRE de un `Field` o `Param` en cualquier
/// firma del programa? (GRAMMAR.md §3.22, cierra el límite que §3.21 dejó
/// documentado.) Recorre exactamente los mismos lugares que
/// `find_named_type_in_program` -- si esto da `true`, `get_definition_inner`
/// debe responder `None` de forma AUTORITATIVA en vez de dejar que el loop
/// viejo de coincidencia-por-palabra salte a un `type`/`enum`/`fn`/`const`
/// homónimo en otro namespace: el NOMBRE de un campo/parámetro no es una
/// referencia a otro símbolo (a diferencia de su TIPO, que si el cursor
/// cayera ahí ya lo resuelve `find_named_type_in_program` arriba) -- no hay
/// ninguna declaración a la que saltar.
fn is_field_or_param_name_at(program: &Program, offset: usize) -> bool {
    let in_span = |span: Span| offset >= span.start && offset < span.end;
    program.items.iter().any(|item| match item {
        Item::Type(t) => field_name_at_in_type(&t.ty, offset),
        Item::Enum(e) => e.variants.iter().any(|v| {
            v.fields
                .as_ref()
                .is_some_and(|fields| fields.iter().any(|f| in_span(f.name_span) || field_name_at_in_type(&f.ty, offset)))
        }),
        Item::Service(s) => s.members.iter().any(|m| {
            let rpc = match m {
                ast::Member::Rpc(r) | ast::Member::Stream(r) => r,
            };
            rpc.params.iter().any(|p| in_span(p.name_span) || field_name_at_in_type(&p.ty, offset))
                || field_name_at_in_type(&rpc.return_type, offset)
        }),
        Item::Fn(f) => {
            f.params.iter().any(|p| in_span(p.name_span) || field_name_at_in_type(&p.ty, offset))
                || field_name_at_in_type(&f.return_type, offset)
        }
        Item::Const(c) => field_name_at_in_type(&c.ty, offset),
        Item::Db(d) => d.collections.iter().any(|f| in_span(f.name_span) || field_name_at_in_type(&f.ty, offset)),
        Item::Import(_) | Item::Test(_) => false,
    })
}

/// Busca la DECLARACIÓN (`type`/`enum`) de nombre `name`, para resolver a
/// dónde saltar una vez que `find_named_type_in_program` identificó que el
/// cursor está sobre un USO de ese nombre. `None` para un tipo builtin
/// (`Int`/`String`/...) o un parámetro de tipo genérico -- ninguno de los
/// dos tiene una declaración de nivel superior a la que saltar. Devuelve
/// también el ÍNDICE del ítem en `program.items` -- lo que `respond` (en
/// `get_definition_inner`) necesita para mirar `item_files[index]` y saber
/// de qué archivo real vino, ya que esta búsqueda puede cruzar a un `type`/
/// `enum` declarado en un archivo IMPORTADO, no en el documento abierto.
fn find_type_declaration(program: &Program, name: &str) -> Option<(usize, Span)> {
    program.items.iter().enumerate().find_map(|(i, item)| match item {
        Item::Type(t) if t.name == name => Some((i, t.span)),
        Item::Enum(e) if e.name == name => Some((i, e.span)),
        _ => None,
    })
}

/// `item_files`/`overlay` resuelven la identidad de archivo (GRAMMAR.md
/// §3.21, "Not done yet" hasta esta ronda): antes, con más de un archivo en
/// el cierre transitivo, esta función se negaba en bloque, porque un
/// `Span` del `Program` fusionado no decía de qué archivo venía (podía ser
/// de un `import`, no del documento abierto) y devolver una posición
/// adivinada sobre el archivo equivocado es peor que no devolver nada.
/// Ahora cada ítem sabe
/// su archivo real (`item_files[i]`, mismo orden que `program.items`,
/// poblado por `modules::load_program_with_overlay`) -- `respond` la usa
/// para apuntar al archivo correcto (potencialmente distinto de `uri`,
/// exactamente el caso "cruzar a la declaración en el archivo importado")
/// en vez de negarse. `item_files` vacío (el buffer aislado de un test o
/// un documento sin resolver vía `modules.rs`) preserva el comportamiento
/// de siempre: todo pertenece a `uri`/`source`.
fn get_definition_inner(
    uri: &str,
    source: &str,
    line0: usize,
    col0: usize,
    full_program: Option<&Program>,
    item_files: &[PathBuf],
    overlay: &HashMap<PathBuf, String>,
) -> Option<Value> {
    let mut owned = None;
    let program = resolve_program(source, full_program, &mut owned)?;

    let file_aware = !item_files.is_empty() && item_files.len() == program.items.len();

    // Arma la respuesta para el ítem `index` (con span `span`, dentro de
    // SU PROPIO archivo) -- al documento abierto si coincide (rápido, sin
    // tocar disco/overlay) o al archivo real en caso contrario. `None`
    // solo si el archivo real no se puede leer ni desde el overlay ni
    // desde disco (borrado entre el parse y este request).
    let respond = |index: usize, span: Span| -> Option<Value> {
        if !file_aware {
            return Some(json!({ "uri": uri, "range": span_to_range(source, span) }));
        }
        let target_file = &item_files[index];
        let target_uri = path_to_uri(target_file);
        if target_uri == uri {
            return Some(json!({ "uri": uri, "range": span_to_range(source, span) }));
        }
        let target_source = overlay.get(target_file).cloned().or_else(|| std::fs::read_to_string(target_file).ok())?;
        Some(json!({ "uri": target_uri, "range": span_to_range(&target_source, span) }))
    };

    // Nivel 3: goto-def de un nombre de TIPO escrito en una firma
    // (GRAMMAR.md §3.21). Corre PRIMERO y es AUTORITATIVA: si el offset
    // cae dentro de un `TypeExpr::Named`, la respuesta viene de acá o es
    // `None` -- nunca cae al loop de coincidencia-por-palabra de abajo.
    // Necesario para evitar un falso positivo real: un campo con el mismo
    // nombre que un tipo (`type Point = {...}; type Shape = { Point: Int
    // }`) haría que el loop de abajo saltara (mal) al `type Point` al
    // pedir goto-def sobre el NOMBRE DE CAMPO `Point`, no sobre un uso del
    // tipo.
    let offset = char_offset_from_utf16_position(source, line0, col0);
    if let Some((type_name, _)) = find_named_type_in_program(program, offset) {
        return find_type_declaration(program, &type_name).and_then(|(index, span)| respond(index, span));
    }

    // GRAMMAR.md §3.22: el límite que el comentario de arriba describía
    // ("un campo con el mismo nombre que un tipo...") queda cerrado acá --
    // `Field`/`Param` ahora tienen su propio `name_span` (antes no existía,
    // así que este caso caía sin remedio al loop de abajo). También
    // autoritativo: si el cursor está sobre el NOMBRE de un campo/
    // parámetro, no hay ninguna declaración a la que saltar.
    if is_field_or_param_name_at(program, offset) {
        return None;
    }

    let word = get_word_at_pos(source, line0, col0)?;

    for (index, item) in program.items.iter().enumerate() {
        let (name, span) = match item {
            Item::Type(t) => (&t.name, t.span),
            Item::Enum(e) => (&e.name, e.span),
            Item::Service(s) => (&s.name, s.span),
            Item::Fn(f) => (&f.name, f.span),
            Item::Const(c) => (&c.name, c.span),
            Item::Db(d) => {
                for col in &d.collections {
                    if col.name == word {
                        return respond(index, d.span);
                    }
                }
                continue;
            }
            Item::Import(_) | Item::Test(_) => continue,
        };

        if name == &word {
            return respond(index, span);
        }

        if let Item::Service(s) = item {
            for member in &s.members {
                let rpc = match member {
                    ast::Member::Rpc(r) | ast::Member::Stream(r) => r,
                };
                if rpc.name == word {
                    return respond(index, rpc.span);
                }
            }
        }
    }

    None
}

/// Envuelto en `catch_unwind` -- mismo patrón que `compute_diagnostics_for`/
/// `full_program_for` (ver esos comentarios): un panic acá no debe tirar
/// abajo el proceso entero de `linkc lsp`, solo esta respuesta puntual.
/// Único caller de `get_definition_inner`; los tests llaman a esta función
/// (el nombre público no cambia).
pub fn get_definition(
    uri: &str,
    source: &str,
    line0: usize,
    col0: usize,
    full_program: Option<&Program>,
    item_files: &[PathBuf],
    overlay: &HashMap<PathBuf, String>,
) -> Option<Value> {
    match std::panic::catch_unwind(|| get_definition_inner(uri, source, line0, col0, full_program, item_files, overlay)) {
        Ok(result) => result,
        Err(_) => {
            eprintln!("linkc lsp: panic interno en goto-definición para '{uri}' -- el servidor sigue corriendo");
            None
        }
    }
}

pub fn get_completions(source: &str, line0: usize, col0: usize, full_program: Option<&Program>) -> Vec<Value> {
    let prefix = get_line_prefix_at_pos(source, line0, col0);
    let mut items = Vec::new();

    if prefix.trim_end().ends_with('.') {
        // Nivel 3, ronda 3/3 (GRAMMAR.md §3.25): completion sensible al
        // tipo REAL del receptor, reusando la misma máquina que el hover
        // de expresión arbitraria (§3.24, `Checker::hover_type_at`) --
        // reemplaza la lista de siempre (todos los métodos posibles a la
        // vez, sin importar el receptor) cuando el tipo se puede
        // resolver. `receiver_type_before_dot` degrada a `None` en
        // cualquier caso no cubierto (receptor cuyo tipo depende de un
        // archivo importado, expresión sin tipo conocido, etc.) -- nunca
        // ofrece MENOS que antes de esta ronda.
        if let Some(ty) = receiver_type_before_dot(source, line0, col0) {
            if let Some(tailored) = completions_for_receiver_type(&ty) {
                return tailored;
            }
        }

        let methods = [
            ("all()", "Get all records from a db collection", 2),
            ("find(id)", "Find record by ID in a db collection", 2),
            ("insert(record)", "Insert a new record into a db collection", 2),
            ("applyPatch(id, patch)", "Update record in a db collection", 2),
            ("delete(id)", "Delete a record by ID from a db collection", 2),
            ("deleteWhere(fn)", "Delete records matching a predicate", 2),
            ("findWhere(fn)", "Find records matching a predicate", 2),
            ("subscribe()", "Subscribe to db changes in a stream", 2),
            ("length()", "Get length of array/string", 2),
            ("contains(sub)", "Check substring in a string", 2),
            ("take(limit)", "Take first N items of an array", 2),
            ("map(fn)", "Map array items", 2),
            ("filter(fn)", "Filter array items", 2),
            ("toFloat()", "Convert Int to Float", 2),
            ("toInt()", "Convert Float/Int64 to Int", 2),
            ("toInt64()", "Convert Int to Int64", 2),
        ];
        for (label, detail, kind) in methods {
            items.push(json!({
                "label": label,
                "kind": kind,
                "detail": detail,
            }));
        }

        if prefix.trim_end().ends_with("db.") {
            let mut owned = None;
            if let Some(program) = resolve_program(source, full_program, &mut owned) {
                for item in &program.items {
                    if let Item::Db(db) = item {
                        for col in &db.collections {
                            items.push(json!({
                                "label": col.name,
                                "kind": 5, // Field
                                "detail": "Database collection",
                            }));
                        }
                    }
                }
            }
        }

        return items;
    }

    if prefix.contains('@') {
        items.push(json!({
            "label": "authenticated",
            "kind": 14,
            "detail": "Annotation: Requires valid authenticated session",
        }));
        items.push(json!({
            "label": "requires",
            "kind": 14,
            "detail": "Annotation: Requires session with specified Role",
        }));
        return items;
    }

    let keywords = [
        "type", "enum", "service", "rpc", "stream", "match", "db", "fn", "test",
        "let", "mut", "const", "return", "if", "else", "while", "import", "from", "pub"
    ];
    for kw in keywords {
        items.push(json!({
            "label": kw,
            "kind": 14, // Keyword
            "detail": "Keyword",
        }));
    }

    let builtins = ["Int", "Int64", "Timestamp", "Float", "String", "Bool", "Void", "Result", "Patch"];
    for b in builtins {
        items.push(json!({
            "label": b,
            "kind": 7, // Class/Type
            "detail": "Built-in Type",
        }));
    }

    let builtin_fns = [
        ("now", "Built-in: Get current Timestamp in UTC"),
        ("assert", "Built-in: Assert condition assert(cond, [msg])"),
        ("panic", "Built-in: Panic and terminate execution with message"),
    ];
    for (name, detail) in builtin_fns {
        items.push(json!({
            "label": format!("{name}()"),
            "kind": 3, // Function
            "detail": detail,
        }));
    }

    let mut owned = None;
    if let Some(program) = resolve_program(source, full_program, &mut owned) {
        for item in &program.items {
            match item {
                Item::Type(t) => {
                    items.push(json!({
                        "label": t.name,
                        "kind": 7,
                        "detail": "Custom Type",
                    }));
                }
                Item::Enum(e) => {
                    items.push(json!({
                        "label": e.name,
                        "kind": 13,
                        "detail": "Custom Enum",
                    }));
                    for v in &e.variants {
                        items.push(json!({
                            "label": format!("{}.{}", e.name, v.name),
                            "kind": 20,
                            "detail": format!("Variant of {}", e.name),
                        }));
                    }
                }
                Item::Service(s) => {
                    items.push(json!({
                        "label": s.name,
                        "kind": 7,
                        "detail": "Service",
                    }));
                }
                Item::Fn(f) => {
                    items.push(json!({
                        "label": f.name,
                        "kind": 3,
                        "detail": "Function",
                    }));
                }
                Item::Const(c) => {
                    items.push(json!({
                        "label": c.name,
                        "kind": 21,
                        "detail": "Constant",
                    }));
                }
                _ => {}
            }
        }
    }

    items
}

/// Igual que `char_offset_from_utf16_position`, pero contando CARACTERES
/// en vez de unidades UTF-16 -- la convención que `get_word_at_pos`/
/// `get_line_prefix_at_pos` ya usan para `col0` (mismo criterio en todo
/// este archivo salvo esas dos conversiones UTF-16, que existen para el
/// protocolo LSP en sí, no para reconstruir offsets a partir de un
/// `String` ya recortado como acá).
fn char_offset_from_char_position(source: &str, target_line: usize, target_col: usize) -> usize {
    let chars: Vec<char> = source.chars().collect();
    let mut line = 0usize;
    let mut col = 0usize;
    for (idx, &c) in chars.iter().enumerate() {
        if line == target_line && col >= target_col {
            return idx;
        }
        if c == '\n' {
            if line == target_line {
                return idx;
            }
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    chars.len()
}

/// El `Type` real de la expresión receptora justo antes del `.` en
/// `line0`/`col0` (GRAMMAR.md §3.25) -- `None` si no se puede determinar.
///
/// Mientras se escribe "x.", el buffer casi siempre NO PARSEA (un `.`
/// colgante, sin nada después, es un error de sintaxis) -- así que en vez
/// de tipar el buffer TAL CUAL, se tipa una COPIA con el `.` colgante (y
/// cualquier espacio hasta el cursor) reemplazado por espacios en blanco
/// de el MISMO largo. Todos los demás offsets del archivo (antes del `.`
/// y después del cursor) quedan intactos -- el receptor (`x`) y el resto
/// del archivo parsean normal, y el offset que se le pasa a
/// `hover_type_at` sigue siendo válido contra el texto ORIGINAL para
/// cualquier uso posterior.
///
/// Límite honesto: la copia se re-parsea de forma AISLADA (`parser::parse`
/// directo, no `modules::load_program_with_overlay`) -- si el tipo del
/// receptor depende de un `type`/`enum` declarado en un archivo
/// IMPORTADO, la copia no lo resuelve y esto da `None` (cae al fallback
/// de siempre en `get_completions`, nunca a una respuesta incorrecta).
/// Cerrar esto necesitaría reconstruir el overlay completo del `LspServer`
/// acá, que es un método de instancia, no de esta función libre.
fn receiver_type_before_dot(source: &str, line0: usize, col0: usize) -> Option<Type> {
    let prefix = get_line_prefix_at_pos(source, line0, col0);
    let trimmed = prefix.trim_end();
    if !trimmed.ends_with('.') {
        return None;
    }
    let receiver = trimmed[..trimmed.len() - 1].trim_end();
    if receiver.is_empty() {
        return None;
    }

    let dot_col = trimmed.chars().count() - 1;
    let dot_offset = char_offset_from_char_position(source, line0, dot_col);
    let cursor_offset = char_offset_from_char_position(source, line0, col0).max(dot_offset);

    let mut patched: Vec<char> = source.chars().collect();
    for c in patched.get_mut(dot_offset..cursor_offset)? {
        if *c != '\n' {
            *c = ' ';
        }
    }
    let patched_source: String = patched.into_iter().collect();

    let tokens = lexer::tokenize(&patched_source).ok()?;
    let program = parser::parse(tokens).ok()?;

    let receiver_end_col = receiver.chars().count().saturating_sub(1);
    let receiver_offset = char_offset_from_char_position(source, line0, receiver_end_col);
    Checker::hover_type_at(&program, receiver_offset)
}

/// Lista de completions tailoreada al `Type` real de un receptor (GRAMMAR.md
/// §3.25) -- `None` señala "este tipo no está cubierto acá todavía", para
/// que el caller caiga a la lista genérica de siempre en vez de ofrecer
/// MENOS que antes de esta ronda. `Type::Db` es un `None` a propósito: ya
/// tiene su propio manejo en `get_completions` (nombres de colección, no
/// métodos) que necesita el `Program` completo, no solo el `Type`.
fn completions_for_receiver_type(ty: &Type) -> Option<Vec<Value>> {
    let method = |label: &str, detail: &str| json!({ "label": label, "kind": 2, "detail": detail });
    match ty {
        Type::DbCollection(_) => Some(vec![
            method("all()", "Get all records from this collection"),
            method("find(id)", "Find a record by id in this collection"),
            method("insert(record)", "Insert a new record into this collection"),
            method("applyPatch(id, patch)", "Update a record in this collection"),
            method("delete(id)", "Delete a record by id from this collection"),
            method("deleteWhere(fn)", "Delete records matching a predicate"),
            method("findWhere(fn)", "Find records matching a predicate"),
            method("subscribe()", "Subscribe to live changes in a stream"),
        ]),
        Type::List(_) => Some(vec![
            method("length()", "Get the length of this list"),
            method("take(limit)", "Take the first N items"),
            method("map(fn)", "Map this list's items"),
            method("filter(fn)", "Filter this list's items"),
        ]),
        Type::String => Some(vec![
            method("length()", "Get the length of this string"),
            method("contains(sub)", "Check if this string contains a substring"),
        ]),
        Type::Int => Some(vec![
            method("toFloat()", "Convert this Int to Float"),
            method("toInt64()", "Convert this Int to Int64"),
        ]),
        Type::Int64 => Some(vec![method("toInt()", "Convert this Int64 to Int")]),
        // Lista vacía EXPLÍCITA, no `None` -- v0 no tiene ningún método
        // sobre Timestamp (GRAMMAR.md §3.31); caer al fallback genérico
        // ofrecería métodos de otros tipos que acá no aplican.
        Type::Timestamp => Some(vec![]),
        Type::Float => Some(vec![method("toInt()", "Convert this Float to Int")]),
        Type::Auth => Some(vec![
            method("createSession(role)", "Create an opaque session token for the given Role"),
            method("createSessionWithId(role, userId)", "Create an opaque session token for the given Role and userId"),
            method("destroySession()", "Destroy the current session"),
            method("currentRole()", "Get the caller's authenticated role (String?)"),
            method("currentUserId()", "Get the caller's authenticated user id (Int?)"),
        ]),
        Type::Struct { fields, .. } => Some(
            fields
                .iter()
                .map(|f| json!({ "label": f.name, "kind": 5, "detail": format!("Field: {}", render_type(&f.ty)) }))
                .collect(),
        ),
        // Un `T?` no puede desreferenciarse por `.campo` -- ofrecer los
        // campos de `inner` acá sería sugerir código que el checker rechaza
        // ni bien se acepte (GRAMMAR.md §3.69). Los únicos dos métodos
        // reales sobre un opcional son estos; leer el valor de verdad
        // necesita 'match' (sin completado propio -- no es un método).
        Type::Optional(_) => Some(vec![
            method("isSome()", "Check if this optional has a value, without reading it"),
            method("isNone()", "Check if this optional is null, without reading it"),
        ]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Directorio temporal aislado por test -- mismo patrón que
    /// `modules.rs::tests::TempDir`, necesario acá porque
    /// `compute_diagnostics_for`/`full_program_for` ahora resuelven contra
    /// archivos REALES en disco (para soportar `import` de verdad), a
    /// diferencia de las URIs ficticias que alcanzaban antes de esta ronda.
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("cscript-lsp-test-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
        fn write(&self, rel: &str, contents: &str) -> String {
            let path = self.0.join(rel);
            std::fs::write(&path, contents).unwrap();
            path_to_uri(&std::fs::canonicalize(&path).unwrap())
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn did_open(server: &mut LspServer, uri: &str, text: &str) -> Value {
        let req = json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": { "uri": uri, "languageId": "c-script", "version": 1, "text": text }
            }
        });
        server.handle_message(&req).expect("Debe retornar notificación")
    }

    /// `compute_diagnostics_for`/`full_program_for` envuelven su lógica
    /// real en `catch_unwind` para que un panic futuro en el checker (hoy
    /// no hay ninguno conocido alcanzable desde texto inválido, pero es un
    /// componente que sigue creciendo) degrade a un solo diagnóstico en
    /// vez de tirar abajo el proceso de `linkc lsp` entero -- ver el
    /// comentario en `compute_diagnostics_for`. No hay ningún input real
    /// hoy que dispare ese panic, así que esto prueba el MECANISMO en sí
    /// (con un panic sintético, mismo patrón exacto de captura de `&self`
    /// que el código real usa) en vez de una regresión de negocio puntual
    /// -- si `LspServer` alguna vez ganara mutabilidad interior (un
    /// `RefCell`/`Mutex`), este mismo test seguiría compilando pero ya no
    /// probaría lo mismo, así que también sirve de canario para eso.
    #[test]
    fn test_catch_unwind_around_a_document_recheck_does_not_crash_the_server() {
        let server = LspServer::new();
        let prev_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {})); // el panic es intencional -- no ensuciar la salida de `cargo test`
        let result = std::panic::catch_unwind(|| {
            let _ = server.documents.len(); // mismo patrón de captura de &self que compute_diagnostics_for/full_program_for
            panic!("panic sintético -- prueba el mecanismo de catch_unwind, no un bug real conocido");
        });
        std::panic::set_hook(prev_hook);
        assert!(result.is_err(), "catch_unwind debería atrapar el panic en vez de dejarlo propagar");
    }

    #[test]
    fn test_initialize_handshake() {
        let mut server = LspServer::new();
        let req = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {}
        });

        let resp = server.handle_message(&req).expect("Debe retornar respuesta");
        assert_eq!(resp["id"], 1);
        assert_eq!(resp["result"]["capabilities"]["textDocumentSync"], 1);
        assert_eq!(resp["result"]["capabilities"]["hoverProvider"], true);
        assert_eq!(resp["result"]["capabilities"]["definitionProvider"], true);
    }

    #[test]
    fn test_a_request_with_an_id_but_no_method_gets_an_explicit_error_not_silence() {
        // Gap real encontrado en un reparso: antes, esto devolvía `None`
        // -- un cliente real esperando una respuesta a ESE id se quedaba
        // esperando para siempre, sin ningún indicio de qué pasó.
        let mut server = LspServer::new();
        let req = json!({ "jsonrpc": "2.0", "id": 42, "params": {} });

        let resp = server.handle_message(&req).expect("un request con id debe recibir una respuesta, aunque sea un error");
        assert_eq!(resp["id"], 42, "el error debe traer el mismo id que el request roto");
        assert!(resp["error"]["code"].is_i64(), "debe ser un error JSON-RPC real, no un resultado inventado: {resp:?}");
    }

    #[test]
    fn test_a_notification_with_no_method_and_no_id_is_silently_ignored() {
        // Mismo input roto que el test de arriba, pero SIN "id" -- una
        // notificación (no un request) no espera respuesta según el
        // protocolo, así que `None` acá sigue siendo lo correcto, no una
        // regresión del fix de arriba.
        let mut server = LspServer::new();
        let req = json!({ "jsonrpc": "2.0", "params": {} });
        assert!(server.handle_message(&req).is_none());
    }

    #[test]
    fn test_did_open_and_publish_diagnostics_clean() {
        // URI ficticio (sin archivo real en disco) -- ejercita el fallback
        // `compute_diagnostics_standalone`, mismo comportamiento que antes
        // de esta ronda.
        let mut server = LspServer::new();
        let code = "type User = { id: Int, name: String }";
        let resp = did_open(&mut server, "file:///test.link", code);
        assert_eq!(resp["method"], "textDocument/publishDiagnostics");
        assert_eq!(resp["params"]["uri"], "file:///test.link");
        assert_eq!(resp["params"]["diagnostics"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn test_did_open_publishes_syntax_error_diagnostics() {
        let mut server = LspServer::new();
        let code = "type User = { id Int }"; // Falta ':'
        let resp = did_open(&mut server, "file:///bad.link", code);
        assert_eq!(resp["method"], "textDocument/publishDiagnostics");
        let diags = resp["params"]["diagnostics"].as_array().unwrap();
        assert!(!diags.is_empty());
        assert!(diags[0]["message"].as_str().unwrap().contains("Colon"));
    }

    #[test]
    fn test_import_across_real_files_does_not_produce_a_false_positive() {
        // El bug real que esta ronda arregla: antes, cualquier archivo con
        // `import` daba "no declarado" porque el LSP nunca resolvía el
        // archivo importado -- acá `a.link` importa `Point` de `b.link`,
        // ambos archivos reales en disco, y no debería haber NINGÚN
        // diagnóstico.
        let dir = TempDir::new("import_ok");
        dir.write("b.link", "type Point = { x: Int, y: Int }");
        let uri_a = dir.write(
            "a.link",
            r#"import { Point } from "./b.link"; fn origin() -> Point { Point { x: 0, y: 0 } }"#,
        );

        let mut server = LspServer::new();
        let text_a = std::fs::read_to_string(uri_to_path(&uri_a).unwrap()).unwrap();
        let resp = did_open(&mut server, &uri_a, &text_a);
        let diags = resp["params"]["diagnostics"].as_array().unwrap();
        assert_eq!(diags.len(), 0, "un import válido a un archivo real no debería dar ningún diagnóstico: {diags:?}");
    }

    #[test]
    fn test_syntax_error_in_an_imported_file_is_surfaced_not_swallowed() {
        let dir = TempDir::new("import_syntax_error");
        dir.write("b.link", "type Point = { x Int }"); // falta ':'
        let uri_a = dir.write("a.link", r#"import { Point } from "./b.link";"#);

        let mut server = LspServer::new();
        let text_a = std::fs::read_to_string(uri_to_path(&uri_a).unwrap()).unwrap();
        let resp = did_open(&mut server, &uri_a, &text_a);
        let diags = resp["params"]["diagnostics"].as_array().unwrap();
        assert!(!diags.is_empty(), "un error de sintaxis en el archivo importado tiene que aparecer, no desaparecer en silencio");
    }

    #[test]
    fn test_hover_sees_a_type_imported_from_another_real_file() {
        let dir = TempDir::new("hover_import");
        dir.write("b.link", "type Point = { x: Int, y: Int }");
        let uri_a = dir.write("a.link", r#"import { Point } from "./b.link"; fn origin() -> Point { Point { x: 0, y: 0 } }"#);
        let text_a = std::fs::read_to_string(uri_to_path(&uri_a).unwrap()).unwrap();

        let mut server = LspServer::new();
        did_open(&mut server, &uri_a, &text_a);

        let req = json!({
            "jsonrpc": "2.0", "id": 1, "method": "textDocument/hover",
            "params": { "textDocument": { "uri": uri_a }, "position": { "line": 0, "character": 42 } }
        });
        let resp = server.handle_message(&req).expect("debe responder");
        let value = resp["result"]["contents"]["value"].as_str().unwrap_or("");
        assert!(value.contains("Point"), "el hover sobre 'Point' (usado, no declarado en este archivo) tendría que resolverlo vía import: {value}");
    }

    #[test]
    fn test_span_to_range_handles_a_multi_line_span() {
        let source = "type Foo = {\n  id: Int,\n  name: String\n}\n";
        // El span de la declaración entera: desde "type" (offset 0) hasta
        // la '}' de cierre inclusive.
        let end = source.find('}').unwrap() + 1;
        let span = Span::new(0, end, 1, 1);
        let range = span_to_range(source, span);
        assert_eq!(range["start"]["line"], 0);
        assert_eq!(range["end"]["line"], 3, "el fin del span está en la 4ta línea (índice 3), no en la primera: {range}");
    }

    #[test]
    fn test_hover_builtin_keyword() {
        let code = "service Users {}";
        let hover = get_hover(code, 0, 2, None).expect("Hover debe retornar resultado");
        let markdown = hover["contents"]["value"].as_str().unwrap();
        assert!(markdown.contains("Keyword `service`"));
    }

    #[test]
    fn test_hover_custom_type() {
        let code = "type User = { id: Int }\nservice Users {}";
        let hover = get_hover(code, 0, 6, None).expect("Hover debe retornar tipo");
        let markdown = hover["contents"]["value"].as_str().unwrap();
        assert!(markdown.contains("type User"));
    }

    #[test]
    fn test_hover_builtin_type_int64() {
        let code = "type Counter = { big: Int64 }\n";
        let col = code.find("Int64").unwrap();
        let hover = get_hover(code, 0, col, None).expect("Hover debe retornar resultado");
        let markdown = hover["contents"]["value"].as_str().unwrap();
        assert!(markdown.contains("Builtin Type `Int64`"), "{markdown}");
    }

    #[test]
    fn test_hover_builtin_type_timestamp() {
        let code = "type Event = { at: Timestamp }\n";
        let col = code.find("Timestamp").unwrap();
        let hover = get_hover(code, 0, col, None).expect("Hover debe retornar resultado");
        let markdown = hover["contents"]["value"].as_str().unwrap();
        assert!(markdown.contains("Builtin Type `Timestamp`"), "{markdown}");
    }

    // ---- hover de expresión arbitraria (Nivel 3 ronda 2/3, GRAMMAR.md §3.24) ----

    #[test]
    fn test_hover_on_a_param_reference_inside_a_body_shows_its_type() {
        let code = "fn f(x: Int) -> Bool { x > 5 }\n";
        let col = code.find("x > 5").unwrap(); // sobre la 'x'
        let hover = get_hover(code, 0, col, None).expect("debe encontrar el tipo de x");
        let markdown = hover["contents"]["value"].as_str().unwrap();
        assert!(markdown.contains("number"), "x: Int se renderiza como TypeScript 'number': {markdown}");
    }

    #[test]
    fn test_hover_on_a_struct_typed_expression_renders_the_real_type_name() {
        // render_type (ts_emit.rs) es el MISMO renderer que ya usa el
        // contrato real -- un struct nombrado muestra su nombre TS, no un
        // volcado de Debug de Rust.
        let code = "type Point = { x: Int, y: Int }\nfn origin() -> Point { let p: Point = Point { x: 0, y: 0 }; p }\n";
        let line1 = code.lines().nth(1).unwrap();
        let col = line1.rfind("p }").unwrap(); // la 'p' final, la expresión de retorno
        let hover = get_hover(code, 1, col, None).expect("debe encontrar el tipo de p");
        let markdown = hover["contents"]["value"].as_str().unwrap();
        assert!(markdown.contains("Point"), "debe mostrar el nombre real del tipo, no una estructura anónima: {markdown}");
    }

    #[test]
    fn test_hover_on_whitespace_outside_any_body_still_returns_none() {
        let code = "type User = { id: Int }\n";
        let hover = get_hover(code, 0, 11, None); // el espacio entre '=' y '{'
        assert!(hover.is_none(), "no hay ninguna expresión ni palabra clave ahí: {hover:?}");
    }

    #[test]
    fn test_completions() {
        let code = "type User = { id: Int }\n";
        let completions = get_completions(code, 1, 0, None);
        assert!(completions.iter().any(|c| c["label"] == "service"));
        assert!(completions.iter().any(|c| c["label"] == "User"));
    }

    // ---- completion sensible al tipo real del receptor (Nivel 3 ronda 3/3, GRAMMAR.md §3.25) ----

    #[test]
    fn test_completion_after_dot_on_a_list_receiver_only_offers_list_methods() {
        let code = "fn f(xs: Int[]) -> Int[] { xs. }\n";
        let col = code.find("xs.").unwrap() + 3; // justo después del '.'
        let completions = get_completions(code, 0, col, None);
        assert!(completions.iter().any(|c| c["label"] == "map(fn)"), "{completions:?}");
        assert!(
            !completions.iter().any(|c| c["label"] == "contains(sub)"),
            "una lista no tiene 'contains' (método de String) -- la lista no debería estar sin tailorear: {completions:?}"
        );
    }

    #[test]
    fn test_completion_after_dot_on_a_string_receiver_only_offers_string_methods() {
        let code = "fn f(s: String) -> Int { s. }\n";
        let col = code.find("s.").unwrap() + 2;
        let completions = get_completions(code, 0, col, None);
        assert!(completions.iter().any(|c| c["label"] == "contains(sub)"), "{completions:?}");
        assert!(!completions.iter().any(|c| c["label"] == "map(fn)"), "un String no tiene 'map' (método de lista): {completions:?}");
    }

    #[test]
    fn test_completion_after_dot_on_an_int64_receiver_offers_toint_not_tofloat() {
        let code = "fn f(n: Int64) -> Int { n. }\n";
        let col = code.find("n.").unwrap() + 2;
        let completions = get_completions(code, 0, col, None);
        assert!(completions.iter().any(|c| c["label"] == "toInt()"), "{completions:?}");
        assert!(
            !completions.iter().any(|c| c["label"] == "toFloat()"),
            "Int64 no tiene toFloat (eso es de Int) -- el receptor no debería estar sin tailorear: {completions:?}"
        );
    }

    #[test]
    fn test_completion_after_dot_on_a_timestamp_receiver_offers_nothing() {
        // v0 no tiene ningún método sobre Timestamp (GRAMMAR.md §3.31) --
        // lista vacía tailoreada, no el fallback genérico con métodos de
        // otros tipos que acá no aplican.
        let code = "fn f(t: Timestamp) -> Int { t. }\n";
        let col = code.find("t.").unwrap() + 2;
        let completions = get_completions(code, 0, col, None);
        assert!(completions.is_empty(), "{completions:?}");
    }

    #[test]
    fn test_completion_after_dot_on_a_struct_receiver_offers_its_field_names() {
        // Capacidad NUEVA de esta ronda: antes, ningún tipo de receptor
        // ofrecía nombres de CAMPO como completion, solo métodos builtin.
        let code = "type Point = { x: Int, y: Int }\nfn f(p: Point) -> Int { p. }\n";
        let line1 = code.lines().nth(1).unwrap();
        let col = line1.find("p.").unwrap() + 2;
        let completions = get_completions(code, 1, col, None);
        assert!(completions.iter().any(|c| c["label"] == "x"), "{completions:?}");
        assert!(completions.iter().any(|c| c["label"] == "y"), "{completions:?}");
        assert!(!completions.iter().any(|c| c["label"] == "map(fn)"), "un struct no tiene métodos de lista: {completions:?}");
    }

    #[test]
    fn test_completion_after_dot_on_an_optional_receiver_only_offers_is_some_is_none() {
        // GRAMMAR.md §3.69: un `T?` no puede desreferenciarse por `.campo`
        // (ni por completion) -- solo `isSome()`/`isNone()` son válidos acá,
        // nunca los campos de `Point` (eso necesita 'match' primero).
        let code = "type Point = { x: Int, y: Int }\nfn f(p: Point?) -> Bool { p. }\n";
        let line1 = code.lines().nth(1).unwrap();
        let col = line1.find("p.").unwrap() + 2;
        let completions = get_completions(code, 1, col, None);
        assert!(completions.iter().any(|c| c["label"] == "isSome()"), "{completions:?}");
        assert!(completions.iter().any(|c| c["label"] == "isNone()"), "{completions:?}");
        assert!(!completions.iter().any(|c| c["label"] == "x"), "un T? no ofrece los campos de T: {completions:?}");
    }

    #[test]
    fn test_completion_after_dot_on_a_specific_db_collection_only_offers_collection_methods() {
        let code = "db { users: User[] }\ntype User = { id: Int }\nfn f() -> Int { db.users. }\n";
        let line2 = code.lines().nth(2).unwrap();
        let col = line2.find("db.users.").unwrap() + "db.users.".len();
        let completions = get_completions(code, 2, col, None);
        assert!(completions.iter().any(|c| c["label"] == "insert(record)"), "{completions:?}");
        assert!(
            !completions.iter().any(|c| c["label"] == "map(fn)"),
            "una colección específica no es una lista genérica -- no tiene 'map' directo: {completions:?}"
        );
    }

    #[test]
    fn test_completion_after_dot_falls_back_to_the_generic_list_when_the_type_is_unknown() {
        // Regresión: si el tipo del receptor no se puede determinar (acá,
        // un identificador que ni siquiera existe), la lista genérica de
        // siempre sigue disponible -- nunca ofrecer MENOS que antes de
        // esta ronda.
        let code = "fn f() -> Int { unknownVar. }\n";
        let col = code.find("unknownVar.").unwrap() + "unknownVar.".len();
        let completions = get_completions(code, 0, col, None);
        assert!(completions.iter().any(|c| c["label"] == "map(fn)"), "{completions:?}");
        assert!(completions.iter().any(|c| c["label"] == "contains(sub)"), "{completions:?}");
    }

    #[test]
    fn test_definition() {
        let code = "type User = { id: Int }\n";
        let def = get_definition("file:///test.link", code, 0, 6, None, &[], &HashMap::new()).expect("Debe encontrar definición");
        assert_eq!(def["uri"], "file:///test.link");
        assert_eq!(def["range"]["start"]["line"], 0);
    }

    // ---- goto-def de un nombre de tipo en una firma (Nivel 3, GRAMMAR.md §3.21) ----

    #[test]
    fn test_goto_def_on_type_name_in_return_type() {
        let code = "type Point = { x: Int, y: Int }\nfn origin() -> Point { Point { x: 0, y: 0 } }\n";
        let line = code.lines().nth(1).unwrap();
        let col = line.find("-> Point").unwrap() + 3; // 'P' de Point, no el '-' de '->'
        let def =
            get_definition("file:///test.link", code, 1, col, None, &[], &HashMap::new()).expect("debe encontrar la declaración de Point");
        assert_eq!(def["range"]["start"]["line"], 0, "debe apuntar a 'type Point' (línea 0), no a la firma donde se usa");
    }

    #[test]
    fn test_goto_def_on_type_name_in_param_type() {
        let code = "type Point = { x: Int, y: Int }\nfn dist(a: Point, b: Point) -> Int { 0 }\n";
        let line = code.lines().nth(1).unwrap();
        let col = line.find("a: Point").unwrap() + 3; // 'P' de Point en el primer parámetro
        let def =
            get_definition("file:///test.link", code, 1, col, None, &[], &HashMap::new()).expect("debe encontrar la declaración de Point");
        assert_eq!(def["range"]["start"]["line"], 0);
    }

    #[test]
    fn test_goto_def_on_type_name_nested_inside_a_generic() {
        let code = "type Line = { startX: Int, endX: Int }\ntype Box<T> = { value: T }\nfn f() -> Box<Line> { Box { value: Line { startX: 0, endX: 0 } } }\n";
        let line = code.lines().nth(2).unwrap();
        let col = line.find("Box<Line>").unwrap() + "Box<".len(); // 'L' de Line, no la 'B' de Box
        let def = get_definition("file:///test.link", code, 2, col, None, &[], &HashMap::new()).expect("debe encontrar Line, no Box");
        assert_eq!(def["range"]["start"]["line"], 0, "debe apuntar a 'type Line' (línea 0), no a 'type Box' (línea 1)");
    }

    #[test]
    fn test_goto_def_on_a_builtin_type_name_does_not_jump_to_an_unrelated_same_named_const() {
        // "Int" es un tipo builtin sin declaración type/enum propia. Si
        // además existe un `const Int = ...` (nombre coincidente, otro
        // namespace), el viejo loop de coincidencia-por-palabra saltaría
        // (mal) a ESE const al pedir goto-def sobre el "Int" del tipo de
        // retorno -- la búsqueda nueva es autoritativa (offset dentro de un
        // TypeExpr::Named): responde `None` ella misma y nunca cae al loop
        // viejo.
        let code = "const Int: Bool = true;\nfn f() -> Int { 0 }\n";
        let line = code.lines().nth(1).unwrap();
        let col = line.find("-> Int").unwrap() + 3;
        let result = get_definition("file:///test.link", code, 1, col, None, &[], &HashMap::new());
        assert!(result.is_none(), "'Int' es builtin, sin declaración type/enum -- no debe saltar al const homónimo: {result:?}");
    }

    // ---- Field/Param ganan name_span (GRAMMAR.md §3.22) ----

    #[test]
    fn test_goto_def_on_a_field_name_that_collides_with_an_existing_type_name_does_not_jump() {
        // El límite honesto que §3.21 dejó documentado, ahora cerrado:
        // "Point" es tanto un `type` real COMO el nombre de un campo de
        // `Shape`. Pedir goto-def sobre el NOMBRE DE CAMPO `Point` (no su
        // tipo, que acá es `Int`) antes caía al loop viejo de
        // coincidencia-por-palabra, que saltaba (mal) a `type Point` --
        // `Field::name_span` (nuevo en esta ronda) permite distinguir
        // ambos casos: acá debe responder `None`, no una posición.
        let code = "type Point = { x: Int, y: Int }\ntype Shape = { Point: Int }\n";
        let line = code.lines().nth(1).unwrap();
        let col = line.find("Point").unwrap() + 1; // dentro del nombre de campo 'Point', no de su tipo 'Int'
        let result = get_definition("file:///test.link", code, 1, col, None, &[], &HashMap::new());
        assert!(result.is_none(), "el cursor está sobre el NOMBRE de un campo, no un uso de tipo -- no debe saltar: {result:?}");
    }

    #[test]
    fn test_goto_def_on_the_field_type_still_jumps_when_the_field_name_collides_with_it() {
        // Contraparte del test anterior, mismo código: pedir goto-def
        // sobre el TIPO del campo (`Int`, después de los ':') sigue
        // funcionando como siempre -- el gate nuevo no debe volverse
        // sobre-amplio y tragarse casos que sí son un uso de tipo real.
        let code = "type Marker = { x: Int }\ntype Shape = { Marker: Marker }\n";
        let line = code.lines().nth(1).unwrap();
        let col = line.rfind("Marker").unwrap() + 1; // el segundo 'Marker' (el tipo), no el primero (el nombre de campo)
        let def = get_definition("file:///test.link", code, 1, col, None, &[], &HashMap::new())
            .expect("el cursor está sobre el TIPO del campo -- debe resolver a 'type Marker'");
        assert_eq!(def["range"]["start"]["line"], 0, "debe apuntar a 'type Marker' (línea 0): {def:?}");
    }

    #[test]
    fn test_goto_def_on_a_param_name_that_collides_with_an_existing_type_name_does_not_jump() {
        // Mismo bug, en un `Param` de `fn` en vez de un `Field` de `type`.
        let code = "type Point = { x: Int, y: Int }\nfn f(Point: Int) -> Int { Point }\n";
        let line = code.lines().nth(1).unwrap();
        let col = line.find("Point").unwrap() + 1; // el nombre del parámetro, no su tipo Int
        let result = get_definition("file:///test.link", code, 1, col, None, &[], &HashMap::new());
        assert!(result.is_none(), "el cursor está sobre el NOMBRE de un parámetro, no un uso de tipo -- no debe saltar: {result:?}");
    }

    #[test]
    fn test_goto_def_without_item_files_treats_everything_as_the_open_document() {
        // `item_files` vacío (el buffer aislado que estos mismos tests
        // usan, sin pasar por `modules.rs`) es el modo "sin identidad de
        // archivo real disponible" -- `file_aware` da `false` y todo
        // resuelve contra `uri`/`source`, igual que ANTES de esta ronda
        // (cuando `touched_len > 1` era el único criterio y este caso caía
        // siempre en la rama de un solo archivo). El caso de verdad
        // multi-archivo (con `item_files` real) se prueba más abajo, con
        // archivos reales en disco -- acá no hay forma honesta de simular
        // "más de un archivo" sin ellos.
        let code = "type Point = { x: Int, y: Int }\nfn origin() -> Point { Point { x: 0, y: 0 } }\n";
        let line = code.lines().nth(1).unwrap();
        let col = line.find("-> Point").unwrap() + 3;
        let def = get_definition("file:///test.link", code, 1, col, None, &[], &HashMap::new())
            .expect("sin item_files, debe resolver contra el propio documento como siempre");
        assert_eq!(def["uri"], "file:///test.link");
        assert_eq!(def["range"]["start"]["line"], 0);
    }

    #[test]
    fn test_goto_def_resolves_to_the_real_imported_file_when_item_files_is_available() {
        // El caso que GRAMMAR.md §3.21 dejaba pendiente ("Not done yet"):
        // goto-def sobre un tipo declarado en un archivo IMPORTADO, con
        // item_files real (mismo shape que `modules::load_program_with_
        // overlay` produce) -- antes esto se negaba en bloque
        // (`touched_len > 1 -> None`); ahora debe resolver al archivo Y
        // rango reales de la declaración, no al documento que abrió la
        // request.
        let dir = TempDir::new("gotodef_cross_file_resolves");
        let b_uri = dir.write("b.link", "type Point = { x: Int, y: Int }\n");
        let a_text = "import { Point } from \"./b.link\";\nfn origin() -> Point { Point { x: 0, y: 0 } }\n";
        let a_uri = dir.write("a.link", a_text);
        let a_path = uri_to_path(&a_uri).expect("a_uri debe convertir de vuelta a un Path");

        let (program, _touched, item_files) = modules::load_program(&a_path).expect("a.link debe cargar bien");
        let col = a_text.lines().nth(1).unwrap().find("-> Point").unwrap() + 3;

        let def = get_definition(&a_uri, a_text, 1, col, Some(&program), &item_files, &HashMap::new())
            .expect("debe resolver la declaración de Point en b.link, no devolver null");
        assert_eq!(def["uri"], b_uri, "debe apuntar a b.link, no al a.link que abrió la request: {def:?}");
        assert_eq!(def["range"]["start"]["line"], 0, "'type Point' es la línea 0 de b.link: {def:?}");
    }

    #[test]
    fn test_uri_path_roundtrip_windows_drive_letter_and_space() {
        let path = PathBuf::from("C:/Users/Some User/proj/main.link");
        let uri = path_to_uri(&path);
        assert_eq!(uri, "file:///C:/Users/Some%20User/proj/main.link".replace("%20", " "), "path_to_uri no percent-encodea (no hace falta para nuestro propio roundtrip)");
        let back = uri_to_path(&uri).unwrap();
        assert_eq!(back, path);
    }

    #[test]
    fn test_uri_path_roundtrip_percent_encoded_space() {
        // Lo que un editor real SÍ manda: un espacio percent-encoded.
        let uri = "file:///C:/Users/Some%20User/proj/main.link";
        let path = uri_to_path(uri).unwrap();
        assert_eq!(path, PathBuf::from("C:/Users/Some User/proj/main.link"));
    }
}
