// Tests de integración: `linkc lsp` como SUBPROCESO REAL, no llamadas
// in-process a `handle_message()` (`compiler/src/lsp.rs` ya tiene sus
// propios tests unitarios para eso). Esto cubre lo que esos no pueden --
// framing real de `Content-Length` sobre pipes de sistema operativo de
// verdad (particularmente en Windows), el bucle lector/escritor de
// `run_stdio()` contra handles reales, y que el BINARIO compilado (no
// solo la librería) expone el subcomando `lsp` correctamente. Mismo
// requisito que el plan aprobado de LSP pedía como "capa secundaria,
// subproceso real" -- ver GRAMMAR.md §3.19.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

struct LspProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl LspProcess {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_linkc"))
            .arg("lsp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null()) // no nos importa acá; null evita cualquier riesgo de bloqueo por buffer lleno
            .spawn()
            .expect("no se pudo iniciar 'linkc lsp'");
        let stdin = child.stdin.take().expect("stdin del proceso hijo");
        let stdout = BufReader::new(child.stdout.take().expect("stdout del proceso hijo"));
        LspProcess { child, stdin, stdout }
    }

    fn send(&mut self, msg: &Value) {
        let body = serde_json::to_string(msg).expect("serializar el mensaje JSON-RPC");
        write!(self.stdin, "Content-Length: {}\r\n\r\n{}", body.len(), body).expect("escribir al stdin del hijo");
        self.stdin.flush().expect("flush del stdin del hijo");
    }

    fn recv(&mut self) -> Value {
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            let n = self.stdout.read_line(&mut line).expect("leer un header del stdout del hijo");
            assert_ne!(n, 0, "el proceso hijo cerró stdout antes de mandar una respuesta completa");
            let trimmed = line.trim();
            if trimmed.is_empty() {
                break;
            }
            if let Some((k, v)) = trimmed.split_once(':') {
                if k.trim().eq_ignore_ascii_case("content-length") {
                    content_length = v.trim().parse().expect("Content-Length debe ser numérico");
                }
            }
        }
        let mut buf = vec![0u8; content_length];
        self.stdout.read_exact(&mut buf).expect("leer el body del stdout del hijo");
        serde_json::from_slice(&buf).expect("el body debe ser JSON válido")
    }

    fn initialize(&mut self) {
        self.send(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}));
        let resp = self.recv();
        assert_eq!(resp["id"], 1, "initialize debe responder con el mismo id");
    }

    fn did_open(&mut self, uri: &str, text: &str) -> Value {
        self.send(&json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": { "textDocument": { "uri": uri, "languageId": "c-script", "version": 1, "text": text } }
        }));
        self.recv()
    }

    fn definition(&mut self, uri: &str, line: u64, character: u64) -> Value {
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": 99,
            "method": "textDocument/definition",
            "params": { "textDocument": { "uri": uri }, "position": { "line": line, "character": character } }
        }));
        self.recv()
    }

    /// Cierra stdin (el servidor ve EOF en su loop de `run_stdio` y
    /// termina solo) y espera a que el proceso hijo salga -- no dejar
    /// ningún `linkc.exe` huérfano corriendo después de un test.
    fn shutdown(mut self) {
        drop(self.stdin);
        let status = self.child.wait().expect("esperar a que 'linkc lsp' termine");
        assert!(status.success(), "linkc lsp debería salir limpio (código 0) al ver EOF en stdin, salió con {status:?}");
    }
}

/// Misma lógica que `linkc::lsp::path_to_uri` (que es `pub` en la
/// librería) -- se reimplementa mínimamente acá en vez de importarla para
/// no acoplar el test a un detalle interno del módulo, dado que lo único
/// que hace falta es construir un URI `file://` válido para una ruta ya
/// canonicalizada, que es exactamente lo que un editor real manda.
fn path_to_uri(path: &Path) -> String {
    let mut s = path.to_string_lossy().replace('\\', "/");
    if !s.starts_with('/') {
        s = format!("/{s}");
    }
    format!("file://{s}")
}

fn examples_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../examples")
}

#[test]
fn initialize_returns_the_expected_capabilities_over_a_real_subprocess() {
    let mut proc = LspProcess::start();
    proc.initialize();
    proc.send(&json!({"jsonrpc": "2.0", "id": 2, "method": "initialize", "params": {}}));
    let resp = proc.recv();
    assert_eq!(resp["result"]["capabilities"]["textDocumentSync"], 1);
    assert_eq!(resp["result"]["capabilities"]["hoverProvider"], true);
    assert_eq!(resp["result"]["capabilities"]["definitionProvider"], true);
    proc.shutdown();
}

#[test]
fn opening_the_real_flagship_example_produces_no_diagnostics_over_a_real_subprocess() {
    let path = examples_dir().join("users.link");
    let text = std::fs::read_to_string(&path).expect("leer examples/users.link");
    let canon = std::fs::canonicalize(&path).expect("canonicalizar examples/users.link");
    let uri = path_to_uri(&canon);

    let mut proc = LspProcess::start();
    proc.initialize();
    let diag_notif = proc.did_open(&uri, &text);
    assert_eq!(diag_notif["method"], "textDocument/publishDiagnostics");
    assert_eq!(diag_notif["params"]["uri"], uri);
    let diags = diag_notif["params"]["diagnostics"].as_array().expect("diagnostics debe ser un array");
    assert!(diags.is_empty(), "examples/users.link es el demo insignia -- no debería dar ningún diagnóstico: {diags:?}");
    proc.shutdown();
}

