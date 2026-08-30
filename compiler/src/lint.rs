//! Linter estático para análisis de calidad de código en Link.
//! Detecta variables no utilizadas, mutabilidad redundante y tests vacíos.

use crate::ast::{BinaryOp, Block, ConstDecl, Expr, Item, Member, MatchArmBody, Program, Spanned, Stmt};

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
                // AUDIT-2026-08-27.md #10: un rpc `@cron` nunca puede llevar
                // `@authenticated`/`@requires` (el checker lo prohíbe -- es
                // la ÚNICA anotación que admite, GRAMMAR.md §3.159) y nunca
                // es alcanzable vía HTTP, así que su falta de auth no dice
                // NADA sobre la superficie HTTP real del servicio. Sin
                // excluirlo acá, cualquier servicio con al menos un job
                // `@cron` y al menos un rpc protegido disparaba este lint
                // aunque TODOS los endpoints HTTP reales estuvieran
                // protegidos de manera uniforme -- exactamente el patrón que
                // `@cron` fue diseñado para soportar (una API protegida con
                // un job de limpieza al lado).
                let has_auth = s.members.iter().any(|m| match m {
                    Member::Rpc(r) | Member::Stream(r) => r.cron().is_none() && r.auth().is_some(),
                });
                let has_unauth = s.members.iter().any(|m| match m {
                    Member::Rpc(r) | Member::Stream(r) => r.cron().is_none() && r.auth().is_none(),
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
                    // GRAMMAR.md §3.188: reformulación del lint de
                    // "autorización de fachada" original (esa versión
                    // resultó ser de mala señal -- el caso MÁS COMÚN y
                    // CORRECTO de `@requires(Role.Admin)` nunca llama a
                    // `auth.currentRole()`/`currentUserId()`, así que
                    // exigirlo habría sido ruido constante sobre código
                    // bien escrito). La versión con señal real es la
                    // INVERSA: un rpc que hace su PROPIA verificación
                    // manual de rol adentro del cuerpo, llamando a
                    // `auth.currentRole()`/`currentUserId()`, pero SIN
                    // `@requires`/`@authenticated` en su propia anotación
                    // -- el chequeo real vive en lógica ad-hoc del cuerpo,
                    // así que un bug ahí bypasea todo en silencio (el
                    // rpc sigue pareciendo "protegido" a simple vista).
                    // Mismo criterio que `mixed-service-auth` arriba para
                    // excluir `@cron`: nunca alcanzable vía HTTP, así que
                    // su falta de auth no dice nada real.
                    if rpc.cron().is_none() && rpc.auth().is_none() && block_calls_auth_identity(&rpc.body) {
                        warnings.push(LintWarning {
                            rule: "manual-role-check-without-requires",
                            message: format!(
                                "'{}' llama a auth.currentRole()/currentUserId() para hacer su propia verificación de rol, pero no tiene @requires/@authenticated -- un bug en esa lógica ad-hoc bypasea el chequeo entero en silencio; declará @requires(Role.X) o @authenticated en el rpc",
                                rpc.name
                            ),
                            line: rpc.span.line,
                            col: rpc.span.col,
                        });
                    }
                }
            }
            Item::Const(c) => {
                lint_hardcoded_secret_const(c, &mut warnings);
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
    lint_delete_then_insert_in_block(block, warnings);
}

/// ¿El nombre SUGIERE un secreto? Substring en minúsculas, deliberadamente
/// laxo -- mejor un falso positivo ocasional sobre un identificador raro
/// (`tokenCount`, por ejemplo) que dejar pasar el caso real que esta regla
/// existe para atrapar.
fn looks_like_a_secret(name: &str) -> bool {
    let lower = name.to_lowercase();
    ["secret", "token", "password", "apikey", "api_key"].iter().any(|kw| lower.contains(kw))
}

