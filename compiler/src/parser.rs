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
        write!(f, "error de sintaxis en línea {}, columna {}: {}", self.span.line, self.span.col, self.message)
    }
}

/// Recuperación de errores (GRAMMAR.md/README: prerrequisito 2/3 para un
/// LSP): antes, el primer error de sintaxis abortaba TODO el parseo -- acá
/// se intenta seguir después de cada error, acumulando todos los que
/// encuentre en una sola pasada, en vez de que el usuario los vea de a uno
/// por vez. Nunca devuelve un `Program` parcial: o parsea TODO sin
/// errores, o devuelve la lista completa (espejo exacto del
/// `Result<(), Vec<CheckError>>` que ya usa `Checker::check_program`).
pub fn parse(tokens: Vec<Token>) -> Result<Program, Vec<ParseError>> {
    let mut parser = Parser { tokens, pos: 0, errors: Vec::new() };
    let program = parser.parse_program();
    if parser.errors.is_empty() {
        Ok(program)
    } else {
        Err(parser.errors)
    }
}

/// Cota simple (no deduplicación heurística de errores en cascada) para
/// acotar el caso patológico -- ver `parse_program`.
const MAX_ERRORS: usize = 100;

/// `start`/`end` de dos spans ya ordenados (izquierda a derecha) en uno solo
/// que los cubre a ambos -- `line`/`col` siempre del lado IZQUIERDO (`Span`
/// describe solo su inicio, ver token.rs), así que un span de varios tokens
/// se arma acumulando desde el primero.
fn merge(start: Span, end: Span) -> Span {
    Span::new(start.start, end.end, start.line, start.col)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    errors: Vec<ParseError>,
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

    /// Span del ÚLTIMO token consumido -- para cerrar el span de un nodo
    /// justo después de un `eat`/`advance` de cierre (`}`/`)`/`]`/etc.),
    /// sin tener que guardar ese token en una variable aparte.
    fn prev_span(&self) -> Span {
        debug_assert!(self.pos > 0, "prev_span() antes de consumir ningún token");
        self.tokens[self.pos - 1].span
    }

    fn advance(&mut self) -> Token {
        debug_assert!(!self.check(&TokenKind::Eof), "advance() en Eof");
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

    fn parse_program(&mut self) -> Program {
        let mut items = Vec::new();
        while !self.check(&TokenKind::Eof) {
            if self.errors.len() >= MAX_ERRORS {
                break;
            }
            match self.parse_item() {
                Ok(item) => items.push(item),
                Err(e) => {
                    self.errors.push(e);
                    self.synchronize();
                }
            }
        }
        Program { items }
    }

    /// EXACTAMENTE la misma condición que `parse_item` usa para despachar --
    /// única fuente de verdad, co-ubicada físicamente al lado para que
    /// nunca diverjan (la clase de bug "dos lugares que tienen que
    /// coincidir y no coinciden" ya pasó varias veces en este proyecto).
    fn at_item_start(&self) -> bool {
        matches!(
            self.peek(),
            TokenKind::Import | TokenKind::Type | TokenKind::Enum | TokenKind::Service | TokenKind::Const | TokenKind::Fn
        ) || matches!(self.peek(), TokenKind::Ident(name) if name == "db" && *self.peek_at(1) == TokenKind::LBrace)
    }

    /// Modo pánico, granularidad de ÍTEM DE NIVEL SUPERIOR únicamente
    /// (alcance v0 deliberado -- no por miembro de `service` ni por
    /// sentencia dentro de un bloque; bajar la granularidad es un fast-
    /// follow futuro, no esta ronda). Salta tokens hasta encontrar algo que
    /// parezca el inicio de un ítem nuevo, o EOF.
    ///
    /// A propósito NO avanza incondicionalmente antes de chequear: los 6
    /// keywords de `at_item_start` son reservados y solo aparecen ahí en
    /// toda la gramática de hoy, así que una falla de 0 tokens consumidos
    /// de `parse_item` nunca puede coincidir con `at_item_start()==true` --
    /// avanzar de más ahí SE COME el primer token del próximo ítem real
    /// cada vez que el error ocurre anidado (el caso más común: una llave
    /// sin cerrar dentro de un `service` dejaría el error justo en el token
    /// que en realidad es el inicio del siguiente `fn`/`type`/etc., y
    /// avanzar de más lo descartaría en silencio sin reportar su propio
    /// error). Bug real encontrado por review antes de implementar esto.
    fn synchronize(&mut self) {
        while !self.check(&TokenKind::Eof) && !self.at_item_start() {
            self.advance();
        }
    }

    fn parse_item(&mut self) -> Result<Item, ParseError> {
        match self.peek().clone() {
            TokenKind::Import => Ok(Item::Import(self.parse_import_decl()?)),
            TokenKind::Type => Ok(Item::Type(self.parse_type_decl()?)),
            TokenKind::Enum => Ok(Item::Enum(self.parse_enum_decl()?)),
            TokenKind::Service => Ok(Item::Service(self.parse_service_decl()?)),
            TokenKind::Const => Ok(Item::Const(self.parse_const_decl()?)),
            TokenKind::Fn => Ok(Item::Fn(self.parse_fn_decl()?)),
            // `db` NO es palabra reservada (ast.rs, doc de DbDecl) -- se
            // reconoce por texto solo acá, en posición de ítem de nivel
            // superior, seguido de `{`. En cualquier otro contexto (una
            // expresión, un patrón, un nombre de campo) "db" sigue siendo
            // un identificador común y corriente. (Límite conocido: como
            // "db" no es reservado, un fragmento de basura que por
            // casualidad contenga `db {` durante `synchronize` puede parar
            // ahí y hacer que esto se intente sobre basura -- autocorrectivo,
            // `parse_db_decl` fallaría con su propio error si el contenido
            // no tiene forma de campo:tipo, pero puede sumar un error de
            // ruido en ese caso específico. Los otros 6 keywords no tienen
            // este problema: son reservados, nunca aparecen en otra posición.)
            TokenKind::Ident(name) if name == "db" && *self.peek_at(1) == TokenKind::LBrace => {
                Ok(Item::Db(self.parse_db_decl()?))
            }
            other => Err(self.error(format!(
                "se esperaba un ítem de nivel superior (import/type/enum/service/const/fn/db), se encontró {other:?}"
            ))),
        }
    }

    fn parse_db_decl(&mut self) -> Result<DbDecl, ParseError> {
        let start = self.span();
        self.advance(); // "db"
        self.eat(&TokenKind::LBrace)?;
        let collections = self.parse_field_list()?;
        self.eat(&TokenKind::RBrace)?;
        let span = merge(start, self.prev_span());
        Ok(DbDecl { collections, span })
    }

    fn parse_import_decl(&mut self) -> Result<ImportDecl, ParseError> {
        let start = self.span();
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
        let span = merge(start, self.prev_span());
        self.eat(&TokenKind::Semi)?;
        Ok(ImportDecl { names, from, span })
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
        let start = self.span();
        self.eat(&TokenKind::Type)?;
        let name = self.eat_ident()?;
        let type_params = self.parse_type_params()?;
        self.eat(&TokenKind::Equals)?;
        let ty = self.parse_type_expr()?;
        let span = merge(start, self.prev_span());
        if self.check(&TokenKind::Semi) {
            self.advance();
        }
        Ok(TypeDecl { name, type_params, ty, span })
    }

    fn parse_enum_decl(&mut self) -> Result<EnumDecl, ParseError> {
        let start = self.span();
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
        let span = merge(start, self.prev_span());
        Ok(EnumDecl {
            name,
            type_params,
            variants,
            span,
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
        let start = self.span();
        self.eat(&TokenKind::Const)?;
        let name = self.eat_ident()?;
        self.eat(&TokenKind::Colon)?;
        let ty = self.parse_type_expr()?;
        self.eat(&TokenKind::Equals)?;
        let value = self.parse_expr()?;
        let span = merge(start, value.span);
        self.eat(&TokenKind::Semi)?;
        Ok(ConstDecl { name, ty, value, span })
    }

    fn parse_service_decl(&mut self) -> Result<ServiceDecl, ParseError> {
        let start = self.span();
        self.eat(&TokenKind::Service)?;
        let name = self.eat_ident()?;
        self.eat(&TokenKind::LBrace)?;
        let mut members = Vec::new();
        while !self.check(&TokenKind::RBrace) {
            members.push(self.parse_member()?);
        }
        self.eat(&TokenKind::RBrace)?;
        let span = merge(start, self.prev_span());
        Ok(ServiceDecl { name, members, span })
    }

    fn parse_member(&mut self) -> Result<Member, ParseError> {
        // Capturado ANTES de la anotación opcional -- si `parse_rpc_like`
        // tomara su propio inicio al entrar, `@authenticated`/`@requires`
        // quedaría sistemáticamente AFUERA del span del rpc.
        let start = self.span();
        let annotation = self.parse_optional_annotation()?;
        match self.peek().clone() {
            TokenKind::Rpc => Ok(Member::Rpc(self.parse_rpc_like(start, TokenKind::Rpc, annotation)?)),
            TokenKind::Stream => Ok(Member::Stream(self.parse_rpc_like(start, TokenKind::Stream, annotation)?)),
            other => Err(self.error(format!("se esperaba 'rpc' o 'stream', se encontró {other:?}"))),
        }
    }

    /// Auth v0 (GRAMMAR.md §3.14): `@authenticated` o `@requires(Enum.Variante)`
    /// antes de `rpc`/`stream`. A propósito NO reusa `parse_pattern_atom`
    /// completo -- ese acepta opcionalmente `{ campos }` (destructuración),
    /// algo que acá nunca corresponde: `@requires` solo compara el tag de la
    /// variante, nunca mira campos.
    fn parse_optional_annotation(&mut self) -> Result<Option<Annotation>, ParseError> {
        if !self.check(&TokenKind::At) {
            return Ok(None);
        }
        self.advance();
        let name = self.eat_ident()?;
        match name.as_str() {
            "authenticated" => Ok(Some(Annotation::Authenticated)),
            "requires" => {
                self.eat(&TokenKind::LParen)?;
                let enum_name = self.eat_ident()?;
                self.eat(&TokenKind::Dot)?;
                let variant_name = self.eat_ident()?;
                self.eat(&TokenKind::RParen)?;
                Ok(Some(Annotation::Requires { enum_name, variant_name }))
            }
            other => Err(self.error(format!(
                "anotación desconocida '@{other}' (se esperaba '@authenticated' o '@requires(Enum.Variante)')"
            ))),
        }
    }

    /// `start` viene de `parse_member` (capturado antes de la anotación
    /// opcional, ver ahí). El span de la declaración cubre la FIRMA hasta el
    /// return type -- se calcula ANTES de parsear `body`, a propósito (el
    /// cuerpo tiene sus propios spans precisos, ver ast.rs::RpcDecl).
    fn parse_rpc_like(&mut self, start: Span, kw: TokenKind, annotation: Option<Annotation>) -> Result<RpcDecl, ParseError> {
        self.eat(&kw)?;
        let name = self.eat_ident()?;
        self.eat(&TokenKind::LParen)?;
        let params = self.parse_param_list()?;
        self.eat(&TokenKind::RParen)?;
        self.eat(&TokenKind::Arrow)?;
        let return_type = self.parse_type_expr()?;
        let span = merge(start, self.prev_span());
        let body = self.parse_block()?;
        Ok(RpcDecl {
            name,
            params,
            return_type,
            body,
            annotation,
            span,
        })
    }

    fn parse_fn_decl(&mut self) -> Result<FnDecl, ParseError> {
        let start = self.span();
        self.eat(&TokenKind::Fn)?;
        let name = self.eat_ident()?;
        self.eat(&TokenKind::LParen)?;
        let params = self.parse_param_list()?;
        self.eat(&TokenKind::RParen)?;
        self.eat(&TokenKind::Arrow)?;
        let return_type = self.parse_type_expr()?;
        // Mismo criterio que parse_rpc_like: span de firma, calculado ANTES
        // del cuerpo.
        let span = merge(start, self.prev_span());
        let body = self.parse_block()?;
        Ok(FnDecl {
            name,
            params,
            return_type,
            body,
            span,
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
        let start = self.span();
        self.eat(&TokenKind::LBrace)?;
        let mut stmts = Vec::new();
        let mut tail = None;
        while !self.check(&TokenKind::RBrace) {
            match self.peek().clone() {
                TokenKind::Let => stmts.push(self.parse_let_stmt()?),
                TokenKind::Return => stmts.push(self.parse_return_stmt()?),
                TokenKind::While => stmts.push(self.parse_while_stmt()?),
                // `identifier =` (y no `==`, ya son tokens distintos) es una
                // asignación -- se detecta con 1 token de lookahead antes de
                // caer al parseo genérico de expresión, igual que la
                // desambiguación de struct_or_variant_lit (GRAMMAR.md §2.2).
                TokenKind::Ident(name) if matches!(self.peek_at(1), TokenKind::Equals) => {
                    let stmt_start = self.span();
                    self.advance();
                    self.advance(); // '='
                    let value = self.parse_expr()?;
                    let span = merge(stmt_start, value.span);
                    self.eat(&TokenKind::Semi)?;
                    stmts.push(Spanned { node: Stmt::Assign { name, value }, span });
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
                    let span = e.span;
                    stmts.push(Spanned { node: Stmt::Expr(e), span });
                }
                _ => {
                    let e = self.parse_expr()?;
                    if self.check(&TokenKind::Semi) {
                        self.advance();
                        let span = e.span;
                        stmts.push(Spanned { node: Stmt::Expr(e), span });
                    } else {
                        tail = Some(Box::new(e));
                        break;
                    }
                }
            }
        }
        self.eat(&TokenKind::RBrace)?;
        let span = merge(start, self.prev_span());
        Ok(Block { stmts, tail, span })
    }

    fn parse_let_stmt(&mut self) -> Result<Spanned<Stmt>, ParseError> {
        let start = self.span();
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
        let span = merge(start, value.span);
        self.eat(&TokenKind::Semi)?;
        Ok(Spanned {
            node: Stmt::Let {
                name,
                mutable,
                ty,
                value,
            },
            span,
        })
    }

    fn parse_return_stmt(&mut self) -> Result<Spanned<Stmt>, ParseError> {
        let start = self.span();
        self.eat(&TokenKind::Return)?;
        let value = if self.check(&TokenKind::Semi) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        // `return;` sin valor: el span es solo el propio token 'return' -- el
        // ';' es terminador, nunca se incluye (regla de terminador).
        let span = match &value {
            Some(v) => merge(start, v.span),
            None => start,
        };
        self.eat(&TokenKind::Semi)?;
        Ok(Spanned { node: Stmt::Return(value), span })
    }

    /// `while cond { body }` (GRAMMAR.md §3.15) -- más simple que
    /// `parse_if_expr`: `while` nunca es "block-like ambiguo con tail"
    /// (siempre va a `stmts`, nunca a `tail`), así que no hace falta
    /// ninguna de las dos ramas extra que `parse_block` tiene para `if`/
    /// `match`.
    fn parse_while_stmt(&mut self) -> Result<Spanned<Stmt>, ParseError> {
        let start = self.span();
        self.eat(&TokenKind::While)?;
        // Misma restricción que la condición de un `if` (parse_if_expr):
        // sin esto, `while x { ... }` sería tan ambiguo con un struct-lit
        // `x { ... }` como `if x { ... }` ya lo es.
        let cond = self.parse_or_expr(true)?;
        let body = self.parse_block()?;
        let span = merge(start, self.prev_span());
        Ok(Spanned { node: Stmt::While { cond, body }, span })
    }

    fn parse_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
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
    fn parse_expr_ctx(&mut self, no_struct_lit: bool) -> Result<Spanned<Expr>, ParseError> {
        if self.check(&TokenKind::Match) {
            self.parse_match_expr()
        } else if self.check(&TokenKind::If) {
            self.parse_if_expr()
        } else {
            self.parse_or_expr(no_struct_lit)
        }
    }

    fn parse_if_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let start = self.span();
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
            // Expr como tail, así que esto no pierde generalidad. El Block
            // sintético no tiene llaves propias -- su span es el del if
            // anidado que envuelve.
            let nested = self.parse_if_expr()?;
            let span = nested.span;
            Block { stmts: Vec::new(), tail: Some(Box::new(nested)), span }
        } else {
            self.parse_block()?
        };
        let span = merge(start, self.prev_span());
        Ok(Spanned { node: Expr::If { cond, then_block, else_block }, span })
    }

    fn parse_or_expr(&mut self, no_struct_lit: bool) -> Result<Spanned<Expr>, ParseError> {
        let mut left = self.parse_and_expr(no_struct_lit)?;
        while self.check(&TokenKind::PipePipe) {
            self.advance();
            let right = self.parse_and_expr(no_struct_lit)?;
            let span = merge(left.span, right.span);
            left = Spanned { node: Expr::Binary { op: BinaryOp::Or, left: Box::new(left), right: Box::new(right) }, span };
        }
        Ok(left)
    }

    fn parse_and_expr(&mut self, no_struct_lit: bool) -> Result<Spanned<Expr>, ParseError> {
        let mut left = self.parse_equality_expr(no_struct_lit)?;
        while self.check(&TokenKind::AmpAmp) {
            self.advance();
            let right = self.parse_equality_expr(no_struct_lit)?;
            let span = merge(left.span, right.span);
            left = Spanned { node: Expr::Binary { op: BinaryOp::And, left: Box::new(left), right: Box::new(right) }, span };
        }
        Ok(left)
    }

    fn parse_equality_expr(&mut self, no_struct_lit: bool) -> Result<Spanned<Expr>, ParseError> {
        let mut left = self.parse_relational_expr(no_struct_lit)?;
        loop {
            let op = match self.peek() {
                TokenKind::EqEq => BinaryOp::Eq,
                TokenKind::NotEq => BinaryOp::NotEq,
                _ => break,
            };
            self.advance();
            let right = self.parse_relational_expr(no_struct_lit)?;
            let span = merge(left.span, right.span);
            left = Spanned { node: Expr::Binary { op, left: Box::new(left), right: Box::new(right) }, span };
        }
        Ok(left)
    }

    fn parse_relational_expr(&mut self, no_struct_lit: bool) -> Result<Spanned<Expr>, ParseError> {
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
            let span = merge(left.span, right.span);
            left = Spanned { node: Expr::Binary { op, left: Box::new(left), right: Box::new(right) }, span };
        }
        Ok(left)
    }

    fn parse_additive_expr(&mut self, no_struct_lit: bool) -> Result<Spanned<Expr>, ParseError> {
        let mut left = self.parse_multiplicative_expr(no_struct_lit)?;
        loop {
            let op = match self.peek() {
                TokenKind::Plus => BinaryOp::Add,
                TokenKind::Minus => BinaryOp::Sub,
                _ => break,
            };
            self.advance();
            let right = self.parse_multiplicative_expr(no_struct_lit)?;
            let span = merge(left.span, right.span);
            left = Spanned { node: Expr::Binary { op, left: Box::new(left), right: Box::new(right) }, span };
        }
        Ok(left)
    }

    fn parse_multiplicative_expr(&mut self, no_struct_lit: bool) -> Result<Spanned<Expr>, ParseError> {
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
            let span = merge(left.span, right.span);
            left = Spanned { node: Expr::Binary { op, left: Box::new(left), right: Box::new(right) }, span };
        }
        Ok(left)
    }

    fn parse_unary_expr(&mut self, no_struct_lit: bool) -> Result<Spanned<Expr>, ParseError> {
        let op = match self.peek() {
            TokenKind::Bang => Some(UnaryOp::Not),
            TokenKind::Minus => Some(UnaryOp::Neg),
            _ => None,
        };
        match op {
            Some(op) => {
                let start = self.span();
                self.advance();
                let operand = self.parse_unary_expr(no_struct_lit)?;
                let span = merge(start, operand.span);
                Ok(Spanned { node: Expr::Unary { op, operand: Box::new(operand) }, span })
            }
            None => self.parse_postfix_expr(no_struct_lit),
        }
    }

    fn parse_match_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let start = self.span();
        self.eat(&TokenKind::Match)?;
        let scrutinee = Box::new(self.parse_expr_ctx(true)?);
        self.eat(&TokenKind::LBrace)?;
        let mut arms = Vec::new();
        while !self.check(&TokenKind::RBrace) {
            arms.push(self.parse_match_arm()?);
        }
        self.eat(&TokenKind::RBrace)?;
        let span = merge(start, self.prev_span());
        Ok(Spanned { node: Expr::Match { scrutinee, arms }, span })
    }

    fn parse_match_arm(&mut self) -> Result<MatchArm, ParseError> {
        let start = self.span();
        let pattern = self.parse_pattern()?;
        let guard = if self.check(&TokenKind::If) {
            self.advance();
            // Mismo no_struct_lit=true que la condición de un 'if' (GRAMMAR.md
            // §2.3) -- un guard, igual que esa condición, siempre termina
            // justo antes de un token que decide el resto del arm (acá, `=>`),
            // así que restringir struct-lits acá no pierde generalidad real.
            Some(self.parse_or_expr(true)?)
        } else {
            None
        };
        self.eat(&TokenKind::FatArrow)?;
        let (body, span) = if self.check(&TokenKind::LBrace) {
            let block = self.parse_block()?;
            let span = merge(start, block.span);
            (MatchArmBody::Block(block), span)
        } else {
            let e = self.parse_expr()?;
            let span = merge(start, e.span);
            // La coma separa un arm-expr del siguiente (GRAMMAR.md §2.3),
            // pero en el ÚLTIMO arm no hay nada que separar: exigirla ahí
            // rechazaba `match x { A => 1, B => 2 }` con un críptico "se
            // esperaba Comma, se encontró RBrace". Es opcional justo antes
            // del `}` de cierre, igual que en Rust. Se consume DESPUÉS de
            // computar el span (regla de terminador): la coma no es parte
            // del arm.
            if !self.check(&TokenKind::RBrace) {
                self.eat(&TokenKind::Comma)?;
            }
            (MatchArmBody::Expr(e), span)
        };
        Ok(MatchArm { pattern, guard, body, span })
    }

    /// `pattern_atom , { "|" , pattern_atom }` (GRAMMAR.md §3.3) -- un solo
    /// átomo se devuelve tal cual, sin envolver en `Or`, para no complicar
    /// el resto del checker/runtime con el caso trivial de un solo patrón.
    fn parse_pattern(&mut self) -> Result<Pattern, ParseError> {
        let first = self.parse_pattern_atom()?;
        if self.check(&TokenKind::Pipe) {
            let mut alts = vec![first];
            while self.check(&TokenKind::Pipe) {
                self.advance();
                alts.push(self.parse_pattern_atom()?);
            }
            Ok(Pattern::Or(alts))
        } else {
            Ok(first)
        }
    }

    fn parse_pattern_atom(&mut self) -> Result<Pattern, ParseError> {
        match self.peek().clone() {
            TokenKind::Int(n) => {
                self.advance();
                Ok(Pattern::Literal(LiteralPattern::Int(n)))
            }
            // `-1` como patrón: no es "unario aplicado a un patrón" (los
            // patrones no son expresiones) -- es un solo literal negativo,
            // igual que en Rust. Por eso se combina acá mismo, no se delega
            // a ninguna regla general de unario.
            TokenKind::Minus => {
                self.advance();
                match self.peek().clone() {
                    TokenKind::Int(n) => {
                        self.advance();
                        Ok(Pattern::Literal(LiteralPattern::Int(-n)))
                    }
                    other => Err(self.error(format!(
                        "se esperaba un entero después de '-' en un patrón, se encontró {other:?}"
                    ))),
                }
            }
            TokenKind::Str(s) => {
                self.advance();
                Ok(Pattern::Literal(LiteralPattern::Str(s)))
            }
            TokenKind::True => {
                self.advance();
                Ok(Pattern::Literal(LiteralPattern::Bool(true)))
            }
            TokenKind::False => {
                self.advance();
                Ok(Pattern::Literal(LiteralPattern::Bool(false)))
            }
            _ => {
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
                } else if self.check(&TokenKind::Colon) {
                    // `nombre: Tipo` -- narrowing de uniones (GRAMMAR.md
                    // §3.9). `parse_postfix_type`, NO `parse_type_expr`:
                    // ese último consume `|` en loop para uniones y se
                    // comería el `|` que en realidad separa esta alternativa
                    // de la siguiente en un or-pattern (`i: Int | s: String`
                    // tiene que quedar como Or([Type(i,Int), Type(s,String)]),
                    // no fusionarse en un solo Type(i, Union([Int,String]))).
                    self.advance();
                    let ty = self.parse_postfix_type()?;
                    Ok(Pattern::Type(name, ty))
                } else {
                    Ok(Pattern::Bind(name))
                }
            }
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

    fn parse_postfix_expr(&mut self, no_struct_lit: bool) -> Result<Spanned<Expr>, ParseError> {
        let mut e = self.parse_primary_expr(no_struct_lit)?;
        loop {
            match self.peek().clone() {
                // `.identifier` es FieldAccess, `.0`/`.1`/... (un entero) es
                // acceso posicional a tupla -- se distinguen con 1 token de
                // lookahead después del punto.
                TokenKind::Dot => {
                    self.advance();
                    match self.peek().clone() {
                        TokenKind::Int(n) => {
                            self.advance();
                            let index: usize = n
                                .try_into()
                                .map_err(|_| self.error("índice de tupla inválido (negativo)"))?;
                            let span = merge(e.span, self.prev_span());
                            e = Spanned { node: Expr::TupleIndex { base: Box::new(e), index }, span };
                        }
                        _ => {
                            let field = self.eat_ident()?;
                            let span = merge(e.span, self.prev_span());
                            e = Spanned { node: Expr::FieldAccess { base: Box::new(e), field }, span };
                        }
                    }
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
                    let span = merge(e.span, self.prev_span());
                    e = Spanned { node: Expr::Call { callee: Box::new(e), args }, span };
                }
                TokenKind::LBracket => {
                    self.advance();
                    let index = self.parse_expr()?; // dentro de [], struct lit permitido de nuevo
                    self.eat(&TokenKind::RBracket)?;
                    let span = merge(e.span, self.prev_span());
                    e = Spanned { node: Expr::Index { base: Box::new(e), index: Box::new(index) }, span };
                }
                _ => break,
            }
        }
        Ok(e)
    }

    fn parse_primary_expr(&mut self, no_struct_lit: bool) -> Result<Spanned<Expr>, ParseError> {
        match self.peek().clone() {
            TokenKind::Int(n) => {
                let t = self.advance();
                Ok(Spanned { node: Expr::Int(n), span: t.span })
            }
            TokenKind::Float(n) => {
                let t = self.advance();
                Ok(Spanned { node: Expr::Float(n), span: t.span })
            }
            TokenKind::Str(s) => {
                let t = self.advance();
                Ok(Spanned { node: Expr::Str(s), span: t.span })
            }
            TokenKind::True => {
                let t = self.advance();
                Ok(Spanned { node: Expr::Bool(true), span: t.span })
            }
            TokenKind::False => {
                let t = self.advance();
                Ok(Spanned { node: Expr::Bool(false), span: t.span })
            }
            TokenKind::Null => {
                let t = self.advance();
                Ok(Spanned { node: Expr::Null, span: t.span })
            }
            TokenKind::LBracket => {
                let start = self.span();
                self.advance();
                let mut items = Vec::new();
                if !self.check(&TokenKind::RBracket) {
                    items.push(self.parse_expr()?);
                    while self.check(&TokenKind::Comma) {
                        self.advance();
                        if self.check(&TokenKind::RBracket) {
                            break; // coma final: [1, 2, 3,]
                        }
                        items.push(self.parse_expr()?);
                    }
                }
                self.eat(&TokenKind::RBracket)?;
                let span = merge(start, self.prev_span());
                Ok(Spanned { node: Expr::ArrayLit(items), span })
            }
            TokenKind::LParen => {
                let start = self.span();
                self.advance();
                // Misma ambigüedad que a nivel de tipos (§2.2): (a) es
                // agrupación, (a,) es tupla de 1, (a,b) es tupla de 2+.
                let mut items = vec![self.parse_expr()?]; // dentro de (), struct lit permitido de nuevo
                let mut had_comma = false;
                while self.check(&TokenKind::Comma) {
                    self.advance();
                    had_comma = true;
                    if self.check(&TokenKind::RParen) {
                        break; // coma final: (a,)
                    }
                    items.push(self.parse_expr()?);
                }
                self.eat(&TokenKind::RParen)?;
                let span = merge(start, self.prev_span());
                if items.len() == 1 && !had_comma {
                    Ok(Spanned { node: Expr::Paren(Box::new(items.into_iter().next().unwrap())), span })
                } else {
                    Ok(Spanned { node: Expr::TupleLit(items), span })
                }
            }
            TokenKind::Ident(name) => {
                let start = self.span();
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
                        let span = merge(start, self.prev_span());
                        return Ok(Spanned { node: Expr::StructLit { name, variant: Some(variant_name), fields }, span });
                    }
                    if self.check(&TokenKind::LBrace) {
                        let fields = self.parse_field_init_list()?;
                        let span = merge(start, self.prev_span());
                        return Ok(Spanned { node: Expr::StructLit { name, variant: None, fields }, span });
                    }
                }
                Ok(Spanned { node: Expr::Ident(name), span: start })
            }
            TokenKind::Pipe => self.parse_closure_expr(),
            other => Err(self.error(format!("se esperaba una expresión, se encontró {other:?}"))),
        }
    }

    /// `|params| { block }` (GRAMMAR.md §3.10). `||` lexea como un solo
    /// token `PipePipe` (token.rs), así que nunca llega hasta acá -- pero
    /// `| |` (con espacio) sigue siendo dos `Pipe` separados, de ahí el
    /// chequeo explícito de "al menos 1 param" más abajo en vez de confiar
    /// en que el token ya lo descarta.
    fn parse_closure_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let start = self.span();
        self.eat(&TokenKind::Pipe)?;
        let mut params = Vec::new();
        if !self.check(&TokenKind::Pipe) {
            params.push(self.parse_closure_param()?);
            while self.check(&TokenKind::Comma) {
                self.advance();
                if self.check(&TokenKind::Pipe) {
                    break; // coma final: |x, y,|
                }
                params.push(self.parse_closure_param()?);
            }
        }
        self.eat(&TokenKind::Pipe)?;
        if params.is_empty() {
            return Err(self.error(
                "un closure necesita al menos 1 parámetro -- `||` (0 parámetros) no se soporta todavía (GRAMMAR.md §3.10)",
            ));
        }
        let body = self.parse_block()?;
        let span = merge(start, self.prev_span());
        Ok(Spanned { node: Expr::Closure { params, body }, span })
    }

    /// `nombre (":" tipo)?` -- a diferencia de `parse_param` (fn/rpc), la
    /// anotación es OPCIONAL (se infiere en modo chequeo desde el
    /// `Type::Function` esperado, ver checker.rs). El tipo se parsea con
    /// `parse_postfix_type`, NO `parse_type_expr`: ese último consume `|`
    /// en loop para uniones, y se comería el `|` de CIERRE del closure
    /// (`|x: Int | String| {...}` se malinterpretaría). Un tipo unión en
    /// un param de closure necesita paréntesis: `|x: (Int | String)| {...}`.
    fn parse_closure_param(&mut self) -> Result<ClosureParam, ParseError> {
        let name = self.eat_ident()?;
        let ty = if self.check(&TokenKind::Colon) {
            self.advance();
            Some(self.parse_postfix_type()?)
        } else {
            None
        };
        Ok(ClosureParam { name, ty })
    }

    fn parse_field_init_list(&mut self) -> Result<Vec<(String, Spanned<Expr>)>, ParseError> {
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

    fn parse_field_init(&mut self) -> Result<(String, Spanned<Expr>), ParseError> {
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
        parse(tokens).unwrap_or_else(|e| panic!("{e:?}"))
    }

    /// Envuelve un nodo en `Spanned` con un span dummy -- para construir un
    /// `Expr`/`Stmt` literal del lado derecho de un `assert_eq!` en los
    /// tests de acá abajo. `Spanned::eq` ignora el span (ast.rs), así que el
    /// valor exacto acá no importa, solo hace falta que el tipo cierre.
    fn sp<T>(node: T) -> Spanned<T> {
        Spanned { node, span: Span::new(0, 0, 0, 0) }
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
        match &tail.node {
            Expr::Call { callee, args } => {
                assert_eq!(args.len(), 1);
                match &callee.node {
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
        match &tail.node {
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
    fn literal_and_or_patterns_parse() {
        let prog = parse_source(
            r#"fn describe(n: Int) -> String {
                match n {
                    1 | 2 => "bajo",
                    -1 => "negativo",
                    _ => "otro",
                }
            }"#,
        );
        let Item::Fn(FnDecl { body, .. }) = &prog.items[0] else { panic!() };
        let tail = body.tail.as_deref().unwrap();
        match &tail.node {
            Expr::Match { arms, .. } => {
                assert_eq!(arms.len(), 3);
                assert_eq!(
                    arms[0].pattern,
                    Pattern::Or(vec![
                        Pattern::Literal(LiteralPattern::Int(1)),
                        Pattern::Literal(LiteralPattern::Int(2)),
                    ])
                );
                assert_eq!(arms[1].pattern, Pattern::Literal(LiteralPattern::Int(-1)));
                assert_eq!(arms[2].pattern, Pattern::Bind("_".into()));
            }
            other => panic!("se esperaba Match, fue {other:?}"),
        }
    }

    #[test]
    fn match_arm_guard_parses_between_pattern_and_arrow() {
        let prog = parse_source(
            r#"fn describe(n: Int) -> String {
                match n {
                    x if x > 0 => "positivo",
                    _ => "no positivo",
                }
            }"#,
        );
        let Item::Fn(FnDecl { body, .. }) = &prog.items[0] else { panic!() };
        let tail = body.tail.as_deref().unwrap();
        match &tail.node {
            Expr::Match { arms, .. } => {
                assert_eq!(arms[0].pattern, Pattern::Bind("x".into()));
                assert!(arms[0].guard.is_some());
                assert!(arms[1].guard.is_none());
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
        match &body.tail.as_deref().unwrap().node {
            Expr::Match { scrutinee, arms } => {
                assert_eq!(scrutinee.node, Expr::Ident("x".into()));
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
        match &tail.node {
            Expr::Binary { op: BinaryOp::Add, left, right } => {
                assert_eq!(left.node, Expr::Ident("a".into()));
                match &right.node {
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
        match &body.tail.as_deref().unwrap().node {
            Expr::Binary { op: BinaryOp::And, left, .. } => match &left.node {
                Expr::Binary { op: BinaryOp::Lt, left, .. } => {
                    assert!(matches!(left.node, Expr::Binary { op: BinaryOp::Add, .. }));
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
        match &body.tail.as_deref().unwrap().node {
            Expr::Unary { op: UnaryOp::Neg, operand } => {
                assert!(matches!(operand.node, Expr::Unary { op: UnaryOp::Neg, .. }));
            }
            other => panic!("se esperaba Neg(Neg(a)), fue {other:?}"),
        }

        let prog2 = parse_source("fn f() -> Bool { !ok }");
        let Item::Fn(FnDecl { body, .. }) = &prog2.items[0] else { panic!() };
        assert!(matches!(
            body.tail.as_deref().unwrap().node,
            Expr::Unary { op: UnaryOp::Not, .. }
        ));
    }

    #[test]
    fn if_else_requires_else_and_parses_both_blocks() {
        let prog = parse_source("fn f(x: Int) -> Int { if x > 0 { x } else { 0 } }");
        let Item::Fn(FnDecl { body, .. }) = &prog.items[0] else { panic!() };
        match &body.tail.as_deref().unwrap().node {
            Expr::If { cond, then_block, else_block } => {
                assert!(matches!(cond.node, Expr::Binary { op: BinaryOp::Gt, .. }));
                assert_eq!(then_block.tail.as_deref().map(|s| &s.node), Some(&Expr::Ident("x".into())));
                assert_eq!(else_block.tail.as_deref().map(|s| &s.node), Some(&Expr::Int(0)));
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
        match &body.tail.as_deref().unwrap().node {
            Expr::If { else_block, .. } => {
                // else_block.tail debe ser el If anidado (else if), no un valor simple
                assert!(matches!(else_block.tail.as_deref().map(|s| &s.node), Some(Expr::If { .. })));
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
        match &body.tail.as_deref().unwrap().node {
            Expr::Match { scrutinee, .. } => {
                assert!(matches!(scrutinee.node, Expr::Binary { op: BinaryOp::Add, .. }));
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
        assert!(matches!(&body.stmts[0].node, Stmt::Expr(e) if matches!(e.node, Expr::If { .. })));
        assert_eq!(body.tail.as_deref().map(|s| &s.node), Some(&Expr::Ident("r".into())));
    }

    #[test]
    fn assignment_statement_parses() {
        let prog = parse_source("fn f() -> Int { let mut x = 1; x = 2; x }");
        let Item::Fn(FnDecl { body, .. }) = &prog.items[0] else { panic!() };
        assert_eq!(body.stmts.len(), 2);
        match &body.stmts[1].node {
            Stmt::Assign { name, value } => {
                assert_eq!(name, "x");
                assert_eq!(value.node, Expr::Int(2));
            }
            other => panic!("se esperaba Stmt::Assign, fue {other:?}"),
        }
    }

    #[test]
    fn array_literal_and_indexing_parse() {
        let prog = parse_source("fn f() -> Int { let xs = [1, 2, 3]; xs[0] }");
        let Item::Fn(FnDecl { body, .. }) = &prog.items[0] else { panic!() };
        match &body.stmts[0].node {
            Stmt::Let { value, .. } => {
                assert_eq!(
                    value.node,
                    Expr::ArrayLit(vec![sp(Expr::Int(1)), sp(Expr::Int(2)), sp(Expr::Int(3))])
                );
            }
            other => panic!("se esperaba Stmt::Let, fue {other:?}"),
        }
        match &body.tail.as_deref().unwrap().node {
            Expr::Index { base, index } => {
                assert_eq!(base.node, Expr::Ident("xs".into()));
                assert_eq!(index.node, Expr::Int(0));
            }
            other => panic!("se esperaba Index, fue {other:?}"),
        }
    }

    #[test]
    fn empty_array_literal_and_trailing_comma_parse() {
        assert_eq!(
            parse_source("fn f() -> Int[] { [] }").items.len(),
            1
        );
        let prog = parse_source("fn f() -> Int { let xs = [1, 2,]; 0 }"); // coma final
        let Item::Fn(FnDecl { body, .. }) = &prog.items[0] else { panic!() };
        match &body.stmts[0].node {
            Stmt::Let { value, .. } => {
                assert_eq!(value.node, Expr::ArrayLit(vec![sp(Expr::Int(1)), sp(Expr::Int(2))]));
            }
            other => panic!("se esperaba Stmt::Let, fue {other:?}"),
        }
    }

    #[test]
    fn tuple_literal_vs_grouping_disambiguation() {
        // (a) sigue siendo agrupación -- misma regla que a nivel de tipos.
        let prog = parse_source("fn f() -> Int { (1) }");
        let Item::Fn(FnDecl { body, .. }) = &prog.items[0] else { panic!() };
        assert_eq!(
            body.tail.as_deref().map(|s| &s.node),
            Some(&Expr::Paren(Box::new(sp(Expr::Int(1)))))
        );

        // (a, b) es TupleLit de 2.
        let prog2 = parse_source(r#"fn f() -> Int { (1, "a") }"#);
        let Item::Fn(FnDecl { body, .. }) = &prog2.items[0] else { panic!() };
        assert_eq!(
            body.tail.as_deref().map(|s| &s.node),
            Some(&Expr::TupleLit(vec![sp(Expr::Int(1)), sp(Expr::Str("a".into()))]))
        );

        // (a,) con coma final es TupleLit de 1, no agrupación.
        let prog3 = parse_source("fn f() -> Int { (1,) }");
        let Item::Fn(FnDecl { body, .. }) = &prog3.items[0] else { panic!() };
        assert_eq!(
            body.tail.as_deref().map(|s| &s.node),
            Some(&Expr::TupleLit(vec![sp(Expr::Int(1))]))
        );
    }

    #[test]
    fn tuple_positional_access_parses() {
        let prog = parse_source(r#"fn f() -> Int { let t = (1, "a"); t.0 }"#);
        let Item::Fn(FnDecl { body, .. }) = &prog.items[0] else { panic!() };
        match &body.tail.as_deref().unwrap().node {
            Expr::TupleIndex { base, index } => {
                assert_eq!(base.node, Expr::Ident("t".into()));
                assert_eq!(*index, 0);
            }
            other => panic!("se esperaba TupleIndex, fue {other:?}"),
        }
    }

    #[test]
    fn full_users_demo_file_parses() {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/users.link"),
        )
        .expect("no se pudo leer examples/users.link");
        let prog = parse_source(&src);
        // 3 type (User, NewUser, NewUserRecord) + 3 enum (Role,
        // ValidationError, ValidateResult) + 1 db + 1 fn + 1 service = 9
        assert_eq!(prog.items.len(), 9);
        let service = prog
            .items
            .iter()
            .find_map(|i| match i {
                Item::Service(s) => Some(s),
                _ => None,
            })
            .expect("se esperaba un service");
        assert_eq!(service.name, "Users");
        // list, getById, create, update, remove, login, logout, listByRole,
        // listEmails, findByIdOrEmail, watchAll (stream)
        assert_eq!(service.members.len(), 11);
    }


    // ---- recuperación de errores (LSP prerrequisito 2/3) ----

    fn parse_errors(src: &str) -> Vec<ParseError> {
        let tokens = tokenize(src).unwrap_or_else(|e| panic!("{e}"));
        parse(tokens).expect_err("se esperaban errores de sintaxis")
    }

    #[test]
    fn well_formed_source_still_parses_with_no_errors() {
        // Recuperación no debería cambiar el camino feliz -- Ok, no Err([]).
        let tokens = tokenize("type P = { x: Int }").unwrap();
        assert!(parse(tokens).is_ok());
    }

    #[test]
    fn missing_closing_brace_does_not_swallow_the_next_item_error() {
        // Bug real encontrado por el review antes de implementar esto: una
        // versión de `synchronize()` que avanza un token incondicionalmente
        // ANTES de chequear se come el primer token del próximo ítem real
        // cada vez que el error ocurre anidado -- acá, la llave sin cerrar
        // de `service S` deja el error en el token `fn`, que en realidad es
        // el inicio de `ok`. La versión corregida (chequear ANTES de
        // avanzar) da 2 errores; la buggeada daba 1 (se comía `ok` entero).
        let src = "service S { rpc bad() -> Int { 1 }\nfn ok(*) -> Int { 2 }";
        let errors = parse_errors(src);
        assert_eq!(errors.len(), 2, "errores: {errors:?}");
    }

    #[test]
    fn parse_reports_multiple_independent_errors_in_one_pass() {
        let src = "fn a(*) -> Int { 1 } fn b(*) -> Int { 2 }";
        let errors = parse_errors(src);
        assert_eq!(errors.len(), 2, "errores: {errors:?}");
    }

    #[test]
    fn three_unrelated_top_level_errors_are_all_reported() {
        let src = r#"
            fn a(*) -> Int { 1 }
            enum E { }
            fn b(*) -> Int { 2 }
        "#;
        // `enum E { }` (sin variantes) no es en sí un error de sintaxis --
        // este test es sobre los 2 `fn` rotos, con un ítem BIEN formado en
        // el medio, confirmando que la recuperación no se confunde con eso.
        let errors = parse_errors(src);
        assert_eq!(errors.len(), 2, "errores: {errors:?}");
    }

    #[test]
    fn each_reported_error_carries_a_real_span() {
        let errors = parse_errors("fn a(*) -> Int { 1 }");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].span.line >= 1);
    }

    // ---- spans en el AST (LSP prerrequisito 3/3, Ronda A) ----

    #[test]
    fn binary_expr_span_covers_from_left_operand_to_right_operand() {
        let src = "fn f() -> Int { a + b }";
        let prog = parse_source(src);
        let Item::Fn(FnDecl { body, .. }) = &prog.items[0] else { panic!() };
        let tail = body.tail.as_deref().unwrap();
        let a_pos = src.find('a').unwrap();
        let b_pos = src.find('b').unwrap();
        assert_eq!(tail.span.start, a_pos, "el span de 'a + b' debería empezar en 'a'");
        assert_eq!(tail.span.end, b_pos + 1, "el span de 'a + b' debería terminar justo después de 'b'");
    }

    #[test]
    fn rpc_decl_span_includes_the_leading_annotation() {
        // Bug real encontrado por el review antes de implementar esto: si
        // parse_rpc_like capturara su propio inicio al entrar (la regla
        // "mecánica" ingenua), el span quedaría sistemáticamente AFUERA de
        // la @annotation, porque parse_optional_annotation ya la consumió
        // antes de que parse_rpc_like arranque. Ver parse_member.
        let src = "enum Role { Admin }\nservice S { @requires(Role.Admin) rpc f() -> Int { 1 } }";
        let prog = parse_source(src);
        let Item::Service(ServiceDecl { members, .. }) = &prog.items[1] else { panic!() };
        let Member::Rpc(rpc) = &members[0] else { panic!() };
        let at_pos = src.find('@').unwrap();
        let int_end = src.find("Int").unwrap() + "Int".len();
        assert_eq!(rpc.span.start, at_pos, "el span del rpc debería empezar en su propia @annotation");
        assert_eq!(rpc.span.end, int_end, "el span del rpc debería terminar en el return type, sin incluir el cuerpo");
    }

    // ---- constructo de loop: `while` (GRAMMAR.md §3.15) ----

    #[test]
    fn while_stmt_parses_condition_and_body() {
        let src = "fn f() -> Int { while a { b; } 0 }";
        let prog = parse_source(src);
        let Item::Fn(FnDecl { body, .. }) = &prog.items[0] else { panic!() };
        let Stmt::While { cond, body: while_body } = &body.stmts[0].node else {
            panic!("se esperaba Stmt::While, fue {:?}", body.stmts[0].node)
        };
        assert_eq!(cond.node, Expr::Ident("a".into()));
        assert_eq!(while_body.stmts.len(), 1);
    }

    #[test]
    fn while_stmt_span_covers_from_the_keyword_to_its_own_closing_brace() {
        let src = "fn f() -> Int { while a { b; } 0 }";
        let prog = parse_source(src);
        let Item::Fn(FnDecl { body, .. }) = &prog.items[0] else { panic!() };
        let while_pos = src.find("while").unwrap();
        // "b; }" matchea solo la llave de cierre DEL WHILE (no la del fn,
        // que cierra más adelante, después de "0 }") -- confirma que el
        // span no se come de más ni se queda corto.
        let while_close = src.find("b; }").unwrap() + "b; }".len();
        assert_eq!(body.stmts[0].span.start, while_pos, "el span debería empezar en 'while'");
        assert_eq!(body.stmts[0].span.end, while_close, "el span debería terminar en la llave de cierre del propio while");
    }

    #[test]
    fn while_condition_does_not_swallow_the_body_brace_as_a_struct_literal() {
        // Sin no_struct_lit=true en la condición (mismo mecanismo que ya
        // usa `if`/`match`), "while x { 1; }" sería tan ambiguo con un
        // struct-lit "x { 1; }" como "if x { ... }" ya lo es -- y como
        // "1;" no es un campo válido de struct-lit, la ambigüedad
        // resuelta mal haría que esto ni siquiera parseara.
        let src = "fn f(x: Bool) -> Int { while x { 1; } 0 }";
        let prog = parse_source(src);
        let Item::Fn(FnDecl { body, .. }) = &prog.items[0] else { panic!() };
        let Stmt::While { cond, .. } = &body.stmts[0].node else { panic!("se esperaba Stmt::While") };
        assert_eq!(cond.node, Expr::Ident("x".into()));
    }
}
