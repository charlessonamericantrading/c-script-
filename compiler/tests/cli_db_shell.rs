// `linkc db shell` (GRAMMAR.md §3.189) como SUBPROCESO REAL: un REPL de
// solo lectura, una línea de SQL por consulta, con un separador fijo
// (`--fin--`) al final de cada respuesta -- ver `db_admin.rs::run_shell_loop`.
// Esto prueba lo que un test in-process de `run_query_sqlite` no puede: el
// framing línea-por-línea sobre pipes de sistema operativo de verdad, que
// SQLite realmente rechace una escritura en una conexión `SQLITE_OPEN_READ_ONLY`
// (no solo que el código lo intente), y que `.exit`/línea vacía/cierre de
// stdin terminen el proceso limpio -- mismo criterio que `lsp_stdio.rs`.

use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

const PROGRAM: &str = r#"
type Item = { id: Int, name: String, price: Decimal }
type NewItem = { name: String, price: Decimal }
db { items: Item[] }
service Items {
  rpc add(name: String, price: Decimal) -> Item { db.items.insert(NewItem { name: name, price: price }) }
}
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "linkc-db-shell-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&path).expect("crear tempdir");
        Self(path)
    }

    fn write(&self, name: &str, content: &str) -> PathBuf {
        let full = self.0.join(name);
        std::fs::write(&full, content).unwrap();
        full
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Puebla una base SQLite real vía `linkc serve` -- mismo criterio que
/// `cli_db_inspect.rs`: una base armada por el runtime real, no un `.db`
/// escrito a mano, porque lo que este archivo prueba es el camino REAL de
/// filas físicas hasta la salida del shell.
fn seed(link_path: &PathBuf, db_path: &PathBuf) {
    let port = {
        std::net::TcpListener::bind(("127.0.0.1", 0)).expect("puerto efímero").local_addr().unwrap().port()
    };
    let mut child = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(link_path)
        .arg(port.to_string())
        .arg("--db")
        .arg(db_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("iniciar 'linkc serve'");
    for _ in 0..200 {
        if ureq::get(&format!("http://127.0.0.1:{port}/health")).call().is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    ureq::post(&format!("http://127.0.0.1:{port}/Items/add"))
        .set("Content-Type", "application/json")
        .send_string(r#"{"name":"Widget","price":"19.9900"}"#)
        .unwrap_or_else(|e| panic!("Items/add falló: {e}"));
    let _ = child.kill();
    let _ = child.wait();
}

/// Cliente del REPL real: escribe una línea de SQL a stdin, lee líneas de
/// stdout hasta el separador `--fin--` que `run_shell_loop` imprime después
/// de cada respuesta. El prompt (`"db> "`) se imprime SIN salto de línea
/// antes de bloquear en la próxima lectura -- por eso queda pegado al
/// principio de la primera línea de cada respuesta, y hay que despegarlo acá
/// en vez de asumir que cada línea de stdout es contenido puro.
struct ShellProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: std::io::BufReader<ChildStdout>,
}

impl ShellProcess {
    fn start(link_path: &PathBuf, db_path: &PathBuf) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_linkc"))
            .arg("db")
            .arg("shell")
            .arg(link_path)
            .arg("--db")
            .arg(db_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("no se pudo iniciar 'linkc db shell'");
        let stdin = child.stdin.take().expect("stdin del proceso hijo");
        let stdout = std::io::BufReader::new(child.stdout.take().expect("stdout del proceso hijo"));
        ShellProcess { child, stdin, stdout }
    }

    fn send(&mut self, sql: &str) {
        use std::io::Write;
        writeln!(self.stdin, "{sql}").expect("escribir la consulta al stdin del hijo");
        self.stdin.flush().expect("flush del stdin del hijo");
    }

    /// Devuelve las líneas de la respuesta (sin el prompt pegado ni el
    /// separador final).
    fn recv(&mut self) -> Vec<String> {
        use std::io::BufRead;
        let mut lines = Vec::new();
        let mut first = true;
        loop {
            let mut line = String::new();
            let n = self.stdout.read_line(&mut line).expect("leer una línea del stdout del hijo");
            assert_ne!(n, 0, "el proceso hijo cerró stdout antes de mandar '--fin--'");
            let mut content = line.trim_end_matches(['\r', '\n']).to_string();
            if first {
                content = content.strip_prefix("db> ").unwrap_or(&content).to_string();
                first = false;
            }
            if content == "--fin--" {
                return lines;
            }
            lines.push(content);
        }
    }

    /// Cierra stdin (el shell ve EOF en su loop y termina solo, mismo
    /// criterio que `linkc lsp`) y espera a que el proceso hijo salga.
    fn shutdown(mut self) {
        drop(self.stdin);
        let status = self.child.wait().expect("esperar a que 'linkc db shell' termine");
        assert!(status.success(), "linkc db shell debería salir limpio (código 0) al ver EOF en stdin, salió con {status:?}");
    }
}

#[test]
fn a_real_select_against_data_seeded_by_linkc_serve_returns_the_actual_row() {
    let temp = TempDir::new("select");
    let src = temp.write("app.link", PROGRAM);
    let db_path = temp.path("app.db");
    seed(&src, &db_path);

    let mut shell = ShellProcess::start(&src, &db_path);
    shell.send("SELECT id, name FROM items;");
    let lines = shell.recv();
    let joined = lines.join("\n");
    assert!(joined.contains("Widget"), "debe devolver la fila real sembrada por linkc serve: {joined:?}");
    assert!(joined.contains("1 fila(s)"), "debe reportar el conteo de filas: {joined:?}");
    shell.shutdown();
}

#[test]
fn an_insert_attempt_is_rejected_by_sqlites_own_read_only_connection() {
    let temp = TempDir::new("insert-rejected");
    let src = temp.write("app.link", PROGRAM);
    let db_path = temp.path("app.db");
    seed(&src, &db_path);

    let mut shell = ShellProcess::start(&src, &db_path);
    shell.send("INSERT INTO items (name, price) VALUES ('hack', 1);");
    let lines = shell.recv();
    let joined = lines.join("\n");
    assert!(joined.starts_with("error:"), "un intento de escritura debe reportarse como error, no como resultado: {joined:?}");
    assert!(
        joined.contains("readonly") || joined.contains("read-only") || joined.contains("read only"),
        "el mensaje debe nombrar la causa real (conexión de solo lectura), no un error genérico: {joined:?}"
    );

    // El rechazo de la escritura no debe tumbar el loop -- la conexión sigue
    // sirviendo consultas después.
    shell.send("SELECT count(*) FROM items;");
    let lines2 = shell.recv();
    assert!(lines2.join("\n").contains("1 fila(s)"), "el shell debe seguir respondiendo después de un error: {lines2:?}");
    shell.shutdown();
}

#[test]
fn a_syntactically_invalid_query_reports_a_clean_error_and_the_loop_keeps_going() {
    let temp = TempDir::new("bad-sql");
    let src = temp.write("app.link", PROGRAM);
    let db_path = temp.path("app.db");
    seed(&src, &db_path);

    let mut shell = ShellProcess::start(&src, &db_path);
    shell.send("NOT VALID SQL AT ALL");
    let lines = shell.recv();
    assert!(lines.join("\n").starts_with("error:"), "SQL inválido debe reportarse como error, no crashear: {lines:?}");

    shell.send("SELECT 1;");
    let lines2 = shell.recv();
    assert!(!lines2.is_empty(), "el loop debe seguir sirviendo consultas después de un error de sintaxis: {lines2:?}");
    shell.shutdown();
}

#[test]
fn dot_exit_terminates_the_process_cleanly() {
    let temp = TempDir::new("dot-exit");
    let src = temp.write("app.link", PROGRAM);
    let db_path = temp.path("app.db");
    seed(&src, &db_path);

    let mut shell = ShellProcess::start(&src, &db_path);
    shell.send(".exit");
    let status = shell.child.wait().expect("esperar a que 'linkc db shell' termine");
    assert!(status.success(), "'.exit' debe terminar el proceso con código 0, salió con {status:?}");
}

#[test]
fn an_empty_line_terminates_the_process_cleanly() {
    let temp = TempDir::new("empty-line");
    let src = temp.write("app.link", PROGRAM);
    let db_path = temp.path("app.db");
    seed(&src, &db_path);

    let mut shell = ShellProcess::start(&src, &db_path);
    shell.send("");
    let status = shell.child.wait().expect("esperar a que 'linkc db shell' termine");
    assert!(status.success(), "una línea vacía debe terminar el proceso con código 0, salió con {status:?}");
}

#[test]
fn closing_stdin_without_any_query_terminates_the_process_cleanly() {
    let temp = TempDir::new("eof");
    let src = temp.write("app.link", PROGRAM);
    let db_path = temp.path("app.db");
    seed(&src, &db_path);

    let shell = ShellProcess::start(&src, &db_path);
    shell.shutdown();
}
