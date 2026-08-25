//! Generador de especificación OpenAPI 3.1 (openapi.json) a partir de servicios Link.
//! Permite documentación Swagger UI interactiva, generación de SDKs y testing en Postman.

use std::collections::BTreeMap;
use serde_json::{json, Value};
use crate::ast::{Expr, FieldValidator, Item, Member, Program, TypeExpr, UnaryOp};
use crate::checker::Checker;
use crate::types::Type;

/// El valor JSON de un `= default` (GRAMMAR.md §3.74), si es un literal
/// escalar simple -- `None` para cualquier otra expresión (una llamada como
/// `crypto.uuid()`, una referencia a `const`, un `struct`/array) que no
/// tiene una forma JSON fija conocida en compilación sin evaluarla.
fn scalar_literal_json(e: &Expr) -> Option<Value> {
    match e {
        Expr::Int(n) => Some(json!(n)),
        Expr::Float(n) => Some(json!(n)),
        Expr::Str(s) => Some(json!(s)),
        Expr::Bool(b) => Some(json!(b)),
        Expr::Null => Some(Value::Null),
        _ => None,
    }
}

/// Convierte una expresión de `@example(request: ..., response: ...)`
/// (GRAMMAR.md §3.119) a JSON -- el checker ya garantizó, con
/// `is_literal_expr` (checker.rs), que `e` es un valor literal (nunca una
/// llamada/variable/`db`/etc.), así que esta conversión es total: cualquier
/// nodo no cubierto explícitamente es letra muerta en la práctica, y cae a
/// `Value::Null` en vez de entrar en pánico -- un ejemplo mal armado no
/// tiene por qué tirar abajo `linkc build` en un `unwrap`. `StructLit`
/// ignora `name`/`variant` (solo importan para el checker, que ya validó el
/// tipo) y emite un objeto plano con sus campos -- mismo criterio que
/// `type_to_json_schema` usa para un struct anónimo.
fn literal_expr_to_json(e: &Expr) -> Value {
    match e {
        Expr::Int(n) => json!(n),
        Expr::Float(n) => json!(n),
        Expr::Str(s) => json!(s),
        Expr::Bool(b) => json!(b),
        Expr::Null => Value::Null,
        Expr::Unary { op: UnaryOp::Neg, operand } => match &operand.node {
            Expr::Int(n) => json!(-n),
            Expr::Float(n) => json!(-n),
            _ => Value::Null,
        },
        Expr::ArrayLit(items) | Expr::TupleLit(items) => Value::Array(items.iter().map(|i| literal_expr_to_json(&i.node)).collect()),
        Expr::StructLit { fields, .. } => {
            let mut obj = serde_json::Map::new();
            for (name, value) in fields {
                obj.insert(name.clone(), literal_expr_to_json(&value.node));
            }
            Value::Object(obj)
        }
        _ => Value::Null,
    }
}

fn type_to_json_schema(ty: &Type) -> Value {
    match ty {
        Type::Int => json!({ "type": "integer", "format": "int32" }),
        Type::Int64 => json!({ "type": "integer", "format": "int64" }),
        Type::Float => json!({ "type": "number", "format": "double" }),
        Type::String => json!({ "type": "string" }),
        // "format": "uuid" es el idiom estándar de JSON Schema/OpenAPI para
        // esto -- ningún `pattern` propio hace falta, a diferencia de
        // validators.ts/schemas.ts (que sí necesitan su propia regex,
        // GRAMMAR.md §3.70).
        Type::Uuid => json!({ "type": "string", "format": "uuid" }),
        Type::Bool => json!({ "type": "boolean" }),
        Type::Void => json!({ "type": "null" }),
        Type::Timestamp => json!({ "type": "string", "format": "date-time" }),
        Type::Optional(inner) => {
            let inner_schema = type_to_json_schema(inner);
            json!({
                "anyOf": [inner_schema, { "type": "null" }]
            })
        }
        Type::List(elem) => {
            json!({
                "type": "array",
                "items": type_to_json_schema(elem)
            })
        }
        Type::MapOf(_, v) => {
            json!({
                "type": "object",
                "additionalProperties": type_to_json_schema(v)
            })
        }
        Type::Struct { name: Some(n), .. } => {
            json!({ "$ref": format!("#/components/schemas/{}", n) })
        }
        Type::Struct { name: None, fields } => {
            let mut props = json!({});
            let mut required = Vec::new();
            for f in fields {
                props[f.name.as_str()] = type_to_json_schema(&f.ty);
                if !f.optional {
                    required.push(f.name.clone());
                }
            }
            json!({
                "type": "object",
                "properties": props,
                "required": required
            })
        }
        Type::Enum(name) => {
            json!({ "$ref": format!("#/components/schemas/{}", name) })
        }
        Type::ResultOf(ok_ty, err_ty) => {
            json!({
                "type": "object",
                "properties": {
                    "ok": { "type": "boolean" },
                    "value": type_to_json_schema(ok_ty),
                    "error": type_to_json_schema(err_ty)
                },
                "required": ["ok"]
            })
        }
        Type::Union(members) => {
            let schemas: Vec<_> = members.iter().map(type_to_json_schema).collect();
            json!({ "anyOf": schemas })
        }
        _ => json!({ "type": "object" }),
    }
}

