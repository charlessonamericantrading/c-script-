//! Codegen WASM nativo v0: soporta solo aritmética/comparación entera sobre
//! `Int`/`Bool` (ambos representados como i64) -- nada de String/Float/
//! structs/enums/Optional/List, y nada de sentencias dentro de un cuerpo
//! (solo la expresión final de un bloque). Fuera de ese subconjunto, emitir
//! SIEMPRE es un error explícito, nunca un placeholder silencioso -- antes,
//! cualquier construcción no soportada se reemplazaba por `I64Const(0)` (o
//! un bloque con sentencias simplemente las ignoraba), así que `linkc wasm`/
//! `linkc build` reportaban éxito mientras producían bytecode incorrecto.

use std::collections::HashMap;
use crate::ast::{self, Item, Program};
use wasm_encoder::{
    CodeSection, ExportKind, ExportSection, Function, FunctionSection, Instruction, Module,
    TypeSection, ValType,
};

/// Único tipo escalar que este codegen sabe representar: `Int` y `Bool`
/// mapean ambos a `i64` (falso/verdadero como 0/1, mismo esquema que
/// `Expr::Bool` ya usaba). Cualquier otro tipo (String, Float, un struct,
/// un enum, `T?`, `T[]`, ...) no tiene una representación wasm en v0.
fn wasm_scalar_type(ty: &ast::TypeExpr) -> Result<ValType, String> {
    match ty {
        ast::TypeExpr::Named(name, args) if args.is_empty() && (name == "Int" || name == "Bool") => {
            Ok(ValType::I64)
        }
        other => Err(format!(
            "el codegen wasm nativo solo soporta 'Int'/'Bool' como tipo de parámetro o de retorno -- se encontró {other:?}"
        )),
    }
}

fn emit_expr(expr: &ast::Expr, param_map: &HashMap<String, u32>, func: &mut Function) -> Result<(), String> {
    match expr {
        ast::Expr::Int(n) => {
            func.instruction(&Instruction::I64Const(*n));
        }
        ast::Expr::Bool(b) => {
            func.instruction(&Instruction::I64Const(if *b { 1 } else { 0 }));
        }
        ast::Expr::Ident(id) => {
            let Some(&idx) = param_map.get(id) else {
                return Err(format!(
                    "el codegen wasm nativo solo puede leer parámetros de la función -- identificador no soportado: '{id}'"
                ));
            };
            func.instruction(&Instruction::LocalGet(idx));
        }
        ast::Expr::Binary { op, left, right } => {
            emit_expr(&left.node, param_map, func)?;
            emit_expr(&right.node, param_map, func)?;
            match op {
                ast::BinaryOp::Add => { func.instruction(&Instruction::I64Add); }
                ast::BinaryOp::Sub => { func.instruction(&Instruction::I64Sub); }
                ast::BinaryOp::Mul => { func.instruction(&Instruction::I64Mul); }
                ast::BinaryOp::Div => { func.instruction(&Instruction::I64DivS); }
                ast::BinaryOp::Rem => { func.instruction(&Instruction::I64RemS); }
                ast::BinaryOp::Eq => { func.instruction(&Instruction::I64Eq); }
                ast::BinaryOp::NotEq => { func.instruction(&Instruction::I64Ne); }
                ast::BinaryOp::Lt => { func.instruction(&Instruction::I64LtS); }
                ast::BinaryOp::Gt => { func.instruction(&Instruction::I64GtS); }
                ast::BinaryOp::LtEq => { func.instruction(&Instruction::I64LeS); }
                ast::BinaryOp::GtEq => { func.instruction(&Instruction::I64GeS); }
                other => {
                    return Err(format!("el codegen wasm nativo no soporta el operador binario {other:?} (solo aritmética/comparación entera)"));
                }
            }
        }
        ast::Expr::Paren(inner) => {
            emit_expr(&inner.node, param_map, func)?;
        }
        other => {
            return Err(format!("el codegen wasm nativo no soporta esta expresión todavía: {other:?}"));
        }
    }
    Ok(())
}

fn emit_block(block: &ast::Block, param_map: &HashMap<String, u32>, func: &mut Function) -> Result<(), String> {
    if !block.stmts.is_empty() {
        return Err(format!(
            "el codegen wasm nativo solo soporta una expresión final por cuerpo -- este bloque tiene {} sentencia(s) (let/asignación/if/while) que se ignorarían en silencio",
            block.stmts.len()
        ));
    }
    match &block.tail {
        Some(tail) => emit_expr(&tail.node, param_map, func),
        None => Err("el codegen wasm nativo requiere una expresión final en el cuerpo -- este bloque no tiene ninguna".to_string()),
    }
}

