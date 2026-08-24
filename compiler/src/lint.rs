//! Linter estático para análisis de calidad de código en Link.
//! Detecta variables no utilizadas, mutabilidad redundante y tests vacíos.

use crate::ast::{BinaryOp, Block, Expr, Item, Member, MatchArmBody, Program, Spanned, Stmt};

#[derive(Debug, Clone, PartialEq)]
pub struct LintWarning {
    pub rule: &'static str,
    pub message: String,
    pub line: usize,
    pub col: usize,
}

pub fn lint_program(program: &Program) -> Vec<LintWarning> {
    let mut warnings = Vec::new();

    for item in &program.items {
        match item {
            Item::Fn(f) => {
                lint_block(&f.body, &mut warnings);
            }
            Item::Service(s) => {
                let has_auth = s.members.iter().any(|m| match m {
                    Member::Rpc(r) | Member::Stream(r) => r.auth().is_some(),
                });
                let has_unauth = s.members.iter().any(|m| match m {
                    Member::Rpc(r) | Member::Stream(r) => r.auth().is_none(),
                });

                if has_auth && has_unauth {
                    warnings.push(LintWarning {
                        rule: "mixed-service-auth",
                        message: format!(
                            "el servicio '{}' mezcla RPCs protegidos por @requires/@authenticated con RPCs públicos sin anotación",
                            s.name
                        ),
                        line: s.span.line,
                        col: s.span.col,
                    });
                }

                for m in &s.members {
                    let rpc = match m {
                        Member::Rpc(r) | Member::Stream(r) => r,
                    };
                    lint_block(&rpc.body, &mut warnings);
                }
            }
            Item::Test(t) => {
                if t.body.stmts.is_empty() && t.body.tail.is_none() {
                    warnings.push(LintWarning {
                        rule: "empty-test",
                        message: format!("el bloque de prueba \"{}\" está vacío y no realiza aserciones", t.name),
                        line: t.span.line,
                        col: t.span.col,
                    });
                } else {
                    lint_block(&t.body, &mut warnings);
                }
            }
            _ => {}
        }
    }

    warnings
}

fn lint_block(block: &Block, warnings: &mut Vec<LintWarning>) {
    let mut declared_lets: Vec<(&String, bool, usize, usize)> = Vec::new();

    for stmt in &block.stmts {
        match &stmt.node {
            Stmt::Let { name, mutable, .. } => {
                if !name.starts_with('_') {
                    declared_lets.push((name, *mutable, stmt.span.line, stmt.span.col));
                }
            }
            Stmt::While { body, .. } => {
                lint_block(body, warnings);
            }
            _ => {}
        }
    }

    for (name, mutable, line, col) in declared_lets {
        let is_used = block_uses_ident(block, name);
        if !is_used {
            warnings.push(LintWarning {
                rule: "unused-var",
                message: format!("la variable '{}' fue declarada pero nunca se utiliza (usa '_{}' si es intencional)", name, name),
                line,
                col,
            });
        } else if mutable {
            let is_reassigned = block_reassigns_ident(block, name);
            if !is_reassigned {
                warnings.push(LintWarning {
                    rule: "unused-mut",
                    message: format!("la variable '{}' fue declarada con 'mut' pero nunca es reasignada", name),
                    line,
                    col,
                });
            }
        }
    }

    lint_secret_comparisons_in_block(block, warnings);
}

/// ¿El nombre SUGIERE un secreto? Substring en minúsculas, deliberadamente
/// laxo -- mejor un falso positivo ocasional sobre un identificador raro
/// (`tokenCount`, por ejemplo) que dejar pasar el caso real que esta regla
/// existe para atrapar.
fn looks_like_a_secret(name: &str) -> bool {
    let lower = name.to_lowercase();
    ["secret", "token", "password", "apikey", "api_key"].iter().any(|kw| lower.contains(kw))
}

/// El nombre "de forma" de un operando -- el de un `Ident` o el CAMPO final
/// de un `FieldAccess` (`user.token` -> `"token"`). `None` para cualquier
/// otra forma (un literal, una llamada, ...) que no tiene un nombre propio
/// que juzgar.
fn operand_name(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Ident(name) => Some(name),
        Expr::FieldAccess { field, .. } => Some(field),
        _ => None,
    }
}

