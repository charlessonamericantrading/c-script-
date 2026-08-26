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

    /// El docstring `///` que precede DIRECTAMENTE al token actual, si hay
    /// (GRAMMAR.md §3.72) -- tiene que leerse ANTES de consumir ningún token
    /// de la declaración (incluida una `@annotation` opcional, que puede
    /// venir DESPUÉS del docstring en el `.link` fuente), porque
    /// `leading_doc` vive en el primer token, no en uno fijo.
    fn peek_leading_doc(&self) -> Option<String> {
        self.tokens[self.pos].leading_doc.clone()
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

    /// Como `eat_ident`, pero en posicion de NOMBRE DE CAMPO tambien acepta
    /// una palabra clave. Ahi no hay ambiguedad -- una declaracion de campo es
    /// siempre `NOMBRE : tipo`, y despues de un `.` solo puede venir un campo
    /// o un indice de tupla -- y sin esto no se puede modelar una tabla con
    /// una columna `service`, `type` o `from`, que son nombres corrientes.
    fn eat_field_name(&mut self) -> Result<String, ParseError> {
        match self.peek().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                Ok(name)
            }
            other => match other.keyword_text() {
                Some(text) => {
                    self.advance();
                    Ok(text.to_string())
                }
                None => Err(self.error(format!(
                    "se esperaba un nombre de campo, se encontro {other:?}"
                ))),
            },
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

    /// Un número literal en una posición que NO es una expresión general
    /// (ej. el argumento de `@check(min, N)`, GRAMMAR.md §3.96) -- `Int` o
    /// `Float`, con un `-` opcional combinado acá mismo (mismo criterio que
    /// `parse_pattern_atom` para un patrón `-1`: es UN literal negativo, no
    /// "unario aplicado a algo", así que no se delega a la regla general de
    /// unario de expresiones). Siempre `f64` -- un límite de `@check` sobre
    /// un campo `Int`/`Int64` se compara igual de exacto como flotante.
    fn eat_number(&mut self) -> Result<f64, ParseError> {
        let negative = if self.check(&TokenKind::Minus) {
            self.advance();
            true
        } else {
            false
        };
        let n = match self.peek().clone() {
            TokenKind::Int(n) => {
                self.advance();
                n as f64
            }
            TokenKind::Float(n) => {
                self.advance();
                n
            }
            other => return Err(self.error(format!("se esperaba un número, se encontró {other:?}"))),
        };
        Ok(if negative { -n } else { n })
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
            TokenKind::Import | TokenKind::Type | TokenKind::Enum | TokenKind::Service | TokenKind::Const | TokenKind::Fn | TokenKind::Test
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
            TokenKind::Test => Ok(Item::Test(self.parse_test_decl()?)),
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
                "se esperaba un ítem de nivel superior (import/type/enum/service/const/fn/db/test), se encontró {other:?}"
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
        let annotations = self.parse_field_annotations()?;
        let name_span = self.span(); // cubre solo el identificador -- ver ast.rs::Field::name_span
        let name = self.eat_field_name()?;
        let optional = if self.check(&TokenKind::Question) {
            self.advance();
            true
        } else {
            false
        };
        self.eat(&TokenKind::Colon)?;
        let ty = self.parse_type_expr()?;
        // `= expr` (GRAMMAR.md §3.74) -- mismo lugar y misma sintaxis que
        // `Param::default` (`parse_param`), no una `@annotation`.
        let default = if self.check(&TokenKind::Equals) {
            self.advance();
            Some(self.parse_expr()?)
        } else {
            None
        };
        Ok(Field { name, optional, ty, name_span, annotations, default })
    }

    /// `@deprecated("...")`/`@validate(...)`/`@autoUpdate` antes de un campo
    /// de struct (GRAMMAR.md §3.71, §3.73 y §3.77) -- ver
    /// ast.rs::Field::annotations para por qué esto NO reusa
    /// `parse_optional_annotation` (esa devuelve `Vec<Annotation>` y acepta
    /// cualquier anotación de rpc, ninguna de las cuales tiene sentido sobre
    /// un campo). A lo sumo UNA de cada -- una segunda `@deprecated`/
    /// `@validate`/`@autoUpdate` sobre el mismo campo es un error acá mismo,
    /// mismo criterio que `@content_type`/`@route` en un rpc
    /// (`check_annotation_combination`), salvo que acá no hace falta el
    /// checker: es una cuenta puramente sintáctica. La FORMA del regex de
    /// `@validate(regex, "...")` se valida en el checker (`check_field_validators`),
    /// no acá -- necesita la crate `regex`, que el parser no depende de. Que
    /// `@autoUpdate` solo aplique sobre `Timestamp` TAMBIÉN se valida en el
    /// checker, mismo motivo (necesita el tipo resuelto).
    fn parse_field_annotations(&mut self) -> Result<Vec<FieldAnnotation>, ParseError> {
        let mut annotations = Vec::new();
        let mut seen_deprecated = false;
        let mut seen_validate = false;
        while self.check(&TokenKind::At) {
            self.advance();
            let name = self.eat_ident()?;
            match name.as_str() {
                "deprecated" => {
                    if seen_deprecated {
                        return Err(self.error("'@deprecated' repetido sobre el mismo campo: un campo tiene un solo motivo de baja".to_string()));
                    }
                    seen_deprecated = true;
                    self.eat(&TokenKind::LParen)?;
                    let reason = self.eat_string()?;
                    self.eat(&TokenKind::RParen)?;
                    if reason.trim().is_empty() {
                        return Err(self.error("`@deprecated(\"\")` sobre un campo: el motivo no puede estar vacío".to_string()));
                    }
                    annotations.push(FieldAnnotation::Deprecated(reason));
                }
                "validate" => {
                    if seen_validate {
                        return Err(self.error("'@validate' repetido sobre el mismo campo: un campo tiene un solo validador".to_string()));
                    }
                    seen_validate = true;
                    self.eat(&TokenKind::LParen)?;
                    let kind = self.eat_ident()?;
                    let validator = match kind.as_str() {
                        "email" => FieldValidator::Email,
                        "regex" => {
                            self.eat(&TokenKind::Comma)?;
                            let pattern = self.eat_string()?;
                            FieldValidator::Regex(pattern)
                        }
                        other => {
                            return Err(self.error(format!(
                                "'@validate({other}, ...)' desconocido (se esperaba '@validate(email)' o '@validate(regex, \"patrón\")')"
                            )))
                        }
                    };
                    self.eat(&TokenKind::RParen)?;
                    annotations.push(FieldAnnotation::Validate(validator));
                }
                // Sin paréntesis -- a diferencia de `@deprecated`/`@validate`,
                // no toma ningún argumento (GRAMMAR.md §3.77).
                "autoUpdate" => {
                    if annotations.iter().any(|a| matches!(a, FieldAnnotation::AutoUpdate)) {
                        return Err(self.error("'@autoUpdate' repetido sobre el mismo campo".to_string()));
                    }
                    annotations.push(FieldAnnotation::AutoUpdate);
                }
                // Sin paréntesis, mismo criterio que `@autoUpdate` (GRAMMAR.md §3.78).
                "softDelete" => {
                    if annotations.iter().any(|a| matches!(a, FieldAnnotation::SoftDelete)) {
                        return Err(self.error("'@softDelete' repetido sobre el mismo campo".to_string()));
                    }
                    annotations.push(FieldAnnotation::SoftDelete);
                }
                // `@index`/`@unique` (GRAMMAR.md §3.80) -- sin paréntesis,
                // a lo sumo UNO de los dos por campo (los dos piden un
                // índice, `@unique` además una restricción de unicidad --
                // combinarlos sería redundante, no un error del checker,
                // rechazado acá mismo por forma).
                "index" | "unique" => {
                    if annotations.iter().any(|a| matches!(a, FieldAnnotation::Index { .. })) {
                        return Err(self.error("'@index'/'@unique' repetido (o combinado) sobre el mismo campo -- un campo tiene a lo sumo uno de los dos".to_string()));
                    }
                    annotations.push(FieldAnnotation::Index { unique: name == "unique" });
                }
                // `@check(min, N)`/`@check(max, N)`/`@check(range, N, M)`
                // (GRAMMAR.md §3.96) -- mismo criterio de forma "kind +
                // argumento(s)" que `@validate` arriba.
                "check" => {
                    if annotations.iter().any(|a| matches!(a, FieldAnnotation::Check(_))) {
                        return Err(self.error("'@check' repetido sobre el mismo campo: un campo tiene a lo sumo un constraint".to_string()));
                    }
                    self.eat(&TokenKind::LParen)?;
                    let kind = self.eat_ident()?;
                    let check = match kind.as_str() {
                        "min" => {
                            self.eat(&TokenKind::Comma)?;
                            FieldCheck::Min(self.eat_number()?)
                        }
                        "max" => {
                            self.eat(&TokenKind::Comma)?;
                            FieldCheck::Max(self.eat_number()?)
                        }
                        "range" => {
                            self.eat(&TokenKind::Comma)?;
                            let min = self.eat_number()?;
                            self.eat(&TokenKind::Comma)?;
                            let max = self.eat_number()?;
                            FieldCheck::Range(min, max)
                        }
                        // `@check(minLength, N)`/`@check(maxLength, N)`
                        // (GRAMMAR.md §3.146) -- misma forma "kind + N" que
                        // `min`/`max`, pero para `String`/`String?` en vez de
                        // numérico; el checker es quien distingue cuál tipo
                        // de campo exige cada uno.
                        "minLength" => {
                            self.eat(&TokenKind::Comma)?;
                            FieldCheck::MinLength(self.eat_number()?)
                        }
                        "maxLength" => {
                            self.eat(&TokenKind::Comma)?;
                            FieldCheck::MaxLength(self.eat_number()?)
                        }
                        other => {
                            return Err(self.error(format!(
                                "'@check({other}, ...)' desconocido (se esperaba '@check(min, N)', '@check(max, N)', '@check(range, N, M)', '@check(minLength, N)' o '@check(maxLength, N)')"
                            )))
                        }
                    };
                    self.eat(&TokenKind::RParen)?;
                    annotations.push(FieldAnnotation::Check(check));
                }
                other => {
                    return Err(self.error(format!(
                        "anotación desconocida '@{other}' sobre un campo (se esperaba '@deprecated(\"motivo\")', '@validate(...)', '@autoUpdate', '@softDelete', '@index', '@unique' o '@check(...)')"
                    )))
                }
            }
        }
        Ok(annotations)
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
        // quedaría sistemáticamente AFUERA del span del rpc. Mismo motivo
        // para `doc` (GRAMMAR.md §3.72): un `///` en el `.link` fuente
        // siempre va ANTES de la `@annotation`, así que vive en el token que
        // hoy es `start`, no en el de `rpc`/`stream`.
        let start = self.span();
        let doc = self.peek_leading_doc();
        let annotations = self.parse_optional_annotation()?;
        match self.peek().clone() {
            TokenKind::Rpc => Ok(Member::Rpc(self.parse_rpc_like(start, TokenKind::Rpc, annotations, doc)?)),
            TokenKind::Stream => Ok(Member::Stream(self.parse_rpc_like(start, TokenKind::Stream, annotations, doc)?)),
            other => Err(self.error(format!("se esperaba 'rpc' o 'stream', se encontró {other:?}"))),
        }
    }

    /// Auth v0 (GRAMMAR.md §3.14): `@authenticated` o `@requires(Enum.Variante)`
    /// antes de `rpc`/`stream`. A propósito NO reusa `parse_pattern_atom`
    /// completo -- ese acepta opcionalmente `{ campos }` (destructuración),
    /// algo que acá nunca corresponde: `@requires` solo compara el tag de la
    /// variante, nunca mira campos.
    fn parse_optional_annotation(&mut self) -> Result<Vec<Annotation>, ParseError> {
        let mut annotations = Vec::new();
        // Varias anotaciones seguidas: `@requires(Role.Admin) @content_type("text/html")`.
        // Qué combinaciones son legales lo decide el checker, no el parser --
        // acá solo importa la forma.
        while self.check(&TokenKind::At) {
            self.advance();
            let name = self.eat_ident()?;
            let annotation = match name.as_str() {
                "authenticated" => Annotation::Authenticated,
                "requires" => {
                    self.eat(&TokenKind::LParen)?;
                    let enum_name = self.eat_ident()?;
                    self.eat(&TokenKind::Dot)?;
                    let mut variant_names = vec![self.eat_ident()?];
                    // `@requires(Role.Admin | Role.Agent)` (GRAMMAR.md
                    // §3.49): reusa el `|` que ya existe para uniones de
                    // tipo (`A | B`) -- mismo token, significado análogo
                    // ("cualquiera de estos"), sin gramática nueva. Los
                    // sucesivos `Enum.Variante` tienen que nombrar el MISMO
                    // enum que el primero -- se valida ACÁ, no en el
                    // checker, porque es puramente sintáctico (comparar
                    // identificadores, no hace falta tabla de símbolos) y
                    // el error sale antes, en el lugar exacto del token que
                    // no matchea.
                    while self.check(&TokenKind::Pipe) {
                        self.advance();
                        let next_enum = self.eat_ident()?;
                        if next_enum != enum_name {
                            return Err(self.error(format!(
                                "'@requires' mezcla dos enums distintos ('{enum_name}' y '{next_enum}') -- una sesión tiene el rol de UN solo enum, así que todas las alternativas de un mismo '@requires' tienen que venir del mismo enum"
                            )));
                        }
                        self.eat(&TokenKind::Dot)?;
                        variant_names.push(self.eat_ident()?);
                    }
                    self.eat(&TokenKind::RParen)?;
                    Annotation::Requires { enum_name, variant_names }
                }
                "content_type" => {
                    self.eat(&TokenKind::LParen)?;
                    let value = self.eat_string()?;
                    self.eat(&TokenKind::RParen)?;
                    Annotation::ContentType(value)
                }
                "route" => {
                    self.eat(&TokenKind::LParen)?;
                    let value = self.eat_string()?;
                    self.eat(&TokenKind::RParen)?;
                    Annotation::Route(value)
                }
                // `@rate_limit("N/ventana")` o, desde GRAMMAR.md §3.142,
                // `@rate_limit("N/ventana", key: <param>)` -- `key` es la
                // ÚNICA palabra clave aceptada acá (no un loop de claves
                // arbitrarias como `@example`, porque solo hay una cosa que
                // nombrar).
                "rate_limit" => {
                    self.eat(&TokenKind::LParen)?;
                    let spec = self.eat_string()?;
                    let key_param = if self.check(&TokenKind::Comma) {
                        self.advance();
                        let kw = self.eat_ident()?;
                        if kw != "key" {
                            return Err(self.error(format!(
                                "'@rate_limit' solo acepta 'key: <parámetro>' como segundo argumento, no '{kw}'"
                            )));
                        }
                        self.eat(&TokenKind::Colon)?;
                        Some(self.eat_ident()?)
                    } else {
                        None
                    };
                    self.eat(&TokenKind::RParen)?;
                    Annotation::RateLimit { spec, key_param }
                }
                "deprecated" => {
                    self.eat(&TokenKind::LParen)?;
                    let value = self.eat_string()?;
                    self.eat(&TokenKind::RParen)?;
                    Annotation::Deprecated(value)
                }
                "cache_control" => {
                    self.eat(&TokenKind::LParen)?;
                    let value = self.eat_string()?;
                    self.eat(&TokenKind::RParen)?;
                    Annotation::CacheControl(value)
                }
                // `@example(request: <expr>, response: <expr>)` (GRAMMAR.md
                // §3.119) -- a diferencia de las demás anotaciones, sus
                // valores son EXPRESIONES (mismo `parse_expr` que un
                // literal/struct-literal normal), no `String` crudo. Mismo
                // loop de coma que `parse_field_init_list`, pero con claves
                // fijas ('request'/'response') en vez de nombres de campo
                // arbitrarios.
                "example" => {
                    self.eat(&TokenKind::LParen)?;
                    if self.check(&TokenKind::RParen) {
                        return Err(self.error("'@example()' vacío no aporta nada -- declará al menos 'request' o 'response'"));
                    }
                    let mut request = None;
                    let mut response = None;
                    loop {
                        let key = self.eat_ident()?;
                        self.eat(&TokenKind::Colon)?;
                        let value = self.parse_expr()?;
                        match key.as_str() {
                            "request" if request.is_none() => request = Some(Box::new(value)),
                            "response" if response.is_none() => response = Some(Box::new(value)),
                            "request" | "response" => return Err(self.error(format!("'@example' declara '{key}' más de una vez"))),
                            other => return Err(self.error(format!("'@example' solo acepta 'request'/'response', no '{other}'"))),
                        }
                        if self.check(&TokenKind::Comma) {
                            self.advance();
                        } else {
                            break;
                        }
                    }
                    self.eat(&TokenKind::RParen)?;
                    // El chequeo de `()` vacío ya pasó ANTES del loop: si se
                    // llega acá, el loop parseó al menos un 'request:'/
                    // 'response:', así que esta rama siempre tiene alguno de
                    // los dos -- nunca los dos `None` a la vez.
                    Annotation::Example { request, response }
                }
                // `@invalidates(rpc1, rpc2, ...)` (GRAMMAR.md §3.125) --
                // identificadores sueltos (nombres de rpc de la misma
                // service), no `Enum.Variante` como `@requires` ni un
                // `String` -- mismo loop de coma que el resto de las listas
                // de este parser, sin claves.
                "invalidates" => {
                    self.eat(&TokenKind::LParen)?;
                    if self.check(&TokenKind::RParen) {
                        return Err(self.error("'@invalidates()' vacío no aporta nada -- nombrá al menos un rpc"));
                    }
                    let mut names = vec![self.eat_ident()?];
                    while self.check(&TokenKind::Comma) {
                        self.advance();
                        names.push(self.eat_ident()?);
                    }
                    self.eat(&TokenKind::RParen)?;
                    Annotation::Invalidates(names)
                }
                "infinite" => {
                    self.eat(&TokenKind::LParen)?;
                    let cursor_param = self.eat_ident()?;
                    self.eat(&TokenKind::Comma)?;
                    let limit_param = self.eat_ident()?;
                    self.eat(&TokenKind::RParen)?;
                    Annotation::Infinite { cursor_param, limit_param }
                }
                // Sin argumentos, igual que "authenticated" (GRAMMAR.md
                // §3.140) -- ninguna clave que parsear.
                "idempotent" => Annotation::Idempotent,
                "cache" => {
                    self.eat(&TokenKind::LParen)?;
                    let ttl = self.eat_string()?;
                    self.eat(&TokenKind::RParen)?;
                    Annotation::Cache(ttl)
                }
                "cors" => {
                    self.eat(&TokenKind::LParen)?;
                    let value = self.eat_string()?;
                    self.eat(&TokenKind::RParen)?;
                    Annotation::Cors(value)
                }
                other => {
                    return Err(self.error(format!(
                        "anotación desconocida '@{other}' (se esperaba '@authenticated', '@requires(Enum.Variante)', '@content_type(\"tipo/mime\")', '@route(\"/ruta/:param\")', '@rate_limit(\"N/ventana\")', '@deprecated(\"motivo\")', '@cache_control(\"public, max-age=N\")', '@example(request: ..., response: ...)', '@invalidates(rpc1, rpc2, ...)', '@infinite(cursor, limit)', '@idempotent', '@cache(\"60s\")' o '@cors(\"https://origen.com\")')"
                    )))
                }
            };
            annotations.push(annotation);
        }
        Ok(annotations)
    }

    /// `start` viene de `parse_member` (capturado antes de la anotación
    /// opcional, ver ahí). El span de la declaración cubre la FIRMA hasta el
    /// return type -- se calcula ANTES de parsear `body`, a propósito (el
    /// cuerpo tiene sus propios spans precisos, ver ast.rs::RpcDecl). `doc`
    /// idem, viene de `parse_member` (GRAMMAR.md §3.72).
    fn parse_rpc_like(&mut self, start: Span, kw: TokenKind, annotations: Vec<Annotation>, doc: Option<String>) -> Result<RpcDecl, ParseError> {
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
            annotations,
            doc,
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

    fn parse_test_decl(&mut self) -> Result<TestDecl, ParseError> {
        let start = self.span();
        self.eat(&TokenKind::Test)?;
        let name_span = Some(self.span());
        let name = match self.peek().clone() {
            TokenKind::Str(s) => {
                self.advance();
                s
            }
            TokenKind::Ident(s) => {
                self.advance();
                s
            }
            other => {
                return Err(self.error(format!(
                    "se esperaba el nombre del test (string o identificador), se encontró {other:?}"
                )));
            }
        };
        let body = self.parse_block()?;
        let span = merge(start, body.span);
        Ok(TestDecl {
            name,
            name_span,
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
        let name_span = self.span(); // cubre solo el identificador -- ver ast.rs::Param::name_span
        let name = self.eat_ident()?;
        self.eat(&TokenKind::Colon)?;
        let ty = self.parse_type_expr()?;
        let default = if self.check(&TokenKind::Equals) {
            self.advance();
            Some(self.parse_expr()?)
        } else {
            None
        };
        Ok(Param { name, ty, default, name_span })
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
                let name_span = self.span(); // cubre solo el identificador, no los type_args -- ver ast.rs::TypeExpr::Named
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
                Ok(TypeExpr::Named(name, args, name_span))
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
                // `if`/`match`/`transaction` son "block-like": terminan en
                // '}', así que no deberían necesitar un ';' para seguir
                // siendo una sentencia seguida de más código (`if cond { x
                // = 1; } else { x = 2; }` sin ';' y con algo más abajo).
                // Sin este caso, el `_` de abajo los trataría como el tail
                // apenas ve que no hay ';', y rompería con cualquier código
                // real después.
                TokenKind::If | TokenKind::Match | TokenKind::Transaction => {
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
        let cond = self.parse_coalesce_expr(true)?;
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
        } else if self.check(&TokenKind::Transaction) {
            self.parse_transaction_expr()
        } else {
            self.parse_coalesce_expr(no_struct_lit)
        }
    }

    /// `transaction { ... }` (GRAMMAR.md §3.154) -- más simple que
    /// `parse_if_expr`: sin condición, un solo `Block`, ninguna rama `else`
    /// que resolver. No hay ambigüedad de struct-lit que evitar (a
    /// diferencia de `if`/`match`, acá no hay ningún escrutinio/condición
    /// antes de la llave de apertura).
    fn parse_transaction_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let start = self.span();
        self.eat(&TokenKind::Transaction)?;
        let block = self.parse_block()?;
        let span = merge(start, self.prev_span());
        Ok(Spanned { node: Expr::Transaction(block), span })
    }

    /// `a ?? b` (GRAMMAR.md §3.9) -- la precedencia más floja de todas (ata
    /// después de `||`/`&&`/comparaciones), mismo lugar que ocupa en
    /// TypeScript/Rust (`.unwrap_or`). `a ?? b ?? c` asocia a IZQUIERDA,
    /// como cualquier otro binario de esta cadena.
    fn parse_coalesce_expr(&mut self, no_struct_lit: bool) -> Result<Spanned<Expr>, ParseError> {
        let mut left = self.parse_or_expr(no_struct_lit)?;
        while self.check(&TokenKind::QuestionQuestion) {
            self.advance();
            let right = self.parse_or_expr(no_struct_lit)?;
            let span = merge(left.span, right.span);
            left = Spanned { node: Expr::Binary { op: BinaryOp::Coalesce, left: Box::new(left), right: Box::new(right) }, span };
        }
        Ok(left)
    }

    fn parse_if_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let start = self.span();
        self.eat(&TokenKind::If)?;
        // La condición siempre restringe struct-lits, sin importar el
        // contexto exterior: `if x { ... }` es ambiguo igual que `match`.
        let cond = Box::new(self.parse_coalesce_expr(true)?);
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
            Some(self.parse_coalesce_expr(true)?)
        } else {
            None
        };
        self.eat(&TokenKind::FatArrow)?;
        let (body, span) = if self.check(&TokenKind::LBrace) {
            let block = self.parse_block()?;
            let span = merge(start, block.span);
            if self.check(&TokenKind::Comma) {
                self.advance();
            }
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
            // `null` como patrón (GRAMMAR.md §3.9): narrowing real de un
            // `T?` -- `match x { v: Item => ..., null => ... }`. Solo válido
            // contra un escrutinio opcional, el checker es quien lo rechaza
            // en cualquier otro lado (check_literal_matches_type).
            TokenKind::Null => {
                self.advance();
                Ok(Pattern::Literal(LiteralPattern::Null))
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
        let name = self.eat_field_name()?;
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
                            let field = self.eat_field_name()?;
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
        let name = self.eat_field_name()?;
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

    // Span placeholder para literales de test -- TypeExpr::PartialEq ignora
    // el span de Named a propósito (ast.rs), así que cualquier valor sirve.
    const NOSPAN: Span = Span { start: 0, end: 0, line: 0, col: 0 };

    #[test]
    fn postfix_order_changes_the_type() {
        // T[]? = Optional(List(T))
        let prog = parse_source("type A = User[]?;");
        let Item::Type(TypeDecl { ty, .. }) = &prog.items[0] else { panic!() };
        assert_eq!(
            *ty,
            TypeExpr::Optional(Box::new(TypeExpr::List(Box::new(TypeExpr::Named(
                "User".into(),
                vec![],
                NOSPAN
            )))))
        );

        // T?[] = List(Optional(T))
        let prog2 = parse_source("type B = User?[];");
        let Item::Type(TypeDecl { ty, .. }) = &prog2.items[0] else { panic!() };
        assert_eq!(
            *ty,
            TypeExpr::List(Box::new(TypeExpr::Optional(Box::new(TypeExpr::Named(
                "User".into(),
                vec![],
                NOSPAN
            )))))
        );
    }

    #[test]
    fn paren_grouping_vs_tuple_vs_function_type() {
        let prog = parse_source(
            "type A = (Int); type B = (Int, String); type C = (Int, String) -> Bool;",
        );
        let Item::Type(TypeDecl { ty: a, .. }) = &prog.items[0] else { panic!() };
        assert_eq!(*a, TypeExpr::Named("Int".into(), vec![], NOSPAN)); // agrupación pura

        let Item::Type(TypeDecl { ty: b, .. }) = &prog.items[1] else { panic!() };
        assert_eq!(
            *b,
            TypeExpr::Tuple(vec![
                TypeExpr::Named("Int".into(), vec![], NOSPAN),
                TypeExpr::Named("String".into(), vec![], NOSPAN)
            ])
        );

        let Item::Type(TypeDecl { ty: c, .. }) = &prog.items[2] else { panic!() };
        assert_eq!(
            *c,
            TypeExpr::Function(
                vec![
                    TypeExpr::Named("Int".into(), vec![], NOSPAN),
                    TypeExpr::Named("String".into(), vec![], NOSPAN)
                ],
                Box::new(TypeExpr::Named("Bool".into(), vec![], NOSPAN))
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
                    TypeExpr::Named("User".into(), vec![], NOSPAN),
                    TypeExpr::Named("ValidationError".into(), vec![], NOSPAN)
                ],
                NOSPAN
            )
        );
    }

    #[test]
    fn keywords_are_valid_field_names_in_declarations_literals_and_access() {
        // Un esquema real puede tener una columna `service`, `type` o `from`.
        // En posicion de nombre de campo no hay ambiguedad posible, asi que
        // reservarlas ahi hacia el modelo indescriptible sin renombrar la
        // columna de produccion.
        let program = parse_source(
            "type Lead = { id: Int, service: String, type: String, from: String }
             fn pick(l: Lead) -> String { l.service }",
        );
        assert_eq!(program.items.len(), 2);

        let program = parse_source(
            "type L = { id: Int, service: String }
             fn make() -> L { L { id: 1, service: \"seo\" } }",
        );
        assert_eq!(program.items.len(), 2);
    }

    #[test]
    fn a_keyword_is_still_rejected_where_a_real_identifier_is_required() {
        // El relajamiento es SOLO en posicion de campo: un `type` como nombre
        // de rpc, de fn o de parametro sigue siendo un error de sintaxis.
        let tokens = tokenize("fn service() -> Int { 1 }").unwrap();
        assert!(parse(tokens).is_err(), "'service' no deberia valer como nombre de fn");

        let tokens = tokenize("type type = { id: Int }").unwrap();
        assert!(parse(tokens).is_err(), "'type' no deberia valer como nombre de tipo");
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

    /// `@example(request: <expr>, response: <expr>)` (GRAMMAR.md §3.119) --
    /// a diferencia del resto de las anotaciones, sus valores son
    /// expresiones de verdad (acepta un `StructLit` completo), no `String`.
    #[test]
    fn example_annotation_parses_request_and_response_as_expressions() {
        let prog = parse_source(
            r#"
            service Tasks {
                @example(request: Input { title: "x" }, response: Task { id: 1 })
                rpc create(title: String) -> Task { Task { id: 1 } }
            }
        "#,
        );
        let Item::Service(s) = &prog.items[0] else { panic!() };
        let Member::Rpc(r) = &s.members[0] else { panic!() };
        let Some(Annotation::Example { request, response }) = r.annotations.first() else {
            panic!("se esperaba Annotation::Example, fue {:?}", r.annotations);
        };
        assert!(matches!(request.as_deref().map(|s| &s.node), Some(Expr::StructLit { name, .. }) if name == "Input"));
        assert!(matches!(response.as_deref().map(|s| &s.node), Some(Expr::StructLit { name, .. }) if name == "Task"));
    }

    #[test]
    fn example_annotation_with_empty_parens_is_a_parse_error() {
        let tokens = tokenize("service S { @example() rpc f() -> Int { 1 } }").unwrap();
        let err = parse(tokens).unwrap_err();
        assert!(err.iter().any(|e| e.message.contains("vacío")), "mensaje inesperado: {err:?}");
    }

    #[test]
    fn example_annotation_rejects_a_key_other_than_request_or_response() {
        let tokens = tokenize(r#"service S { @example(bogus: 1) rpc f() -> Int { 1 } }"#).unwrap();
        assert!(parse(tokens).is_err());
    }

    #[test]
    fn example_annotation_rejects_the_same_key_twice() {
        let tokens = tokenize("service S { @example(response: 1, response: 2) rpc f() -> Int { 1 } }").unwrap();
        let err = parse(tokens).unwrap_err();
        assert!(err.iter().any(|e| e.message.contains("más de una vez")), "mensaje inesperado: {err:?}");
    }

    /// `@invalidates(rpc1, rpc2, ...)` (GRAMMAR.md §3.125) -- identificadores
    /// sueltos (nombres de rpc), no `Enum.Variante` como `@requires`.
    #[test]
    fn invalidates_annotation_parses_a_list_of_bare_rpc_names() {
        let prog = parse_source("service S { @invalidates(list, search) rpc create() -> Int { 1 } }");
        let Item::Service(s) = &prog.items[0] else { panic!() };
        let Member::Rpc(r) = &s.members[0] else { panic!() };
        assert_eq!(r.annotations, vec![Annotation::Invalidates(vec!["list".to_string(), "search".to_string()])]);
    }

    #[test]
    fn invalidates_annotation_with_empty_parens_is_a_parse_error() {
        let tokens = tokenize("service S { @invalidates() rpc f() -> Int { 1 } }").unwrap();
        let err = parse(tokens).unwrap_err();
        assert!(err.iter().any(|e| e.message.contains("vacío")), "mensaje inesperado: {err:?}");
    }

    /// `@infinite(cursor, limit)` (GRAMMAR.md §3.134) -- dos identificadores
    /// sueltos, siempre en ese orden.
    #[test]
    fn infinite_annotation_parses_the_two_bare_param_names() {
        let prog = parse_source(
            "service S { @infinite(cursor, limit) rpc list(cursor: Int?, limit: Int) -> Int[] { [] } }",
        );
        let Item::Service(s) = &prog.items[0] else { panic!() };
        let Member::Rpc(r) = &s.members[0] else { panic!() };
        assert_eq!(
            r.annotations,
            vec![Annotation::Infinite { cursor_param: "cursor".to_string(), limit_param: "limit".to_string() }]
        );
    }

    #[test]
    fn infinite_annotation_requires_exactly_two_names() {
        let tokens = tokenize("service S { @infinite(cursor) rpc f(cursor: Int?) -> Int[] { [] } }").unwrap();
        assert!(parse(tokens).is_err());
    }

    #[test]
    fn rate_limit_annotation_without_a_key_parses_as_before() {
        let prog = parse_source(r#"service S { @rate_limit("5/1m") rpc f() -> Int { 1 } }"#);
        let Item::Service(s) = &prog.items[0] else { panic!() };
        let Member::Rpc(r) = &s.members[0] else { panic!() };
        assert_eq!(r.annotations, vec![Annotation::RateLimit { spec: "5/1m".to_string(), key_param: None }]);
    }

    #[test]
    fn rate_limit_annotation_parses_an_optional_key_clause() {
        let prog =
            parse_source(r#"service S { @rate_limit("5/1m", key: email) rpc f(email: String) -> Int { 1 } }"#);
        let Item::Service(s) = &prog.items[0] else { panic!() };
        let Member::Rpc(r) = &s.members[0] else { panic!() };
        assert_eq!(
            r.annotations,
            vec![Annotation::RateLimit { spec: "5/1m".to_string(), key_param: Some("email".to_string()) }]
        );
    }

    #[test]
    fn rate_limit_annotation_rejects_a_second_argument_that_is_not_key() {
        let tokens = tokenize(r#"service S { @rate_limit("5/1m", other: email) rpc f(email: String) -> Int { 1 } }"#).unwrap();
        assert!(parse(tokens).is_err());
    }

    #[test]
    fn idempotent_annotation_takes_no_arguments() {
        let prog = parse_source("service S { @idempotent rpc create(name: String) -> Int { 1 } }");
        let Item::Service(s) = &prog.items[0] else { panic!() };
        let Member::Rpc(r) = &s.members[0] else { panic!() };
        assert_eq!(r.annotations, vec![Annotation::Idempotent]);
    }

    #[test]
    fn idempotent_annotation_rejects_parentheses() {
        let tokens = tokenize("service S { @idempotent() rpc f() -> Int { 1 } }").unwrap();
        assert!(parse(tokens).is_err());
    }

    #[test]
    fn cache_annotation_parses_the_ttl_string() {
        let prog = parse_source(r#"service S { @cache("60s") rpc f() -> Int { 1 } }"#);
        let Item::Service(s) = &prog.items[0] else { panic!() };
        let Member::Rpc(r) = &s.members[0] else { panic!() };
        assert_eq!(r.annotations, vec![Annotation::Cache("60s".to_string())]);
    }

    #[test]
    fn cache_annotation_requires_a_string_argument() {
        let tokens = tokenize("service S { @cache() rpc f() -> Int { 1 } }").unwrap();
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
        // ValidationError, ValidateResult) + 1 db + 1 fn + 1 service + 2 test = 11
        assert_eq!(prog.items.len(), 11);
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

    // ---- bloques test integrados (PLAN.md §5, Eje 2) ----

    #[test]
    fn parses_test_decl_with_string_name() {
        let src = r#"test "crear usuario basico" { let x = 1; assert(x == 1); }"#;
        let prog = parse_source(src);
        assert_eq!(prog.items.len(), 1);
        let Item::Test(t) = &prog.items[0] else { panic!("se esperaba Item::Test") };
        assert_eq!(t.name, "crear usuario basico");
        assert_eq!(t.body.stmts.len(), 2);
    }

    #[test]
    fn parses_test_decl_with_identifier_name() {
        let src = "test smoke_test { assert(true); }";
        let prog = parse_source(src);
        assert_eq!(prog.items.len(), 1);
        let Item::Test(t) = &prog.items[0] else { panic!("se esperaba Item::Test") };
        assert_eq!(t.name, "smoke_test");
        assert_eq!(t.body.stmts.len(), 1);
    }

    // ---- docstrings `///` (GRAMMAR.md §3.72) ----

    #[test]
    fn a_triple_slash_docstring_directly_above_an_rpc_attaches_to_it() {
        let src = "service S {\n/// crea un usuario nuevo\nrpc create() -> Int { 1 }\n}";
        let prog = parse_source(src);
        let Item::Service(ServiceDecl { members, .. }) = &prog.items[0] else { panic!() };
        let Member::Rpc(r) = &members[0] else { panic!() };
        assert_eq!(r.doc.as_deref(), Some("crea un usuario nuevo"));
    }

    /// El docstring va ANTES de la `@annotation`, no entre esta y `rpc` --
    /// tiene que seguir atribuyéndose al rpc igual (ver `parse_member`).
    #[test]
    fn a_docstring_before_an_annotation_still_attaches_to_the_rpc() {
        let src = "enum Role { Admin }\nservice S {\n/// solo para administradores\n@requires(Role.Admin)\nrpc panel() -> Int { 1 }\n}";
        let prog = parse_source(src);
        let Item::Service(ServiceDecl { members, .. }) = &prog.items[1] else { panic!() };
        let Member::Rpc(r) = &members[0] else { panic!() };
        assert_eq!(r.doc.as_deref(), Some("solo para administradores"));
        assert!(r.auth().is_some());
    }

    #[test]
    fn an_rpc_with_no_docstring_has_doc_none() {
        let src = "service S { rpc f() -> Int { 1 } }";
        let prog = parse_source(src);
        let Item::Service(ServiceDecl { members, .. }) = &prog.items[0] else { panic!() };
        let Member::Rpc(r) = &members[0] else { panic!() };
        assert!(r.doc.is_none());
    }

    // ---- valores por defecto en campos de struct (GRAMMAR.md §3.74) ----

    #[test]
    fn a_field_default_parses_after_the_type() {
        let src = r#"type Task = { title: String, status: String = "pending" }"#;
        let prog = parse_source(src);
        let Item::Type(t) = &prog.items[0] else { panic!() };
        let TypeExpr::Struct(fields) = &t.ty else { panic!() };
        assert!(fields[0].default.is_none());
        assert_eq!(fields[1].default.as_ref().unwrap().node, Expr::Str("pending".into()));
    }

    #[test]
    fn a_field_without_a_default_has_none() {
        let src = "type Task = { title: String }";
        let prog = parse_source(src);
        let Item::Type(t) = &prog.items[0] else { panic!() };
        let TypeExpr::Struct(fields) = &t.ty else { panic!() };
        assert!(fields[0].default.is_none());
    }
}