/// GRAMMAR.md §3.98: ¿`s` tiene la forma de una URL de conexión con
/// credenciales EMBEBIDAS (`esquema://usuario:contraseña@resto`)? Una URL
/// sin credenciales (`postgres://host/db`, un hostname interno sin
/// secreto) NO dispara -- lo que importa acá es la contraseña adentro del
/// literal, no el esquema en sí. Deliberadamente una lista fija de
/// esquemas conocidos, no un parser de URL genérico (RFC 3986 completo) --
/// mismo criterio de "cubrir el caso real, no construir infraestructura
/// nueva" que el resto del proyecto.
fn looks_like_a_connection_url_with_credentials(s: &str) -> bool {
    const SCHEMES: &[&str] = &["postgres://", "postgresql://", "mysql://", "mongodb://", "redis://", "amqp://"];
    let Some(rest) = SCHEMES.iter().find_map(|scheme| s.strip_prefix(scheme)) else {
        return false;
    };
    match rest.find('@') {
        Some(at) => rest[..at].contains(':'),
        None => false,
    }
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
/// `const NOMBRE: String = "literal"` de nivel superior (GRAMMAR.md §3.98):
/// una URL de conexión con credenciales embebidas, o un literal cuyo
/// NOMBRE sugiere un secreto (mismo heurístico laxo que
/// `looks_like_a_secret`, arriba -- mejor un falso positivo ocasional que
/// dejar pasar el caso real). Deliberadamente acotado a `const` de nivel
/// superior -- el lugar más común, y el más fácil de reconocer sin
/// ambigüedad, para "esto es configuración escrita a mano en el código",
/// no un `let` armado dentro de un test o de la lógica de un rpc.
fn lint_hardcoded_secret_const(c: &ConstDecl, warnings: &mut Vec<LintWarning>) {
    let Expr::Str(s) = &c.value.node else { return };
    if s.is_empty() {
        return;
    }
    if looks_like_a_connection_url_with_credentials(s) {
        warnings.push(LintWarning {
            rule: "hardcoded-secret-literal",
            message: format!(
                "'{}' es una URL de conexión con credenciales embebidas escrita literal en el código -- un 'const' solo admite literales (no puede llamar a env.get(...)), así que la forma segura es NO declararlo como const: leelo con env.get(\"...\") en el momento que lo necesites, adentro del rpc/fn que lo usa. Esto termina en el control de versiones tal cual está",
                c.name
            ),
            line: c.span.line,
            col: c.span.col,
        });
    } else if looks_like_a_secret(&c.name) {
        warnings.push(LintWarning {
            rule: "hardcoded-secret-literal",
            message: format!(
                "'{}' se llama como un secreto (token/password/API key) pero su valor es un literal escrito en el código -- si de verdad lo es, un 'const' no es el lugar (solo admite literales, no env.get(...)): leelo con env.get(\"...\") en el momento que lo necesites, adentro del rpc/fn que lo usa, en vez de dejarlo en el .link, que termina en el control de versiones",
                c.name
            ),
            line: c.span.line,
            col: c.span.col,
        });
    }
}

/// Si `expr` es `db.<coleccion>.<method>(args)`, devuelve `(coleccion,
/// args)`. Reconoce SOLO esa forma exacta (`db` como identificador, dos
/// `FieldAccess` anidados, después un `Call`) -- mismo criterio de "shape
/// chico y ancho, no un intérprete de expresiones parcial" que
/// `ast::recognize_field_selector` ya usa para `sumBy`/`maxBy`/etc.
fn recognize_db_call<'a>(expr: &'a Expr, method: &str) -> Option<(&'a str, &'a [Spanned<Expr>])> {
    let Expr::Call { callee, args } = expr else { return None };
    let Expr::FieldAccess { base, field } = &callee.node else { return None };
    if field != method {
        return None;
    }
    let Expr::FieldAccess { base: db_base, field: collection } = &base.node else { return None };
    let Expr::Ident(name) = &db_base.node else { return None };
    if name != "db" {
        return None;
    }
    Some((collection.as_str(), args.as_slice()))
}

/// Representación de texto CANÓNICA de un subconjunto chico de formas
/// (`Ident`, `campo.anidado`, un literal `Int`) -- suficiente para comparar
/// "¿el id que se borró es el mismo que el que se intentó preservar en el
/// insert?" sin necesitar un comparador de expresiones genérico. Cualquier
/// otra forma (una llamada, una expresión aritmética, ...) da `None` --
/// el lint simplemente no dispara ahí, nunca un falso positivo por
/// adivinar mal una equivalencia.
fn simple_expr_key(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Ident(name) => Some(name.clone()),
        Expr::FieldAccess { base, field } => Some(format!("{}.{field}", simple_expr_key(&base.node)?)),
        Expr::Int(n) => Some(n.to_string()),
        _ => None,
    }
}