/// Las 4 secciones WASM que cada función/rpc/stream compilada alimenta, más
/// el índice de tipo compartido -- agrupadas para que `emit_fn_like` no
/// necesite un parámetro por sección (clippy::too_many_arguments).
struct WasmSink<'a> {
    type_count: u32,
    types: &'a mut TypeSection,
    functions: &'a mut FunctionSection,
    exports: &'a mut ExportSection,
    codes: &'a mut CodeSection,
}

fn emit_fn_like(
    name: &str,
    params: &[ast::Param],
    return_type: &ast::TypeExpr,
    body: &ast::Block,
    export_name: &str,
    sink: &mut WasmSink,
) -> Result<(), String> {
    let mut wasm_params = Vec::with_capacity(params.len());
    for p in params {
        let vt = wasm_scalar_type(&p.ty).map_err(|e| format!("en '{name}', parámetro '{}': {e}", p.name))?;
        wasm_params.push(vt);
    }
    let result_ty = wasm_scalar_type(return_type).map_err(|e| format!("en '{name}', tipo de retorno: {e}"))?;

    sink.types.function(wasm_params, vec![result_ty]);
    sink.functions.function(sink.type_count);
    sink.exports.export(export_name, ExportKind::Func, sink.type_count);

    let mut param_map = HashMap::new();
    for (idx, p) in params.iter().enumerate() {
        param_map.insert(p.name.clone(), idx as u32);
    }

    let mut func = Function::new(vec![]);
    emit_block(body, &param_map, &mut func).map_err(|e| format!("en '{name}': {e}"))?;
    func.instruction(&Instruction::End);

    sink.codes.function(&func);
    sink.type_count += 1;
    Ok(())
}

pub fn emit_wasm(program: &Program) -> Result<Vec<u8>, String> {
    let mut module = Module::new();

    let mut types = TypeSection::new();
    let mut functions = FunctionSection::new();
    let mut exports = ExportSection::new();
    let mut codes = CodeSection::new();
    let mut sink = WasmSink { type_count: 0, types: &mut types, functions: &mut functions, exports: &mut exports, codes: &mut codes };

    for item in &program.items {
        match item {
            Item::Fn(f) => {
                emit_fn_like(&f.name, &f.params, &f.return_type, &f.body, &f.name, &mut sink)?;
            }
            Item::Service(s) => {
                for member in &s.members {
                    let rpc = match member {
                        ast::Member::Rpc(r) | ast::Member::Stream(r) => r,
                    };
                    let full_name = format!("{}.{}", s.name, rpc.name);
                    emit_fn_like(&full_name, &rpc.params, &rpc.return_type, &rpc.body, &full_name, &mut sink)?;
                }
            }
            _ => {}
        }
    }

    module.section(&types);
    module.section(&functions);
    module.section(&exports);
    module.section(&codes);

    Ok(module.finish())
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer;
    use crate::parser;

    fn compile(code: &str) -> Result<Vec<u8>, String> {
        let tokens = lexer::tokenize(code).unwrap();
        let program = parser::parse(tokens).unwrap();
        emit_wasm(&program)
    }

    #[test]
    fn test_wasm_emit_header_magic() {
        let code = "fn add(a: Int, b: Int) -> Int { a + b }\nservice Users { rpc ping() -> Int { 1 } }";
        let wasm_bytes = compile(code).unwrap();
        assert!(wasm_bytes.len() > 8);
        // Header mágico WASM: \0asm (\x00\x61\x73\x6d)
        assert_eq!(&wasm_bytes[0..4], &[0x00, 0x61, 0x73, 0x6d]);
    }

    #[test]
    fn test_comparison_ops_still_supported() {
        let code = "fn isBigger(a: Int, b: Int) -> Bool { a > b }";
        assert!(compile(code).is_ok());
    }

    #[test]
    fn test_a_let_statement_in_the_body_fails_loudly_instead_of_being_ignored() {
        let code = "fn f(a: Int) -> Int { let b = a + 1; b }";
        let err = compile(code).unwrap_err();
        assert!(err.contains("sentencia"), "el error tiene que explicar que hay sentencias no soportadas: {err}");
    }

    #[test]
    fn test_a_string_parameter_fails_loudly_instead_of_being_treated_as_i64() {
        let code = "fn greet(name: String) -> String { name }";
        let err = compile(code).unwrap_err();
        assert!(err.contains("String"), "el error tiene que nombrar el tipo no soportado: {err}");
    }

    #[test]
    fn test_a_logical_operator_fails_loudly_instead_of_silently_becoming_addition() {
        let code = "fn both(a: Bool, b: Bool) -> Bool { a && b }";
        let err = compile(code).unwrap_err();
        assert!(err.contains("And"), "el error tiene que nombrar el operador no soportado: {err}");
    }

    #[test]
    fn test_an_empty_body_fails_loudly_instead_of_defaulting_to_zero() {
        let code = "fn f() -> Int { }";
        assert!(compile(code).is_err());
    }
}
