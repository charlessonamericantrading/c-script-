//! Linter estático para análisis de calidad de código en Link.
//! Detecta variables no utilizadas, mutabilidad redundante y tests vacíos.

use crate::ast::{Block, Expr, Item, Member, Program, Stmt};

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
                    Member::Rpc(r) | Member::Stream(r) => r.annotation.is_some(),
                });
                let has_unauth = s.members.iter().any(|m| match m {
                    Member::Rpc(r) | Member::Stream(r) => r.annotation.is_none(),
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
    }
}