/// GRAMMAR.md §3.106: `db.<c>.delete(x.id)` seguido, más adelante en el
/// MISMO bloque, de `db.<c>.insert(MismoTipo { id: x.id, ... })` sobre la
/// MISMA colección y con el MISMO id -- un intento de "actualizar
/// borrando y reinsertando" que no hace lo que parece: `insert` SIEMPRE
/// asigna un id nuevo por autoincrement (§3.17), nunca respeta el valor
/// que el literal declara para `id` -- así que la fila resultante queda
/// con OTRO id, rompiendo cualquier referencia externa a la fila
/// original. Encontrado en IgnisLove (`bandit_rewards`/`bot_defense`/
/// `stock_cache`/etc. ya migraron a `upsert`/`applyPatch` citando
/// exactamente este motivo; `banners.link` todavía no). Puramente
/// informativo, como el resto del linter -- `linkc lint` sigue saliendo
/// con código 0.
fn lint_delete_then_insert_in_block(block: &Block, warnings: &mut Vec<LintWarning>) {
    let exprs: Vec<(&Spanned<Expr>, usize, usize)> =
        block.stmts.iter().filter_map(|s| if let Stmt::Expr(e) = &s.node { Some((e, s.span.line, s.span.col)) } else { None }).collect();

    for (i, (expr, _, _)) in exprs.iter().enumerate() {
        let Some((del_coll, del_args)) = recognize_db_call(&expr.node, "delete") else { continue };
        let Some(del_id_expr) = del_args.first() else { continue };
        let Some(del_key) = simple_expr_key(&del_id_expr.node) else { continue };

        for (later, line, col) in &exprs[i + 1..] {
            let Some((ins_coll, ins_args)) = recognize_db_call(&later.node, "insert") else { continue };
            if ins_coll != del_coll {
                continue;
            }
            let Some(Expr::StructLit { fields, .. }) = ins_args.first().map(|a| &a.node) else { continue };
            let Some((_, id_value)) = fields.iter().find(|(name, _)| name == "id") else { continue };
            if simple_expr_key(&id_value.node).as_deref() != Some(del_key.as_str()) {
                continue;
            }
            warnings.push(LintWarning {
                rule: "delete-then-insert-same-id",
                message: format!(
                    "'db.{del_coll}.delete({del_key})' seguido de 'db.{ins_coll}.insert(... id: {del_key} ...)' -- insert() SIEMPRE asigna un id NUEVO por autoincrement, nunca respeta el valor pasado en 'id', así que esto no preserva la fila original (cualquier referencia externa al id viejo queda apuntando a una fila borrada). Usá 'applyPatch'/'upsert' en su lugar"
                ),
                line: *line,
                col: *col,
            });
        }
    }
}

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
        Expr::Transaction(block) => lint_secret_comparisons_in_block(block, warnings),
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
        // GRAMMAR.md §3.115 (issue #11, reportado por IgnisLove): estos
        // seis arms faltaban -- cualquier uso de `target` que solo
        // apareciera DENTRO de uno de ellos era invisible para este
        // contador, así que `unused-var` lo marcaba como no usado aunque sí
        // lo estuviera. El caso real más común: una closure pasada a
        // `.filter()`/`upsert`/`findWhere` (`Expr::Closure`, siempre
        // argumento de un `Expr::Call` que SÍ recorre `args`, pero el
        // contador nunca bajaba adentro del `body` de esa closure) y el
        // valor de un campo de un struct-literal de cola o pasado a
        // `insert`/`upsert` (`Expr::StructLit`, cuyos `fields` nunca se
        // recorrían).
        Expr::Index { base, index } => expr_count_ident(&base.node, target) + expr_count_ident(&index.node, target),
        Expr::TupleLit(elems) => elems.iter().map(|e| expr_count_ident(&e.node, target)).sum(),
        Expr::TupleIndex { base, .. } => expr_count_ident(&base.node, target),
        Expr::StructLit { fields, .. } => fields.iter().map(|(_, v)| expr_count_ident(&v.node, target)).sum(),
        Expr::Closure { body, .. } => usize::from(block_uses_ident(body, target)),
        // GRAMMAR.md §3.154: mismo motivo que el bloque de comentario de
        // arriba -- sin este arm, `target` usado SOLO adentro de un
        // `transaction { ... }` sería invisible para `unused-var`, mismo
        // bug que el de issue #11 pero para esta forma nueva.
        Expr::Transaction(block) => usize::from(block_uses_ident(block, target)),
        Expr::Match { scrutinee, arms } => {
            let mut c = expr_count_ident(&scrutinee.node, target);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    c += expr_count_ident(&guard.node, target);
                }
                c += match &arm.body {
                    MatchArmBody::Expr(e) => expr_count_ident(&e.node, target),
                    MatchArmBody::Block(b) => usize::from(block_uses_ident(b, target)),
                };
            }
            c
        }
        _ => 0,
    }
}

