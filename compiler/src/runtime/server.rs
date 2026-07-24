// Servidor HTTP mínimo que expone cada `rpc` como POST /{Service}/{method},
// leyendo argumentos como un objeto JSON (el mismo shape que produce
// client.ts) y devolviendo el resultado serializado. CORS abierto porque el
// frontend de la demo corre en otro puerto (ver examples/frontend/).

use super::db::Db;
use super::invoke_rpc;
use crate::ast::Program;

pub fn serve(program: Program, port: u16) {
    let server = tiny_http::Server::http(("0.0.0.0", port))
        .unwrap_or_else(|e| panic!("no se pudo iniciar el servidor en el puerto {port}: {e}"));
    let db = Db::seeded();
    println!("c-script server escuchando en http://localhost:{port}  (Ctrl+C para detener)");

    for mut request in server.incoming_requests() {
        if *request.method() == tiny_http::Method::Options {
            let _ = request.respond(cors_response(204, String::new()));
            continue;
        }

        let mut body = String::new();
        let _ = request.as_reader().read_to_string(&mut body);
        let path = request.url().to_string();

        let (status, response_body) = handle(&program, &db, &path, &body);
        let _ = request.respond(cors_response(status, response_body));
    }
}

fn handle(program: &Program, db: &Db, path: &str, body: &str) -> (u16, String) {
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let [service_name, rpc_name] = segments.as_slice() else {
        return (404, error_json("URL debe tener la forma /Service/method"));
    };

    let args_json: serde_json::Value = if body.trim().is_empty() {
        serde_json::json!({})
    } else {
        match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return (400, error_json(&format!("JSON inválido: {e}"))),
        }
    };

    match invoke_rpc(program, service_name, rpc_name, &args_json, db) {
        Ok(result) => (200, result.to_string()),
        Err(e) => (500, error_json(&e.to_string())),
    }
}

fn error_json(message: &str) -> String {
    serde_json::json!({ "error": message }).to_string()
}

fn cors_response(status: u16, body: String) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    let content_type = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
    let allow_origin = tiny_http::Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]).unwrap();
    let allow_methods =
        tiny_http::Header::from_bytes(&b"Access-Control-Allow-Methods"[..], &b"POST, OPTIONS"[..]).unwrap();
    let allow_headers =
        tiny_http::Header::from_bytes(&b"Access-Control-Allow-Headers"[..], &b"Content-Type"[..]).unwrap();
    tiny_http::Response::from_string(body)
        .with_status_code(status)
        .with_header(content_type)
        .with_header(allow_origin)
        .with_header(allow_methods)
        .with_header(allow_headers)
}
