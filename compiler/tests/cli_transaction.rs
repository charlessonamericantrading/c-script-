// `transaction { ... }` (GRAMMAR.md §3.154): transacciones SQL reales
// multi-escritura para `db{}`. Se prueba acá, contra el BINARIO real, el
// mecanismo que un test in-process no puede ejercitar por sí solo: que un
// `stream` conectado por un socket real NUNCA reciba el evento de una
// escritura que después se rollbackea, y sí reciba exactamente uno para
// una que confirma -- la publicación diferida (`Db::commit_transaction`)
// solo se puede probar de punta a punta con una conexión SSE de verdad.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

const PROGRAM: &str = r#"
type Order = { id: Int, productId: Int, qty: Int }
type Stock = { id: Int, productId: Int, quantity: Int }
db { orders: Order[], stock: Stock[] }

service Shop {
  rpc seedStock(productId: Int, qty: Int) -> Stock {
    db.stock.insert(Stock { id: 0, productId: productId, quantity: qty })
  }

  rpc checkout(productId: Int, qty: Int) -> Order {
    transaction {
      let matches = db.stock.findWhere(|s: Stock| { s.productId == productId });
      if matches.length() == 0 {
        panic("sin stock para ese producto");
      } else {
      }
      let s = matches[0];
      if s.quantity < qty {
        panic("stock insuficiente");
      } else {
      }
      db.stock.increment(s.id, |x: Stock| { x.quantity }, 0 - qty);
      db.orders.insert(Order { id: 0, productId: productId, qty: qty })
    }
  }

  rpc stockFor(productId: Int) -> Int {
    let matches = db.stock.findWhere(|s: Stock| { s.productId == productId });
    matches[0].quantity
  }

  stream watchOrders() -> Order {
    while true {
      db.orders.subscribe()
    }
  }
}
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "linkc-transaction-{name}-{}-{}",
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
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0)).expect("bindear puerto efímero").local_addr().unwrap().port()
}

fn wait_for_port(port: u16) {
    let mut buf = [0u8; 1];
    for _ in 0..200 {
        if let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) {
            let ready = stream
                .write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
                .is_ok()
                && matches!(stream.read(&mut buf), Ok(n) if n > 0);
            if ready {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("'linkc serve' no abrió el puerto {port} a tiempo");
}

struct Serve {
    child: Child,
    port: u16,
}

impl Serve {
    fn start(link_path: &PathBuf) -> Self {
        let port = free_port();
        let child = Command::new(env!("CARGO_BIN_EXE_linkc"))
            .arg("serve")
            .arg(link_path)
            .arg(port.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("iniciar 'linkc serve'");
        wait_for_port(port);
        Serve { child, port }
    }

    fn rpc(&self, path: &str, body: &str) -> (u16, serde_json::Value) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("conectar");
        let request = format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            self.port,
            body.len()
        );
        stream.write_all(request.as_bytes()).expect("escribir request");
        stream.flush().ok();

        let mut reader = BufReader::new(stream);
        let mut status_line = String::new();
        reader.read_line(&mut status_line).expect("línea de estado");
        let status: u16 = status_line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);

        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            let n = reader.read_line(&mut line).expect("header");
            if n == 0 || line.trim().is_empty() {
                break;
            }
            if let Some((k, v)) = line.trim().split_once(':') {
                if k.trim().eq_ignore_ascii_case("content-length") {
                    content_length = v.trim().parse().unwrap_or(0);
                }
            }
        }
        let mut buf = vec![0u8; content_length];
        reader.read_exact(&mut buf).ok();
        let json = if buf.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&buf).expect("el body debe ser JSON") };
        (status, json)
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Conexión SSE real, abierta y mantenida viva -- lee eventos `data: ...`
/// uno por uno con un timeout corto, para poder afirmar tanto "llegó esto"
/// como "no llegó nada" sin colgarse para siempre esperando un evento que
/// nunca va a aparecer (el caso exacto que este archivo prueba).
struct StreamClient {
    reader: BufReader<TcpStream>,
}

