use crate::ast::{self, Item, Program};
use crate::checker::Checker;
use crate::lexer;
use crate::modules;
use crate::parser;
use crate::token::Span;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};

pub struct LspServer {
    documents: HashMap<String, String>,
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

    fn full_program_for_inner(&self, uri: &str) -> Option<Program> {
        let entry_path = uri_to_path(uri)?;
        let entry_canon = std::fs::canonicalize(&entry_path).ok()?;
        let overlay = self.build_overlay();
        modules::load_program_with_overlay(&entry_canon, &overlay).ok().map(|(program, _)| program)
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
            Ok((program, touched)) => {
                let (_, errors) = Checker::check_program_full(&program);
                if touched.len() <= 1 {
                    // Un solo archivo en el cierre transitivo -- mismo
                    // documento que `uri`, así que cada span se puede
                    // convertir con precisión total.
                    let source = self.documents.get(uri).cloned().unwrap_or_default();
                    errors
                        .into_iter()
                        .map(|e| {
                            let range = match e.span {
                                Some(span) => span_to_range(&source, span),
                                None => zero_range(),
                            };
                            json!({ "range": range, "severity": 1, "source": "c-script", "message": e.message })
                        })
                        .collect()
                } else if errors.is_empty() {
                    vec![]
                } else {
                    // Más de un archivo: un CheckError no tiene identidad
                    // de archivo tras el merge (a diferencia de
                    // LoadError::Syntax arriba), así que NUNCA se adivina
                    // una posición -- mismo criterio que
                    // `main.rs::report_check_errors`'s gate `touched.len()
                    // == 1`. Se publican todos los mensajes igual (nunca
                    // esconder que algo está mal), anclados en una
                    // posición degradada que dice explícitamente por qué.
                    let combined = errors.iter().map(|e| e.message.as_str()).collect::<Vec<_>>().join("; ");
                    vec![zero_diagnostic(format!(
                        "(la posición puede estar en uno de los {} archivos importados) {combined}",
                        touched.len()
                    ))]
                }
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

            let Some(len) = content_length else {
                continue;
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
        let method = req.get("method")?.as_str()?;
        let id = req.get("id");

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
                            "definitionProvider": true
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

                let full_program = self.full_program_for(uri);
                let loc = if let Some(source) = self.documents.get(uri) {
                    get_definition(uri, source, line, character, full_program.as_ref()).unwrap_or(serde_json::Value::Null)
                } else {
                    serde_json::Value::Null
                };

                Some(json!({
                    "jsonrpc": "2.0",
                    "id": id_val,
                    "result": loc
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
    let word = get_word_at_pos(source, line0, col0)?;

    let builtin_hover = match word.as_str() {
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

pub fn get_definition(uri: &str, source: &str, line0: usize, col0: usize, full_program: Option<&Program>) -> Option<Value> {
    let word = get_word_at_pos(source, line0, col0)?;

    let mut owned = None;
    let program = resolve_program(source, full_program, &mut owned)?;

    for item in &program.items {
        let (name, span) = match item {
            Item::Type(t) => (&t.name, t.span),
            Item::Enum(e) => (&e.name, e.span),
            Item::Service(s) => (&s.name, s.span),
            Item::Fn(f) => (&f.name, f.span),
            Item::Const(c) => (&c.name, c.span),
            Item::Db(d) => {
                for col in &d.collections {
                    if col.name == word {
                        return Some(json!({
                            "uri": uri,
                            "range": span_to_range(source, d.span)
                        }));
                    }
                }
                continue;
            }
            Item::Import(_) => continue,
        };

        if name == &word {
            return Some(json!({
                "uri": uri,
                "range": span_to_range(source, span)
            }));
        }

        if let Item::Service(s) = item {
            for member in &s.members {
                let rpc = match member {
                    ast::Member::Rpc(r) | ast::Member::Stream(r) => r,
                };
                if rpc.name == word {
                    return Some(json!({
                        "uri": uri,
                        "range": span_to_range(source, rpc.span)
                    }));
                }
            }
        }
    }

    None
}

pub fn get_completions(source: &str, line0: usize, col0: usize, full_program: Option<&Program>) -> Vec<Value> {
    let prefix = get_line_prefix_at_pos(source, line0, col0);
    let mut items = Vec::new();

    if prefix.trim_end().ends_with('.') {
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
            ("toInt()", "Convert Float/String to Int", 2),
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
        "type", "enum", "service", "rpc", "stream", "match", "db", "fn",
        "let", "mut", "const", "return", "if", "else", "while", "import", "from", "pub"
    ];
    for kw in keywords {
        items.push(json!({
            "label": kw,
            "kind": 14, // Keyword
            "detail": "Keyword",
        }));
    }

    let builtins = ["Int", "Float", "String", "Bool", "Void", "Result", "Patch"];
    for b in builtins {
        items.push(json!({
            "label": b,
            "kind": 7, // Class/Type
            "detail": "Built-in Type",
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
    fn test_completions() {
        let code = "type User = { id: Int }\n";
        let completions = get_completions(code, 1, 0, None);
        assert!(completions.iter().any(|c| c["label"] == "service"));
        assert!(completions.iter().any(|c| c["label"] == "User"));
    }

    #[test]
    fn test_definition() {
        let code = "type User = { id: Int }\n";
        let def = get_definition("file:///test.link", code, 0, 6, None).expect("Debe encontrar definición");
        assert_eq!(def["uri"], "file:///test.link");
        assert_eq!(def["range"]["start"]["line"], 0);
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
