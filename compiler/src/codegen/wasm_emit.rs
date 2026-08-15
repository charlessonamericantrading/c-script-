//! Codegen WASM nativo: compila funciones y RPCs a bytecode WebAssembly estándar.
//! Soporta `Int`, `Int64`, `Bool` (como i64) y `Float` (como f64), variables locales
//! (`let`, `mut`), control de flujo (`if/else`, `while`, `return`), conversiones
//! numéricas (`.toInt()`, `.toFloat()`, `.toInt64()`), y llamadas entre funciones (`call`).

use std::collections::HashMap;
use crate::ast::{self, Item, Program};
use wasm_encoder::{
    BlockType, CodeSection, ExportKind, ExportSection, Function, FunctionSection, Instruction, Module,
    TypeSection, ValType,
};

/// Mapeo de tipos escalares a WebAssembly ValType.
fn wasm_scalar_type(ty: &ast::TypeExpr) -> Result<ValType, String> {
    match ty {
        ast::TypeExpr::Named(name, args, _) if args.is_empty() => match name.as_str() {
            "Int" | "Bool" | "Int64" => Ok(ValType::I64),
            "Float" => Ok(ValType::F64),
            other => Err(format!(
                "el codegen wasm nativo solo soporta 'Int', 'Int64', 'Bool' y 'Float' como tipo de parámetro o retorno -- se encontró '{other}'"
            )),
        },
        other => Err(format!(
            "el codegen wasm nativo no soporta tipos compuestos ({other:?}) en firmas"
        )),
    }
}

struct WasmFuncCtx<'a> {
    fn_indices: &'a HashMap<String, u32>,
    locals: HashMap<String, (u32, ValType)>,
    local_types: Vec<ValType>,
}

impl<'a> WasmFuncCtx<'a> {
    fn new(fn_indices: &'a HashMap<String, u32>, params: &[ast::Param]) -> Result<Self, String> {
        let mut locals = HashMap::new();
        for (idx, p) in params.iter().enumerate() {
            let vt = wasm_scalar_type(&p.ty)?;
            locals.insert(p.name.clone(), (idx as u32, vt));
        }
        Ok(Self {
            fn_indices,
            locals,
            local_types: Vec::new(),
        })
    }

    fn add_local(&mut self, name: String, ty: ValType) -> u32 {
        let idx = (self.locals.len()) as u32;
        self.local_types.push(ty);
        self.locals.insert(name, (idx, ty));
        idx
    }

    fn get_local(&self, name: &str) -> Option<(u32, ValType)> {
        self.locals.get(name).copied()
    }
}

fn group_locals(local_types: &[ValType]) -> Vec<(u32, ValType)> {
    let mut groups: Vec<(u32, ValType)> = Vec::new();
    for &ty in local_types {
        if let Some(last) = groups.last_mut() {
            if last.1 == ty {
                last.0 += 1;
                continue;
            }
        }
        groups.push((1, ty));
    }
    groups
}

fn infer_expr_valtype(expr: &ast::Expr, ctx: &WasmFuncCtx) -> Result<ValType, String> {
    match expr {
        ast::Expr::Int(_) | ast::Expr::Bool(_) => Ok(ValType::I64),
        ast::Expr::Float(_) => Ok(ValType::F64),
        ast::Expr::Ident(id) => {
            if let Some((_, vt)) = ctx.get_local(id) {
                Ok(vt)
            } else {
                Err(format!("identificador no declarado: '{id}'"))
            }
        }
        ast::Expr::Paren(inner) => infer_expr_valtype(&inner.node, ctx),
        ast::Expr::Unary { op: _, operand } => infer_expr_valtype(&operand.node, ctx),
        ast::Expr::Binary { op, left, right } => {
            let l_ty = infer_expr_valtype(&left.node, ctx)?;
            let r_ty = infer_expr_valtype(&right.node, ctx)?;
            match op {
                ast::BinaryOp::Eq
                | ast::BinaryOp::NotEq
                | ast::BinaryOp::Lt
                | ast::BinaryOp::Gt
                | ast::BinaryOp::LtEq
                | ast::BinaryOp::GtEq
                | ast::BinaryOp::And
                | ast::BinaryOp::Or => Ok(ValType::I64),
                _ => {
                    if l_ty == ValType::F64 || r_ty == ValType::F64 {
                        Ok(ValType::F64)
                    } else {
                        Ok(ValType::I64)
                    }
                }
            }
        }
        ast::Expr::If { then_block, else_block, .. } => {
            if let Some(tail) = &then_block.tail {
                infer_expr_valtype(&tail.node, ctx)
            } else if let Some(tail) = &else_block.tail {
                infer_expr_valtype(&tail.node, ctx)
            } else {
                Ok(ValType::I64)
            }
        }
        ast::Expr::Call { callee, .. } => {
            if let ast::Expr::FieldAccess { field, .. } = &callee.node {
                if field == "toFloat" {
                    return Ok(ValType::F64);
                } else if field == "toInt" || field == "toInt64" {
                    return Ok(ValType::I64);
                }
            }
            Ok(ValType::I64)
        }
        other => Err(format!("no se puede inferir tipo wasm para: {other:?}")),
    }
}