/// `==`/`!=` sobre algo que PARECE un secreto (GRAMMAR.md §3.88): un `==`
/// de `String` corta en el primer byte distinto -- el mismo canal lateral
/// de tiempo que `crypto.timingSafeEqual` (§3.54) existe justamente para
/// cerrar, pero que nada obliga a usar en vez del operador de siempre.
fn lint_secret_comparisons_in_block(block: &Block, warnings: &mut Vec<LintWarning>) {
    for stmt in &block.stmts {
        match &stmt.node {
            Stmt::Let { value, .. } | Stmt::Assign { value, .. } => lint_secret_comparisons_in_expr(value, warnings),
            Stmt::Return(Some(e)) | Stmt::Expr(e) => lint_secret_comparisons_in_expr(e, warnings),
            Stmt::Return(None) => {}
            // El BODY de un `while` NO se recorre acá -- `lint_block` (el
            // caller de siempre) ya recursa ahí por su cuenta para las
            // reglas de variables sin usar, y esa recursión vuelve a
            // llamar a ESTA función sobre ese mismo bloque -- recorrerlo
            // acá TAMBIÉN duplicaría cada warning que caiga adentro de un
            // `while`.
            Stmt::While { cond, .. } => lint_secret_comparisons_in_expr(cond, warnings),
        }
    }
    if let Some(tail) = &block.tail {
        lint_secret_comparisons_in_expr(tail, warnings);
    }
}

fn lint_secret_comparisons_in_expr(expr: &Spanned<Expr>, warnings: &mut Vec<LintWarning>) {
    match &expr.node {
        Expr::Binary { op, left, right } => {
            lint_secret_comparisons_in_expr(left, warnings);
            lint_secret_comparisons_in_expr(right, warnings);
            if matches!(op, BinaryOp::Eq | BinaryOp::NotEq) {
                // Comparar contra `null` es un chequeo de PRESENCIA
                // (`token != null`), no de VALOR -- ningún canal lateral de
                // tiempo que cerrar ahí, así que no cuenta.
                let against_null = matches!(left.node, Expr::Null) || matches!(right.node, Expr::Null);
                if !against_null {
                    let flagged = operand_name(&left.node)
                        .filter(|n| looks_like_a_secret(n))
                        .or_else(|| operand_name(&right.node).filter(|n| looks_like_a_secret(n)));
                    if let Some(name) = flagged {
                        warnings.push(LintWarning {
                            rule: "timing-unsafe-secret-comparison",
                            message: format!(
                                "'{name}' se compara con '==' -- si es un secreto (token/password/API key), usá crypto.timingSafeEqual(a, b) en vez de '==' para no filtrar cuánto acertó por el tiempo que tarda la comparación"
                            ),
                            line: expr.span.line,
                            col: expr.span.col,
                        });
                    }
                }
            }
        }
        Expr::Unary { operand, .. } | Expr::Paren(operand) | Expr::FieldAccess { base: operand, .. } | Expr::TupleIndex { base: operand, .. } => {
            lint_secret_comparisons_in_expr(operand, warnings);
        }
        Expr::Index { base, index } => {
            lint_secret_comparisons_in_expr(base, warnings);
            lint_secret_comparisons_in_expr(index, warnings);
        }
        Expr::Call { callee, args } => {
            lint_secret_comparisons_in_expr(callee, warnings);
            for a in args {
                lint_secret_comparisons_in_expr(a, warnings);
            }
        }
        Expr::ArrayLit(items) | Expr::TupleLit(items) => {
            for e in items {
                lint_secret_comparisons_in_expr(e, warnings);
            }
        }
        Expr::StructLit { fields, .. } => {
            for (_, v) in fields {
                lint_secret_comparisons_in_expr(v, warnings);
            }
        }
        Expr::If { cond, then_block, else_block } => {
            lint_secret_comparisons_in_expr(cond, warnings);
            lint_secret_comparisons_in_block(then_block, warnings);
            lint_secret_comparisons_in_block(else_block, warnings);
        }
        Expr::Match { scrutinee, arms } => {
            lint_secret_comparisons_in_expr(scrutinee, warnings);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    lint_secret_comparisons_in_expr(guard, warnings);
                }
                match &arm.body {
                    MatchArmBody::Expr(e) => lint_secret_comparisons_in_expr(e, warnings),
                    MatchArmBody::Block(b) => lint_secret_comparisons_in_block(b, warnings),
                }
            }
        }
        Expr::Closure { body, .. } => lint_secret_comparisons_in_block(body, warnings),
        Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Bool(_) | Expr::Null | Expr::Ident(_) => {}
    }
}

