//! Generador de especificación OpenAPI 3.1 (openapi.json) a partir de servicios Link.
//! Permite documentación Swagger UI interactiva, generación de SDKs y testing en Postman.

use std::collections::BTreeMap;
use serde_json::{json, Value};
use crate::ast::{Item, Member, Program, TypeExpr};
use crate::checker::Checker;
use crate::types::Type;

fn type_to_json_schema(ty: &Type) -> Value {
    match ty {
        Type::Int => json!({ "type": "integer", "format": "int32" }),
        Type::Int64 => json!({ "type": "integer", "format": "int64" }),
        Type::Float => json!({ "type": "number", "format": "double" }),
        Type::String => json!({ "type": "string" }),
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
                        props[f.name.as_str()] = type_to_json_schema(&ty);
                        if !f.optional {
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

            let path_key = format!("/{}/{}", service.name, rpc.name);
            paths[path_key.as_str()] = json!({
                "post": operation
            });
        }
    }

    let doc = json!({
        "openapi": "3.1.0",
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
    }
}