fn scan_locals(block: &ast::Block, ctx: &mut WasmFuncCtx) -> Result<(), String> {
    for stmt in &block.stmts {
        match &stmt.node {
            ast::Stmt::Let { name, ty, value, .. } => {
                let val_ty = if let Some(t) = ty {
                    wasm_scalar_type(t)?
                } else {
                    infer_expr_valtype(&value.node, ctx)?
                };
                ctx.add_local(name.clone(), val_ty);
            }
            ast::Stmt::While { body, .. } => {
                scan_locals(body, ctx)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn emit_stmt(stmt: &ast::Stmt, ctx: &mut WasmFuncCtx, func: &mut Function) -> Result<(), String> {
    match stmt {
        ast::Stmt::Let { name, value, .. } => {
            let (idx, _) = ctx.get_local(name).ok_or_else(|| format!("local no encontrado: {name}"))?;
            emit_expr(&value.node, ctx, func)?;
            func.instruction(&Instruction::LocalSet(idx));
        }
        ast::Stmt::Assign { name, value, .. } => {
            let (idx, _) = ctx.get_local(name).ok_or_else(|| format!("variable no declarada en wasm: '{name}'"))?;
            emit_expr(&value.node, ctx, func)?;
            func.instruction(&Instruction::LocalSet(idx));
        }
        ast::Stmt::Expr(expr) => {
            let has_val = emit_expr(&expr.node, ctx, func)?;
            if has_val {
                func.instruction(&Instruction::Drop);
            }
        }
        ast::Stmt::While { cond, body, .. } => {
            func.instruction(&Instruction::Block(BlockType::Empty));
            func.instruction(&Instruction::Loop(BlockType::Empty));

            emit_expr(&cond.node, ctx, func)?;
            func.instruction(&Instruction::I64Eqz);
            func.instruction(&Instruction::BrIf(1));

            emit_block(body, ctx, func, false)?;

            func.instruction(&Instruction::Br(0));
            func.instruction(&Instruction::End);
            func.instruction(&Instruction::End);
        }
        ast::Stmt::Return(expr) => {
            if let Some(e) = expr {
                emit_expr(&e.node, ctx, func)?;
            }
            func.instruction(&Instruction::Return);
        }
    }
    Ok(())
}

fn emit_expr(expr: &ast::Expr, ctx: &mut WasmFuncCtx, func: &mut Function) -> Result<bool, String> {
    match expr {
        ast::Expr::Int(n) => {
            func.instruction(&Instruction::I64Const(*n));
            Ok(true)
        }
        ast::Expr::Float(f) => {
            func.instruction(&Instruction::F64Const(*f));
            Ok(true)
        }
        ast::Expr::Bool(b) => {
            func.instruction(&Instruction::I64Const(if *b { 1 } else { 0 }));
            Ok(true)
        }
        ast::Expr::Ident(id) => {
            let Some((idx, _)) = ctx.get_local(id) else {
                return Err(format!(
                    "el codegen wasm nativo solo puede leer variables locales o parámetros -- identificador no soportado: '{id}'"
                ));
            };
            func.instruction(&Instruction::LocalGet(idx));
            Ok(true)
        }
        ast::Expr::Unary { op, operand } => {
            let ty = infer_expr_valtype(&operand.node, ctx)?;
            emit_expr(&operand.node, ctx, func)?;
            match op {
                ast::UnaryOp::Not => {
                    func.instruction(&Instruction::I64Eqz);
                }
                ast::UnaryOp::Neg => {
                    if ty == ValType::F64 {
                        func.instruction(&Instruction::F64Neg);
                    } else {
                        func.instruction(&Instruction::I64Const(-1));
                        func.instruction(&Instruction::I64Mul);
                    }
                }
            }
            Ok(true)
        }
        ast::Expr::Binary { op, left, right } => {
            let l_ty = infer_expr_valtype(&left.node, ctx)?;
            let r_ty = infer_expr_valtype(&right.node, ctx)?;
            emit_expr(&left.node, ctx, func)?;
            emit_expr(&right.node, ctx, func)?;

            if l_ty == ValType::F64 || r_ty == ValType::F64 {
                match op {
                    ast::BinaryOp::Add => { func.instruction(&Instruction::F64Add); }
                    ast::BinaryOp::Sub => { func.instruction(&Instruction::F64Sub); }
                    ast::BinaryOp::Mul => { func.instruction(&Instruction::F64Mul); }
                    ast::BinaryOp::Div => { func.instruction(&Instruction::F64Div); }
                    ast::BinaryOp::Eq => { func.instruction(&Instruction::F64Eq); }
                    ast::BinaryOp::NotEq => { func.instruction(&Instruction::F64Ne); }
                    ast::BinaryOp::Lt => { func.instruction(&Instruction::F64Lt); }
                    ast::BinaryOp::Gt => { func.instruction(&Instruction::F64Gt); }
                    ast::BinaryOp::LtEq => { func.instruction(&Instruction::F64Le); }
                    ast::BinaryOp::GtEq => { func.instruction(&Instruction::F64Ge); }
                    other => {
                        return Err(format!("operador {other:?} no soportado para Float en wasm"));
                    }
                }
            } else {
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
                    ast::BinaryOp::And => { func.instruction(&Instruction::I64And); }
                    ast::BinaryOp::Or => { func.instruction(&Instruction::I64Or); }
                }
            }
            Ok(true)
        }
        ast::Expr::Paren(inner) => emit_expr(&inner.node, ctx, func),
        ast::Expr::If { cond, then_block, else_block } => {
            emit_expr(&cond.node, ctx, func)?;
            let has_result = then_block.tail.is_some() || else_block.tail.is_some();
            let res_ty = if let Some(tail) = &then_block.tail {
                Some(infer_expr_valtype(&tail.node, ctx)?)
            } else if let Some(tail) = &else_block.tail {
                Some(infer_expr_valtype(&tail.node, ctx)?)
            } else {
                None
            };
            let block_type = match res_ty {
                Some(vt) => BlockType::Result(vt),
                None => BlockType::Empty,
            };
            func.instruction(&Instruction::If(block_type));
            emit_block(then_block, ctx, func, has_result)?;
            let has_else = !else_block.stmts.is_empty() || else_block.tail.is_some();
            if has_else {
                func.instruction(&Instruction::Else);
                emit_block(else_block, ctx, func, has_result)?;
            }
            func.instruction(&Instruction::End);
            Ok(has_result)
        }
        ast::Expr::Call { callee, args } => {
            // Check numeric conversion methods on primitive receivers
            if let ast::Expr::FieldAccess { base, field } = &callee.node {
                if field == "toFloat" && args.is_empty() {
                    emit_expr(&base.node, ctx, func)?;
                    func.instruction(&Instruction::F64ConvertI64S);
                    return Ok(true);
                } else if field == "toInt" && args.is_empty() {
                    emit_expr(&base.node, ctx, func)?;
                    func.instruction(&Instruction::I64TruncF64S);
                    return Ok(true);
                } else if field == "toInt64" && args.is_empty() {
                    emit_expr(&base.node, ctx, func)?;
                    return Ok(true);
                }
            }

            let fn_name = match &callee.node {
                ast::Expr::Ident(name) => name.clone(),
                ast::Expr::FieldAccess { base, field } => {
                    if let ast::Expr::Ident(base_name) = &base.node {
                        format!("{base_name}.{field}")
                    } else {
                        return Err("llamadas complejas a métodos no soportadas en wasm nativo".to_string());
                    }
                }
                _ => return Err("llamadas dinámicas a expresiones no soportadas en wasm nativo".to_string()),
            };

            let Some(&func_idx) = ctx.fn_indices.get(&fn_name) else {
                return Err(format!("función desconocida o no exportable en wasm: '{fn_name}'"));
            };

            for arg in args {
                emit_expr(&arg.node, ctx, func)?;
            }
            func.instruction(&Instruction::Call(func_idx));
            Ok(true)
        }
        other => {
            Err(format!("el codegen wasm nativo no soporta esta expresión todavía: {other:?}"))
        }
    }
}

fn emit_block(
    block: &ast::Block,
    ctx: &mut WasmFuncCtx,
    func: &mut Function,
    expect_result: bool,
) -> Result<(), String> {
    for stmt in &block.stmts {
        emit_stmt(&stmt.node, ctx, func)?;
    }
    match &block.tail {
        Some(tail) => {
            emit_expr(&tail.node, ctx, func)?;
        }
        None => {
            if expect_result {
                return Err("se esperaba una expresión de retorno en el bloque".to_string());
            }
        }
    }
    Ok(())
}

pub fn emit_wasm(program: &Program) -> Result<Vec<u8>, String> {
    let mut module = Module::new();

    let mut types = TypeSection::new();
    let mut functions = FunctionSection::new();
    let mut exports = ExportSection::new();
    let mut codes = CodeSection::new();

    let mut fn_indices = HashMap::new();
    let mut fn_declarations = Vec::new();

    let mut func_idx = 0u32;
    for item in &program.items {
        match item {
            Item::Fn(f) => {
                fn_indices.insert(f.name.clone(), func_idx);
                fn_declarations.push((f.name.clone(), f.params.clone(), f.return_type.clone(), f.body.clone(), f.name.clone()));
                func_idx += 1;
            }
            Item::Service(s) => {
                for member in &s.members {
                    let rpc = match member {
                        ast::Member::Rpc(r) | ast::Member::Stream(r) => r,
                    };
                    let full_name = format!("{}.{}", s.name, rpc.name);
                    fn_indices.insert(full_name.clone(), func_idx);
                    fn_declarations.push((full_name.clone(), rpc.params.clone(), rpc.return_type.clone(), rpc.body.clone(), full_name.clone()));
                    func_idx += 1;
                }
            }
            _ => {}
        }
    }

    if fn_declarations.is_empty() {
        return Err("el programa no contiene ninguna función o RPC para compilar a WASM".to_string());
    }

    for (idx, (name, params, return_type, body, export_name)) in fn_declarations.iter().enumerate() {
        let mut wasm_params = Vec::with_capacity(params.len());
        for p in params {
            let vt = wasm_scalar_type(&p.ty).map_err(|e| format!("en '{name}', parámetro '{}': {e}", p.name))?;
            wasm_params.push(vt);
        }

        let is_void = matches!(return_type, ast::TypeExpr::Named(n, args, _) if args.is_empty() && n == "Void");
        let wasm_results = if is_void {
            vec![]
        } else {
            vec![wasm_scalar_type(return_type).map_err(|e| format!("en '{name}', tipo de retorno: {e}"))?]
        };

        types.function(wasm_params, wasm_results);
        functions.function(idx as u32);
        exports.export(export_name, ExportKind::Func, idx as u32);

        let mut ctx = WasmFuncCtx::new(&fn_indices, params)
            .map_err(|e| format!("en '{name}': {e}"))?;

        scan_locals(body, &mut ctx)?;

        let locals = group_locals(&ctx.local_types);
        let mut func = Function::new(locals);

        emit_block(body, &mut ctx, &mut func, !is_void)
            .map_err(|e| format!("en '{name}': {e}"))?;
        func.instruction(&Instruction::End);

        codes.function(&func);
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
    fn test_let_statements_and_local_variables() {
        let code = "fn f(a: Int) -> Int { let b = a + 1; let mut c = b * 2; c = c + 5; c }";
        let wasm_bytes = compile(code).unwrap();
        assert!(wasm_bytes.len() > 8);
    }

    #[test]
    fn test_if_else_control_flow() {
        let code = "fn max(a: Int, b: Int) -> Int { if a > b { a } else { b } }";
        let wasm_bytes = compile(code).unwrap();
        assert!(wasm_bytes.len() > 8);
    }

    #[test]
    fn test_while_loop() {
        let code = "fn sumTo(n: Int) -> Int { let mut sum = 0; let mut i = 1; while i <= n { sum = sum + i; i = i + 1; } sum }";
        let wasm_bytes = compile(code).unwrap();
        assert!(wasm_bytes.len() > 8);
    }

    #[test]
    fn test_float_arithmetic_and_conversions() {
        let code = "fn circleArea(r: Float) -> Float { let pi = 3.14159; pi * r * r }\nfn intToFloat(x: Int) -> Float { x.toFloat() }";
        let wasm_bytes = compile(code).unwrap();
        assert!(wasm_bytes.len() > 8);
    }

    #[test]
    fn test_function_calls_and_recursion() {
        let code = "fn square(x: Int) -> Int { x * x }\nfn sumSquares(a: Int, b: Int) -> Int { square(a) + square(b) }\nfn fact(n: Int) -> Int { if n <= 1 { 1 } else { n * fact(n - 1) } }";
        let wasm_bytes = compile(code).unwrap();
        assert!(wasm_bytes.len() > 8);
    }

    #[test]
    fn test_logical_operators() {
        let code = "fn both(a: Bool, b: Bool) -> Bool { a && b }\nfn either(a: Bool, b: Bool) -> Bool { a || b }";
        let wasm_bytes = compile(code).unwrap();
        assert!(wasm_bytes.len() > 8);
    }

    #[test]
    fn test_a_string_parameter_fails_loudly_instead_of_being_treated_as_i64() {
        let code = "fn greet(name: String) -> String { name }";
        let err = compile(code).unwrap_err();
        assert!(err.contains("String"), "el error tiene que nombrar el tipo no soportado: {err}");
    }

    #[test]
    fn test_an_empty_body_fails_loudly_instead_of_defaulting_to_zero() {
        let code = "fn f() -> Int { }";
        assert!(compile(code).is_err());
    }
}