fn block_uses_ident(block: &Block, target: &str) -> bool {
    let mut count = 0;
    for stmt in &block.stmts {
        match &stmt.node {
            Stmt::Let { value, .. } => {
                count += expr_count_ident(&value.node, target);
            }
            Stmt::Assign { name, value } => {
                if name == target {
                    count += 1;
                }
                count += expr_count_ident(&value.node, target);
            }
            Stmt::Expr(e) => {
                count += expr_count_ident(&e.node, target);
            }
            Stmt::Return(Some(e)) => {
                count += expr_count_ident(&e.node, target);
            }
            Stmt::While { cond, body } => {
                count += expr_count_ident(&cond.node, target);
                if block_uses_ident(body, target) {
                    count += 1;
                }
            }
            _ => {}
        }
    }
    if let Some(tail) = &block.tail {
        count += expr_count_ident(&tail.node, target);
    }
    count > 0
}

fn block_reassigns_ident(block: &Block, target: &str) -> bool {
    for stmt in &block.stmts {
        match &stmt.node {
            Stmt::Assign { name, .. } if name == target => return true,
            Stmt::While { body, .. } => {
                if block_reassigns_ident(body, target) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn expr_count_ident(expr: &Expr, target: &str) -> usize {
    match expr {
        Expr::Ident(name) if name == target => 1,
        Expr::Unary { operand, .. } => expr_count_ident(&operand.node, target),
        Expr::Binary { left, right, .. } => {
            expr_count_ident(&left.node, target) + expr_count_ident(&right.node, target)
        }
        Expr::Paren(inner) => expr_count_ident(&inner.node, target),
        Expr::If { cond, then_block, else_block } => {
            expr_count_ident(&cond.node, target)
                + (if block_uses_ident(then_block, target) { 1 } else { 0 })
                + (if block_uses_ident(else_block, target) { 1 } else { 0 })
        }
        Expr::Call { callee, args } => {
            let mut c = expr_count_ident(&callee.node, target);
            for a in args {
                c += expr_count_ident(&a.node, target);
            }
            c
        }
        Expr::FieldAccess { base, .. } => expr_count_ident(&base.node, target),
        Expr::ArrayLit(elems) => elems.iter().map(|e| expr_count_ident(&e.node, target)).sum(),
        _ => 0,
    }
}

/// Corrige automáticamente advertencias corregibles del linter (unused-mut y unused-var).
pub fn fix_source(source: &str, _warnings: &[LintWarning]) -> String {
    let mut fixed = source.to_string();
    let Ok(tokens) = crate::lexer::tokenize(&fixed) else { return fixed; };
    let Ok(program) = crate::parser::parse(tokens) else { return fixed; };
    let warnings = lint_program(&program);

    for w in warnings {
        match w.rule {
            "unused-mut" => {
                if let Some(var_name) = w.message.split('\'').nth(1) {
                    let target_mut = format!("let mut {var_name}");
                    let repl = format!("let {var_name}");
                    fixed = fixed.replace(&target_mut, &repl);
                }
            }
            "unused-var" => {
                if let Some(var_name) = w.message.split('\'').nth(1) {
                    let target_let = format!("let {var_name}");
                    let repl = format!("let _{var_name}");
                    fixed = fixed.replace(&target_let, &repl);

                    let target_mut = format!("let mut {var_name}");
                    let repl_mut = format!("let _{var_name}");
                    fixed = fixed.replace(&target_mut, &repl_mut);
                }
            }
            _ => {}
        }
    }
    fixed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer;
    use crate::parser;

    #[test]
    fn test_linter_warns_on_unused_var_and_unused_mut() {
        let code = r#"
            fn calculate(a: Int) -> Int {
                let unused = 42;
                let mut never_changed = 100;
                never_changed + a
            }
            test "empty test" { }
        "#;
        let tokens = lexer::tokenize(code).unwrap();
        let program = parser::parse(tokens).unwrap();
        let warnings = lint_program(&program);

        assert!(warnings.iter().any(|w| w.rule == "unused-var" && w.message.contains("unused")));
        assert!(warnings.iter().any(|w| w.rule == "unused-mut" && w.message.contains("never_changed")));
        assert!(warnings.iter().any(|w| w.rule == "empty-test"));

        let fixed = fix_source(code, &warnings);
        assert!(fixed.contains("let _unused = 42;"));
        assert!(fixed.contains("let never_changed = 100;"));
    }

    // ---- `timing-unsafe-secret-comparison` (GRAMMAR.md §3.88) ----

    fn secret_comparison_warnings(code: &str) -> Vec<LintWarning> {
        let tokens = lexer::tokenize(code).unwrap_or_else(|e| panic!("{e}"));
        let program = parser::parse(tokens).unwrap_or_else(|e| panic!("{e:?}"));
        lint_program(&program).into_iter().filter(|w| w.rule == "timing-unsafe-secret-comparison").collect()
    }

    #[test]
    fn an_ident_named_like_a_secret_compared_with_eq_is_flagged() {
        let code = r#"
            fn check(token: String, expected: String) -> Bool {
                token == expected
            }
        "#;
        let warnings = secret_comparison_warnings(code);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].message.contains("token"), "{warnings:?}");
        assert!(warnings[0].message.contains("timingSafeEqual"), "{warnings:?}");
    }

    #[test]
    fn a_field_access_named_like_a_secret_is_also_flagged() {
        let code = r#"
            type Req = { password: String }
            fn check(r: Req, expected: String) -> Bool {
                r.password == expected
            }
        "#;
        let warnings = secret_comparison_warnings(code);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].message.contains("password"), "{warnings:?}");
    }

    #[test]
    fn not_eq_is_flagged_the_same_as_eq() {
        let code = r#"
            fn check(apiKey: String, expected: String) -> Bool {
                apiKey != expected
            }
        "#;
        let warnings = secret_comparison_warnings(code);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
    }

    #[test]
    fn comparing_against_null_is_a_presence_check_not_flagged() {
        let code = r#"
            fn check(token: String?) -> Bool {
                token != null
            }
        "#;
        assert!(secret_comparison_warnings(code).is_empty());
    }

    #[test]
    fn a_comparison_between_ordinary_names_is_not_flagged() {
        let code = r#"
            fn check(a: Int, b: Int) -> Bool {
                a == b
            }
        "#;
        assert!(secret_comparison_warnings(code).is_empty());
    }

    #[test]
    fn a_secret_comparison_inside_a_while_loop_is_flagged_exactly_once() {
        // Regresión: `lint_block` ya recorre el BODY de un `while` por su
        // cuenta (para unused-var/-mut) -- si esta regla TAMBIÉN recorriera
        // ese mismo bloque desde `Stmt::While`, cada warning adentro de un
        // `while` saldría duplicado.
        let code = r#"
            fn check() -> Bool {
                let mut i = 0;
                let mut found = false;
                while i < 3 {
                    let secret = "x";
                    found = secret == "y";
                    i = i + 1;
                }
                found
            }
        "#;
        let warnings = secret_comparison_warnings(code);
        assert_eq!(warnings.len(), 1, "no debe duplicarse dentro de un while: {warnings:?}");
    }

    #[test]
    fn a_secret_comparison_inside_an_if_branch_and_a_closure_is_found() {
        let code = r#"
            service S {
                rpc check(token: String, expected: String) -> Bool {
                    if true {
                        token == expected
                    } else {
                        false
                    }
                }

                rpc anyMatch(tokens: String[], expected: String) -> Bool {
                    tokens.filter(|token| { token == expected }).length() > 0
                }
            }
        "#;
        let warnings = secret_comparison_warnings(code);
        assert_eq!(warnings.len(), 2, "una en el if, otra en el closure: {warnings:?}");
    }
}