#[test]
fn opening_a_file_with_a_real_syntax_error_reports_it_over_a_real_subprocess() {
    let dir = std::env::temp_dir().join(format!("cscript-lsp-integration-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("crear directorio temporal");
    let path = dir.join("broken.link");
    std::fs::write(&path, "type User = { id Int }").expect("escribir archivo roto"); // falta ':'
    let canon = std::fs::canonicalize(&path).expect("canonicalizar broken.link");
    let uri = path_to_uri(&canon);
    let text = std::fs::read_to_string(&path).unwrap();

    let mut proc = LspProcess::start();
    proc.initialize();
    let diag_notif = proc.did_open(&uri, &text);
    let diags = diag_notif["params"]["diagnostics"].as_array().expect("diagnostics debe ser un array");
    assert!(!diags.is_empty(), "un archivo con un error de sintaxis real debe reportar al menos un diagnóstico");
    assert!(
        diags[0]["message"].as_str().unwrap_or("").contains("Colon"),
        "el mensaje debería nombrar el token esperado: {diags:?}"
    );
    proc.shutdown();

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_valid_import_across_two_real_files_produces_no_false_positive_over_a_real_subprocess() {
    // El bug concreto que esta ronda de trabajo arregló: antes de conectar
    // lsp.rs a modules::load_program_with_overlay, cualquier archivo con
    // `import` daba "no declarado" en falso porque el LSP nunca resolvía
    // el archivo importado -- acá se prueba contra el BINARIO real, no
    // solo la llamada in-process a handle_message que ya cubren los tests
    // unitarios de lsp.rs.
    let dir = std::env::temp_dir().join(format!("cscript-lsp-integration-import-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("crear directorio temporal");
    std::fs::write(dir.join("b.link"), "type Point = { x: Int, y: Int }").expect("escribir b.link");
    let path_a = dir.join("a.link");
    let text_a = r#"import { Point } from "./b.link"; fn origin() -> Point { Point { x: 0, y: 0 } }"#;
    std::fs::write(&path_a, text_a).expect("escribir a.link");
    let canon_a = std::fs::canonicalize(&path_a).expect("canonicalizar a.link");
    let uri_a = path_to_uri(&canon_a);

    let mut proc = LspProcess::start();
    proc.initialize();
    let diag_notif = proc.did_open(&uri_a, text_a);
    let diags = diag_notif["params"]["diagnostics"].as_array().expect("diagnostics debe ser un array");
    assert!(diags.is_empty(), "un import válido a un archivo real no debería dar ningún diagnóstico: {diags:?}");
    proc.shutdown();

    let _ = std::fs::remove_dir_all(&dir);
}

// ---- goto-def de un nombre de tipo en una firma (Nivel 3, GRAMMAR.md §3.21) ----

#[test]
fn goto_def_across_files_returns_null_instead_of_a_false_position_over_a_real_subprocess() {
    // Un tipo usado en la firma de un archivo, declarado en OTRO archivo
    // real -- con el gate de más-de-un-archivo-tocado (mismo criterio que
    // `compute_diagnostics_for_inner` ya usa para diagnósticos), debe dar
    // `null` en vez de una posición falsa calculada sobre el archivo
    // equivocado.
    let dir = std::env::temp_dir().join(format!("cscript-lsp-integration-gotodef-crossfile-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("crear directorio temporal");
    std::fs::write(dir.join("b.link"), "type Point = { x: Int, y: Int }").expect("escribir b.link");
    let path_a = dir.join("a.link");
    let text_a = r#"import { Point } from "./b.link"; fn origin() -> Point { Point { x: 0, y: 0 } }"#;
    std::fs::write(&path_a, text_a).expect("escribir a.link");
    let canon_a = std::fs::canonicalize(&path_a).expect("canonicalizar a.link");
    let uri_a = path_to_uri(&canon_a);

    let mut proc = LspProcess::start();
    proc.initialize();
    proc.did_open(&uri_a, text_a);
    let col = text_a.find("-> Point").expect("el texto de prueba debe contener '-> Point'") as u64 + 3;
    let resp = proc.definition(&uri_a, 0, col);
    assert!(resp["result"].is_null(), "cruzando archivos, nunca se debe arriesgar una posición: {resp:?}");
    proc.shutdown();

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn goto_def_on_a_type_name_within_the_same_file_returns_the_exact_range_over_a_real_subprocess() {
    let dir = std::env::temp_dir().join(format!("cscript-lsp-integration-gotodef-samefile-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("crear directorio temporal");
    let path = dir.join("shapes.link");
    let text = "type Point = { x: Int, y: Int }\nfn origin() -> Point { Point { x: 0, y: 0 } }\n";
    std::fs::write(&path, text).expect("escribir shapes.link");
    let canon = std::fs::canonicalize(&path).expect("canonicalizar shapes.link");
    let uri = path_to_uri(&canon);

    let mut proc = LspProcess::start();
    proc.initialize();
    proc.did_open(&uri, text);
    let line1 = text.lines().nth(1).expect("el texto de prueba debe tener una segunda línea");
    let col = line1.find("-> Point").expect("la segunda línea debe contener '-> Point'") as u64 + 3;
    let resp = proc.definition(&uri, 1, col);
    assert_eq!(resp["result"]["uri"], uri);
    assert_eq!(resp["result"]["range"]["start"]["line"], 0, "debe apuntar a 'type Point' en la línea 0: {resp:?}");
    proc.shutdown();

    let _ = std::fs::remove_dir_all(&dir);
}