impl StreamClient {
    fn connect(port: u16, path: &str) -> Self {
        let stream = TcpStream::connect(("127.0.0.1", port)).expect("conectar al stream");
        stream.set_read_timeout(Some(Duration::from_millis(800))).unwrap();
        let body = "{}";
        let request = format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let mut stream = stream;
        stream.write_all(request.as_bytes()).expect("escribir request");
        stream.flush().ok();

        let mut reader = BufReader::new(stream);
        let mut status_line = String::new();
        reader.read_line(&mut status_line).expect("línea de estado del stream");
        assert!(status_line.contains("200"), "el stream no arrancó bien: {status_line}");
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).expect("header del stream");
            if line.trim().is_empty() {
                break;
            }
        }
        StreamClient { reader }
    }

    /// Un evento real (`data: {...}`) parseado a JSON, o `None` si no llegó
    /// nada dentro del timeout -- la señal que este archivo necesita para
    /// afirmar "un checkout fallido no generó ningún evento".
    fn next_event(&mut self) -> Option<serde_json::Value> {
        loop {
            let mut line = String::new();
            match self.reader.read_line(&mut line) {
                Ok(0) => return None,
                Ok(_) => {
                    let trimmed = line.trim();
                    if let Some(data) = trimmed.strip_prefix("data: ") {
                        return serde_json::from_str(data).ok();
                    }
                    // Líneas vacías entre eventos, comentarios SSE, etc. --
                    // seguir leyendo hasta el próximo `data: ` real o el
                    // timeout.
                }
                Err(_) => return None, // timeout -- read_timeout expiró
            }
        }
    }
}

/// GRAMMAR.md §3.154, el caso real que motivó el ítem: un checkout que
/// rollbackea por falta de stock NUNCA debe anunciarse a un `stream`
/// suscripto a `orders` -- publicar ahí una fila que la base nunca aceptó
/// de verdad le mentiría a cualquier cliente en vivo escuchando.
#[test]
fn a_rolled_back_transaction_never_reaches_a_live_stream_subscriber() {
    let temp = TempDir::new("stream-no-event-on-rollback");
    let link = temp.write("app.link", PROGRAM);
    let server = Serve::start(&link);

    server.rpc("/Shop/seedStock", r#"{"productId":1,"qty":2}"#);
    let mut watcher = StreamClient::connect(server.port, "/Shop/watchOrders");
    // El snapshot inicial de `subscribe()` -- la colección "orders" empieza
    // vacía, así que no hay ningún evento de snapshot que drenar acá.

    let (status, body) = server.rpc("/Shop/checkout", r#"{"productId":1,"qty":999}"#);
    assert_eq!(status, 500, "stock insuficiente debe fallar: {body:?}");

    assert!(watcher.next_event().is_none(), "un checkout que hizo rollback NO debe generar ningún evento de stream");

    // Confirma que el stream sigue vivo y funcionando de verdad -- si
    // estuviera roto de alguna forma, el `None` de arriba sería falso
    // positivo (cualquier cosa daría None). Un checkout exitoso después sí
    // debe llegar.
    let (status, order) = server.rpc("/Shop/checkout", r#"{"productId":1,"qty":1}"#);
    assert_eq!(status, 200, "{order:?}");
    let event = watcher.next_event().expect("el checkout exitoso sí debe generar un evento de stream real");
    assert_eq!(event["productId"], serde_json::json!(1), "{event:?}");
    assert_eq!(event["qty"], serde_json::json!(1), "{event:?}");
}

#[test]
fn a_successful_transaction_over_http_persists_and_a_failed_one_leaves_no_trace() {
    let temp = TempDir::new("http-commit-rollback");
    let link = temp.write("app.link", PROGRAM);
    let server = Serve::start(&link);

    server.rpc("/Shop/seedStock", r#"{"productId":5,"qty":4}"#);

    let (status, body) = server.rpc("/Shop/checkout", r#"{"productId":5,"qty":999}"#);
    assert_eq!(status, 500, "{body:?}");
    let (_, stock) = server.rpc("/Shop/stockFor", r#"{"productId":5}"#);
    assert_eq!(stock, serde_json::json!(4), "el rollback no debe haber tocado el stock");

    let (status, order) = server.rpc("/Shop/checkout", r#"{"productId":5,"qty":3}"#);
    assert_eq!(status, 200, "{order:?}");
    assert_eq!(order["qty"], serde_json::json!(3), "{order:?}");
    let (_, stock) = server.rpc("/Shop/stockFor", r#"{"productId":5}"#);
    assert_eq!(stock, serde_json::json!(1), "el commit sí debe reflejarse: 4 - 3 = 1");
}