/// GRAMMAR.md §3.188: ¿este bloque llama a `auth.currentRole()`/
/// `auth.currentUserId()` en algún lugar? Mismo recorrido exhaustivo por
/// variante de `Stmt`/`Expr` que `block_uses_ident`/`expr_count_ident` ya
/// establecen arriba -- mismo motivo (GRAMMAR.md §3.115): omitir un arm acá
/// dejaría una llamada real (ej. adentro de una closure o un `match`)
/// invisible para `manual-role-check-without-requires`, el mismo tipo de
/// bug que esa ronda ya encontró y cerró para `unused-var`.
fn block_calls_auth_identity(block: &Block) -> bool {
    for stmt in &block.stmts {
        let found = match &stmt.node {
            Stmt::Let { value, .. } | Stmt::Assign { value, .. } => expr_calls_auth_identity(&value.node),
            Stmt::Expr(e) | Stmt::Return(Some(e)) => expr_calls_auth_identity(&e.node),
            Stmt::Return(None) => false,
            Stmt::While { cond, body } => expr_calls_auth_identity(&cond.node) || block_calls_auth_identity(body),
        };
        if found {
            return true;
        }
    }
    block.tail.as_ref().is_some_and(|t| expr_calls_auth_identity(&t.node))
}

fn expr_calls_auth_identity(expr: &Expr) -> bool {
    match expr {
        Expr::Call { callee, args } => {
            let is_auth_identity_call = matches!(
                &callee.node,
                Expr::FieldAccess { base, field }
                    if matches!(&base.node, Expr::Ident(name) if name == "auth")
                        && (field == "currentRole" || field == "currentUserId")
            );
            is_auth_identity_call || expr_calls_auth_identity(&callee.node) || args.iter().any(|a| expr_calls_auth_identity(&a.node))
        }
        Expr::Unary { operand, .. }
        | Expr::Paren(operand)
        | Expr::FieldAccess { base: operand, .. }
        | Expr::TupleIndex { base: operand, .. } => expr_calls_auth_identity(&operand.node),
        Expr::Binary { left, right, .. } => expr_calls_auth_identity(&left.node) || expr_calls_auth_identity(&right.node),
        Expr::Index { base, index } => expr_calls_auth_identity(&base.node) || expr_calls_auth_identity(&index.node),
        Expr::ArrayLit(elems) | Expr::TupleLit(elems) => elems.iter().any(|e| expr_calls_auth_identity(&e.node)),
        Expr::StructLit { fields, .. } => fields.iter().any(|(_, v)| expr_calls_auth_identity(&v.node)),
        Expr::Closure { body, .. } | Expr::Transaction(body) => block_calls_auth_identity(body),
        Expr::If { cond, then_block, else_block } => {
            expr_calls_auth_identity(&cond.node) || block_calls_auth_identity(then_block) || block_calls_auth_identity(else_block)
        }
        Expr::Match { scrutinee, arms } => {
            expr_calls_auth_identity(&scrutinee.node)
                || arms.iter().any(|arm| {
                    arm.guard.as_ref().is_some_and(|g| expr_calls_auth_identity(&g.node))
                        || match &arm.body {
                            MatchArmBody::Expr(e) => expr_calls_auth_identity(&e.node),
                            MatchArmBody::Block(b) => block_calls_auth_identity(b),
                        }
                })
        }
        _ => false,
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

    // ---- 14 falsos positivos de `unused-var` (GRAMMAR.md §3.115, issue
    // #11 reportado por IgnisLove): `expr_count_ident` no bajaba adentro
    // del `body` de una `Expr::Closure` ni de los valores de
    // `Expr::StructLit`, así que una variable usada SOLO ahí adentro se
    // marcaba como no usada. Los tres tests que siguen son los tres repros
    // exactos del issue, tal cual los reportaron -- no simplificados.

    fn lint_warnings(code: &str) -> Vec<LintWarning> {
        let tokens = lexer::tokenize(code).unwrap_or_else(|e| panic!("{e}"));
        let program = parser::parse(tokens).unwrap_or_else(|e| panic!("{e:?}"));
        lint_program(&program)
    }

    #[test]
    fn mixed_service_auth_still_fires_for_a_genuinely_public_rpc_next_to_a_protected_one() {
        let code = r#"
            service Jobs {
                @authenticated
                rpc me() -> Void { }
                rpc ping() -> Void { }
            }
        "#;
        let warnings = lint_warnings(code);
        assert!(warnings.iter().any(|w| w.rule == "mixed-service-auth"), "{warnings:?}");
    }

    /// AUDIT-2026-08-27.md #10: un rpc `@cron` nunca puede llevar
    /// `@authenticated`/`@requires` y nunca es alcanzable vía HTTP -- antes
    /// de este fix, un servicio con SOLO rpcs protegidos más un job `@cron`
    /// disparaba `mixed-service-auth` igual, aunque ningún endpoint HTTP
    /// real quedara sin protección.
    #[test]
    fn mixed_service_auth_does_not_fire_for_a_cron_job_next_to_protected_rpcs() {
        let code = r#"
            service Jobs {
                @authenticated rpc me() -> Void { }
                @authenticated rpc me2() -> Void { }
                @cron("5m") rpc sweep() -> Void { }
            }
        "#;
        let warnings = lint_warnings(code);
        assert!(
            !warnings.iter().any(|w| w.rule == "mixed-service-auth"),
            "un @cron no debería contar como rpc 'público' para este lint: {warnings:?}"
        );
    }

    // ---- `manual-role-check-without-requires` (GRAMMAR.md §3.188) ----

    #[test]
    fn a_manual_role_check_with_no_requires_annotation_is_flagged() {
        let code = r#"
            enum Role { Admin, Member }
            service S {
                rpc deleteUser(id: Int) -> Bool {
                    if auth.currentRole() != "Admin" {
                        panic("no autorizado");
                    } else {
                    }
                    true
                }
            }
        "#;
        let warnings = lint_warnings(code);
        let hit = warnings.iter().find(|w| w.rule == "manual-role-check-without-requires");
        assert!(hit.is_some(), "{warnings:?}");
        assert!(hit.unwrap().message.contains("deleteUser"), "{warnings:?}");
    }

    #[test]
    fn a_manual_check_of_current_user_id_with_no_requires_is_also_flagged() {
        let code = r#"
            service S {
                rpc me(id: Int) -> Bool { auth.currentUserId() == id }
            }
        "#;
        let warnings = lint_warnings(code);
        assert!(warnings.iter().any(|w| w.rule == "manual-role-check-without-requires"), "{warnings:?}");
    }

    /// Caso legítimo, no un bug: el rpc SÍ tiene `@requires` -- el chequeo
    /// real ya vive en la anotación (aplicado ANTES de que el cuerpo
    /// corra), así que llamar a `auth.currentRole()` adentro del cuerpo
    /// (para lógica adicional, no para el gate de autorización en sí) es
    /// redundante como mucho, nunca la única defensa.
    #[test]
    fn a_manual_check_alongside_a_real_requires_annotation_is_not_flagged() {
        let code = r#"
            enum Role { Admin, Member }
            service S {
                @requires(Role.Admin)
                rpc deleteUser(id: Int) -> Bool {
                    if auth.currentRole() != "Admin" {
                        panic("no autorizado");
                    } else {
                    }
                    true
                }
            }
        "#;
        let warnings = lint_warnings(code);
        assert!(
            !warnings.iter().any(|w| w.rule == "manual-role-check-without-requires"),
            "un rpc que YA tiene @requires no es el caso que este lint busca: {warnings:?}"
        );
    }

    /// Mismo criterio que `mixed-service-auth`: un `@cron` nunca es
    /// alcanzable vía HTTP, así que su falta de `@requires` no es una
    /// superficie real, aunque su cuerpo llame a `auth.currentRole()` (algo
    /// que ni siquiera tendría sentido, pero no es este lint el que debe
    /// avisarlo).
    #[test]
    fn a_cron_job_calling_auth_identity_is_not_flagged() {
        let code = r#"
            service S {
                @cron("5m")
                rpc sweep() -> Void { let r = auth.currentRole(); }
            }
        "#;
        let warnings = lint_warnings(code);
        assert!(
            !warnings.iter().any(|w| w.rule == "manual-role-check-without-requires"),
            "un @cron nunca es alcanzable vía HTTP: {warnings:?}"
        );
    }

    #[test]
    fn a_body_with_no_auth_identity_call_at_all_is_not_flagged() {
        let code = r#"
            service S {
                rpc ping() -> Void { }
            }
        "#;
        let warnings = lint_warnings(code);
        assert!(!warnings.iter().any(|w| w.rule == "manual-role-check-without-requires"), "{warnings:?}");
    }

    /// El caso de issue #11 (GRAMMAR.md §3.115), pero para ESTE detector:
    /// una llamada a `auth.currentRole()` que aparece SOLO adentro de una
    /// closure (o un `match`) tiene que ser visible igual -- si
    /// `expr_calls_auth_identity` alguna vez perdiera un arm, este es
    /// exactamente el tipo de falso negativo silencioso que reaparecería.
    #[test]
    fn an_auth_identity_call_inside_a_closure_or_match_is_still_detected() {
        let code = r#"
            service S {
                rpc a(ids: Int[]) -> Int[] {
                    ids.filter(|x: Int| { auth.currentUserId() == x })
                }
            }
        "#;
        assert!(lint_warnings(code).iter().any(|w| w.rule == "manual-role-check-without-requires"), "closure case");

        let code2 = r#"
            service S {
                rpc b(id: Int?) -> Bool {
                    match id {
                        n: Int => auth.currentUserId() == n,
                        null => false,
                    }
                }
            }
        "#;
        assert!(lint_warnings(code2).iter().any(|w| w.rule == "manual-role-check-without-requires"), "match case");
    }

    #[test]
    fn a_variable_used_only_inside_a_filter_closure_is_not_a_false_positive() {
        // Repro 1 del issue: `target` se usa DOS veces, pero las dos
        // adentro del `body` de la closure que `.filter()` recibe como
        // argumento -- antes de esta ronda, `Expr::Call` sí recorría sus
        // `args`, pero `expr_count_ident` no sabía bajar adentro de un
        // `Expr::Closure` una vez que llegaba a él.
        let code = r#"
            type FacetItem = { id: Int, productId: Int, category: String, inStock: Bool }
            db { facets: FacetItem[] }
            service S {
                rpc queryFacetCounts(category: String) -> Int {
                    let target = category.toLower();
                    let matches = db.facets.all().filter(|f: FacetItem| {
                        target == "all" || f.category == target
                    });
                    matches.length()
                }
            }
        "#;
        let warnings = lint_warnings(code);
        assert!(
            !warnings.iter().any(|w| w.rule == "unused-var" && w.message.contains("target")),
            "'target' se usa dos veces adentro de la closure de .filter(): {warnings:?}"
        );
    }

    #[test]
    fn a_variable_used_only_as_a_tail_struct_literal_field_value_is_not_a_false_positive() {
        // Repro 2 del issue: las tres variables (`total`, `inStock`,
        // `outOfStock`) se usan de la MISMA forma, como valor de un campo
        // del mismo struct-literal de cola -- pero antes de esta ronda solo
        // `outOfStock` se marcaba (falso positivo), porque `total`/
        // `inStock` tenían un uso ADICIONAL fuera del struct-literal
        // (`total - inStock`) que alcanzaba para no disparar la regla; el
        // bug real es que NINGUNA de las tres debería depender de ese uso
        // extra -- `expr_count_ident` nunca recorría `Expr::StructLit`.
        let code = r#"
            type FacetCounts = { total: Int, inStock: Int, outOfStock: Int }
            service S {
                rpc counts(total: Int, inStock: Int) -> FacetCounts {
                    let outOfStock = total - inStock;
                    FacetCounts {
                        total: total,
                        inStock: inStock,
                        outOfStock: outOfStock,
                    }
                }
            }
        "#;
        let warnings = lint_warnings(code);
        assert!(
            !warnings.iter().any(|w| w.rule == "unused-var"),
            "las tres variables se usan como valor de un campo del struct-literal de cola: {warnings:?}"
        );
    }

    #[test]
    fn variables_used_only_inside_upsert_closures_and_struct_literals_are_not_false_positives() {
        // Repro 3 del issue: `reward` se usa en el struct-literal del
        // segundo argumento de `upsert` (`insertValue`) Y adentro del
        // `body` de la closure del tercer argumento (`updateFn`) --
        // ninguno de los dos contaba antes de esta ronda.
        let code = r#"
            type Arm = { id: Int, code: String, pulls: Int, total: Int }
            db { arms: Arm[] }
            service S {
                rpc bump(code: String, reward: Int) -> Arm {
                    db.arms.upsert(
                        |a: Arm| { a.code == code },
                        Arm { id: 0, code: code, pulls: 1, total: reward },
                        |existing: Arm| {
                            Arm { id: 0, code: code, pulls: existing.pulls + 1, total: existing.total + reward }
                        }
                    )
                }
            }
        "#;
        let warnings = lint_warnings(code);
        assert!(
            !warnings.iter().any(|w| w.rule == "unused-var" && (w.message.contains("reward") || w.message.contains("code"))),
            "'reward' y 'code' se usan adentro de struct-literals y closures de upsert: {warnings:?}"
        );
    }

    #[test]
    fn a_variable_used_only_inside_a_match_arm_is_not_a_false_positive() {
        // Mismo bug de fondo (`expr_count_ident` sin arm para
        // `Expr::Match`), un caso que el issue no citó con un `.link` real
        // pero que comparte la misma causa raíz -- cubierto de una vez.
        let code = r#"
            fn describe(x: Int?) -> String {
                let label = "valor";
                match x {
                    v: Int => label + ": " + v.toString(),
                    null => "sin " + label,
                }
            }
        "#;
        let warnings = lint_warnings(code);
        assert!(
            !warnings.iter().any(|w| w.rule == "unused-var" && w.message.contains("label")),
            "'label' se usa en los dos arms del match: {warnings:?}"
        );
    }

    #[test]
    fn a_genuinely_unused_variable_is_still_flagged_after_the_fix() {
        // No-regresión: el fix de arriba no debe volver el linter ciego a
        // un caso real -- una variable que NO se usa en ningún lado (ni
        // adentro de una closure, ni de un struct-literal) sigue
        // marcándose.
        let code = r#"
            service S {
                rpc f(name: String) -> String {
                    let reallyUnused = name.toLower();
                    name
                }
            }
        "#;
        let warnings = lint_warnings(code);
        assert!(
            warnings.iter().any(|w| w.rule == "unused-var" && w.message.contains("reallyUnused")),
            "una variable de verdad sin usar tiene que seguir marcándose: {warnings:?}"
        );
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

    // ---- `hardcoded-secret-literal` (GRAMMAR.md §3.98) ----

    fn hardcoded_secret_warnings(code: &str) -> Vec<LintWarning> {
        let tokens = lexer::tokenize(code).unwrap_or_else(|e| panic!("{e}"));
        let program = parser::parse(tokens).unwrap_or_else(|e| panic!("{e:?}"));
        lint_program(&program).into_iter().filter(|w| w.rule == "hardcoded-secret-literal").collect()
    }

    #[test]
    fn a_connection_url_with_embedded_credentials_is_flagged() {
        let code = r#"const DB_URL: String = "postgres://admin:supersecret@prod-db.internal:5432/app";"#;
        let warnings = hardcoded_secret_warnings(code);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].message.contains("DB_URL"), "{warnings:?}");
        assert!(warnings[0].message.contains("env.get"), "{warnings:?}");
    }

    #[test]
    fn a_connection_url_without_credentials_is_not_flagged() {
        // Un hostname interno sin contraseña no es un secreto -- lo que
        // importa es la credencial adentro, no el esquema en sí.
        let code = r#"const DB_HOST: String = "postgres://internal-db.local/app";"#;
        assert!(hardcoded_secret_warnings(code).is_empty());
    }

    #[test]
    fn a_const_named_like_a_secret_with_a_literal_value_is_flagged() {
        let code = r#"const API_SECRET: String = "sk_live_abc123def456";"#;
        let warnings = hardcoded_secret_warnings(code);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].message.contains("API_SECRET"), "{warnings:?}");
    }

    #[test]
    fn a_const_with_an_ordinary_name_and_value_is_not_flagged() {
        let code = r#"
            const MAX_ITEMS: Int = 100;
            const APP_NAME: String = "c-script demo";
        "#;
        assert!(hardcoded_secret_warnings(code).is_empty());
    }

    #[test]
    fn an_empty_string_literal_is_never_flagged_even_with_a_secret_like_name() {
        // Un placeholder vacío ("" -- se llena después, ej. vía env.get) no
        // es un secreto REAL escrito en el código -- nada que advertir.
        let code = r#"const API_TOKEN: String = "";"#;
        assert!(hardcoded_secret_warnings(code).is_empty());
    }

    #[test]
    fn a_non_literal_const_value_is_never_flagged() {
        // No es un literal (`Expr::Str`), así que la regla ni siquiera lo
        // mira -- irrelevante que el checker (`check_const`, checker.rs)
        // vaya a rechazar esto por otro motivo (un 'const' solo admite
        // literales, nunca una llamada como env.get(...)): el lint corre
        // sobre el AST parseado, antes/aparte del checker.
        let code = r#"const DB_URL: String = env.get("LINK_DATABASE_URL");"#;
        assert!(hardcoded_secret_warnings(code).is_empty());
    }

    // ---- `delete-then-insert-same-id` (GRAMMAR.md §3.106) ----

    fn delete_then_insert_warnings(code: &str) -> Vec<LintWarning> {
        let tokens = lexer::tokenize(code).unwrap_or_else(|e| panic!("{e}"));
        let program = parser::parse(tokens).unwrap_or_else(|e| panic!("{e:?}"));
        lint_program(&program).into_iter().filter(|w| w.rule == "delete-then-insert-same-id").collect()
    }

    #[test]
    fn delete_then_insert_with_the_same_id_on_the_same_collection_is_flagged() {
        let code = r#"
            type Banner = { id: Int, name: String, impressionsCount: Int }
            db { banners: Banner[] }
            service S {
                rpc bump(x: Banner) -> Void {
                    db.banners.delete(x.id);
                    db.banners.insert(Banner { id: x.id, name: x.name, impressionsCount: x.impressionsCount + 1 });
                }
            }
        "#;
        let warnings = delete_then_insert_warnings(code);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].message.contains("banners"), "{warnings:?}");
        assert!(warnings[0].message.contains("applyPatch"), "{warnings:?}");
    }

    #[test]
    fn delete_then_insert_on_a_different_collection_is_not_flagged() {
        // Archivar (borrar de una colección, insertar en OTRA) es un
        // patrón legítimo -- no es "actualizar borrando y reinsertando".
        let code = r#"
            type Item = { id: Int, name: String }
            type Archive = { id: Int, name: String }
            db { items: Item[], archive: Archive[] }
            service S {
                rpc archiveIt(x: Item) -> Void {
                    db.items.delete(x.id);
                    db.archive.insert(Archive { id: 0, name: x.name });
                }
            }
        "#;
        assert!(delete_then_insert_warnings(code).is_empty());
    }

    #[test]
    fn delete_then_insert_with_a_different_id_is_not_flagged() {
        // Borrar una fila e insertar OTRA fila distinta en la misma
        // colección es normal -- lo que dispara el lint es preservar EL
        // MISMO id, la señal de que se está tratando de "actualizar".
        let code = r#"
            type Item = { id: Int, name: String }
            db { items: Item[] }
            service S {
                rpc replace(x: Item, other: Item) -> Void {
                    db.items.delete(x.id);
                    db.items.insert(Item { id: other.id, name: other.name });
                }
            }
        "#;
        assert!(delete_then_insert_warnings(code).is_empty());
    }

    #[test]
    fn a_plain_insert_with_no_preceding_delete_is_not_flagged() {
        let code = r#"
            type Item = { id: Int, name: String }
            db { items: Item[] }
            service S {
                rpc create(name: String) -> Item { db.items.insert(Item { id: 0, name: name }) }
            }
        "#;
        assert!(delete_then_insert_warnings(code).is_empty());
    }
}