pub fn emit_openapi_json(program: &Program, title: &str) -> Result<String, String> {
    let (checker, errors) = Checker::build_symbols(program);
    if let Some(e) = errors.into_iter().next() {
        return Err(e.to_string());
    }

    let mut schemas = BTreeMap::new();

    for item in &program.items {
        match item {
            Item::Enum(e) => {
                let variants: Vec<Value> = e.variants.iter().map(|v| json!(v.name)).collect();
                schemas.insert(
                    e.name.clone(),
                    json!({
                        "type": "string",
                        "enum": variants
                    }),
                );
            }
            Item::Type(t) => {
                if let TypeExpr::Struct(fields) = &t.ty {
                    let mut props = json!({});
                    let mut required = Vec::new();
                    for f in fields {
                        let ty = checker.resolve_type(&f.ty).map_err(|e| e.to_string())?;
                        let mut schema = type_to_json_schema(&ty);
                        // `@deprecated` sobre un campo (GRAMMAR.md §3.71) --
                        // "deprecated" es una keyword estándar de JSON Schema
                        // 2020-12 (la que usa OpenAPI 3.1), así que no hace
                        // falta ninguna extensión propietaria.
                        if let Some(reason) = f.deprecated() {
                            if let Some(obj) = schema.as_object_mut() {
                                obj.insert("deprecated".to_string(), json!(true));
                                obj.insert("description".to_string(), json!(reason));
                            }
                        }
                        // `@validate(...)` sobre un campo (GRAMMAR.md §3.73)
                        // -- "format"/"pattern" son también keywords
                        // estándar de JSON Schema, no una extensión propia.
                        // Solo se aplica al campo directo, no dentro de
                        // `Optional` -- `type_to_json_schema` ya envuelve un
                        // `String?` en `anyOf` (ver arriba), donde no hay un
                        // único objeto de propiedades sobre el que escribir.
                        if let Some(v) = f.validator() {
                            if let Some(obj) = schema.as_object_mut() {
                                match v {
                                    FieldValidator::Email => {
                                        obj.insert("format".to_string(), json!("email"));
                                    }
                                    FieldValidator::Regex(pattern) => {
                                        obj.insert("pattern".to_string(), json!(pattern));
                                    }
                                }
                            }
                        }
                        // `= default` (GRAMMAR.md §3.74) -- un campo con
                        // default puede omitirse de un request body igual
                        // que uno `?:`, así que sale de `required`. Cuando
                        // el default es un literal simple (no una llamada
                        // como `crypto.uuid()`, que no tiene forma JSON
                        // fija) se suma además como `"default"` -- keyword
                        // estándar de JSON Schema, valor puramente
                        // informativo para quien lea el spec.
                        if let Some(default) = &f.default {
                            if let Some(obj) = schema.as_object_mut() {
                                if let Some(v) = scalar_literal_json(&default.node) {
                                    obj.insert("default".to_string(), v);
                                }
                            }
                        }
                        props[f.name.as_str()] = schema;
                        if !f.optional && f.default.is_none() {
                            required.push(f.name.clone());
                        }
                    }
                    schemas.insert(
                        t.name.clone(),
                        json!({
                            "type": "object",
                            "properties": props,
                            "required": required
                        }),
                    );
                }
            }
            _ => {}
        }
    }

    let mut paths = json!({});

    for item in &program.items {
        let Item::Service(service) = item else { continue };

        for member in &service.members {
            let (rpc, is_stream) = match member {
                Member::Rpc(r) => (r, false),
                Member::Stream(r) => (r, true),
            };

            let mut req_props = json!({});
            let mut req_required = Vec::new();
            for p in &rpc.params {
                let ty = checker.resolve_type(&p.ty).map_err(|e| e.to_string())?;
                req_props[p.name.as_str()] = type_to_json_schema(&ty);
                if p.default.is_none() {
                    req_required.push(p.name.clone());
                }
            }

            let ret_ty = checker.resolve_type(&rpc.return_type).map_err(|e| e.to_string())?;
            let res_schema = type_to_json_schema(&ret_ty);

            // Un rpc con `@content_type` responde ese tipo, no JSON
            // (GRAMMAR.md §3.35) -- si el spec dijera application/json, un
            // cliente generado desde este OpenAPI intentaría parsear el HTML.
            let response_content_type = if is_stream {
                "text/event-stream"
            } else {
                rpc.content_type().unwrap_or("application/json")
            };

            let mut operation = json!({
                "summary": format!("{}::{}", service.name, rpc.name),
                "tags": [service.name.clone()],
                "responses": {
                    "200": {
                        "description": if is_stream { "Server-Sent Events Stream" } else { "Respuesta exitosa" },
                        "content": {
                            response_content_type: {
                                "schema": res_schema
                            }
                        }
                    }
                }
            });

            // Docstring `///` (GRAMMAR.md §3.72) -> "description" del
            // Operation Object -- antes de esta ronda, el único texto que un
            // rpc tenía en el spec generado era su nombre.
            if let Some(doc) = &rpc.doc {
                operation["description"] = json!(doc);
            }

            // `@deprecated` sobre un rpc (GRAMMAR.md §3.71) -- "deprecated"
            // es una keyword nativa de Operation Object en OpenAPI 3.x. Si
            // YA hay un docstring, el motivo se agrega en vez de pisarlo --
            // las dos cosas coexisten (documentar por qué existe Y por qué
            // ya no usarlo son preguntas distintas).
            if let Some(reason) = rpc.deprecated() {
                operation["deprecated"] = json!(true);
                operation["description"] = json!(match &rpc.doc {
                    Some(doc) => format!("{doc}\n\nDeprecated: {reason}"),
                    None => reason.to_string(),
                });
            }

            if !rpc.params.is_empty() {
                operation["requestBody"] = json!({
                    "required": true,
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "object",
                                "properties": req_props,
                                "required": req_required
                            }
                        }
                    }
                });
            }

            // `@example(request: ..., response: ...)` (GRAMMAR.md §3.119) --
            // el checker ya validó las dos expresiones contra la forma real
            // del rpc (`request` contra sus params, `response` contra
            // `return_type`), así que acá solo hace falta convertirlas a
            // JSON y ponerlas donde OpenAPI espera un ejemplo: `"example"`
            // dentro del Media Type Object, mismo nivel que `"schema"` --
            // `request` solo puede estar presente si `rpc.params` no está
            // vacío (ya lo garantiza el checker), así que `requestBody` ya
            // existe acá cuando hace falta.
            if let Some((request, response)) = rpc.example() {
                if let Some(req_expr) = request {
                    operation["requestBody"]["content"]["application/json"]["example"] = literal_expr_to_json(&req_expr.node);
                }
                if let Some(res_expr) = response {
                    operation["responses"]["200"]["content"][response_content_type]["example"] = literal_expr_to_json(&res_expr.node);
                }
            }

            let path_key = format!("/{}/{}", service.name, rpc.name);
            paths[path_key.as_str()] = json!({
                "post": operation
            });
        }
    }

    let doc = json!({
        "openapi": "3.1.0",
        // `x-generated-by` (PLAN.md §9.7, GRAMMAR.md §3.83): extensión de
        // vendor ESTÁNDAR de OpenAPI (prefijo `x-`, cualquier herramienta la
        // ignora sin romper la validación del documento) -- no
        // `info.version`, que es la versión del API DOCUMENTADA (algo que
        // decide quien escribe el `.link`, ver GRAMMAR.md §3.83), no la del
        // compilador que lo generó.
        "x-generated-by": format!("linkc v{}", crate::VERSION),
        "info": {
            "title": title,
            "version": "1.0.0",
            "description": "API generada automáticamente por Link (c-script)"
        },
        "paths": paths,
        "components": {
            "schemas": schemas,
            "securitySchemes": {
                "BearerAuth": {
                    "type": "http",
                    "scheme": "bearer",
                    "bearerFormat": "opaque"
                }
            }
        }
    });

    serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer;
    use crate::parser;

    #[test]
    fn test_openapi_emits_valid_spec() {
        let code = r#"
            type Task = { id: Int, title: String }
            service Tasks {
                rpc list() -> Task[] { [] }
                rpc create(title: String) -> Task { Task { id: 1, title: title } }
            }
        "#;
        let tokens = lexer::tokenize(code).unwrap();
        let program = parser::parse(tokens).unwrap();
        let spec_str = emit_openapi_json(&program, "Task API").unwrap();
        let spec: Value = serde_json::from_str(&spec_str).unwrap();

        assert_eq!(spec["openapi"], "3.1.0");
        assert_eq!(spec["info"]["title"], "Task API");
        assert!(spec["paths"]["/Tasks/list"]["post"].is_object());
        assert!(spec["paths"]["/Tasks/create"]["post"]["requestBody"].is_object());
        assert!(spec["components"]["schemas"]["Task"].is_object());
        // PLAN.md §9.7, GRAMMAR.md §3.83: `x-generated-by` -- extensión de
        // vendor de OpenAPI, NUNCA `info.version` (esa es la versión del API
        // documentada, no la del compilador).
        assert_eq!(spec["x-generated-by"], format!("linkc v{}", crate::VERSION));
        assert_ne!(spec["info"]["version"], format!("linkc v{}", crate::VERSION), "info.version es del API, no del compilador");
    }

    /// `@deprecated` sobre un rpc se propaga como `deprecated: true` +
    /// `description` en el Operation Object (GRAMMAR.md §3.71) -- keyword
    /// nativa de OpenAPI, sin extensión propietaria.
    #[test]
    fn deprecated_rpc_sets_deprecated_true_on_the_operation() {
        let code = r#"
            service Tasks {
                @deprecated("usa listV2 en su lugar")
                rpc list() -> Int { 1 }
                rpc listV2() -> Int { 2 }
            }
        "#;
        let tokens = lexer::tokenize(code).unwrap();
        let program = parser::parse(tokens).unwrap();
        let spec_str = emit_openapi_json(&program, "Task API").unwrap();
        let spec: Value = serde_json::from_str(&spec_str).unwrap();

        assert_eq!(spec["paths"]["/Tasks/list"]["post"]["deprecated"], true);
        assert_eq!(spec["paths"]["/Tasks/list"]["post"]["description"], "usa listV2 en su lugar");
        assert!(spec["paths"]["/Tasks/listV2"]["post"]["deprecated"].is_null());
    }

    /// `@deprecated` sobre un campo se propaga como `deprecated: true` +
    /// `description` en el schema de esa propiedad (JSON Schema 2020-12,
    /// la base de OpenAPI 3.1).
    #[test]
    fn deprecated_field_sets_deprecated_true_on_the_property_schema() {
        let code = r#"
            type Lead = { id: Int, @deprecated("usa email") legacyPhone: String, email: String }
        "#;
        let tokens = lexer::tokenize(code).unwrap();
        let program = parser::parse(tokens).unwrap();
        let spec_str = emit_openapi_json(&program, "Task API").unwrap();
        let spec: Value = serde_json::from_str(&spec_str).unwrap();

        let props = &spec["components"]["schemas"]["Lead"]["properties"];
        assert_eq!(props["legacyPhone"]["deprecated"], true);
        assert_eq!(props["legacyPhone"]["description"], "usa email");
        assert!(props["email"]["deprecated"].is_null());
    }

    /// Un docstring `///` sobre un rpc (GRAMMAR.md §3.72) se propaga como
    /// `description` del Operation Object -- antes de esta ronda, el único
    /// texto de un rpc en el spec generado era su nombre.
    #[test]
    fn a_docstring_on_an_rpc_becomes_the_operation_description() {
        let code = r#"
            service Tasks {
                /// Lista todas las tareas pendientes, ordenadas por id.
                rpc list() -> Int { 1 }
            }
        "#;
        let tokens = lexer::tokenize(code).unwrap();
        let program = parser::parse(tokens).unwrap();
        let spec_str = emit_openapi_json(&program, "Task API").unwrap();
        let spec: Value = serde_json::from_str(&spec_str).unwrap();

        assert_eq!(
            spec["paths"]["/Tasks/list"]["post"]["description"],
            "Lista todas las tareas pendientes, ordenadas por id."
        );
        assert!(spec["paths"]["/Tasks/list"]["post"]["deprecated"].is_null());
    }

    /// Docstring Y `@deprecated` a la vez: el motivo se agrega al final de
    /// la descripción en vez de pisarla -- las dos preguntas ("qué hace" y
    /// "por qué ya no usarlo") coexisten en el mismo campo.
    #[test]
    fn a_docstring_and_deprecated_together_combine_into_one_description() {
        let code = r#"
            service Tasks {
                /// Lista todas las tareas.
                @deprecated("usa listV2")
                rpc list() -> Int { 1 }
            }
        "#;
        let tokens = lexer::tokenize(code).unwrap();
        let program = parser::parse(tokens).unwrap();
        let spec_str = emit_openapi_json(&program, "Task API").unwrap();
        let spec: Value = serde_json::from_str(&spec_str).unwrap();

        let desc = spec["paths"]["/Tasks/list"]["post"]["description"].as_str().unwrap();
        assert!(desc.contains("Lista todas las tareas."), "{desc}");
        assert!(desc.contains("Deprecated: usa listV2"), "{desc}");
        assert_eq!(spec["paths"]["/Tasks/list"]["post"]["deprecated"], true);
    }

    /// `@validate(email)`/`@validate(regex, "...")` (GRAMMAR.md §3.73) se
    /// propagan como las keywords estándar de JSON Schema "format"/"pattern"
    /// -- sin extensión propietaria.
    #[test]
    fn validate_email_and_regex_set_standard_json_schema_keywords() {
        let code = r#"
            type Signup = {
                @validate(email) email: String,
                @validate(regex, "^[A-Z]{3}$") code: String,
            }
        "#;
        let tokens = lexer::tokenize(code).unwrap();
        let program = parser::parse(tokens).unwrap();
        let spec_str = emit_openapi_json(&program, "Task API").unwrap();
        let spec: Value = serde_json::from_str(&spec_str).unwrap();

        let props = &spec["components"]["schemas"]["Signup"]["properties"];
        assert_eq!(props["email"]["format"], "email");
        assert_eq!(props["code"]["pattern"], "^[A-Z]{3}$");
        assert!(props["email"]["pattern"].is_null());
    }

    /// Un campo con `= default` (GRAMMAR.md §3.74) sale de `required`, y un
    /// default de literal simple se propaga como `"default"` -- keyword
    /// estándar de JSON Schema.
    #[test]
    fn a_field_with_a_literal_default_is_excluded_from_required_and_gets_the_default_keyword() {
        let code = r#"type Task = { title: String, status: String = "pending" }"#;
        let tokens = lexer::tokenize(code).unwrap();
        let program = parser::parse(tokens).unwrap();
        let spec_str = emit_openapi_json(&program, "Task API").unwrap();
        let spec: Value = serde_json::from_str(&spec_str).unwrap();

        let schema = &spec["components"]["schemas"]["Task"];
        let required: Vec<&str> = schema["required"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        assert!(required.contains(&"title"), "{required:?}");
        assert!(!required.contains(&"status"), "{required:?}");
        assert_eq!(schema["properties"]["status"]["default"], "pending");
    }

    /// Un default NO literal (una llamada como `crypto.uuid()`) sigue
    /// sacando el campo de `required`, pero no tiene una forma JSON fija
    /// que propagar como `"default"` -- se omite la keyword, no un valor
    /// inventado.
    #[test]
    fn a_field_with_a_non_literal_default_is_excluded_from_required_without_a_default_keyword() {
        let code = r#"type Session = { id: Int, token: Uuid = crypto.uuid() }"#;
        let tokens = lexer::tokenize(code).unwrap();
        let program = parser::parse(tokens).unwrap();
        let spec_str = emit_openapi_json(&program, "Task API").unwrap();
        let spec: Value = serde_json::from_str(&spec_str).unwrap();

        let schema = &spec["components"]["schemas"]["Session"];
        let required: Vec<&str> = schema["required"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        assert!(!required.contains(&"token"), "{required:?}");
        assert!(schema["properties"]["token"]["default"].is_null());
    }

    /// `@example(request: ..., response: ...)` (GRAMMAR.md §3.119) -- las
    /// dos mitades se propagan como `"example"` dentro del Media Type
    /// Object correspondiente, mismo nivel que `"schema"`.
    #[test]
    fn example_annotation_sets_the_request_and_response_examples() {
        let code = r#"
            type Task = { id: Int, title: String }
            type CreateInput = { title: String }
            service Tasks {
                @example(request: CreateInput { title: "Comprar leche" }, response: Task { id: 1, title: "Comprar leche" })
                rpc create(title: String) -> Task { Task { id: 1, title: title } }
            }
        "#;
        let tokens = lexer::tokenize(code).unwrap();
        let program = parser::parse(tokens).unwrap();
        let spec_str = emit_openapi_json(&program, "Task API").unwrap();
        let spec: Value = serde_json::from_str(&spec_str).unwrap();

        let op = &spec["paths"]["/Tasks/create"]["post"];
        assert_eq!(op["requestBody"]["content"]["application/json"]["example"], json!({"title": "Comprar leche"}));
        assert_eq!(op["responses"]["200"]["content"]["application/json"]["example"], json!({"id": 1, "title": "Comprar leche"}));
    }

    /// Un `@example` con solo `response` (el caso común de un rpc sin
    /// parámetros, ej. `list()`) no toca `requestBody` para nada -- no
    /// aparece ni siquiera vacío.
    #[test]
    fn example_annotation_with_only_a_response_leaves_request_body_untouched() {
        let code = r#"
            type Task = { id: Int }
            service Tasks {
                @example(response: [Task { id: 1 }])
                rpc list() -> Task[] { [] }
            }
        "#;
        let tokens = lexer::tokenize(code).unwrap();
        let program = parser::parse(tokens).unwrap();
        let spec_str = emit_openapi_json(&program, "Task API").unwrap();
        let spec: Value = serde_json::from_str(&spec_str).unwrap();

        let op = &spec["paths"]["/Tasks/list"]["post"];
        assert!(op["requestBody"].is_null());
        assert_eq!(op["responses"]["200"]["content"]["application/json"]["example"], json!([{"id": 1}]));
    }

    /// Sin `@example`, ningún path gana una clave `"example"` de la nada --
    /// mismo criterio que el resto de las anotaciones opcionales.
    #[test]
    fn no_example_annotation_means_no_example_key_at_all() {
        let code = r#"
            service Tasks {
                rpc list() -> Int { 1 }
            }
        "#;
        let tokens = lexer::tokenize(code).unwrap();
        let program = parser::parse(tokens).unwrap();
        let spec_str = emit_openapi_json(&program, "Task API").unwrap();
        let spec: Value = serde_json::from_str(&spec_str).unwrap();

        assert!(spec["paths"]["/Tasks/list"]["post"]["responses"]["200"]["content"]["application/json"]["example"].is_null());
    }
}
