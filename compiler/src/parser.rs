// Parser recursivo descendente que produce el AST (ast.rs) a partir de los
// tokens del lexer, siguiendo GRAMMAR.md §2. Cada función de parseo lleva el
// nombre de la producción EBNF que implementa para poder ir y volver al
// documento sin traducir mentalmente.

use crate::ast::*;
use crate::token::{Span, Token, TokenKind};

#[derive(Debug)]
pub struct ParseError {
    pub message: String,
    pub span: Span,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "error de sintaxis en línea {}: {}", self.span.line, self.message)
    }
}

pub fn parse(tokens: Vec<Token>) -> Result<Program, ParseError> {
    Parser { tokens, pos: 0 }.parse_program()
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> &TokenKind {
        &self.tokens[self.pos].kind
    }

    fn peek_at(&self, offset: usize) -> &TokenKind {
        self.tokens
            .get(self.pos + offset)
            .map(|t| &t.kind)
            .unwrap_or(&TokenKind::Eof)
    }

    fn span(&self) -> Span {
        self.tokens[self.pos].span
    }

    fn advance(&mut self) -> Token {
        let t = self.tokens[self.pos].clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        t
    }

    fn check(&self, kind: &TokenKind) -> bool {
        self.peek() == kind
    }

    fn eat(&mut self, kind: &TokenKind) -> Result<Token, ParseError> {
        if self.check(kind) {
            Ok(self.advance())
        } else {
            Err(self.error(format!(
                "se esperaba {kind:?}, se encontró {:?}",
                self.peek()
            )))
        }
    }

    fn eat_ident(&mut self) -> Result<String, ParseError> {
        match self.peek().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                Ok(name)
            }
            other => Err(self.error(format!("se esperaba un identificador, se encontró {other:?}"))),
        }
    }

    fn eat_string(&mut self) -> Result<String, ParseError> {
        match self.peek().clone() {
            TokenKind::Str(s) => {
                self.advance();
                Ok(s)
            }
            other => Err(self.error(format!("se esperaba un string, se encontró {other:?}"))),
        }
    }

    fn error(&self, message: impl Into<String>) -> ParseError {
        ParseError {
            message: message.into(),
            span: self.span(),
        }
    }

    // ---- §2.1 Programa e ítems de nivel superior ----

    fn parse_program(&mut self) -> Result<Program, ParseError> {
        let mut items = Vec::new();
        while !self.check(&TokenKind::Eof) {
            items.push(self.parse_item()?);
        }
        Ok(Program { items })
    }

    fn parse_item(&mut self) -> Result<Item, ParseError> {
        match self.peek().clone() {
            TokenKind::Import => Ok(Item::Import(self.parse_import_decl()?)),
            TokenKind::Type => Ok(Item::Type(self.parse_type_decl()?)),
            TokenKind::Enum => Ok(Item::Enum(self.parse_enum_decl()?)),
            TokenKind::Service => Ok(Item::Service(self.parse_service_decl()?)),
            TokenKind::Const => Ok(Item::Const(self.parse_const_decl()?)),
            TokenKind::Fn => Ok(Item::Fn(self.parse_fn_decl()?)),
            other => Err(self.error(format!(
                "se esperaba un ítem de nivel superior (import/type/enum/service/const/fn), se encontró {other:?}"
            ))),
        }
    }

    fn parse_import_decl(&mut self) -> Result<ImportDecl, ParseError> {
        self.eat(&TokenKind::Import)?;
        self.eat(&TokenKind::LBrace)?;
        let mut names = vec![self.eat_ident()?];
        while self.check(&TokenKind::Comma) {
            self.advance();
            names.push(self.eat_ident()?);
        }
        self.eat(&TokenKind::RBrace)?;
        self.eat(&TokenKind::From)?;
        let from = self.eat_string()?;
        self.eat(&TokenKind::Semi)?;
        Ok(ImportDecl { names, from })
    }

    fn parse_type_params(&mut self) -> Result<Vec<String>, ParseError> {
        let mut params = Vec::new();
        if self.check(&TokenKind::Lt) {
            self.advance();
            params.push(self.eat_ident()?);
            while self.check(&TokenKind::Comma) {
                self.advance();
                params.push(self.eat_ident()?);
            }
            self.eat(&TokenKind::Gt)?;
        }
        Ok(params)
    }

    /// El `;` final es opcional (ver nota en GRAMMAR.md §2.1): `type X = {...}`
    /// ya termina en `}`, exigir además `;` es la incomodidad que Rust/Go evitan.
    fn parse_type_decl(&mut self) -> Result<TypeDecl, ParseError> {
        self.eat(&TokenKind::Type)?;
        let name = self.eat_ident()?;
        let type_params = self.parse_type_params()?;
        self.eat(&TokenKind::Equals)?;
        let ty = self.parse_type_expr()?;
        if self.check(&TokenKind::Semi) {
            self.advance();
        }
        Ok(TypeDecl { name, type_params, ty })
    }

    fn parse_enum_decl(&mut self) -> Result<EnumDecl, ParseError> {
        self.eat(&TokenKind::Enum)?;
        let name = self.eat_ident()?;
        let type_params = self.parse_type_params()?;
        self.eat(&TokenKind::LBrace)?;
        let mut variants = Vec::new();
        if !self.check(&TokenKind::RBrace) {
            variants.push(self.parse_variant()?);
            while self.check(&TokenKind::Comma) {
                self.advance();
                if self.check(&TokenKind::RBrace) {
                    break; // coma final
                }
                variants.push(self.parse_variant()?);
            }
        }
        self.eat(&TokenKind::RBrace)?;
        Ok(EnumDecl {
            name,
            type_params,
            variants,
        })
    }

    fn parse_variant(&mut self) -> Result<Variant, ParseError> {
        let name = self.eat_ident()?;
        let fields = if self.check(&TokenKind::LBrace) {
            self.advance();
            let fs = self.parse_field_list()?;
            self.eat(&TokenKind::RBrace)?;
            Some(fs)
        } else {
            None
        };
        Ok(Variant { name, fields })
    }

    /// field_list — usado tanto por variantes de enum como por structs inline.
    fn parse_field_list(&mut self) -> Result<Vec<Field>, ParseError> {
        let mut fields = Vec::new();
        if !self.check(&TokenKind::RBrace) {
            fields.push(self.parse_field()?);
            while self.check(&TokenKind::Comma) {
                self.advance();
                if self.check(&TokenKind::RBrace) {
                    break;
                }
                fields.push(self.parse_field()?);
            }
        }
        Ok(fields)
    }

    fn parse_field(&mut self) -> Result<Field, ParseError> {
        let name = self.eat_ident()?;
        let optional = if self.check(&TokenKind::Question) {
            self.advance();
            true
        } else {
            false
        };
        self.eat(&TokenKind::Colon)?;
        let ty = self.parse_type_expr()?;
        Ok(Field { name, optional, ty })
    }

    fn parse_const_decl(&mut self) -> Result<ConstDecl, ParseError> {
        self.eat(&TokenKind::Const)?;
        let name = self.eat_ident()?;
        self.eat(&TokenKind::Colon)?;
        let ty = self.parse_type_expr()?;
        self.eat(&TokenKind::Equals)?;
        let value = self.parse_expr()?;
        self.eat(&TokenKind::Semi)?;
        Ok(ConstDecl { name, ty, value })
    }

    fn parse_service_decl(&mut self) -> Result<ServiceDecl, ParseError> {
        self.eat(&TokenKind::Service)?;
        let name = self.eat_ident()?;
        self.eat(&TokenKind::LBrace)?;
        let mut members = Vec::new();
        while !self.check(&TokenKind::RBrace) {
            members.push(self.parse_member()?);
        }
        self.eat(&TokenKind::RBrace)?;
        Ok(ServiceDecl { name, members })
    }

    fn parse_member(&mut self) -> Result<Member, ParseError> {
        match self.peek().clone() {
            TokenKind::Rpc => Ok(Member::Rpc(self.parse_rpc_like(TokenKind::Rpc)?)),
            TokenKind::Stream => Ok(Member::Stream(self.parse_rpc_like(TokenKind::Stream)?)),
            other => Err(self.error(format!("se esperaba 'rpc' o 'stream', se encontró {other:?}"))),
        }
    }

    fn parse_rpc_like(&mut self, kw: TokenKind) -> Result<RpcDecl, ParseError> {
        self.eat(&kw)?;
        let name = self.eat_ident()?;
        self.eat(&TokenKind::LParen)?;
        let params = self.parse_param_list()?;
        self.eat(&TokenKind::RParen)?;
        self.eat(&TokenKind::Arrow)?;
        let return_type = self.parse_type_expr()?;
        let body = self.parse_block()?;
        Ok(RpcDecl {
            name,
            params,
            return_type,
            body,
        })
    }

    fn parse_fn_decl(&mut self) -> Result<FnDecl, ParseError> {
        self.eat(&TokenKind::Fn)?;
        let name = self.eat_ident()?;
        self.eat(&TokenKind::LParen)?;
        let params = self.parse_param_list()?;
        self.eat(&TokenKind::RParen)?;
        self.eat(&TokenKind::Arrow)?;
        let return_type = self.parse_type_expr()?;
        let body = self.parse_block()?;
        Ok(FnDecl {
            name,
            params,
            return_type,
            body,
        })
    }

    fn parse_param_list(&mut self) -> Result<Vec<Param>, ParseError> {
        let mut params = Vec::new();
        if !self.check(&TokenKind::RParen) {
            params.push(self.parse_param()?);
            while self.check(&TokenKind::Comma) {
                self.advance();
                if self.check(&TokenKind::RParen) {
                    break;
                }
                params.push(self.parse_param()?);
            }
        }
        Ok(params)
    }

    fn parse_param(&mut self) -> Result<Param, ParseError> {
        let name = self.eat_ident()?;
        self.eat(&TokenKind::Colon)?;
        let ty = self.parse_type_expr()?;
        let default = if self.check(&TokenKind::Equals) {
            self.advance();
            Some(self.parse_expr()?)
        } else {
            None
        };
        Ok(Param { name, ty, default })
    }

    // ---- §2.2 Expresiones de tipo ----

    fn parse_type_expr(&mut self) -> Result<TypeExpr, ParseError> {
        let first = self.parse_postfix_type()?;
        if self.check(&TokenKind::Pipe) {
            let mut variants = vec![first];
            while self.check(&TokenKind::Pipe) {
                self.advance();
                variants.push(self.parse_postfix_type()?);
            }
            Ok(TypeExpr::Union(variants))
        } else {
            Ok(first)
        }
    }

    /// primary_type , { type_postfix_op } — el orden de '?' y '[]' importa
    /// (GRAMMAR.md §2.2 insight), por eso esto es un loop, no dos campos fijos.
    fn parse_postfix_type(&mut self) -> Result<TypeExpr, ParseError> {
        let mut ty = self.parse_primary_type()?;
        loop {
            match self.peek().clone() {
                TokenKind::Question => {
                    self.advance();
                    ty = TypeExpr::Optional(Box::new(ty));
                }
                TokenKind::LBracket => {
                    self.advance();
                    self.eat(&TokenKind::RBracket)?;
                    ty = TypeExpr::List(Box::new(ty));
                }
                _ => break,
            }
        }
        Ok(ty)
    }

    /// NOTA: la forma `{ type_expr : type_expr }` (map) no se implementa acá
    /// — es ambigua con un struct de un solo campo sin coma final (ver nota
    /// en GRAMMAR.md §2.2). Usar `Map<K, V>` mientras tanto.
    fn parse_primary_type(&mut self) -> Result<TypeExpr, ParseError> {
        match self.peek().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                let args = if self.check(&TokenKind::Lt) {
                    self.advance();
                    let mut args = vec![self.parse_type_expr()?];
                    while self.check(&TokenKind::Comma) {
                        self.advance();
                        args.push(self.parse_type_expr()?);
                    }
                    self.eat(&TokenKind::Gt)?;
                    args
                } else {
                    Vec::new()
                };
                Ok(TypeExpr::Named(name, args))
            }
            TokenKind::LBrace => {
                self.advance();
                let fields = self.parse_field_list()?;
                self.eat(&TokenKind::RBrace)?;
                Ok(TypeExpr::Struct(fields))
            }
            TokenKind::LParen => self.parse_paren_type(),
            other => Err(self.error(format!("se esperaba un tipo, se encontró {other:?}"))),
        }
    }

    /// Agrupación `(A)`, tupla `(A, B, ...)` o tipo función `(A, B) -> C`.
    /// Las tres formas empiezan igual; se desambiguan DESPUÉS de cerrar el
    /// paréntesis, mirando si sigue `->` (función) y si hubo coma (tupla vs
    /// agrupación pura) — ver GRAMMAR.md §2.2.
    fn parse_paren_type(&mut self) -> Result<TypeExpr, ParseError> {
        self.eat(&TokenKind::LParen)?;
        let mut items = Vec::new();
        let mut had_comma = false;
        if !self.check(&TokenKind::RParen) {
            items.push(self.parse_type_expr()?);
            while self.check(&TokenKind::Comma) {
                self.advance();
                had_comma = true;
                if self.check(&TokenKind::RParen) {
                    break; // coma final: (A,)
                }
                items.push(self.parse_type_expr()?);
            }
        }
        self.eat(&TokenKind::RParen)?;

        if self.check(&TokenKind::Arrow) {
            self.advance();
            let ret = self.parse_type_expr()?;
            return Ok(TypeExpr::Function(items, Box::new(ret)));
        }

        if items.is_empty() {
            return Err(self.error(
                "'()' vacío solo es válido como parámetros de un tipo función: () -> T",
            ));
        }

        if items.len() == 1 && !had_comma {
            Ok(items.into_iter().next().unwrap()) // agrupación pura, ver GRAMMAR.md §2.2
        } else {
            Ok(TypeExpr::Tuple(items))
        }
    }

    // ---- §2.3 Expresiones, sentencias y patrones ----

    fn parse_block(&mut self) -> Result<Block, ParseError> {
        self.eat(&TokenKind::LBrace)?;
        let mut stmts = Vec::new();
        let mut tail = None;
        while !self.check(&TokenKind::RBrace) {
            match self.peek().clone() {
                TokenKind::Let => stmts.push(self.parse_let_stmt()?),
                TokenKind::Return => stmts.push(self.parse_return_stmt()?),
                // `identifier =` (y no `==`, ya son tokens distintos) es una
                // asignación -- se detecta con 1 token de lookahead antes de
                // caer al parseo genérico de expresión, igual que la
                // desambiguación de struct_or_variant_lit (GRAMMAR.md §2.2).
                TokenKind::Ident(name) if matches!(self.peek_at(1), TokenKind::Equals) => {
                    self.advance();
                    self.advance(); // '='
                    let value = self.parse_expr()?;
                    self.eat(&TokenKind::Semi)?;
                    stmts.push(Stmt::Assign { name, value });
                }
                // `if`/`match` son "block-like": terminan en '}', así que no
                // deberían necesitar un ';' para seguir siendo una sentencia
                // seguida de más código (`if cond { x = 1; } else { x = 2; }`
                // sin ';' y con algo más abajo). Sin este caso, el `_` de
                // abajo los trataría como el tail apenas ve que no hay ';',
                // y rompería con cualquier código real después.
                TokenKind::If | TokenKind::Match => {
                    let e = self.parse_expr()?;
                    if self.check(&TokenKind::RBrace) {
                        tail = Some(Box::new(e));
                        break;
                    }
                    if self.check(&TokenKind::Semi) {
                        self.advance(); // ';' opcional acá, no obligatorio
                    }
                    stmts.push(Stmt::Expr(e));
                }
                _ => {
                    let e = self.parse_expr()?;
                    if self.check(&TokenKind::Semi) {
                        self.advance();
                        stmts.push(Stmt::Expr(e));
                    } else {
                        tail = Some(Box::new(e));
                        break;
                    }
                }
            }
        }
        self.eat(&TokenKind::RBrace)?;
        Ok(Block { stmts, tail })
    }

    fn parse_let_stmt(&mut self) -> Result<Stmt, ParseError> {
        self.eat(&TokenKind::Let)?;
        let mutable = if self.check(&TokenKind::Mut) {
            self.advance();
            true
        } else {
            false
        };
        let name = self.eat_ident()?;
        let ty = if self.check(&TokenKind::Colon) {
            self.advance();
            Some(self.parse_type_expr()?)
        } else {
            None
        };
        self.eat(&TokenKind::Equals)?;
        let value = self.parse_expr()?;
        self.eat(&TokenKind::Semi)?;
        Ok(Stmt::Let {
            name,
            mutable,
            ty,
            value,
        })
    }

    fn parse_return_stmt(&mut self) -> Result<Stmt, ParseError> {
        self.eat(&TokenKind::Return)?;
        let value = if self.check(&TokenKind::Semi) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        self.eat(&TokenKind::Semi)?;
        Ok(Stmt::Return(value))
    }

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_expr_ctx(false)
    }

    /// `no_struct_lit`: true solo para el escrutinio de `match`/`if` (ver nota
    /// de implementación en GRAMMAR.md §2.3) — evita que `match x { ... }`
    /// confunda el `{` de los arms con un literal de struct `x { ... }`. Se
    /// "resetea" a false apenas se entra a paréntesis, argumentos o el
    /// cuerpo de un bloque, igual que la restricción NoStructLiteral de Rust.
    /// Se propaga por TODA la cadena de precedencia (no solo el primer
    /// token) porque la ambigüedad puede aparecer en cualquier operando:
    /// `match a + Foo { x: 1 } { ... }` es tan ambiguo como `match Foo { ... }`.
    fn parse_expr_ctx(&mut self, no_struct_lit: bool) -> Result<Expr, ParseError> {
        if self.check(&TokenKind::Match) {
            self.parse_match_expr()
        } else if self.check(&TokenKind::If) {
            self.parse_if_expr()
        } else {
            self.parse_or_expr(no_struct_lit)
        }
    }

    fn parse_if_expr(&mut self) -> Result<Expr, ParseError> {
        self.eat(&TokenKind::If)?;
        // La condición siempre restringe struct-lits, sin importar el
        // contexto exterior: `if x { ... }` es ambiguo igual que `match`.
        let cond = Box::new(self.parse_or_expr(true)?);
        let then_block = self.parse_block()?;
        self.eat(&TokenKind::Else)?;
        let else_block = if self.check(&TokenKind::If) {
            // `else if` -- GRAMMAR.md §2.3 permite anidar if_expr acá. Se
            // envuelve en un Block cuyo tail es el if anidado para que
            // Expr::If siempre tenga dos Block (ver ast.rs), no un Expr
            // suelto -- eval_block/check_block ya saben evaluar cualquier
            // Expr como tail, así que esto no pierde generalidad.
            let nested = self.parse_if_expr()?;
            Block { stmts: Vec::new(), tail: Some(Box::new(nested)) }
        } else {
            self.parse_block()?
        };
        Ok(Expr::If { cond, then_block, else_block })
    }

    fn parse_or_expr(&mut self, no_struct_lit: bool) -> Result<Expr, ParseError> {
        let mut left = self.parse_and_expr(no_struct_lit)?;
        while self.check(&TokenKind::PipePipe) {
            self.advance();
            let right = self.parse_and_expr(no_struct_lit)?;
            left = Expr::Binary { op: BinaryOp::Or, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_and_expr(&mut self, no_struct_lit: bool) -> Result<Expr, ParseError> {
        let mut left = self.parse_equality_expr(no_struct_lit)?;
        while self.check(&TokenKind::AmpAmp) {
            self.advance();
            let right = self.parse_equality_expr(no_struct_lit)?;
            left = Expr::Binary { op: BinaryOp::And, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_equality_expr(&mut self, no_struct_lit: bool) -> Result<Expr, ParseError> {
        let mut left = self.parse_relational_expr(no_struct_lit)?;
        loop {
            let op = match self.peek() {
                TokenKind::EqEq => BinaryOp::Eq,
                TokenKind::NotEq => BinaryOp::NotEq,
                _ => break,
            };
            self.advance();
            let right = self.parse_relational_expr(no_struct_lit)?;
            left = Expr::Binary { op, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_relational_expr(&mut self, no_struct_lit: bool) -> Result<Expr, ParseError> {
        let mut left = self.parse_additive_expr(no_struct_lit)?;
        loop {
            let op = match self.peek() {
                TokenKind::Lt => BinaryOp::Lt,
                TokenKind::LtEq => BinaryOp::LtEq,
                TokenKind::Gt => BinaryOp::Gt,
                TokenKind::GtEq => BinaryOp::GtEq,
                _ => break,
            };
            self.advance();
            let right = self.parse_additive_expr(no_struct_lit)?;
            left = Expr::Binary { op, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_additive_expr(&mut self, no_struct_lit: bool) -> Result<Expr, ParseError> {
        let mut left = self.parse_multiplicative_expr(no_struct_lit)?;
        loop {
            let op = match self.peek() {
                TokenKind::Plus => BinaryOp::Add,
                TokenKind::Minus => BinaryOp::Sub,
                _ => break,
            };
            self.advance();
            let right = self.parse_multiplicative_expr(no_struct_lit)?;
            left = Expr::Binary { op, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_multiplicative_expr(&mut self, no_struct_lit: bool) -> Result<Expr, ParseError> {
        let mut left = self.parse_unary_expr(no_struct_lit)?;
        loop {
            let op = match self.peek() {
                TokenKind::Star => BinaryOp::Mul,
                TokenKind::Slash => BinaryOp::Div,
                TokenKind::Percent => BinaryOp::Rem,
                _ => break,
            };
            self.advance();
            let right = self.parse_unary_expr(no_struct_lit)?;
            left = Expr::Binary { op, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_unary_expr(&mut self, no_struct_lit: bool) -> Result<Expr, ParseError> {
        let op = match self.peek() {
            TokenKind::Bang => Some(UnaryOp::Not),
            TokenKind::Minus => Some(UnaryOp::Neg),
            _ => None,
        };
        match op {
            Some(op) => {
                self.advance();
                let operand = self.parse_unary_expr(no_struct_lit)?;
                Ok(Expr::Unary { op, operand: Box::new(operand) })
            }
            None => self.parse_postfix_expr(no_struct_lit),
        }
    }

    fn parse_match_expr(&mut self) -> Result<Expr, ParseError> {
        self.eat(&TokenKind::Match)?;
        let scrutinee = Box::new(self.parse_expr_ctx(true)?);
        self.eat(&TokenKind::LBrace)?;
        let mut arms = Vec::new();
        while !self.check(&TokenKind::RBrace) {
            arms.push(self.parse_match_arm()?);
        }
        self.eat(&TokenKind::RBrace)?;
        Ok(Expr::Match { scrutinee, arms })
    }

    fn parse_match_arm(&mut self) -> Result<MatchArm, ParseError> {
        let pattern = self.parse_pattern()?;
        self.eat(&TokenKind::FatArrow)?;
        let body = if self.check(&TokenKind::LBrace) {
            MatchArmBody::Block(self.parse_block()?)
        } else {
            let e = self.parse_expr()?;
            self.eat(&TokenKind::Comma)?; // obligatoria tras un arm-expr (GRAMMAR.md §2.3)
            MatchArmBody::Expr(e)
        };
        Ok(MatchArm { pattern, body })
    }

    fn parse_pattern(&mut self) -> Result<Pattern, ParseError> {
        let name = self.eat_ident()?;
        if self.check(&TokenKind::Dot) {
            self.advance();
            let variant_name = self.eat_ident()?;
            let fields = if self.check(&TokenKind::LBrace) {
                self.advance();
                let mut fs = Vec::new();
                if !self.check(&TokenKind::RBrace) {
                    fs.push(self.parse_field_pattern()?);
                    while self.check(&TokenKind::Comma) {
                        self.advance();
                        if self.check(&TokenKind::RBrace) {
                            break;
                        }
                        fs.push(self.parse_field_pattern()?);
                    }
                }
                self.eat(&TokenKind::RBrace)?;
                Some(fs)
            } else {
                None
            };
            Ok(Pattern::Variant {
                enum_name: name,
                variant_name,
                fields,
            })
        } else {
            Ok(Pattern::Bind(name))
        }
    }

    fn parse_field_pattern(&mut self) -> Result<FieldPattern, ParseError> {
        let name = self.eat_ident()?;
        let pattern = if self.check(&TokenKind::Colon) {
            self.advance();
            self.parse_pattern()?
        } else {
            Pattern::Bind(name.clone()) // shorthand `x` ≡ `x: x`
        };
        Ok(FieldPattern { name, pattern })
    }

    fn parse_postfix_expr(&mut self, no_struct_lit: bool) -> Result<Expr, ParseError> {
        let mut e = self.parse_primary_expr(no_struct_lit)?;
        loop {
            match self.peek().clone() {
                TokenKind::Dot => {
                    self.advance();
                    let field = self.eat_ident()?;
                    e = Expr::FieldAccess {
                        base: Box::new(e),
                        field,
                    };
                }
                TokenKind::LParen => {
                    self.advance();
                    let mut args = Vec::new();
                    if !self.check(&TokenKind::RParen) {
                        args.push(self.parse_expr()?); // dentro de argumentos, struct lit permitido de nuevo
                        while self.check(&TokenKind::Comma) {
                            self.advance();
                            args.push(self.parse_expr()?);
                        }
                    }
                    self.eat(&TokenKind::RParen)?;
                    e = Expr::Call {
                        callee: Box::new(e),
                        args,
                    };
                }
                _ => break,
            }
        }
        Ok(e)
    }

    fn parse_primary_expr(&mut self, no_struct_lit: bool) -> Result<Expr, ParseError> {
        match self.peek().clone() {
            TokenKind::Int(n) => {
                self.advance();
                Ok(Expr::Int(n))
            }
            TokenKind::Float(n) => {
                self.advance();
                Ok(Expr::Float(n))
            }
            TokenKind::Str(s) => {
                self.advance();
                Ok(Expr::Str(s))
            }
            TokenKind::True => {
                self.advance();
                Ok(Expr::Bool(true))
            }
            TokenKind::False => {
                self.advance();
                Ok(Expr::Bool(false))
            }
            TokenKind::Null => {
                self.advance();
                Ok(Expr::Null)
            }
            TokenKind::LParen => {
                self.advance();
                let e = self.parse_expr()?; // dentro de paréntesis, struct lit permitido de nuevo
                self.eat(&TokenKind::RParen)?;
                Ok(Expr::Paren(Box::new(e)))
            }
            TokenKind::Ident(name) => {
                self.advance();
                if !no_struct_lit {
                    // Lookahead de hasta 2 tokens: Nombre['.'Nombre] '{' -> literal.
                    if self.check(&TokenKind::Dot)
                        && matches!(self.peek_at(1), TokenKind::Ident(_))
                        && matches!(self.peek_at(2), TokenKind::LBrace)
                    {
                        self.advance(); // '.'
                        let variant_name = self.eat_ident()?;
                        let fields = self.parse_field_init_list()?;
                        return Ok(Expr::StructLit {
                            name,
                            variant: Some(variant_name),
                            fields,
                        });
                    }
                    if self.check(&TokenKind::LBrace) {
                        let fields = self.parse_field_init_list()?;
                        return Ok(Expr::StructLit {
                            name,
                            variant: None,
                            fields,
                        });
                    }
                }
                Ok(Expr::Ident(name))
            }
            other => Err(self.error(format!("se esperaba una expresión, se encontró {other:?}"))),
        }
    }

    fn parse_field_init_list(&mut self) -> Result<Vec<(String, Expr)>, ParseError> {
        self.eat(&TokenKind::LBrace)?;
        let mut fields = Vec::new();
        if !self.check(&TokenKind::RBrace) {
            fields.push(self.parse_field_init()?);
            while self.check(&TokenKind::Comma) {
                self.advance();
                if self.check(&TokenKind::RBrace) {
                    break;
                }
                fields.push(self.parse_field_init()?);
            }
        }
        self.eat(&TokenKind::RBrace)?;
        Ok(fields)
    }

    fn parse_field_init(&mut self) -> Result<(String, Expr), ParseError> {
        let name = self.eat_ident()?;
        self.eat(&TokenKind::Colon)?;
        let value = self.parse_expr()?;
        Ok((name, value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;

    fn parse_source(src: &str) -> Program {
        let tokens = tokenize(src).unwrap_or_else(|e| panic!("{e}"));
        parse(tokens).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn parses_simple_type_decl_without_semicolon() {
        let prog = parse_source("type User = { id: Int, name: String }");
        assert_eq!(prog.items.len(), 1);
        match &prog.items[0] {
            Item::Type(TypeDecl { name, ty, .. }) => {
                assert_eq!(name, "User");
                match ty {
                    TypeExpr::Struct(fields) => assert_eq!(fields.len(), 2),
                    other => panic!("se esperaba Struct, fue {other:?}"),
                }
            }
            other => panic!("se esperaba Item::Type, fue {other:?}"),
        }
    }

    #[test]
    fn postfix_order_changes_the_type() {
        // T[]? = Optional(List(T))
        let prog = parse_source("type A = User[]?;");
        let Item::Type(TypeDecl { ty, .. }) = &prog.items[0] else { panic!() };
        assert_eq!(
            *ty,
            TypeExpr::Optional(Box::new(TypeExpr::List(Box::new(TypeExpr::Named(
                "User".into(),
                vec![]
            )))))
        );

        // T?[] = List(Optional(T))
        let prog2 = parse_source("type B = User?[];");
        let Item::Type(TypeDecl { ty, .. }) = &prog2.items[0] else { panic!() };
        assert_eq!(
            *ty,
            TypeExpr::List(Box::new(TypeExpr::Optional(Box::new(TypeExpr::Named(
                "User".into(),
                vec![]
            )))))
        );
    }

    #[test]
    fn paren_grouping_vs_tuple_vs_function_type() {
        let prog = parse_source(
            "type A = (Int); type B = (Int, String); type C = (Int, String) -> Bool;",
        );
        let Item::Type(TypeDecl { ty: a, .. }) = &prog.items[0] else { panic!() };
        assert_eq!(*a, TypeExpr::Named("Int".into(), vec![])); // agrupación pura

        let Item::Type(TypeDecl { ty: b, .. }) = &prog.items[1] else { panic!() };
        assert_eq!(
            *b,
            TypeExpr::Tuple(vec![
                TypeExpr::Named("Int".into(), vec![]),
                TypeExpr::Named("String".into(), vec![])
            ])
        );

        let Item::Type(TypeDecl { ty: c, .. }) = &prog.items[2] else { panic!() };
        assert_eq!(
            *c,
            TypeExpr::Function(
                vec![
                    TypeExpr::Named("Int".into(), vec![]),
                    TypeExpr::Named("String".into(), vec![])
                ],
                Box::new(TypeExpr::Named("Bool".into(), vec![]))
            )
        );
    }

    #[test]
    fn generic_type_args_parse() {
        let prog = parse_source("type R = Result<User, ValidationError>;");
        let Item::Type(TypeDecl { ty, .. }) = &prog.items[0] else { panic!() };
        assert_eq!(
            *ty,
            TypeExpr::Named(
                "Result".into(),
                vec![
                    TypeExpr::Named("User".into(), vec![]),
                    TypeExpr::Named("ValidationError".into(), vec![])
                ]
            )
        );
    }

    #[test]
    fn field_access_chain_and_call() {
        let prog = parse_source("service S { rpc f() -> Int { db.users.all().take(1) } }");
        let Item::Service(ServiceDecl { members, .. }) = &prog.items[0] else { panic!() };
        let Member::Rpc(RpcDecl { body, .. }) = &members[0] else { panic!() };
        // db.users.all().take(1) — debe ser Call(FieldAccess(Call(FieldAccess(FieldAccess(db,users),all)),take),[1])
        let tail = body.tail.as_ref().expect("se esperaba tail expr");
        match &**tail {
            Expr::Call { callee, args } => {
                assert_eq!(args.len(), 1);
                match &**callee {
                    Expr::FieldAccess { field, .. } => assert_eq!(field, "take"),
                    other => panic!("se esperaba FieldAccess, fue {other:?}"),
                }
            }
            other => panic!("se esperaba Call, fue {other:?}"),
        }
    }

    #[test]
    fn struct_and_variant_literals() {
        let prog = parse_source(
            r#"service S {
                rpc f() -> Int {
                    match g() {
                        Result.Ok { value: v } => v,
                        _ => 0,
                    }
                }
            }"#,
        );
        let Item::Service(ServiceDecl { members, .. }) = &prog.items[0] else { panic!() };
        let Member::Rpc(RpcDecl { body, .. }) = &members[0] else { panic!() };
        let tail = body.tail.as_ref().unwrap();
        match &**tail {
            Expr::Match { arms, .. } => {
                assert_eq!(arms.len(), 2);
                match &arms[0].pattern {
                    Pattern::Variant {
                        enum_name,
                        variant_name,
                        fields,
                    } => {
                        assert_eq!(enum_name, "Result");
                        assert_eq!(variant_name, "Ok");
                        assert!(fields.is_some());
                    }
                    other => panic!("se esperaba Pattern::Variant, fue {other:?}"),
                }
                assert_eq!(arms[1].pattern, Pattern::Bind("_".into()));
            }
            other => panic!("se esperaba Match, fue {other:?}"),
        }
    }

    #[test]
    fn match_scrutinee_does_not_swallow_arm_brace() {
        // "match x { ... }": x es un Ident suelto (no struct lit) — el '{' es
        // de los arms, no de un literal x{...}. Ver no_struct_lit en el parser.
        let prog = parse_source(
            r#"service S {
                rpc f(x: Int) -> Int {
                    match x {
                        _ => 0,
                    }
                }
            }"#,
        );
        let Item::Service(ServiceDecl { members, .. }) = &prog.items[0] else { panic!() };
        let Member::Rpc(RpcDecl { body, .. }) = &members[0] else { panic!() };
        match body.tail.as_deref().unwrap() {
            Expr::Match { scrutinee, arms } => {
                assert_eq!(**scrutinee, Expr::Ident("x".into()));
                assert_eq!(arms.len(), 1);
            }
            other => panic!("se esperaba Match, fue {other:?}"),
        }
    }

    #[test]
    fn operator_precedence_matches_grammar_3_7() {
        // a + b * c  ==  a + (b * c), no (a + b) * c
        let prog = parse_source("fn f() -> Int { a + b * c }");
        let Item::Fn(FnDecl { body, .. }) = &prog.items[0] else { panic!() };
        let tail = body.tail.as_deref().unwrap();
        match tail {
            Expr::Binary { op: BinaryOp::Add, left, right } => {
                assert_eq!(**left, Expr::Ident("a".into()));
                match &**right {
                    Expr::Binary { op: BinaryOp::Mul, .. } => {}
                    other => panic!("el lado derecho de '+' debería ser 'b * c', fue {other:?}"),
                }
            }
            other => panic!("se esperaba Add en la raíz, fue {other:?}"),
        }
    }

    #[test]
    fn comparison_binds_looser_than_arithmetic_but_tighter_than_logical() {
        // a + 1 < b && c  ==  ((a + 1) < b) && c
        let prog = parse_source("fn f() -> Bool { a + 1 < b && c }");
        let Item::Fn(FnDecl { body, .. }) = &prog.items[0] else { panic!() };
        match body.tail.as_deref().unwrap() {
            Expr::Binary { op: BinaryOp::And, left, .. } => match &**left {
                Expr::Binary { op: BinaryOp::Lt, left, .. } => {
                    assert!(matches!(**left, Expr::Binary { op: BinaryOp::Add, .. }));
                }
                other => panic!("se esperaba Lt del lado izquierdo del &&, fue {other:?}"),
            },
            other => panic!("se esperaba And en la raíz, fue {other:?}"),
        }
    }

    #[test]
    fn unary_minus_and_not_parse_right_associatively() {
        let prog = parse_source("fn f() -> Int { --a }"); // -(-a)
        let Item::Fn(FnDecl { body, .. }) = &prog.items[0] else { panic!() };
        match body.tail.as_deref().unwrap() {
            Expr::Unary { op: UnaryOp::Neg, operand } => {
                assert!(matches!(**operand, Expr::Unary { op: UnaryOp::Neg, .. }));
            }
            other => panic!("se esperaba Neg(Neg(a)), fue {other:?}"),
        }

        let prog2 = parse_source("fn f() -> Bool { !ok }");
        let Item::Fn(FnDecl { body, .. }) = &prog2.items[0] else { panic!() };
        assert!(matches!(
            body.tail.as_deref().unwrap(),
            Expr::Unary { op: UnaryOp::Not, .. }
        ));
    }

    #[test]
    fn if_else_requires_else_and_parses_both_blocks() {
        let prog = parse_source("fn f(x: Int) -> Int { if x > 0 { x } else { 0 } }");
        let Item::Fn(FnDecl { body, .. }) = &prog.items[0] else { panic!() };
        match body.tail.as_deref().unwrap() {
            Expr::If { cond, then_block, else_block } => {
                assert!(matches!(**cond, Expr::Binary { op: BinaryOp::Gt, .. }));
                assert_eq!(then_block.tail.as_deref(), Some(&Expr::Ident("x".into())));
                assert_eq!(else_block.tail.as_deref(), Some(&Expr::Int(0)));
            }
            other => panic!("se esperaba If, fue {other:?}"),
        }
    }

    #[test]
    fn if_without_else_is_a_parse_error() {
        // GRAMMAR.md §3.7: if siempre exige else -- es una expresión total.
        let tokens = tokenize("fn f() -> Int { if true { 1 } }").unwrap();
        assert!(parse(tokens).is_err());
    }

    #[test]
    fn else_if_chains_via_a_nested_block() {
        let src = "fn f(x: Int) -> Int { if x > 0 { 1 } else if x < 0 { -1 } else { 0 } }";
        let prog = parse_source(src);
        let Item::Fn(FnDecl { body, .. }) = &prog.items[0] else { panic!() };
        match body.tail.as_deref().unwrap() {
            Expr::If { else_block, .. } => {
                // else_block.tail debe ser el If anidado (else if), no un valor simple
                assert!(matches!(else_block.tail.as_deref(), Some(Expr::If { .. })));
            }
            other => panic!("se esperaba If, fue {other:?}"),
        }
    }

    #[test]
    fn match_scrutinee_still_restricts_struct_lit_through_operators() {
        // El no_struct_lit de un match debe propagarse a través de la cadena
        // de precedencia completa, no solo el primer token del escrutinio.
        let prog = parse_source(
            r#"service S {
                rpc f(x: Int) -> Int {
                    match x + 1 {
                        _ => 0,
                    }
                }
            }"#,
        );
        let Item::Service(ServiceDecl { members, .. }) = &prog.items[0] else { panic!() };
        let Member::Rpc(RpcDecl { body, .. }) = &members[0] else { panic!() };
        match body.tail.as_deref().unwrap() {
            Expr::Match { scrutinee, .. } => {
                assert!(matches!(**scrutinee, Expr::Binary { op: BinaryOp::Add, .. }));
            }
            other => panic!("se esperaba Match, fue {other:?}"),
        }
    }

    #[test]
    fn if_else_as_statement_does_not_need_a_semicolon() {
        // Bug real encontrado al implementar asignación: sin este caso, el
        // parser trataba el if/else como el tail del bloque en cuanto veía
        // que no había ';', y fallaba con cualquier código real después.
        let prog = parse_source(
            "fn f(n: Int) -> Int { if n > 0 { r = 1; } else { r = -1; } r }",
        );
        let Item::Fn(FnDecl { body, .. }) = &prog.items[0] else { panic!() };
        assert_eq!(body.stmts.len(), 1); // el if/else es la única sentencia; `r` es el tail
        assert!(matches!(body.stmts[0], Stmt::Expr(Expr::If { .. })));
        assert_eq!(body.tail.as_deref(), Some(&Expr::Ident("r".into())));
    }

    #[test]
    fn assignment_statement_parses() {
        let prog = parse_source("fn f() -> Int { let mut x = 1; x = 2; x }");
        let Item::Fn(FnDecl { body, .. }) = &prog.items[0] else { panic!() };
        assert_eq!(body.stmts.len(), 2);
        match &body.stmts[1] {
            Stmt::Assign { name, value } => {
                assert_eq!(name, "x");
                assert_eq!(*value, Expr::Int(2));
            }
            other => panic!("se esperaba Stmt::Assign, fue {other:?}"),
        }
    }

    #[test]
    fn full_users_demo_file_parses() {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/users.link"),
        )
        .expect("no se pudo leer examples/users.link");
        let prog = parse_source(&src);
        // 2 type + 3 enum + 1 fn + 1 service = 7 ítems de nivel superior
        assert_eq!(prog.items.len(), 7);
        let service = prog
            .items
            .iter()
            .find_map(|i| match i {
                Item::Service(s) => Some(s),
                _ => None,
            })
            .expect("se esperaba un service");
        assert_eq!(service.name, "Users");
        assert_eq!(service.members.len(), 4);
    }
}
