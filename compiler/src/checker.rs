// Type checker bidireccional (GRAMMAR.md §3): síntesis (⇒) para lo que se
// puede inferir de abajo hacia arriba, chequeo (⇐) para lo que necesita un
// tipo esperado (match, y la construcción de Result<T,E> — ver más abajo).

use crate::ast::*;
use crate::token::Span;
use crate::types::{is_subtype, FieldType, Type};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// `span` es `Option` (no obligatorio) -- algunos errores no son cleanly
/// "sobre" un nodo puntual (ej. un nombre duplicado detectado entre DOS
/// declaraciones en `build_symbols`). `err(...)` sigue sin tocarse: produce
/// `span: None`, y los ~113 call sites existentes no necesitan saber nada de
/// esto -- el span se estampa DESPUÉS, en un puñado de puntos de frontera
/// (`with_span`, más abajo), no en cada sitio de error.
///
/// `file` (identidad de archivo, GRAMMAR.md §3.21 "Not done yet") sigue el
/// mismo patrón que `span`: `None` por defecto, estampado en los mismos 5
/// puntos de entrada de `check_program_full` cuando el caller le pasa
/// `item_files` (ver ese método) -- `None` para cualquier caller que no
/// tiene identidad de archivo (todos los tests existentes, que construyen
/// un `Program` a mano sin pasar por `modules.rs`), preservando su
/// comportamiento exacto de antes.
/// `code` (GRAMMAR.md §3.210): igual criterio que `span`/`file` arriba --
/// `None` por defecto vía `err(...)`, estampado explícitamente solo en el
/// puñado de sitios curados con su propia entrada en `error_codes::CODES`
/// (`.with_code("L0001")`, ver ese módulo para la lista completa y por qué
/// NO todo error tiene uno).
#[derive(Debug)]
pub struct CheckError {
    pub message: String,
    pub span: Option<Span>,
    pub file: Option<PathBuf>,
    pub code: Option<&'static str>,
}

impl std::fmt::Display for CheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.code {
            Some(c) => write!(f, "error de tipos [{c}]: {}", self.message),
            None => write!(f, "error de tipos: {}", self.message),
        }
    }
}

fn err(msg: impl Into<String>) -> CheckError {
    CheckError { message: msg.into(), span: None, file: None, code: None }
}

/// Fast-path para un builtin CURADO nuevo (namespace.method) CON AL MENOS 1
/// ARGUMENTO -- un builtin de 0 args sigue con `expect_no_args` de siempre,
/// nunca con este macro (ver nota abajo). La forma destructurar-N-args-
/// exactos + un `check_expr` por posición + devolver un `Type` es IDÉNTICA
/// en decenas de arms de `try_builtin_method` (`crypto`/`http`/`math`/etc.)
/// -- este macro la genera a partir de la firma declarada, reusando el
/// MISMO patrón de slice (`let [a, b, ...] = args else { ... }`) que esos
/// arms ya escriben a mano. El lado RUNTIME (`call_method`, `runtime/mod.rs`)
/// sigue escrito a mano a propósito: la lógica real (Argon2, HTTP, HMAC...)
/// varía demasiado para generarse, y tratar de unificarla escondería la
/// lógica real detrás de una capa que no aporta nada ahí -- este macro SOLO
/// ataca el lado que de verdad es mecánico, el checker nunca tiene lógica
/// propia, solo tipa (GRAMMAR.md §3.186). Alcance v0: SOLO para builtins
/// NUEVOS de acá en adelante, no un retrofit retroactivo de todos los que
/// ya existen -- funcionan, están probados, tocarlos sin necesidad real es
/// riesgo sin beneficio.
///
/// Requiere al menos 1 argumento (repetición `+`, nunca `*`): con `*` un
/// builtin de 0 args expandiría `[$($pdesc),*].join(", ")` a `[].join(", ")`,
/// que no compila (`E0282`, un array vacío no tiene forma de inferir su
/// tipo de elemento) -- encontrado revisando este macro ANTES de escribirlo
/// de verdad, no en el compilador. Un builtin de 0 args de todos modos tiene
/// su propio mensaje distinto ("no toma argumentos" vs. "toma exactamente 0
/// argumentos ()") -- unificar los dos casos no vale la complejidad para
/// los pocos builtins sin argumentos que existen.
macro_rules! builtin_args {
    ($self:ident, $args:ident, $env:ident, $qualified_name:literal, [$(($pname:ident, $pdesc:literal, $pty:expr)),+ $(,)?] -> $ret:expr) => {{
        let [$($pname),+] = $args else {
            let n = 0usize $(+ { let _ = stringify!($pname); 1 })+;
            let word = if n == 1 { "argumento" } else { "argumentos" };
            let desc = [$($pdesc),+].join(", ");
            return Err(err(format!("'{}' toma exactamente {n} {word} ({desc})", $qualified_name)));
        };
        $($self.check_expr($pname, &$pty, $env)?;)+
        Some($ret)
    }};
}

/// El tipo que `http.getWithHeaders`/`http.postWithHeaders` (GRAMMAR.md
/// §3.47) esperan para cada header: SIN nombre (`name: None`) a propósito --
/// el subtipado estructural (§3.2) ya acepta cualquier struct declarado por
/// el usuario que tenga estos dos campos, así que no hace falta que el
/// lenguaje invente un tipo `Header` propio ni que el usuario nombre el suyo
/// de una forma particular. `is_subtype` ignora el nombre en la comparación
/// (`structural_subtyping_ignores_the_name`, types.rs), que es exactamente
/// la propiedad que esto usa.
fn http_header_type() -> Type {
    Type::Struct {
        name: None,
        fields: vec![
            FieldType { name: "name".to_string(), optional: false, ty: Type::String },
            FieldType { name: "value".to_string(), optional: false, ty: Type::String },
        ],
    }
}

/// Lo que `http.getWithStatus`/`http.postWithStatus` (GRAMMAR.md §3.60)
/// devuelven -- mismo criterio que `http_header_type`: estructural, SIN
/// nombre, así que cualquier `type` que el programa declare con estos tres
/// campos exactos sirve como destino, sin que el lenguaje tenga que inventar
/// un `HttpResponse` propio.
fn http_response_type() -> Type {
    Type::Struct {
        name: None,
        fields: vec![
            FieldType { name: "status".to_string(), optional: false, ty: Type::Int },
            FieldType { name: "headers".to_string(), optional: false, ty: Type::List(Box::new(http_header_type())) },
            FieldType { name: "body".to_string(), optional: false, ty: Type::String },
        ],
    }
}

/// El tipo que `sitemapXml(urls)` (GRAMMAR.md §3.116) espera para cada
/// entrada -- mismo criterio estructural sin nombre que `http_header_type`.
/// `lastmod` opcional: la mayoría de las URLs de un sitio real no tienen
/// (o no vale la pena calcular) una fecha de última modificación exacta --
/// el protocolo de sitemaps.org ya trata ese elemento como opcional.
fn sitemap_url_type() -> Type {
    Type::Struct {
        name: None,
        fields: vec![
            FieldType { name: "loc".to_string(), optional: false, ty: Type::String },
            FieldType { name: "lastmod".to_string(), optional: true, ty: Type::Timestamp },
        ],
    }
}

/// El tipo que `robotsTxt(rules, sitemapUrl)` (GRAMMAR.md §3.116) espera
/// para cada bloque `User-agent: ...` -- `allow`/`disallow` OPCIONALES
/// (`String[]?`, no listas requeridas): el caso real más común es
/// "solo bloquear" o "solo permitir" un user-agent, así que se puede omitir
/// la que no haga falta en vez de escribir `[]` a mano; en runtime, ausente
/// (`null`) se trata exactamente igual que una lista vacía -- ningún
/// `Disallow`/`Allow` para ese bloque.
fn robots_rule_type() -> Type {
    Type::Struct {
        name: None,
        fields: vec![
            FieldType { name: "userAgent".to_string(), optional: false, ty: Type::String },
            FieldType { name: "disallow".to_string(), optional: true, ty: Type::List(Box::new(Type::String)) },
            FieldType { name: "allow".to_string(), optional: true, ty: Type::List(Box::new(Type::String)) },
        ],
    }
}

/// `@example(request: <expr>, response: <expr>)` (GRAMMAR.md §3.119) solo
/// acepta expresiones LITERALES -- un valor fijo conocido en compilación,
/// nunca algo recalculado en cada build (`crypto.uuid()`/`now()` darían un
/// `openapi.json` distinto cada vez, rompiendo `linkc build --diff`,
/// GRAMMAR.md §3.79). Recorre `ArrayLit`/`TupleLit`/`StructLit` porque un
/// ejemplo real es casi siempre un struct anidado, no un escalar suelto;
/// `Unary(Neg, ...)` sobre un número es el único caso no-atómico admitido,
/// para que `-1` siga siendo un literal y no una "expresión calculada".
fn is_literal_expr(e: &Expr) -> bool {
    match e {
        Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Bool(_) | Expr::Null => true,
        Expr::Unary { op: UnaryOp::Neg, operand } => matches!(operand.node, Expr::Int(_) | Expr::Float(_)),
        Expr::ArrayLit(items) | Expr::TupleLit(items) => items.iter().all(|i| is_literal_expr(&i.node)),
        Expr::StructLit { fields, .. } => fields.iter().all(|(_, v)| is_literal_expr(&v.node)),
        _ => false,
    }
}

/// El tipo que `metaTags(tags)` (GRAMMAR.md §3.117) espera para cada
/// entrada -- mismo criterio estructural sin nombre que `sitemap_url_type`.
/// Meta tags clásicos (`description`, `robots`, `viewport`, ...) usan el
/// atributo `name`; Open Graph usa `property` en cambio, de ahí que sea un
/// `type` estructural distinto (`open_graph_tag_type` abajo) y no el mismo
/// reusado con un campo opcional.
/// Lo que `staticRoutes(baseUrl)` (GRAMMAR.md §3.222) devuelve por
/// elemento -- deliberadamente la forma MÍNIMA que `sitemapXml` acepta
/// (`lastmod` es opcional ahí, §3.116), para que
/// `sitemapXml(staticRoutes("https://x.com"))` tipe sin adaptador.
fn static_route_type() -> Type {
    Type::Struct { name: None, fields: vec![FieldType { name: "loc".to_string(), optional: false, ty: Type::String }] }
}

/// Lo que `hreflangLinks(alternates)` (GRAMMAR.md §3.222) espera por
/// elemento: un código de idioma/región (`es`, `en-US`, `x-default`) y la
/// URL absoluta de esa variante. Mismo criterio estructural que `metaTags`.
fn hreflang_link_type() -> Type {
    Type::Struct {
        name: None,
        fields: vec![
            FieldType { name: "lang".to_string(), optional: false, ty: Type::String },
            FieldType { name: "href".to_string(), optional: false, ty: Type::String },
        ],
    }
}

fn meta_tag_type() -> Type {
    Type::Struct {
        name: None,
        fields: vec![
            FieldType { name: "name".to_string(), optional: false, ty: Type::String },
            FieldType { name: "content".to_string(), optional: false, ty: Type::String },
        ],
    }
}

/// El tipo que `openGraphTags(tags)` (GRAMMAR.md §3.117) espera para cada
/// entrada -- mismo campo `content` que `meta_tag_type`, pero `property` en
/// vez de `name`, porque así es como Open Graph (`og:title`, `og:image`,
/// ...) distingue sus meta tags de los clásicos en el HTML real.
fn open_graph_tag_type() -> Type {
    Type::Struct {
        name: None,
        fields: vec![
            FieldType { name: "property".to_string(), optional: false, ty: Type::String },
            FieldType { name: "content".to_string(), optional: false, ty: Type::String },
        ],
    }
}

/// El tipo de cada entrada de `attachments` en `smtp.sendMessage(...)`
/// (GRAMMAR.md §3.141) -- `contentBase64` porque c-script no tiene un tipo
/// de bytes crudos: el contenido del archivo viaja codificado en base64,
/// igual que cualquier binario dentro de JSON, y se decodifica del lado del
/// runtime (`runtime::send_email_advanced`) directo a `Vec<u8>` sin pasar
/// por `base64.decode` (que exige UTF-8 válido en el resultado, algo que un
/// adjunto binario real casi nunca es).
fn smtp_attachment_type() -> Type {
    Type::Struct {
        name: None,
        fields: vec![
            FieldType { name: "filename".to_string(), optional: false, ty: Type::String },
            FieldType { name: "contentType".to_string(), optional: false, ty: Type::String },
            FieldType { name: "contentBase64".to_string(), optional: false, ty: Type::String },
        ],
    }
}

/// El tipo que `smtp.sendMessage(message)` (GRAMMAR.md §3.141) espera --
/// variante "kitchen sink" de `send`/`sendToMany`/`sendHtml` (arriba, sin
/// cambios) para el caso que esos tres no cubren: copia oculta/visible y
/// adjuntos reales. `cc`/`bcc`/`attachments` opcionales-por-clave (mismo
/// criterio que `disallow`/`allow` de `robots_rule_type`) -- el caso más
/// común (sin ninguno de los tres) no obliga a escribir `[]` a mano.
fn smtp_message_type() -> Type {
    Type::Struct {
        name: None,
        fields: vec![
            FieldType { name: "to".to_string(), optional: false, ty: Type::List(Box::new(Type::String)) },
            FieldType { name: "cc".to_string(), optional: true, ty: Type::List(Box::new(Type::String)) },
            FieldType { name: "bcc".to_string(), optional: true, ty: Type::List(Box::new(Type::String)) },
            FieldType { name: "subject".to_string(), optional: false, ty: Type::String },
            FieldType { name: "body".to_string(), optional: false, ty: Type::String },
            FieldType { name: "html".to_string(), optional: true, ty: Type::Bool },
            FieldType { name: "attachments".to_string(), optional: true, ty: Type::List(Box::new(smtp_attachment_type())) },
        ],
    }
}

impl CheckError {
    /// El PRIMER stamp gana: a medida que un error burbujea desde adentro
    /// hacia afuera (ej. de una sub-expresión hasta la sentencia que la
    /// contiene), el span más profundo -- el más específico -- es el que
    /// queda, nunca uno más externo lo pisa.
    fn with_span(mut self, span: Span) -> Self {
        if self.span.is_none() {
            self.span = Some(span);
        }
        self
    }

    /// A diferencia de `with_span`, acá no hace falta "primer stamp gana":
    /// el archivo es constante para TODO el subárbol que
    /// `check_program_full` chequea en una misma iteración de su loop
    /// top-level (un ítem nunca se parte entre dos archivos), así que
    /// siempre es correcto pisarlo con el valor más reciente -- no hay
    /// noción de "más específico" como con el span, que sí varía por
    /// profundidad.
    fn with_file(mut self, file: PathBuf) -> Self {
        self.file = Some(file);
        self
    }

    /// GRAMMAR.md §3.210: mismo criterio que `with_span` -- el primer stamp
    /// gana, para que envolver un error de más abajo (ej. dentro de
    /// `synth_expr_inner` re-entrando sobre sí mismo, como hace la sugar de
    /// §3.209) nunca le pise el código más específico que ya tenía.
    fn with_code(mut self, code: &'static str) -> Self {
        if self.code.is_none() {
            self.code = Some(code);
        }
        self
    }
}

fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let mut dp = vec![vec![0; b_chars.len() + 1]; a_chars.len() + 1];

    for (i, row) in dp.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in dp[0].iter_mut().enumerate() {
        *cell = j;
    }

    for (i, ca) in a_chars.iter().enumerate() {
        for (j, cb) in b_chars.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            dp[i + 1][j + 1] = std::cmp::min(
                std::cmp::min(dp[i][j + 1] + 1, dp[i + 1][j] + 1),
                dp[i][j] + cost,
            );
        }
    }

    dp[a_chars.len()][b_chars.len()]
}

pub(crate) fn find_best_suggestion<'a>(target: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let mut best: Option<(&'a str, usize)> = None;
    for cand in candidates {
        let dist = levenshtein_distance(target, cand);
        let max_allowed = if target.len() <= 3 { 1 } else { 2 };
        if dist <= max_allowed {
            if let Some((_, best_dist)) = best {
                if dist < best_dist {
                    best = Some((cand, dist));
                }
            } else {
                best = Some((cand, dist));
            }
        }
    }
    best.map(|(s, _)| s.to_string())
}

/// Cada binding rastrea su tipo Y si se declaró `mut` -- lo segundo es lo
/// que `check_block` consulta al validar un `assign_stmt` (GRAMMAR.md §2.3).
#[derive(Clone)]
struct Binding {
    ty: Type,
    mutable: bool,
}

fn immutable(ty: Type) -> Binding {
    Binding { ty, mutable: false }
}

type Env = HashMap<String, Binding>;

/// Todos los nombres que un patrón ligaría, recursivamente -- usado por
/// `bind_pattern` solo para RECHAZAR un `Pattern::Or` cuyas alternativas
/// intenten bindear algo (ver su doc en ast.rs), no para resolver tipos.
fn pattern_bindings(pattern: &Pattern) -> Vec<String> {
    match pattern {
        Pattern::Bind(name) => vec![name.clone()],
        Pattern::Literal(_) => Vec::new(),
        Pattern::Variant { fields, .. } => fields
            .iter()
            .flatten()
            .flat_map(|fp| pattern_bindings(&fp.pattern))
            .collect(),
        Pattern::Or(subs) => subs.iter().flat_map(pattern_bindings).collect(),
        // A diferencia de `Literal` (que no liga nada), `Type` SÍ liga un
        // nombre -- devolver `Vec::new()` acá dejaría pasar en silencio un
        // `i: Int | s: String` dentro de un Or (que debería rechazarse
        // igual que cualquier otro binding dentro de un Or, ver el doc de
        // `Pattern::Or` en ast.rs).
        Pattern::Type(name, _) => vec![name.clone()],
    }
}

/// ¿Hay algún `Stmt::Return` alcanzable desde este bloque? Usado por
/// `synth_block` (GRAMMAR.md §3.10) para rechazar `return` de entrada en
/// vez de heredar el bug preexistente de `check_block` (mismo `expected`
/// para la cola y para un `return` anidado dentro de un if/match en
/// posición de sentencia), y por `check_stmt` (GRAMMAR.md §3.15) para
/// rechazar `return` dentro de un cuerpo de `while` de entrada. `return` es
/// siempre una SENTENCIA (ast.rs) -- nunca aparece anidado dentro de una
/// expresión -- así que la única forma de que esté "escondido" respecto de
/// este bloque es a través de un `if`/`match`/`while` cuyo cuerpo es, a su
/// vez, otro `Block`; por eso alcanza con recursar exactamente en esos tres
/// casos.
fn block_has_return(block: &Block) -> bool {
    block.stmts.iter().any(|s| match &s.node {
        Stmt::Return(_) => true,
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => expr_has_return(&value.node),
        Stmt::Expr(e) => expr_has_return(&e.node),
        // Tercera forma (además de if/match, ver expr_has_return) de
        // "esconder" un return de este bloque -- mismo tratamiento
        // (GRAMMAR.md §3.15).
        Stmt::While { cond, body } => expr_has_return(&cond.node) || block_has_return(body),
    }) || block.tail.as_deref().is_some_and(|e| expr_has_return(&e.node))
}

fn expr_has_return(e: &Expr) -> bool {
    match e {
        Expr::If { cond, then_block, else_block } => {
            expr_has_return(&cond.node) || block_has_return(then_block) || block_has_return(else_block)
        }
        Expr::Match { scrutinee, arms } => {
            expr_has_return(&scrutinee.node)
                || arms.iter().any(|arm| match &arm.body {
                    MatchArmBody::Expr(e) => expr_has_return(&e.node),
                    MatchArmBody::Block(b) => block_has_return(b),
                })
        }
        // Un closure anidado tiene su PROPIO contexto de retorno,
        // chequeado aparte cuando a él le toque (synth_expr/check_expr
        // sobre ESE Expr::Closure) -- su `return` no es asunto del
        // synth_block que lo contiene.
        Expr::Closure { .. } => false,
        // Todo lo demás no puede contener un `return` sintácticamente --
        // `return` es una sentencia, nunca anidada dentro de una expresión.
        _ => false,
    }
}

/// ¿`ty` es, o contiene recursivamente (en un campo, elemento, miembro de
/// unión, etc.), un `Type::Function`? Usado para rechazar `==`/`!=` sobre
/// closures (`synth_binary`, GRAMMAR.md §3.10). Motivo real, no teórico: un
/// closure recursivo armado reasignando un `mut` (`let mut f = |x|{x}; f =
/// |x|{ ... f(x-1) ... };`) captura, en runtime, un `Env` que termina
/// conteniendo una referencia a sí mismo (`Rc` cíclico) -- comparar o
/// debug-imprimir un valor así recursaría para siempre. Rechazarlo acá, en
/// tiempo de chequeo, es más barato y más claro que confiar solo en que
/// `Value` nunca se compare/imprima por accidente en runtime.
fn type_contains_function(ty: &Type) -> bool {
    match ty {
        Type::Function(..) => true,
        Type::Optional(inner) | Type::List(inner) | Type::PatchOf(inner) | Type::DbCollection(inner) | Type::DbQuery(inner) => {
            type_contains_function(inner)
        }
        Type::Tuple(items) | Type::Union(items) => items.iter().any(type_contains_function),
        Type::ResultOf(a, b) | Type::MapOf(a, b) => type_contains_function(a) || type_contains_function(b),
        Type::Struct { fields, .. } => fields.iter().any(|f| type_contains_function(&f.ty)),
        _ => false,
    }
}

/// El tipo que produce LEER un campo (`v.campo`). Para un campo declarado
/// `x?: T` -- clave que puede estar AUSENTE (GRAMMAR.md §3.4) -- eso es
/// `T?`, no `T`: leerlo puede no dar nada, y el lenguaje ya tiene una forma
/// de expresar eso.
///
/// Sin esto (el bug que había hasta la auditoría), `o.note` sobre un
/// `note?: String` sintetizaba `String` a secas -- así que un rpc podía
/// declarar `-> String`, devolver `o.note`, tipar perfecto, y después
/// fallar en runtime con "no existe el campo 'note'" al recibir un objeto
/// SIN esa clave (que es exactamente lo que `x?: T` permite). También hacía
/// pasar aritmética como `o.note + 1`. Nótese el contraste con `x: T?`
/// (clave siempre presente, valor nullable), que ya se rechazaba bien:
/// el bug era solo para la opcionalidad de CLAVE.
fn field_access_ty(f: &FieldType) -> Type {
    if f.optional {
        Type::Optional(Box::new(f.ty.clone()))
    } else {
        f.ty.clone()
    }
}

/// Recorre `ty` rechazando lo que no puede viajar como JSON. `top_level_ret`
/// solo habilita `Void` en la posición donde sí tiene sentido: el retorno
/// entero de un rpc que no devuelve nada.
fn check_wire_safe(ty: &Type, position: &str, top_level_ret: bool) -> Result<(), CheckError> {
    match ty {
        Type::Function(..) => Err(err(format!(
            "{position} incluye un tipo función, que no puede viajar por la red (GRAMMAR.md §4) -- \
             una función solo existe dentro del backend"
        ))),
        Type::Void if !top_level_ret => Err(err(format!(
            "{position} usa 'Void', que solo es válido como el retorno completo de un rpc (GRAMMAR.md §4)"
        ))),
        Type::Void => Ok(()),
        Type::Optional(inner) | Type::List(inner) | Type::PatchOf(inner) => {
            check_wire_safe(inner, position, false)
        }
        Type::Tuple(items) | Type::Union(items) => {
            items.iter().try_for_each(|t| check_wire_safe(t, position, false))
        }
        Type::ResultOf(a, b) | Type::MapOf(a, b) => {
            check_wire_safe(a, position, false)?;
            check_wire_safe(b, position, false)
        }
        Type::Struct { fields, .. } => fields.iter().try_for_each(|f| check_wire_safe(&f.ty, position, false)),
        Type::Generic(_, args) => args.iter().try_for_each(|t| check_wire_safe(t, position, false)),
        _ => Ok(()),
    }
}

/// Forma de literal permitida para el valor de un `const` (GRAMMAR.md §2.1) --
/// misma lista de casos que `ts_emit.rs::render_const_value`, pero acá solo
/// para VALIDAR la forma (no para renderizar), así que corre en `check_const`
/// y por lo tanto también en `linkc serve`, no solo en `linkc build`. Ver el
/// porqué completo en el doc-comment de `check_const`.
fn is_const_literal_shape(e: &Expr) -> bool {
    match e {
        Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Bool(_) | Expr::Null => true,
        Expr::ArrayLit(items) | Expr::TupleLit(items) => items.iter().all(|it| is_const_literal_shape(&it.node)),
        Expr::StructLit { fields, .. } => fields.iter().all(|(_, fe)| is_const_literal_shape(&fe.node)),
        _ => false,
    }
}

/// ¿Se puede PROBAR que dos miembros de una unión son distinguibles en
/// runtime? "No" es la respuesta segura cuando el análisis no puede probar
/// que sí (GRAMMAR.md §3.9, `check_exhaustive_union`) -- falla cerrado, no
/// asume que está bien.
///
/// Un chequeo ingenuo de `is_subtype` mutuo entre los dos NO alcanza:
/// `{x:Int,y:Int}` y `{x:Int,z:Int}` no son subtipo mutuo entre sí, pero un
/// TERCER tipo más ancho (`{x:Int,y:Int,z:Int}`, construible por cualquier
/// usuario vía subtipado estructural de ancho) satisface los campos
/// requeridos de los DOS a la vez -- un valor de ese tercer tipo sería
/// ambiguo para cualquiera de las dos reglas que solo miran nombres de
/// campo. La condición real: existe al menos un campo REQUERIDO por ambos
/// cuyos tipos declarados tengan discriminantes de `Value` (runtime/mod.rs)
/// mutuamente excluyentes -- un valor real solo puede tener UNA forma
/// concreta en ese campo (nunca ambas a la vez), así que ESE campo sí
/// distingue de forma confiable, sin importar qué tan ancho sea el valor
/// real que llegue. `value_matches_type` (runtime/mod.rs) chequea
/// exactamente esto -- el tipo REAL del valor en cada campo requerido, no
/// solo su presencia -- para que este argumento de solidez se sostenga.
fn union_members_are_distinguishable(a: &Type, b: &Type) -> bool {
    match (a, b) {
        (Type::Struct { fields: fa, .. }, Type::Struct { fields: fb, .. }) => fa.iter().any(|field_a| {
            !field_a.optional
                && fb.iter().any(|field_b| {
                    !field_b.optional && field_a.name == field_b.name && shallow_tag_conflict(&field_a.ty, &field_b.ty)
                })
        }),
        _ => shallow_tag_conflict(a, b),
    }
}

/// ¿`a` y `b` tienen discriminantes de `Value` mutuamente excluyentes? "No"
/// (el lado seguro) para `Dynamic` emparejado con cualquier cosa (acepta
/// cualquier forma en ambas direcciones, `is_subtype`), dos `List` (una
/// lista vacía matchea cualquiera de los dos) y dos `Optional` (`null`
/// matchea ambos) -- ninguno de estos tres tiene un discriminante de
/// runtime que los distinga de forma confiable. Dos `Struct` siempre "no
/// conflicto" ACÁ (nivel de campo, chequeo superficial no recursivo): la
/// comparación real entre dos structs vive en
/// `union_members_are_distinguishable`, que mira sus campos compartidos,
/// no acá.
fn shallow_tag_conflict(a: &Type, b: &Type) -> bool {
    use Type::*;
    match (a, b) {
        (Dynamic, _) | (_, Dynamic) => false,
        (List(_), List(_)) => false,
        (Optional(_), _) | (_, Optional(_)) => false,
        (Struct { .. }, Struct { .. }) => false,
        (Enum(na), Enum(nb)) => na != nb,
        _ if a == b => false,
        _ => true,
    }
}

pub struct Checker {
    pub(crate) types: HashMap<String, TypeDecl>,
    /// GRAMMAR.md §3.232: nombre de `type` -> sus campos `@hidden`. Solo
    /// los types que tienen alguno; consultado por el runtime al serializar
    /// un resultado (`strip_hidden_json`) y por el codegen de validadores.
    pub(crate) hidden_fields: HashMap<String, HashSet<String>>,
    /// GRAMMAR.md §3.234: los modelos de `ai { }`, en orden de declaración
    /// (todos los bloques, todos los archivos). `check_program` valida
    /// alias únicos y specs no vacías; el runtime (`serve`) y `doctor` los
    /// resuelven a un GGUF real.
    pub(crate) ai_models: Vec<AiModel>,
    pub(crate) enums: HashMap<String, EnumDecl>,
    fns: HashMap<String, (Vec<Type>, Type)>,
    pub(crate) services: HashMap<String, HashMap<String, (Vec<Type>, Type)>>,
    pub(crate) service_decls: HashMap<String, ServiceDecl>,
    /// Nombre de colección -> tipo de elemento ya resuelto, desde `db {
    /// ... }` (GRAMMAR.md §2.1, DbDecl). Vacío si el programa no declara
    /// ninguna `db` -- en ese caso `db` sigue existiendo como identificador
    /// (Type::Db), simplemente sin ninguna colección real.
    db_collections: HashMap<String, Type>,
    /// `const X: T = v` de nivel superior. Se guarda la declaración ENTERA
    /// (no solo el tipo) porque el runtime también la necesita: es el único
    /// lugar donde vive el valor. Hasta la auditoría, un `const` se
    /// declaraba, se chequeaba y se emitía a `client.ts`, pero era
    /// inusable desde el propio lenguaje -- `MAX` dentro de un rpc daba
    /// "variable no declarada". Una feature a medias.
    pub(crate) consts: HashMap<String, ConstDecl>,
    /// Hover de expresión arbitraria (GRAMMAR.md §3.24, LSP Nivel 3 ronda
    /// 2/3): offset a buscar, `None` en cualquier chequeo NORMAL (`build`/
    /// `serve`/diagnósticos del LSP) -- ver `hover_type_at`, el único
    /// lugar que lo pone en `Some`. Interior mutability (`RefCell`, no
    /// `&mut self`) porque el resto de `Checker` chequea con `&self` de
    /// punta a punta -- agregar `&mut self` acá para un solo caso de uso
    /// hubiera obligado a tocar los ~40 sitios que ya llaman
    /// `check_expr`/`synth_expr` con `&self`.
    hover_target: Option<usize>,
    /// `(ancho_del_span, tipo)` del match más ESPECÍFICO visto hasta
    /// ahora -- no "el último visto": un `Expr` padre siempre tiene un
    /// span que CONTIENE al de sus hijos, y el padre se re-procesa
    /// DESPUÉS de que sus hijos ya retornaron (la recursión entra a los
    /// hijos ANTES de que el padre calcule su propio resultado), así que
    /// "última escritura gana" terminaría quedándose con el nodo más
    /// EXTERNO, no el más específico bajo el cursor. `probe_hover` compara
    /// anchos en vez de simplemente sobreescribir.
    /// `std::sync::Mutex`, no `RefCell` -- mismo motivo que
    /// `in_stream_body`/`in_transaction` (Pilar 1 del roadmap de
    /// concurrencia, 26/08/2026): `Db` necesita `Sync` para compartirse
    /// entre hilos de request, y `RefCell` no lo es bajo ninguna
    /// circunstancia -- aunque este campo específico solo se usa de
    /// verdad durante `linkc lsp` (una sesión de un solo cliente, nunca la
    /// copia de `Checker` que `Db` guarda para runtime). `std::sync`, no
    /// `parking_lot` -- este módulo compila también a `wasm32-unknown-
    /// unknown` sin el feature `runtime` (que es lo único detrás de lo que
    /// vive `parking_lot`, ver Cargo.toml); `std::sync::Mutex` no necesita
    /// ningún feature.
    hover_result: std::sync::Mutex<Option<(usize, Type)>>,
    /// `true` mientras `check_rpc` chequea el cuerpo de un `stream` (nunca
    /// un `rpc` normal) -- mismo motivo de interior mutability que
    /// `hover_result`: el resto de `Checker` chequea con `&self`. Lo único
    /// que lo consulta es `(Type::Response, "setStatus")`: el status de una
    /// conexión SSE es fijo para toda su duración (GRAMMAR.md §3.46), así
    /// que llamarlo desde un `stream` pasaba desapercibido en v0 -- un
    /// no-op silencioso que un desarrollador solo descubría en producción.
    /// `AtomicBool`, no `Cell<bool>` -- Pilar 1 del roadmap de concurrencia
    /// (26/08/2026): `Db` guarda un `Checker` propio para resolver tipos en
    /// runtime (ver su doc), y `Db` necesita ser `Sync` para compartirse
    /// entre hilos de request (`runtime/server.rs`) -- `Cell` no es `Sync`
    /// bajo ninguna circunstancia. En la práctica este campo NUNCA se muta
    /// en esa copia (solo se usa durante el CHEQUEO de tipos, una pasada
    /// sincrónica de `linkc build`/`linkc test`, nunca durante la
    /// interpretación); `Ordering::Relaxed` alcanza porque no hay ninguna
    /// otra escritura con la que coordinarse -- es un flag de anidamiento
    /// LOCAL a una sola pasada de chequeo, no un dato compartido de verdad.
    in_stream_body: std::sync::atomic::AtomicBool,
    /// GRAMMAR.md §3.154: `true` mientras se chequea el CUERPO de un
    /// `transaction { ... }` -- mismo mecanismo que `in_stream_body`, para
    /// rechazar anidar una `transaction` dentro de otra en compilación
    /// (una sola transacción SQL por vez; savepoints/anidamiento real
    /// quedan fuera de v0).
    in_transaction: std::sync::atomic::AtomicBool,
}

/// `enum PdfBlock { Text { content: String, bold: Bool, size: Int }, Table {
/// headers: String[], rows: String[][] } }` -- construido a mano (no hay
/// texto fuente que parsear) porque es un ADT reservado por el compilador,
/// pre-registrado en `checker.enums` por `build_symbols` (ver el comentario
/// ahí). `Span::new(0, 0, 0, 0)` en todos lados: ningún nodo de este
/// `EnumDecl` corresponde a una posición real del archivo del usuario.
pub(crate) fn pdf_block_enum_decl() -> EnumDecl {
    let dummy = Span::new(0, 0, 0, 0);
    let named = |name: &str| TypeExpr::Named(name.to_string(), vec![], dummy);
    let field = |name: &str, ty: TypeExpr| Field {
        name: name.to_string(),
        optional: false,
        ty,
        name_span: dummy,
        annotations: vec![],
        default: None,
    };
    EnumDecl {
        name: "PdfBlock".to_string(),
        type_params: vec![],
        variants: vec![
            Variant {
                name: "Text".to_string(),
                fields: Some(vec![
                    field("content", named("String")),
                    field("bold", named("Bool")),
                    field("size", named("Int")),
                ]),
            },
            Variant {
                name: "Table".to_string(),
                fields: Some(vec![
                    field("headers", TypeExpr::List(Box::new(named("String")))),
                    field("rows", TypeExpr::List(Box::new(TypeExpr::List(Box::new(named("String")))))),
                ]),
            },
        ],
        span: dummy,
    }
}

/// `enum ExcelCell { Text { value: String }, Number { value: Decimal },
/// Date { value: Timestamp }, Bool { value: Bool }, Empty }` -- mismo
/// mecanismo que `pdf_block_enum_decl` (ADT reservado por el compilador,
/// pre-registrado en `checker.enums`). `Number` carga `Decimal`, no
/// `Float` -- este lenguaje ya trata `Decimal` como el tipo de dinero
/// (GRAMMAR.md §3.184), y el caso real (conciliación bancaria) es
/// justamente donde la precisión importa; la conversión a/desde el `f64`
/// crudo que `.xlsx` almacena internamente pasa por los bordes de
/// `runtime/excel.rs`, nunca se expone un `Float` acá. `Empty` es una
/// variante unitaria (`fields: None`), igual que `Role.Admin` -- se
/// construye igual, `ExcelCell.Empty {}` con llaves (CLAUDE.md/AGENTS.md).
pub(crate) fn excel_cell_enum_decl() -> EnumDecl {
    let dummy = Span::new(0, 0, 0, 0);
    let named = |name: &str| TypeExpr::Named(name.to_string(), vec![], dummy);
    let field = |name: &str, ty: TypeExpr| Field {
        name: name.to_string(),
        optional: false,
        ty,
        name_span: dummy,
        annotations: vec![],
        default: None,
    };
    EnumDecl {
        name: "ExcelCell".to_string(),
        type_params: vec![],
        variants: vec![
            Variant { name: "Text".to_string(), fields: Some(vec![field("value", named("String"))]) },
            Variant { name: "Number".to_string(), fields: Some(vec![field("value", named("Decimal"))]) },
            Variant { name: "Date".to_string(), fields: Some(vec![field("value", named("Timestamp"))]) },
            Variant { name: "Bool".to_string(), fields: Some(vec![field("value", named("Bool"))]) },
            Variant { name: "Empty".to_string(), fields: None },
        ],
        span: dummy,
    }
}

/// `type ExcelSheet = { name: String, headers: String[], rows: ExcelCell[][] }`
/// -- a diferencia de `ExcelCell` (un enum, NOMINAL en este lenguaje,
/// GRAMMAR.md §3.2), `ExcelSheet` es un STRUCT, y los structs subtipan
/// ESTRUCTURALMENTE -- así que registrarlo acá es una mejora de
/// ERGONOMÍA (nombra el tipo en errores, permite a un usuario escribir
/// `sheets: ExcelSheet[]` sin repetir la forma completa), no un requisito
/// de corrección: cualquier struct de OTRO nombre con la misma forma ya
/// tipa igual de bien contra `excel.build`/`excel.parse` sin este
/// pre-registro, por subtipado estructural puro.
pub(crate) fn excel_sheet_type_decl() -> TypeDecl {
    let dummy = Span::new(0, 0, 0, 0);
    let named = |name: &str| TypeExpr::Named(name.to_string(), vec![], dummy);
    let field = |name: &str, ty: TypeExpr| Field {
        name: name.to_string(),
        optional: false,
        ty,
        name_span: dummy,
        annotations: vec![],
        default: None,
    };
    TypeDecl {
        name: "ExcelSheet".to_string(),
        type_params: vec![],
        ty: TypeExpr::Struct(vec![
            field("name", named("String")),
            field("headers", TypeExpr::List(Box::new(named("String")))),
            field("rows", TypeExpr::List(Box::new(TypeExpr::List(Box::new(named("ExcelCell")))))),
        ]),
        annotations: vec![],
        span: dummy,
    }
}

/// `Type::Struct` resuelto para la forma de `ExcelSheet` -- usado en la
/// firma de `excel.build`/`excel.parse` en vez de resolver `TypeExpr` a
/// mano dos veces. Mismo `name: Some("ExcelSheet")` que resolvería
/// `resolve_named_type_subst` si un usuario escribiera el tipo por nombre
/// -- así que un struct nombrado explícitamente O uno estructuralmente
/// idéntico con otro nombre tipan igual acá (§3.2).
/// GRAMMAR.md §3.235: `AiMessage = { role: String, content: String }`, el
/// turno de `ai.chat`. Pre-sembrado en `checker.types` como `ExcelSheet`
/// (para poder escribir `AiMessage { ... }`), y estructural como argumento
/// (`ai_message_struct_type`): cualquier struct con esos dos campos entra.
pub(crate) fn ai_message_type_decl() -> TypeDecl {
    let dummy = Span::new(0, 0, 0, 0);
    let named = |name: &str| TypeExpr::Named(name.to_string(), vec![], dummy);
    let field = |name: &str, ty: TypeExpr| Field {
        name: name.to_string(),
        optional: false,
        ty,
        name_span: dummy,
        annotations: vec![],
        default: None,
    };
    TypeDecl {
        name: "AiMessage".to_string(),
        type_params: vec![],
        ty: TypeExpr::Struct(vec![field("role", named("String")), field("content", named("String"))]),
        span: dummy,
        annotations: vec![],
    }
}

/// GRAMMAR.md §3.236: `AiToken = { token: String, done: Bool }`, el elemento
/// de `ai.stream`. Pre-sembrado y estructural como `AiMessage`.
pub(crate) fn ai_token_type_decl() -> TypeDecl {
    let dummy = Span::new(0, 0, 0, 0);
    let named = |name: &str| TypeExpr::Named(name.to_string(), vec![], dummy);
    let field = |name: &str, ty: TypeExpr| Field {
        name: name.to_string(),
        optional: false,
        ty,
        name_span: dummy,
        annotations: vec![],
        default: None,
    };
    TypeDecl {
        name: "AiToken".to_string(),
        type_params: vec![],
        ty: TypeExpr::Struct(vec![field("token", named("String")), field("done", named("Bool"))]),
        span: dummy,
        annotations: vec![],
    }
}

fn ai_token_struct_type() -> Type {
    Type::Struct {
        name: Some("AiToken".to_string()),
        fields: vec![
            FieldType { name: "token".to_string(), optional: false, ty: Type::String },
            FieldType { name: "done".to_string(), optional: false, ty: Type::Bool },
        ],
    }
}

fn ai_message_struct_type() -> Type {
    Type::Struct {
        name: Some("AiMessage".to_string()),
        fields: vec![
            FieldType { name: "role".to_string(), optional: false, ty: Type::String },
            FieldType { name: "content".to_string(), optional: false, ty: Type::String },
        ],
    }
}

fn excel_sheet_struct_type() -> Type {
    Type::Struct {
        name: Some("ExcelSheet".to_string()),
        fields: vec![
            FieldType { name: "name".to_string(), optional: false, ty: Type::String },
            FieldType { name: "headers".to_string(), optional: false, ty: Type::List(Box::new(Type::String)) },
            FieldType {
                name: "rows".to_string(),
                optional: false,
                ty: Type::List(Box::new(Type::List(Box::new(Type::Enum("ExcelCell".to_string()))))),
            },
        ],
    }
}

impl Checker {
    /// Construye las tablas de símbolos (types/enums/fns) sin chequear los
    /// cuerpos de fn/rpc. Lo usa tanto `check_program` como el emisor de
    /// contrato (codegen/ts_emit.rs), que necesita `resolve_type` pero no
    /// quiere duplicar la lógica de resolución de nombres.
    pub fn build_symbols(program: &Program) -> (Self, Vec<CheckError>) {
        let mut checker = Checker {
            types: HashMap::new(),
            hidden_fields: HashMap::new(),
            ai_models: Vec::new(),
            enums: HashMap::new(),
            fns: HashMap::new(),
            services: HashMap::new(),
            service_decls: HashMap::new(),
            db_collections: HashMap::new(),
            consts: HashMap::new(),
            hover_target: None,
            hover_result: std::sync::Mutex::new(None),
            in_stream_body: std::sync::atomic::AtomicBool::new(false),
            in_transaction: std::sync::atomic::AtomicBool::new(false),
        };
        // `PdfBlock` (GRAMMAR.md §3.201) es un ADT reservado por el
        // compilador, no un enum que el usuario declare -- su forma la dicta
        // lo que `pdf.build` sabe renderizar. Pre-registrarlo ACÁ, antes del
        // loop de abajo, reusa el mecanismo genérico de ADT tal cual (mismo
        // camino que cualquier enum de usuario: `resolve_named_type_subst`
        // ya resuelve cualquier nombre presente en `checker.enums`,
        // `synth_struct_lit` ya tipa `Enum.Variante { ... }` contra
        // `checker.enums`) -- sin ESTO, no haría falta ningún caso especial
        // nuevo. Como bonus gratis, un `enum PdfBlock { ... }` de usuario
        // cae en la rama de "enum duplicado" de ese mismo loop (mismo
        // mensaje de error que colisionar con cualquier otro enum).
        checker.enums.insert("PdfBlock".to_string(), pdf_block_enum_decl());
        // GRAMMAR.md §3.202: mismo mecanismo que `PdfBlock` arriba, para
        // `ExcelCell` (ADT nominal). `ExcelSheet` es un `type` (struct),
        // no un `enum` -- se pre-registra en `checker.types` por ergonomía
        // (ver el comentario de `excel_sheet_type_decl`), no porque haga
        // falta para que el subtipado estructural funcione.
        checker.enums.insert("ExcelCell".to_string(), excel_cell_enum_decl());
        checker.types.insert("ExcelSheet".to_string(), excel_sheet_type_decl());
        // GRAMMAR.md §3.235: `AiMessage`, mismo criterio que `ExcelSheet`.
        checker.types.insert("AiMessage".to_string(), ai_message_type_decl());
        checker.types.insert("AiToken".to_string(), ai_token_type_decl());
        let mut errors = Vec::new();

        for item in &program.items {
            match item {
                // Duplicado detectado -- hallado al diseñar imports
                // multi-archivo (GRAMMAR.md §2.1): dos `type`/`enum` con el
                // mismo nombre ganaban por orden de inserción, en silencio.
                // Con un solo archivo ya era un gap real; con imports,
                // colisiones entre archivos se vuelven mucho más probables.
                Item::Type(t) if checker.types.contains_key(&t.name) => {
                    // Estampado con la SEGUNDA declaración (la que se está
                    // procesando cuando se detecta el choque), no la
                    // primera -- límite de v0 documentado: no hay forma de
                    // un "note: previous definition here" con un solo Span
                    // por error.
                    errors.push(err(format!("'{}' ya está declarado (type duplicado)", t.name)).with_span(t.span));
                }
                Item::Ai(ai) => checker.ai_models.extend(ai.models.iter().cloned()),
                Item::Type(t) => {
                    if let TypeExpr::Struct(fields) = &t.ty {
                        let hidden: HashSet<String> = fields.iter().filter(|f| f.hidden()).map(|f| f.name.clone()).collect();
                        if !hidden.is_empty() {
                            checker.hidden_fields.insert(t.name.clone(), hidden);
                        }
                    }
                    checker.types.insert(t.name.clone(), t.clone());
                }
                Item::Enum(e) if checker.enums.contains_key(&e.name) => {
                    errors.push(err(format!("'{}' ya está declarado (enum duplicado)", e.name)).with_span(e.span));
                }
                Item::Enum(e) => {
                    // Una variante CON datos se emite en TypeScript como
                    // `{ type: "Variante"; ...campos }` (codegen/ts_emit.rs):
                    // el discriminante ocupa la clave `type`. Un campo de
                    // payload con ese mismo nombre produciria un identificador
                    // duplicado en el contrato generado, asi que se rechaza
                    // aqui y no mas tarde, en un archivo .d.ts que no compila.
                    for variant in &e.variants {
                        if let Some(fields) = &variant.fields {
                            if let Some(f) = fields.iter().find(|f| f.name == "type") {
                                errors.push(
                                    err(format!(
                                        "'type' no puede ser el nombre de un campo de la variante '{}::{}': esa clave la ocupa el discriminante del union generado. Renombralo (p. ej. 'kind').",
                                        e.name, variant.name
                                    ))
                                    .with_span(f.name_span),
                                );
                            }
                        }
                    }
                    checker.enums.insert(e.name.clone(), e.clone());
                }
                _ => {}
            }
        }

        for item in &program.items {
            if let Item::Fn(f) = item {
                if checker.fns.contains_key(&f.name) {
                    errors.push(err(format!("'{}' ya está declarado (fn duplicada)", f.name)).with_span(f.span));
                    continue;
                }
                match checker.resolve_fn_signature(f) {
                    Ok(sig) => {
                        checker.fns.insert(f.name.clone(), sig);
                    }
                    Err(e) => errors.push(e.with_span(f.span)),
                }
            }
        }

        for item in &program.items {
            if let Item::Const(c) = item {
                if checker.consts.contains_key(&c.name) {
                    errors.push(err(format!("'{}' ya está declarado (const duplicado)", c.name)).with_span(c.span));
                    continue;
                }
                checker.consts.insert(c.name.clone(), c.clone());
            }
        }

        // Item::Db -- DESPUÉS de types/enums (resolver "User[]" necesita
        // poder encontrar "User" ya insertado). CUALQUIER cantidad de
        // `db { ... }` en el Program ya fusionado (imports lo aplanan todo a
        // un solo Program, modules.rs) se fusiona en un solo namespace de
        // colecciones -- GRAMMAR.md §3.172, cierra el "qué queda REALMENTE
        // abierto del Pilar 3" que §3.161 había dejado explícito ("permitir
        // varios `db {}` es una decisión de diseño con su propio peso").
        // Antes de esto, un SEGUNDO `db { ... }` en el cierre transitivo era
        // un error duro sin importar sus nombres -- el patrón real que
        // dependía de esto (`schema.link` central con el `db {}`, importado
        // por módulos de servicio) sigue funcionando exactamente igual;
        // ahora TAMBIÉN funciona que cada módulo sea dueño de sus propias
        // colecciones. Lo único que sigue siendo un error duro es un nombre
        // de colección REPETIDO -- sin importar si las dos apariciones caen
        // en el mismo `db { ... }` o en dos de archivos distintos, mismo
        // criterio que ya aplica a `type`/`enum`/`fn`/`const` duplicados
        // (`build_symbols`, arriba). Antes de esta ronda, un nombre repetido
        // DENTRO de un solo bloque se perdía en silencio (el `insert` de más
        // abajo simplemente pisaba la primera aparición sin ningún aviso) --
        // un gap preexistente cerrado de paso, no solo el caso nuevo.
        // GRAMMAR.md §3.234: `ai { }` -- alias únicos en todos los bloques
        // y archivos (mismo criterio que las colecciones de `db`), spec no
        // vacía. Que el modelo EXISTA no se sabe acá: eso es del runtime
        // (`serve` se niega a arrancar) y de `linkc doctor`.
        let mut ai_alias_span: HashMap<String, Span> = HashMap::new();
        for item in &program.items {
            if let Item::Ai(ai) = item {
                for m in &ai.models {
                    if m.spec.trim().is_empty() {
                        errors.push(
                            err(format!(
                                "el modelo '{}' de 'ai' tiene una spec vacía -- se esperaba un nombre de Ollama (\"qwen2.5:0.5b\") o una ruta a un .gguf (GRAMMAR.md §3.234)",
                                m.alias
                            ))
                            .with_span(m.span),
                        );
                        continue;
                    }
                    if ai_alias_span.contains_key(&m.alias) {
                        errors.push(
                            err(format!(
                                "el modelo '{}' de 'ai' ya está declarado -- un alias no puede repetirse, ni dentro del mismo 'ai {{ ... }}' ni en otro distinto",
                                m.alias
                            ))
                            .with_span(m.span),
                        );
                        continue;
                    }
                    ai_alias_span.insert(m.alias.clone(), m.span);
                }
            }
        }

        let mut collection_span: HashMap<String, Span> = HashMap::new();
        for item in &program.items {
            if let Item::Db(db) = item {
                for coll in &db.collections {
                    // `coll` es un `Field` -- fuera de alcance para tener su
                    // propio span (ver ast.rs) -- así que el mejor span
                    // disponible es el de todo el `db { ... }` que lo
                    // contiene.
                    if collection_span.contains_key(&coll.name) {
                        errors.push(
                            err(format!(
                                "la colección '{}' ya está declarada -- un nombre de colección no puede repetirse, \
                                 ni dentro del mismo 'db {{ ... }}' ni en otro distinto",
                                coll.name
                            ))
                            .with_span(db.span),
                        );
                        continue;
                    }
                    collection_span.insert(coll.name.clone(), db.span);
                    match checker.resolve_type(&coll.ty) {
                        Ok(Type::List(element_ty)) => match checker.validate_db_element_type(&element_ty) {
                            Ok(()) => {
                                checker.db_collections.insert(coll.name.clone(), *element_ty);
                            }
                            Err(e) => errors.push(e.with_span(db.span)),
                        },
                        Ok(other) => errors.push(
                            err(format!(
                                "la colección '{}' de 'db' tiene que ser una lista de structs (T[]), se encontró {other}",
                                coll.name
                            ))
                            .with_span(db.span),
                        ),
                        Err(e) => errors.push(e.with_span(db.span)),
                    }
                }
            }
        }

        for item in &program.items {
            if let Item::Service(s) = item {
                let mut methods = HashMap::new();
                for m in &s.members {
                    let rpc = match m {
                        Member::Rpc(r) | Member::Stream(r) => r,
                    };
                    if let Ok(sig) = checker.resolve_rpc_signature(rpc) {
                        methods.insert(rpc.name.clone(), sig);
                    }
                }
                checker.services.insert(s.name.clone(), methods);
                checker.service_decls.insert(s.name.clone(), s.clone());
            }
        }

        (checker, errors)
    }

    /// Nombre de colección -> tipo de elemento ya resuelto. Puramente
    /// aditivo (cero cambio de comportamiento de chequeo) -- lo usa
    /// `runtime::db::Db` para derivar el schema SQL de cada colección
    /// (GRAMMAR.md §3.17), sin duplicar la resolución de tipos que este
    /// `Checker` ya hizo al procesar `db { ... }`.
    /// GRAMMAR.md §3.234: `(alias, spec)` de cada modelo de `ai { }`.
    pub fn ai_models(&self) -> Vec<(String, String)> {
        self.ai_models.iter().map(|m| (m.alias.clone(), m.spec.clone())).collect()
    }

    pub(crate) fn db_collections(&self) -> &HashMap<String, Type> {
        &self.db_collections
    }

    /// Único punto de instrumentación de hover (GRAMMAR.md §3.24, LSP
    /// Nivel 3 ronda 2/3), llamado desde `synth_expr`/`check_expr` -- los
    /// dos wrappers públicos por los que pasa CUALQUIER expresión del
    /// programa, así que instrumentar acá alcanza para cubrir el árbol
    /// entero sin tocar cada uno de los ~15 `synth_*`/`check_*` internos.
    ///
    /// No-op inmediato si `hover_target` es `None` (cualquier chequeo
    /// NORMAL -- build/serve/diagnósticos del LSP) o si `span` no contiene
    /// el offset -- `compute` (que puede clonar un `Type`) ni se evalúa en
    /// ese caso.
    ///
    /// Cuando SÍ contiene el offset, solo reemplaza el resultado ya
    /// guardado si `span` es MÁS ANGOSTO que el mejor visto hasta ahora --
    /// no "el último visto" (ver el doc de `hover_result`). `compute`
    /// devolviendo `None` (el chequeo de ESTE nodo falló) no borra un
    /// resultado ya encontrado en un ancestro más externo -- best-effort:
    /// si el nodo más específico no tipa, mostrar el tipo del contexto
    /// que sí lo hizo es mejor que no mostrar nada.
    fn probe_hover(&self, span: Span, compute: impl FnOnce() -> Option<Type>) {
        let Some(target) = self.hover_target else { return };
        if !(span.start <= target && target < span.end) {
            return;
        }
        let width = span.end - span.start;
        let mut best = self.hover_result.lock().unwrap_or_else(|e| e.into_inner());
        let is_more_specific = match &*best {
            None => true,
            Some((best_width, _)) => width < *best_width,
        };
        if is_more_specific {
            if let Some(ty) = compute() {
                *best = Some((width, ty));
            }
        }
    }

    /// Toda colección de `db` necesita un campo `id: Int` o `id: Uuid`
    /// requerido -- es lo que hace posible `insert(x: Omit<T,"id">)` sin
    /// romper la forma completa de T (GRAMMAR.md §2.1): sin esta regla,
    /// `insert` exigiendo el struct COMPLETO habría rechazado el propio
    /// demo insignia, donde `NewUser` es deliberadamente un subconjunto
    /// de `User`. `id: Uuid` (GRAMMAR.md §3.177): la PK se genera del
    /// lado de la aplicación (mismo generador que `crypto.uuid()`) en
    /// vez de autoincremento -- pensado para adoptar una tabla existente
    /// con `id uuid`, el bloqueo real que impedía migrar iaacademy
    /// (GRAMMAR.md §3.176).
    fn validate_db_element_type(&self, element_ty: &Type) -> Result<(), CheckError> {
        let Type::Struct { fields, .. } = element_ty else {
            return Err(err(format!(
                "el tipo de elemento de una colección de 'db' tiene que ser un struct, se encontró {element_ty:?}"
            )));
        };
        let id_ok = fields.iter().any(|f| f.name == "id" && !f.optional && matches!(f.ty, Type::Int | Type::Uuid));
        if !id_ok {
            return Err(err(
                "toda colección de 'db' necesita un campo 'id: Int' o 'id: Uuid' requerido (no opcional, no nullable)",
            ));
        }
        Ok(())
    }

    /// El tipo REAL del campo `id` de una colección ya validada por
    /// `validate_db_element_type` -- `Int` (autoincremento) o `Uuid`
    /// (generado del lado de la aplicación, GRAMMAR.md §3.177).
    /// `find`/`applyPatch`/`delete`/`increment`/`pageAfter` tipan su
    /// argumento/cursor de id contra ESTE tipo en vez de un `Type::Int`
    /// fijo, para aceptar los dos casos con el mismo código.
    pub(crate) fn db_id_type(element_ty: &Type) -> Type {
        let Type::Struct { fields, .. } = element_ty else {
            unreachable!("validate_db_element_type ya garantizó que element_ty sea un struct");
        };
        fields
            .iter()
            .find(|f| f.name == "id")
            .map(|f| f.ty.clone())
            .expect("validate_db_element_type ya garantizó que 'id' exista")
    }

    /// Estampa el span de LA DECLARACIÓN en cada uno de los 5 puntos de
    /// entrada de abajo -- de último recurso: si el error ya viene con un
    /// span más preciso desde adentro (una sub-expresión que falló primero,
    /// vía `check_expr`/`synth_expr`), `with_span` no lo pisa (primer stamp
    /// gana). Esto solo importa para errores que se originan en la firma
    /// misma (ej. `resolve_type` sobre un tipo desconocido) y nunca pasan
    /// por ningún `Expr`.
    pub fn check_program(program: &Program) -> Result<(), Vec<CheckError>> {
        let (_, errors) = Self::check_program_full(program, &[]);
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Igual que `check_program`, pero con identidad de archivo por error
    /// (`CheckError.file`, ver `check_program_full`) -- lo que
    /// `main.rs::report_check_errors` necesita para renderizar un snippet
    /// real sobre un error de tipos que vino de un archivo IMPORTADO, no
    /// solo del archivo de entrada (antes de esta ronda, `touched.len() ==
    /// 1` era la única forma de saber que un snippet era seguro de
    /// mostrar). `main.rs` no puede llamar a `check_program_full`
    /// directamente porque es `pub(crate)` de la librería -- invisible
    /// desde el crate binario, aunque compartan paquete Cargo -- así que
    /// esta es la fachada pública equivalente, igual que `check_program`
    /// ya es la fachada pública de "no me importa el `Checker`".
    pub fn check_program_with_files(program: &Program, item_files: &[PathBuf]) -> Result<(), Vec<CheckError>> {
        let (_, errors) = Self::check_program_full(program, item_files);
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Igual que `check_program`, pero devuelve el `Checker` mismo en vez de
    /// descartarlo -- el protocolo LSP lo necesita para hover/completion/
    /// goto-def (`.types`/`.enums`/`.fns`/`.consts`, `resolve_type`) sin
    /// tener que volver a chequear el programa entero para cada request.
    /// `check_program` es simplemente el caso "no me importa el Checker
    /// NI la identidad de archivo", así que sus call sites de un solo
    /// archivo (tests que arman un `Program` a mano) no cambian.
    ///
    /// `item_files` es opcional en la práctica aunque no en el tipo: pasar
    /// `&[]` (como hace `check_program`) desactiva el stamping de archivo
    /// sin romper nada -- `item_files.get(i)` da `None` para cualquier
    /// índice, así que `CheckError.file` queda en `None` exactamente como
    /// antes de esta ronda. Cuando SÍ viene poblado (mismo largo y orden
    /// que `program.items`, ver `modules::load_program_with_overlay`),
    /// cada error sale con el archivo real de la declaración que lo
    /// originó, sin importar a qué profundidad del item se generó (un
    /// item nunca se parte entre dos archivos).
    pub(crate) fn check_program_full(program: &Program, item_files: &[PathBuf]) -> (Self, Vec<CheckError>) {
        let (checker, mut errors) = Self::build_symbols(program);
        let file_for = |index: usize| item_files.get(index).cloned();

        for (index, item) in program.items.iter().enumerate() {
            match item {
                Item::Fn(f) => {
                    if let Err(e) = checker.check_fn(f) {
                        let mut e = e.with_span(f.span);
                        if let Some(file) = file_for(index) {
                            e = e.with_file(file);
                        }
                        errors.push(e);
                    }
                }
                Item::Service(s) => {
                    for m in &s.members {
                        let (rpc, is_stream) = match m {
                            Member::Rpc(r) => (r, false),
                            Member::Stream(r) => (r, true),
                        };
                        if let Err(e) = checker.check_rpc(rpc, is_stream) {
                            let mut e = e.with_span(rpc.span);
                            if let Some(file) = file_for(index) {
                                e = e.with_file(file);
                            }
                            errors.push(e);
                        }
                        if let Err(e) = checker.check_rpc_crosses_the_wire(rpc) {
                            let mut e = e.with_span(rpc.span);
                            if let Some(file) = file_for(index) {
                                e = e.with_file(file);
                            }
                            errors.push(e);
                        }
                        if let Err(e) = checker.check_rpc_annotation(rpc, is_stream, s) {
                            let mut e = e.with_span(rpc.span);
                            if let Some(file) = file_for(index) {
                                e = e.with_file(file);
                            }
                            errors.push(e);
                        }
                    }
                }
                Item::Const(c) => {
                    if let Err(e) = checker.check_const(c) {
                        let mut e = e.with_span(c.span);
                        if let Some(file) = file_for(index) {
                            e = e.with_file(file);
                        }
                        errors.push(e);
                    }
                }
                Item::Test(t) => {
                    if let Err(e) = checker.check_test(t) {
                        let mut e = e.with_span(t.span);
                        if let Some(file) = file_for(index) {
                            e = e.with_file(file);
                        }
                        errors.push(e);
                    }
                }
                // `@validate(...)` (GRAMMAR.md §3.73) y `= default`
                // (§3.74): los dos necesitan resolver el tipo del campo
                // (el primero para exigir `String`/`String?`, el segundo
                // para tipar la expresión contra ese tipo), así que viven
                // en `check_program_full` (con símbolos ya poblados por
                // `build_symbols`) y no en el parser, a diferencia de la
                // validación "motivo no vacío" de `@deprecated`, que es
                // puramente sintáctica.
                Item::Type(t) => {
                    // `check_type_annotations` corre SIEMPRE (incluso si
                    // `t.ty` no es un struct -- ahí mismo es donde rechaza
                    // '@unique' sobre un alias/unión), a diferencia del
                    // resto de los `check_field_*` de abajo, que solo tienen
                    // sentido sobre la forma struct.
                    let mut item_errors: Vec<CheckError> = checker.check_type_annotations(t);
                    if let TypeExpr::Struct(fields) = &t.ty {
                        item_errors.extend(
                            checker
                                .check_field_validators(fields, &t.type_params)
                                .into_iter()
                                .chain(checker.check_field_defaults(fields, &t.type_params))
                                .chain(checker.check_field_auto_update(fields, &t.type_params))
                                .chain(checker.check_field_soft_delete(fields, &t.type_params))
                                .chain(checker.check_field_encrypted(fields, &t.type_params))
                                .chain(checker.check_field_hidden(fields))
                                .chain(checker.check_field_checks(fields, &t.type_params)),
                        );
                    }
                    for e in item_errors {
                        let mut e = e;
                        if let Some(file) = file_for(index) {
                            e = e.with_file(file);
                        }
                        errors.push(e);
                    }
                }
                Item::Enum(en) => {
                    for variant in &en.variants {
                        if let Some(fields) = &variant.fields {
                            for e in checker
                                .check_field_validators(fields, &en.type_params)
                                .into_iter()
                                .chain(checker.check_field_defaults(fields, &en.type_params))
                                .chain(checker.check_field_auto_update(fields, &en.type_params))
                                .chain(checker.check_field_soft_delete(fields, &en.type_params))
                                .chain(checker.check_field_encrypted(fields, &en.type_params))
                                .chain(checker.check_field_checks(fields, &en.type_params))
                            {
                                let mut e = e;
                                if let Some(file) = file_for(index) {
                                    e = e.with_file(file);
                                }
                                errors.push(e);
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        // Aparte del loop de arriba: un conflicto de `@route` es entre DOS
        // rpc, no un error DE un rpc individual -- necesita haber visto el
        // programa entero antes de poder decidir nada. Sin span/file por
        // rpc individual (el mensaje ya nombra a los dos), así que estos
        // errores no pasan por `file_for`.
        errors.extend(checker.check_route_conflicts(program));

        (checker, errors)
    }

    /// Hover de expresión arbitraria (GRAMMAR.md §3.24, LSP Nivel 3 ronda
    /// 2/3): el tipo de la expresión MÁS ESPECÍFICA que contiene `offset`,
    /// dentro del `fn`/`rpc`/`stream` cuyo BODY lo contiene -- `None` si
    /// `offset` no cae dentro de ningún body, o si el chequeo de ese body
    /// para antes de llegar a la expresión buscada (ver el límite abajo).
    ///
    /// Reusa `check_fn`/`check_rpc` TAL CUAL -- no reconstruye bindings de
    /// parámetros ni reglas de scoping por su cuenta (evitaría una segunda
    /// fuente de verdad para reglas que ya viven ahí); lo único nuevo es
    /// `hover_target`/`probe_hover`, que esas mismas funciones ya activan
    /// indirectamente a través de cada llamada a `synth_expr`/`check_expr`
    /// dentro de su recorrido normal. `item.span`/`rpc.span` (firma
    /// solamente, GRAMMAR.md §3.19) no sirven para esta búsqueda -- se usa
    /// `body.span` (`Block.span`, cubre el body completo, prerrequisito
    /// 3/3 del LSP), la única razón por la que esta ronda no necesitó
    /// agregar ningún span nuevo.
    ///
    /// Límite honesto: `check_fn`/`check_rpc` paran en el PRIMER error
    /// dentro del body -- el checker no tiene recuperación de errores a
    /// nivel de SENTENCIA (el parser sí, pero a nivel de ítem completo,
    /// GRAMMAR.md prerrequisito 2/3). Si el body tiene un error ANTES de
    /// la expresión hovereada, esa expresión nunca se llega a chequear y
    /// esto devuelve `None` -- ausente, no incorrecto, pero cerrar esto
    /// necesitaría recuperación de errores a nivel de sentencia en el
    /// checker, una extensión propia y más grande que esta ronda.
    pub(crate) fn hover_type_at(program: &Program, offset: usize) -> Option<Type> {
        let (mut checker, _) = Self::build_symbols(program);
        checker.hover_target = Some(offset);

        let in_body = |body: &Block| offset >= body.span.start && offset < body.span.end;
        for item in &program.items {
            match item {
                Item::Fn(f) if in_body(&f.body) => {
                    let _ = checker.check_fn(f);
                }
                Item::Test(t) if in_body(&t.body) => {
                    let _ = checker.check_test(t);
                }
                Item::Service(s) => {
                    for m in &s.members {
                        let (rpc, is_stream) = match m {
                            Member::Rpc(r) => (r, false),
                            Member::Stream(r) => (r, true),
                        };
                        if in_body(&rpc.body) {
                            let _ = checker.check_rpc(rpc, is_stream);
                        }
                    }
                }
                _ => {}
            }
        }

        checker.hover_result.into_inner().unwrap_or_else(|e| e.into_inner()).map(|(_, ty)| ty)
    }

    // ---- resolución de TypeExpr (sintáctico) -> Type (resuelto) ----
    //
    // `resolve_type` es la fachada pública (subst vacío) que ya usa el
    // resto del checker sin cambios. `resolve_type_subst` es la que de
    // verdad hace el trabajo, y sabe qué hacer cuando un identificador de
    // tipo (ej. "T") está LIGADO a un tipo concreto por el subst actual --
    // así es como se resuelve el CUERPO de un genérico instanciado
    // (GRAMMAR.md §3.6, monomorfización): `Box<Int>` arma `{"T": Int}` y
    // resuelve `{value: T}` con ese subst, dando `{value: Int}`.

    pub(crate) fn resolve_type(&self, texpr: &TypeExpr) -> Result<Type, CheckError> {
        self.resolve_type_subst(texpr, &HashMap::new())
    }

    /// Resuelve la declaración ABSTRACTA (sin instanciar) de un genérico,
    /// para emitir `interface Box<T> { value: T }` tal cual en el .d.ts
    /// (ts_emit.rs) -- cada type_param se liga a `Type::TypeParam(nombre)`,
    /// que se renderiza literalmente como ese nombre en TypeScript.
    pub(crate) fn resolve_type_abstract(&self, texpr: &TypeExpr, type_params: &[String]) -> Result<Type, CheckError> {
        let subst: HashMap<String, Type> = type_params
            .iter()
            .map(|p| (p.clone(), Type::TypeParam(p.clone())))
            .collect();
        self.resolve_type_subst(texpr, &subst)
    }

    fn resolve_type_subst(&self, texpr: &TypeExpr, subst: &HashMap<String, Type>) -> Result<Type, CheckError> {
        match texpr {
            TypeExpr::Named(name, args, _) => self.resolve_named_type_subst(name, args, subst),
            TypeExpr::Struct(fields) => {
                let mut ftys = Vec::new();
                for f in fields {
                    ftys.push(FieldType {
                        name: f.name.clone(),
                        optional: f.optional,
                        ty: self.resolve_type_subst(&f.ty, subst)?,
                    });
                }
                Ok(Type::Struct { name: None, fields: ftys })
            }
            TypeExpr::Optional(inner) => Ok(Type::Optional(Box::new(self.resolve_type_subst(inner, subst)?))),
            TypeExpr::List(inner) => Ok(Type::List(Box::new(self.resolve_type_subst(inner, subst)?))),
            TypeExpr::Tuple(items) => {
                let mut tys = Vec::new();
                for i in items {
                    tys.push(self.resolve_type_subst(i, subst)?);
                }
                Ok(Type::Tuple(tys))
            }
            TypeExpr::Function(params, ret) => {
                let mut ptys = Vec::new();
                for p in params {
                    ptys.push(self.resolve_type_subst(p, subst)?);
                }
                Ok(Type::Function(ptys, Box::new(self.resolve_type_subst(ret, subst)?)))
            }
            TypeExpr::Map(_, _) => Err(err(
                "tipo map { K: V } todavía no soportado por el checker (ambigüedad real con structs de un campo, GRAMMAR.md §2.2) — usa Map<K, V>",
            )),
            TypeExpr::Union(members) => {
                let mut tys = Vec::new();
                for m in members {
                    tys.push(self.resolve_type_subst(m, subst)?);
                }
                Ok(Type::Union(tys))
            }
        }
    }

    fn resolve_named_type_subst(&self, name: &str, args: &[TypeExpr], subst: &HashMap<String, Type>) -> Result<Type, CheckError> {
        // "T" dentro del cuerpo de un genérico que YA está siendo resuelto
        // (instanciado o en modo abstracto) -- ver resolve_type_abstract.
        if let Some(bound) = subst.get(name) {
            if !args.is_empty() {
                return Err(err(format!("'{name}' es un parámetro de tipo, no toma argumentos")));
            }
            return Ok(bound.clone());
        }
        match name {
            "Int" => Ok(Type::Int),
            "Int64" => Ok(Type::Int64),
            "Decimal" => Ok(Type::Decimal),
            "Timestamp" => Ok(Type::Timestamp),
            "Float" => Ok(Type::Float),
            "String" => Ok(Type::String),
            "Uuid" => Ok(Type::Uuid),
            "Bool" => Ok(Type::Bool),
            "Void" => Ok(Type::Void),
            "Result" => {
                // Builtin (GRAMMAR.md §3.5), no un enum declarado por el
                // usuario. Sus variantes fijas se resuelven on-demand en
                // check_result_lit/variant_field_types, no acá.
                let [a, b] = args else {
                    return Err(err("Result<T, E> requiere exactamente 2 argumentos de tipo"));
                };
                Ok(Type::ResultOf(
                    Box::new(self.resolve_type_subst(a, subst)?),
                    Box::new(self.resolve_type_subst(b, subst)?),
                ))
            }
            "Patch" => {
                // Builtin (GRAMMAR.md §3.4). T debe resolver a un struct —
                // "parchear" un Int o un enum no tiene sentido en este diseño.
                let [inner] = args else {
                    return Err(err("Patch<T> requiere exactamente 1 argumento de tipo"));
                };
                match self.resolve_type_subst(inner, subst)? {
                    Type::Struct { .. } => Ok(Type::PatchOf(Box::new(self.resolve_type_subst(inner, subst)?))),
                    other => Err(err(format!(
                        "Patch<T> requiere que T sea un struct, se encontró {other}"
                    ))),
                }
            }
            "Map" => {
                // Builtin (GRAMMAR.md §2.2) -- documentado como el reemplazo
                // de `{K: V}` desde que se descubrió esa ambigüedad, pero
                // nunca conectado acá hasta ahora (bug real, no solo gap).
                let [k, v] = args else {
                    return Err(err("Map<K, V> requiere exactamente 2 argumentos de tipo"));
                };
                let k_ty = self.resolve_type_subst(k, subst)?;
                if !matches!(k_ty, Type::String | Type::Int) {
                    return Err(err(format!(
                        "Map<K, V>: K debe ser String o Int (son las únicas claves JSON válidas), se encontró {k_ty:?}"
                    )));
                }
                Ok(Type::MapOf(Box::new(k_ty), Box::new(self.resolve_type_subst(v, subst)?)))
            }
            _ => {
                if let Some(decl) = self.types.get(name) {
                    if decl.type_params.is_empty() {
                        if !args.is_empty() {
                            return Err(err(format!("'{name}' no es genérico, no toma argumentos de tipo")));
                        }
                        let resolved = self.resolve_type_subst(&decl.ty, subst)?;
                        Ok(match resolved {
                            Type::Struct { fields, .. } => Type::Struct { name: Some(name.to_string()), fields },
                            other => other, // alias a un tipo no-struct, ej. `type Id = Int`
                        })
                    } else {
                        // Genérico (GRAMMAR.md §3.6): NO se expande acá --
                        // queda "opaco" como Type::Generic hasta que hace
                        // falta la forma real (expand_generic_struct,
                        // variant_field_types), igual que Result/Patch/Map.
                        if args.len() != decl.type_params.len() {
                            return Err(err(format!(
                                "'{name}' espera {} argumento(s) de tipo, se dieron {}",
                                decl.type_params.len(),
                                args.len()
                            )));
                        }
                        let resolved_args = args
                            .iter()
                            .map(|a| self.resolve_type_subst(a, subst))
                            .collect::<Result<Vec<_>, _>>()?;
                        Ok(Type::Generic(name.to_string(), resolved_args))
                    }
                } else if let Some(decl) = self.enums.get(name) {
                    if decl.type_params.is_empty() {
                        if !args.is_empty() {
                            return Err(err(format!("'{name}' no es genérico, no toma argumentos de tipo")));
                        }
                        Ok(Type::Enum(name.to_string()))
                    } else {
                        if args.len() != decl.type_params.len() {
                            return Err(err(format!(
                                "'{name}' espera {} argumento(s) de tipo, se dieron {}",
                                decl.type_params.len(),
                                args.len()
                            )));
                        }
                        let resolved_args = args
                            .iter()
                            .map(|a| self.resolve_type_subst(a, subst))
                            .collect::<Result<Vec<_>, _>>()?;
                        Ok(Type::Generic(name.to_string(), resolved_args))
                    }
                } else {
                    let mut candidates: Vec<&str> = vec!["Int", "Int64", "Decimal", "Timestamp", "Float", "String", "Uuid", "Bool", "Void", "Result", "Patch", "Map"];
                    candidates.extend(self.types.keys().map(String::as_str));
                    candidates.extend(self.enums.keys().map(String::as_str));
                    if let Some(sug) = find_best_suggestion(name, candidates) {
                        Err(err(format!("tipo desconocido: '{name}' -- ¿quisiste decir '{sug}'?")))
                    } else {
                        Err(err(format!("tipo desconocido: '{name}'")))
                    }
                }
            }
        }
    }

    /// Expande un `type` genérico instanciado a sus campos reales, ej.
    /// `Box<Int>` -> `[FieldType{value, Int}]`. Usado por field access y
    /// construcción (ver check_expr/synth_expr) -- nunca por is_subtype,
    /// que compara genéricos nominalmente (mismo nombre + mismos args ya
    /// alcanza vía la igualdad derivada, ver types.rs).
    pub(crate) fn expand_generic_struct(&self, name: &str, args: &[Type]) -> Result<Vec<FieldType>, CheckError> {
        let decl = self.types.get(name).ok_or_else(|| err(format!("tipo desconocido: '{name}'")))?;
        let TypeExpr::Struct(fields) = &decl.ty else {
            return Err(err(format!("'{name}' no es un struct genérico, no se puede construir con {{...}}")));
        };
        let subst: HashMap<String, Type> = decl.type_params.iter().cloned().zip(args.iter().cloned()).collect();
        fields
            .iter()
            .map(|f| {
                Ok(FieldType {
                    name: f.name.clone(),
                    optional: f.optional,
                    ty: self.resolve_type_subst(&f.ty, &subst)?,
                })
            })
            .collect()
    }

    fn resolve_fn_signature(&self, f: &FnDecl) -> Result<(Vec<Type>, Type), CheckError> {
        let mut params = Vec::new();
        for p in &f.params {
            params.push(self.resolve_type(&p.ty)?);
        }
        Ok((params, self.resolve_type(&f.return_type)?))
    }

    fn resolve_rpc_signature(&self, rpc: &RpcDecl) -> Result<(Vec<Type>, Type), CheckError> {
        let mut params = Vec::new();
        for p in &rpc.params {
            params.push(self.resolve_type(&p.ty)?);
        }
        Ok((params, self.resolve_type(&rpc.return_type)?))
    }

    // ---- ítems de nivel superior ----

    fn check_test(&self, t: &TestDecl) -> Result<(), CheckError> {
        let mut local = Env::new();
        for stmt in &t.body.stmts {
            self.check_stmt(stmt, &Type::Void, &mut local).map_err(|ce| ce.with_span(stmt.span))?;
        }
        if let Some(tail) = &t.body.tail {
            if matches!(tail.node, Expr::If { .. } | Expr::Match { .. } | Expr::Transaction(_)) {
                self.check_expr(tail, &Type::Void, &local)?;
            } else {
                self.synth_expr(tail, &local)?;
            }
        }
        Ok(())
    }

    fn check_fn(&self, f: &FnDecl) -> Result<(), CheckError> {
        let ret = self.resolve_type(&f.return_type)?;
        let mut env = Env::new();
        for p in &f.params {
            // Los parámetros no tienen sintaxis `mut` propia -- son
            // siempre inmutables, igual que los bindings de patrones.
            env.insert(p.name.clone(), immutable(self.resolve_type(&p.ty)?));
        }
        self.check_block(&f.body, &ret, &env)
    }

    /// `const X: T = v` (GRAMMAR.md §2.1) -- hallado sin chequear ni emitir
    /// durante la auditoría final: se parseaba, pero `check_program` lo
    /// ignoraba del todo (`_ => {}`) y el emisor nunca lo tocaba. Ahora se
    /// valida `v ⇐ T` igual que cualquier otro valor con tipo esperado.
    ///
    /// La restricción de forma-literal (hallada al diseñar auth v0, GRAMMAR.md
    /// §3.14) va ACÁ y no solo en `ts_emit.rs::render_const_value`: esa
    /// función solo corre en `linkc build`, nunca en `linkc serve`
    /// (`main.rs::cmd_serve` no llama a ningún emisor). Sin este chequeo acá,
    /// `const X: String = auth.createSession(Role.Admin {});` tipaba en
    /// `serve` igual que en `build`, y en runtime cada referencia a `X`
    /// recreaba una sesión Admin nueva (los `const` no se memoizan) sin que
    /// nadie la pidiera ni forma de limpiarla -- ya era una rareza inocua con
    /// `db` (releer la colección en cada uso), pero con `auth` deja de serlo.
    fn check_const(&self, c: &ConstDecl) -> Result<(), CheckError> {
        if !is_const_literal_shape(&c.value.node) {
            return Err(err(format!(
                "el valor de un 'const' tiene que ser un literal (número, string, bool, null, array, tupla \
                 o struct/variant literal) -- '{}' no lo es (es una computación en runtime, no un valor fijo)",
                c.name
            )));
        }
        let ty = self.resolve_type(&c.ty)?;
        self.check_expr(&c.value, &ty, &Env::new())
    }

    /// `is_stream` (`stream` en vez de `rpc`, GRAMMAR.md §2.1): la firma
    /// declara el tipo de ELEMENTO (ej. `-> User`, igual que un rpc normal
    /// -- así `AsyncIterable<User>` en el contrato TS sale del mismo
    /// `resolve_type(&r.return_type)` sin ningún caso especial en
    /// ts_emit.rs). Dos formas de cuerpo:
    /// - El shape reconocido de push real v0 (`ast::recognize_live_subscribe`,
    ///   GRAMMAR.md §3.16): se delega ENTERO a `check_live_subscribe`, que
    ///   valida la colección y el tipo -- nunca llama a `check_block`, no
    ///   hay ningún `Value` que el intérprete vaya a producir para ese
    ///   cuerpo (`server.rs` lo intercepta antes de invocar, ver
    ///   `runtime::live_subscribe_collection`).
    /// - Cualquier otro cuerpo: el camino de siempre, tiene que producir la
    ///   secuencia COMPLETA ya calculada (`List<User>`, no `User` suelto).
    fn check_rpc(&self, r: &RpcDecl, is_stream: bool) -> Result<(), CheckError> {
        let ret = self.resolve_type(&r.return_type)?;
        if is_stream {
            if let Some(collection) = crate::ast::recognize_live_subscribe(&r.body) {
                return self.check_live_subscribe(r, collection, &ret);
            }
        }
        let expected = if is_stream { Type::List(Box::new(ret)) } else { ret };
        let mut env = Env::new();
        for p in &r.params {
            let pty = self.resolve_type(&p.ty)?;
            // GRAMMAR.md §3.232: el contrato generado emite un type con
            // campos `@hidden` SIN esos campos, así que ningún cliente
            // podría mandarlos -- como parámetro sería una firma imposible
            // de cumplir. Mismo criterio que `NewX` para `insert`: un type
            // de entrada aparte.
            if let Some(offender) = self.type_mentions_hidden(&pty) {
                return Err(err(format!(
                    "el parámetro '{}' de '{}' es (o contiene) '{offender}', un type con campos '@hidden' -- el contrato generado lo emite sin esos campos, así que ningún cliente podría mandarlos; declará un type de entrada aparte, sin '@hidden' (GRAMMAR.md §3.232)",
                    p.name, r.name
                )));
            }
            if let Some(default) = &p.default {
                self.check_expr(default, &pty, &Env::new())?;
            }
            env.insert(p.name.clone(), immutable(pty));
        }
        let prev_in_stream = self.in_stream_body.swap(is_stream, std::sync::atomic::Ordering::Relaxed);
        let result = self.check_block(&r.body, &expected, &env);
        self.in_stream_body.store(prev_in_stream, std::sync::atomic::Ordering::Relaxed);
        result
    }

    /// El cuerpo de `r` ya matcheó el shape reconocido de push real
    /// (`while true { db.<coleccion>.subscribe() }`, GRAMMAR.md §3.16) --
    /// acá solo queda confirmar que `collection` existe de verdad y que su
    /// tipo de elemento es compatible con el retorno declarado.
    fn check_live_subscribe(&self, r: &RpcDecl, collection: &str, ret: &Type) -> Result<(), CheckError> {
        if !r.params.is_empty() {
            return Err(err(format!(
                "'{}': un stream de suscripción en vivo no toma parámetros en v0 (filtrar por id queda deliberadamente afuera de esta ronda)",
                r.name
            )));
        }
        let element_ty = self.db_collections.get(collection).ok_or_else(|| {
            err(format!(
                "'{}': 'db.{collection}' no es una colección declarada en 'db {{ ... }}'",
                r.name
            ))
        })?;
        if !is_subtype(element_ty, ret) {
            return Err(err(format!(
                "'{}': 'db.{collection}.subscribe()' produce {element_ty:?}, incompatible con el retorno declarado {ret:?}",
                r.name
            )));
        }
        Ok(())
    }

    // ---- bloques y sentencias ----

    fn check_block(&self, block: &Block, expected: &Type, env: &Env) -> Result<(), CheckError> {
        let mut local = env.clone();
        for stmt in &block.stmts {
            self.check_stmt(stmt, expected, &mut local).map_err(|ce| ce.with_span(stmt.span))?;
        }
        match &block.tail {
            Some(e) => self.check_expr(e, expected, &local),
            None => {
                if is_subtype(&Type::Void, expected) {
                    Ok(())
                } else {
                    Err(err(format!(
                        "el bloque no termina en una expresión y se esperaba un valor de tipo {expected}"
                    )))
                }
            }
        }
    }

    /// Chequea UNA sentencia de `check_block`, mutando `local` con cualquier
    /// binding que introduzca (`let`). Separada de `check_block` para que su
    /// loop pueda estampar el span DE LA SENTENCIA en cualquier error que no
    /// traiga ya uno más preciso puesto desde una sub-expresión --
    /// `check_expr`/`synth_expr` ya se ocupan de eso solos (`with_span`,
    /// primer stamp gana, nunca pisa uno más profundo).
    fn check_stmt(&self, stmt: &Spanned<Stmt>, expected: &Type, local: &mut Env) -> Result<(), CheckError> {
        match &stmt.node {
            Stmt::Let { name, mutable, ty, value } => {
                let value_ty = match ty {
                    Some(t) => {
                        let resolved = self.resolve_type(t)?;
                        self.check_expr(value, &resolved, local)?;
                        resolved
                    }
                    None => self.synth_expr(value, local)?,
                };
                local.insert(name.clone(), Binding { ty: value_ty, mutable: *mutable });
                Ok(())
            }
            Stmt::Assign { name, value } => {
                let binding = local
                    .get(name)
                    .ok_or_else(|| err(format!("variable no declarada: '{name}'")))?
                    .clone();
                if !binding.mutable {
                    return Err(err(format!(
                        "no se puede asignar a '{name}': no fue declarada con 'mut' (GRAMMAR.md §2.3)"
                    )));
                }
                self.check_expr(value, &binding.ty, local)
            }
            Stmt::Return(Some(e)) => self.check_expr(e, expected, local),
            Stmt::Return(None) => {
                if !is_subtype(&Type::Void, expected) {
                    return Err(err("'return' sin valor en una función que no devuelve Void"));
                }
                Ok(())
            }
            // if/match en posición de sentencia no tienen valor que alguien
            // use -- se chequean contra Void, lo que en la práctica exige
            // que cada rama sea puro efecto (sin tail), igual que exigir
            // `if cond { ... } else { ... }` sin usar el resultado.
            // synth_expr no sirve acá: if/match nunca sintetizan (§3.1/§3.7,
            // son de modo chequeo). Guard en vez de un binding-pattern con
            // or-pattern anidado: `Spanned<Expr>` no se puede matchear como
            // si fuera el enum `Expr` un nivel más abajo.
            Stmt::Expr(e) if matches!(e.node, Expr::If { .. } | Expr::Match { .. } | Expr::Transaction(_)) => {
                self.check_expr(e, &Type::Void, local)
            }
            Stmt::Expr(e) => self.synth_expr(e, local).map(|_| ()),
            // GRAMMAR.md §3.15: `cond` tiene que ser Bool; `body` corre por
            // efecto solamente, se chequea contra Void igual que un
            // if/match en posición de sentencia (mismo `check_block`, sin
            // cambios -- ESO es lo que hace que `let mut i=0;` declarado
            // ANTES del loop se pueda mutar adentro, gratis). `return`
            // alcanzable desde `body` se rechaza de entrada: en vez de
            // reescribir el mecanismo de señalización de control de flujo
            // (un cambio mucho más grande), un `while` simplemente no deja
            // usar `return` en su cuerpo -- sacá el valor final con una
            // variable `mut` declarada antes del loop y un tail después.
            Stmt::While { cond, body } => {
                self.check_expr(cond, &Type::Bool, local)?;
                if block_has_return(body) {
                    return Err(err(
                        "'return' no está permitido dentro del cuerpo de un 'while' en v0 (GRAMMAR.md §3.15) -- \
                         usá una variable 'mut' declarada antes del loop y un valor de cola después de él",
                    ));
                }
                self.check_block(body, &Type::Void, local)
            }
        }
    }

    /// Todo lo que aparece en la firma de un `rpc`/`stream` viaja de verdad
    /// por la red, así que tiene que ser expresable como JSON.
    ///
    /// La tabla de mapeo (GRAMMAR.md §4) ya decía que un tipo función "no
    /// cruza el wire" y que `Void` es "solo válido como retorno de rpc",
    /// pero nada lo hacía cumplir: `type T = { h: (Int) -> String }` usado
    /// como retorno tipaba, emitía `h: (arg0: number) => string` al
    /// contrato, y generaba un validador con `typeof x.h === "function"` --
    /// una condición que ningún payload JSON puede satisfacer, así que el
    /// cliente rechazaba siempre. Mejor un error claro acá que un contrato
    /// imposible de cumplir.
    ///
    /// `Void` sí es válido como retorno de nivel superior (un rpc que no
    /// devuelve nada), pero no como parámetro ni anidado dentro de otro
    /// tipo, donde no significa nada.
    fn check_rpc_crosses_the_wire(&self, r: &RpcDecl) -> Result<(), CheckError> {
        for p in &r.params {
            let ty = self.resolve_type(&p.ty)?;
            check_wire_safe(&ty, &format!("el parámetro '{}' de '{}'", p.name, r.name), false)?;
        }
        let ret = self.resolve_type(&r.return_type)?;
        check_wire_safe(&ret, &format!("el retorno de '{}'", r.name), true)
    }

    /// Qué combinaciones de anotaciones son legales (GRAMMAR.md §3.35). El
    /// parser acepta cualquier secuencia a propósito -- que `@content_type`
    /// sobre un `stream` sea un error es una regla del lenguaje, no de la
    /// gramática, y da mejor mensaje acá.
    fn check_annotation_combination(&self, r: &RpcDecl, is_stream: bool) -> Result<(), CheckError> {
        let auth_count = r
            .annotations
            .iter()
            .filter(|a| matches!(a, Annotation::Authenticated | Annotation::Requires { .. }))
            .count();
        if auth_count > 1 {
            return Err(err(format!(
                "'{}' declara más de una anotación de auth -- `@requires` ya implica autenticado, no hace falta sumarle `@authenticated`",
                r.name
            )));
        }

        let deprecated: Vec<&String> = r
            .annotations
            .iter()
            .filter_map(|a| match a {
                Annotation::Deprecated(reason) => Some(reason),
                _ => None,
            })
            .collect();
        if deprecated.len() > 1 {
            return Err(err(format!(
                "'{}' declara `@deprecated` más de una vez: un rpc tiene un solo motivo de baja",
                r.name
            )));
        }
        if let Some(reason) = deprecated.first() {
            if reason.trim().is_empty() {
                return Err(err(format!("`@deprecated(\"\")` en '{}': el motivo no puede estar vacío", r.name)));
            }
        }

        let content_types: Vec<&String> = r
            .annotations
            .iter()
            .filter_map(|a| match a {
                Annotation::ContentType(ct) => Some(ct),
                _ => None,
            })
            .collect();
        if content_types.len() > 1 {
            return Err(err(format!(
                "'{}' declara `@content_type` más de una vez: una respuesta tiene un solo Content-Type",
                r.name
            )));
        }
        let Some(ct) = content_types.first() else {
            return Ok(());
        };
        if ct.trim().is_empty() {
            return Err(err(format!(
                "`@content_type(\"\")` en '{}': el tipo MIME no puede estar vacío",
                r.name
            )));
        }
        if is_stream {
            return Err(err(format!(
                "`@content_type` en el stream '{}': un `stream` siempre se sirve como Server-Sent Events (text/event-stream), no se puede cambiar su Content-Type (GRAMMAR.md §3.35)",
                r.name
            )));
        }
        let ret = self.resolve_type(&r.return_type)?;
        if ret != Type::String {
            return Err(err(format!(
                "`@content_type` en '{}': el rpc tiene que devolver `String` -- el cuerpo de la respuesta se escribe tal cual, y {} no es texto que se pueda mandar sin serializar a JSON (GRAMMAR.md §3.35)",
                r.name,
                ret
            )));
        }
        Ok(())
    }

    /// Valida la forma de `@route("...")` y que el rpc tenga EXACTAMENTE los
    /// parámetros que esa forma implica (GRAMMAR.md §3.37). El conflicto
    /// ENTRE rutas de distintos rpc (dos patrones con la misma forma) no se
    /// puede resolver acá -- necesita ver TODO el programa a la vez -- así
    /// que lo hace `check_route_conflicts`, llamado aparte desde
    /// `check_program_full` después de recorrer todos los rpc.
    fn check_route_annotation(&self, r: &RpcDecl, is_stream: bool) -> Result<(), CheckError> {
        let routes: Vec<&String> = r
            .annotations
            .iter()
            .filter_map(|a| match a {
                Annotation::Route(pattern) => Some(pattern),
                _ => None,
            })
            .collect();
        if routes.len() > 1 {
            return Err(err(format!(
                "'{}' declara `@route` más de una vez: un rpc tiene una sola URL amigable adicional",
                r.name
            )));
        }
        let Some(raw) = routes.first() else {
            return Ok(());
        };
        if is_stream {
            return Err(err(format!(
                "`@route` en el stream '{}': un `stream` no tiene una request/response HTTP normal a la que pegarle una URL alternativa (GRAMMAR.md §3.37)",
                r.name
            )));
        }
        let pattern = crate::route::parse_route_pattern(raw).map_err(|e| err(format!("`@route(\"{raw}\")` en '{}': {e}", r.name)))?;

        // El rpc tiene que tomar AL MENOS los parámetros que la ruta declara
        // -- de menos, no (§3.42: cada uno con el MISMO nombre; el orden del
        // rpc no tiene por qué coincidir con el de la ruta). De MÁS sí se
        // acepta desde la ronda de query string (§3.62): cualquier parámetro
        // del rpc que no venga del path se lee de la query string por
        // nombre -- útil para un filtro (`?estado=activo`) sin tener que
        // duplicar el rpc completo solo para eso. Body sigue sin leerse,
        // a propósito: la URL de `@route` sirve tal cual para un crawler,
        // que nunca manda un POST con JSON.
        let route_params = pattern.param_names();
        if r.params.len() < route_params.len() {
            return Err(err(format!(
                "`@route(\"{raw}\")` en '{}': la ruta declara {} parámetro(s) ({}), pero el rpc toma solo {} -- le faltan",
                r.name,
                route_params.len(),
                route_params.iter().map(|n| format!(":{n}")).collect::<Vec<_>>().join(", "),
                r.params.len()
            )));
        }
        for name in &route_params {
            let Some(param) = r.params.iter().find(|p| &p.name == name) else {
                return Err(err(format!(
                    "`@route(\"{raw}\")` en '{}': la ruta tiene un parámetro ':{name}', pero el rpc no tiene ninguno que se llame '{name}' -- tienen que coincidir por nombre",
                    r.name
                )));
            };
            // De un segmento de URL sale texto. `Int` se acepta parseando
            // el segmento; cualquier otra cosa (Bool, Float, un struct) no
            // tiene una representación de un solo segmento sin ambigüedad.
            let param_ty = self.resolve_type(&param.ty)?;
            let is_catchall = pattern.catchall_name() == Some(*name);
            if is_catchall {
                // El catch-all captura CERO o más segmentos unidos con "/"
                // -- ese texto puede contener "/" y estar vacío, ninguna de
                // las dos cosas es un `Int` válido, así que a diferencia de
                // un `:param` normal acá NO se acepta `Int`.
                if !matches!(param_ty, Type::String) {
                    return Err(err(format!(
                        "`@route(\"{raw}\")` en '{}': ':{name}*' es un catch-all, captura texto arbitrario (incluyendo '/'), así que el parámetro tiene que ser `String` -- es {param_ty}",
                        r.name
                    )));
                }
            } else if !matches!(param_ty, Type::String | Type::Int) {
                return Err(err(format!(
                    "`@route(\"{raw}\")` en '{}': ':{name}' viene de un segmento de URL, así que el parámetro tiene que ser `String` o `Int` -- es {param_ty}",
                    r.name
                )));
            }
        }
        // Cualquier parámetro del rpc que NO esté en la ruta viene de la
        // query string (§3.62) -- mismo criterio de tipo que un segmento de
        // path (texto, `Int` se acepta parseando), pero además puede ser
        // `String?`/`Int?`: a diferencia de un segmento de path, un query
        // param puede estar simplemente AUSENTE de la URL sin que eso sea un
        // 404 -- `null` en ese caso, no un error.
        for param in &r.params {
            if route_params.contains(&param.name.as_str()) {
                continue;
            }
            let param_ty = self.resolve_type(&param.ty)?;
            let inner = match &param_ty {
                Type::Optional(inner) => inner.as_ref(),
                other => other,
            };
            if !matches!(inner, Type::String | Type::Int) {
                return Err(err(format!(
                    "`@route(\"{raw}\")` en '{}': '{}' no está en la ruta, así que viene de la query string -- tiene que ser `String`, `Int`, `String?` o `Int?` -- es {param_ty}",
                    r.name, param.name
                )));
            }
        }
        Ok(())
    }

    /// Valida el FORMATO de `@rate_limit("N/ventana")` en compilación --
    /// `crate::rate_limit::RateLimitSpec::parse` es la misma función que usa
    /// el servidor para interpretarlo en runtime (GRAMMAR.md §3.39, mismo
    /// motivo de módulo compartido que `check_route_annotation` de arriba).
    /// No hay restricción de auth/content_type/route acá: rate limiting es
    /// una dimensión ortogonal, se puede combinar con cualquiera de esas.
    fn check_rate_limit_annotation(&self, r: &RpcDecl) -> Result<(), CheckError> {
        let specs: Vec<(&String, &Option<String>)> = r
            .annotations
            .iter()
            .filter_map(|a| match a {
                Annotation::RateLimit { spec, key_param } => Some((spec, key_param)),
                _ => None,
            })
            .collect();
        if specs.len() > 1 {
            return Err(err(format!(
                "'{}' declara `@rate_limit` más de una vez: un rpc tiene un solo límite",
                r.name
            )));
        }
        let Some((raw, key_param)) = specs.first() else {
            return Ok(());
        };
        crate::rate_limit::RateLimitSpec::parse(raw).map_err(|e| err(format!("`@rate_limit(\"{raw}\")` en '{}': {e}", r.name)))?;
        // `key: <param>` (GRAMMAR.md §3.142) -- tiene que nombrar un
        // parámetro REAL de este rpc, de tipo `String`/`Int` (los únicos dos
        // que se pueden combinar con la IP en una clave de bucket de forma
        // determinística y sin ambigüedad).
        if let Some(key_param) = key_param {
            let Some(param) = r.params.iter().find(|p| &p.name == key_param) else {
                return Err(err(format!(
                    "`@rate_limit(..., key: {key_param})` en '{}': '{key_param}' no es un parámetro de este rpc",
                    r.name
                )));
            };
            let ty = self.resolve_type(&param.ty)?;
            if ty != Type::String && ty != Type::Int {
                return Err(err(format!(
                    "`@rate_limit(..., key: {key_param})` en '{}': '{key_param}' tiene que ser `String` o `Int` -- es `{ty}`",
                    r.name
                )));
            }
        }
        Ok(())
    }

    /// `@cache_control("...")` (GRAMMAR.md §3.113) -- dimensión ORTOGONAL,
    /// se combina con cualquier otra anotación (mismo criterio que
    /// `check_rate_limit_annotation`). Rechazado sobre un `stream`, mismo
    /// motivo exacto que `response.redirect`/`response.setStatus` dentro de
    /// un `stream` (§3.46/§3.111): una conexión SSE nunca es cacheable de
    /// forma sensata, así que declarar un `Cache-Control` ahí no tendría
    /// ningún efecto real -- error de compilación en vez de un no-op
    /// silencioso que solo se nota mirando los headers en producción.
    fn check_cache_control_annotation(&self, r: &RpcDecl, is_stream: bool) -> Result<(), CheckError> {
        let values: Vec<&String> = r
            .annotations
            .iter()
            .filter_map(|a| match a {
                Annotation::CacheControl(v) => Some(v),
                _ => None,
            })
            .collect();
        if values.len() > 1 {
            return Err(err(format!(
                "'{}' declara `@cache_control` más de una vez: una respuesta tiene un solo header Cache-Control",
                r.name
            )));
        }
        let Some(value) = values.first() else {
            return Ok(());
        };
        if value.trim().is_empty() {
            return Err(err(format!("`@cache_control(\"\")` en '{}': el valor no puede estar vacío", r.name)));
        }
        if is_stream {
            return Err(err(format!(
                "`@cache_control` en el stream '{}': una conexión SSE nunca es cacheable de forma sensata (GRAMMAR.md §3.113) -- llamalo desde un 'rpc' normal",
                r.name
            )));
        }
        Ok(())
    }

    /// `@example(request: <expr>, response: <expr>)` (GRAMMAR.md §3.119,
    /// PLAN.md §9.9 último ítem) -- las dos expresiones se tipan con el
    /// MISMO mecanismo que `= default` de un campo/param (`check_expr` con
    /// `Env::new()` vacío, ver `check_field_defaults`): `request` contra un
    /// struct anónimo armado de los parámetros del rpc (mismo criterio que
    /// `req_props` en `openapi_emit`, un param CON default es opcional ahí
    /// también), `response` contra el `return_type` resuelto. Un ejemplo
    /// desincronizado del contrato real es un error de compilación, no un
    /// dato que puede mentir en silencio en `openapi.json`.
    fn check_example_annotation(&self, r: &RpcDecl, is_stream: bool) -> Result<(), CheckError> {
        let examples: Vec<crate::ast::ExampleHalves> = r
            .annotations
            .iter()
            .filter_map(|a| match a {
                Annotation::Example { request, response } => Some((request.as_deref(), response.as_deref())),
                _ => None,
            })
            .collect();
        if examples.len() > 1 {
            return Err(err(format!("'{}' declara `@example` más de una vez: un rpc tiene un solo ejemplo de request/response", r.name)));
        }
        let Some((request, response)) = examples.into_iter().next() else {
            return Ok(());
        };
        if is_stream {
            return Err(err(format!(
                "`@example` en el stream '{}': un stream no tiene una única respuesta que ejemplificar (GRAMMAR.md §3.119) -- llamalo desde un 'rpc' normal",
                r.name
            )));
        }
        if let Some(req_expr) = request {
            if r.params.is_empty() {
                return Err(err(format!(
                    "`@example(request: ...)` en '{}': el rpc no toma parámetros, no hay ningún request body que ejemplificar",
                    r.name
                ))
                .with_span(req_expr.span));
            }
            if !is_literal_expr(&req_expr.node) {
                return Err(err(format!(
                    "`@example(request: ...)` en '{}': solo acepta un valor literal (struct/lista/escalar), no una expresión calculada",
                    r.name
                ))
                .with_span(req_expr.span));
            }
            let mut fields = Vec::with_capacity(r.params.len());
            for p in &r.params {
                fields.push(FieldType { name: p.name.clone(), optional: p.default.is_some(), ty: self.resolve_type(&p.ty)? });
            }
            self.check_expr(req_expr, &Type::Struct { name: None, fields }, &Env::new())?;
        }
        if let Some(res_expr) = response {
            if !is_literal_expr(&res_expr.node) {
                return Err(err(format!(
                    "`@example(response: ...)` en '{}': solo acepta un valor literal (struct/lista/escalar), no una expresión calculada",
                    r.name
                ))
                .with_span(res_expr.span));
            }
            let ret_ty = self.resolve_type(&r.return_type)?;
            self.check_expr(res_expr, &ret_ty, &Env::new())?;
        }
        Ok(())
    }

    /// `@invalidates(rpc1, rpc2, ...)` (GRAMMAR.md §3.125) -- cada nombre
    /// tiene que ser un rpc/stream declarado en la MISMA `service` (nunca
    /// cruzando a otra: el cache de Query generado en el frontend está
    /// organizado por `"{Servicio}.{rpc}(...)"`, invalidar algo de OTRO
    /// servicio no tendría ninguna clave real que matchear) que además
    /// genere un hook de Query (`RpcDecl::looks_like_a_query`, mismo
    /// heurístico que `emit_hooks` usa para decidir eso mismo) -- nombrar
    /// un rpc que nunca tuvo una entrada de cache no invalidaría nada.
    /// Rechazado sobre un `stream` (el rpc ANOTADO, no el nombrado): un
    /// stream no genera un hook de Mutation, el único lugar donde la
    /// invalidación se dispara en el código generado.
    fn check_invalidates_annotation(&self, r: &RpcDecl, is_stream: bool, service: &ServiceDecl) -> Result<(), CheckError> {
        let values: Vec<&Vec<String>> =
            r.annotations.iter().filter_map(|a| match a { Annotation::Invalidates(names) => Some(names), _ => None }).collect();
        if values.len() > 1 {
            return Err(err(format!("'{}' declara `@invalidates` más de una vez", r.name)));
        }
        let Some(names) = values.into_iter().next() else {
            return Ok(());
        };
        if is_stream {
            return Err(err(format!(
                "`@invalidates` en el stream '{}': un stream no genera un hook de Mutation (GRAMMAR.md §3.125) -- llamalo desde un 'rpc' normal",
                r.name
            )));
        }
        for name in names {
            let target = service.members.iter().find_map(|m| match m {
                Member::Rpc(t) | Member::Stream(t) if &t.name == name => Some((t, matches!(m, Member::Stream(_)))),
                _ => None,
            });
            let Some((target_rpc, target_is_stream)) = target else {
                return Err(err(format!(
                    "`@invalidates({name})` en '{}': '{name}' no es un rpc declarado en el service '{}'",
                    r.name, service.name
                )));
            };
            // AUDIT-2026-08-27.md #7: `looks_like_a_query()` (ast.rs) dice
            // "sí" para CUALQUIER rpc con cero parámetros -- y un rpc
            // `@cron` siempre tiene cero parámetros (el checker se lo exige
            // más abajo), así que sin este chequeo explícito, `@invalidates`
            // sobre un rpc `@cron` compilaba `OK`. `emit_hooks` (ts_emit.rs,
            // el emisor real de hooks de Query) SÍ excluye `@cron` -- así
            // que ese target nunca generaba un hook, y `@invalidates`
            // apuntaba a una entrada de caché que jamás existió: una
            // llamada muerta para siempre en `hooks.ts` (confirmado en vivo,
            // `linkc build` daba `OK`), exactamente la clase de "artefacto
            // generado que miente en silencio" que este proyecto rechaza en
            // otros lados (`@example`, GRAMMAR.md).
            if target_is_stream || target_rpc.cron().is_some() || !target_rpc.looks_like_a_query() {
                return Err(err(format!(
                    "`@invalidates({name})` en '{}': '{name}' no genera un hook de Query -- no hay ninguna entrada de cache que invalidar",
                    r.name
                )));
            }
        }
        Ok(())
    }

    /// `@infinite(cursor, limit)` (GRAMMAR.md §3.134) -- mismas firmas que
    /// `db.<c>.pageAfter(cursor: Int?, limit: Int)` (§3.61, el único
    /// mecanismo de paginación por cursor que el lenguaje ya tiene): el
    /// parámetro `cursor` nombrado tiene que ser `Int?`, el `limit`
    /// nombrado tiene que ser `Int`, y el retorno tiene que ser `T[]` con
    /// `T` teniendo un campo `id: Int` -- el próximo cursor que el hook
    /// generado calcula es el `id` del último elemento de la página
    /// (`ts_emit.rs`), mismo criterio que `pageAfter` usa puertas adentro.
    fn check_infinite_annotation(&self, r: &RpcDecl, is_stream: bool) -> Result<(), CheckError> {
        let count = r.annotations.iter().filter(|a| matches!(a, Annotation::Infinite { .. })).count();
        if count > 1 {
            return Err(err(format!("'{}' declara `@infinite` más de una vez", r.name)));
        }
        let Some((cursor_param, limit_param)) = r.infinite() else {
            return Ok(());
        };
        if is_stream {
            return Err(err(format!(
                "`@infinite` en el stream '{}': un stream ya empuja eventos en vivo, no necesita paginación (GRAMMAR.md §3.134)",
                r.name
            )));
        }
        if cursor_param == limit_param {
            return Err(err(format!(
                "`@infinite({cursor_param}, {limit_param})` en '{}': el cursor y el límite tienen que ser dos parámetros distintos",
                r.name
            )));
        }
        let find_param = |name: &str| r.params.iter().find(|p| p.name == name);
        let Some(cursor) = find_param(cursor_param) else {
            return Err(err(format!(
                "`@infinite({cursor_param}, ...)` en '{}': '{cursor_param}' no es un parámetro de este rpc",
                r.name
            )));
        };
        let cursor_ty = self.resolve_type(&cursor.ty)?;
        if cursor_ty != Type::Optional(Box::new(Type::Int)) {
            return Err(err(format!(
                "`@infinite({cursor_param}, ...)` en '{}': '{cursor_param}' tiene que ser `Int?` (mismo tipo que `cursor` en `db.<c>.pageAfter`) -- es `{cursor_ty}`",
                r.name
            )));
        }
        let Some(limit) = find_param(limit_param) else {
            return Err(err(format!(
                "`@infinite(..., {limit_param})` en '{}': '{limit_param}' no es un parámetro de este rpc",
                r.name
            )));
        };
        let limit_ty = self.resolve_type(&limit.ty)?;
        if limit_ty != Type::Int {
            return Err(err(format!(
                "`@infinite(..., {limit_param})` en '{}': '{limit_param}' tiene que ser `Int` (mismo tipo que `limit` en `db.<c>.pageAfter`) -- es `{limit_ty}`",
                r.name
            )));
        }
        let ret_ty = self.resolve_type(&r.return_type)?;
        let Type::List(elem_ty) = &ret_ty else {
            return Err(err(format!(
                "`@infinite` en '{}': el retorno tiene que ser una lista (`T[]`, una página de elementos) -- es `{ret_ty}`",
                r.name
            )));
        };
        let has_id_int = matches!(elem_ty.as_ref(), Type::Struct { fields, .. } if fields.iter().any(|f| f.name == "id" && f.ty == Type::Int));
        if !has_id_int {
            return Err(err(format!(
                "`@infinite` en '{}': el elemento de la lista de retorno tiene que tener un campo `id: Int` -- el hook generado usa el `id` del último elemento como el próximo cursor",
                r.name
            )));
        }
        Ok(())
    }

    /// `@idempotent` (GRAMMAR.md §3.140, PLAN.md §9.3) -- sin argumentos, así
    /// que la única forma inválida es sobre un `stream`: una conexión SSE no
    /// tiene un ÚNICO resultado que grabar y repetir, mismo motivo que
    /// `@cache_control`/`@example` rechazan ahí (GRAMMAR.md §3.113/§3.119).
    fn check_idempotent_annotation(&self, r: &RpcDecl, is_stream: bool) -> Result<(), CheckError> {
        if r.idempotent() && is_stream {
            return Err(err(format!(
                "`@idempotent` en el stream '{}': una conexión SSE no tiene un único resultado que grabar y repetir (GRAMMAR.md §3.140) -- llamalo desde un 'rpc' normal",
                r.name
            )));
        }
        Ok(())
    }

    /// `@cache("60s")` (GRAMMAR.md §3.144, PLAN.md §9.3) -- mismo criterio
    /// que `check_cache_control_annotation`: rechaza más de una vez y sobre
    /// un `stream` (una conexión SSE no tiene un único resultado que
    /// cachear). El formato de la duración lo valida `cache::parse_ttl`,
    /// misma función que el runtime usa para calcular la expiración real.
    fn check_cache_annotation(&self, r: &RpcDecl, is_stream: bool) -> Result<(), CheckError> {
        let values: Vec<&String> =
            r.annotations.iter().filter_map(|a| match a { Annotation::Cache(v) => Some(v), _ => None }).collect();
        if values.len() > 1 {
            return Err(err(format!("'{}' declara `@cache` más de una vez: un rpc tiene un solo TTL de cache", r.name)));
        }
        let Some(raw) = values.first() else {
            return Ok(());
        };
        if is_stream {
            return Err(err(format!(
                "`@cache` en el stream '{}': una conexión SSE no tiene un único resultado que cachear (GRAMMAR.md §3.144) -- llamalo desde un 'rpc' normal",
                r.name
            )));
        }
        // Auditoría adversarial (27/08/2026, AUDIT-2026-08-27.md #2): la
        // clave de caché es (service, rpc, argumentos) -- NUNCA incluye la
        // sesión/token del caller. Un rpc `@cache` que además es
        // `@authenticated`/`@requires` y cuya respuesta depende de LA
        // IDENTIDAD del caller (`auth.currentUserId()`, el patrón que
        // GRAMMAR.md §3.53 documenta y promueve para "mis notas"/"mi
        // dashboard") sirve, dentro del TTL, la respuesta de OTRO usuario
        // autenticado a cualquiera que llegue con los mismos argumentos
        // (casi siempre ninguno) -- confirmado en vivo: Alice llama, queda
        // cacheado; Bob, con su PROPIO token válido, recibe el perfil de
        // Alice completo. Rechazado acá hasta que exista un diseño real de
        // scoping por sesión (incluir el userId/token en la clave) -- mismo
        // criterio que el proyecto ya usa para otras combinaciones sin
        // sentido (`@cron` + cualquier otra anotación, más abajo).
        if r.auth().is_some() {
            return Err(err(format!(
                "'{}' combina `@cache` con `@authenticated`/`@requires`: la clave de caché no distingue quién llama, así que dentro del TTL cualquier caller autenticado recibiría la respuesta cacheada de OTRO -- sacá `@cache` de este rpc, o dejá de depender de la identidad del caller dentro del cuerpo (GRAMMAR.md §3.144)",
                r.name
            )));
        }
        crate::cache::parse_ttl(raw).map_err(|e| err(format!("`@cache(\"{raw}\")` en '{}': {e}", r.name)))?;
        Ok(())
    }

    /// `@cors("...")` (GRAMMAR.md §3.147) -- dimensión ORTOGONAL, se combina
    /// con cualquier otra anotación (mismo criterio que `@cache_control`).
    /// Válido sobre un `stream` también (a diferencia de `@cache_control`/
    /// `@idempotent`) -- un stream SSE sigue mandando headers de CORS reales
    /// (`sse_preamble`), así que un override por ruta tiene el mismo sentido
    /// ahí que en un `rpc` normal.
    fn check_cors_annotation(&self, r: &RpcDecl) -> Result<(), CheckError> {
        let values: Vec<&String> = r.annotations.iter().filter_map(|a| match a { Annotation::Cors(v) => Some(v), _ => None }).collect();
        if values.len() > 1 {
            return Err(err(format!("'{}' declara `@cors` más de una vez: un rpc tiene un solo override de CORS", r.name)));
        }
        if let Some(value) = values.first() {
            if value.trim().is_empty() {
                return Err(err(format!("`@cors(\"\")` en '{}': el valor no puede estar vacío", r.name)));
            }
        }
        Ok(())
    }

    /// `@cron("5m")` (GRAMMAR.md §3.159) -- tarea recurrente nativa, nunca
    /// alcanzable vía HTTP. A diferencia del resto de las anotaciones (que
    /// se combinan libremente), esta tiene que ser la ÚNICA: ninguna otra
    /// anotación (`@route`/`@authenticated`/`@rate_limit`/`@cache`/etc.)
    /// tiene sentido sobre algo que nunca recibe una request real -- en vez
    /// de dejarlas ahí sin efecto (una fuente clásica de confusión, "¿por
    /// qué mi `@rate_limit` no hace nada?"), el checker las rechaza de
    /// entrada. Sin parámetros (nada externo lo dispara) y retorno `Void`
    /// (nada consume una respuesta) -- mismo criterio de forma que
    /// `check_rpc_crosses_the_wire` ya aplica en general, pero acá son
    /// obligatorios, no solo permitidos.
    fn check_cron_annotation(&self, r: &RpcDecl, is_stream: bool) -> Result<(), CheckError> {
        let values: Vec<&String> = r.annotations.iter().filter_map(|a| match a { Annotation::Cron(v) => Some(v), _ => None }).collect();
        if values.len() > 1 {
            return Err(err(format!("'{}' declara `@cron` más de una vez: un rpc corre con un solo intervalo", r.name)));
        }
        let Some(raw) = values.first() else {
            return Ok(());
        };
        if is_stream {
            return Err(err(format!(
                "`@cron` en el stream '{}': una tarea recurrente no es una conexión SSE que alguien pueda suscribirse -- llamalo desde un 'rpc' normal (GRAMMAR.md §3.159)",
                r.name
            )));
        }
        if r.annotations.len() > 1 {
            return Err(err(format!(
                "'{}' combina `@cron` con otra anotación -- un rpc con `@cron` nunca se llama vía HTTP, así que `@route`/`@authenticated`/`@rate_limit`/`@cache`/etc. no tendrían ningún efecto ahí (GRAMMAR.md §3.159)",
                r.name
            )));
        }
        crate::cron::parse_interval(raw).map_err(|e| err(format!("`@cron(\"{raw}\")` en '{}': {e}", r.name)))?;
        if !r.params.is_empty() {
            return Err(err(format!(
                "'{}' declara `@cron` con parámetros -- nada externo dispara una tarea recurrente, así que no hay de dónde sacar sus argumentos en cada corrida (GRAMMAR.md §3.159)",
                r.name
            )));
        }
        let ret = self.resolve_type(&r.return_type)?;
        if !matches!(ret, Type::Void) {
            return Err(err(format!(
                "'{}' declara `@cron` con retorno '{}' -- una tarea recurrente no tiene ningún caller que reciba una respuesta, así que su retorno tiene que ser 'Void' (GRAMMAR.md §3.159)",
                r.name, ret
            )));
        }
        Ok(())
    }

    /// `@validate(...)` (GRAMMAR.md §3.73) sobre cada campo de `fields` --
    /// llamado tanto para un `type X = { ... }` como para los campos de cada
    /// variante de un `enum` (comparten `Field`, ver `ast.rs`). Dos cosas se
    /// validan acá, no en el parser: que el campo sea `String`/`String?`
    /// (necesita el tipo RESUELTO, no solo la forma sintáctica) y que un
    /// patrón de `@validate(regex, "...")` compile de verdad con la crate
    /// `regex` -- un patrón roto tiene que fallar en `linkc build`, nunca en
    /// el primer request real que lo dispare.
    fn check_field_validators(&self, fields: &[Field], type_params: &[String]) -> Vec<CheckError> {
        let mut errors = Vec::new();
        for f in fields {
            let Some(validator) = f.validator() else { continue };
            let ty = if type_params.is_empty() {
                self.resolve_type(&f.ty)
            } else {
                self.resolve_type_abstract(&f.ty, type_params)
            };
            let ty = match ty {
                Ok(ty) => ty,
                Err(e) => {
                    errors.push(e.with_span(f.name_span));
                    continue;
                }
            };
            let inner = match &ty {
                Type::Optional(inner) => inner.as_ref(),
                other => other,
            };
            if !matches!(inner, Type::String) {
                errors.push(
                    err(format!(
                        "'@validate' en el campo '{}': solo aplica sobre `String` o `String?` -- es `{ty}`",
                        f.name
                    ))
                    .with_span(f.name_span),
                );
                continue;
            }
            if let FieldValidator::Regex(pattern) = validator {
                if let Err(e) = regex::Regex::new(pattern) {
                    errors.push(
                        err(format!(
                            "`@validate(regex, \"{pattern}\")` en el campo '{}': patrón inválido -- {e}",
                            f.name
                        ))
                        .with_span(f.name_span),
                    );
                }
            }
        }
        errors
    }

    /// `= expr` sobre cada campo de `fields` (GRAMMAR.md §3.74) -- que el
    /// default TIPE contra el tipo declarado del campo, en `linkc build`,
    /// no recién cuando se evalúa por primera vez (`x: Int = "hola"` tiene
    /// que fallar acá). `Env::new()` vacío, mismo criterio EXACTO que ya usa
    /// `check_rpc` para `Param::default`: un default no ve otros campos del
    /// mismo literal ni el entorno que lo rodea, es una expresión
    /// autocontenida (ver `runtime/mod.rs::eval_expr`, mismo `Env::new()`
    /// en la evaluación real).
    fn check_field_defaults(&self, fields: &[Field], type_params: &[String]) -> Vec<CheckError> {
        let mut errors = Vec::new();
        for f in fields {
            let Some(default) = &f.default else { continue };
            let ty = if type_params.is_empty() {
                self.resolve_type(&f.ty)
            } else {
                self.resolve_type_abstract(&f.ty, type_params)
            };
            let ty = match ty {
                Ok(ty) => ty,
                Err(e) => {
                    errors.push(e.with_span(f.name_span));
                    continue;
                }
            };
            if let Err(e) = self.check_expr(default, &ty, &Env::new()) {
                errors.push(e.with_span(default.span));
            }
        }
        errors
    }

    /// `@autoUpdate` (GRAMMAR.md §3.77) solo sobre un campo `Timestamp`
    /// exacto -- ni `Timestamp?` ni cualquier otro tipo. El significado
    /// ("pisar a `now()` en cada `applyPatch`") no tiene sentido en
    /// ningún otro tipo, y necesita el tipo RESUELTO (no solo la forma
    /// sintáctica), por eso vive acá y no en el parser.
    fn check_field_auto_update(&self, fields: &[Field], type_params: &[String]) -> Vec<CheckError> {
        let mut errors = Vec::new();
        for f in fields {
            if !f.auto_update() {
                continue;
            }
            let ty = if type_params.is_empty() {
                self.resolve_type(&f.ty)
            } else {
                self.resolve_type_abstract(&f.ty, type_params)
            };
            match ty {
                Ok(Type::Timestamp) => {}
                Ok(ty) => errors.push(
                    err(format!(
                        "'@autoUpdate' en el campo '{}': solo aplica sobre `Timestamp` -- es `{ty}`",
                        f.name
                    ))
                    .with_span(f.name_span),
                ),
                Err(e) => errors.push(e.with_span(f.name_span)),
            }
        }
        errors
    }

    /// `@softDelete` (GRAMMAR.md §3.78) solo sobre un campo `Timestamp?`
    /// exacto -- ni `Timestamp` requerido ni cualquier otro tipo. Tiene que
    /// ser opcional porque "ausente/`null`" ES el estado "no borrado";
    /// requerido no dejaría representar eso.
    fn check_field_soft_delete(&self, fields: &[Field], type_params: &[String]) -> Vec<CheckError> {
        let mut errors = Vec::new();
        let marked: Vec<&Field> = fields.iter().filter(|f| f.soft_delete()).collect();
        // Más de un `@softDelete` sería ambiguo: `delete()` no sabría cuál
        // de los dos fijar. Se reporta UNA vez, nombrando los dos campos, en
        // vez de un error por campo.
        if marked.len() > 1 {
            let names: Vec<&str> = marked.iter().map(|f| f.name.as_str()).collect();
            errors.push(err(format!(
                "más de un campo con '@softDelete' ({}) -- a lo sumo uno por struct, si no `delete()` no sabría cuál fijar",
                names.join(", ")
            )));
        }
        for f in marked {
            let ty = if type_params.is_empty() {
                self.resolve_type(&f.ty)
            } else {
                self.resolve_type_abstract(&f.ty, type_params)
            };
            match ty {
                Ok(Type::Optional(inner)) if matches!(*inner, Type::Timestamp) => {}
                Ok(ty) => errors.push(
                    err(format!(
                        "'@softDelete' en el campo '{}': solo aplica sobre `Timestamp?` -- es `{ty}`",
                        f.name
                    ))
                    .with_span(f.name_span),
                ),
                Err(e) => errors.push(e.with_span(f.name_span)),
            }
        }
        errors
    }

    /// `@hidden` (GRAMMAR.md §3.232): nunca sobre `id` -- todo el runtime
    /// y el cliente generado lo necesitan en el JSON (`find`, `applyPatch`,
    /// cursores, `pageAfter`); ocultarlo dejaría un contrato sin forma de
    /// referirse a una fila.
    fn check_field_hidden(&self, fields: &[Field]) -> Vec<CheckError> {
        fields
            .iter()
            .filter(|f| f.hidden() && f.name == "id")
            .map(|f| {
                err("'@hidden' sobre 'id': el id tiene que viajar en el JSON (find, applyPatch, pageAfter, el cliente generado) -- no se puede ocultar (GRAMMAR.md §3.232)")
                    .with_span(f.name_span)
            })
            .collect()
    }

    /// GRAMMAR.md §3.232: ¿`ty` es, o contiene en cualquier nivel, un type
    /// con campos `@hidden`? Devuelve el nombre del primero que encuentra
    /// (para el mensaje). `seen` corta los types recursivos.
    pub(crate) fn type_mentions_hidden(&self, ty: &Type) -> Option<String> {
        self.type_mentions_hidden_inner(ty, &mut Vec::new())
    }

    fn type_mentions_hidden_inner(&self, ty: &Type, seen: &mut Vec<String>) -> Option<String> {
        match ty {
            Type::Struct { name, fields } => {
                if let Some(n) = name {
                    if self.hidden_fields.contains_key(n) {
                        return Some(n.clone());
                    }
                    if seen.contains(n) {
                        return None;
                    }
                    seen.push(n.clone());
                }
                fields.iter().find_map(|f| self.type_mentions_hidden_inner(&f.ty, seen))
            }
            Type::Optional(t) | Type::List(t) | Type::PatchOf(t) => self.type_mentions_hidden_inner(t, seen),
            Type::Tuple(items) | Type::Union(items) => items.iter().find_map(|t| self.type_mentions_hidden_inner(t, seen)),
            Type::MapOf(k, v) | Type::ResultOf(k, v) => {
                self.type_mentions_hidden_inner(k, seen).or_else(|| self.type_mentions_hidden_inner(v, seen))
            }
            Type::Generic(name, args) => {
                if self.hidden_fields.contains_key(name) {
                    return Some(name.clone());
                }
                args.iter().find_map(|t| self.type_mentions_hidden_inner(t, seen))
            }
            _ => None,
        }
    }

    /// `@encrypted` (GRAMMAR.md §3.191) solo sobre `String`/`String?` --
    /// ningún otro tipo tiene sentido para AES-256-GCM (que opera sobre
    /// texto, guardado en la MISMA columna `TEXT` de siempre, sin
    /// `ColumnKind` nuevo). Incompatible con `@index`/`@unique` en el mismo
    /// campo -- ver la doc de `FieldAnnotation::Encrypted` para el motivo
    /// real (el nonce aleatorio hace que un constraint SQL sobre esa
    /// columna sea una garantía falsa, no solo redundante).
    fn check_field_encrypted(&self, fields: &[Field], type_params: &[String]) -> Vec<CheckError> {
        let mut errors = Vec::new();
        for f in fields {
            if !f.encrypted() {
                continue;
            }
            if f.index().is_some() {
                errors.push(
                    err(format!(
                        "'@encrypted' en el campo '{}': incompatible con '@index'/'@unique' en el mismo campo -- el nonce aleatorio de AES-GCM hace que el ciphertext sea distinto en cada escritura, así que un constraint SQL sobre esa columna sería siempre \"único\", incluso para el mismo valor en texto plano",
                        f.name
                    ))
                    .with_span(f.name_span),
                );
            }
            // `x?: T?` (opcional-por-clave Y nullable-por-tipo a la vez,
            // GRAMMAR.md §3.4) fuerza el envoltorio JSON en `ColumnPlan`
            // (`for_field`) así T sea `String` -- el chokepoint de
            // cifrado vive en la rama de columna NATIVA (`write_param`/
            // `decode_row`, arm `(Type::String, Cell::Text(t))`), nunca
            // en la rama JSON. Aceptar esta combinación dejaría el campo
            // SIN cifrar, en silencio -- se rechaza acá, no en runtime.
            if f.optional && matches!(f.ty, TypeExpr::Optional(_)) {
                errors.push(
                    err(format!(
                        "'@encrypted' en el campo '{}': no se puede combinar con 'x?: T?' (opcional por clave Y nullable a la vez) -- fuerza el envoltorio JSON internamente, que este chokepoint de cifrado no cubre. Usá 'x: String?' (nullable, requerido por clave) en su lugar",
                        f.name
                    ))
                    .with_span(f.name_span),
                );
            }
            let ty = if type_params.is_empty() {
                self.resolve_type(&f.ty)
            } else {
                self.resolve_type_abstract(&f.ty, type_params)
            };
            match ty {
                Ok(Type::String) => {}
                Ok(Type::Optional(inner)) if matches!(*inner, Type::String) => {}
                Ok(ty) => errors.push(
                    err(format!(
                        "'@encrypted' en el campo '{}': solo aplica sobre `String`/`String?` -- es `{ty}`",
                        f.name
                    ))
                    .with_span(f.name_span),
                ),
                Err(e) => errors.push(e.with_span(f.name_span)),
            }
        }
        errors
    }

    /// `@check(...)` (GRAMMAR.md §3.96) sobre cada campo de `fields` --
    /// mismo criterio que `check_field_validators`: necesita el tipo
    /// RESUELTO del campo (`Int`/`Int64`/`Float`, requerido u opcional), así
    /// que vive acá, no en el parser. `@check(range, N, M)` con `N > M`
    /// también se rechaza acá -- un rango vacío ("nunca puede pasar") es
    /// casi siempre un error de tipeo, no una restricción real que alguien
    /// quiso escribir a propósito.
    fn check_field_checks(&self, fields: &[Field], type_params: &[String]) -> Vec<CheckError> {
        let mut errors = Vec::new();
        for f in fields {
            let Some(check) = f.check() else { continue };
            let ty = if type_params.is_empty() {
                self.resolve_type(&f.ty)
            } else {
                self.resolve_type_abstract(&f.ty, type_params)
            };
            let ty = match ty {
                Ok(ty) => ty,
                Err(e) => {
                    errors.push(e.with_span(f.name_span));
                    continue;
                }
            };
            let inner = match &ty {
                Type::Optional(inner) => inner.as_ref(),
                other => other,
            };
            let is_length_check = matches!(check, FieldCheck::MinLength(_) | FieldCheck::MaxLength(_));
            if is_length_check {
                if !matches!(inner, Type::String) {
                    errors.push(
                        err(format!(
                            "'@check(minLength/maxLength, ...)' en el campo '{}': solo aplica sobre `String` (u opcional de eso) -- es `{ty}`",
                            f.name
                        ))
                        .with_span(f.name_span),
                    );
                    continue;
                }
            } else if !matches!(inner, Type::Int | Type::Int64 | Type::Decimal | Type::Float) {
                errors.push(
                    err(format!(
                        "'@check(min/max/range, ...)' en el campo '{}': solo aplica sobre `Int`/`Int64`/`Decimal`/`Float` (u opcional de esos) -- es `{ty}`",
                        f.name
                    ))
                    .with_span(f.name_span),
                );
                continue;
            }
            match check {
                FieldCheck::Range(min, max) if min > max => {
                    errors.push(
                        err(format!(
                            "`@check(range, {min}, {max})` en el campo '{}': el mínimo es mayor que el máximo -- ningún valor podría pasar nunca",
                            f.name
                        ))
                        .with_span(f.name_span),
                    );
                }
                // GRAMMAR.md §3.146: una longitud es una CANTIDAD de
                // caracteres -- negativa o fraccionaria no tiene significado
                // (a diferencia de `min`/`max`/`range`, que sí aceptan
                // cualquier `f64` real porque el campo que limitan puede ser
                // `Float`).
                FieldCheck::MinLength(n) | FieldCheck::MaxLength(n) if *n < 0.0 || n.fract() != 0.0 => {
                    errors.push(
                        err(format!(
                            "'@check(minLength/maxLength, {n})' en el campo '{}': una longitud tiene que ser un entero no negativo",
                            f.name
                        ))
                        .with_span(f.name_span),
                    );
                }
                _ => {}
            }
        }
        errors
    }

    /// `@unique(campo1, campo2, ...)`/`@check(<expr>)` a nivel de `type`
    /// (GRAMMAR.md §3.155/§3.173) -- dos anotaciones que complementan, sin
    /// reemplazar, sus formas de un solo campo ya existentes
    /// (`FieldAnnotation::Index`/`FieldAnnotation::Check`, §3.80/§3.96).
    /// Viven en un método aparte de `check_field_checks` porque operan
    /// sobre el `TypeDecl` entero, no field por field -- las dos necesitan
    /// la lista COMPLETA de campos declarados (`@unique` para validar que
    /// cada nombre exista, `@check` para tipar la expresión contra ella).
    fn check_type_annotations(&self, t: &TypeDecl) -> Vec<CheckError> {
        let mut errors = Vec::new();
        if t.annotations.is_empty() {
            return errors;
        }
        let TypeExpr::Struct(fields) = &t.ty else {
            errors.push(err(format!(
                "'{}': una '@anotación' de nivel type ('@unique(...)'/'@check(...)') solo aplica sobre un `type` con forma de struct (`{{ campo: Tipo, ... }}`), no sobre un alias/unión",
                t.name
            )));
            return errors;
        };
        let field_names: std::collections::HashSet<&str> = fields.iter().map(|f| f.name.as_str()).collect();
        // La clave de redundancia incluye la CONDICIÓN (§3.174, `where
        // <expr>`), no solo el conjunto de campos -- dos `@unique` con el
        // mismo conjunto pero condiciones DISTINTAS son dos constraints
        // PARCIALES distintos, no un duplicado (`composite_unique_index_name`,
        // runtime/db.rs, ya asume esto para no colisionar de nombre).
        let mut seen_sets: Vec<(std::collections::BTreeSet<String>, Option<Expr>)> = Vec::new();
        for ann in &t.annotations {
            match ann {
                // GRAMMAR.md §3.239: `@index(...)` comparte TODAS las reglas
                // de `@unique(...)` (2+ campos, sin repetidos, campos reales,
                // `where` validado, sin duplicados) -- solo cambia el DDL.
                TypeAnnotation::Unique(names, condition) | TypeAnnotation::Index(names, condition) => {
                    let kind = if matches!(ann, TypeAnnotation::Unique(..)) { "@unique" } else { "@index" };
                    if names.len() < 2 {
                        errors.push(err(format!(
                            "'{}': '{kind}(...)' a nivel de type necesita al menos 2 campos -- para uno solo, poné '{kind}' directamente sobre ESE campo (GRAMMAR.md §3.80)",
                            t.name
                        )));
                        continue;
                    }
                    let set: std::collections::BTreeSet<String> = names.iter().cloned().collect();
                    if set.len() != names.len() {
                        errors.push(err(format!("'{}': '{kind}({})' repite el mismo campo más de una vez", t.name, names.join(", "))));
                        continue;
                    }
                    for name in names {
                        if !field_names.contains(name.as_str()) {
                            errors.push(err(format!(
                                "'{}': '{kind}(...)' nombra '{name}', que no es un campo declarado de este type",
                                t.name
                            )));
                        }
                    }
                    // `where <expr>` (GRAMMAR.md §3.174) -- MISMA validación
                    // de forma y tipo que `@check(<expr>)`: la condición
                    // puede referenciar CUALQUIER campo del struct, no solo
                    // los que integran el conjunto único (el caso real
                    // motivador cita justo eso: `status`, ajeno al
                    // conjunto `(userId, appointmentDate, startTime)`).
                    if let Some(cond) = condition {
                        errors.extend(self.check_type_level_check_expr(t, fields, cond));
                    }
                    let key = (set.clone(), condition.as_ref().map(|c| c.node.clone()));
                    if seen_sets.contains(&key) {
                        errors.push(err(format!(
                            "'{}': '{kind}({})' ya está declarado con la misma condición -- dos anotaciones ('@unique' o '@index') con exactamente el mismo conjunto de campos Y la misma 'where <expr>' son redundantes (un UNIQUE ya indexa)",
                            t.name,
                            names.join(", ")
                        )));
                    } else {
                        seen_sets.push(key);
                    }
                }
                TypeAnnotation::Check(expr) => {
                    errors.extend(self.check_type_level_check_expr(t, fields, expr));
                }
            }
        }
        errors
    }

    /// `@check(<expr>)` a nivel de `type` (GRAMMAR.md §3.173) -- una
    /// expresión booleana referenciando campos del propio struct por
    /// nombre PELADO (`endDate > startDate`, sin `self.`/prefijo: no hay
    /// ningún parámetro que bindear, a diferencia de un closure de
    /// `findWhere`/etc.), traducida a un `CHECK (...)` real de la base
    /// (`runtime::db::type_check_expr_sql`) Y aplicada del lado de la
    /// aplicación reusando el evaluador de expresiones normal
    /// (`runtime/mod.rs::eval_expr`) sobre un `Env` armado con los valores
    /// de esa fila -- mismo enforcement DOBLE que el resto de `@check`.
    ///
    /// Alcance deliberadamente acotado a lo que un `CHECK` de SQL puede
    /// expresar sin reimplementar un evaluador de expresiones acá:
    /// identificadores (que tienen que ser un campo declarado de ESTE
    /// struct), literales, y los operadores `==`/`!=`/`<`/`<=`/`>`/`>=`/
    /// `&&`/`||`/`!`/`-` (unario)/`+`/`-`/`*`/`/`/`%`. `validate_check_expr_shape`
    /// (ast.rs) rechaza CUALQUIER otra forma (llamada, acceso a `db`,
    /// closure, índice, literal de struct) ANTES de intentar tipar --
    /// ninguna de esas formas puede evaluarse dentro de un `CHECK` de SQL,
    /// así que dejarlas pasar solo para fallar después (en runtime, o
    /// nunca, silenciosamente mal) sería peor que rechazarlas acá con un
    /// mensaje claro.
    fn check_type_level_check_expr(&self, t: &TypeDecl, fields: &[Field], expr: &Spanned<Expr>) -> Vec<CheckError> {
        let mut errors = Vec::new();
        if let Err(bad_span) = crate::ast::validate_check_expr_shape(expr) {
            errors.push(
                err(format!(
                    "'{}': '@check(...)' a nivel de type solo admite nombres de campo, literales y los operadores \
                     ==, !=, <, <=, >, >=, &&, ||, !, +, -, *, /, % -- no llamadas, acceso a 'db', closures, índices \
                     ni literales de struct/enum",
                    t.name
                ))
                .with_span(bad_span),
            );
            return errors;
        }
        let mut env: Env = HashMap::new();
        for f in fields {
            let ty =
                if t.type_params.is_empty() { self.resolve_type(&f.ty) } else { self.resolve_type_abstract(&f.ty, &t.type_params) };
            match ty {
                Ok(ty) => {
                    env.insert(f.name.clone(), immutable(ty));
                }
                Err(e) => {
                    errors.push(e.with_span(f.name_span));
                    return errors;
                }
            }
        }
        // Un `Ident` referenciado que no sea un campo de este struct ya lo
        // rechaza `check_expr`/`synth_expr` con su error normal de "no
        // declarado" -- no hace falta duplicar esa validación acá.
        if let Err(e) = self.check_expr(expr, &Type::Bool, &env) {
            errors.push(e.with_span(t.span));
        }
        errors
    }

    /// Dos `@route` en CONFLICTO (`RoutePattern::conflicts_with`, route.rs:
    /// pueden matchear el mismo path real y ninguna es más específica que la
    /// otra) son indistinguibles al despachar una request real -- no hay
    /// forma de saber cuál de los dos rpc debería atenderla. Se rechaza en
    /// compilación, no se resuelve por orden de declaración ni "el primero
    /// gana": ese tipo de regla implícita es exactamente lo que después
    /// alguien pisa sin darse cuenta. Cuando SÍ hay una más específica
    /// (más segmentos literales fijos), esa gana determinísticamente y NO
    /// es un error -- `resolve_route` en runtime/server.rs es quien aplica
    /// esa prioridad.
    fn check_route_conflicts(&self, program: &Program) -> Vec<CheckError> {
        let mut seen: Vec<(String, crate::route::RoutePattern)> = Vec::new();
        let mut errors = Vec::new();
        for item in &program.items {
            let Item::Service(s) = item else { continue };
            for m in &s.members {
                let Member::Rpc(r) = m else { continue };
                let Some(raw) = r.route() else { continue };
                // Una ruta con forma inválida ya la reportó
                // `check_route_annotation` -- acá solo hace falta no volver
                // a fallar parseándola de nuevo, no duplicar el error.
                let Ok(pattern) = crate::route::parse_route_pattern(raw) else { continue };
                if let Some((other_rpc, _)) = seen.iter().find(|(_, p)| p.conflicts_with(&pattern)) {
                    errors.push(err(format!(
                        "`@route(\"{raw}\")` en '{}' entra en conflicto con la ruta de '{other_rpc}' -- \
                         un path real podría matchear las dos, y ninguna es más específica (mismo número de \
                         segmentos literales fijos), así que no hay forma determinística de saber cuál debería \
                         atenderla. Agregar un segmento literal a una de las dos alcanza para desempatar \
                         (ej. '/blog/{{prefijo}}/:slug')",
                        r.name
                    )));
                }
                seen.push((r.name.clone(), pattern));
            }
        }
        errors
    }

    /// `@requires(Enum.Variante)` (GRAMMAR.md §3.14, auth v0) tiene que
    /// nombrar un enum de verdad y una variante que de verdad exista en él --
    /// si no, el error aparece acá, en tiempo de compilación, no como un 403
    /// que nunca se puede satisfacer en runtime. Sin restricción de "enum
    /// simple" (ver `check_auth_method`): la comparación en runtime es solo
    /// por tag, nunca mira campos.
    fn check_rpc_annotation(&self, r: &RpcDecl, is_stream: bool, service: &ServiceDecl) -> Result<(), CheckError> {
        self.check_annotation_combination(r, is_stream)?;
        self.check_route_annotation(r, is_stream)?;
        self.check_rate_limit_annotation(r)?;
        self.check_cache_control_annotation(r, is_stream)?;
        self.check_example_annotation(r, is_stream)?;
        self.check_invalidates_annotation(r, is_stream, service)?;
        self.check_infinite_annotation(r, is_stream)?;
        self.check_idempotent_annotation(r, is_stream)?;
        self.check_cache_annotation(r, is_stream)?;
        self.check_cors_annotation(r)?;
        self.check_cron_annotation(r, is_stream)?;
        let Some(Annotation::Requires { enum_name, variant_names, ownership }) = r.auth() else {
            return Ok(());
        };
        let decl = self.enums.get(enum_name).ok_or_else(|| {
            err(format!(
                "@requires({enum_name}...) en '{}': '{enum_name}' no es un enum declarado",
                r.name
            ))
        })?;
        for variant_name in variant_names {
            if !decl.variants.iter().any(|v| &v.name == variant_name) {
                return Err(err(format!(
                    "@requires({enum_name}.{variant_name}) en '{}': '{enum_name}' no tiene una variante '{variant_name}'",
                    r.name
                )));
            }
        }
        if let Some(clause) = ownership {
            if is_stream {
                // `server.rs` deliberadamente SALTEA esta etapa para
                // `stream` (una suscripción de larga vida re-chequeando
                // dueño por evento es un problema distinto, límite honesto
                // de GRAMMAR.md §3.190) -- aceptar la cláusula acá sería
                // dejarla silenciosamente sin efecto en runtime, exactamente
                // el tipo de bug de "parece protegido pero no lo está" que
                // este proyecto rechaza en tiempo de compilación siempre
                // que puede detectarlo.
                return Err(err(format!(
                    "@requires(..., ownerOf: {}) en '{}': la cláusula de dueño no se puede usar sobre un 'stream' -- solo sobre 'rpc' (GRAMMAR.md §3.190)",
                    clause.collection, r.name
                )));
            }
            self.check_requires_ownership_clause(r, clause)?;
        }
        Ok(())
    }

    /// `@requires(..., ownerOf: <colección>, id: <parámetro>, field: <campo>)`
    /// (GRAMMAR.md §3.190) -- las tres validaciones que hacen que la
    /// cláusula tenga sentido antes de que `server.rs` la use en runtime:
    /// la colección existe, `id` nombra un parámetro real de ESTE rpc cuyo
    /// tipo coincide con la PK de esa colección (mismo criterio que
    /// `find`/`applyPatch` ya exigen vía `db_id_type`), y `field` es un
    /// campo `Int` real de esa colección (tiene que calzar con
    /// `auth.currentUserId(): Int?` para la comparación).
    fn check_requires_ownership_clause(&self, r: &RpcDecl, clause: &OwnershipClause) -> Result<(), CheckError> {
        let Some(element_ty) = self.db_collections().get(&clause.collection) else {
            return Err(err(format!(
                "@requires(..., ownerOf: {}) en '{}': '{}' no es una colección declarada en 'db'",
                clause.collection, r.name, clause.collection
            )));
        };
        let Some(param) = r.params.iter().find(|p| p.name == clause.id_param) else {
            return Err(err(format!(
                "@requires(..., id: {}) en '{}': '{}' no es un parámetro de este rpc",
                clause.id_param, r.name, clause.id_param
            )));
        };
        let param_ty = self.resolve_type(&param.ty)?;
        let id_ty = Self::db_id_type(element_ty);
        if param_ty != id_ty {
            return Err(err(format!(
                "@requires(..., id: {}) en '{}': '{}' tiene que ser {id_ty} (la PK de '{}'), es {param_ty}",
                clause.id_param, r.name, clause.id_param, clause.collection
            )));
        }
        let Type::Struct { fields, .. } = element_ty else {
            unreachable!("db_collections() ya garantizó que element_ty sea un struct (validate_db_element_type)");
        };
        let Some(field) = fields.iter().find(|f| f.name == clause.field) else {
            return Err(err(format!(
                "@requires(..., field: {}) en '{}': '{}' no es un campo de '{}'",
                clause.field, r.name, clause.field, clause.collection
            )));
        };
        if field.ty != Type::Int {
            return Err(err(format!(
                "@requires(..., field: {}) en '{}': '{}' tiene que ser Int (se compara contra auth.currentUserId()), es {}",
                clause.field, r.name, clause.field, field.ty
            )));
        }
        Ok(())
    }

    /// Sintetiza el tipo de la cola de un bloque SIN ningún tipo esperado
    /// del contexto -- lo que necesita un closure cuyos parámetros están
    /// anotados pero cuyo tipo de retorno no viene de ningún lado
    /// (GRAMMAR.md §3.10, `synth_expr(Expr::Closure)`).
    ///
    /// A propósito NO es "espejar `check_block` y cambiar `check_expr` por
    /// `synth_expr` en la cola": `check_block` usa el mismo `expected` tanto
    /// para la cola como para cualquier `Stmt::Return` anidado, y un
    /// `if`/`match` en posición de sentencia (no cola) YA se chequea hoy
    /// contra `Type::Void` sin importar el `expected` real del bloque que
    /// lo contiene -- un bug preexistente y real (nunca ejercitado: `return`
    /// no se usa en ningún `.link` ni test existente), pero ortogonal a
    /// esta ronda -- se documenta acá, no se arregla. En vez de heredar ese
    /// bug de otra forma (ej. intentando enhebrar un "expected de return"
    /// separado a través de `check_expr`/`check_match`, que reimplementaría
    /// esa lógica), `synth_block` rechaza de entrada, con un error claro,
    /// cualquier `return` alcanzable desde el bloque que recorre -- incluso
    /// dentro de un `if`/`match` no-cola. `block_has_return`/`expr_has_return`
    /// (funciones libres, debajo de este `impl`) hacen ese barrido; nunca
    /// descienden a un `Expr::Closure` anidado -- ese closure tiene su
    /// PROPIO contexto de retorno, chequeado aparte cuando a él le toque.
    fn synth_block(&self, block: &Block, env: &Env) -> Result<Type, CheckError> {
        if block_has_return(block) {
            return Err(err(
                "un closure sin tipo de retorno conocido por contexto no puede usar 'return' -- anotá los tipos para que se chequee contra un tipo esperado, o reescribilo sin 'return' (GRAMMAR.md §3.10)",
            ));
        }
        let mut local = env.clone();
        for stmt in &block.stmts {
            self.synth_stmt(stmt, &mut local).map_err(|ce| ce.with_span(stmt.span))?;
        }
        match &block.tail {
            Some(e) => self.synth_expr(e, &local),
            None => Ok(Type::Void),
        }
    }

    /// Análogo a `check_stmt`, para `synth_block` -- sin `expected` (acá no
    /// hay ninguno: `Return` ya fue descartado arriba por `block_has_return`,
    /// así que no hace falta el parámetro que `check_stmt` sí necesita solo
    /// para esa rama).
    fn synth_stmt(&self, stmt: &Spanned<Stmt>, local: &mut Env) -> Result<(), CheckError> {
        match &stmt.node {
            Stmt::Let { name, mutable, ty, value } => {
                let value_ty = match ty {
                    Some(t) => {
                        let resolved = self.resolve_type(t)?;
                        self.check_expr(value, &resolved, local)?;
                        resolved
                    }
                    None => self.synth_expr(value, local)?,
                };
                local.insert(name.clone(), Binding { ty: value_ty, mutable: *mutable });
                Ok(())
            }
            Stmt::Assign { name, value } => {
                let binding = local
                    .get(name)
                    .ok_or_else(|| err(format!("variable no declarada: '{name}'")))?
                    .clone();
                if !binding.mutable {
                    return Err(err(format!(
                        "no se puede asignar a '{name}': no fue declarada con 'mut' (GRAMMAR.md §2.3)"
                    )));
                }
                self.check_expr(value, &binding.ty, local)
            }
            Stmt::Return(_) => unreachable!("descartado por block_has_return arriba"),
            Stmt::Expr(e) if matches!(e.node, Expr::If { .. } | Expr::Match { .. } | Expr::Transaction(_)) => {
                self.check_expr(e, &Type::Void, local)
            }
            Stmt::Expr(e) => self.synth_expr(e, local).map(|_| ()),
            // Mismo brazo que check_stmt, sin la validación de `return`:
            // el scan de `block_has_return` al principio de `synth_block`
            // ya lo garantizó para TODO el bloque, incluido este `while`
            // (block_has_return ya recursa a su cuerpo).
            Stmt::While { cond, body } => {
                self.check_expr(cond, &Type::Bool, local)?;
                self.check_block(body, &Type::Void, local)
            }
        }
    }

    // ---- chequeo (modo ⇐): match y la construcción de Result<T,E> ----

    /// Wrapper delgado: `check_expr_inner` hace todo el trabajo real, esto
    /// solo estampa el `Span` de `e` en cualquier error que suba SIN span
    /// propio ya puesto -- un error que ya viene estampado desde más adentro
    /// (una sub-expresión que falló primero) queda con SU span, más preciso,
    /// nunca lo pisa este nivel más externo (`with_span`, primer stamp gana).
    fn check_expr(&self, e: &Spanned<Expr>, expected: &Type, env: &Env) -> Result<(), CheckError> {
        let result = self.check_expr_inner(&e.node, expected, env).map_err(|ce| ce.with_span(e.span));
        // Hover (GRAMMAR.md §3.24): en modo CHEQUEO (if/match/closure/...,
        // ver check_expr_inner) no hay un tipo SINTETIZADO propio -- pero
        // si el chequeo tuvo éxito, `expected` es, por construcción, un
        // tipo válido para esta expresión, así que es lo que se muestra.
        //
        // Si el chequeo FALLÓ (ej. el tail de un fn no matchea su tipo de
        // retorno declarado), `expected` NO es un tipo real de la
        // expresión -- es justamente lo que no matcheó. Bug real
        // encontrado escribiendo los tests de completion (§3.25, que
        // reusa esta misma máquina): el fallback de `check_expr_inner`
        // (más abajo) sintetiza `e` vía `synth_expr_inner` DIRECTO, no el
        // wrapper `synth_expr` con el probe -- así que ese tipo
        // (potencialmente el que el usuario quiere ver en el hover,
        // "esto ES una List(Int), aunque no matchea el Int esperado")
        // nunca llegaba a grabarse. Acá se reintenta la síntesis SOLO
        // para el hover (nunca afecta `result`, el error real de
        // chequeo sigue propagándose) -- redundante con la síntesis que
        // ya corrió adentro de `check_expr_inner`, pero solo se paga
        // cuando `hover_target` está activo (`probe_hover` es no-op
        // inmediato si no), nunca en un chequeo normal.
        self.probe_hover(e.span, || match &result {
            Ok(()) => Some(expected.clone()),
            Err(_) => self.synth_expr_inner(&e.node, env).ok(),
        });
        result
    }

    fn check_expr_inner(&self, e: &Expr, expected: &Type, env: &Env) -> Result<(), CheckError> {
        match e {
            Expr::Match { scrutinee, arms } => self.check_match(scrutinee, arms, expected, env),
            // if/else es de modo chequeo, igual que match (GRAMMAR.md §3.7):
            // no tiene un tipo propio, necesita el esperado para verificar
            // que ambas ramas produzcan lo mismo que el contexto pide.
            Expr::If { cond, then_block, else_block } => {
                self.check_expr(cond, &Type::Bool, env)?;
                self.check_block(then_block, expected, env)?;
                self.check_block(else_block, expected, env)
            }
            // GRAMMAR.md §3.154: mismo criterio de modo-chequeo que
            // if/match, arriba. `return` alcanzable adentro se rechaza de
            // entrada, mismo motivo/mensaje que `Stmt::While` (más abajo,
            // check_stmt) -- reescribir el mecanismo de señalización de
            // control de flujo para que "atraviese" un commit/rollback es
            // un cambio mucho más grande que este ítem amerita. El
            // anidamiento se rechaza con `in_transaction`: una sola
            // transacción SQL real por vez, sin savepoints en v0.
            Expr::Transaction(block) => {
                if self.in_transaction.load(std::sync::atomic::Ordering::Relaxed) {
                    return Err(err(
                        "'transaction' no puede anidarse dentro de otra 'transaction' -- una sola transacción SQL por vez en v0 (GRAMMAR.md §3.154)",
                    ));
                }
                if block_has_return(block) {
                    return Err(err(
                        "'return' no está permitido dentro del cuerpo de un 'transaction' en v0 (GRAMMAR.md §3.154) -- \
                         usá una variable 'mut' declarada antes del bloque y un valor de cola después de él",
                    ));
                }
                self.in_transaction.store(true, std::sync::atomic::Ordering::Relaxed);
                let result = self.check_block(block, expected, env);
                self.in_transaction.store(false, std::sync::atomic::Ordering::Relaxed);
                result
            }
            Expr::StructLit { name, variant: Some(v), fields } if name == "Result" => {
                self.check_result_lit(v, fields, expected, env)
            }
            // Construcción de un type/enum genérico DECLARADO POR EL USUARIO
            // (GRAMMAR.md §3.6) -- igual que Result, no se puede sintetizar
            // sin contexto (¿de dónde saldrían los argumentos de tipo?),
            // así que necesita el `expected` ya instanciado como Generic.
            Expr::StructLit { name, variant, fields } if self.is_user_generic(name) => {
                self.check_generic_struct_lit(name, variant.as_deref(), fields, expected, env)
            }
            // GRAMMAR.md §3.209: mismo caso que el brazo de arriba, pero
            // para la forma AZÚCAR sin llaves (`Maybe.Nothing`, no
            // `Maybe.Nothing {}`) de una variante sin campos de un enum
            // GENÉRICO -- el fallback genérico de más abajo (`_ =>`) llama
            // a `synth_expr_inner` en modo SÍNTESIS, sin `expected`, así
            // que no tiene de dónde sacar los argumentos de tipo
            // (`Maybe<Int>`); acá SÍ está disponible, así que se redirige
            // al mismo `check_generic_struct_lit` que ya resuelve la forma
            // con llaves. Sin este brazo, `Maybe.Nothing {}` tipaba en un
            // contexto con `expected` (ej. la rama de un `if` cuyo tipo ya
            // se conoce) pero la forma sin llaves no -- una asimetría
            // nueva que este mismo ítem existe para cerrar, no para crear.
            Expr::FieldAccess { base, field } if self.is_bare_unit_variant_of_generic_enum(base, field, env) => {
                let Expr::Ident(base_name) = &base.node else { unreachable!("garantizado por el guard de arriba") };
                self.check_generic_struct_lit(base_name, Some(field.as_str()), &[], expected, env)
            }
            // '[]' vacío: sin esto, synth_expr fallaría (no hay elemento del
            // que inferir el tipo). Con un List(T) esperado, alcanza con
            // verificar que efectivamente se pidió una lista -- vacía
            // satisface "todos los elementos son T" sin elementos que revisar.
            Expr::ArrayLit(items) if items.is_empty() => match expected {
                Type::List(_) | Type::Dynamic => Ok(()),
                other => Err(err(format!(
                    "un array vacío '[]' requiere un tipo esperado de lista, se esperaba {other}"
                ))),
            },
            // Rama EXPLÍCITA, no delegar al fallback de abajo -- si esto
            // solo existiera en `synth_expr`, un closure sin anotar caería
            // acá, exigiría anotación en cada param (la regla de síntesis) y
            // perdería toda la inferencia contextual, que es todo el punto
            // de chequear (⇐) un closure contra un `Type::Function` ya
            // conocido (GRAMMAR.md §3.10, ej. el callback de `.filter`).
            Expr::Closure { params, body } => self.check_closure(params, body, expected, env),
            // Fallback genérico: sintetiza y verifica subtipado. Llama a
            // `synth_expr_inner` DIRECTAMENTE, no al wrapper público
            // `synth_expr` -- acá `e` ya es el `&Expr` desenvuelto (esto es
            // una RE-ENTRADA sobre el MISMO nodo, en el otro modo, no un
            // descenso a un hijo), así que no hay ningún `Spanned<Expr>` del
            // que sacarlo. Bug real encontrado por el review antes de
            // implementar esto: llamar al wrapper público acá ni siquiera
            // compila.
            _ => {
                let t = self.synth_expr_inner(e, env)?;
                if is_subtype(&t, expected) {
                    Ok(())
                } else {
                    Err(err(format!("se esperaba un valor de tipo {expected}, se encontró {t}")))
                }
            }
        }
    }

    /// `check_expr(Expr::Closure, expected, env)` -- solo válido si
    /// `expected` es un `Type::Function` ya conocido. Por cada parámetro,
    /// anotado o no, `expected_params[i]` es la referencia: si el usuario
    /// anotó un tipo, tiene que ACEPTAR lo que el contexto va a pasarle
    /// (contravariante -- `is_subtype(expected_pty, anotación)`, NUNCA al
    /// revés; ver el contraejemplo real en `types::params_accept`, que
    /// documenta esta misma dirección para que no se pueda invertir por
    /// accidente en un segundo lugar); si no anotó, se liga directo a
    /// `expected_params[i]` -- acá es donde `list.filter(|x| x.activo)`
    /// infiere el tipo de `x` sin que el usuario lo escriba.
    fn check_closure(
        &self,
        params: &[ClosureParam],
        body: &Block,
        expected: &Type,
        env: &Env,
    ) -> Result<(), CheckError> {
        let Type::Function(expected_params, expected_ret) = expected else {
            return Err(err(format!("se esperaba un valor de tipo {expected}, se encontró un closure")));
        };
        if params.len() != expected_params.len() {
            return Err(err(format!(
                "el closure tiene {} parámetro(s), el contexto espera {}",
                params.len(),
                expected_params.len()
            )));
        }
        let mut local = env.clone();
        for (p, expected_pty) in params.iter().zip(expected_params) {
            let pty = match &p.ty {
                Some(texpr) => {
                    let annotated = self.resolve_type(texpr)?;
                    if !is_subtype(expected_pty, &annotated) {
                        return Err(err(format!(
                            "el parámetro '{}' anotado como {annotated:?} no acepta lo que el contexto le pasa ({expected_pty:?})",
                            p.name
                        )));
                    }
                    annotated
                }
                None => expected_pty.clone(),
            };
            local.insert(p.name.clone(), immutable(pty));
        }
        self.check_block(body, expected_ret, &local)
    }

    fn check_result_lit(
        &self,
        variant: &str,
        fields: &[(String, Spanned<Expr>)],
        expected: &Type,
        env: &Env,
    ) -> Result<(), CheckError> {
        let Type::ResultOf(ok_ty, err_ty) = expected else {
            return Err(err(format!(
                "'Result.{variant} {{...}}' usado donde se esperaba {expected}, no un Result<T, E>"
            )));
        };
        match variant {
            "Ok" => self.check_single_field(fields, "value", ok_ty, env),
            "Err" => self.check_single_field(fields, "error", err_ty, env),
            other => Err(err(format!("Result no tiene variante '{other}' (solo Ok/Err)"))),
        }
    }

    fn check_single_field(
        &self,
        fields: &[(String, Spanned<Expr>)],
        expected_name: &str,
        ty: &Type,
        env: &Env,
    ) -> Result<(), CheckError> {
        if fields.len() != 1 || fields[0].0 != expected_name {
            return Err(err(format!("se esperaba exactamente el campo '{expected_name}'")));
        }
        self.check_expr(&fields[0].1, ty, env)
    }

    /// `true` si `name` es un `type`/`enum` DECLARADO POR EL USUARIO con
    /// type_params -- distinto de "Result"/"Patch"/"Map" (builtins, ya
    /// manejados aparte) y de un type/enum normal (sin type_params, sigue
    /// el camino existente de synth_struct_lit).
    fn is_user_generic(&self, name: &str) -> bool {
        self.types.get(name).is_some_and(|d| !d.type_params.is_empty())
            || self.enums.get(name).is_some_and(|d| !d.type_params.is_empty())
    }

    /// GRAMMAR.md §3.209: mismo chequeo que la síntesis de `Expr::FieldAccess`
    /// (`synth_expr_inner`) usa para decidir si `Enum.Variante` sin llaves es
    /// azúcar válida -- repetido acá, no extraído a un helper compartido con
    /// esa otra función, porque las dos formas AHÍ necesitan el `&Expr::Ident`
    /// desenvuelto de maneras ligeramente distintas (una para armar el
    /// mensaje de error, esta solo para el guard de un match).
    fn is_bare_unit_variant_of_generic_enum(&self, base: &Spanned<Expr>, field: &str, env: &Env) -> bool {
        let Expr::Ident(base_name) = &base.node else { return false };
        if env.contains_key(base_name) {
            return false;
        }
        if !self.is_user_generic(base_name) {
            return false;
        }
        let Some(decl) = self.enums.get(base_name) else { return false };
        decl.variants.iter().find(|v| v.name == field).is_some_and(|v| v.fields.as_ref().is_none_or(|fs| fs.is_empty()))
    }

    fn check_generic_struct_lit(
        &self,
        name: &str,
        variant: Option<&str>,
        fields: &[(String, Spanned<Expr>)],
        expected: &Type,
        env: &Env,
    ) -> Result<(), CheckError> {
        let Type::Generic(gname, gargs) = expected else {
            return Err(err(format!(
                "'{name}' es genérico -- se necesita un tipo esperado ya instanciado (ej. anotá el 'let', o usalo donde el tipo ya se conoce), se encontró {expected}"
            )));
        };
        if gname != name {
            return Err(err(format!("se esperaba '{gname}', se encontró una construcción de '{name}'")));
        }
        let field_decls: Vec<FieldType> = match variant {
            None => self.expand_generic_struct(name, gargs)?,
            Some(vname) => self
                .variant_field_types(expected, name, vname)?
                .into_iter()
                .map(|(n, ty)| FieldType { name: n, optional: false, ty })
                .collect(),
        };
        self.check_fields_against_resolved(&field_decls, fields, env)
    }

    fn check_match(
        &self,
        scrutinee: &Spanned<Expr>,
        arms: &[MatchArm],
        expected: &Type,
        env: &Env,
    ) -> Result<(), CheckError> {
        let scrutinee_ty = self.synth_expr(scrutinee, env)?;

        match &scrutinee_ty {
            Type::Enum(_) | Type::ResultOf(_, _) | Type::Generic(_, _) => {
                let enum_name = match &scrutinee_ty {
                    Type::Enum(n) => n.clone(),
                    Type::ResultOf(_, _) => "Result".to_string(),
                    Type::Generic(n, _) => n.clone(), // enum genérico instanciado, ej. Option<Int>
                    _ => unreachable!(),
                };
                self.check_exhaustive_enum(&scrutinee_ty, &enum_name, arms)?;
            }
            // Extensión más allá de enum (GRAMMAR.md §3.3): matchear un
            // primitivo con patrones de literal. Deliberadamente sin Float
            // (igualdad exacta de floats) ni Optional/Null (matchear un `T?`
            // directamente queda para más adelante, ver Pattern::Literal).
            // Int64 sí entra -- misma semántica de igualdad exacta que Int,
            // a diferencia de Float (GRAMMAR.md §3.30).
            Type::Int | Type::Int64 | Type::String | Type::Bool => {
                self.check_exhaustive_literal(&scrutinee_ty, arms)?;
            }
            // Narrowing de uniones (GRAMMAR.md §3.9): patrones `nombre: Tipo`.
            Type::Union(members) => {
                self.check_exhaustive_union(members, arms)?;
            }
            // Narrowing real de un opcional (GRAMMAR.md §3.9): `null` para
            // el caso ausente, `nombre: T` para el caso presente -- mismo
            // mecanismo de patrones que ya usa la unión de arriba, pero acá
            // el escrutinio no es una lista de miembros, es "T o ausente".
            Type::Optional(inner) => {
                self.check_exhaustive_optional(inner, arms)?;
            }
            other => {
                return Err(err(format!(
                    "'match' requiere un valor de tipo enum, Int, String, Bool, unión u opcional (T?); se encontró {other}"
                )))
            }
        }

        for arm in arms {
            let mut arm_env = env.clone();
            self.bind_pattern(&arm.pattern, &scrutinee_ty, &mut arm_env)?;
            // El guard ve las variables que el patrón acaba de ligar, ej.
            // `Status.Setting { level } if level > 10 => ...` -- por eso se
            // chequea acá, con arm_env, no con env.
            if let Some(guard) = &arm.guard {
                self.check_expr(guard, &Type::Bool, &arm_env)?;
            }
            match &arm.body {
                MatchArmBody::Expr(e) => self.check_expr(e, expected, &arm_env)?,
                MatchArmBody::Block(b) => self.check_block(b, expected, &arm_env)?,
            }
        }
        Ok(())
    }

    /// Algoritmo de GRAMMAR.md §3.3: cualquier `Pattern::Bind` SIN GUARD
    /// (incluye `_` y bindings con nombre, ej. `otro => ...`) es un catch-all
    /// irrefutable. Un arm CON guard nunca descarta exhaustividad -- la
    /// condición podría ser falsa en runtime, así que no cuenta como cubierto.
    fn check_exhaustive_enum(&self, scrutinee_ty: &Type, enum_name: &str, arms: &[MatchArm]) -> Result<(), CheckError> {
        let variants: Vec<String> = if matches!(scrutinee_ty, Type::ResultOf(_, _)) {
            vec!["Ok".to_string(), "Err".to_string()]
        } else {
            self.enum_variant_names(enum_name)?
        };

        let mut covered = HashSet::new();
        let mut wildcard = false;
        for arm in arms {
            if arm.guard.is_some() {
                continue;
            }
            self.collect_variant_coverage(&arm.pattern, enum_name, &mut wildcard, &mut covered)?;
        }

        if wildcard || variants.iter().all(|v| covered.contains(v)) {
            Ok(())
        } else {
            let missing: Vec<_> = variants.into_iter().filter(|v| !covered.contains(v)).collect();
            Err(err(format!(
                "match no exhaustivo sobre '{enum_name}': falta cubrir {missing:?} (GRAMMAR.md §3.3)"
            )))
        }
    }

    /// Recorre un patrón (posiblemente un `Or`) sumando a `covered`/`wildcard`
    /// -- separado de `check_exhaustive_enum` para que `Or` sea un solo punto
    /// de recursión, no un caso más a duplicar en cada algoritmo.
    fn collect_variant_coverage(
        &self,
        pattern: &Pattern,
        enum_name: &str,
        wildcard: &mut bool,
        covered: &mut HashSet<String>,
    ) -> Result<(), CheckError> {
        match pattern {
            Pattern::Bind(_) => {
                *wildcard = true;
                Ok(())
            }
            Pattern::Variant { enum_name: en, variant_name, .. } => {
                if en != enum_name {
                    return Err(err(format!(
                        "patrón para el enum '{en}' no coincide con el tipo del escrutinio ('{enum_name}')"
                    )));
                }
                covered.insert(variant_name.clone());
                Ok(())
            }
            Pattern::Or(subs) => {
                for s in subs {
                    self.collect_variant_coverage(s, enum_name, wildcard, covered)?;
                }
                Ok(())
            }
            Pattern::Literal(lit) => Err(err(format!(
                "patrón literal {lit:?} no válido contra un escrutinio de tipo enum ('{enum_name}')"
            ))),
            Pattern::Type(name, texpr) => Err(err(format!(
                "patrón de tipo '{name}: {texpr:?}' no válido contra un escrutinio de tipo enum ('{enum_name}') -- \
                 el narrowing de uniones (GRAMMAR.md §3.9) es solo para escrutinios Type::Union"
            ))),
        }
    }

    /// Exhaustividad para un escrutinio Int/String/Bool (GRAMMAR.md §3.3):
    /// los patrones de literal nunca alcanzan por sí solos -- Int/String
    /// tienen un espacio de valores no enumerable, así que siempre hace
    /// falta un catch-all sin guard. Única excepción: Bool, que sí se puede
    /// cubrir del todo con 'true' Y 'false' (es, en los hechos, un enum de
    /// dos variantes).
    fn check_exhaustive_literal(&self, scrutinee_ty: &Type, arms: &[MatchArm]) -> Result<(), CheckError> {
        let mut wildcard = false;
        let mut covered_bools: HashSet<bool> = HashSet::new();
        for arm in arms {
            if arm.guard.is_some() {
                continue;
            }
            self.collect_literal_coverage(&arm.pattern, scrutinee_ty, &mut wildcard, &mut covered_bools)?;
        }

        let bool_exhaustive = matches!(scrutinee_ty, Type::Bool) && covered_bools.len() == 2;

        if wildcard || bool_exhaustive {
            Ok(())
        } else {
            Err(err(format!(
                "match no exhaustivo: los patrones de literal nunca alcanzan por sí solos sobre {scrutinee_ty:?} -- \
                 hace falta un arm final sin guard que capture el resto (ej. '_ => ...'), salvo Bool con 'true' y \
                 'false' ambos cubiertos (GRAMMAR.md §3.3)"
            )))
        }
    }

    fn collect_literal_coverage(
        &self,
        pattern: &Pattern,
        scrutinee_ty: &Type,
        wildcard: &mut bool,
        covered_bools: &mut HashSet<bool>,
    ) -> Result<(), CheckError> {
        match pattern {
            Pattern::Bind(_) => {
                *wildcard = true;
                Ok(())
            }
            Pattern::Literal(lit) => {
                self.check_literal_matches_type(lit, scrutinee_ty)?;
                if let LiteralPattern::Bool(b) = lit {
                    covered_bools.insert(*b);
                }
                Ok(())
            }
            Pattern::Or(subs) => {
                for s in subs {
                    self.collect_literal_coverage(s, scrutinee_ty, wildcard, covered_bools)?;
                }
                Ok(())
            }
            Pattern::Variant { enum_name, .. } => Err(err(format!(
                "patrón de variante de enum ('{enum_name}') no válido contra un escrutinio de tipo {scrutinee_ty:?}"
            ))),
            Pattern::Type(name, texpr) => Err(err(format!(
                "patrón de tipo '{name}: {texpr:?}' no válido contra un escrutinio de tipo {scrutinee_ty:?} -- \
                 el narrowing de uniones (GRAMMAR.md §3.9) es solo para escrutinios Type::Union"
            ))),
        }
    }

    /// Narrowing de uniones (GRAMMAR.md §3.9): rechaza de entrada, ANTES de
    /// mirar los arms siquiera, una unión cuyos miembros no se puedan
    /// distinguir de forma demostrable (`union_members_are_distinguishable`)
    /// -- es una propiedad de la unión en sí, no de cómo se la matchea, así
    /// que corre una sola vez por par de miembros. Después, exhaustividad:
    /// mismo algoritmo que enum/literal (un `Pattern::Bind` sin guard cubre
    /// el resto; un arm con guard nunca descarta cobertura).
    fn check_exhaustive_union(&self, members: &[Type], arms: &[MatchArm]) -> Result<(), CheckError> {
        for i in 0..members.len() {
            for j in (i + 1)..members.len() {
                if !union_members_are_distinguishable(&members[i], &members[j]) {
                    return Err(err(format!(
                        "no se puede hacer 'match' sobre esta unión: los miembros {:?} y {:?} no se pueden \
                         distinguir de forma demostrable en runtime (GRAMMAR.md §3.9) -- si hace falta \
                         distinguirlos, modelá la alternancia como un 'enum' en vez de una unión estructural",
                        members[i], members[j]
                    )));
                }
            }
        }

        let mut wildcard = false;
        let mut covered = vec![false; members.len()];
        for arm in arms {
            if arm.guard.is_some() {
                continue;
            }
            self.collect_union_coverage(&arm.pattern, members, &mut wildcard, &mut covered)?;
        }

        if wildcard || covered.iter().all(|c| *c) {
            Ok(())
        } else {
            let missing: Vec<&Type> = members.iter().zip(&covered).filter(|(_, c)| !**c).map(|(m, _)| m).collect();
            Err(err(format!("match no exhaustivo sobre la unión: falta cubrir {missing:?} (GRAMMAR.md §3.9)")))
        }
    }

    /// Análogo a `collect_variant_coverage`/`collect_literal_coverage`, mismo
    /// patrón de "Or recursa, todo lo demás que no sea el patrón propio de
    /// este escrutinio es un error". `Type` no deriva `Hash`/`Eq` (solo
    /// `PartialEq`, y ese deriva es posicional sobre el `Vec<FieldType>` de
    /// un struct -- dos structs estructuralmente idénticos con campos en
    /// otro orden serían `==`-distintos), así que membership se decide por
    /// `is_subtype` MUTUO, no `==`; y la cobertura se trackea por posición
    /// (`Vec<bool>`), no `HashSet<Type>`.
    fn collect_union_coverage(
        &self,
        pattern: &Pattern,
        members: &[Type],
        wildcard: &mut bool,
        covered: &mut [bool],
    ) -> Result<(), CheckError> {
        match pattern {
            Pattern::Bind(_) => {
                *wildcard = true;
                Ok(())
            }
            Pattern::Type(_, texpr) => {
                let resolved = self.resolve_type(texpr)?;
                let mut matched_any = false;
                for (i, m) in members.iter().enumerate() {
                    if is_subtype(&resolved, m) && is_subtype(m, &resolved) {
                        covered[i] = true;
                        matched_any = true;
                    }
                }
                if matched_any {
                    Ok(())
                } else {
                    Err(err(format!(
                        "el patrón de tipo '{resolved:?}' no corresponde a ningún miembro de esta unión ({members:?})"
                    )))
                }
            }
            Pattern::Or(subs) => {
                for s in subs {
                    self.collect_union_coverage(s, members, wildcard, covered)?;
                }
                Ok(())
            }
            Pattern::Literal(lit) => Err(err(format!(
                "patrón literal {lit:?} no válido contra un escrutinio de tipo unión -- usá 'nombre: Tipo' (GRAMMAR.md §3.9)"
            ))),
            Pattern::Variant { enum_name, .. } => Err(err(format!(
                "patrón de variante de enum ('{enum_name}') no válido contra un escrutinio de tipo unión -- usá \
                 'nombre: Tipo' (GRAMMAR.md §3.9)"
            ))),
        }
    }

    /// Narrowing real de `T?` (GRAMMAR.md §3.9). Dos "miembros" a cubrir, no
    /// una lista: el caso ausente (patrón `null`) y el caso presente (patrón
    /// `nombre: T`, que liga `nombre` al `T` DESENVUELTO -- ya no `T?`). Un
    /// `Pattern::Bind` sin guard cubre los dos a la vez, mismo criterio que
    /// el resto de los escrutinios de esta función.
    fn check_exhaustive_optional(&self, inner: &Type, arms: &[MatchArm]) -> Result<(), CheckError> {
        let mut wildcard = false;
        let mut covers_null = false;
        let mut covers_value = false;
        for arm in arms {
            if arm.guard.is_some() {
                continue;
            }
            self.collect_optional_coverage(&arm.pattern, inner, &mut wildcard, &mut covers_null, &mut covers_value)?;
        }

        if wildcard || (covers_null && covers_value) {
            Ok(())
        } else {
            let mut missing = Vec::new();
            if !covers_null {
                missing.push("null".to_string());
            }
            if !covers_value {
                missing.push(inner.to_string());
            }
            Err(err(format!("match no exhaustivo sobre {inner}?: falta cubrir {missing:?} (GRAMMAR.md §3.9)")))
        }
    }

    /// Análogo a `collect_union_coverage`, pero con dos `bool` en vez de un
    /// `Vec<bool>` por miembro -- un opcional no es una lista de tipos
    /// distinguibles, es "T o ausente", así que la cobertura es binaria.
    fn collect_optional_coverage(
        &self,
        pattern: &Pattern,
        inner: &Type,
        wildcard: &mut bool,
        covers_null: &mut bool,
        covers_value: &mut bool,
    ) -> Result<(), CheckError> {
        match pattern {
            Pattern::Bind(_) => {
                *wildcard = true;
                Ok(())
            }
            Pattern::Literal(LiteralPattern::Null) => {
                *covers_null = true;
                Ok(())
            }
            Pattern::Type(_, texpr) => {
                let resolved = self.resolve_type(texpr)?;
                if is_subtype(&resolved, inner) && is_subtype(inner, &resolved) {
                    *covers_value = true;
                    Ok(())
                } else {
                    Err(err(format!(
                        "el patrón de tipo '{resolved}' no corresponde al tipo interno de este opcional ({inner}?)"
                    )))
                }
            }
            Pattern::Or(subs) => {
                for s in subs {
                    self.collect_optional_coverage(s, inner, wildcard, covers_null, covers_value)?;
                }
                Ok(())
            }
            Pattern::Literal(lit) => Err(err(format!(
                "patrón literal {lit:?} no válido contra un escrutinio T? -- usá 'null' o 'nombre: Tipo' (GRAMMAR.md §3.9)"
            ))),
            Pattern::Variant { enum_name, .. } => Err(err(format!(
                "patrón de variante de enum ('{enum_name}') no válido contra un escrutinio T? -- usá \
                 'null' o 'nombre: Tipo' (GRAMMAR.md §3.9)"
            ))),
        }
    }

    fn check_literal_matches_type(&self, lit: &LiteralPattern, ty: &Type) -> Result<(), CheckError> {
        let ok = matches!(
            (lit, ty),
            // Un patrón literal entero (`5 => ...`) vale contra un
            // escrutinio Int64 igual que contra Int -- no hay una sintaxis
            // de literal Int64 propia (GRAMMAR.md §3.30), así que el mismo
            // LiteralPattern::Int sirve para ambos tipos de escrutinio.
            (LiteralPattern::Int(_), Type::Int | Type::Int64)
                | (LiteralPattern::Str(_), Type::String)
                | (LiteralPattern::Bool(_), Type::Bool)
                // `null` solo tiene sentido contra un escrutinio opcional --
                // ver check_match/check_exhaustive_optional, que es el único
                // lugar que llega acá con un `Type::Optional` de verdad.
                | (LiteralPattern::Null, Type::Optional(_))
        );
        if ok {
            Ok(())
        } else {
            Err(err(format!("el patrón literal {lit:?} no coincide con el tipo del escrutinio ({ty})")))
        }
    }

    pub(crate) fn enum_variant_names(&self, name: &str) -> Result<Vec<String>, CheckError> {
        self.enums
            .get(name)
            .map(|e| e.variants.iter().map(|v| v.name.clone()).collect())
            .ok_or_else(|| err(format!("enum desconocido: '{name}'")))
    }

    /// Da tipo a las variables que un patrón introduce, recursivamente —
    /// `Enum.Variante { campo: patrón_anidado }` puede anidar otro patrón.
    fn bind_pattern(&self, pattern: &Pattern, ty: &Type, env: &mut Env) -> Result<(), CheckError> {
        match pattern {
            Pattern::Bind(name) => {
                env.insert(name.clone(), immutable(ty.clone()));
                Ok(())
            }
            Pattern::Literal(lit) => self.check_literal_matches_type(lit, ty),
            Pattern::Variant { enum_name, variant_name, fields } => {
                let variant_fields = self.variant_field_types(ty, enum_name, variant_name)?;
                if let Some(fps) = fields {
                    for fp in fps {
                        let field_ty = variant_fields
                            .iter()
                            .find(|(n, _)| n == &fp.name)
                            .map(|(_, t)| t.clone())
                            .ok_or_else(|| err(format!("'{enum_name}.{variant_name}' no tiene campo '{}'", fp.name)))?;
                        self.bind_pattern(&fp.pattern, &field_ty, env)?;
                    }
                }
                Ok(())
            }
            // Alcance v0 (ver doc de Pattern::Or en ast.rs): ninguna
            // alternativa puede ligar nombres -- evita tener que reconciliar
            // "las N ramas bindean las mismas variables del mismo tipo"
            // (la parte cara de or-patterns en otros lenguajes).
            Pattern::Or(subs) => {
                for s in subs {
                    if !pattern_bindings(s).is_empty() {
                        return Err(err(
                            "las alternativas de un patrón 'A | B' no pueden introducir bindings (GRAMMAR.md §3.3) \
                             -- ninguna rama puede usar un nombre propio ni capturar un campo, solo literales o \
                             variantes sin capturar",
                        ));
                    }
                    self.bind_pattern(s, ty, env)?;
                }
                Ok(())
            }
            // Genérico a propósito -- no valida membership contra ninguna
            // lista de miembros de unión: para cuando el escrutinio es
            // Type::Union, `check_exhaustive_union` ya validó eso ANTES de
            // que el loop de arms llegue a bindear (checker.rs::check_match),
            // así que acá alcanza con resolver y ligar. Escribirlo así,
            // sin pedir una `Type::Union` en particular, es también lo que
            // permite que un `Pattern::Type` funcione anidado dentro de un
            // `FieldPattern` (`Enum.Variante { campo: p: Int }`), no solo
            // como patrón de tope de un match sobre unión.
            Pattern::Type(name, texpr) => {
                let resolved = self.resolve_type(texpr)?;
                env.insert(name.clone(), immutable(resolved));
                Ok(())
            }
        }
    }

    pub(crate) fn variant_field_types(
        &self,
        scrutinee_ty: &Type,
        enum_name: &str,
        variant_name: &str,
    ) -> Result<Vec<(String, Type)>, CheckError> {
        if let Type::ResultOf(ok_ty, err_ty) = scrutinee_ty {
            return match variant_name {
                "Ok" => Ok(vec![("value".to_string(), (**ok_ty).clone())]),
                "Err" => Ok(vec![("error".to_string(), (**err_ty).clone())]),
                other => Err(err(format!("Result no tiene variante '{other}'"))),
            };
        }
        // Enum genérico instanciado (GRAMMAR.md §3.6): arma el subst
        // type_param->arg concreto y resuelve los campos de la variante
        // con ESE subst, igual que expand_generic_struct para structs.
        if let Type::Generic(base_name, args) = scrutinee_ty {
            let decl = self
                .enums
                .get(base_name.as_str())
                .ok_or_else(|| err(format!("enum desconocido: '{base_name}'")))?;
            let variant = decl
                .variants
                .iter()
                .find(|v| v.name == variant_name)
                .ok_or_else(|| err(format!("'{base_name}' no tiene variante '{variant_name}'")))?;
            let subst: HashMap<String, Type> =
                decl.type_params.iter().cloned().zip(args.iter().cloned()).collect();
            let mut out = Vec::new();
            if let Some(fields) = &variant.fields {
                for f in fields {
                    out.push((f.name.clone(), self.resolve_type_subst(&f.ty, &subst)?));
                }
            }
            return Ok(out);
        }
        let decl = self
            .enums
            .get(enum_name)
            .ok_or_else(|| err(format!("enum desconocido: '{enum_name}'")))?;
        let variant = decl
            .variants
            .iter()
            .find(|v| v.name == variant_name)
            .ok_or_else(|| err(format!("'{enum_name}' no tiene variante '{variant_name}'")))?;
        let mut out = Vec::new();
        if let Some(fields) = &variant.fields {
            for f in fields {
                out.push((f.name.clone(), self.resolve_type(&f.ty)?));
            }
        }
        Ok(out)
    }

    // ---- síntesis (modo ⇒) ----

    /// Wrapper delgado -- mismo criterio que `check_expr`/`check_expr_inner`:
    /// estampa el span de `e` en cualquier error sin span propio, sin pisar
    /// uno más profundo que ya haya estampado una sub-expresión.
    fn synth_expr(&self, e: &Spanned<Expr>, env: &Env) -> Result<Type, CheckError> {
        let result = self.synth_expr_inner(&e.node, env).map_err(|ce| ce.with_span(e.span));
        // Hover (GRAMMAR.md §3.24): ver `probe_hover` para el criterio de
        // qué nodo gana cuando varios spans anidados contienen el offset.
        self.probe_hover(e.span, || result.as_ref().ok().cloned());
        result
    }

    fn synth_expr_inner(&self, e: &Expr, env: &Env) -> Result<Type, CheckError> {
        match e {
            Expr::Int(_) => Ok(Type::Int),
            Expr::Float(_) => Ok(Type::Float),
            Expr::Str(_) => Ok(Type::String),
            Expr::Bool(_) => Ok(Type::Bool),
            Expr::Null => Ok(Type::Null),
            Expr::Ident(name) => {
                // El lookup de variables va PRIMERO -- antes, "db" se
                // chequeaba acá arriba de todo, así que un `let db = ...`
                // de un usuario quedaba sombreado en silencio por el
                // builtin (hallado al diseñar "DB tipada", GRAMMAR.md §2.1).
                if let Some(b) = env.get(name) {
                    return Ok(b.ty.clone());
                }
                if name == "db" {
                    return Ok(Type::Db);
                }
                if name == "auth" {
                    return Ok(Type::Auth);
                }
                if name == "math" {
                    return Ok(Type::Math);
                }
                if name == "crypto" {
                    return Ok(Type::Crypto);
                }
                if name == "http" {
                    return Ok(Type::Http);
                }
                if name == "json" {
                    return Ok(Type::Json);
                }
                if name == "base64" {
                    return Ok(Type::Base64);
                }
                if name == "pdf" {
                    return Ok(Type::Pdf);
                }
                if name == "excel" {
                    return Ok(Type::Excel);
                }
                if name == "ai" {
                    return Ok(Type::Ai);
                }
                if name == "mcp" {
                    return Ok(Type::Mcp);
                }
                if name == "env" {
                    return Ok(Type::Env);
                }
                if name == "request" {
                    return Ok(Type::Request);
                }
                if name == "smtp" {
                    return Ok(Type::Smtp);
                }
                if name == "response" {
                    return Ok(Type::Response);
                }
                if name == "now" {
                    return Ok(Type::Function(vec![], Box::new(Type::Timestamp)));
                }
                // GRAMMAR.md §3.90: cierra el límite que quedaba abierto en
                // §3.31 ("un Timestamp solo llega de un rpc o de la base,
                // nunca se CONSTRUYE arbitrariamente adentro del backend")
                // -- `now()` solo da el INSTANTE actual, esto da cualquier
                // fecha/hora de calendario (ej. el límite de un trimestre).
                if name == "dateFromParts" {
                    return Ok(Type::Function(
                        vec![Type::Int, Type::Int, Type::Int, Type::Int, Type::Int, Type::Int],
                        Box::new(Type::Timestamp),
                    ));
                }
                // GRAMMAR.md §3.116: builtins sin receptor, mismo criterio
                // que `dateFromParts` -- "helper que devuelve String", no un
                // motor de templates nuevo. `sitemapXml` arma un
                // `sitemap.xml` bien formado (protocolo sitemaps.org);
                // `robotsTxt` arma un `robots.txt` bien formado, con
                // `Sitemap: <url>` al final si se pasa una.
                if name == "sitemapXml" {
                    return Ok(Type::Function(vec![Type::List(Box::new(sitemap_url_type()))], Box::new(Type::String)));
                }
                if name == "robotsTxt" {
                    return Ok(Type::Function(
                        vec![Type::List(Box::new(robots_rule_type())), Type::Optional(Box::new(Type::String))],
                        Box::new(Type::String),
                    ));
                }
                // GRAMMAR.md §3.117: mismo criterio que sitemapXml/robotsTxt
                // -- "helper que devuelve String" para el resto de PLAN.md
                // §9.9 (metadata SEO clásica). `jsonLd` acepta `Dynamic`
                // porque un dato JSON-LD real (schema.org) no tiene una
                // forma fija que el checker pueda exigir de antemano.
                if name == "metaTags" {
                    return Ok(Type::Function(vec![Type::List(Box::new(meta_tag_type()))], Box::new(Type::String)));
                }
                if name == "openGraphTags" {
                    return Ok(Type::Function(vec![Type::List(Box::new(open_graph_tag_type()))], Box::new(Type::String)));
                }
                if name == "canonicalLink" {
                    return Ok(Type::Function(vec![Type::String], Box::new(Type::String)));
                }
                if name == "jsonLd" {
                    return Ok(Type::Function(vec![Type::Dynamic], Box::new(Type::String)));
                }
                // GRAMMAR.md §3.222: las rutas ESTÁTICAS del propio programa
                // (todo `@route` sin `:param` ni catch-all, sin auth) como
                // `{loc}[]` listo para `sitemapXml`, y los `<link
                // rel="alternate" hreflang>` de un sitio multi-idioma.
                if name == "staticRoutes" {
                    return Ok(Type::Function(vec![Type::String], Box::new(Type::List(Box::new(static_route_type())))));
                }
                if name == "hreflangLinks" {
                    return Ok(Type::Function(vec![Type::List(Box::new(hreflang_link_type()))], Box::new(Type::String)));
                }
                if name == "assert" {
                    return Ok(Type::Function(vec![Type::Bool], Box::new(Type::Void)));
                }
                if name == "panic" {
                    return Ok(Type::Function(vec![Type::String], Box::new(Type::Void)));
                }
                if self.services.contains_key(name) {
                    return Ok(Type::Service(name.clone()));
                }
                // Un `const` de nivel superior es visible desde cualquier
                // cuerpo, igual que una `fn`. Faltaba: se declaraba y se
                // emitía, pero usarlo daba "variable no declarada".
                if let Some(c) = self.consts.get(name) {
                    return self.resolve_type(&c.ty);
                }
                if let Some((params, ret)) = self.fns.get(name) {
                    return Ok(Type::Function(params.clone(), Box::new(ret.clone())));
                }
                let mut candidates: Vec<&str> =
                    vec!["db", "auth", "now", "dateFromParts", "assert", "panic", "sitemapXml", "robotsTxt", "metaTags", "openGraphTags", "canonicalLink", "jsonLd", "staticRoutes", "hreflangLinks"];
                candidates.extend(env.keys().map(String::as_str));
                candidates.extend(self.consts.keys().map(String::as_str));
                candidates.extend(self.fns.keys().map(String::as_str));
                candidates.extend(self.services.keys().map(String::as_str));
                if let Some(sug) = find_best_suggestion(name, candidates) {
                    Err(err(format!("variable no declarada: '{name}' -- ¿quisiste decir '{sug}'?")))
                } else {
                    Err(err(format!("variable no declarada: '{name}'")))
                }
            }
            Expr::FieldAccess { base, field } => {
                // GRAMMAR.md §3.209 (evolución de §3.206): `Enum.Variante`
                // sin `{}` en posición de EXPRESIÓN -- el parser SIEMPRE
                // parsea esto como `FieldAccess` (nunca tiene tabla de
                // símbolos para elegir otra forma en el momento), así que
                // la desambiguación real pasa acá, en el checker, no en el
                // parser. `!env.contains_key` primero: una variable local
                // que sombree el nombre del enum sigue resolviendo como
                // variable, igual que el chequeo de `db` de más abajo
                // respeta esa misma prioridad.
                if let Expr::Ident(base_name) = &base.node {
                    if !env.contains_key(base_name) {
                        if let Some(decl) = self.enums.get(base_name) {
                            match decl.variants.iter().find(|v| &v.name == field) {
                                // Variante SIN campos (unitaria, o `{}`
                                // explícita pero vacía): `Enum.Variante` es
                                // azúcar por `Enum.Variante {}` -- exactamente
                                // la misma construcción, delegada al mismo
                                // camino que ya la tipa (StructLit), para que
                                // las dos formas nunca puedan divergir en
                                // comportamiento. Antes de esta ronda esto
                                // era un error pidiendo agregar `{}`; ahora
                                // `{}` es opcional cuando no hay nada que
                                // agregar.
                                Some(variant) if variant.fields.as_ref().is_none_or(|fs| fs.is_empty()) => {
                                    return self.synth_expr_inner(
                                        &Expr::StructLit { name: base_name.clone(), variant: Some(field.clone()), fields: vec![] },
                                        env,
                                    );
                                }
                                // Variante CON campos: no hay de dónde
                                // inferir los valores sin llaves, así que
                                // acá SÍ sigue siendo un error -- pero
                                // dirigido, no el "variable no declarada"
                                // genérico de antes de §3.206.
                                Some(_) => {
                                    return Err(err(format!(
                                        "'{base_name}.{field}' es una variante de enum CON campos -- no se puede usar sin llaves, no hay de dónde inferir sus valores: escribí '{base_name}.{field} {{ ... }}' (GRAMMAR.md §3.209)"
                                    )).with_code("L0001"));
                                }
                                // `field` no nombra ninguna variante real de
                                // este enum -- ya sabemos que `base_name` es
                                // un enum, no una variable, así que "variable
                                // no declarada" sería la respuesta equivocada
                                // sin importar qué diga `field`.
                                None => {
                                    let variant_names: Vec<&str> = decl.variants.iter().map(|v| v.name.as_str()).collect();
                                    return Err(match find_best_suggestion(field, variant_names) {
                                        Some(sug) => err(format!(
                                            "'{base_name}' no tiene ninguna variante '{field}' -- ¿quisiste decir '{base_name}.{sug}'?"
                                        ))
                                        .with_code("L0002"),
                                        None => err(format!("'{base_name}' no tiene ninguna variante '{field}'")).with_code("L0002"),
                                    });
                                }
                            }
                        }
                    }
                }
                let base_ty = self.synth_expr(base, env)?;
                match base_ty {
                    Type::Dynamic => Ok(Type::Dynamic),
                    Type::Service(s_name) => {
                        let methods = self.services.get(&s_name).ok_or_else(|| err(format!("service desconocido: '{s_name}'")))?;
                        if let Some((params, ret)) = methods.get(field.as_str()) {
                            Ok(Type::Function(params.clone(), Box::new(ret.clone())))
                        } else if let Some(sug) = find_best_suggestion(field, methods.keys().map(String::as_str)) {
                            Err(err(format!("el service '{s_name}' no tiene ningún rpc '{field}' -- ¿quisiste decir '{sug}'?")))
                        } else {
                            Err(err(format!("el service '{s_name}' no tiene ningún rpc '{field}'")))
                        }
                    }
                    Type::Struct { fields, .. } => {
                        if let Some(f) = fields.iter().find(|f| &f.name == field) {
                            Ok(field_access_ty(f))
                        } else if let Some(sug) = find_best_suggestion(field, fields.iter().map(|f| f.name.as_str())) {
                            Err(err(format!("el struct no tiene campo '{field}' -- ¿quisiste decir '{sug}'?")))
                        } else {
                            Err(err(format!("el struct no tiene campo '{field}'")))
                        }
                    }
                    // struct genérico instanciado, ej. una variable Box<Int>
                    Type::Generic(name, args) => {
                        let s_fields = self.expand_generic_struct(&name, &args)?;
                        if let Some(f) = s_fields.iter().find(|f| &f.name == field) {
                            Ok(field_access_ty(f))
                        } else if let Some(sug) = find_best_suggestion(field, s_fields.iter().map(|f| f.name.as_str())) {
                            Err(err(format!("el struct no tiene campo '{field}' -- ¿quisiste decir '{sug}'?")))
                        } else {
                            Err(err(format!("el struct no tiene campo '{field}'")))
                        }
                    }
                    // `db.<coleccion>` -- nombre desconocido ya es un error
                    // acá mismo, no `Dynamic` dejando pasar cualquier cosa.
                    Type::Db => {
                        if let Some(element_ty) = self.db_collections.get(field.as_str()) {
                            Ok(Type::DbCollection(Box::new(element_ty.clone())))
                        } else if let Some(sug) = find_best_suggestion(field, self.db_collections.keys().map(String::as_str)) {
                            Err(err(format!("'db' no tiene ninguna colección llamada '{field}' -- ¿quisiste decir '{sug}'?")))
                        } else {
                            Err(err(format!("'db' no tiene ninguna colección llamada '{field}'")))
                        }
                    }
                    // `T?` aparte: es el error que mas se comete, porque en
                    // TypeScript `if (x != null)` SI angosta y acá no
                    // (GRAMMAR.md §3.4). Decir solo "no se puede" deja a quien
                    // lo lee -- humano o modelo -- probando variantes que
                    // tampoco existen. `if x != null { x.campo }` sigue sin
                    // angostar (a propósito, ver §3.4) -- pero desde §3.9 SÍ
                    // hay dos formas reales de leer el valor: `match`
                    // narrowing (`match x { v: {inner} => v.{field}, null => ... }`)
                    // o, si el caso es "dame un default", `x ?? default`.
                    Type::Optional(inner) => Err(err(format!(
                        "no se puede acceder al campo '{field}' sobre {inner}?: un valor nullable no se angosta con `if x != null` (GRAMMAR.md §3.4). Usá 'match' para desarmarlo de verdad -- `match x {{ v: {inner} => v.{field}, null => ... }}` (GRAMMAR.md §3.9) -- o devolvé el {inner}? tal cual y desarmalo del lado de TypeScript, que también angosta `{inner} | null`"
                    )).with_code("L0004")),
                    other => Err(err(format!("no se puede acceder al campo '{field}' sobre {other}"))),
                }
            }
            Expr::Call { callee, args } => {
                if let Expr::Ident(name) = &callee.node {
                    if name == "assert" && !env.contains_key("assert") && !self.fns.contains_key("assert") {
                        if args.is_empty() || args.len() > 2 {
                            return Err(err("'assert' toma 1 o 2 argumentos: assert(cond: Bool, [msg: String])"));
                        }
                        self.check_expr(&args[0], &Type::Bool, env)?;
                        if let Some(msg) = args.get(1) {
                            self.check_expr(msg, &Type::String, env)?;
                        }
                        return Ok(Type::Void);
                    }
                }
                if let Some(ty) = self.try_builtin_method(callee, args, env)? {
                    return Ok(ty);
                }
                let callee_ty = self.synth_expr(callee, env)?;
                match callee_ty {
                    Type::Dynamic => {
                        for a in args {
                            self.synth_expr(a, env)?;
                        }
                        Ok(Type::Dynamic)
                    }
                    Type::Function(params, ret) => {
                        if params.len() != args.len() {
                            return Err(err(format!(
                                "se esperaban {} argumentos, se dieron {}",
                                params.len(),
                                args.len()
                            )));
                        }
                        for (a, p) in args.iter().zip(&params) {
                            self.check_expr(a, p, env)?;
                        }
                        Ok(*ret)
                    }
                    other => Err(err(format!("no se puede llamar un valor de tipo {other}"))),
                }
            }
            Expr::StructLit { name, variant, fields } => {
                self.synth_struct_lit(name, variant.as_deref(), fields, env)
            }
            Expr::Match { .. } => Err(err(
                "'match' en posición de síntesis no soportado — necesita un tipo esperado del contexto (GRAMMAR.md §3.1, regla Match es de modo chequeo)",
            )),
            Expr::If { .. } => Err(err(
                "'if' en posición de síntesis no soportado — necesita un tipo esperado del contexto (GRAMMAR.md §3.7, misma familia que match)",
            )),
            Expr::Transaction { .. } => Err(err(
                "'transaction' en posición de síntesis no soportado — necesita un tipo esperado del contexto (GRAMMAR.md §3.154, misma familia que if/match)",
            )),
            Expr::Binary { op, left, right } => self.synth_binary(*op, left, right, env),
            Expr::Unary { op, operand } => self.synth_unary(*op, operand, env),
            // Un array vacío no sintetiza -- no hay de dónde inferir el
            // tipo del elemento (GRAMMAR.md §2.3). Eso vive en check_expr.
            Expr::ArrayLit(items) => {
                let mut iter = items.iter();
                let Some(first) = iter.next() else {
                    return Err(err(
                        "un array vacío '[]' no se puede sintetizar sin un tipo esperado (ej. anotá el 'let': let xs: Int[] = [])",
                    ));
                };
                let elem_ty = self.synth_expr(first, env)?;
                for item in iter {
                    self.check_expr(item, &elem_ty, env)?;
                }
                Ok(Type::List(Box::new(elem_ty)))
            }
            Expr::Index { base, index } => {
                let base_ty = self.synth_expr(base, env)?;
                self.check_expr(index, &Type::Int, env)?;
                match base_ty {
                    Type::List(elem_ty) => Ok(*elem_ty),
                    Type::Dynamic => Ok(Type::Dynamic),
                    other => Err(err(format!("no se puede indexar un valor de tipo {other} (se esperaba una lista)"))),
                }
            }
            Expr::TupleLit(items) => {
                let mut tys = Vec::new();
                for item in items {
                    tys.push(self.synth_expr(item, env)?);
                }
                Ok(Type::Tuple(tys))
            }
            Expr::TupleIndex { base, index } => {
                let base_ty = self.synth_expr(base, env)?;
                match base_ty {
                    Type::Tuple(items) => items.get(*index).cloned().ok_or_else(|| {
                        err(format!(
                            "índice de tupla .{index} fuera de rango (tiene {} elementos)",
                            items.len()
                        ))
                    }),
                    Type::Dynamic => Ok(Type::Dynamic),
                    other => Err(err(format!("'.{index}' requiere una tupla, se encontró {other}"))),
                }
            }
            Expr::Paren(inner) => self.synth_expr(inner, env),
            // Sin ningún `Type::Function` esperado del contexto (a
            // diferencia de `check_closure`), la ÚNICA forma de saber el
            // tipo de cada parámetro es que el usuario lo haya anotado --
            // de ahí el error explícito si falta, en vez de un mensaje
            // confuso de "no se pudo resolver" más abajo.
            Expr::Closure { params, body } => {
                let mut param_tys = Vec::new();
                let mut local = env.clone();
                for p in params {
                    let Some(texpr) = &p.ty else {
                        return Err(err(format!(
                            "el parámetro '{}' de este closure necesita anotación de tipo -- sin un tipo de función esperado del contexto (ej. como argumento de '.filter'/'.map', o un 'let' con tipo declarado), cada parámetro tiene que anotarse (GRAMMAR.md §3.10)",
                            p.name
                        )));
                    };
                    let ty = self.resolve_type(texpr)?;
                    local.insert(p.name.clone(), immutable(ty.clone()));
                    param_tys.push(ty);
                }
                let ret_ty = self.synth_block(body, &local)?;
                Ok(Type::Function(param_tys, Box::new(ret_ty)))
            }
        }
    }

    /// GRAMMAR.md §3.7 — sin coerción implícita: Int+Int o Float+Float, no
    /// mezclados. `Dynamic` (el escape hatch de `db`, ver types.rs) sigue
    /// siendo compatible con cualquier operando, igual que en el resto del
    /// checker.
    fn synth_binary(&self, op: BinaryOp, left: &Spanned<Expr>, right: &Spanned<Expr>, env: &Env) -> Result<Type, CheckError> {
        use BinaryOp::*;
        match op {
            // '+' es el único aritmético que también sirve para concatenar
            // strings -- resta/multiplicación/división sobre texto no
            // tienen un significado razonable, así que quedan aparte.
            Add => {
                let l = self.synth_expr(left, env)?;
                let r = self.synth_expr(right, env)?;
                match (&l, &r) {
                    (Type::Int, Type::Int) => Ok(Type::Int),
                    (Type::Int64, Type::Int64) => Ok(Type::Int64),
                    (Type::Decimal, Type::Decimal) => Ok(Type::Decimal),
                    (Type::Float, Type::Float) => Ok(Type::Float),
                    (Type::String, Type::String) => Ok(Type::String),
                    // PLAN.md §9.14 ítem 2: concatenación pura de listas del
                    // mismo tipo de elemento -- combinada con `let mut`/
                    // reasignación (ya existente, ver ast.rs) resuelve
                    // "acumular una lista creciendo en un loop" sin inventar
                    // ningún mecanismo de mutación nuevo. A diferencia de
                    // `.sum()`, concatenar nunca necesita inspeccionar el
                    // tipo de elemento en runtime -- así que no hereda la
                    // ambigüedad de lista vacía que `.sum()` sí tiene.
                    (Type::List(a), Type::List(b)) if a == b => Ok(Type::List(a.clone())),
                    (Type::Dynamic, _) | (_, Type::Dynamic) => Ok(Type::Dynamic),
                    _ => Err(err(format!(
                        "'+' requiere Int+Int, Int64+Int64, Decimal+Decimal, Float+Float, String+String o List<T>+List<T> (mismo T) sin mezclar (GRAMMAR.md §3.7); se encontró {l:?} y {r:?}"
                    ))),
                }
            }
            // GRAMMAR.md §3.184: Decimal entra en Sub/Mul/Div (aritmética
            // exacta con redondeo half-up al re-escalar, ver runtime/mod.rs)
            // pero NO en Rem -- un resto sobre un decimal escalado no tiene
            // una semántica bien definida todavía, y aceptarlo acá sin
            // implementarlo en runtime sería el mismo tipo de desacuerdo
            // checker/runtime que este proyecto siempre trata como bug.
            Rem => {
                let l = self.synth_expr(left, env)?;
                let r = self.synth_expr(right, env)?;
                match (&l, &r) {
                    (Type::Int, Type::Int) => Ok(Type::Int),
                    (Type::Int64, Type::Int64) => Ok(Type::Int64),
                    (Type::Float, Type::Float) => Ok(Type::Float),
                    (Type::Dynamic, _) | (_, Type::Dynamic) => Ok(Type::Dynamic),
                    _ => Err(err(format!(
                        "'%' requiere Int+Int, Int64+Int64 o Float+Float sin mezclar -- Decimal no soporta '%' (GRAMMAR.md §3.184); se encontró {l:?} y {r:?}"
                    ))),
                }
            }
            Sub | Mul | Div => {
                let l = self.synth_expr(left, env)?;
                let r = self.synth_expr(right, env)?;
                match (&l, &r) {
                    (Type::Int, Type::Int) => Ok(Type::Int),
                    (Type::Int64, Type::Int64) => Ok(Type::Int64),
                    (Type::Decimal, Type::Decimal) => Ok(Type::Decimal),
                    (Type::Float, Type::Float) => Ok(Type::Float),
                    (Type::Dynamic, _) | (_, Type::Dynamic) => Ok(Type::Dynamic),
                    _ => Err(err(format!(
                        "operador aritmético requiere Int+Int, Int64+Int64, Decimal+Decimal o Float+Float sin mezclar (GRAMMAR.md §3.7); se encontró {l:?} y {r:?}"
                    ))),
                }
            }
            Eq | NotEq => {
                let l = self.synth_expr(left, env)?;
                let r = self.synth_expr(right, env)?;
                if type_contains_function(&l) || type_contains_function(&r) {
                    return Err(err(
                        "'==' / '!=' no están definidos sobre valores de tipo función/closure (GRAMMAR.md §3.10)",
                    ));
                }
                // Comparables si son mutuamente compatibles (mismo tipo, o
                // uno de los dos Dynamic) -- no solo primitivos: dos enums
                // nominales del mismo tipo también se pueden comparar.
                if matches!(l, Type::Dynamic) || matches!(r, Type::Dynamic) || is_subtype(&l, &r) || is_subtype(&r, &l)
                {
                    Ok(Type::Bool)
                } else {
                    Err(err(format!(
                        "'==' / '!=' requieren operandos de tipos compatibles; se encontró {l:?} y {r:?}"
                    )))
                }
            }
            Lt | LtEq | Gt | GtEq => {
                let l = self.synth_expr(left, env)?;
                let r = self.synth_expr(right, env)?;
                match (&l, &r) {
                    (Type::Int, Type::Int)
                    | (Type::Int64, Type::Int64)
                    | (Type::Decimal, Type::Decimal)
                    | (Type::Float, Type::Float)
                    // Timestamp SOLO entra acá (comparación/orden) -- sin
                    // aritmética, sin Neg (GRAMMAR.md §3.31): no hay
                    // arriba/abajo simétrico como con un número.
                    | (Type::Timestamp, Type::Timestamp) => Ok(Type::Bool),
                    (Type::Dynamic, _) | (_, Type::Dynamic) => Ok(Type::Bool),
                    _ => Err(err(format!(
                        "operador relacional requiere Int+Int, Int64+Int64, Decimal+Decimal, Float+Float o Timestamp+Timestamp; se encontró {l:?} y {r:?}"
                    ))),
                }
            }
            And | Or => {
                self.check_expr(left, &Type::Bool, env)?;
                self.check_expr(right, &Type::Bool, env)?;
                Ok(Type::Bool)
            }
            // `a ?? b` (GRAMMAR.md §3.9): `a` tiene que ser de verdad `T?` --
            // si ya no es opcional no hay nada que este operador aporte, y
            // dejarlo pasar en silencio (evaluando siempre a `a`) escondería
            // ese error de tipeo en vez de señalarlo. `b` acepta DOS formas:
            // el `T` desenvuelto (el caso común, "dame un default seguro" --
            // el resultado queda definitivo, ya no opcional) o el MISMO `T?`
            // (para encadenar `a ?? b ?? default`, donde `b` todavía puede
            // ser null -- el resultado sigue siendo `T?` hasta que algún
            // eslabón sea definitivo). `a ?? b ?? c` asocia a izquierda, así
            // que esto se resuelve solo, sin ningún caso especial para la
            // cadena completa: cada `??` mira nada más que sus dos operandos
            // inmediatos.
            Coalesce => {
                let l = self.synth_expr(left, env)?;
                match l {
                    Type::Optional(inner) => {
                        let r = self.synth_expr(right, env)?;
                        if is_subtype(&r, &inner) && is_subtype(&inner, &r) {
                            Ok(*inner)
                        } else if let Type::Optional(r_inner) = &r {
                            if is_subtype(r_inner, &inner) && is_subtype(&inner, r_inner) {
                                Ok(Type::Optional(inner))
                            } else {
                                Err(err(format!(
                                    "'??' requiere que el lado derecho sea {inner} o {inner}?; se encontró {r}"
                                )))
                            }
                        } else {
                            Err(err(format!("'??' requiere que el lado derecho sea {inner} o {inner}?; se encontró {r}")))
                        }
                    }
                    Type::Dynamic => {
                        self.synth_expr(right, env)?;
                        Ok(Type::Dynamic)
                    }
                    other => Err(err(format!(
                        "'??' requiere que el lado izquierdo sea un tipo opcional (T?); se encontró {other} -- si ya no es opcional, usalo directo sin '??'"
                    ))),
                }
            }
        }
    }

    fn synth_unary(&self, op: UnaryOp, operand: &Spanned<Expr>, env: &Env) -> Result<Type, CheckError> {
        match op {
            UnaryOp::Neg => {
                let t = self.synth_expr(operand, env)?;
                match t {
                    Type::Int | Type::Int64 | Type::Decimal | Type::Float | Type::Dynamic => Ok(t),
                    other => Err(err(format!("'-' unario requiere Int, Int64, Decimal o Float, se encontró {other}"))),
                }
            }
            UnaryOp::Not => {
                self.check_expr(operand, &Type::Bool, env)?;
                Ok(Type::Bool)
            }
        }
    }

    /// Reconoce `base.metodo(args)` como un builtin sobre un primitivo
    /// (GRAMMAR.md §3.8) antes de que el camino genérico intente resolver
    /// `callee` como FieldAccess normal (que fallaría: Int/Float/String no
    /// son Struct ni Dynamic). `Ok(None)` = no es un builtin conocido, seguí
    /// con el camino genérico de Call sin tocar nada.
    fn try_builtin_method(&self, callee: &Spanned<Expr>, args: &[Spanned<Expr>], env: &Env) -> Result<Option<Type>, CheckError> {
        let Expr::FieldAccess { base, field } = &callee.node else {
            return Ok(None);
        };
        let base_ty = self.synth_expr(base, env)?;
        // `db.<coleccion>.<metodo>(...)` -- a diferencia de los builtins de
        // primitivos de abajo, un nombre de método desconocido acá es
        // siempre un error, nunca `Ok(None)` (que dejaría que el camino
        // genérico de Call lo reintente y produzca un error más confuso).
        if let Type::DbCollection(element_ty) = &base_ty {
            return self.check_db_method(element_ty, field, args, env).map(Some);
        }
        // GRAMMAR.md §3.230: una consulta ya ordenada -- mismo criterio de
        // "método desconocido = error acá" que la colección de arriba.
        if let Type::DbQuery(element_ty) = &base_ty {
            return self.check_db_query_method(element_ty, field, args, env).map(Some);
        }
        // `auth.<metodo>(...)` (GRAMMAR.md §3.14, auth v0) -- mismo trato que
        // `db.<coleccion>.<metodo>`: un nombre de método desconocido acá es
        // siempre un error, nunca `Ok(None)`.
        if let Type::Auth = &base_ty {
            return self.check_auth_method(field, args, env).map(Some);
        }
        let ty = match (&base_ty, field.as_str()) {
            // `db.vacuum()`/`db.tableStats()` (GRAMMAR.md §3.151) -- a
            // diferencia de `db.<coleccion>.<metodo>(...)` (interceptado
            // arriba vía `Type::DbCollection`), estos son builtins sobre
            // `db` DIRECTO, sin colección de por medio -- mismo criterio de
            // "sin gramática nueva" que el resto de esta lista.
            (Type::Db, "vacuum") => {
                self.expect_no_args(args, "vacuum")?;
                Some(Type::Void)
            }
            (Type::Db, "tableStats") => {
                self.expect_no_args(args, "tableStats")?;
                Some(Type::MapOf(Box::new(Type::String), Box::new(Type::Int)))
            }
            (Type::Int, "toFloat") => {
                self.expect_no_args(args, "toFloat")?;
                Some(Type::Float)
            }
            // Ambas direcciones son exactas (mismo rango i64), nunca lossy
            // -- a diferencia de toFloat/toInt entre Int y Float. Es la
            // ÚNICA forma de obtener un Int64 desde código fuente en v0: un
            // literal entero siempre sintetiza Type::Int (Expr::Int arriba),
            // nunca Type::Int64 directamente (GRAMMAR.md §3.30).
            (Type::Int, "toInt64") => {
                self.expect_no_args(args, "toInt64")?;
                Some(Type::Int64)
            }
            (Type::Int64, "toInt") => {
                self.expect_no_args(args, "toInt")?;
                Some(Type::Int)
            }
            (Type::Float, "toInt") => {
                self.expect_no_args(args, "toInt")?;
                Some(Type::Int)
            }
            // GRAMMAR.md §3.184: sin sintaxis de literal Decimal propia --
            // mismo criterio que Int64 (§3.30), `.toDecimal()` es la ÚNICA
            // forma de obtenerlo desde código fuente. `Int.toDecimal()` es
            // exacto (escala ×10.000 sin pérdida); `Float.toDecimal()`
            // redondea el f64 ya parseado al 4to decimal -- seguro en la
            // práctica para cualquier magnitud financiera real (la
            // precisión de f64 excede por muchísimo la resolución de 4
            // decimales), documentado como límite honesto en GRAMMAR.md.
            (Type::Int, "toDecimal") | (Type::Float, "toDecimal") => {
                self.expect_no_args(args, "toDecimal")?;
                Some(Type::Decimal)
            }
            (Type::Decimal, "toFloat") => {
                self.expect_no_args(args, "toFloat")?;
                Some(Type::Float)
            }
            (Type::Int, "toString")
            | (Type::Int64, "toString")
            | (Type::Decimal, "toString")
            | (Type::Float, "toString")
            | (Type::Bool, "toString")
            // `Uuid` -> `String`: la salida "downgrade" explícita para
            // cualquier operación de String que un Uuid no tiene por sí
            // mismo (concatenar, `.length()`, etc.) -- mismo criterio que
            // `.toInt64()`/`.toInt()`, nunca mezcla implícita entre los dos
            // tipos (GRAMMAR.md §3.70).
            | (Type::Uuid, "toString") => {
                self.expect_no_args(args, "toString")?;
                Some(Type::String)
            }
            // `.isSome()`/`.isNone()` (GRAMMAR.md §3.9): la rama de un `T?`
            // sin necesitar 'match' -- útil cuando el cuerpo solo necesita
            // SABER si hay valor, no leerlo (leer el valor sigue exigiendo
            // 'match', el único lugar que de verdad angosta a `T`).
            (Type::Optional(_), "isSome") | (Type::Optional(_), "isNone") => {
                self.expect_no_args(args, field)?;
                Some(Type::Bool)
            }
            (Type::String, "length") => {
                self.expect_no_args(args, "length")?;
                Some(Type::Int)
            }
            (Type::String, "contains") => {
                let [needle] = args else {
                    return Err(err("'contains' toma exactamente 1 argumento"));
                };
                self.check_expr(needle, &Type::String, env)?;
                Some(Type::Bool)
            }
            (Type::String, "startsWith") => {
                let [needle] = args else {
                    return Err(err("'startsWith' toma exactamente 1 argumento"));
                };
                self.check_expr(needle, &Type::String, env)?;
                Some(Type::Bool)
            }
            (Type::String, "endsWith") => {
                let [needle] = args else {
                    return Err(err("'endsWith' toma exactamente 1 argumento"));
                };
                self.check_expr(needle, &Type::String, env)?;
                Some(Type::Bool)
            }
            (Type::String, "trim") => {
                self.expect_no_args(args, "trim")?;
                Some(Type::String)
            }
            (Type::String, "toUpper") => {
                self.expect_no_args(args, "toUpper")?;
                Some(Type::String)
            }
            (Type::String, "toLower") => {
                self.expect_no_args(args, "toLower")?;
                Some(Type::String)
            }
            (Type::String, "escapeHtml") => {
                self.expect_no_args(args, "escapeHtml")?;
                Some(Type::String)
            }
            // GRAMMAR.md §3.198: slicing/replace/split/padding -- superficie
            // que faltaba, bloqueaba exports de texto real (fixed-width,
            // CSV-safe) en un adoptador real.
            (Type::String, "substring") => builtin_args!(
                self, args, env, "String.substring",
                [(start, "start: Int", Type::Int), (end, "end: Int", Type::Int)] -> Type::String
            ),
            (Type::String, "replace") => builtin_args!(
                self, args, env, "String.replace",
                [(target, "target: String", Type::String), (replacement, "replacement: String", Type::String)] -> Type::String
            ),
            (Type::String, "split") => builtin_args!(
                self, args, env, "String.split",
                [(separator, "separator: String", Type::String)] -> Type::List(Box::new(Type::String))
            ),
            (Type::String, "padStart") => builtin_args!(
                self, args, env, "String.padStart",
                [(length, "length: Int", Type::Int), (pad, "pad: String", Type::String)] -> Type::String
            ),
            (Type::String, "padEnd") => builtin_args!(
                self, args, env, "String.padEnd",
                [(length, "length: Int", Type::Int), (pad, "pad: String", Type::String)] -> Type::String
            ),
            (Type::Timestamp, "toMillis") => {
                self.expect_no_args(args, "toMillis")?;
                Some(Type::Int64)
            }
            (Type::Timestamp, "diffMillis") => {
                let [other] = args else {
                    return Err(err("'diffMillis' toma exactamente 1 argumento (other: Timestamp)"));
                };
                self.check_expr(other, &Type::Timestamp, env)?;
                Some(Type::Int64)
            }
            (Type::Timestamp, "toIsoString") => {
                self.expect_no_args(args, "toIsoString")?;
                Some(Type::String)
            }
            // GRAMMAR.md §3.196: aritmética de Timestamp -- Value::Timestamp
            // ya es milisegundos planos desde epoch, sumar/restar tiempo es
            // aritmética entera pura, sin lógica de calendario.
            (Type::Timestamp, "addMillis") => builtin_args!(
                self, args, env, "Timestamp.addMillis",
                [(n, "n: Int64", Type::Int64)] -> Type::Timestamp
            ),
            (Type::Timestamp, "addSeconds") => builtin_args!(
                self, args, env, "Timestamp.addSeconds",
                [(n, "n: Int", Type::Int)] -> Type::Timestamp
            ),
            (Type::Timestamp, "addMinutes") => builtin_args!(
                self, args, env, "Timestamp.addMinutes",
                [(n, "n: Int", Type::Int)] -> Type::Timestamp
            ),
            (Type::Timestamp, "addHours") => builtin_args!(
                self, args, env, "Timestamp.addHours",
                [(n, "n: Int", Type::Int)] -> Type::Timestamp
            ),
            (Type::Timestamp, "addDays") => builtin_args!(
                self, args, env, "Timestamp.addDays",
                [(n, "n: Int", Type::Int)] -> Type::Timestamp
            ),
            (Type::Math, "sqrt") => {
                let [arg] = args else {
                    return Err(err("'math.sqrt' toma exactamente 1 argumento (x: Float)"));
                };
                self.check_expr(arg, &Type::Float, env)?;
                Some(Type::Float)
            }
            (Type::Math, "abs") => {
                let [arg] = args else {
                    return Err(err("'math.abs' toma exactamente 1 argumento (x: Float)"));
                };
                self.check_expr(arg, &Type::Float, env)?;
                Some(Type::Float)
            }
            (Type::Math, "floor") => {
                let [arg] = args else {
                    return Err(err("'math.floor' toma exactamente 1 argumento (x: Float)"));
                };
                self.check_expr(arg, &Type::Float, env)?;
                Some(Type::Int)
            }
            (Type::Math, "ceil") => {
                let [arg] = args else {
                    return Err(err("'math.ceil' toma exactamente 1 argumento (x: Float)"));
                };
                self.check_expr(arg, &Type::Float, env)?;
                Some(Type::Int)
            }
            (Type::Math, "round") => {
                let [arg] = args else {
                    return Err(err("'math.round' toma exactamente 1 argumento (x: Float)"));
                };
                self.check_expr(arg, &Type::Float, env)?;
                Some(Type::Int)
            }
            (Type::Math, "min") => {
                let [a, b] = args else {
                    return Err(err("'math.min' toma exactamente 2 argumentos (a: Float, b: Float)"));
                };
                self.check_expr(a, &Type::Float, env)?;
                self.check_expr(b, &Type::Float, env)?;
                Some(Type::Float)
            }
            (Type::Math, "max") => {
                let [a, b] = args else {
                    return Err(err("'math.max' toma exactamente 2 argumentos (a: Float, b: Float)"));
                };
                self.check_expr(a, &Type::Float, env)?;
                self.check_expr(b, &Type::Float, env)?;
                Some(Type::Float)
            }
            (Type::Math, "pow") => {
                let [a, b] = args else {
                    return Err(err("'math.pow' toma exactamente 2 argumentos (base: Float, exp: Float)"));
                };
                self.check_expr(a, &Type::Float, env)?;
                self.check_expr(b, &Type::Float, env)?;
                Some(Type::Float)
            }
            (Type::Crypto, "hashSha256") => {
                let [data] = args else {
                    return Err(err("'crypto.hashSha256' toma exactamente 1 argumento (data: String)"));
                };
                self.check_expr(data, &Type::String, env)?;
                Some(Type::String)
            }
            (Type::Crypto, "hmacSha256") => {
                let [secret, message] = args else {
                    return Err(err("'crypto.hmacSha256' toma exactamente 2 argumentos (secret: String, message: String)"));
                };
                self.check_expr(secret, &Type::String, env)?;
                self.check_expr(message, &Type::String, env)?;
                Some(Type::String)
            }
            (Type::Crypto, "awsS3PresignedUrl") => {
                let [access_key_id, secret_access_key, region, bucket, object_key, expires_seconds] = args else {
                    return Err(err(
                        "'crypto.awsS3PresignedUrl' toma exactamente 6 argumentos (accessKeyId: String, secretAccessKey: String, region: String, bucket: String, objectKey: String, expiresSeconds: Int)",
                    ));
                };
                self.check_expr(access_key_id, &Type::String, env)?;
                self.check_expr(secret_access_key, &Type::String, env)?;
                self.check_expr(region, &Type::String, env)?;
                self.check_expr(bucket, &Type::String, env)?;
                self.check_expr(object_key, &Type::String, env)?;
                self.check_expr(expires_seconds, &Type::Int, env)?;
                Some(Type::String)
            }
            // GRAMMAR.md §3.194: mismo mecanismo SigV4 que `awsS3PresignedUrl`
            // (GET, arriba), método PUT y un `contentType` que se firma como
            // header adicional -- quien recibe la URL solo puede subir con
            // ESE Content-Type exacto, no cualquiera.
            (Type::Crypto, "awsS3PresignedUploadUrl") => builtin_args!(
                self, args, env, "crypto.awsS3PresignedUploadUrl",
                [
                    (access_key_id, "accessKeyId: String", Type::String),
                    (secret_access_key, "secretAccessKey: String", Type::String),
                    (region, "region: String", Type::String),
                    (bucket, "bucket: String", Type::String),
                    (object_key, "objectKey: String", Type::String),
                    (expires_seconds, "expiresSeconds: Int", Type::Int),
                    (content_type, "contentType: String", Type::String)
                ] -> Type::String
            ),
            (Type::Crypto, "randomToken") => {
                let [length] = args else {
                    return Err(err("'crypto.randomToken' toma exactamente 1 argumento (length: Int)"));
                };
                self.check_expr(length, &Type::Int, env)?;
                Some(Type::String)
            }
            // GRAMMAR.md §3.186: retrofit de prueba del fast-path
            // `builtin_args!` -- mensaje de error y comportamiento
            // IDÉNTICOS al arm manual que reemplaza (1 argumento).
            (Type::Crypto, "hashPassword") => builtin_args!(
                self, args, env, "crypto.hashPassword",
                [(pwd, "password: String", Type::String)] -> Type::String
            ),
            (Type::Crypto, "verifyPassword") => {
                let [pwd, hash] = args else {
                    return Err(err("'crypto.verifyPassword' toma exactamente 2 argumentos (password: String, hash: String)"));
                };
                self.check_expr(pwd, &Type::String, env)?;
                self.check_expr(hash, &Type::String, env)?;
                Some(Type::Bool)
            }
            (Type::Crypto, "isLegacyHash") => {
                let [hash] = args else {
                    return Err(err("'crypto.isLegacyHash' toma exactamente 1 argumento (hash: String)"));
                };
                self.check_expr(hash, &Type::String, env)?;
                Some(Type::Bool)
            }
            (Type::Crypto, "uuid") => {
                self.expect_no_args(args, "uuid")?;
                Some(Type::Uuid)
            }
            // GRAMMAR.md §3.186: retrofit de prueba del fast-path
            // `builtin_args!` -- mensaje de error y comportamiento
            // IDÉNTICOS al arm manual que reemplaza (2 argumentos, cubre
            // la concordancia plural del mensaje que "hashPassword" no
            // ejercita).
            (Type::Crypto, "randomInt") => builtin_args!(
                self, args, env, "crypto.randomInt",
                [(min, "min: Int", Type::Int), (max, "max: Int", Type::Int)] -> Type::Int
            ),
            (Type::Crypto, "timingSafeEqual") => {
                let [a, b] = args else {
                    return Err(err("'crypto.timingSafeEqual' toma exactamente 2 argumentos (a: String, b: String)"));
                };
                self.check_expr(a, &Type::String, env)?;
                self.check_expr(b, &Type::String, env)?;
                Some(Type::Bool)
            }
            (Type::Http, "get") => {
                let [url] = args else {
                    return Err(err("'http.get' toma exactamente 1 argumento (url: String)"));
                };
                self.check_expr(url, &Type::String, env)?;
                Some(Type::String)
            }
            (Type::Http, "post") => {
                let [url, body] = args else {
                    return Err(err("'http.post' toma exactamente 2 argumentos (url: String, body: String)"));
                };
                self.check_expr(url, &Type::String, env)?;
                self.check_expr(body, &Type::String, env)?;
                Some(Type::String)
            }
            (Type::Http, "getWithHeaders") => {
                let [url, headers] = args else {
                    return Err(err(
                        "'http.getWithHeaders' toma exactamente 2 argumentos (url: String, headers: {name: String, value: String}[])",
                    ));
                };
                self.check_expr(url, &Type::String, env)?;
                self.check_expr(headers, &Type::List(Box::new(http_header_type())), env)?;
                Some(Type::String)
            }
            (Type::Http, "getWithStatus") => {
                let [url, headers] = args else {
                    return Err(err(
                        "'http.getWithStatus' toma exactamente 2 argumentos (url: String, headers: {name: String, value: String}[])",
                    ));
                };
                self.check_expr(url, &Type::String, env)?;
                self.check_expr(headers, &Type::List(Box::new(http_header_type())), env)?;
                Some(http_response_type())
            }
            (Type::Http, "postWithStatus") => {
                let [url, body, headers] = args else {
                    return Err(err(
                        "'http.postWithStatus' toma exactamente 3 argumentos (url: String, body: String, headers: {name: String, value: String}[])",
                    ));
                };
                self.check_expr(url, &Type::String, env)?;
                self.check_expr(body, &Type::String, env)?;
                self.check_expr(headers, &Type::List(Box::new(http_header_type())), env)?;
                Some(http_response_type())
            }
            (Type::Http, "postWithHeaders") => {
                let [url, body, headers] = args else {
                    return Err(err(
                        "'http.postWithHeaders' toma exactamente 3 argumentos (url: String, body: String, headers: {name: String, value: String}[])",
                    ));
                };
                self.check_expr(url, &Type::String, env)?;
                self.check_expr(body, &Type::String, env)?;
                self.check_expr(headers, &Type::List(Box::new(http_header_type())), env)?;
                Some(Type::String)
            }
            // GRAMMAR.md §3.160: reintenta con backoff exponencial FIJO (no
            // configurable, mismo criterio que MAX_WHILE_ITERATIONS §3.15 --
            // un backstop generoso, no un sistema fino de política de
            // reintentos) ante CUALQUIER falla -- red o un status no-2xx,
            // mismo criterio de "falla" que `post`/`postWithHeaders` ya
            // usan. `maxAttempts` es el único knob real: cuánto tolera cada
            // caller puntual antes de rendirse, algo que sí varía caso a
            // caso (un webhook de cobro vs. una notificación de baja
            // prioridad).
            (Type::Http, "postWithRetry") => {
                let [url, body, headers, max_attempts] = args else {
                    return Err(err(
                        "'http.postWithRetry' toma exactamente 4 argumentos (url: String, body: String, headers: {name: String, value: String}[], maxAttempts: Int)",
                    ));
                };
                self.check_expr(url, &Type::String, env)?;
                self.check_expr(body, &Type::String, env)?;
                self.check_expr(headers, &Type::List(Box::new(http_header_type())), env)?;
                self.check_expr(max_attempts, &Type::Int, env)?;
                Some(Type::String)
            }
            (Type::Json, "parse") => {
                let [str_arg] = args else {
                    return Err(err("'json.parse' toma exactamente 1 argumento (text: String)"));
                };
                self.check_expr(str_arg, &Type::String, env)?;
                Some(Type::Dynamic)
            }
            (Type::Json, "stringify") => {
                let [val_arg] = args else {
                    return Err(err("'json.stringify' toma exactamente 1 argumento (value: Dynamic)"));
                };
                self.synth_expr(val_arg, env)?;
                Some(Type::String)
            }
            (Type::Base64, "encode") => {
                let [str_arg] = args else {
                    return Err(err("'base64.encode' toma exactamente 1 argumento (data: String)"));
                };
                self.check_expr(str_arg, &Type::String, env)?;
                Some(Type::String)
            }
            (Type::Env, "get") => {
                let [name_arg] = args else {
                    return Err(err("'env.get' toma exactamente 1 argumento (nombre: String)"));
                };
                self.check_expr(name_arg, &Type::String, env)?;
                Some(Type::Optional(Box::new(Type::String)))
            }
            (Type::Request, "rawBody") => {
                self.expect_no_args(args, "rawBody")?;
                Some(Type::String)
            }
            (Type::Request, "header") => {
                let [name_arg] = args else {
                    return Err(err("'request.header' toma exactamente 1 argumento (nombre: String)"));
                };
                self.check_expr(name_arg, &Type::String, env)?;
                Some(Type::Optional(Box::new(Type::String)))
            }
            (Type::Smtp, "send") => {
                let [to, subject, body] = args else {
                    return Err(err("'smtp.send' toma exactamente 3 argumentos (to: String, subject: String, body: String)"));
                };
                self.check_expr(to, &Type::String, env)?;
                self.check_expr(subject, &Type::String, env)?;
                self.check_expr(body, &Type::String, env)?;
                Some(Type::Void)
            }
            (Type::Smtp, "sendToMany") => {
                let [to, subject, body] = args else {
                    return Err(err("'smtp.sendToMany' toma exactamente 3 argumentos (to: String[], subject: String, body: String)"));
                };
                self.check_expr(to, &Type::List(Box::new(Type::String)), env)?;
                self.check_expr(subject, &Type::String, env)?;
                self.check_expr(body, &Type::String, env)?;
                Some(Type::Void)
            }
            (Type::Smtp, "sendHtml") => {
                let [to, subject, html] = args else {
                    return Err(err("'smtp.sendHtml' toma exactamente 3 argumentos (to: String[], subject: String, html: String)"));
                };
                self.check_expr(to, &Type::List(Box::new(Type::String)), env)?;
                self.check_expr(subject, &Type::String, env)?;
                self.check_expr(html, &Type::String, env)?;
                Some(Type::Void)
            }
            (Type::Smtp, "sendMessage") => {
                let [message] = args else {
                    return Err(err(
                        "'smtp.sendMessage' toma exactamente 1 argumento (message: { to: String[], cc: String[]?, bcc: String[]?, subject: String, body: String, html: Bool?, attachments: {...}[]? })",
                    ));
                };
                self.check_expr(message, &smtp_message_type(), env)?;
                Some(Type::Void)
            }
            (Type::Response, "setStatus") => {
                let [code_arg] = args else {
                    return Err(err("'response.setStatus' toma exactamente 1 argumento (code: Int)"));
                };
                if self.in_stream_body.load(std::sync::atomic::Ordering::Relaxed) {
                    return Err(err(
                        "'response.setStatus' no tiene efecto dentro de un 'stream': el status de una conexión SSE es fijo para toda su duración (GRAMMAR.md §3.46) -- llamalo desde un 'rpc' normal",
                    ));
                }
                self.check_expr(code_arg, &Type::Int, env)?;
                Some(Type::Void)
            }
            (Type::Response, "redirect") => {
                let [url, permanent] = args else {
                    return Err(err("'response.redirect' toma exactamente 2 argumentos (url: String, permanent: Bool)"));
                };
                if self.in_stream_body.load(std::sync::atomic::Ordering::Relaxed) {
                    return Err(err(
                        "'response.redirect' no tiene efecto dentro de un 'stream': mismo motivo que 'response.setStatus' (GRAMMAR.md §3.46) -- una conexión SSE ya envió su status antes de que el cuerpo corra",
                    ));
                }
                self.check_expr(url, &Type::String, env)?;
                self.check_expr(permanent, &Type::Bool, env)?;
                Some(Type::Void)
            }
            (Type::Base64, "decode") => {
                let [str_arg] = args else {
                    return Err(err("'base64.decode' toma exactamente 1 argumento (base64_str: String)"));
                };
                self.check_expr(str_arg, &Type::String, env)?;
                Some(Type::String)
            }
            (Type::Pdf, "build") => builtin_args!(
                self, args, env, "pdf.build",
                [(blocks, "blocks: PdfBlock[]", Type::List(Box::new(Type::Enum("PdfBlock".to_string()))))] -> Type::String
            ),
            (Type::Excel, "build") => builtin_args!(
                self, args, env, "excel.build",
                [(sheets, "sheets: ExcelSheet[]", Type::List(Box::new(excel_sheet_struct_type())))] -> Type::String
            ),
            (Type::Excel, "parse") => builtin_args!(
                self, args, env, "excel.parse",
                [(base64, "base64: String", Type::String)] -> Type::List(Box::new(excel_sheet_struct_type()))
            ),
            // GRAMMAR.md §3.235: inferencia local. `maxTokens` es explícito a
            // propósito -- el techo de tokens es la decisión de costo más
            // importante de cada llamada, nunca un default escondido.
            (Type::Ai, "generate") => builtin_args!(
                self, args, env, "ai.generate",
                [(model, "model: String", Type::String), (prompt, "prompt: String", Type::String), (max_tokens, "maxTokens: Int", Type::Int)] -> Type::String
            ),
            (Type::Ai, "chat") => builtin_args!(
                self, args, env, "ai.chat",
                [(model, "model: String", Type::String), (messages, "messages: AiMessage[]", Type::List(Box::new(ai_message_struct_type()))), (max_tokens, "maxTokens: Int", Type::Int)] -> Type::String
            ),
            (Type::Ai, "models") => {
                self.expect_no_args(args, "ai.models")?;
                Some(Type::List(Box::new(Type::String)))
            }
            // GRAMMAR.md §3.236: tipa como `AiToken[]` -- como cuerpo completo
            // de un `stream -> AiToken` el servidor lo emite token a token;
            // en cualquier otra posición devuelve la lista entera.
            (Type::Ai, "stream") => builtin_args!(
                self, args, env, "ai.stream",
                [(model, "model: String", Type::String), (messages, "messages: AiMessage[]", Type::List(Box::new(ai_message_struct_type()))), (max_tokens, "maxTokens: Int", Type::Int)] -> Type::List(Box::new(ai_token_struct_type()))
            ),
            (Type::Mcp, "sample") => builtin_args!(
                self, args, env, "mcp.sample",
                [(prompt, "prompt: String", Type::String)] -> Type::String
            ),
            (Type::List(_inner), "join") => {
                let [sep_arg] = args else {
                    return Err(err("'join' toma exactamente 1 argumento (sep: String)"));
                };
                self.check_expr(sep_arg, &Type::String, env)?;
                Some(Type::String)
            }
            (Type::List(inner), "reverse") => {
                self.expect_no_args(args, "reverse")?;
                Some(Type::List(inner.clone()))
            }
            (Type::List(inner), "take") => {
                let [n_arg] = args else {
                    return Err(err("'take' toma exactamente 1 argumento (n: Int)"));
                };
                self.check_expr(n_arg, &Type::Int, env)?;
                Some(Type::List(inner.clone()))
            }
            // GRAMMAR.md §3.230: orden EN MEMORIA por una clave derivada --
            // el complemento de `db.<c>.orderBy` para lo que no es una
            // columna (una lista ya filtrada, un campo calculado). La clave
            // tiene que ser de un tipo con orden total real
            // (`is_orderable_key`) o su versión nullable: los `null` van al
            // final en las dos direcciones, igual que en SQL.
            (Type::List(inner), "sortBy" | "sortByDesc") => {
                let [f_arg] = args else {
                    return Err(err(format!("'{field}' toma exactamente 1 argumento (clave: (T) -> K)")));
                };
                let key_ty = self.synth_callback_result(f_arg, inner, env)?;
                let base = match &key_ty {
                    Type::Optional(k) => k.as_ref(),
                    other => other,
                };
                if !Self::is_orderable_key(base) {
                    return Err(err(format!(
                        "'{field}': la clave de orden es {key_ty} -- tiene que ser Int, Int64, Float, Decimal, String, Bool, Timestamp o Uuid (o su versión nullable), GRAMMAR.md §3.230"
                    )));
                }
                Some(Type::List(inner.clone()))
            }
            // Mismo nombre que String.length() (GRAMMAR.md §3.8) -- faltaba
            // por la misma razón que .take() faltó en su momento: nada lo
            // había necesitado todavía. Encontrado al escribir `login` para
            // auth v0 (necesita "¿matcheó algún usuario?").
            (Type::List(_), "length") => {
                self.expect_no_args(args, "length")?;
                Some(Type::Int)
            }
            // GRAMMAR.md §3.101: en esta ronda solo `List<Int>` -- `List<Int64>`/
            // `List<Float>` quedan deliberadamente afuera. No es una restricción
            // sintáctica: en runtime `Value::List` (runtime/mod.rs) no lleva
            // ningún tag de tipo de elemento (mismo límite ya documentado en la
            // doc de `Value::Uuid` ahí mismo) -- una lista VACÍA no tiene de
            // dónde sacar "esto era Int64" o "esto era Float" para elegir el
            // `Value` correcto a devolver, y equivocarse ahí sería un bug de
            // serialización silencioso (`Int64` viaja como string en el wire,
            // `Int` como número -- GRAMMAR.md §3.30). `List<Int>` no tiene esa
            // ambigüedad (un solo tipo posible para "vacío" también), así que
            // es lo único que se resuelve acá.
            (Type::List(inner), "sum") => {
                self.expect_no_args(args, "sum")?;
                if !matches!(inner.as_ref(), Type::Int) {
                    return Err(err(format!(
                        "'.sum()' en esta ronda solo aplica sobre `List<Int>`/`Int[]` -- es `List<{inner}>`. \
                         `Int64`/`Float` quedan deliberadamente afuera (GRAMMAR.md §3.101)"
                    )));
                }
                Some(Type::Int)
            }
            // El caso FÁCIL de los dos métodos de orden superior (GRAMMAR.md
            // §3.10): el tipo del callback (T) -> Bool ya se conoce ENTERO
            // de entrada, así que alcanza con el mismo check_expr(Closure,
            // expected, ...) que cualquier otro argumento de tipo función.
            (Type::List(inner), "filter") => {
                let [pred_arg] = args else {
                    return Err(err("'filter' toma exactamente 1 argumento (predicado (T) -> Bool)"));
                };
                let expected_fn = Type::Function(vec![(**inner).clone()], Box::new(Type::Bool));
                self.check_expr(pred_arg, &expected_fn, env)?;
                Some(Type::List(inner.clone()))
            }
            // El caso DIFÍCIL: el tipo de retorno del callback (U) es
            // exactamente lo que no se conoce de entrada -- `synth_callback_result`
            // lo sintetiza en vez de chequearlo contra un tipo ya fijo.
            (Type::List(inner), "map") => {
                let [f_arg] = args else {
                    return Err(err("'map' toma exactamente 1 argumento (f: (T) -> U)"));
                };
                let result_ty = self.synth_callback_result(f_arg, inner, env)?;
                Some(Type::List(Box::new(result_ty)))
            }
            // PLAN.md §9.14 ítem 2: ¿aparece `item` en la lista? Acotado a
            // los tipos de elemento donde `==` en runtime (Value::PartialEq)
            // ya es sólido -- Decimal queda fuera (el bug de igualdad,
            // §3.195, recién se cerró esta misma ronda) y también Struct/
            // Variant (su PartialEq es sensible al ORDEN textual de un
            // literal fuente -- un bug latente preexistente que extender
            // `.contains()` ahí lo heredaría en silencio, fuera de alcance
            // de esta pieza). `List<T>` anidada tampoco entra, sin evidencia
            // de demanda todavía.
            (Type::List(inner), "contains") if matches!(
                inner.as_ref(),
                Type::Int | Type::Int64 | Type::Float | Type::String | Type::Bool | Type::Uuid | Type::Timestamp
            ) => builtin_args!(
                self, args, env, "contains",
                [(item, "item: T", (**inner).clone())] -> Type::Bool
            ),
            _ => None,
        };
        Ok(ty)
    }

    /// Tipo de retorno de invocar `callback` con un único argumento de tipo
    /// `param_ty` -- lo que `.map` necesita para saber el tipo de elemento
    /// de la lista resultante, que no se conoce de entrada (a diferencia de
    /// `.filter`, cuyo callback siempre devuelve `Bool`).
    ///
    /// Dos formas de callback, dos caminos DISTINTOS a propósito:
    /// - Un closure literal: no hay ningún `Type::Function` ya resuelto del
    ///   que `synth_expr` pueda partir (el propio closure es lo que hay que
    ///   sintetizar) -- se liga el param a `param_ty` (single-source-of-truth
    ///   si no está anotado; si está anotado, tiene que ACEPTAR `param_ty`,
    ///   comprobado con `is_subtype(param_ty, anotación)` -- dirección NORMAL
    ///   de argumento, no la contravariante de `check_closure`, porque acá
    ///   `param_ty` es un tipo CONCRETO que de verdad se va a pasar, no el
    ///   tipo esperado de un `Type::Function` completo) y se sintetiza el
    ///   cuerpo con `synth_block`.
    /// - Cualquier otra cosa (ej. una `fn` referenciada por nombre): ya
    ///   sintetiza un `Type::Function` completo por su cuenta -- se verifica
    ///   que acepte `param_ty` como argumento (subtipado normal, igual que
    ///   cualquier otro argumento de una llamada) y se devuelve su retorno.
    fn synth_callback_result(&self, callback: &Spanned<Expr>, param_ty: &Type, env: &Env) -> Result<Type, CheckError> {
        if let Expr::Closure { params, body } = &callback.node {
            let [p] = params.as_slice() else {
                return Err(err(format!(
                    "el callback de 'map' necesita exactamente 1 parámetro, se encontraron {}",
                    params.len()
                )));
            };
            let bound_ty = match &p.ty {
                Some(texpr) => {
                    let annotated = self.resolve_type(texpr)?;
                    if !is_subtype(param_ty, &annotated) {
                        return Err(err(format!(
                            "el parámetro '{}' anotado como {annotated:?} no acepta el elemento real de la lista ({param_ty:?})",
                            p.name
                        )));
                    }
                    annotated
                }
                None => param_ty.clone(),
            };
            let mut local = env.clone();
            local.insert(p.name.clone(), immutable(bound_ty));
            return self.synth_block(body, &local);
        }
        let callback_ty = self.synth_expr(callback, env)?;
        let Type::Function(actual_params, ret) = callback_ty else {
            return Err(err(format!(
                "el callback de 'map' tiene que ser una función de 1 parámetro, se encontró {callback_ty:?}"
            )));
        };
        let [actual_param] = actual_params.as_slice() else {
            return Err(err(format!(
                "el callback de 'map' necesita exactamente 1 parámetro, se encontraron {}",
                actual_params.len()
            )));
        };
        if !is_subtype(param_ty, actual_param) {
            return Err(err(format!(
                "el callback de 'map' no acepta el elemento real de la lista ({param_ty:?} no es subtipo de {actual_param:?})"
            )));
        }
        Ok(*ret)
    }

    /// `all/find/insert/applyPatch` sobre una colección de `db` (GRAMMAR.md
    /// §2.1) -- resueltos contra `element_ty` de verdad, así que un método
    /// desconocido ya es un error de tipos acá, no algo que se descubre en
    /// runtime (`Type::Dynamic` dejaba pasar cualquier nombre antes).
    fn check_db_method(&self, element_ty: &Type, method: &str, args: &[Spanned<Expr>], env: &Env) -> Result<Type, CheckError> {
        match method {
            "all" => {
                self.expect_no_args(args, "all")?;
                Ok(Type::List(Box::new(element_ty.clone())))
            }
            "find" => {
                let [id_arg] = args else {
                    return Err(err("'find' toma exactamente 1 argumento (id: Int o Uuid, según la PK de la colección)"));
                };
                self.check_expr(id_arg, &Self::db_id_type(element_ty), env)?;
                Ok(Type::Optional(Box::new(element_ty.clone())))
            }
            "insert" => {
                // Omit<T, "id"> (GRAMMAR.md §2.1): T completo rechazaría el
                // propio demo insignia, donde el shape de creación
                // (NewUser) es deliberadamente un subconjunto de T.
                let [value_arg] = args else {
                    return Err(err("'insert' toma exactamente 1 argumento"));
                };
                let insertable = self.omit_id_field(element_ty)?;
                self.check_expr(value_arg, &insertable, env)?;
                Ok(element_ty.clone())
            }
            // GRAMMAR.md §3.76. Cada elemento sigue siendo `insert` real
            // (una sentencia SQL autocommit por fila, mismo criterio que el
            // resto del lenguaje -- ver "Límites honestos") -- lo que evita
            // es la ida y vuelta HTTP N veces, no el costo de N inserts en
            // la base.
            "insertMany" => {
                let [items_arg] = args else {
                    return Err(err("'insertMany' toma exactamente 1 argumento (items: Omit<T,\"id\">[])"));
                };
                let insertable = self.omit_id_field(element_ty)?;
                self.check_expr(items_arg, &Type::List(Box::new(insertable)), env)?;
                Ok(Type::List(Box::new(element_ty.clone())))
            }
            "applyPatch" => {
                let [id_arg, patch_arg] = args else {
                    return Err(err("'applyPatch' toma exactamente 2 argumentos (id: Int o Uuid, patch: Patch<T>)"));
                };
                self.check_expr(id_arg, &Self::db_id_type(element_ty), env)?;
                self.check_expr(patch_arg, &Type::PatchOf(Box::new(element_ty.clone())), env)?;
                Ok(element_ty.clone())
            }
            "delete" => {
                let [id_arg] = args else {
                    return Err(err("'delete' toma exactamente 1 argumento (id: Int o Uuid, según la PK de la colección)"));
                };
                self.check_expr(id_arg, &Self::db_id_type(element_ty), env)?;
                Ok(Type::Bool)
            }
            "deleteWhere" => {
                let [pred_arg] = args else {
                    return Err(err("'deleteWhere' toma exactamente 1 argumento (fn(T) -> Bool)"));
                };
                let pred_ty = Type::Function(vec![element_ty.clone()], Box::new(Type::Bool));
                self.check_expr(pred_arg, &pred_ty, env)?;
                Ok(Type::Int)
            }
            "findWhere" => {
                let [pred_arg] = args else {
                    return Err(err("'findWhere' toma exactamente 1 argumento (fn(T) -> Bool)"));
                };
                let pred_ty = Type::Function(vec![element_ty.clone()], Box::new(Type::Bool));
                self.check_expr(pred_arg, &pred_ty, env)?;
                Ok(Type::List(Box::new(element_ty.clone())))
            }
            "count" => {
                self.expect_no_args(args, "count")?;
                Ok(Type::Int)
            }
            // GRAMMAR.md §3.95: `db.<c>.countWhere(fn(T) -> Bool) -> Int`,
            // mismo contrato de tipos que `findWhere`/`deleteWhere` (arriba)
            // -- la diferencia es puramente de EJECUCIÓN (empuja a SQL
            // cuando el predicado tiene la forma `|x| x.campo == valor`,
            // GRAMMAR.md §3.95), invisible acá.
            "countWhere" => {
                let [pred_arg] = args else {
                    return Err(err("'countWhere' toma exactamente 1 argumento (fn(T) -> Bool)"));
                };
                let pred_ty = Type::Function(vec![element_ty.clone()], Box::new(Type::Bool));
                self.check_expr(pred_arg, &pred_ty, env)?;
                Ok(Type::Int)
            }
            "page" => {
                let [limit_arg, offset_arg] = args else {
                    return Err(err("'page' toma exactamente 2 argumentos (limit: Int, offset: Int)"));
                };
                self.check_expr(limit_arg, &Type::Int, env)?;
                self.check_expr(offset_arg, &Type::Int, env)?;
                Ok(Type::List(Box::new(element_ty.clone())))
            }
            "pageAfter" => {
                let [cursor_arg, limit_arg] = args else {
                    return Err(err("'pageAfter' toma exactamente 2 argumentos (cursor: Int?, limit: Int)"));
                };
                // GRAMMAR.md §3.177: rechazado a propósito sobre una PK
                // Uuid, no solo "todavía no soportado". La garantía real
                // de pageAfter ("nunca se salta una fila insertada
                // durante la paginación") depende de que el id crezca EN
                // EL MISMO ORDEN que la inserción -- cierto para un
                // autoincremento, falso para un Uuid aleatorio: una fila
                // insertada concurrentemente con id menor al cursor
                // actual quedaría afuera de TODA página futura de ese
                // pase, sin ningún error que lo señale. Dejarlo pasar con
                // orden lexicográfico habría sido "compila y corre" con
                // una garantía documentada rota en silencio.
                if Self::db_id_type(element_ty) == Type::Uuid {
                    return Err(err(
                        "'pageAfter' no está soportado sobre una colección con 'id: Uuid' -- su garantía de que \
                         nunca se salta una fila insertada durante la paginación depende de que el id crezca en \
                         el mismo orden que la inserción (autoincremento); un Uuid aleatorio no tiene ese orden, \
                         así que una fila insertada concurrentemente con id menor al cursor actual quedaría \
                         afuera de toda página futura, en silencio. Usá 'page' (offset), que no depende de ese \
                         orden, o un campo propio de fecha/secuencia para paginación estable",
                    ));
                }
                self.check_expr(cursor_arg, &Type::Optional(Box::new(Type::Int)), env)?;
                self.check_expr(limit_arg, &Type::Int, env)?;
                Ok(Type::List(Box::new(element_ty.clone())))
            }
            "sumBy" | "countBy" | "avgBy" | "maxBy" | "minBy" => self.check_aggregate_by(element_ty, method, args, env),
            "maxRow" | "minRow" => self.check_top_row(element_ty, method, args, env),
            "orderBy" | "orderByDesc" => self.check_order_by(element_ty, method, args),
            "increment" => self.check_increment(element_ty, method, args, env),

            // GRAMMAR.md §3.75. `updateFn` devuelve `Omit<T,"id">` (un
            // VALOR completo), no `Patch<T>` -- a propósito: `Patch<T>` no
            // tiene sintaxis de literal en el lenguaje (solo llega
            // decodificado del wire como parámetro de rpc, GRAMMAR.md §3.4),
            // así que un `updateFn` que "devolviera un Patch<T>" sería
            // imposible de escribir DESDE ADENTRO de un cuerpo de rpc/fn --
            // no hay forma de construir ese valor ahí. Devolver el shape
            // insertable completo sí es constructible (`NewX { ... }`, un
            // literal común) y sigue permitiendo que el update dependa de
            // los otros campos de la fila existente (ej. incrementar un
            // contador), que es la ventaja real de una función sobre un
            // valor estático.
            "upsert" => {
                let [match_arg, insert_arg, update_arg] = args else {
                    return Err(err(
                        "'upsert' toma exactamente 3 argumentos (matchFn: (T) -> Bool, insertValue: Omit<T,\"id\">, updateFn: (T) -> Omit<T,\"id\">)",
                    ));
                };
                let pred_ty = Type::Function(vec![element_ty.clone()], Box::new(Type::Bool));
                self.check_expr(match_arg, &pred_ty, env)?;
                let insertable = self.omit_id_field(element_ty)?;
                self.check_expr(insert_arg, &insertable, env)?;
                let update_ty = Type::Function(vec![element_ty.clone()], Box::new(insertable));
                self.check_expr(update_arg, &update_ty, env)?;
                Ok(element_ty.clone())
            }

            // Deliberadamente SIEMPRE un error acá, nunca una firma normal
            // y libremente componible como las de arriba (GRAMMAR.md
            // §3.16): la única forma de que `subscribe()` tipe en TODO el
            // programa es a través de `check_live_subscribe`, que corre
            // ANTES (en `check_rpc`) y nunca llega a llamar a esta función
            // para ese shape exacto. Si `subscribe` tuviera una firma
            // normal acá, `rpc getOne() -> User { db.users.subscribe() }`
            // (fuera del shape reconocido) tipiaría bien sin tener ningún
            // comportamiento sensato en runtime -- ni bloquear el hilo
            // principal para siempre, ni inventar un dato.
            "subscribe" => Err(err(
                "'subscribe' solo es válido como cuerpo COMPLETO de un stream, exactamente \
                 `while true { db.<coleccion>.subscribe() }` -- no se puede usar en ninguna otra posición (GRAMMAR.md §3.16)",
            )),
            other => Err(err(format!(
                "'{other}' no es un método conocido de una colección de 'db' (all/find/insert/insertMany/applyPatch/delete/deleteWhere/findWhere/count/countWhere/page/upsert/sumBy/countBy/avgBy/maxBy/minBy/maxRow/minRow/increment/subscribe)"
            ))),
        }
    }

    /// `auth.createSession(role)` / `auth.destroySession()` (GRAMMAR.md
    /// §3.14, auth v0). El enum de `role` NO necesita ser "simple" (todas
    /// las variantes unitarias) -- la sesión solo guarda el TAG
    /// (enum_name+variant_name, nunca campos), así que un enum con una
    /// variante con datos (ej. `Role.ServiceAccount{scopes:[...]}`) puede
    /// tener otra variante unitaria (`Role.Admin`) usada acá sin problema.
    /// `destroySession` toma CERO argumentos a propósito: opera sobre la
    /// sesión que ya autenticó la request actual (extraída del header en
    /// server.rs), no sobre un token que el caller nombre -- si tomara un
    /// token como parámetro, cualquiera podría destruir la sesión de
    /// cualquier otro con solo adivinar/conocer ese string (hallado en el
    /// review adversarial de esta ronda).
    fn check_auth_method(&self, method: &str, args: &[Spanned<Expr>], env: &Env) -> Result<Type, CheckError> {
        match method {
            "createSession" => {
                let [role_arg] = args else {
                    return Err(err("'createSession' toma exactamente 1 argumento (role: un valor de un enum declarado)"));
                };
                match self.synth_expr(role_arg, env)? {
                    Type::Enum(_) => Ok(Type::String),
                    other => Err(err(format!(
                        "'createSession' espera un valor de un enum declarado (ej. Role.Admin {{}}), se encontró {other}"
                    ))),
                }
            }
            "createSessionWithId" => {
                let [role_arg, user_id_arg] = args else {
                    return Err(err("'createSessionWithId' toma exactamente 2 argumentos (role: un valor de un enum declarado, userId: Int)"));
                };
                match self.synth_expr(role_arg, env)? {
                    Type::Enum(_) => {}
                    other => return Err(err(format!(
                        "'createSessionWithId' espera un valor de un enum declarado como primer argumento (ej. Role.Admin {{}}), se encontró {other}"
                    ))),
                }
                match self.synth_expr(user_id_arg, env)? {
                    Type::Int => Ok(Type::String),
                    other => Err(err(format!(
                        "'createSessionWithId' espera un Int como segundo argumento (userId), se encontró {other}"
                    ))),
                }
            }
            "destroySession" => {
                self.expect_no_args(args, "destroySession")?;
                Ok(Type::Void)
            }
            // GRAMMAR.md §3.84 -- a diferencia de `destroySession` (cero
            // argumentos, opera sobre la sesión ya autenticada), ÉSTE sí
            // toma un `userId: Int` explícito: mismo criterio que
            // `createSessionWithId`, un `user_id` es una clave de
            // aplicación, no un secreto adivinable.
            "destroyAllSessions" => {
                let [user_id_arg] = args else {
                    return Err(err("'destroyAllSessions' toma exactamente 1 argumento (userId: Int)"));
                };
                match self.synth_expr(user_id_arg, env)? {
                    Type::Int => Ok(Type::Int),
                    other => Err(err(format!(
                        "'destroyAllSessions' espera un Int como argumento (userId), se encontró {other}"
                    ))),
                }
            }
            "currentRole" => {
                self.expect_no_args(args, "currentRole")?;
                Ok(Type::Optional(Box::new(Type::String)))
            }
            "currentUserId" => {
                self.expect_no_args(args, "currentUserId")?;
                Ok(Type::Optional(Box::new(Type::Int)))
            }
            // GRAMMAR.md §3.197: accessor genérico de un claim JWT por
            // nombre -- a diferencia de currentRole/currentUserId (slots
            // fijos configurados UNA vez al arrancar via --jwt-role-claim/
            // --jwt-user-id-claim), el nombre del claim es un argumento
            // normal en cada llamada, sin flag de CLI nuevo.
            "claim" => {
                let [name] = args else {
                    return Err(err("'claim' toma exactamente 1 argumento (name: String)"));
                };
                self.check_expr(name, &Type::String, env)?;
                Ok(Type::Optional(Box::new(Type::String)))
            }
            // GRAMMAR.md §3.152: bloqueo de cuenta configurable -- tres
            // primitivas chicas en vez de un mecanismo mágico, mismo
            // criterio que el resto del lenguaje (§9.1 del PLAN): quien
            // escribe el `.link` decide el umbral/ventana en su PROPIO
            // código de login, sin ninguna anotación ni flag de servidor
            // nueva. `identifier` es responsabilidad de quien llama (email,
            // user id como String, IP -- lo que tenga sentido para SU login).
            "recordFailedLogin" => {
                let [identifier] = args else {
                    return Err(err("'recordFailedLogin' toma exactamente 1 argumento (identifier: String)"));
                };
                self.check_expr(identifier, &Type::String, env)?;
                Ok(Type::Void)
            }
            "failedLoginCount" => {
                let [identifier, window_seconds] = args else {
                    return Err(err("'failedLoginCount' toma exactamente 2 argumentos (identifier: String, windowSeconds: Int)"));
                };
                self.check_expr(identifier, &Type::String, env)?;
                self.check_expr(window_seconds, &Type::Int, env)?;
                Ok(Type::Int)
            }
            "resetFailedLogins" => {
                let [identifier] = args else {
                    return Err(err("'resetFailedLogins' toma exactamente 1 argumento (identifier: String)"));
                };
                self.check_expr(identifier, &Type::String, env)?;
                Ok(Type::Void)
            }
            other => Err(err(format!(
                "'{other}' no es un método conocido de 'auth' (createSession/createSessionWithId/destroySession/destroyAllSessions/currentRole/currentUserId/claim/recordFailedLogin/failedLoginCount/resetFailedLogins)"
            ))),
        }
    }

    /// "Los campos de T menos 'id'" -- estructural, sin sintaxis de tipo
    /// nueva (se emite como `Omit<T, "id">`, un utility type nativo de TS,
    /// ver ts_emit.rs). `validate_db_element_type` ya garantizó que 'id'
    /// existe al procesar `Item::Db` -- el chequeo acá es una segunda
    /// defensa, no la única.
    fn omit_id_field(&self, element_ty: &Type) -> Result<Type, CheckError> {
        let Type::Struct { fields, .. } = element_ty else {
            return Err(err("una colección de 'db' debe resolver a un struct"));
        };
        if !fields.iter().any(|f| f.name == "id") {
            return Err(err("cada colección de 'db' necesita un campo 'id: Int'"));
        }
        let without_id: Vec<FieldType> = fields.iter().filter(|f| f.name != "id").cloned().collect();
        Ok(Type::Struct { name: None, fields: without_id })
    }

    /// Extrae el campo que selecciona `|item: T| item.campo` (GRAMMAR.md
    /// §3.52) -- exige EXACTAMENTE ese shape (`ast::recognize_field_selector`)
    /// y que `campo` sea de verdad un campo de `element_ty`, devolviendo su
    /// tipo. `role` ("de agrupación"/"de valor") es solo para el mensaje.
    fn field_selector(&self, element_ty: &Type, arg: &Spanned<Expr>, method: &str, role: &str) -> Result<(String, Type), CheckError> {
        let shape_err = || {
            err(format!(
                "'{method}' espera un selector de campo {role} de la forma `|item: T| item.campo` -- un acceso de campo simple, sin expresiones derivadas ni llamadas a método (no hay forma de traducir eso a una columna SQL real)"
            ))
        };
        let Expr::Closure { params, body } = &arg.node else {
            return Err(shape_err());
        };
        let param_names: Vec<String> = params.iter().map(|p| p.name.clone()).collect();
        let Some(field_name) = crate::ast::recognize_field_selector(&param_names, body) else {
            return Err(shape_err());
        };
        let Type::Struct { fields, .. } = element_ty else {
            return Err(err("una colección de 'db' debe resolver a un struct"));
        };
        let Some(field) = fields.iter().find(|f| f.name == field_name) else {
            return Err(err(format!("'{method}': '{field_name}' no es un campo de este struct")));
        };
        // Dos formas de "opcional" (GRAMMAR.md §3.4): por CLAVE (`campo?: T`,
        // `field.optional`) y por TIPO/nullable (`campo: T?`,
        // `Type::Optional`) -- ninguna de las dos está soportada como
        // selector todavía, y las dos se rechazan con el MISMO mensaje: un
        // grupo/valor "ausente" no tiene una fila SQL real que representarlo.
        if field.optional || matches!(field.ty, Type::Optional(_)) {
            return Err(err(format!(
                "'{method}': el campo {role} '{field_name}' es opcional -- agregar por un campo que puede faltar (por clave, `campo?: T`, o nullable, `campo: T?`) todavía no está soportado"
            )));
        }
        Ok((field_name.to_string(), field.ty.clone()))
    }

    /// Igual que `field_selector`, pero SOLO para el selector de CLAVE de
    /// agrupación (`sumBy`/etc.) -- admite además la forma con truncado de
    /// fecha, `|item: T| item.campo.truncateToDay/Month/Year()`
    /// (GRAMMAR.md §3.157). Nunca se usa para el selector de VALOR ni para
    /// ningún otro selector del lenguaje (`maxRow`/`minRow`/`increment`) --
    /// esos siguen con `field_selector` sola, sin truncado.
    fn group_key_selector(
        &self,
        element_ty: &Type,
        arg: &Spanned<Expr>,
        method: &str,
    ) -> Result<(String, Type, Option<crate::ast::TimeGranularity>, bool), CheckError> {
        let shape_err = || {
            err(format!(
                "'{method}' espera un selector de agrupación de la forma `|item: T| item.campo` (o `|item: T| item.campo.truncateToDay/Month/Year()` sobre un Timestamp) -- un acceso de campo simple, sin otras expresiones derivadas ni llamadas a método (no hay forma de traducir eso a una columna SQL real)"
            ))
        };
        let Expr::Closure { params, body } = &arg.node else {
            return Err(shape_err());
        };
        let param_names: Vec<String> = params.iter().map(|p| p.name.clone()).collect();
        let Some((field_name, granularity)) = crate::ast::recognize_group_key_selector(&param_names, body) else {
            return Err(shape_err());
        };
        let Type::Struct { fields, .. } = element_ty else {
            return Err(err("una colección de 'db' debe resolver a un struct"));
        };
        let Some(field) = fields.iter().find(|f| f.name == field_name) else {
            return Err(err(format!("'{method}': '{field_name}' no es un campo de este struct")));
        };
        // GRAMMAR.md §3.231: la forma NULLABLE (`campo: T?`) sí agrupa --
        // el grupo de los NULL es un grupo más, `GROUP BY` ya lo hace en
        // SQL -- y el tipo base se valida abajo como siempre; la clave del
        // resultado sale `T?`. La forma por CLAVE (`campo?: T`) sigue
        // afuera: se guarda como JSON, sin una columna nativa que agrupar.
        if field.optional {
            return Err(err(format!(
                "'{method}': el campo de agrupación '{field_name}' es opcional por clave (`campo?: T`) -- se guarda como JSON, sin una columna nativa por la que agrupar; declaralo nullable (`campo: T?`) si puede faltar (GRAMMAR.md §3.231)"
            )));
        }
        let (base_ty, nullable) = match &field.ty {
            Type::Optional(inner) => ((**inner).clone(), true),
            other => (other.clone(), false),
        };
        Ok((field_name.to_string(), base_ty, granularity, nullable))
    }

    /// ¿El campo `field_name` de `element_ty` (un `Type::Struct` ya
    /// resuelto, sin anotaciones) lleva `@encrypted`? `Type::Struct` es
    /// estructural -- hay que cruzar con `self.types` (el `TypeDecl`
    /// ORIGINAL, con el `ast::Field` que sí conserva anotaciones) por el
    /// `name: Some(...)` que un elemento de colección siempre conserva.
    /// `false` (nunca un error) si `element_ty` no tiene nombre o no
    /// resuelve a un `type` conocido -- mismo criterio permisivo que el
    /// resto de los cruces de este archivo cuando la anotación
    /// sencillamente no aplica.
    fn field_is_encrypted(&self, element_ty: &Type, field_name: &str) -> bool {
        let Type::Struct { name: Some(type_name), .. } = element_ty else { return false };
        let Some(decl) = self.types.get(type_name) else { return false };
        let TypeExpr::Struct(fields) = &decl.ty else { return false };
        fields.iter().any(|f| f.name == field_name && f.encrypted())
    }

    /// `sumBy`/`countBy`/`avgBy`/`maxBy`/`minBy` (GRAMMAR.md §3.52):
    /// agregación con `GROUP BY` empujada a SQL de verdad -- a diferencia de
    /// `findWhere`/`deleteWhere` (predicado como closure, evaluado en el
    /// intérprete DESPUÉS de traer todas las filas), acá el closure nunca se
    /// ejecuta: solo se usa para NOMBRAR una columna (`field_selector`
    /// arriba), y la agregación entera corre adentro de la base.
    /// `countBy` es el único de los cinco sin selector de valor -- cuenta
    /// FILAS por grupo (`COUNT(*)`), no un campo específico.
    fn check_aggregate_by(&self, element_ty: &Type, method: &str, args: &[Spanned<Expr>], _env: &Env) -> Result<Type, CheckError> {
        let needs_value = method != "countBy";
        let expected = if needs_value { 2 } else { 1 };
        if args.len() != expected {
            let shape =
                if needs_value { "(groupBy: |item: T| item.campo, value: |item: T| item.campo)" } else { "(groupBy: |item: T| item.campo)" };
            return Err(err(format!("'{method}' toma exactamente {expected} argumento(s) {shape}")));
        }

        let (key_field, key_ty, granularity, key_nullable) = self.group_key_selector(element_ty, &args[0], method)?;
        // GRAMMAR.md §3.191: `GROUP BY` sobre una columna `@encrypted` (a
        // diferencia de `findWhere`/`countWhere`/`deleteWhere`, que
        // simplemente NO empujan un predicado sobre esa columna a SQL y
        // caen a filtrado interpretado, seguro por construcción) no tiene
        // ningún fallback -- `select_grouped` (`runtime/db.rs`) SIEMPRE
        // arma un `GROUP BY` SQL real. Agrupar por ciphertext (distinto en
        // cada escritura, por el nonce aleatorio de AES-GCM) daría un grupo
        // por FILA siempre, en silencio -- sin fallback seguro posible,
        // así que se rechaza acá, en compile-time.
        if self.field_is_encrypted(element_ty, &key_field) {
            return Err(err(format!(
                "'{method}': no se puede agrupar por '{key_field}' -- es un campo '@encrypted', y el ciphertext es distinto en cada escritura (nonce aleatorio), así que agrupar por esa columna daría un grupo por fila siempre, en silencio"
            )));
        }
        match granularity {
            // GRAMMAR.md §3.157: truncado explícito -- SOLO válido sobre un
            // campo `Timestamp` de verdad, el resultado agrupado sigue
            // siendo `Timestamp` (con menos precisión), no un tipo nuevo.
            Some(_) if key_ty != Type::Timestamp => {
                return Err(err(format!(
                    "'{method}': '.truncateTo...()' solo es válido sobre un campo Timestamp -- '{key_field}' es {key_ty}"
                )));
            }
            None if !matches!(key_ty, Type::String | Type::Int | Type::Int64 | Type::Bool | Type::Enum(_)) => {
                return Err(err(format!(
                    "'{method}': el campo de agrupación '{key_field}' es {key_ty} -- solo se puede agrupar por String, Int, Int64, Bool, un enum, o un Timestamp truncado con .truncateToDay()/.truncateToMonth()/.truncateToYear() (Float no, GRAMMAR.md §3.52/§3.65/§3.157)"
                )));
            }
            _ => {}
        }

        let value_ty = if needs_value {
            let (value_field, field_ty) = self.field_selector(element_ty, &args[1], method, "de valor")?;
            // GRAMMAR.md §3.184: `Decimal` entra en sumBy/maxBy/minBy (SQL
            // pushdown exacto en los dos backends -- SUM/MAX/MIN sobre una
            // columna Decimal se decodifica con el mismo `ColumnKind::Decimal`
            // de siempre) pero NO en `avgBy`, a propósito -- Postgres guarda
            // Decimal como NUMERIC nativo (valor real, sin escalar) pero
            // SQLite lo guarda como INTEGER YA escalado ×10.000 (sin tipo
            // decimal nativo); `AVG()` sobre esas dos representaciones
            // FÍSICAS distintas no da resultados comparables sin una
            // decodificación específica por backend que esta ronda no
            // construyó -- límite honesto, no atacado en v0.
            let field_ty_ok = if method == "avgBy" {
                matches!(field_ty, Type::Int | Type::Int64 | Type::Float)
            } else {
                matches!(field_ty, Type::Int | Type::Int64 | Type::Decimal | Type::Float)
            };
            if !field_ty_ok {
                let allowed = if method == "avgBy" { "Int, Int64 o Float" } else { "Int, Int64, Decimal o Float" };
                return Err(err(format!(
                    "'{method}': el campo de valor '{value_field}' es {field_ty} -- tiene que ser {allowed} (GRAMMAR.md §3.65/§3.184)"
                )));
            }
            Some(field_ty)
        } else {
            None
        };

        let result_ty = match method {
            "countBy" => Type::Int,
            "avgBy" => Type::Float,
            _ => value_ty.expect("sumBy/maxBy/minBy siempre validan un selector de valor arriba"),
        };

        Ok(Type::List(Box::new(Type::Struct {
            name: None,
            fields: vec![
                // GRAMMAR.md §3.231: clave nullable -> `key: T?`, el `null`
                // del grupo de los NULL viaja como cualquier otro `T?`.
                FieldType { name: "key".to_string(), optional: false, ty: if key_nullable { Type::Optional(Box::new(key_ty)) } else { key_ty } },
                FieldType { name: "value".to_string(), optional: false, ty: result_ty },
            ],
        })))
    }

    /// `db.<c>.maxRow(selector)` / `db.<c>.minRow(selector)` (GRAMMAR.md
    /// §3.102): la fila COMPLETA con el valor máximo/mínimo de un campo --
    /// a diferencia de `maxBy`/`minBy` (arriba), que solo agregan un VALOR
    /// (sin `GROUP BY`, siempre 0 o 1 grupo total). Mismo shape reconocido
    /// (`field_selector`, un acceso de campo simple) y mismas restricciones
    /// de tipo que el campo de VALOR de `sumBy`/`maxBy`/`minBy` -- solo
    /// `Int`/`Int64`/`Float`, nunca opcional.
    fn check_top_row(&self, element_ty: &Type, method: &str, args: &[Spanned<Expr>], _env: &Env) -> Result<Type, CheckError> {
        let [selector_arg] = args else {
            return Err(err(format!("'{method}' toma exactamente 1 argumento (selector: |item: T| item.campo)")));
        };
        let (field_name, field_ty) = self.field_selector(element_ty, selector_arg, method, "de orden")?;
        // GRAMMAR.md §3.184: Decimal entra acá sin el límite que avgBy tiene
        // en check_aggregate_by -- maxRow/minRow es un ORDER BY ... LIMIT 1
        // puro (runtime/db.rs::top_row), nunca un CAST de agregación, así
        // que la representación física distinta entre backends (NUMERIC vs
        // INTEGER escalado) no importa: ordenar por cualquiera de las dos da
        // el mismo orden relativo.
        if !matches!(field_ty, Type::Int | Type::Int64 | Type::Decimal | Type::Float) {
            return Err(err(format!(
                "'{method}': el campo de orden '{field_name}' es {field_ty} -- tiene que ser Int, Int64, Decimal o Float (mismo criterio que el campo de valor de sumBy/maxBy/minBy, GRAMMAR.md §3.52/§3.184)"
            )));
        }
        Ok(Type::Optional(Box::new(element_ty.clone())))
    }

    /// `db.<c>.orderBy(|item: T| item.campo)` / `orderByDesc(...)`
    /// (GRAMMAR.md §3.230, PLAN.md §9.19 ítem 5): el resultado es una
    /// consulta ORDENADA (`Type::DbQuery`), no una lista todavía -- el
    /// `ORDER BY` viaja dentro del SQL del `all()`/`page()`/`findWhere()`
    /// que venga después. Selector con la misma forma que `maxRow`
    /// (`recognize_field_selector`), pero SIN la restricción de
    /// `field_selector` sobre campos nullable: ordenar por un `T?` es válido
    /// (los `null` van siempre al final, `NULLS LAST`, en los dos motores).
    /// Un campo opcional por CLAVE (`campo?: T`) sí se rechaza: se guarda
    /// como JSON, sin una columna nativa que ordenar.
    fn check_order_by(&self, element_ty: &Type, method: &str, args: &[Spanned<Expr>]) -> Result<Type, CheckError> {
        let [selector_arg] = args else {
            return Err(err(format!("'{method}' toma exactamente 1 argumento (selector: |item: T| item.campo)")));
        };
        let shape_err = || {
            err(format!(
                "'{method}' espera un selector de campo de orden de la forma `|item: T| item.campo` -- un acceso de campo simple, sin expresiones derivadas ni llamadas a método (no hay forma de traducir eso a un ORDER BY real; para ordenar por una clave calculada usá `.all().sortBy(...)`, en memoria)"
            ))
        };
        let Expr::Closure { params, body } = &selector_arg.node else {
            return Err(shape_err());
        };
        let param_names: Vec<String> = params.iter().map(|p| p.name.clone()).collect();
        let Some(field_name) = crate::ast::recognize_field_selector(&param_names, body) else {
            return Err(shape_err());
        };
        let Type::Struct { fields, .. } = element_ty else {
            return Err(err("una colección de 'db' debe resolver a un struct"));
        };
        let Some(field) = fields.iter().find(|f| f.name == field_name) else {
            return Err(err(format!("'{method}': '{field_name}' no es un campo de este struct")));
        };
        if field.optional {
            return Err(err(format!(
                "'{method}': el campo de orden '{field_name}' es opcional por clave (`campo?: T`) -- se guarda como JSON, sin una columna nativa que ordenar; declaralo nullable (`campo: T?`) si puede faltar"
            )));
        }
        // GRAMMAR.md §3.191: mismo motivo que el GROUP BY de
        // `check_aggregate_by` -- ordenar por ciphertext (distinto en cada
        // escritura, nonce aleatorio) daría un orden aleatorio en silencio.
        if self.field_is_encrypted(element_ty, field_name) {
            return Err(err(format!(
                "'{method}': no se puede ordenar por '{field_name}' -- es un campo '@encrypted', y el ciphertext no tiene ningún orden útil (GRAMMAR.md §3.191/§3.230)"
            )));
        }
        let base = match &field.ty {
            Type::Optional(inner) => inner.as_ref(),
            other => other,
        };
        if !Self::is_orderable_key(base) {
            return Err(err(format!(
                "'{method}': el campo de orden '{field_name}' es {} -- solo se puede ordenar por Int, Int64, Float, Decimal, String, Bool, Timestamp o Uuid (o su versión nullable `T?`); una lista, un struct, un enum o un Map se guardan como JSON y no tienen un orden SQL real (GRAMMAR.md §3.230)",
                field.ty
            )));
        }
        Ok(Type::DbQuery(Box::new(element_ty.clone())))
    }

    /// Tipos con un orden total real tanto en SQL (`ORDER BY`) como en
    /// memoria (`List<T>.sortBy`, runtime/mod.rs::order_cmp) -- los DOS
    /// lados tienen que coincidir, por eso es una sola lista. Un enum
    /// simple queda afuera a propósito: su "orden" sería el alfabético del
    /// nombre de la variante, que ningún programa real quiere.
    fn is_orderable_key(ty: &Type) -> bool {
        matches!(ty, Type::Int | Type::Int64 | Type::Float | Type::Decimal | Type::String | Type::Bool | Type::Timestamp | Type::Uuid)
    }

    /// Métodos sobre una consulta ya ordenada (GRAMMAR.md §3.230): solo los
    /// de LECTURA que pueden llevar el `ORDER BY` dentro de su SQL. Un
    /// nombre desconocido acá es un error, nunca `Ok(None)`.
    fn check_db_query_method(&self, element_ty: &Type, method: &str, args: &[Spanned<Expr>], env: &Env) -> Result<Type, CheckError> {
        match method {
            "orderBy" | "orderByDesc" => self.check_order_by(element_ty, method, args),
            "all" | "page" | "findWhere" => self.check_db_method(element_ty, method, args, env),
            "pageAfter" => Err(err(
                "'pageAfter' no se puede combinar con 'orderBy'/'orderByDesc': su cursor es una posición en el orden por id (GRAMMAR.md §3.61), que un ORDER BY distinto rompería en silencio -- usá 'page(limit, offset)' sobre la consulta ordenada",
            )),
            other => Err(err(format!(
                "'{other}' no existe sobre una consulta ordenada (db.<c>.orderBy(...)) -- solo all(), page(limit, offset), findWhere(...) y otro orderBy/orderByDesc como clave secundaria (GRAMMAR.md §3.230)"
            ))),
        }
    }

    /// `db.<c>.increment(id, selector, delta) -> T` (GRAMMAR.md §3.105):
    /// mismo shape de selector (`field_selector`) que `maxRow`/`minRow`, pero
    /// acá el campo tiene que ser ESCRIBIBLE de verdad -- `UPDATE "campo" =
    /// "campo" + ?` en runtime. Alcance deliberadamente acotado a `Int` en
    /// esta ronda -- `Int64`/`Float` quedan afuera a propósito: los casos
    /// reales que motivaron esto (contadores como `totalPulls`/
    /// `requestCount`) son todos `Int`, y ampliar el alcance sin evidencia
    /// real de demanda sería adivinar. Devuelve `T` (no `T?`) -- un `id` que
    /// no existe es un error claro en runtime, mismo criterio que
    /// `applyPatch`.
    fn check_increment(&self, element_ty: &Type, method: &str, args: &[Spanned<Expr>], env: &Env) -> Result<Type, CheckError> {
        let [id_arg, selector_arg, delta_arg] = args else {
            return Err(err(format!(
                "'{method}' toma exactamente 3 argumentos (id: Int o Uuid, selector: |item: T| item.campo, delta: Int)"
            )));
        };
        self.check_expr(id_arg, &Self::db_id_type(element_ty), env)?;
        let (field_name, field_ty) = self.field_selector(element_ty, selector_arg, method, "a incrementar")?;
        if !matches!(field_ty, Type::Int) {
            return Err(err(format!(
                "'{method}': el campo '{field_name}' es {field_ty} -- en esta ronda solo aplica sobre Int (Int64/Float quedan deliberadamente afuera, GRAMMAR.md §3.105)"
            )));
        }
        self.check_expr(delta_arg, &Type::Int, env)?;
        Ok(element_ty.clone())
    }

    fn expect_no_args(&self, args: &[Spanned<Expr>], method: &str) -> Result<(), CheckError> {
        if !args.is_empty() {
            return Err(err(format!("'{method}' no toma argumentos")));
        }
        Ok(())
    }

    fn synth_struct_lit(
        &self,
        name: &str,
        variant: Option<&str>,
        fields: &[(String, Spanned<Expr>)],
        env: &Env,
    ) -> Result<Type, CheckError> {
        if name == "Result" {
            return Err(err(
                "'Result.Ok'/'Result.Err' necesitan un tipo esperado del contexto (ej. el retorno declarado del rpc) — no se pueden usar en posición de síntesis (GRAMMAR.md §3.5)",
            ));
        }
        // Un type/enum genérico no puede sintetizarse: ¿de dónde saldrían
        // sus argumentos de tipo sin un `expected` que ya los traiga? Mismo
        // motivo que Result arriba -- ver check_generic_struct_lit.
        if self.is_user_generic(name) {
            return Err(err(format!(
                "'{name}' es genérico -- necesita un tipo esperado del contexto para inferir los argumentos de tipo (ej. anotá el 'let', o usalo donde el tipo ya se conoce)"
            )));
        }
        match variant {
            Some(vname) => {
                let decl = self
                    .enums
                    .get(name)
                    .ok_or_else(|| err(format!("enum desconocido: '{name}'")))?;
                let v = decl
                    .variants
                    .iter()
                    .find(|v| v.name == vname)
                    .ok_or_else(|| err(format!("'{name}' no tiene variante '{vname}'")))?;
                self.check_fields_against(v.fields.as_deref().unwrap_or(&[]), fields, env)?;
                Ok(Type::Enum(name.to_string()))
            }
            None => {
                let decl = self.types.get(name).ok_or_else(|| err(format!("tipo desconocido: '{name}'")))?;
                let TypeExpr::Struct(decl_fields) = &decl.ty else {
                    return Err(err(format!("'{name}' no es un tipo struct, no se puede construir con {{...}}")));
                };
                self.check_fields_against(decl_fields, fields, env)?;
                // Construcción sintética (sin texto fuente real detrás) solo
                // para reusar el dispatch de resolve_type -- mismo span
                // placeholder que ya usa parser.rs:1264 para un Spanned
                // sintético análogo.
                self.resolve_type(&TypeExpr::Named(name.to_string(), vec![], Span::new(0, 0, 0, 0)))
            }
        }
    }

    fn check_fields_against(
        &self,
        decl_fields: &[Field],
        given: &[(String, Spanned<Expr>)],
        env: &Env,
    ) -> Result<(), CheckError> {
        let resolved = decl_fields
            .iter()
            .map(|f| {
                Ok(FieldType {
                    name: f.name.clone(),
                    // Un campo CON default (GRAMMAR.md §3.74) puede
                    // omitirse de un literal igual que uno `?:` -- se marca
                    // `optional` acá, PURAMENTE para esta comprobación de
                    // completitud (`check_fields_against_resolved` solo usa
                    // este flag para decidir "¿falta este campo?"), sin que
                    // el tipo real del campo cambie a `Optional` en ningún
                    // otro lado. Genéricos NO pasan por acá (usan
                    // `check_fields_against_resolved` directo con
                    // `FieldType` ya resuelto por `expand_generic_struct`,
                    // que no conserva `default`) -- alcance de esta ronda,
                    // ver GRAMMAR.md §3.74 "Límites honestos".
                    optional: f.optional || f.default.is_some(),
                    ty: self.resolve_type(&f.ty)?,
                })
            })
            .collect::<Result<Vec<_>, CheckError>>()?;
        self.check_fields_against_resolved(&resolved, given, env)
    }

    /// Igual que `check_fields_against`, pero para cuando los campos ya
    /// están resueltos (con un subst de genérico ya aplicado) -- ver
    /// `check_generic_struct_lit`, que no puede usar `resolve_type` normal
    /// porque los campos de un genérico instanciado necesitan `resolve_type_subst`.
    fn check_fields_against_resolved(
        &self,
        decl_fields: &[FieldType],
        given: &[(String, Spanned<Expr>)],
        env: &Env,
    ) -> Result<(), CheckError> {
        for (fname, fexpr) in given {
            let decl_f = decl_fields
                .iter()
                .find(|f| &f.name == fname)
                .ok_or_else(|| err(format!("campo desconocido: '{fname}'")))?;
            self.check_expr(fexpr, &decl_f.ty, env)?;
        }
        for decl_f in decl_fields {
            if !decl_f.optional && !given.iter().any(|(n, _)| n == &decl_f.name) {
                return Err(err(format!("falta el campo requerido '{}'", decl_f.name)));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::parse;

    fn check_source(src: &str) -> Result<(), Vec<CheckError>> {
        let tokens = tokenize(src).unwrap_or_else(|e| panic!("{e}"));
        let program = parse(tokens).unwrap_or_else(|e| panic!("{e:?}"));
        Checker::check_program(&program)
    }

    /// Parsea `src` y llama a `hover_type_at` en el offset de la PRIMERA
    /// aparición de `needle` -- suficientemente preciso para estos tests
    /// (`needle` se elige para que no matchee antes de donde importa).
    fn hover_at(src: &str, needle: &str) -> Option<Type> {
        let tokens = tokenize(src).unwrap_or_else(|e| panic!("{e}"));
        let program = parse(tokens).unwrap_or_else(|e| panic!("{e:?}"));
        let offset = src.find(needle).unwrap_or_else(|| panic!("'{needle}' no aparece en el source de prueba: {src}"));
        Checker::hover_type_at(&program, offset)
    }

    /// Con las palabras clave admitidas como nombre de campo, `type` pasa a
    /// ser escribible -- pero en una variante CON datos choca con la clave del
    /// discriminante y produciria `{ type: "Ok"; type: string }`, que es un
    /// identificador duplicado y no compila en TypeScript.
    #[test]
    fn a_variant_payload_field_named_type_collides_with_the_discriminant() {
        let errs = check_source("enum Res { Ok { type: String } }
type T = { id: Int, r: Res }")
            .expect_err("deberia rechazarse");
        assert!(
            errs.iter().any(|e| e.message.contains("discriminante")),
            "mensaje inesperado: {errs:?}"
        );
    }

    /// La restriccion es SOLO de las variantes con datos: un struct corriente
    /// se emite como `interface`, ahi no hay discriminante que chocar.
    #[test]
    fn a_plain_struct_field_named_type_is_allowed() {
        check_source("type Lead = { id: Int, type: String, service: String }
db { leads: Lead[] }")
            .expect("un struct normal puede tener un campo 'type'");
    }

    /// `@requires(Role.Admin | Role.Agent)` (GRAMMAR.md §3.49): dos roles
    /// declarados, ambos existentes en el enum -- tiene que tipar limpio.
    #[test]
    fn requires_with_or_of_roles_typechecks() {
        check_source(
            "enum Role { Admin, Agent, Member }
service S {
  @requires(Role.Admin | Role.Agent)
  rpc panel() -> Int { 1 }
}",
        )
        .expect("un OR de roles del mismo enum debe tipar");
    }

    /// Cada alternativa del OR se valida contra el enum -- una que no
    /// existe es un error de tipos, no algo que se descubra en runtime
    /// como un 403 imposible de satisfacer.
    #[test]
    fn requires_with_or_of_roles_rejects_an_unknown_variant() {
        let errs = check_source(
            "enum Role { Admin, Agent }
service S {
  @requires(Role.Admin | Role.Nonexistent)
  rpc panel() -> Int { 1 }
}",
        )
        .expect_err("una variante inexistente en el OR debe rechazarse");
        assert!(
            errs.iter().any(|e| e.message.contains("Nonexistent") && e.message.contains("no tiene una variante")),
            "mensaje inesperado: {errs:?}"
        );
    }

    /// Mezclar dos enums en un mismo `@requires` no tiene significado (una
    /// sesión tiene el rol de UN enum) -- se rechaza en el PARSER, no acá,
    /// porque es puramente sintáctico (comparar identificadores).
    #[test]
    fn requires_with_or_across_two_different_enums_is_a_parse_error() {
        let src = "enum Role { Admin }
enum Status { Active }
service S {
  @requires(Role.Admin | Status.Active)
  rpc panel() -> Int { 1 }
}";
        let tokens = tokenize(src).unwrap_or_else(|e| panic!("{e}"));
        let err = parse(tokens).expect_err("mezclar enums en un OR debe rechazarse en el parser");
        assert!(
            err.iter().any(|e| e.message.contains("mezcla dos enums distintos")),
            "mensaje inesperado: {err:?}"
        );
    }

    /// Una variante SIN datos no lleva payload, asi que su nombre nunca choca.
    #[test]
    fn a_unit_variant_is_unaffected() {
        check_source("enum Status { Nuevo, Cerrado }
type T = { id: Int, s: Status }")
            .expect("las variantes unitarias no tienen campos");
    }

    // ---- hover de expresión arbitraria (GRAMMAR.md §3.24) ----

    #[test]
    fn hover_on_a_param_reference_inside_a_comparison_gives_the_param_type_not_the_comparisons_bool() {
        // El caso decisivo para "gana el span más específico" en
        // `probe_hover`: `x > 5` sintetiza Bool, pero hoverear sobre `x`
        // (el operando, un span más angosto) debe dar Int -- si
        // "última escritura gana" en vez de "el span más chico gana",
        // esto daría Bool (el bug real que el diseño de `probe_hover`
        // evita, encontrado analizando el orden de recursión ANTES de
        // implementarlo, no después).
        let src = "fn f(x: Int) -> Bool { x > 5 }";
        assert_eq!(hover_at(src, "x >"), Some(Type::Int));
    }

    #[test]
    fn hover_on_the_whole_comparison_gives_bool() {
        // El offset del operador '>' en sí -- no cubierto por el span de
        // NINGUNO de los dos operandos (`x` termina antes, `5` empieza
        // después), pero sí por el de la comparación completa. Apuntar a
        // "x > 5" en cambio daría el offset de 'x' (Int), no de la
        // comparación -- ver el test anterior para exactamente ese caso.
        let src = "fn f(x: Int) -> Bool { x > 5 }";
        assert_eq!(hover_at(src, "> 5"), Some(Type::Bool));
    }

    #[test]
    fn hover_on_an_if_expression_gives_the_expected_type_from_checking_mode() {
        // if/else no sintetiza un tipo propio -- se CHEQUEA contra
        // `expected` (GRAMMAR.md §3.7, modo ⇐). Prueba que el gate de
        // `check_expr` (no solo `synth_expr`) también alimenta el probe.
        let src = "fn f() -> Int { if true { 1 } else { 2 } }";
        assert_eq!(hover_at(src, "if true"), Some(Type::Int));
    }

    #[test]
    fn hover_inside_an_rpc_body_works_too() {
        let src = r#"service S { rpc f() -> Int { 1 + 1 } }"#;
        assert_eq!(hover_at(src, "1 + 1"), Some(Type::Int));
    }

    #[test]
    fn hover_outside_any_body_returns_none() {
        let src = "fn f() -> Int { 1 }";
        let tokens = tokenize(src).unwrap();
        let program = parse(tokens).unwrap();
        let offset = 0; // la 'f' de 'fn', fuera de cualquier body
        assert_eq!(Checker::hover_type_at(&program, offset), None);
    }

    #[test]
    fn hover_stops_at_an_earlier_error_in_the_same_body() {
        // Límite honesto documentado en `hover_type_at`: `check_fn` para
        // en el PRIMER error -- una expresión hovereada DESPUÉS de un
        // error anterior en el mismo body nunca se llega a chequear.
        let src = "fn f() -> Int { let x: Int = \"nope\"; 1 + 1 }";
        assert_eq!(hover_at(src, "1 + 1"), None, "el error anterior en 'let x' para el chequeo antes de llegar acá");
    }

    #[test]
    fn hover_on_the_tail_expression_gives_its_real_type_even_when_it_mismatches_the_declared_return_type() {
        // Bug real encontrado implementando completion (§3.25, que reusa
        // hover_type_at): el TAIL de un body se chequea en modo ⇐ contra
        // el tipo de retorno declarado (`check_expr`); si NO matchea
        // (acá, List(Int) contra el Int declarado), el chequeo falla --
        // pero la SÍNTESIS de esa misma expresión sí había tenido éxito
        // (es justamente lo que el mensaje de error reporta: "se
        // encontró List(Int)"). El hover debe mostrar ese tipo real
        // (List(Int)), no quedarse sin nada solo porque el chequeo
        // contra `expected` falló -- `check_expr`'s propio probe
        // reintenta sintetizar cuando el chequeo no tuvo éxito,
        // exactamente para este caso.
        let src = "fn f(xs: Int[]) -> Int { xs }";
        assert_eq!(hover_at(src, "xs }"), Some(Type::List(Box::new(Type::Int))));
    }

    #[test]
    fn full_users_demo_file_typechecks() {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/users.link"),
        )
        .expect("no se pudo leer examples/users.link");
        let result = check_source(&src);
        assert!(result.is_ok(), "errores de tipo inesperados: {:#?}", result.unwrap_err());
    }

    #[test]
    fn missing_required_field_is_rejected() {
        let src = r#"
            type Point = { x: Int, y: Int }
            fn origin() -> Point { Point { x: 0 } }
        "#;
        let result = check_source(src);
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains('y'), "el error debería mencionar el campo faltante 'y': {msg}");
    }

    #[test]
    fn non_exhaustive_match_is_rejected() {
        let src = r#"
            enum Status { Active, Paused, Cancelled }
            fn describe(s: Status) -> String {
                match s {
                    Status.Active => "activo",
                    Status.Paused => "pausado",
                }
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("Cancelled"), "debería señalar el variant faltante: {msg}");
    }

    #[test]
    fn wildcard_arm_satisfies_exhaustiveness() {
        let src = r#"
            enum Status { Active, Paused, Cancelled }
            fn describe(s: Status) -> String {
                match s {
                    Status.Active => "activo",
                    other => "otro",
                }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn or_pattern_combines_enum_variants_into_one_arm() {
        let src = r#"
            enum Status { Active, Paused, Cancelled }
            fn describe(s: Status) -> String {
                match s {
                    Status.Active | Status.Paused => "en curso",
                    Status.Cancelled => "cancelado",
                }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn or_pattern_branch_that_binds_a_field_is_rejected() {
        // Alcance v0 (ast.rs, doc de Pattern::Or): ninguna alternativa de un
        // '|' puede introducir bindings, así que combinar dos variantes que
        // capturan un campo -- aunque compartan nombre -- no está permitido.
        let src = r#"
            enum Shape { Circle { r: Int }, Square { r: Int } }
            fn area_hint(s: Shape) -> Int {
                match s {
                    Shape.Circle { r } | Shape.Square { r } => r,
                }
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "un patrón 'A | B' no debería poder bindear campos");
    }

    #[test]
    fn literal_match_over_int_requires_a_trailing_catch_all() {
        // Int tiene un espacio de valores no enumerable -- a diferencia de un
        // enum, ningún conjunto finito de literales agota el tipo (GRAMMAR.md §3.3).
        let src = r#"
            fn describe(n: Int) -> String {
                match n {
                    1 => "uno",
                    2 => "dos",
                }
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "literales sobre Int nunca deberían bastar sin un catch-all");
    }

    #[test]
    fn literal_match_over_int_with_wildcard_and_or_pattern_is_accepted() {
        let src = r#"
            fn describe(n: Int) -> String {
                match n {
                    1 | 2 => "bajo",
                    -1 => "negativo",
                    _ => "otro",
                }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn bool_match_covering_both_values_is_exhaustive_without_wildcard() {
        // Bool es, en los hechos, un enum de dos variantes -- único caso
        // donde literales solos (sin catch-all) sí alcanzan (GRAMMAR.md §3.3).
        let src = r#"
            fn describe(b: Bool) -> String {
                match b {
                    true => "sí",
                    false => "no",
                }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn string_literal_pattern_type_mismatch_is_rejected() {
        let src = r#"
            fn describe(n: Int) -> String {
                match n {
                    "uno" => "no debería tipar",
                    _ => "otro",
                }
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "un patrón String no debería aceptarse contra un escrutinio Int");
    }

    #[test]
    fn guarded_arm_alone_does_not_satisfy_exhaustiveness() {
        // Un guard puede fallar en runtime -- por más que su patrón sería un
        // catch-all sin el guard, no puede ser la única cobertura del match.
        let src = r#"
            fn describe(n: Int) -> String {
                match n {
                    x if x > 0 => "positivo",
                }
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "un solo arm con guard nunca es exhaustivo por sí solo");
    }

    #[test]
    fn guard_condition_must_synthesize_bool() {
        let src = r#"
            fn describe(n: Int) -> String {
                match n {
                    x if x => "no debería tipar",
                    _ => "otro",
                }
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "el guard 'if x' sobre un Int no es Bool");
    }

    #[test]
    fn guard_sees_the_bindings_introduced_by_its_own_pattern() {
        let src = r#"
            enum Setting { Level { value: Int } }
            fn describe(s: Setting) -> String {
                match s {
                    Setting.Level { value } if value > 10 => "alto",
                    Setting.Level { value } => "bajo",
                }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn wrong_argument_count_is_rejected() {
        let src = r#"
            fn add(a: Int, b: Int) -> Int { a }
            fn use_it() -> Int { add(1) }
        "#;
        let result = check_source(src);
        assert!(result.is_err());
    }

    #[test]
    fn assigning_to_mut_variable_is_accepted() {
        assert!(check_source(
            "fn f() -> Int { let mut x = 1; x = 2; x }"
        ).is_ok());
    }

    #[test]
    fn assigning_to_non_mut_variable_is_rejected() {
        let result = check_source("fn f() -> Int { let x = 1; x = 2; x }");
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("mut"), "el error debería mencionar 'mut': {msg}");
    }

    #[test]
    fn assigning_to_undeclared_variable_is_rejected() {
        assert!(check_source("fn f() -> Int { x = 2; 0 }").is_err());
    }

    #[test]
    fn assigning_wrong_type_is_rejected() {
        let result = check_source(r#"fn f() -> Int { let mut x = 1; x = "no"; x }"#);
        assert!(result.is_err());
    }

    #[test]
    fn array_literal_infers_from_first_element_and_checks_the_rest() {
        assert!(check_source("fn f() -> Int[] { [1, 2, 3] }").is_ok());
        assert!(check_source(r#"fn f() -> Int[] { [1, "no", 3] }"#).is_err());
    }

    #[test]
    fn empty_array_needs_an_expected_type() {
        assert!(check_source("fn f() -> Int[] { [] }").is_ok());
        // en posición de síntesis (sin contexto) debe fallar
        assert!(check_source("fn f() -> Int { let xs = []; 0 }").is_err());
    }

    #[test]
    fn indexing_returns_the_element_type_and_requires_int_index() {
        assert!(check_source("fn f() -> Int { let xs = [1, 2, 3]; xs[0] }").is_ok());
        assert!(check_source(r#"fn f() -> Int { let xs = [1, 2, 3]; xs["0"] }"#).is_err());
        assert!(check_source("fn f() -> Int { let x = 5; x[0] }").is_err()); // Int no es indexable
    }

    #[test]
    fn numeric_conversion_methods_work() {
        assert!(check_source("fn f(n: Int) -> Float { n.toFloat() }").is_ok());
        assert!(check_source("fn f(n: Float) -> Int { n.toInt() }").is_ok());
    }

    #[test]
    fn tuple_literal_synthesizes_and_index_returns_element_type() {
        assert!(check_source(r#"fn f() -> (Int, String) { (1, "a") }"#).is_ok());
        assert!(check_source(r#"fn f() -> Int { let t = (1, "a"); t.0 }"#).is_ok());
        assert!(check_source(r#"fn f() -> String { let t = (1, "a"); t.1 }"#).is_ok());
    }

    #[test]
    fn tuple_index_out_of_range_or_wrong_type_is_rejected() {
        assert!(check_source(r#"fn f() -> Int { let t = (1, "a"); t.2 }"#).is_err());
        assert!(check_source("fn f() -> Int { let x = 5; x.0 }").is_err());
    }

    #[test]
    fn string_length_and_contains_work() {
        assert!(check_source(r#"fn f(s: String) -> Int { s.length() }"#).is_ok());
        assert!(check_source(r#"fn f(s: String) -> Bool { s.contains("@") }"#).is_ok());
    }

    #[test]
    fn string_methods_reject_wrong_args() {
        assert!(check_source(r#"fn f(s: String) -> Int { s.length(1) }"#).is_err());
        assert!(check_source(r#"fn f(s: String) -> Bool { s.contains(1) }"#).is_err());
    }

    #[test]
    fn numeric_conversion_rejects_wrong_receiver_or_args() {
        assert!(check_source("fn f(n: Float) -> Float { n.toFloat() }").is_err()); // toFloat es de Int
        assert!(check_source("fn f(n: Int) -> Float { n.toFloat(1) }").is_err()); // no toma argumentos
    }

    // ---- Int64 (GRAMMAR.md §3.30) ----

    #[test]
    fn int64_round_trips_through_conversion_methods() {
        assert!(check_source("fn f(n: Int) -> Int64 { n.toInt64() }").is_ok());
        assert!(check_source("fn f(n: Int64) -> Int { n.toInt() }").is_ok());
    }

    #[test]
    fn int64_conversion_rejects_wrong_receiver_or_args() {
        assert!(check_source("fn f(n: Int64) -> Int64 { n.toInt64() }").is_err()); // toInt64 es de Int
        assert!(check_source("fn f(n: Int) -> Int64 { n.toInt64(1) }").is_err()); // no toma argumentos
    }

    #[test]
    fn int64_does_not_mix_implicitly_with_int_in_arithmetic_or_comparisons() {
        assert!(check_source("fn f(a: Int64, b: Int) -> Int64 { a + b }").is_err());
        assert!(check_source("fn f(a: Int64, b: Int) -> Bool { a < b }").is_err());
    }

    #[test]
    fn int64_supports_arithmetic_and_comparisons_between_two_int64() {
        assert!(check_source("fn f(a: Int64, b: Int64) -> Int64 { a + b }").is_ok());
        assert!(check_source("fn f(a: Int64, b: Int64) -> Int64 { a - b }").is_ok());
        assert!(check_source("fn f(a: Int64, b: Int64) -> Int64 { a * b }").is_ok());
        assert!(check_source("fn f(a: Int64, b: Int64) -> Bool { a < b }").is_ok());
        assert!(check_source("fn f(a: Int64, b: Int64) -> Bool { a == b }").is_ok());
        assert!(check_source("fn f(a: Int64) -> Int64 { -a }").is_ok());
    }

    #[test]
    fn int64_is_a_valid_match_scrutinee_with_literal_patterns() {
        assert!(check_source(
            "fn f(n: Int64) -> String { match n { 0 => \"zero\", other => \"other\" } }"
        )
        .is_ok());
        // Sin arm final que capture el resto: no exhaustivo, mismo criterio
        // que Int/String (GRAMMAR.md §3.3).
        assert!(check_source("fn f(n: Int64) -> String { match n { 0 => \"zero\" } }").is_err());
    }

    // ---- Decimal (GRAMMAR.md §3.184) ----

    #[test]
    fn decimal_round_trips_through_conversion_methods() {
        assert!(check_source("fn f(n: Int) -> Decimal { n.toDecimal() }").is_ok());
        assert!(check_source("fn f(n: Float) -> Decimal { n.toDecimal() }").is_ok());
        assert!(check_source("fn f(n: Decimal) -> Float { n.toFloat() }").is_ok());
        assert!(check_source("fn f(n: Decimal) -> String { n.toString() }").is_ok());
    }

    #[test]
    fn decimal_conversion_rejects_wrong_receiver_or_args() {
        assert!(check_source("fn f(n: Decimal) -> Decimal { n.toDecimal() }").is_err(), "toDecimal es de Int/Float, no de Decimal");
        assert!(check_source("fn f(n: Int) -> Decimal { n.toDecimal(1) }").is_err(), "no toma argumentos");
        // Deliberado: sin .toInt() -- perdería la parte fraccionaria en
        // silencio (a diferencia de Int64, mismo ancho que Int).
        assert!(check_source("fn f(n: Decimal) -> Int { n.toInt() }").is_err());
    }

    #[test]
    fn decimal_does_not_mix_implicitly_with_float_or_int_in_arithmetic_or_comparisons() {
        assert!(check_source("fn f(a: Decimal, b: Float) -> Decimal { a + b }").is_err());
        assert!(check_source("fn f(a: Decimal, b: Int) -> Bool { a < b }").is_err());
    }

    #[test]
    fn decimal_supports_arithmetic_and_comparisons_between_two_decimals() {
        assert!(check_source("fn f(a: Decimal, b: Decimal) -> Decimal { a + b }").is_ok());
        assert!(check_source("fn f(a: Decimal, b: Decimal) -> Decimal { a - b }").is_ok());
        assert!(check_source("fn f(a: Decimal, b: Decimal) -> Decimal { a * b }").is_ok());
        assert!(check_source("fn f(a: Decimal, b: Decimal) -> Decimal { a / b }").is_ok());
        assert!(check_source("fn f(a: Decimal, b: Decimal) -> Bool { a < b }").is_ok());
        assert!(check_source("fn f(a: Decimal, b: Decimal) -> Bool { a == b }").is_ok());
        assert!(check_source("fn f(a: Decimal) -> Decimal { -a }").is_ok());
    }

    #[test]
    fn decimal_rejects_the_modulo_operator_with_a_clear_message() {
        let errors = check_source("fn f(a: Decimal, b: Decimal) -> Decimal { a % b }")
            .expect_err("'%' sobre Decimal no tiene semántica bien definida (GRAMMAR.md §3.184)");
        assert!(
            errors[0].to_string().contains("Decimal no soporta"),
            "el mensaje debe explicar por qué, no solo 'tipo equivocado': {:?}",
            errors[0]
        );
    }

    #[test]
    fn check_min_max_range_applies_to_a_decimal_field() {
        let src = r#"
            type Invoice = {
                id: Int,
                @check(min, 0) subtotal: Decimal,
                @check(range, 0, 100) taxRate: Decimal?
            }
            db { invoices: Invoice[] }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    // ---- Timestamp (GRAMMAR.md §3.31) ----

    #[test]
    fn timestamp_supports_comparisons_between_two_timestamps() {
        assert!(check_source("fn f(a: Timestamp, b: Timestamp) -> Bool { a < b }").is_ok());
        assert!(check_source("fn f(a: Timestamp, b: Timestamp) -> Bool { a <= b }").is_ok());
        assert!(check_source("fn f(a: Timestamp, b: Timestamp) -> Bool { a > b }").is_ok());
        assert!(check_source("fn f(a: Timestamp, b: Timestamp) -> Bool { a >= b }").is_ok());
        assert!(check_source("fn f(a: Timestamp, b: Timestamp) -> Bool { a == b }").is_ok());
    }

    #[test]
    fn timestamp_rejects_arithmetic_and_unary_negation() {
        // Sin tipo Duration -- sumar/restar/etc sobre Timestamp no tiene un
        // significado definido en v0 (GRAMMAR.md §3.31). Pasarlo tal cual
        // (sin operar) sigue siendo válido, por supuesto.
        assert!(check_source("fn f(a: Timestamp, b: Timestamp) -> Timestamp { a }").is_ok());
        assert!(check_source("fn f(a: Timestamp, b: Timestamp) -> Timestamp { a + b }").is_err());
        assert!(check_source("fn f(a: Timestamp) -> Timestamp { -a }").is_err());
    }

    #[test]
    fn timestamp_is_not_a_valid_match_scrutinee() {
        // Mismo criterio que Float: sin igualdad exacta útil como base de
        // exhaustividad (GRAMMAR.md §3.31).
        assert!(check_source(
            "fn f(t: Timestamp) -> String { match t { other => \"x\" } }"
        )
        .is_err());
    }

    #[test]
    fn timestamp_has_no_conversion_methods() {
        assert!(check_source("fn f(t: Timestamp) -> Int { t.toInt() }").is_err());
    }

    #[test]
    fn timestamp_is_a_valid_rpc_param_and_field_type() {
        assert!(check_source(
            "type Event = { at: Timestamp }\nservice S { rpc log(at: Timestamp) -> Event { Event { at: at } } }"
        )
        .is_ok());
    }

    #[test]
    fn now_builtin_returns_timestamp_and_rejects_arguments() {
        assert!(check_source("fn current() -> Timestamp { now() }").is_ok());
        assert!(check_source("fn current() -> Timestamp { let f = now; f() }").is_ok());
        assert!(check_source("fn bad() -> Timestamp { now(123) }").is_err());
        assert!(check_source("fn bad_ret() -> Int { now() }").is_err());
    }

    /// `dateFromParts(year, month, day, hour, minute, second) -> Timestamp`
    /// (GRAMMAR.md §3.90).
    #[test]
    fn date_from_parts_builtin_takes_six_ints_and_returns_timestamp() {
        assert!(check_source("fn boundary() -> Timestamp { dateFromParts(2026, 1, 1, 0, 0, 0) }").is_ok());
        // Como `now`, referenciable como valor de primera clase.
        assert!(check_source("fn boundary() -> Timestamp { let f = dateFromParts; f(2026, 1, 1, 0, 0, 0) }").is_ok());

        let too_few = check_source("fn bad() -> Timestamp { dateFromParts(2026, 1, 1) }");
        assert!(too_few.is_err());

        let wrong_type = check_source("fn bad() -> Timestamp { dateFromParts(\"2026\", 1, 1, 0, 0, 0) }");
        assert!(wrong_type.is_err());

        let wrong_ret = check_source("fn bad() -> Int { dateFromParts(2026, 1, 1, 0, 0, 0) }");
        assert!(wrong_ret.is_err());
    }

    /// GRAMMAR.md §3.196 -- aritmética de `Timestamp` vía método, `n`
    /// negativo resta (sin un `.subtract*` separado).
    #[test]
    fn timestamp_arithmetic_methods_take_the_right_int_type_and_return_timestamp() {
        assert!(check_source("fn f(t: Timestamp) -> Timestamp { t.addMillis(1000.toInt64()) }").is_ok());
        assert!(check_source("fn f(t: Timestamp) -> Timestamp { t.addMillis(1000) }").is_err(), "addMillis toma Int64, sin mezcla implícita con un literal Int (mismo criterio que el resto del lenguaje)");
        assert!(check_source("fn f(t: Timestamp) -> Timestamp { t.addSeconds(30) }").is_ok());
        assert!(check_source("fn f(t: Timestamp) -> Timestamp { t.addMinutes(5) }").is_ok());
        assert!(check_source("fn f(t: Timestamp) -> Timestamp { t.addHours(-2) }").is_ok(), "n negativo tipa igual -- resta en runtime");
        assert!(check_source("fn f(t: Timestamp) -> Timestamp { t.addDays(1) }").is_ok());
    }

    #[test]
    fn timestamp_arithmetic_methods_reject_the_wrong_argument_type() {
        assert!(check_source("fn f(t: Timestamp) -> Timestamp { t.addMinutes(\"5\") }").is_err());
        assert!(check_source("fn f(t: Timestamp) -> Timestamp { t.addDays() }").is_err(), "sin argumento tiene que rechazarse, no asumir 0");
    }

    #[test]
    fn timestamp_arithmetic_composes_with_comparison_for_a_real_otp_expiry_check() {
        // El caso real reportado por un adoptador (MyFinance): expiración
        // de OTP de 2FA, `now() < issuedAt.addMinutes(5)`.
        let src = r#"
            fn stillValid(issuedAt: Timestamp) -> Bool { now() < issuedAt.addMinutes(5) }
        "#;
        assert!(check_source(src).is_ok());
    }

    // ---- GRAMMAR.md §3.198: String.substring/replace/split/padStart/padEnd ----

    #[test]
    fn string_substring_takes_two_ints_and_returns_string() {
        assert!(check_source("fn f(s: String) -> String { s.substring(0, 3) }").is_ok());
        assert!(check_source("fn f(s: String) -> String { s.substring(0) }").is_err(), "faltando 'end'");
        assert!(check_source("fn f(s: String) -> String { s.substring(\"0\", 3) }").is_err(), "start tiene que ser Int");
    }

    #[test]
    fn string_replace_takes_two_strings_and_returns_string() {
        assert!(check_source("fn f(s: String) -> String { s.replace(\";\", \",\") }").is_ok());
        assert!(check_source("fn f(s: String) -> String { s.replace(\";\") }").is_err());
        assert!(check_source("fn f(s: String) -> String { s.replace(1, \",\") }").is_err());
    }

    #[test]
    fn string_split_takes_a_string_and_returns_list_of_string() {
        assert!(check_source("fn f(s: String) -> String[] { s.split(\",\") }").is_ok(), "{:?}", check_source("fn f(s: String) -> String[] { s.split(\",\") }"));
        assert!(check_source("fn f(s: String) -> Int[] { s.split(\",\") }").is_err(), "split devuelve String[], no Int[]");
    }

    #[test]
    fn string_pad_start_and_pad_end_take_an_int_and_a_string_and_return_string() {
        assert!(check_source("fn f(s: String) -> String { s.padStart(10, \"0\") }").is_ok());
        assert!(check_source("fn f(s: String) -> String { s.padEnd(10, \"0\") }").is_ok());
        assert!(check_source("fn f(s: String) -> String { s.padStart(\"10\", \"0\") }").is_err(), "length tiene que ser Int");
        assert!(check_source("fn f(s: String) -> String { s.padEnd(10, 0) }").is_err(), "pad tiene que ser String");
    }

    /// Los dos casos reales citados por un adoptador (MyFinance): sanear
    /// `;`/saltos de línea antes de unir con `;` (ContaPlus/XDIARIO), y
    /// padding fixed-width (A3 Contable).
    #[test]
    fn string_methods_compose_for_the_real_contable_export_use_cases() {
        let src = r#"
            fn sanitizeForCsv(concepto: String) -> String {
                concepto.replace(";", ",").replace("\n", " ")
            }
            fn fixedWidthAmount(amount: String) -> String {
                amount.padStart(12, "0")
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    /// `sitemapXml(urls: {loc, lastmod?}[]) -> String` (GRAMMAR.md §3.116) --
    /// mismo criterio estructural que `http.getWithHeaders` (checker.rs,
    /// `http_header_type`): cualquier `type` declarado por el programa con
    /// estos campos exactos sirve, sin que el lenguaje tenga que inventar
    /// un `SitemapEntry` propio.
    #[test]
    fn sitemap_xml_accepts_any_struct_shaped_like_loc_and_optional_lastmod() {
        let src = r#"
            type Page = { loc: String, lastmod?: Timestamp }
            fn f() -> String { sitemapXml([Page { loc: "https://x.com/" }]) }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn sitemap_xml_rejects_a_struct_missing_loc() {
        let src = r#"
            type Page = { title: String }
            fn f() -> String { sitemapXml([Page { title: "sin loc" }]) }
        "#;
        assert!(check_source(src).is_err());
    }

    /// `robotsTxt(rules: {userAgent, disallow?, allow?}[], sitemapUrl:
    /// String?) -> String` (GRAMMAR.md §3.116).
    #[test]
    fn robots_txt_accepts_any_struct_shaped_like_user_agent_with_optional_lists() {
        let src = r#"
            type Rule = { userAgent: String, disallow?: String[] }
            fn f() -> String { robotsTxt([Rule { userAgent: "GPTBot", disallow: ["/"] }], null) }
        "#;
        assert!(check_source(src).is_ok());
        // `disallow`/`allow` omitidos del todo -- el struct del programa ni
        // siquiera necesita declararlos si nunca los usa (mismo criterio
        // que cualquier campo opcional del lado del supertipo).
        let src2 = r#"
            type Rule = { userAgent: String }
            fn f() -> String { robotsTxt([Rule { userAgent: "*" }], "https://x.com/sitemap.xml") }
        "#;
        assert!(check_source(src2).is_ok());
    }

    #[test]
    fn robots_txt_rejects_the_wrong_number_of_arguments() {
        let src = r#"
            type Rule = { userAgent: String }
            fn f() -> String { robotsTxt([Rule { userAgent: "*" }]) }
        "#;
        assert!(check_source(src).is_err());
    }

    /// `smtp.sendMessage(message)` (GRAMMAR.md §3.141) acepta cualquier
    /// struct con la forma correcta -- `cc`/`bcc`/`html`/`attachments`
    /// opcionales-POR-CLAVE (`x?: T`, se pueden omitir del todo), mismo
    /// criterio que `disallow`/`allow` de `robots_rule_type`.
    #[test]
    fn smtp_send_message_accepts_the_minimal_shape_without_cc_bcc_html_or_attachments() {
        let src = r#"
            type Msg = { to: String[], subject: String, body: String }
            service Sys {
                rpc notify() -> Void { smtp.sendMessage(Msg { to: ["a@x.com"], subject: "s", body: "b" }) }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn smtp_send_message_accepts_the_full_shape_with_attachments() {
        let src = r#"
            type Att = { filename: String, contentType: String, contentBase64: String }
            type Msg = { to: String[], cc?: String[], bcc?: String[], subject: String, body: String, html?: Bool, attachments?: Att[] }
            service Sys {
                rpc notify() -> Void {
                    smtp.sendMessage(Msg {
                        to: ["a@x.com"], cc: ["b@x.com"], bcc: ["c@x.com"], subject: "s", body: "b", html: true,
                        attachments: [Att { filename: "f.txt", contentType: "text/plain", contentBase64: "aGk=" }],
                    })
                }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn smtp_send_message_rejects_a_struct_missing_to() {
        let src = r#"
            type Msg = { subject: String, body: String }
            service Sys {
                rpc notify() -> Void { smtp.sendMessage(Msg { subject: "s", body: "b" }) }
            }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn smtp_send_message_rejects_a_value_optional_cc_where_a_key_optional_field_is_expected() {
        // `cc: String[]?` (valor opcional) NO es lo mismo que `cc?: String[]`
        // (clave opcional, GRAMMAR.md §3.4) -- `T?` nunca es subtipo de `T`.
        let src = r#"
            type Msg = { to: String[], cc: String[]?, subject: String, body: String }
            service Sys {
                rpc notify() -> Void { smtp.sendMessage(Msg { to: ["a@x.com"], cc: null, subject: "s", body: "b" }) }
            }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn smtp_send_message_rejects_the_wrong_number_of_arguments() {
        let src = r#"
            type Msg = { to: String[], subject: String, body: String }
            service Sys {
                rpc notify() -> Void { smtp.sendMessage(Msg { to: ["a@x.com"], subject: "s", body: "b" }, "extra") }
            }
        "#;
        assert!(check_source(src).is_err());
    }

    /// `metaTags`/`openGraphTags`/`canonicalLink`/`jsonLd` (GRAMMAR.md
    /// §3.117), mismo criterio estructural que `sitemapXml`/`robotsTxt` --
    /// cualquier `type` con la forma correcta sirve.
    #[test]
    fn meta_tags_accepts_any_struct_shaped_like_name_and_content() {
        let src = r#"
            type Meta = { name: String, content: String }
            fn f() -> String { metaTags([Meta { name: "description", content: "hola" }]) }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn meta_tags_rejects_a_struct_using_property_instead_of_name() {
        let src = r#"
            type Meta = { property: String, content: String }
            fn f() -> String { metaTags([Meta { property: "og:title", content: "hola" }]) }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn open_graph_tags_accepts_any_struct_shaped_like_property_and_content() {
        let src = r#"
            type Og = { property: String, content: String }
            fn f() -> String { openGraphTags([Og { property: "og:title", content: "hola" }]) }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn canonical_link_takes_one_string_and_returns_string() {
        assert!(check_source(r#"fn f() -> String { canonicalLink("https://x.com/") }"#).is_ok());
        assert!(check_source(r#"fn f() -> String { canonicalLink(1) }"#).is_err());
    }

    #[test]
    fn json_ld_accepts_any_value_via_dynamic() {
        // `Dynamic` es la escotilla de escape deliberada del lenguaje -- un
        // `String`, un `Int`, o el resultado de `json.parse` tipan igual acá.
        assert!(check_source(r#"fn f() -> String { jsonLd("hola") }"#).is_ok());
        assert!(check_source(r#"fn f() -> String { jsonLd(json.parse("{}")) }"#).is_ok());
    }

    #[test]
    fn map_of_string_int_is_accepted() {
        // Bug real: esto estaba documentado en GRAMMAR.md como el reemplazo
        // de {K:V} pero nunca se conectó al checker -- tiraba "tipo
        // desconocido: 'Map'" antes de este fix.
        assert!(check_source("fn f(m: Map<String, Int>) -> Int { 0 }").is_ok());
        assert!(check_source("fn f(m: Map<Int, String>) -> Int { 0 }").is_ok());
    }

    #[test]
    fn map_rejects_non_json_key_types() {
        let result = check_source("fn f(m: Map<Bool, Int>) -> Int { 0 }");
        assert!(result.is_err());
    }

    #[test]
    fn generic_struct_instantiates_constructs_and_accesses_fields() {
        let src = r#"
            type Box<T> = { value: T }
            fn wrap(n: Int) -> Box<Int> { Box { value: n } }
            fn unwrap(b: Box<Int>) -> Int { b.value }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn generic_enum_instantiates_constructs_matches_exhaustively() {
        let src = r#"
            enum Option<T> {
                Some { value: T },
                None,
            }
            fn find(has_it: Bool, n: Int) -> Option<Int> {
                // Option.None necesita "{}" explícito COMO EXPRESIÓN (el
                // lookahead del parser solo reconoce un literal de variante
                // si ve "{" después) -- distinto del patrón de match, que
                // no lo exige para una variante sin campos.
                if has_it { Option.Some { value: n } } else { Option.None {} }
            }
            fn unwrap_or(o: Option<Int>, default: Int) -> Int {
                match o {
                    Option.Some { value: v } => v,
                    Option.None => default,
                }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn bare_unit_enum_variant_is_now_valid_as_a_value_no_braces_needed() {
        // GRAMMAR.md §3.209 (evolución de §3.206): antes de §3.206 esto daba
        // "variable no declarada: 'Role'" -- activamente engañoso, porque
        // `Role` SÍ está declarado (como enum). §3.206 mejoró el MENSAJE
        // (pedía agregar `{}`); §3.209 va más allá y elimina la asimetría de
        // raíz -- una variante SIN campos no necesita llaves para nada,
        // `Role.Admin` y `Role.Admin {}` son ahora la MISMA expresión válida.
        let src = r#"
            enum Role { Admin, Member }
            type User = { id: Int, role: Role }
            fn make() -> User { User { id: 1, role: Role.Admin } }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn bare_data_carrying_enum_variant_still_needs_braces_no_values_to_infer() {
        // A diferencia del caso de arriba, una variante CON campos no puede
        // omitir las llaves -- no hay de dónde sacar los valores. Sigue
        // siendo un error, pero dirigido (GRAMMAR.md §3.209), nunca el
        // "variable no declarada" genérico de antes de §3.206.
        let src = r#"
            enum Outcome { Good { value: Int }, Bad }
            fn f() -> Outcome { Outcome.Good }
        "#;
        let errs = check_source(src).unwrap_err();
        assert!(errs[0].message.contains("es una variante de enum CON campos"), "{}", errs[0].message);
        assert!(errs[0].message.contains("Outcome.Good { ... }"), "{}", errs[0].message);
        assert!(!errs[0].message.contains("variable no declarada"), "{}", errs[0].message);
    }

    #[test]
    fn bare_reference_to_a_nonexistent_variant_suggests_the_real_one() {
        // Tercer caso: `base_name` SÍ es un enum conocido, pero `field` no
        // nombra ninguna de sus variantes -- ya sabemos que no es una
        // variable, así que "variable no declarada" seguiría siendo la
        // respuesta equivocada acá también.
        let src = r#"
            enum Role { Admin, Member }
            fn f() -> Role { Role.Admn }
        "#;
        let errs = check_source(src).unwrap_err();
        assert!(errs[0].message.contains("no tiene ninguna variante 'Admn'"), "{}", errs[0].message);
        assert!(errs[0].message.contains("¿quisiste decir 'Role.Admin'?"), "{}", errs[0].message);
    }

    #[test]
    fn bare_unit_variant_of_a_generic_enum_infers_type_args_from_context() {
        // GRAMMAR.md §3.209: sin el brazo dedicado en `check_expr_inner`,
        // esto fallaba con "'Maybe' es genérico -- necesita un tipo
        // esperado..." aunque el `expected` SÍ estaba disponible en este
        // contexto (la otra rama del `if` ya fija `Maybe<Int>`) -- el
        // fallback de síntesis pura no lo propagaba. La forma CON llaves
        // (`Maybe.Nothing {}`) ya funcionaba acá antes de este ítem.
        let src = r#"
            enum Maybe<T> { Just { value: T }, Nothing }
            fn f(has: Bool) -> Maybe<Int> {
                if has { Maybe.Just { value: 1 } } else { Maybe.Nothing }
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn a_real_local_variable_shadowing_an_enum_name_still_resolves_field_access_normally() {
        // Guarda de no-regresión del fix de arriba: `!env.contains_key`
        // tiene que ganar ANTES del chequeo de enum, para no romper el
        // caso (raro pero válido) de una variable local que sombree el
        // nombre de un enum -- acá `Role` es un parámetro de tipo `Foo`,
        // no el enum, y `Role.Admin` es un acceso a campo legítimo.
        let src = r#"
            enum Role { Admin, Member }
            type Foo = { Admin: Int }
            fn make(Role: Foo) -> Int { Role.Admin }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn generic_match_still_requires_exhaustiveness() {
        let src = r#"
            enum Option<T> { Some { value: T }, None }
            fn f(o: Option<Int>) -> Int {
                match o {
                    Option.Some { value: v } => v,
                }
            }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn generic_construction_without_context_is_rejected() {
        // Igual que Result: no hay de dónde inferir los argumentos de tipo
        // sin un `expected` -- síntesis pura no alcanza (GRAMMAR.md §3.6).
        let src = r#"
            type Box<T> = { value: T }
            fn f() -> Int {
                let b = Box { value: 1 };
                0
            }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn generic_wrong_arg_count_is_rejected() {
        let src = r#"
            type Pair<A, B> = { first: A, second: B }
            fn f(p: Pair<Int>) -> Int { 0 }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn different_generic_instantiations_are_not_interchangeable() {
        // Decisión deliberada (GRAMMAR.md §3.6): una vez genérico, la
        // comparación es NOMINAL (nombre + args), no estructural -- aunque
        // Box<Int> y un struct plano {value: Int} tengan la misma forma,
        // no son intercambiables.
        let src = r#"
            type Box<T> = { value: T }
            fn takes_box(b: Box<Int>) -> Int { b.value }
            fn f(plain: { value: Int }) -> Int { takes_box(plain) }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn patch_of_non_struct_is_rejected() {
        let src = r#"
            fn f(p: Patch<Int>) -> Int { 0 }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "Patch<Int> no debería aceptarse: T tiene que ser un struct");
    }

    #[test]
    fn patch_of_struct_is_accepted_and_widens_all_fields() {
        let src = r#"
            type User = { name: String, bio?: String }
            fn apply(id: Int, patch: Patch<User>) -> User {
                User { name: "x" }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn structurally_equivalent_inline_type_accepted() {
        // Un `type A` con la MISMA forma que el tipo inline del parámetro
        // debe aceptarse — subtipado estructural, no nominal (GRAMMAR.md §3.2).
        let src = r#"
            type A = { x: Int }
            fn f(v: { x: Int }) -> Int { v.x }
            fn use_it() -> Int { f(A { x: 1 }) }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn const_value_matching_its_declared_type_is_accepted() {
        // Hallado sin chequear durante la auditoría final: check_program
        // ignoraba Item::Const del todo (GRAMMAR.md §2.1/§4).
        assert!(check_source("const MAX_RETRIES: Int = 3;").is_ok());
    }

    #[test]
    fn const_value_not_matching_its_declared_type_is_rejected() {
        let result = check_source(r#"const MAX_RETRIES: Int = "tres";"#);
        assert!(result.is_err(), "un const declarado Int no debería aceptar un valor String");
    }

    #[test]
    fn arithmetic_ok_same_numeric_type() {
        assert!(check_source("fn add(a: Int, b: Int) -> Int { a + b * 2 - 1 }").is_ok());
        assert!(check_source("fn add(a: Float, b: Float) -> Float { a / b }").is_ok());
    }

    #[test]
    fn plus_concatenates_strings_but_other_arithmetic_ops_reject_them() {
        assert!(check_source(r#"fn greet(name: String) -> String { "hola, " + name }"#).is_ok());
        assert!(check_source(r#"fn f(a: String, b: String) -> String { a - b }"#).is_err());
    }

    #[test]
    fn arithmetic_rejects_mixed_int_and_float() {
        // GRAMMAR.md §3.7: sin coerción implícita -- Int y Float no se mezclan.
        let result = check_source("fn f(a: Int, b: Float) -> Float { a + b }");
        assert!(result.is_err());
    }

    #[test]
    fn comparison_and_logical_operators_produce_bool() {
        let src = r#"
            fn f(a: Int, b: Int) -> Bool {
                a < b && a != b || !(a == b)
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn logical_operators_reject_non_bool_operands() {
        let result = check_source("fn f(a: Int, b: Int) -> Bool { a && b }");
        assert!(result.is_err());
    }

    #[test]
    fn if_else_both_branches_must_match_expected_type() {
        assert!(check_source("fn f(x: Int) -> Int { if x > 0 { x } else { 0 } }").is_ok());

        // La rama else devuelve String donde se esperaba Int -- debe fallar.
        let result = check_source(r#"fn f(x: Int) -> Int { if x > 0 { x } else { "no" } }"#);
        assert!(result.is_err());
    }

    #[test]
    fn if_condition_must_be_bool() {
        let result = check_source("fn f(x: Int) -> Int { if x { 1 } else { 0 } }");
        assert!(result.is_err());
    }

    #[test]
    fn else_if_chain_typechecks() {
        let src = r#"
            fn classify(x: Int) -> String {
                if x > 0 { "positivo" } else if x < 0 { "negativo" } else { "cero" }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn concrete_member_of_a_union_param_is_accepted() {
        // Alcance v0 (types.rs, doc de Type::Union): flujo de valor hacia
        // un parámetro/campo tipado como unión, sin angosto posterior.
        let src = r#"
            fn f(x: Int | String) -> Int { 0 }
            fn use_it() -> Int { f(1) }
        "#;
        assert!(check_source(src).is_ok());
        let src2 = r#"
            fn f(x: Int | String) -> Int { 0 }
            fn use_it() -> Int { f("hola") }
        "#;
        assert!(check_source(src2).is_ok());
    }

    #[test]
    fn non_member_type_is_rejected_by_union_param() {
        let src = r#"
            fn f(x: Int | String) -> Int { 0 }
            fn use_it() -> Int { f(true) }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "Bool no es miembro de Int | String");
    }

    #[test]
    fn union_field_in_struct_is_accepted() {
        let src = r#"
            type Event = { payload: Int | String }
            fn make() -> Event { Event { payload: 1 } }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn named_fn_referenced_by_name_synthesizes_a_function_type() {
        // GRAMMAR.md §3.10: una `fn` de nivel superior referenciada por
        // nombre (sin llamarla ahí mismo) es un valor de tipo Function --
        // Expr::Ident cae a `self.fns` cuando no hay binding local con ese
        // nombre. Ver runtime/mod.rs para la contraparte en ejecución (FnRef).
        let src = r#"
            fn add_one(x: Int) -> Int { x + 1 }
            fn apply_twice(f: (Int) -> Int, x: Int) -> Int { f(f(x)) }
            fn use_it() -> Int { apply_twice(add_one, 5) }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn fn_reference_with_incompatible_signature_is_rejected() {
        let src = r#"
            fn add_one(x: Int) -> Int { x + 1 }
            fn apply_to_bool(f: (Bool) -> Bool, x: Bool) -> Bool { f(x) }
            fn use_it() -> Bool { apply_to_bool(add_one, true) }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "(Int)->Int no debería servir donde se pide (Bool)->Bool");
    }

    #[test]
    fn db_collection_without_id_field_is_rejected() {
        let src = r#"
            type Post = { title: String }
            db { posts: Post[] }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "toda colección de db necesita un campo 'id: Int'");
    }

    // ---- `id: Uuid` como PK alternativa (GRAMMAR.md §3.177) ----

    #[test]
    fn db_collection_with_uuid_id_field_is_accepted() {
        let src = r#"
            type Lead = { id: Uuid, email: String }
            db { leads: Lead[] }
            fn one(id: Uuid) -> Lead? { db.leads.find(id) }
        "#;
        assert!(check_source(src).is_ok(), "'id: Uuid' tiene que ser una PK válida, igual que 'id: Int'");
    }

    #[test]
    fn db_find_on_a_uuid_pk_collection_rejects_an_int_argument() {
        let src = r#"
            type Lead = { id: Uuid, email: String }
            db { leads: Lead[] }
            fn one(n: Int) -> Lead? { db.leads.find(n) }
        "#;
        let errors = check_source(src).unwrap_err();
        assert!(!errors.is_empty(), "un Int no puede ser el id de una colección con PK Uuid");
    }

    #[test]
    fn db_find_on_an_int_pk_collection_still_rejects_a_uuid_argument() {
        let src = r#"
            type Post = { id: Int, title: String }
            db { posts: Post[] }
            fn one(u: Uuid) -> Post? { db.posts.find(u) }
        "#;
        let errors = check_source(src).unwrap_err();
        assert!(!errors.is_empty(), "un Uuid no puede ser el id de una colección con PK Int -- el camino de siempre no debe aflojar");
    }

    #[test]
    fn db_apply_patch_delete_and_increment_accept_a_uuid_id_on_a_uuid_pk_collection() {
        let src = r#"
            type Lead = { id: Uuid, score: Int }
            db { leads: Lead[] }
            fn patch(id: Uuid, p: Patch<Lead>) -> Lead { db.leads.applyPatch(id, p) }
            fn del(id: Uuid) -> Bool { db.leads.delete(id) }
            fn bump(id: Uuid) -> Lead { db.leads.increment(id, |l: Lead| { l.score }, 1) }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src).unwrap_err());
    }

    #[test]
    fn page_after_is_rejected_on_a_uuid_pk_collection_with_a_clear_message() {
        // GRAMMAR.md §3.177: rechazado a propósito, no "todavía no
        // soportado" -- la garantía de pageAfter depende de que el id
        // crezca en el mismo orden que la inserción, falso para un Uuid
        // aleatorio.
        let src = r#"
            type Lead = { id: Uuid, email: String }
            db { leads: Lead[] }
            fn page() -> Lead[] { db.leads.pageAfter(null, 10) }
        "#;
        let errors = check_source(src).unwrap_err();
        assert!(!errors.is_empty());
        let msg = errors[0].to_string();
        assert!(msg.contains("pageAfter") && msg.contains("Uuid"), "{msg}");
    }

    #[test]
    fn insert_on_a_uuid_pk_collection_still_omits_id_from_the_insertable_shape() {
        // `Omit<T,"id">` (omit_id_field) es por NOMBRE, no por tipo -- este
        // test confirma que sigue funcionando igual cuando ese campo
        // omitido es 'Uuid' en vez de 'Int'.
        let src = r#"
            type Lead = { id: Uuid, email: String }
            type NewLead = { email: String }
            db { leads: Lead[] }
            fn create(email: String) -> Lead { db.leads.insert(NewLead { email: email }) }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src).unwrap_err());
    }

    #[test]
    fn db_collection_that_is_not_a_list_of_structs_is_rejected() {
        let src = "db { posts: Int }";
        let result = check_source(src);
        assert!(result.is_err(), "una colección de db tiene que ser T[], no un tipo suelto");
    }

    /// GRAMMAR.md §3.172: desde esta ronda, un SEGUNDO `db {{ ... }}` ya no
    /// es un error por sí solo (ver el test de abajo que confirma que se
    /// fusionan) -- lo que sigue siendo un error duro es el NOMBRE DE
    /// COLECCIÓN repetido, sin importar si las dos apariciones caen en el
    /// mismo bloque o en dos distintos. Este test antes se llamaba
    /// `duplicate_db_declaration_is_rejected`; sigue fallando, pero por el
    /// motivo real ahora (no por tener dos `db {{ ... }}`, sino por
    /// repetir 'posts').
    #[test]
    fn duplicate_collection_name_across_two_db_blocks_is_rejected() {
        let src = r#"
            type Post = { id: Int }
            db { posts: Post[] }
            db { posts: Post[] }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "el mismo nombre de colección ('posts') repetido en dos 'db {{ ... }}' tiene que fallar");
    }

    /// Gap preexistente cerrado de paso en la misma ronda: un nombre de
    /// colección repetido DENTRO de un único bloque se perdía en silencio
    /// antes (el `insert` de turno pisaba la primera aparición sin ningún
    /// aviso, ver la única `db.<c>` que sobrevivía). Ahora es el MISMO
    /// error que el caso entre dos bloques.
    #[test]
    fn duplicate_collection_name_within_a_single_db_block_is_rejected() {
        let src = r#"
            type Post = { id: Int }
            type OldPost = { id: Int }
            db { posts: Post[], posts: OldPost[] }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "el mismo nombre de colección repetido en el MISMO bloque tiene que fallar, no perderse en silencio");
    }

    /// El caso nuevo que motivó todo esto: dos `db {{ ... }}` con nombres de
    /// colección DISTINTOS -- cada módulo dueño de las suyas -- se fusionan
    /// en un solo namespace, en vez del error duro de antes. Ambas
    /// colecciones quedan usables desde el mismo `rpc`, como si hubieran
    /// vivido siempre en un único bloque.
    #[test]
    fn two_db_blocks_with_disjoint_collection_names_merge_into_one_namespace() {
        let src = r#"
            type User = { id: Int, name: String }
            type Order = { id: Int, total: Int }
            db { users: User[] }
            db { orders: Order[] }
            service S {
                rpc bothCounts() -> Int {
                    db.users.count() + db.orders.count()
                }
            }
        "#;
        let result = check_source(src);
        assert!(result.is_ok(), "dos 'db {{ ... }}' con nombres de colección distintos tienen que fusionarse: {result:?}");
    }

    #[test]
    fn unknown_db_collection_name_is_rejected() {
        let src = r#"
            type Post = { id: Int }
            db { posts: Post[] }
            fn broken() -> Post? { db.comments.find(1) }
        "#;
        let result = check_source(src);
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("comments"), "debería señalar la colección desconocida: {msg}");
    }

    #[test]
    fn unknown_db_method_is_rejected() {
        let src = r#"
            type Post = { id: Int }
            db { posts: Post[] }
            fn broken() -> Post? { db.posts.fnid(1) }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "'fnid' no es un método real de una colección de db");
    }

    #[test]
    fn db_all_and_find_resolve_to_the_real_collection_type() {
        let src = r#"
            type Post = { id: Int, title: String }
            db { posts: Post[] }
            fn listAll() -> Post[] { db.posts.all() }
            fn one(id: Int) -> Post? { db.posts.find(id) }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn db_insert_accepts_the_element_type_without_id_but_rejects_the_id_field_missing_other_fields() {
        let src = r#"
            type Post = { id: Int, title: String }
            db { posts: Post[] }
            type NewPost = { title: String }
            fn create(input: NewPost) -> Post { db.posts.insert(input) }
        "#;
        assert!(check_source(src).is_ok(), "NewPost (sin id) debería servir para insert: Omit<Post,\"id\">");

        let src_missing_field = r#"
            type Post = { id: Int, title: String, body: String }
            db { posts: Post[] }
            type Incomplete = { title: String }
            fn create(input: Incomplete) -> Post { db.posts.insert(input) }
        "#;
        let result = check_source(src_missing_field);
        assert!(result.is_err(), "falta 'body' -- Incomplete no alcanza para Omit<Post,\"id\">");
    }

    #[test]
    fn db_apply_patch_requires_a_patch_of_the_element_type() {
        // Mismo patrón que examples/users.link: Patch<T> llega como
        // parámetro (del wire), no se construye con un literal acá.
        let src = r#"
            type Post = { id: Int, title: String }
            db { posts: Post[] }
            fn rename(id: Int, patch: Patch<Post>) -> Post { db.posts.applyPatch(id, patch) }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn user_variable_named_db_shadows_the_builtin() {
        // Hallado al diseñar "DB tipada": antes, "db" se chequeaba ANTES del
        // lookup de variables, así que un `let db = ...` de un usuario
        // quedaba sombreado en silencio por el builtin. Ahora el lookup de
        // variables va primero.
        let src = "fn f() -> Int { let db = 5; db }";
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn stream_rpc_body_is_checked_against_list_of_the_declared_element_type() {
        // La firma declara el ELEMENTO (`-> User`, igual que un rpc normal),
        // pero el cuerpo de un `stream` tiene que devolver la secuencia
        // COMPLETA ya calculada (GRAMMAR.md §2.1).
        let src = r#"
            type User = { id: Int, name: String }
            db { users: User[] }
            service Users {
                stream watchAll() -> User { db.users.all() }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn stream_rpc_body_returning_bare_element_instead_of_list_is_rejected() {
        // El error real que este chequeo existe para atrapar: devolver un
        // solo User (lo que un `rpc` normal pediría) donde `stream` pide
        // List<User> -- antes de esta ronda el checker chequeaba el cuerpo
        // de un `stream` contra `User` directamente (mismo camino que
        // `rpc`), así que esto SÍ tipaba antes y no debería tipar ahora.
        // Literal directo (no `db.users.find(id)`, que devuelve `User?` y
        // fallaría de todos modos por nullable-vs-no-nullable, sin probar
        // lo que este test quiere probar).
        let src = r#"
            type User = { id: Int, name: String }
            service Users {
                stream watchOne() -> User { User { id: 1, name: "Ada" } }
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "un stream que devuelve un User suelto (no List<User>) debería fallar");
    }

    #[test]
    fn set_status_inside_a_stream_body_is_a_compile_error() {
        // El status de una conexión SSE es fijo para toda su duración
        // (GRAMMAR.md §3.46): antes de esta ronda, `response.setStatus`
        // adentro de un `stream` tipaba sin quejarse y era un no-op
        // silencioso en runtime -- un desarrollador solo lo descubría en
        // producción. Ahora es un error de compilación.
        let src = r#"
            type Item = { id: Int }
            db { items: Item[] }
            service Items {
                stream watchAll() -> Item {
                    response.setStatus(201);
                    db.items.all()
                }
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(
            err.iter().any(|e| e.message.contains("setStatus") && e.message.contains("stream")),
            "mensaje inesperado: {err:?}"
        );
    }

    #[test]
    fn set_status_inside_a_normal_rpc_body_still_works() {
        // Mismo cuerpo, pero en un `rpc` normal en vez de `stream` -- tiene
        // que seguir tipando: el chequeo de arriba es específico de
        // `stream`, no una regresión sobre el caso de siempre.
        let src = r#"
            service Items {
                rpc create() -> Void {
                    response.setStatus(201)
                }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn redirect_inside_a_stream_body_is_a_compile_error() {
        // Mismo motivo que `set_status_inside_a_stream_body_is_a_compile_error`
        // (GRAMMAR.md §3.111): una conexión SSE ya envió su status HTTP antes
        // de que el cuerpo del stream corra, así que un redirect ahí no
        // podría tener ningún efecto -- error de compilación en vez de un
        // no-op silencioso.
        let src = r#"
            type Item = { id: Int }
            db { items: Item[] }
            service Items {
                stream watchAll() -> Item {
                    response.redirect("/otro-lado", false);
                    db.items.all()
                }
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(
            err.iter().any(|e| e.message.contains("redirect") && e.message.contains("stream")),
            "mensaje inesperado: {err:?}"
        );
    }

    #[test]
    fn redirect_inside_a_normal_rpc_body_works_and_returns_void() {
        let src = r#"
            service Web {
                rpc goElsewhere() -> Void {
                    response.redirect("/nueva-ubicacion", false)
                }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn redirect_rejects_the_wrong_number_or_types_of_arguments() {
        for bad in [
            r#"service W { rpc f() -> Void { response.redirect("/a") } }"#,
            r#"service W { rpc f() -> Void { response.redirect("/a", false, true) } }"#,
            r#"service W { rpc f() -> Void { response.redirect(1, false) } }"#,
            r#"service W { rpc f() -> Void { response.redirect("/a", "no") } }"#,
        ] {
            assert!(check_source(bad).is_err(), "debería rechazarse: {bad}");
        }
    }

    /// `@example(request: ..., response: ...)` (GRAMMAR.md §3.119) -- las
    /// dos expresiones se tipan contra la forma real del rpc, mismo
    /// mecanismo que `= default` de un campo/param.
    #[test]
    fn example_response_with_a_matching_literal_typechecks() {
        let src = r#"
            type Task = { id: Int, title: String }
            service Tasks {
                @example(response: Task { id: 1, title: "Comprar leche" })
                rpc get() -> Task { Task { id: 1, title: "x" } }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn example_response_rejects_a_type_mismatch() {
        let src = r#"
            type Task = { id: Int, title: String }
            service Tasks {
                @example(response: Task { id: 1, title: 123 })
                rpc get() -> Task { Task { id: 1, title: "x" } }
            }
        "#;
        assert!(check_source(src).is_err());
    }

    /// `request` se tipa contra un struct armado de los PARÁMETROS del rpc
    /// (mismo criterio que `req_props` en `openapi_emit`) -- un param con
    /// default es opcional en el ejemplo también.
    #[test]
    fn example_request_typechecks_against_the_rpcs_params_including_optional_ones_with_defaults() {
        let src = r#"
            type CreateInput = { title: String, priority: Int }
            service Tasks {
                @example(request: CreateInput { title: "Comprar leche", priority: 1 })
                rpc create(title: String, priority: Int = 0) -> Int { 1 }
            }
        "#;
        assert!(check_source(src).is_ok());
        // Omitiendo el param con default -- sigue tipando, es opcional.
        let src2 = r#"
            type CreateInput = { title: String }
            service Tasks {
                @example(request: CreateInput { title: "Comprar leche" })
                rpc create(title: String, priority: Int = 0) -> Int { 1 }
            }
        "#;
        assert!(check_source(src2).is_ok());
    }

    #[test]
    fn example_request_is_rejected_when_the_rpc_takes_no_params() {
        let src = r#"
            type Task = { id: Int }
            service Tasks {
                @example(request: Task { id: 1 })
                rpc get() -> Int { 1 }
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(err.iter().any(|e| e.message.contains("no toma parámetros")), "mensaje inesperado: {err:?}");
    }

    #[test]
    fn example_rejects_a_non_literal_expression_like_a_function_call() {
        let src = r#"
            service Tasks {
                @example(response: crypto.uuid())
                rpc get() -> String { "x" }
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(err.iter().any(|e| e.message.contains("valor literal")), "mensaje inesperado: {err:?}");
    }

    /// Mismo motivo que `cache_control`/`redirect` dentro de un `stream`: no
    /// hay una única respuesta que ejemplificar en una conexión SSE.
    #[test]
    fn example_is_rejected_on_a_stream() {
        let src = r#"
            type Task = { id: Int }
            db { tasks: Task[] }
            service Tasks {
                @example(response: Task { id: 1 })
                stream watchAll() -> Task {
                    db.tasks.all()
                }
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(
            err.iter().any(|e| e.message.contains("example") && e.message.contains("stream")),
            "mensaje inesperado: {err:?}"
        );
    }

    #[test]
    fn example_rejects_being_declared_twice() {
        let src = r#"
            service Tasks {
                @example(response: 1)
                @example(response: 2)
                rpc get() -> Int { 1 }
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(err.iter().any(|e| e.message.contains("más de una vez")), "mensaje inesperado: {err:?}");
    }

    /// `@invalidates(rpc1, rpc2, ...)` (GRAMMAR.md §3.125) -- cada nombre
    /// tiene que ser un rpc de Query real de la MISMA service.
    #[test]
    fn invalidates_accepts_query_shaped_rpcs_of_the_same_service() {
        let src = r#"
            type Task = { id: Int }
            service Tasks {
                rpc list() -> Task[] { [] }
                rpc search(term: String) -> Task[] { [] }
                @invalidates(list, search)
                rpc create(title: String) -> Task { Task { id: 1 } }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn invalidates_rejects_a_name_that_is_not_an_rpc_in_the_same_service() {
        let src = r#"
            service Tasks {
                @invalidates(noExiste)
                rpc create() -> Int { 1 }
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(err.iter().any(|e| e.message.contains("no es un rpc declarado")), "mensaje inesperado: {err:?}");
    }

    #[test]
    fn invalidates_rejects_a_name_from_a_different_service() {
        let src = r#"
            service Tasks { rpc list() -> Int { 1 } }
            service Other {
                @invalidates(list)
                rpc create() -> Int { 1 }
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(err.iter().any(|e| e.message.contains("no es un rpc declarado")), "mensaje inesperado: {err:?}");
    }

    #[test]
    fn invalidates_rejects_a_target_that_does_not_generate_a_query_hook() {
        let src = r#"
            service Tasks {
                rpc create(title: String) -> Int { 1 }
                @invalidates(create)
                rpc update(id: Int, title: String) -> Int { 1 }
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(err.iter().any(|e| e.message.contains("no genera un hook de Query")), "mensaje inesperado: {err:?}");
    }

    /// AUDIT-2026-08-27.md #7: un rpc `@cron` siempre tiene cero
    /// parámetros, así que `looks_like_a_query()` decía "sí" -- sin este
    /// chequeo explícito, `@invalidates` apuntando a un `@cron` compilaba
    /// `OK` aunque `emit_hooks` (el emisor real) nunca genera un hook de
    /// Query para un rpc `@cron`, dejando una llamada de invalidación
    /// muerta para siempre en `hooks.ts`.
    #[test]
    fn invalidates_rejects_a_cron_target() {
        let src = r#"
            service Jobs {
                @cron("5m")
                rpc sweep() -> Void { }
                @invalidates(sweep)
                rpc create(name: String) -> Int { 1 }
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(err.iter().any(|e| e.message.contains("no genera un hook de Query")), "mensaje inesperado: {err:?}");
    }

    #[test]
    fn invalidates_is_rejected_on_a_stream() {
        let src = r#"
            type Task = { id: Int }
            db { tasks: Task[] }
            service Tasks {
                rpc list() -> Task[] { [] }
                @invalidates(list)
                stream watch() -> Task {
                    db.tasks.all()
                }
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(
            err.iter().any(|e| e.message.contains("invalidates") && e.message.contains("stream")),
            "mensaje inesperado: {err:?}"
        );
    }

    #[test]
    fn invalidates_rejects_being_declared_twice() {
        let src = r#"
            service Tasks {
                rpc list() -> Int { 1 }
                @invalidates(list)
                @invalidates(list)
                rpc create() -> Int { 1 }
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(err.iter().any(|e| e.message.contains("más de una vez")), "mensaje inesperado: {err:?}");
    }

    /// `@infinite(cursor, limit)` (GRAMMAR.md §3.134) -- mismas firmas que
    /// `db.<c>.pageAfter(cursor: Int?, limit: Int)`, retorno `T[]` con `T`
    /// teniendo un campo `id: Int`.
    #[test]
    fn infinite_accepts_pageafter_shaped_signature() {
        let src = r#"
            type Task = { id: Int, title: String }
            db { tasks: Task[] }
            service Tasks {
                @infinite(cursor, limit)
                rpc list(cursor: Int?, limit: Int) -> Task[] { db.tasks.pageAfter(cursor, limit) }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn infinite_rejects_a_cursor_param_that_is_not_int_optional() {
        let src = r#"
            type Task = { id: Int }
            service Tasks {
                @infinite(cursor, limit)
                rpc list(cursor: String?, limit: Int) -> Task[] { [] }
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(err.iter().any(|e| e.message.contains("tiene que ser `Int?`")), "mensaje inesperado: {err:?}");
    }

    #[test]
    fn infinite_rejects_a_limit_param_that_is_not_int() {
        let src = r#"
            type Task = { id: Int }
            service Tasks {
                @infinite(cursor, limit)
                rpc list(cursor: Int?, limit: String) -> Task[] { [] }
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(err.iter().any(|e| e.message.contains("tiene que ser `Int`")), "mensaje inesperado: {err:?}");
    }

    #[test]
    fn infinite_rejects_a_return_type_without_an_id_int_field() {
        let src = r#"
            type Note = { text: String }
            service Notes {
                @infinite(cursor, limit)
                rpc list(cursor: Int?, limit: Int) -> Note[] { [] }
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(err.iter().any(|e| e.message.contains("id: Int")), "mensaje inesperado: {err:?}");
    }

    #[test]
    fn infinite_rejects_a_non_existent_param_name() {
        let src = r#"
            type Task = { id: Int }
            service Tasks {
                @infinite(noExiste, limit)
                rpc list(cursor: Int?, limit: Int) -> Task[] { [] }
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(err.iter().any(|e| e.message.contains("no es un parámetro")), "mensaje inesperado: {err:?}");
    }

    #[test]
    fn infinite_rejects_the_same_param_as_cursor_and_limit() {
        let src = r#"
            type Task = { id: Int }
            service Tasks {
                @infinite(cursor, cursor)
                rpc list(cursor: Int?) -> Task[] { [] }
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(err.iter().any(|e| e.message.contains("dos parámetros distintos")), "mensaje inesperado: {err:?}");
    }

    #[test]
    fn infinite_is_rejected_on_a_stream() {
        let src = r#"
            type Task = { id: Int }
            db { tasks: Task[] }
            service Tasks {
                @infinite(cursor, limit)
                stream list(cursor: Int?, limit: Int) -> Task {
                    db.tasks.all()
                }
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(
            err.iter().any(|e| e.message.contains("infinite") && e.message.contains("stream")),
            "mensaje inesperado: {err:?}"
        );
    }

    #[test]
    fn infinite_rejects_being_declared_twice() {
        let src = r#"
            type Task = { id: Int }
            service Tasks {
                @infinite(cursor, limit)
                @infinite(cursor, limit)
                rpc list(cursor: Int?, limit: Int) -> Task[] { [] }
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(err.iter().any(|e| e.message.contains("más de una vez")), "mensaje inesperado: {err:?}");
    }

    #[test]
    fn idempotent_annotation_type_checks_on_a_plain_rpc() {
        let src = r#"
            service Orders {
                @idempotent
                rpc create(total: Int) -> Int { total }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn idempotent_combines_with_other_annotations() {
        let src = r#"
            service Orders {
                @authenticated
                @idempotent
                @rate_limit("5/1m")
                rpc create(total: Int) -> Int { total }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn idempotent_is_rejected_on_a_stream() {
        let src = r#"
            type Task = { id: Int }
            db { tasks: Task[] }
            service Tasks {
                @idempotent
                stream list() -> Task {
                    db.tasks.all()
                }
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(
            err.iter().any(|e| e.message.contains("idempotent") && e.message.contains("stream")),
            "mensaje inesperado: {err:?}"
        );
    }

    #[test]
    fn cache_annotation_type_checks_on_a_plain_rpc() {
        let src = r#"
            service Stats {
                @cache("60s")
                rpc summary() -> Int { 1 }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn cache_annotation_rejects_a_malformed_ttl() {
        let src = r#"
            service Stats {
                @cache("60")
                rpc summary() -> Int { 1 }
            }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn cache_annotation_rejects_being_declared_twice() {
        let src = r#"
            service Stats {
                @cache("60s")
                @cache("5m")
                rpc summary() -> Int { 1 }
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(err.iter().any(|e| e.message.contains("más de una vez")), "mensaje inesperado: {err:?}");
    }

    #[test]
    fn cache_annotation_is_rejected_on_a_stream() {
        let src = r#"
            type Task = { id: Int }
            db { tasks: Task[] }
            service Tasks {
                @cache("60s")
                stream list() -> Task {
                    db.tasks.all()
                }
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(
            err.iter().any(|e| e.message.contains("cache") && e.message.contains("stream")),
            "mensaje inesperado: {err:?}"
        );
    }

    /// AUDIT-2026-08-27.md #2: la clave de caché es (service, rpc,
    /// argumentos), nunca la sesión del caller -- `@cache` sobre un rpc
    /// `@authenticated`/`@requires` sirve la respuesta de UN usuario a
    /// cualquier OTRO usuario autenticado que llegue con los mismos
    /// argumentos dentro del TTL (confirmado en vivo antes de este fix).
    /// Rechazado en compilación hasta que exista un diseño real de scoping
    /// por sesión.
    #[test]
    fn cache_annotation_is_rejected_when_combined_with_authenticated() {
        let src = r#"
            service Account {
                @authenticated
                @cache("30s")
                rpc myProfile() -> Int { 1 }
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(
            err.iter().any(|e| e.message.contains("@cache") && e.message.contains("@authenticated")),
            "mensaje inesperado: {err:?}"
        );
    }

    #[test]
    fn cache_annotation_is_rejected_when_combined_with_requires() {
        let src = r#"
            enum Role { Admin }
            service Account {
                @requires(Role.Admin)
                @cache("30s")
                rpc adminStats() -> Int { 1 }
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(
            err.iter().any(|e| e.message.contains("@cache") && e.message.contains("@authenticated")),
            "mensaje inesperado: {err:?}"
        );
    }

    #[test]
    fn cron_annotation_type_checks_alone_with_no_params_and_void_return() {
        let src = r#"
            service Jobs {
                @cron("5m")
                rpc sweep() -> Void { }
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn cron_annotation_rejects_a_malformed_interval() {
        let src = r#"
            service Jobs {
                @cron("5")
                rpc sweep() -> Void { }
            }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn cron_annotation_rejects_being_declared_twice() {
        let src = r#"
            service Jobs {
                @cron("5m")
                @cron("1h")
                rpc sweep() -> Void { }
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(err.iter().any(|e| e.message.contains("más de una vez")), "mensaje inesperado: {err:?}");
    }

    #[test]
    fn cron_annotation_is_rejected_on_a_stream() {
        let src = r#"
            type Task = { id: Int }
            db { tasks: Task[] }
            service Tasks {
                @cron("5m")
                stream list() -> Task {
                    db.tasks.all()
                }
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(
            err.iter().any(|e| e.message.contains("cron") && e.message.contains("stream")),
            "mensaje inesperado: {err:?}"
        );
    }

    #[test]
    fn cron_annotation_rejects_combining_with_another_annotation() {
        let src = r#"
            service Jobs {
                @cron("5m")
                @rate_limit("1/1m")
                rpc sweep() -> Void { }
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(err.iter().any(|e| e.message.contains("combina")), "mensaje inesperado: {err:?}");
    }

    #[test]
    fn cron_annotation_rejects_parameters() {
        let src = r#"
            service Jobs {
                @cron("5m")
                rpc sweep(n: Int) -> Void { }
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(err.iter().any(|e| e.message.contains("parámetros")), "mensaje inesperado: {err:?}");
    }

    #[test]
    fn cron_annotation_rejects_a_non_void_return_type() {
        let src = r#"
            service Jobs {
                @cron("5m")
                rpc sweep() -> Int { 1 }
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(err.iter().any(|e| e.message.contains("Void")), "mensaje inesperado: {err:?}");
    }

    #[test]
    fn auth_lockout_builtins_type_check() {
        let src = r#"
            service Sys {
                rpc onFail(email: String) -> Void { auth.recordFailedLogin(email) }
                rpc count(email: String) -> Int { auth.failedLoginCount(email, 900) }
                rpc onSuccess(email: String) -> Void { auth.resetFailedLogins(email) }
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn auth_failed_login_count_requires_string_and_int() {
        let src = r#"service Sys { rpc f(email: Int) -> Int { auth.failedLoginCount(email, 900) } }"#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn db_vacuum_and_table_stats_type_check() {
        let src = r#"
            type Item = { id: Int }
            db { items: Item[] }
            service Admin {
                rpc doVacuum() -> Void { db.vacuum() }
                rpc stats() -> Map<String, Int> { db.tableStats() }
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn db_vacuum_rejects_arguments() {
        let src = r#"
            db { items: { id: Int }[] }
            service Admin { rpc f() -> Void { db.vacuum(1) } }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn a_real_collection_named_vacuum_is_unaffected() {
        // `db.vacuum()` (llamado DIRECTO, cero argumentos) es el builtin --
        // pero una colección de VERDAD llamada "vacuum" sigue funcionando
        // normal con cualquier otro método real sobre ella (GRAMMAR.md
        // §3.151).
        let src = r#"
            type Item = { id: Int }
            db { vacuum: Item[] }
            service Admin { rpc f() -> Item[] { db.vacuum.all() } }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn cors_annotation_type_checks_and_combines_with_other_annotations() {
        let src = r#"
            service Sys {
                @authenticated
                @cors("https://a.com, https://b.com")
                rpc f() -> Int { 1 }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn cors_annotation_is_allowed_on_a_stream() {
        let src = r#"
            type Item = { id: Int }
            db { items: Item[] }
            service Sys {
                @cors("*")
                stream watch() -> Item { db.items.all() }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn cors_annotation_rejects_an_empty_value() {
        let src = r#"service Sys { @cors("") rpc f() -> Int { 1 } }"#;
        let msg = format!("{:?}", check_source(src).unwrap_err());
        assert!(msg.contains("no puede estar vacío"), "{msg}");
    }

    #[test]
    fn cors_annotation_rejects_being_declared_twice() {
        let src = r#"service Sys { @cors("*") @cors("https://a.com") rpc f() -> Int { 1 } }"#;
        let err = check_source(src).unwrap_err();
        assert!(err.iter().any(|e| e.message.contains("más de una vez")), "mensaje inesperado: {err:?}");
    }

    #[test]
    fn base64_encode_and_decode_take_a_string_and_return_a_string() {
        let src = r#"
            service Codec {
                rpc enc(s: String) -> String { base64.encode(s) }
                rpc dec(s: String) -> String { base64.decode(s) }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn base64_encode_rejects_a_non_string_argument() {
        let src = r#"
            service Codec {
                rpc bad() -> String { base64.encode(1) }
            }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn cache_control_annotation_type_checks_and_combines_with_route() {
        // Dimensión ortogonal (GRAMMAR.md §3.113) -- se combina libremente
        // con `@route`, mismo criterio que `@rate_limit`.
        let src = r#"
            service Blog {
                @route("/sitemap.xml")
                @content_type("application/xml")
                @cache_control("public, max-age=3600")
                rpc sitemap() -> String { "<urlset></urlset>" }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn cache_control_rejects_an_empty_value() {
        let src = r#"
            service S {
                @cache_control("")
                rpc f() -> String { "x" }
            }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn cache_control_rejects_being_declared_twice() {
        let src = r#"
            service S {
                @cache_control("public")
                @cache_control("private")
                rpc f() -> String { "x" }
            }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn cache_control_inside_a_stream_is_a_compile_error() {
        // Mismo motivo que `response.setStatus`/`response.redirect` dentro
        // de un `stream` (§3.46/§3.111): una conexión SSE nunca es
        // cacheable de forma sensata.
        let src = r#"
            type Item = { id: Int }
            db { items: Item[] }
            service Items {
                @cache_control("public, max-age=60")
                stream watchAll() -> Item { db.items.all() }
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(
            err.iter().any(|e| e.message.contains("cache_control") && e.message.contains("stream")),
            "mensaje inesperado: {err:?}"
        );
    }

    #[test]
    fn stream_rpc_body_returning_list_of_wrong_element_type_is_rejected() {
        let src = r#"
            type User = { id: Int, name: String }
            type Post = { id: Int, title: String }
            db { users: User[] }
            fn allPosts() -> Post[] { [] }
            service Users {
                stream watchAll() -> User { allPosts() }
            }
        "#;
        let result = check_source(src);
        assert!(
            result.is_err(),
            "un stream declarado -> User no debería aceptar un cuerpo List<Post>"
        );
    }

    // ---- closures + List.map/.filter (GRAMMAR.md §3.10) ----

    #[test]
    fn closure_param_annotated_narrower_than_actual_element_is_accepted() {
        // El closure solo pide lo que usa ({x: Int}) -- la lista real trae
        // más campos (WidePoint), y eso tiene que alcanzar por contravarianza
        // (types::params_accept).
        let src = r#"
            type WidePoint = { x: Int, y: Int, z: Int }
            fn run(points: WidePoint[]) -> WidePoint[] {
                points.filter(|p: { x: Int }| { p.x > 0 })
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn closure_param_annotated_wider_than_actual_element_is_rejected() {
        // Al revés: el closure anota MÁS campos de los que el elemento real
        // tiene -- si esto se aceptara, el cuerpo podría leer un campo que
        // el dato real nunca tuvo y crashear en runtime. Una implementación
        // con la dirección de subtipado invertida aceptaría esto por error
        // (hallado por un review de diseño antes de escribir el resto de
        // la feature, ver el comentario de `types::params_accept`).
        let src = r#"
            type NarrowPoint = { x: Int }
            type WidePoint = { x: Int, y: Int, z: Int }
            fn run(points: NarrowPoint[]) -> NarrowPoint[] {
                points.filter(|p: WidePoint| { p.x > 0 })
            }
        "#;
        let result = check_source(src);
        assert!(
            result.is_err(),
            "un closure que anota MÁS campos de los que el elemento real tiene debería rechazarse"
        );
    }

    #[test]
    fn equality_between_function_typed_values_is_rejected() {
        let src = r#"
            fn same(a: (Int) -> Int, b: (Int) -> Int) -> Bool { a == b }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "comparar dos valores de tipo función debería rechazarse");
    }

    #[test]
    fn closure_without_context_needs_every_param_annotated() {
        let src = r#"
            fn make() -> Int {
                let f = |x| { x + 1 };
                f(1)
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "un closure sin contexto (let sin tipo declarado) necesita anotar sus params");
    }

    #[test]
    fn return_inside_a_closure_without_known_return_type_is_rejected() {
        let src = r#"
            fn make() -> Int {
                let f = |x: Int| { return x + 1; };
                f(1)
            }
        "#;
        let result = check_source(src);
        assert!(
            result.is_err(),
            "un closure sintetizado (sin retorno conocido por contexto) no puede usar 'return'"
        );
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("return"), "el error debería mencionar 'return': {msg}");
    }

    #[test]
    fn closure_with_unannotated_params_infers_from_let_type_annotation() {
        let src = r#"
            fn make() -> Int {
                let f: (Int) -> Int = |x| { x + 1 };
                f(5)
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn filter_over_a_list_keeps_the_same_element_type() {
        let src = r#"
            type User = { id: Int, active: Bool }
            fn activeOnly(users: User[]) -> User[] {
                users.filter(|u: User| { u.active })
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn filter_rejects_a_predicate_that_does_not_return_bool() {
        let src = r#"
            fn run(xs: Int[]) -> Int[] {
                xs.filter(|x: Int| { x + 1 })
            }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn map_over_a_list_can_change_the_element_type() {
        let src = r#"
            type User = { id: Int, name: String }
            fn names(users: User[]) -> String[] {
                users.map(|u: User| { u.name })
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn map_accepts_a_named_fn_reference_as_callback() {
        let src = r#"
            fn double(x: Int) -> Int { x * 2 }
            fn run(xs: Int[]) -> Int[] {
                xs.map(double)
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn map_infers_unannotated_closure_param_from_the_list_element_type() {
        let src = r#"
            type User = { id: Int, name: String }
            fn names(users: User[]) -> String[] {
                users.map(|u| { u.name })
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn nested_closure_inside_a_closure_body_typechecks() {
        let src = r#"
            fn run(xs: Int[]) -> Int[][] {
                xs.map(|x: Int| { xs.filter(|y: Int| { y > x }) })
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    // ---- narrowing de uniones (GRAMMAR.md §3.9) ----

    #[test]
    fn a_const_is_usable_from_a_body() {
        // Hallado en la auditoría: un `const` se declaraba, se chequeaba y
        // se emitía a client.ts, pero usarlo daba "variable no declarada" --
        // una feature a medias, visible desde afuera pero no desde adentro.
        let src = r#"
            const MAX: Int = 20;
            enum Role { Admin, Member }
            const DEF: Role = Role.Member {};
            fn cap(n: Int) -> Int { if n > MAX { MAX } else { n } }
            service S {
                rpc limit() -> Int { MAX }
                rpc capped(n: Int) -> Int { cap(n) }
                rpc defaultRole() -> Role { DEF }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn a_local_binding_shadows_a_const_of_the_same_name() {
        let src = r#"
            const MAX: Int = 20;
            fn f() -> String { let MAX = "texto"; MAX }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn a_duplicate_const_is_rejected() {
        let src = r#"
            const MAX: Int = 1;
            const MAX: Int = 2;
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn a_function_type_cannot_cross_the_wire() {
        // §4 ya lo decía ("no cruza el wire") pero nada lo hacía cumplir:
        // el contrato emitía `h: (arg0: number) => string` y el validador
        // generado exigía `typeof x.h === "function"`, imposible de
        // satisfacer con JSON -- el cliente rechazaba SIEMPRE.
        let nested = r#"
            type T = { id: Int, h: (Int) -> String }
            service S { rpc get() -> T { T { id: 1, h: f } } }
            fn f(x: Int) -> String { "x" }
        "#;
        assert!(check_source(nested).is_err(), "un campo función dentro de un tipo de retorno debe rechazarse");

        let param = r#"
            service S { rpc go(f: (Int) -> Int) -> Int { f(1) } }
        "#;
        assert!(check_source(param).is_err(), "un parámetro de tipo función debe rechazarse");

        // Pero DENTRO del backend sigue siendo perfectamente válido.
        let internal = r#"
            fn add_one(x: Int) -> Int { x + 1 }
            fn apply(f: (Int) -> Int, x: Int) -> Int { f(x) }
            service S { rpc go(x: Int) -> Int { apply(add_one, x) } }
        "#;
        assert!(check_source(internal).is_ok());
    }

    #[test]
    fn void_is_only_valid_as_a_whole_rpc_return() {
        let ok = "service S { rpc ping() -> Void { } }";
        assert!(check_source(ok).is_ok());

        let as_field = r#"
            type T = { id: Int, v: Void }
            service S { rpc get() -> T? { null } }
        "#;
        assert!(check_source(as_field).is_err(), "Void como campo de struct debe rechazarse");

        let as_param = "service S { rpc go(v: Void) -> Int { 1 } }";
        assert!(check_source(as_param).is_err(), "Void como parámetro debe rechazarse");
    }

    #[test]
    fn a_match_does_not_need_a_trailing_comma_on_its_last_arm() {
        // Exigirla rechazaba `match x { A => 1, B => 2 }` con un críptico
        // "se esperaba Comma, se encontró RBrace".
        let src = r#"
            fn describe(n: Int) -> String { match n { 1 => "uno", _ => "otro" } }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn reading_an_optional_key_field_yields_an_optional_type() {
        // `note?: String` es opcionalidad de CLAVE (puede estar ausente,
        // GRAMMAR.md §3.4) -- leerlo da `String?`, no `String`. Antes de la
        // auditoría esto tipaba y después fallaba en runtime con "no existe
        // el campo 'note'" al recibir un objeto válido sin esa clave.
        let src = r#"
            type A = { id: Int, note?: String }
            fn get(a: A) -> String { a.note }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "leer un campo de clave opcional no puede dar el tipo pelado");

        // Declarado como `String?` sí tipa -- que es justo la forma de
        // escribirlo correctamente.
        let ok = r#"
            type A = { id: Int, note?: String }
            fn get(a: A) -> String? { a.note }
        "#;
        assert!(check_source(ok).is_ok());
    }

    #[test]
    fn arithmetic_on_an_optional_key_field_is_rejected() {
        let src = r#"
            type A = { n?: Int }
            fn get(a: A) -> Int { a.n + 1 }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn a_struct_missing_an_optional_field_is_still_a_subtype() {
        // Width subtyping (GRAMMAR.md §3.2/§3.4): si el supertipo declara el
        // campo OPCIONAL, un valor que no lo trae sigue siendo válido -- la
        // clave puede estar ausente, ese es todo el punto de `y?: T`.
        let src = r#"
            type Narrow = { x: Int }
            type Wide = { x: Int, y?: String }
            fn takesWide(w: Wide) -> Int { w.x }
            fn pass(n: Narrow) -> Int { takesWide(n) }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn union_match_with_type_patterns_for_every_member_typechecks() {
        let src = r#"
            fn describe(v: Int | String) -> String {
                match v {
                    i: Int => "es un entero",
                    s: String => "es un string",
                }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn non_exhaustive_union_match_is_rejected() {
        let src = r#"
            fn describe(v: Int | String) -> String {
                match v {
                    i: Int => "es un entero",
                }
            }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn wildcard_covers_the_rest_of_a_union_match() {
        let src = r#"
            fn describe(v: Int | String) -> String {
                match v {
                    i: Int => "es un entero",
                    _ => "otra cosa",
                }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    // ---- narrowing real de `T?` vía 'match' (GRAMMAR.md §3.9) ----

    #[test]
    fn match_narrows_an_optional_struct_to_read_its_field() {
        let src = r#"
            type Item = { id: Int, name: String }
            fn describe(x: Item?) -> String {
                match x {
                    v: Item => v.name,
                    null => "sin item",
                }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn match_over_optional_missing_the_null_arm_is_rejected() {
        let src = r#"
            type Item = { id: Int, name: String }
            fn describe(x: Item?) -> String {
                match x {
                    v: Item => v.name,
                }
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(err.iter().any(|e| e.message.contains("no exhaustivo")), "{err:?}");
    }

    #[test]
    fn match_over_optional_missing_the_value_arm_is_rejected() {
        let src = r#"
            type Item = { id: Int, name: String }
            fn describe(x: Item?) -> String {
                match x {
                    null => "sin item",
                }
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(err.iter().any(|e| e.message.contains("no exhaustivo")), "{err:?}");
    }

    #[test]
    fn wildcard_covers_both_arms_of_an_optional_match() {
        let src = r#"
            fn describe(x: Int?) -> String {
                match x {
                    _ => "cualquier cosa",
                }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn a_null_pattern_against_a_non_optional_scrutinee_is_rejected() {
        let src = r#"
            fn describe(x: Int) -> String {
                match x {
                    n: Int => "un entero",
                    null => "nunca alcanzable",
                }
            }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn a_type_pattern_that_does_not_match_the_optionals_inner_type_is_rejected() {
        let src = r#"
            fn describe(x: Int?) -> String {
                match x {
                    s: String => "nunca puede pasar",
                    null => "sin valor",
                }
            }
        "#;
        assert!(check_source(src).is_err());
    }

    // ---- `??` null-coalescing (GRAMMAR.md §3.9) ----

    #[test]
    fn coalesce_on_an_optional_typechecks_to_the_inner_type() {
        let src = r#"
            fn nameOrDefault(x: String?) -> String {
                x ?? "sin nombre"
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn coalesce_on_a_non_optional_left_side_is_rejected() {
        let src = r#"
            fn bad(x: String) -> String {
                x ?? "default"
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(err.iter().any(|e| e.message.contains("'??'")), "{err:?}");
    }

    #[test]
    fn coalesce_right_side_must_match_the_inner_type() {
        let src = r#"
            fn bad(x: String?) -> String {
                x ?? 5
            }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn coalesce_chains_left_to_right() {
        let src = r#"
            fn firstNonNull(a: String?, b: String?) -> String {
                a ?? b ?? "los dos ausentes"
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    // ---- `.isSome()`/`.isNone()` (GRAMMAR.md §3.9) ----

    #[test]
    fn is_some_and_is_none_typecheck_on_an_optional() {
        let src = r#"
            fn hasValue(x: Int?) -> Bool {
                x.isSome()
            }
            fn missingValue(x: Int?) -> Bool {
                x.isNone()
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn is_some_on_a_non_optional_is_rejected() {
        let src = r#"
            fn bad(x: Int) -> Bool {
                x.isSome()
            }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn is_some_rejects_arguments() {
        let src = r#"
            fn bad(x: Int?) -> Bool {
                x.isSome(1)
            }
        "#;
        assert!(check_source(src).is_err());
    }

    // ---- tipo nativo `Uuid` (GRAMMAR.md §3.70) ----

    #[test]
    fn uuid_resolves_as_a_type_name_and_typechecks_in_struct_fields_and_rpc_signatures() {
        let src = r#"
            type Session = { id: Int, token: Uuid }
            service S {
                rpc echo(u: Uuid) -> Uuid { u }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn crypto_uuid_returns_type_uuid_not_string() {
        let src = r#"
            fn f() -> Uuid { crypto.uuid() }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn aws_s3_presigned_url_takes_five_strings_and_an_int_and_returns_string() {
        let src = r#"
            fn f() -> String {
                crypto.awsS3PresignedUrl("AKID", "secret", "us-east-1", "bucket", "key.pdf", 3600)
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn aws_s3_presigned_url_rejects_the_wrong_number_of_arguments() {
        let src = r#"
            fn f() -> String { crypto.awsS3PresignedUrl("AKID", "secret") }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn aws_s3_presigned_url_rejects_expires_seconds_as_a_string() {
        let src = r#"
            fn f() -> String {
                crypto.awsS3PresignedUrl("AKID", "secret", "us-east-1", "bucket", "key.pdf", "3600")
            }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn aws_s3_presigned_upload_url_takes_six_strings_and_an_int_and_returns_string() {
        let src = r#"
            fn f() -> String {
                crypto.awsS3PresignedUploadUrl("AKID", "secret", "us-east-1", "bucket", "key.pdf", 3600, "application/pdf")
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn aws_s3_presigned_upload_url_rejects_the_wrong_number_of_arguments() {
        let src = r#"
            fn f() -> String { crypto.awsS3PresignedUploadUrl("AKID", "secret") }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn aws_s3_presigned_upload_url_rejects_content_type_as_an_int() {
        let src = r#"
            fn f() -> String {
                crypto.awsS3PresignedUploadUrl("AKID", "secret", "us-east-1", "bucket", "key.pdf", 3600, 1)
            }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn uuid_is_not_implicitly_compatible_with_string() {
        // Sin mezcla implicita, mismo criterio que Int64 vs Int -- un Uuid
        // no es un String hasta que se llama .toString() explicitamente.
        let src = r#"
            fn bad() -> String { crypto.uuid() }
        "#;
        assert!(check_source(src).is_err());
    }

    // ---- GRAMMAR.md §3.186: fast-path `builtin_args!` -- prueba de
    // equivalencia exacta contra el arm manual que reemplazó (mensaje de
    // error de aridad incluido, no solo el tipo devuelto en el caso feliz).

    #[test]
    fn crypto_hash_password_type_checks_with_the_right_arity() {
        let src = r#"
            fn f() -> String { crypto.hashPassword("secreto") }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn crypto_hash_password_rejects_the_wrong_arity_with_the_same_message_as_before_the_macro() {
        let src = r#"
            fn f() -> String { crypto.hashPassword("a", "b") }
        "#;
        let errors = check_source(src).unwrap_err();
        let msg = format!("{:?}", errors);
        assert!(msg.contains("'crypto.hashPassword' toma exactamente 1 argumento (password: String)"), "{msg}");
    }

    #[test]
    fn crypto_random_int_type_checks_with_the_right_arity() {
        let src = r#"
            fn f() -> Int { crypto.randomInt(1, 6) }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn crypto_random_int_rejects_the_wrong_arity_with_the_same_message_as_before_the_macro() {
        let src = r#"
            fn f() -> Int { crypto.randomInt(1) }
        "#;
        let errors = check_source(src).unwrap_err();
        let msg = format!("{:?}", errors);
        assert!(msg.contains("'crypto.randomInt' toma exactamente 2 argumentos (min: Int, max: Int)"), "{msg}");
    }

    #[test]
    fn uuid_concatenation_without_to_string_is_rejected() {
        let src = r#"
            fn bad(u: Uuid) -> String { "id: " + u }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn uuid_to_string_produces_a_real_string() {
        let src = r#"
            fn f(u: Uuid) -> String { u.toString() }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn matching_over_a_plain_int_still_uses_literal_patterns_not_optional_narrowing() {
        let src = r#"
            fn bad(x: Int) -> String {
                match x {
                    1 => "uno",
                    _ => "otro",
                }
            }
        "#;
        // Match sobre Int PRIMITIVO (no opcional) -- pasa por
        // check_exhaustive_literal, no check_exhaustive_optional. Confirma
        // que agregar el brazo Type::Optional al dispatch de check_match no
        // rompió el camino de Int/String/Bool que ya existía.
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn matching_over_an_ambiguous_union_is_rejected() {
        // {x,y} y {x,z} no comparten ningún campo con tipos en conflicto
        // ('x' es Int en los dos) -- un tercer tipo más ancho {x,y,z},
        // construible por cualquier usuario vía subtipado estructural,
        // satisface los requisitos de los DOS a la vez. Un chequeo ingenuo
        // de is_subtype mutuo los aceptaría (ninguno es subtipo del otro)
        // -- exactamente el caso que el análisis real tiene que atrapar.
        let src = r#"
            type A = { x: Int, y: Int }
            type B = { x: Int, z: Int }
            fn describe(v: A | B) -> String {
                match v {
                    a: A => "es A",
                    b: B => "es B",
                }
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "una unión con miembros ambiguos no debería poder matchearse");
    }

    #[test]
    fn matching_over_a_distinguishable_union_is_accepted() {
        // x:Int vs x:String -- un valor real solo puede tener UNA forma
        // concreta en 'x' a la vez, así que sí son distinguibles de forma
        // confiable (a diferencia del caso ambiguo de arriba).
        let src = r#"
            type A = { x: Int }
            type B = { x: String }
            fn describe(v: A | B) -> String {
                match v {
                    a: A => "es A",
                    b: B => "es B",
                }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn union_of_two_optional_members_is_always_ambiguous() {
        let src = r#"
            type A = { x: Int }
            type B = { y: Int }
            fn describe(v: A? | B?) -> String {
                match v {
                    a: A? => "es A",
                    b: B? => "es B",
                }
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "dos miembros Optional siempre son ambiguos: null matchea a los dos");
    }

    #[test]
    fn union_of_two_list_members_is_always_ambiguous() {
        let src = r#"
            fn describe(v: Int[] | String[]) -> String {
                match v {
                    xs: Int[] => "ints",
                    ss: String[] => "strings",
                }
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "dos miembros List siempre son ambiguos: una lista vacía matchea a las dos");
    }

    #[test]
    fn guarded_union_arm_does_not_discharge_exhaustiveness() {
        let src = r#"
            fn describe(v: Int | String) -> String {
                match v {
                    i: Int if i > 0 => "positivo",
                    s: String => "string",
                }
            }
        "#;
        let result = check_source(src);
        assert!(
            result.is_err(),
            "un arm con guard no debería descartar exhaustividad -- falta cubrir Int sin guard"
        );
    }

    #[test]
    fn or_pattern_combining_two_type_patterns_is_rejected() {
        // Un patrón de tipo LIGA un nombre, igual que cualquier otro binding
        // -- prohibido dentro de un Or (mismo criterio que ya rechaza
        // bindings de enum/literal ahí).
        let src = r#"
            fn describe(v: Int | String) -> String {
                match v {
                    i: Int | s: String => "algo",
                }
            }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn type_pattern_against_an_enum_scrutinee_is_rejected() {
        let src = r#"
            enum Status { Active, Paused }
            fn describe(s: Status) -> String {
                match s {
                    x: Int => "no",
                    Status.Active => "activo",
                    Status.Paused => "pausado",
                }
            }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn union_typed_parameter_without_matching_still_works_even_if_members_would_be_ambiguous() {
        // {x,y}|{x,z} sería ambiguo si se intentara matchear -- pero
        // ACEPTAR-Y-PASAR (sin narrowing) no necesita distinguir nada, y no
        // debería verse afectado: el chequeo de ambigüedad corre solo
        // dentro de check_exhaustive_union (match), no en la resolución
        // general de un tipo unión.
        let src = r#"
            type A = { x: Int, y: Int }
            type B = { x: Int, z: Int }
            fn accept(v: A | B) -> A | B { v }
        "#;
        assert!(check_source(src).is_ok());
    }

    // ---- auth v0 (GRAMMAR.md §3.14) ----

    #[test]
    fn authenticated_and_requires_annotations_type_check() {
        let src = r#"
            enum Role { Admin, Member }
            service S {
                @authenticated
                rpc me() -> Int { 1 }

                @requires(Role.Admin)
                rpc deleteThing(id: Int) -> Void { }

                rpc list() -> Int[] { [] }
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn requires_with_unknown_enum_is_rejected() {
        let src = r#"
            service S {
                @requires(NoExiste.Admin)
                rpc deleteThing(id: Int) -> Void { }
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("NoExiste"), "debería señalar el enum inexistente: {msg}");
    }

    #[test]
    fn requires_with_unknown_variant_is_rejected() {
        let src = r#"
            enum Role { Admin, Member }
            service S {
                @requires(Role.SuperAdmin)
                rpc deleteThing(id: Int) -> Void { }
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("SuperAdmin"), "debería señalar la variante inexistente: {msg}");
    }

    #[test]
    fn requires_does_not_need_the_whole_enum_to_be_all_unit() {
        // Hallazgo del review adversarial: la comparación en runtime es solo
        // por tag (enum_name+variant_name), nunca mira campos -- así que una
        // variante HERMANA con datos (ServiceAccount) no debería impedir
        // usar @requires sobre la variante unitaria (Admin).
        let src = r#"
            enum Role { Admin, Member, ServiceAccount { scopes: String[] } }
            service S {
                @requires(Role.Admin)
                rpc deleteThing(id: Int) -> Void { }
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    // ---- @requires(..., ownerOf: ..., id: ..., field: ...) -- GRAMMAR.md §3.190 ----

    #[test]
    fn requires_ownership_clause_with_a_valid_collection_param_and_field_type_checks() {
        let src = r#"
            enum Role { Agent }
            type Invoice = { id: Int, ownerId: Int, amount: Int }
            db { invoices: Invoice[] }
            service S {
                @requires(Role.Agent, ownerOf: invoices, id: id, field: ownerId)
                rpc getInvoice(id: Int) -> Invoice? { db.invoices.find(id) }
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn requires_ownership_clause_naming_an_undeclared_collection_is_rejected() {
        let src = r#"
            enum Role { Agent }
            type Invoice = { id: Int, ownerId: Int }
            db { invoices: Invoice[] }
            service S {
                @requires(Role.Agent, ownerOf: noExiste, id: id, field: ownerId)
                rpc getInvoice(id: Int) -> Invoice? { db.invoices.find(id) }
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("noExiste"), "debería señalar la colección inexistente: {msg}");
    }

    #[test]
    fn requires_ownership_clause_naming_a_param_that_does_not_exist_is_rejected() {
        let src = r#"
            enum Role { Agent }
            type Invoice = { id: Int, ownerId: Int }
            db { invoices: Invoice[] }
            service S {
                @requires(Role.Agent, ownerOf: invoices, id: noExiste, field: ownerId)
                rpc getInvoice(id: Int) -> Invoice? { db.invoices.find(id) }
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("noExiste"), "debería señalar el parámetro inexistente: {msg}");
    }

    #[test]
    fn requires_ownership_clause_where_the_id_param_type_does_not_match_the_collections_pk_is_rejected() {
        let src = r#"
            enum Role { Agent }
            type Invoice = { id: Int, ownerId: Int }
            db { invoices: Invoice[] }
            service S {
                @requires(Role.Agent, ownerOf: invoices, id: id, field: ownerId)
                rpc getInvoice(id: String) -> Void { }
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("id") && msg.contains("Int"), "debería señalar el tipo esperado (la PK, Int): {msg}");
    }

    #[test]
    fn requires_ownership_clause_naming_a_field_that_does_not_exist_is_rejected() {
        let src = r#"
            enum Role { Agent }
            type Invoice = { id: Int, ownerId: Int }
            db { invoices: Invoice[] }
            service S {
                @requires(Role.Agent, ownerOf: invoices, id: id, field: noExiste)
                rpc getInvoice(id: Int) -> Invoice? { db.invoices.find(id) }
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("noExiste"), "debería señalar el campo inexistente: {msg}");
    }

    #[test]
    fn requires_ownership_clause_field_that_is_not_int_is_rejected() {
        let src = r#"
            enum Role { Agent }
            type Invoice = { id: Int, ownerId: String }
            db { invoices: Invoice[] }
            service S {
                @requires(Role.Agent, ownerOf: invoices, id: id, field: ownerId)
                rpc getInvoice(id: Int) -> Invoice? { db.invoices.find(id) }
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("ownerId") && msg.contains("Int"), "debería señalar que el campo tiene que ser Int: {msg}");
    }

    #[test]
    fn requires_ownership_clause_field_that_is_nullable_int_is_rejected() {
        // `Int?` no calza con `auth.currentUserId(): Int?` de forma directa
        // sin ambigüedad -- se rechaza en compile-time en vez de dejar un
        // "siempre 403" silencioso en runtime cuando el campo da null.
        let src = r#"
            enum Role { Agent }
            type Invoice = { id: Int, ownerId: Int? }
            db { invoices: Invoice[] }
            service S {
                @requires(Role.Agent, ownerOf: invoices, id: id, field: ownerId)
                rpc getInvoice(id: Int) -> Invoice? { db.invoices.find(id) }
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("ownerId"), "debería señalar el campo: {msg}");
    }

    #[test]
    fn requires_ownership_clause_works_with_a_uuid_pk_collection() {
        let src = r#"
            enum Role { Agent }
            type Invoice = { id: Uuid, ownerId: Int }
            db { invoices: Invoice[] }
            service S {
                @requires(Role.Agent, ownerOf: invoices, id: id, field: ownerId)
                rpc getInvoice(id: Uuid) -> Invoice? { db.invoices.find(id) }
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn requires_ownership_clause_on_a_stream_is_rejected_since_server_rs_never_enforces_it_there() {
        // `server.rs` saltea deliberadamente esta etapa para `stream`
        // (límite honesto de GRAMMAR.md §3.190) -- aceptar la cláusula en
        // el checker sería dejarla silenciosamente sin efecto en runtime.
        let src = r#"
            enum Role { Agent }
            type Invoice = { id: Int, ownerId: Int }
            db { invoices: Invoice[] }
            service S {
                @requires(Role.Agent, ownerOf: invoices, id: id, field: ownerId)
                stream watchInvoice(id: Int) -> Invoice[] { [] }
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("stream"), "debería señalar que la cláusula no aplica a stream: {msg}");
    }

    #[test]
    fn requires_without_an_ownership_clause_still_type_checks_exactly_as_before() {
        let src = r#"
            enum Role { Admin }
            service S {
                @requires(Role.Admin)
                rpc deleteThing(id: Int) -> Void { }
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn create_session_requires_an_enum_typed_argument() {
        let src = r#"
            enum Role { Admin, Member }
            service S {
                rpc login() -> String { auth.createSession(Role.Admin {}) }
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));

        let bad = r#"
            service S {
                rpc login() -> String { auth.createSession(1) }
            }
        "#;
        let result = check_source(bad);
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("createSession"), "debería mencionar 'createSession': {msg}");
    }

    #[test]
    fn destroy_session_takes_zero_arguments() {
        let src = r#"
            service S {
                @authenticated
                rpc logout() -> Void { auth.destroySession() }
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));

        // Tomar un token como argumento dejaría destruir la sesión de
        // cualquier otro con solo nombrarla -- ver check_auth_method.
        let bad = r#"
            service S {
                @authenticated
                rpc logout(token: String) -> Void { auth.destroySession(token) }
            }
        "#;
        assert!(check_source(bad).is_err());
    }

    #[test]
    fn current_role_types_as_optional_string_and_takes_no_arguments() {
        // GRAMMAR.md §3.51: disponible SIEMPRE, sin requerir ninguna
        // anotación de auth en el rpc que lo llama -- mismo criterio que
        // request.rawBody()/request.header() (§3.38).
        let src = r#"
            service S {
                rpc whoAmI() -> String? { auth.currentRole() }
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));

        let bad = r#"
            service S {
                rpc whoAmI() -> String? { auth.currentRole("Admin") }
            }
        "#;
        let result = check_source(bad);
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("currentRole"), "debería mencionar 'currentRole': {msg}");
    }

    #[test]
    fn create_session_with_id_requires_an_enum_and_an_int() {
        let src = r#"
            enum Role { Admin, Member }
            service S {
                rpc login() -> String { auth.createSessionWithId(Role.Member {}, 42) }
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));

        let bad_role = r#"
            service S {
                rpc login() -> String { auth.createSessionWithId(1, 42) }
            }
        "#;
        let res_role = check_source(bad_role);
        assert!(res_role.is_err());
        let msg_role = format!("{:?}", res_role.unwrap_err());
        assert!(msg_role.contains("createSessionWithId"), "debería mencionar 'createSessionWithId': {msg_role}");

        let bad_id = r#"
            enum Role { Admin, Member }
            service S {
                rpc login() -> String { auth.createSessionWithId(Role.Admin {}, "42") }
            }
        "#;
        let res_id = check_source(bad_id);
        assert!(res_id.is_err());
        let msg_id = format!("{:?}", res_id.unwrap_err());
        assert!(msg_id.contains("Int"), "debería mencionar 'Int': {msg_id}");
    }

    /// `auth.destroyAllSessions(userId: Int) -> Int` (GRAMMAR.md §3.84).
    #[test]
    fn destroy_all_sessions_requires_exactly_one_int_argument_and_types_as_int() {
        let src = r#"
            service S {
                rpc revoke(userId: Int) -> Int { auth.destroyAllSessions(userId) }
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));

        let no_args = r#"
            service S {
                rpc revoke() -> Int { auth.destroyAllSessions() }
            }
        "#;
        let res_no_args = check_source(no_args);
        assert!(res_no_args.is_err());
        assert!(
            format!("{:?}", res_no_args.unwrap_err()).contains("destroyAllSessions"),
            "debería mencionar 'destroyAllSessions'"
        );

        let bad_type = r#"
            service S {
                rpc revoke() -> Int { auth.destroyAllSessions("42") }
            }
        "#;
        let res_bad_type = check_source(bad_type);
        assert!(res_bad_type.is_err());
        assert!(format!("{:?}", res_bad_type.unwrap_err()).contains("Int"), "debería mencionar 'Int'");
    }

    #[test]
    fn current_user_id_types_as_optional_int_and_takes_no_arguments() {
        let src = r#"
            service S {
                rpc whoAmI() -> Int? { auth.currentUserId() }
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));

        let bad = r#"
            service S {
                rpc whoAmI() -> Int? { auth.currentUserId(1) }
            }
        "#;
        let result = check_source(bad);
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("currentUserId"), "debería mencionar 'currentUserId': {msg}");
    }

    /// GRAMMAR.md §3.197 -- `auth.claim(name)` toma exactamente 1 argumento
    /// String y devuelve `String?`, a diferencia de `currentRole`/
    /// `currentUserId` (sin argumentos, slots fijos).
    #[test]
    fn auth_claim_types_as_optional_string_and_takes_exactly_one_string_argument() {
        let src = r#"
            service S {
                rpc tokenVersion() -> String? { auth.claim("tokenVersion") }
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));

        let no_args = r#"
            service S {
                rpc bad() -> String? { auth.claim() }
            }
        "#;
        let result = check_source(no_args);
        assert!(result.is_err());
        assert!(format!("{:?}", result.unwrap_err()).contains("claim"), "debería mencionar 'claim'");

        let wrong_type = r#"
            service S {
                rpc bad() -> String? { auth.claim(42) }
            }
        "#;
        assert!(check_source(wrong_type).is_err());
    }

    #[test]
    fn const_calling_create_session_is_rejected_not_just_at_build_time() {
        // Un const no es un literal si invoca auth.createSession -- si esto
        // tipara, cada referencia al const crearía una sesión Admin nueva
        // (los const no se memoizan en runtime), sin que nadie la pidiera.
        let src = r#"
            enum Role { Admin, Member }
            const TOKEN: String = auth.createSession(Role.Admin {});
        "#;
        let result = check_source(src);
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("literal"), "debería señalar la restricción de forma-literal: {msg}");
    }

    #[test]
    fn const_with_plain_literal_value_still_works() {
        let src = r#"
            enum Role { Admin, Member }
            const DEFAULT_ROLE: Role = Role.Member {};
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn list_length_returns_int() {
        // Faltaba (solo String.length() existía) -- encontrado al escribir
        // `login` para auth v0, que necesita "¿matcheó algún usuario?".
        let src = r#"
            fn count(xs: Int[]) -> Int { xs.length() }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn list_sum_on_int_list_returns_int() {
        let src = r#"
            fn total(xs: Int[]) -> Int { xs.sum() }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn list_sum_on_int64_list_is_rejected_with_a_clear_message() {
        let src = r#"
            fn total(xs: Int64[]) -> Int64 { xs.sum() }
        "#;
        let errors = check_source(src).unwrap_err();
        let msg = errors[0].to_string();
        assert!(msg.contains("List<Int64>"), "{msg}");
        assert!(msg.contains("deliberadamente afuera"), "{msg}");
    }

    #[test]
    fn list_sum_on_float_list_is_rejected_with_a_clear_message() {
        let src = r#"
            fn total(xs: Float[]) -> Float { xs.sum() }
        "#;
        let errors = check_source(src).unwrap_err();
        let msg = errors[0].to_string();
        assert!(msg.contains("List<Float>"), "{msg}");
    }

    #[test]
    fn list_sum_takes_no_arguments() {
        let src = r#"
            fn total(xs: Int[]) -> Int { xs.sum(1) }
        "#;
        let errors = check_source(src).unwrap_err();
        assert!(errors[0].to_string().contains("no toma argumentos"), "{:?}", errors[0]);
    }

    // ---- PLAN.md §9.14 ítem 2: List<T> + List<T> y .contains() ----

    #[test]
    fn list_plus_list_of_the_same_element_type_concatenates() {
        let src = r#"
            fn merge(a: Int[], b: Int[]) -> Int[] { a + b }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn list_plus_list_of_a_different_element_type_is_rejected_with_a_clear_message() {
        let src = r#"
            fn merge(a: Int[], b: String[]) -> Int[] { a + b }
        "#;
        let errors = check_source(src).unwrap_err();
        let msg = errors[0].to_string();
        assert!(msg.contains("List<T>+List<T>"), "{msg}");
    }

    #[test]
    fn list_plus_scalar_is_still_rejected_same_as_before_this_round() {
        let src = r#"
            fn f(a: Int[], b: Int) -> Int[] { a + b }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn list_contains_on_an_int_list_returns_bool() {
        let src = r#"
            fn has(xs: Int[], target: Int) -> Bool { xs.contains(target) }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn list_contains_takes_exactly_one_argument_of_the_element_type() {
        let src = r#"
            fn f(xs: Int[]) -> Bool { xs.contains("no es Int") }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn list_contains_on_a_list_of_struct_is_rejected_not_just_undocumented() {
        // PLAN.md §9.14 ítem 2 -- Struct/Variant quedan explícitamente
        // afuera de esta ronda (PartialEq sensible al orden textual de un
        // literal fuente, GRAMMAR.md §3.200), así que `.contains()` sobre
        // `List<Struct>` no debe tipar en absoluto, no solo quedar "sin
        // ejemplo" en la documentación.
        let src = r#"
            type Item = { id: Int }
            fn has(xs: Item[], target: Item) -> Bool { xs.contains(target) }
        "#;
        assert!(check_source(src).is_err(), "'.contains()' sobre List<Struct> no debería tipar");
    }

    #[test]
    fn list_contains_on_a_list_of_function_is_rejected() {
        let src = r#"
            fn has(xs: ((Int) -> Bool)[], target: (Int) -> Bool) -> Bool { xs.contains(target) }
        "#;
        assert!(check_source(src).is_err());
    }

    // ---- pdf.build (GRAMMAR.md §3.201) ----

    #[test]
    fn pdf_build_takes_a_pdf_block_list_and_returns_string() {
        let src = r#"
            fn make() -> String {
                pdf.build([PdfBlock.Text { content: "hola", bold: false, size: 12 }])
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn pdf_build_rejects_zero_arguments() {
        let src = r#"
            fn f() -> String { pdf.build() }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn pdf_build_rejects_an_argument_of_the_wrong_type() {
        let src = r#"
            fn f() -> String { pdf.build(["no es PdfBlock"]) }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn pdf_block_table_variant_types_like_any_adt() {
        let src = r#"
            fn make() -> String {
                pdf.build([PdfBlock.Table { headers: ["a", "b"], rows: [["1", "2"]] }])
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn user_cannot_declare_their_own_pdf_block_enum() {
        // `PdfBlock` es un ADT reservado por el compilador (pre-registrado
        // en `checker.enums` por `build_symbols`) -- un usuario que declare
        // su propio `enum PdfBlock` cae en la misma rama de "enum
        // duplicado" que colisionar con cualquier otro enum.
        let src = r#"
            enum PdfBlock { Foo }
            fn f() -> Int { 1 }
        "#;
        let errors = check_source(src).unwrap_err();
        assert!(errors[0].to_string().contains("ya está declarado"), "{:?}", errors);
    }

    // ---- excel.build / excel.parse (GRAMMAR.md §3.202) ----

    #[test]
    fn excel_build_takes_an_excel_sheet_list_and_returns_string() {
        let src = r#"
            fn make() -> String {
                excel.build([ExcelSheet {
                    name: "Hoja1",
                    headers: ["Concepto", "Importe"],
                    rows: [[ExcelCell.Text { value: "Servicio" }, ExcelCell.Number { value: 100.00.toDecimal() }]],
                }])
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn excel_parse_takes_a_string_and_returns_an_excel_sheet_list() {
        let src = r#"
            fn f(b64: String) -> ExcelSheet[] { excel.parse(b64) }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn excel_build_rejects_zero_arguments() {
        let src = r#"
            fn f() -> String { excel.build() }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn excel_parse_rejects_a_non_string_argument() {
        let src = r#"
            fn f() -> ExcelSheet[] { excel.parse(123) }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn excel_cell_variants_type_like_any_adt() {
        let src = r#"
            fn make() -> ExcelCell[] {
                [
                    ExcelCell.Text { value: "x" },
                    ExcelCell.Number { value: 1.5.toDecimal() },
                    ExcelCell.Date { value: now() },
                    ExcelCell.Bool { value: true },
                    ExcelCell.Empty {},
                ]
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn user_cannot_declare_their_own_excel_cell_enum() {
        let src = r#"
            enum ExcelCell { Foo }
            fn f() -> Int { 1 }
        "#;
        let errors = check_source(src).unwrap_err();
        assert!(errors[0].to_string().contains("ya está declarado"), "{:?}", errors);
    }

    #[test]
    fn excel_sheet_subtypes_structurally_unlike_the_nominal_pdf_block() {
        // A diferencia de `PdfBlock` (un enum, NOMINAL en este lenguaje),
        // `ExcelSheet` es un struct -- subtipa ESTRUCTURALMENTE (§3.2). Un
        // `type` de usuario con OTRO nombre pero la MISMA forma tiene que
        // tipar igual de bien contra `excel.build`, sin necesitar llamarse
        // "ExcelSheet". Este test es el que prueba que el diseño es
        // correcto, no solo plausible.
        let src = r#"
            type MiHoja = { name: String, headers: String[], rows: ExcelCell[][] }
            fn f(hojas: MiHoja[]) -> String { excel.build(hojas) }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn excel_sheet_with_a_mismatched_shape_is_rejected() {
        let src = r#"
            type NoEsUnaHoja = { titulo: String }
            fn f(x: NoEsUnaHoja[]) -> String { excel.build(x) }
        "#;
        assert!(check_source(src).is_err());
    }

    // ---- mcp.sample (GRAMMAR.md §3.203, Pieza C) ----

    #[test]
    fn mcp_sample_takes_a_string_and_returns_a_string() {
        let src = r#"
            fn f(prompt: String) -> String { mcp.sample(prompt) }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn mcp_sample_rejects_zero_arguments() {
        let src = r#"
            fn f() -> String { mcp.sample() }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn mcp_sample_rejects_a_non_string_argument() {
        let src = r#"
            fn f() -> String { mcp.sample(123) }
        "#;
        assert!(check_source(src).is_err());
    }

    // ---- spans en errores de TIPOS (LSP prerrequisito 3/3, Ronda B) ----

    #[test]
    fn binary_type_mismatch_span_covers_the_whole_binary_expression() {
        // Sin frontera propia dentro de synth_binary -- el error sube sin
        // span hasta el wrapper de synth_expr, que lo estampa con el span
        // de TODA la expresión binaria (no solo el operando problemático).
        let src = r#"fn f() -> Int { 1 + "texto" }"#;
        let errors = check_source(src).unwrap_err();
        assert_eq!(errors.len(), 1, "errores: {errors:?}");
        let span = errors[0].span.expect("se esperaba un span");
        let start = src.find('1').unwrap();
        let end = src.find("\"texto\"").unwrap() + "\"texto\"".len();
        assert_eq!(span.start, start, "el span debería empezar en el '1'");
        assert_eq!(span.end, end, "el span debería terminar al cierre de la comilla de 'texto'");
    }

    #[test]
    fn missing_field_error_span_covers_the_struct_literal() {
        let src = "type Point = { x: Int, y: Int }\nfn origin() -> Point { Point { x: 0 } }";
        let errors = check_source(src).unwrap_err();
        assert_eq!(errors.len(), 1, "errores: {errors:?}");
        let span = errors[0].span.expect("se esperaba un span");
        let start = src.find("Point { x: 0 }").unwrap();
        let end = start + "Point { x: 0 }".len();
        assert_eq!(span.start, start, "el span debería empezar en el literal de struct");
        assert_eq!(span.end, end, "el span debería terminar en la llave de cierre del literal");
    }

    #[test]
    fn rpc_crosses_the_wire_error_span_covers_the_signature_not_the_body() {
        // check_rpc_crosses_the_wire no estampa nada por su cuenta -- lo
        // hace check_program, con el span (de firma, sin el cuerpo) del
        // propio RpcDecl.
        let src = "type Weird = { h: (Int) -> String }\nservice S { rpc bad() -> Weird { 1 } }";
        let errors = check_source(src).unwrap_err();
        let wire_error = errors
            .iter()
            .find(|e| e.message.contains("no puede viajar por la red"))
            .unwrap_or_else(|| panic!("se esperaba el error de check_rpc_crosses_the_wire: {errors:?}"));
        let span = wire_error.span.expect("se esperaba un span");
        let start = src.find("rpc bad").unwrap();
        let end = src.find("-> Weird").unwrap() + "-> Weird".len();
        assert_eq!(span.start, start, "el span debería empezar en 'rpc'");
        assert_eq!(span.end, end, "el span debería terminar en el return type, sin incluir el cuerpo");
    }

    // ---- constructo de loop: `while` (GRAMMAR.md §3.15) ----

    #[test]
    fn while_condition_must_be_bool() {
        let result = check_source("fn f(x: Int) -> Void { while x { } }");
        assert!(result.is_err());
    }

    #[test]
    fn while_with_bool_condition_typechecks() {
        let src = r#"
            fn count_down(n: Int) -> Int {
                let mut i = n;
                while i > 0 {
                    i = i - 1;
                }
                i
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn return_inside_a_while_body_is_rejected() {
        let result = check_source("fn f() -> Int { while true { return 1; } 0 }");
        assert!(result.is_err(), "un 'return' dentro de un 'while' debería rechazarse en v0");
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("'return'"), "el error debería mencionar 'return': {msg}");
    }

    #[test]
    fn return_nested_inside_an_if_inside_a_while_body_is_also_rejected() {
        // block_has_return recursa a través de if/match, así que un return
        // escondido más profundo también se rechaza, no solo el directo.
        let result = check_source(
            "fn f(x: Int) -> Int { while true { if x > 0 { return 1; } else { } } 0 }",
        );
        assert!(result.is_err());
    }

    #[test]
    fn let_mut_declared_before_a_while_is_visible_and_assignable_inside() {
        // Esto es lo que hace útil al loop: check_block/check_stmt no
        // necesitaron ningún código nuevo para esto, es el mismo mecanismo
        // que ya usa `if` (env clonado, Assign valida `mut`).
        let src = "fn f() -> Int { let mut total = 0; while total < 3 { total = total + 1; } total }";
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn assigning_to_a_non_mut_variable_inside_a_while_body_is_rejected() {
        let result = check_source("fn f() -> Int { let total = 0; while true { total = 1; } total }");
        assert!(result.is_err());
    }

    // ---- transacciones multi-escritura: `transaction { ... }` (GRAMMAR.md §3.154) ----

    #[test]
    fn transaction_with_db_writes_typechecks_against_the_rpc_return_type() {
        let src = r#"
            type Item = { id: Int, name: String }
            db { items: Item[] }
            service S {
                rpc create(name: String) -> Item {
                    transaction {
                        db.items.insert(Item { id: 0, name: name })
                    }
                }
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn transaction_as_a_non_tail_statement_is_checked_against_void() {
        let src = r#"
            type Item = { id: Int, name: String }
            db { items: Item[] }
            service S {
                rpc create(name: String) -> Int {
                    transaction {
                        db.items.insert(Item { id: 0, name: name });
                    }
                    db.items.count()
                }
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn nesting_a_transaction_inside_another_is_rejected() {
        let src = r#"
            type Item = { id: Int, name: String }
            db { items: Item[] }
            service S {
                rpc bad() -> Item {
                    transaction {
                        transaction {
                            db.items.insert(Item { id: 0, name: "x" })
                        }
                    }
                }
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "una 'transaction' anidada dentro de otra debería rechazarse");
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("anidar"), "{msg}");
    }

    #[test]
    fn return_inside_a_transaction_body_is_rejected() {
        let result = check_source("fn f() -> Int { transaction { return 1; } }");
        assert!(result.is_err(), "un 'return' dentro de una 'transaction' debería rechazarse en v0");
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("'return'"), "{msg}");
    }

    #[test]
    fn return_nested_inside_an_if_inside_a_transaction_body_is_also_rejected() {
        let result = check_source("fn f(x: Int) -> Int { transaction { if x > 0 { return 1; } else { } 0 } }");
        assert!(result.is_err());
    }

    #[test]
    fn transaction_in_synthesis_position_is_rejected() {
        // Mismo motivo que if/match: sin un `expected` del contexto, no se
        // puede sintetizar -- acá un `let` SIN anotación de tipo fuerza
        // síntesis (`Stmt::Let { ty: None, .. }` -> `synth_expr`).
        let src = r#"
            type Item = { id: Int, name: String }
            db { items: Item[] }
            service S {
                rpc bad() -> Item {
                    let x = transaction { db.items.insert(Item { id: 0, name: "x" }) };
                    x
                }
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err());
    }

    // ---- push real para `stream`: shape reconocido (GRAMMAR.md §3.16) ----

    #[test]
    fn the_recognized_live_subscribe_shape_typechecks() {
        let src = r#"
            type Item = { id: Int, name: String }
            db { items: Item[] }
            service S {
                stream watchItems() -> Item {
                    while true {
                        db.items.subscribe()
                    }
                }
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn live_subscribe_return_type_must_match_the_collection_element_type() {
        // `Other` pide un campo ("extra") que `Item` no tiene -- por más
        // que el subtipado estructural de ancho acepte CAMPOS DE MÁS
        // (GRAMMAR.md §3.2), acá falta uno requerido, así que Item NO es
        // subtipo de Other.
        let src = r#"
            type Item = { id: Int, name: String }
            type Other = { id: Int, extra: String }
            db { items: Item[] }
            service S {
                stream watchItems() -> Other {
                    while true {
                        db.items.subscribe()
                    }
                }
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("incompatible"), "el error debería explicar el desajuste de tipos: {msg}");
    }

    #[test]
    fn live_subscribe_rejects_an_unknown_collection() {
        let src = r#"
            type Item = { id: Int, name: String }
            db { items: Item[] }
            service S {
                stream watchItems() -> Item {
                    while true {
                        db.noExiste.subscribe()
                    }
                }
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("no es una colección declarada"), "{msg}");
    }

    #[test]
    fn live_subscribe_stream_cannot_take_parameters_in_v0() {
        let src = r#"
            type Item = { id: Int, name: String }
            db { items: Item[] }
            service S {
                stream watchItems(id: Int) -> Item {
                    while true {
                        db.items.subscribe()
                    }
                }
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("no toma parámetros"), "{msg}");
    }

    #[test]
    fn an_extra_statement_breaks_the_recognized_shape_and_falls_back_to_the_normal_list_check() {
        // Cualquier cosa que NO matchee el shape exacto sigue el camino de
        // siempre (List<T> ya calculada) -- acá el cuerpo ni siquiera
        // termina en una expresión, así que falla, pero con el error
        // GENÉRICO de bloque-sin-tail, no uno de push real.
        let src = r#"
            type Item = { id: Int, name: String }
            db { items: Item[] }
            service S {
                stream watchItems() -> Item {
                    let x = 1;
                    while true {
                        db.items.subscribe()
                    }
                }
            }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn subscribe_with_a_non_empty_argument_list_is_rejected_with_a_specific_message() {
        // Cierra el hueco TOCTOU: fuera del shape reconocido, `subscribe`
        // SIEMPRE falla en check_db_method (nunca una firma normal y
        // libremente componible como all/find), así que esto da el mensaje
        // específico de "cuerpo COMPLETO de un stream", no el genérico de
        // "método desconocido".
        let src = r#"
            type Item = { id: Int, name: String }
            db { items: Item[] }
            service S {
                stream watchItems() -> Item {
                    while true {
                        db.items.subscribe(1)
                    }
                }
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("cuerpo COMPLETO de un stream"), "{msg}");
    }

    #[test]
    fn subscribe_used_outside_a_stream_body_is_rejected_with_the_same_specific_message() {
        let src = r#"
            type Item = { id: Int, name: String }
            db { items: Item[] }
            fn f() -> Item[] { db.items.subscribe() }
        "#;
        let result = check_source(src);
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("cuerpo COMPLETO de un stream"), "{msg}");
    }

    // ---- test blocks, assert, panic y service calls (Eje 2) ----

    #[test]
    fn test_block_typechecks_valid_statements() {
        let src = r#"
            service Users {
                rpc list() -> Int { 42 }
            }
            test "invocar service y assert" {
                let n = Users.list();
                assert(n == 42);
                assert(n > 0, "debe ser positivo");
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn test_assert_rejects_non_bool_condition() {
        let src = r#"
            test "condicion invalida" {
                assert(123);
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err());
    }

    #[test]
    fn test_assert_rejects_non_string_message() {
        let src = r#"
            test "mensaje invalido" {
                assert(true, 456);
            }
        "#;
        let result = check_source(src);
        assert!(result.is_err());
    }

    #[test]
    fn test_panic_typechecks_string_message() {
        let src = r#"
            test "usar panic" {
                panic("algo salio mal");
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn did_you_mean_suggests_similar_variable_and_type_names() {
        let src_var = "fn f() -> Int { let count = 10; coutn + 1 }";
        let err_var = check_source(src_var).unwrap_err();
        assert!(err_var[0].message.contains("¿quisiste decir 'count'?"), "{}", err_var[0].message);

        let src_type = "fn f(x: Sttring) -> Int { 0 }";
        let err_type = check_source(src_type).unwrap_err();
        assert!(err_type[0].message.contains("¿quisiste decir 'String'?"), "{}", err_type[0].message);

        let src_field = "type User = { name: String } fn f(u: User) -> String { u.nmae }";
        let err_field = check_source(src_field).unwrap_err();
        assert!(err_field[0].message.contains("¿quisiste decir 'name'?"), "{}", err_field[0].message);
    }

    // GRAMMAR.md §3.52: `sumBy`/`countBy`/`avgBy`/`maxBy`/`minBy`.

    #[test]
    fn sum_by_rejects_grouping_by_an_encrypted_field() {
        // GRAMMAR.md §3.191: `select_grouped` (runtime/db.rs) SIEMPRE arma
        // un GROUP BY real, sin fallback -- agrupar por ciphertext (distinto
        // en cada escritura) daría un grupo por fila siempre, en silencio.
        let src = r#"
            type Order = { id: Int, @encrypted customerSsn: String, amountCents: Int }
            db { orders: Order[] }
            fn f() -> Int {
                let rows = db.orders.sumBy(|o: Order| { o.customerSsn }, |o: Order| { o.amountCents });
                rows.length()
            }
        "#;
        let errs = check_source(src).expect_err("agrupar por un campo @encrypted debe rechazarse");
        assert!(errs.iter().any(|e| e.message.contains("@encrypted") && e.message.contains("customerSsn")), "{errs:?}");
    }

    #[test]
    fn count_by_also_rejects_grouping_by_an_encrypted_field() {
        let src = r#"
            type Order = { id: Int, @encrypted customerSsn: String }
            type SsnCount = { key: String, value: Int }
            db { orders: Order[] }
            service S {
                rpc counts() -> SsnCount[] { db.orders.countBy(|o: Order| { o.customerSsn }) }
            }
        "#;
        let errs = check_source(src).expect_err("countBy agrupando por un campo @encrypted debe rechazarse");
        assert!(errs.iter().any(|e| e.message.contains("@encrypted")), "{errs:?}");
    }

    #[test]
    fn sum_by_typechecks_with_a_real_field_selector_pair() {
        let src = r#"
            type Order = { id: Int, planId: String, amountCents: Int }
            db { orders: Order[] }
            fn f() -> Int {
                let rows = db.orders.sumBy(|o: Order| { o.planId }, |o: Order| { o.amountCents });
                rows.length()
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn count_by_grouping_on_an_enum_field_returns_the_real_enum_as_the_key_type() {
        // No degrada a String -- la key sale con el tipo enum REAL
        // (`field_selector` devuelve el tipo declarado tal cual), así que
        // asignarla a un struct con `key: Plan` tiene que tipar.
        let src = r#"
            enum Plan { Free, Pro }
            type Order = { id: Int, plan: Plan }
            type PlanCount = { key: Plan, value: Int }
            db { orders: Order[] }
            service S {
                rpc counts() -> PlanCount[] { db.orders.countBy(|o: Order| { o.plan }) }
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn aggregate_by_rejects_a_derived_expression_as_the_selector() {
        let src = r#"
            type Order = { id: Int, planId: String, amountCents: Int }
            db { orders: Order[] }
            fn f() -> Int {
                let rows = db.orders.sumBy(|o: Order| { o.planId + "x" }, |o: Order| { o.amountCents });
                rows.length()
            }
        "#;
        let msg = format!("{:?}", check_source(src).unwrap_err());
        assert!(msg.contains("selector de agrupación"), "{msg}");
    }

    #[test]
    fn aggregate_by_rejects_a_float_group_key() {
        let src = r#"
            type Order = { id: Int, score: Float, amountCents: Int }
            db { orders: Order[] }
            fn f() -> Int {
                let rows = db.orders.sumBy(|o: Order| { o.score }, |o: Order| { o.amountCents });
                rows.length()
            }
        "#;
        let msg = format!("{:?}", check_source(src).unwrap_err());
        assert!(msg.contains("solo se puede agrupar por"), "{msg}");
    }

    #[test]
    fn sum_by_rejects_a_non_numeric_value_field() {
        let src = r#"
            type Order = { id: Int, planId: String, label: String }
            db { orders: Order[] }
            fn f() -> Int {
                let rows = db.orders.sumBy(|o: Order| { o.planId }, |o: Order| { o.label });
                rows.length()
            }
        "#;
        let msg = format!("{:?}", check_source(src).unwrap_err());
        assert!(msg.contains("tiene que ser Int, Int64, Decimal o Float"), "{msg}");
    }

    // GRAMMAR.md §3.102: `maxRow`/`minRow` -- la fila COMPLETA, a
    // diferencia de `maxBy`/`minBy` (arriba), que solo agregan un valor.

    #[test]
    fn max_row_returns_an_optional_of_the_element_type() {
        let src = r#"
            type Arm = { id: Int, name: String, avgRewardTenths: Int }
            db { arms: Arm[] }
            service S {
                rpc best() -> Arm? { db.arms.maxRow(|a: Arm| { a.avgRewardTenths }) }
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn min_row_returns_an_optional_of_the_element_type() {
        let src = r#"
            type Arm = { id: Int, name: String, avgRewardTenths: Int }
            db { arms: Arm[] }
            service S {
                rpc worst() -> Arm? { db.arms.minRow(|a: Arm| { a.avgRewardTenths }) }
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn max_row_rejects_a_non_numeric_selector_field() {
        let src = r#"
            type Arm = { id: Int, name: String }
            db { arms: Arm[] }
            service S {
                rpc best() -> Arm? { db.arms.maxRow(|a: Arm| { a.name }) }
            }
        "#;
        let msg = format!("{:?}", check_source(src).unwrap_err());
        assert!(msg.contains("tiene que ser Int, Int64, Decimal o Float"), "{msg}");
    }

    #[test]
    fn max_row_rejects_a_derived_expression_as_the_selector() {
        let src = r#"
            type Arm = { id: Int, score: Int }
            db { arms: Arm[] }
            service S {
                rpc best() -> Arm? { db.arms.maxRow(|a: Arm| { a.score + 1 }) }
            }
        "#;
        let msg = format!("{:?}", check_source(src).unwrap_err());
        assert!(msg.contains("selector de campo"), "{msg}");
    }

    #[test]
    fn max_row_takes_exactly_one_argument() {
        let src = r#"
            type Arm = { id: Int, score: Int }
            db { arms: Arm[] }
            service S {
                rpc best() -> Arm? { db.arms.maxRow(|a: Arm| { a.score }, |a: Arm| { a.score }) }
            }
        "#;
        let msg = format!("{:?}", check_source(src).unwrap_err());
        assert!(msg.contains("toma exactamente 1 argumento"), "{msg}");
    }

    // GRAMMAR.md §3.105: `db.<c>.increment(id, selector, delta) -> T`.

    #[test]
    fn increment_returns_the_element_type_not_optional() {
        let src = r#"
            type Counter = { id: Int, hits: Int }
            db { counters: Counter[] }
            service S {
                rpc bump(id: Int) -> Counter { db.counters.increment(id, |c: Counter| { c.hits }, 1) }
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn increment_rejects_an_int64_field() {
        let src = r#"
            type Counter = { id: Int, hits: Int64 }
            db { counters: Counter[] }
            service S {
                rpc bump(id: Int) -> Counter { db.counters.increment(id, |c: Counter| { c.hits }, 1) }
            }
        "#;
        let msg = format!("{:?}", check_source(src).unwrap_err());
        assert!(msg.contains("deliberadamente afuera"), "{msg}");
    }

    #[test]
    fn increment_rejects_a_float_delta() {
        let src = r#"
            type Counter = { id: Int, hits: Int }
            db { counters: Counter[] }
            service S {
                rpc bump(id: Int) -> Counter { db.counters.increment(id, |c: Counter| { c.hits }, 1.5) }
            }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn increment_rejects_a_derived_expression_as_the_selector() {
        let src = r#"
            type Counter = { id: Int, hits: Int }
            db { counters: Counter[] }
            service S {
                rpc bump(id: Int) -> Counter { db.counters.increment(id, |c: Counter| { c.hits + 1 }, 1) }
            }
        "#;
        let msg = format!("{:?}", check_source(src).unwrap_err());
        assert!(msg.contains("selector de campo"), "{msg}");
    }

    #[test]
    fn increment_takes_exactly_three_arguments() {
        let src = r#"
            type Counter = { id: Int, hits: Int }
            db { counters: Counter[] }
            service S {
                rpc bump(id: Int) -> Counter { db.counters.increment(id, |c: Counter| { c.hits }) }
            }
        "#;
        let msg = format!("{:?}", check_source(src).unwrap_err());
        assert!(msg.contains("toma exactamente 3 argumentos"), "{msg}");
    }

    #[test]
    fn aggregate_by_accepts_int64_as_group_key_and_as_value_field() {
        // GRAMMAR.md §3.65: antes de esta ronda, Int64 se rechazaba en las
        // dos posiciones -- ahora tipa en las dos, con el resultado
        // preservando Int64 (no degradado a Int).
        let src = r#"
            type Sale = { id: Int, region: Int64, amount: Int64 }
            type RegionTotal = { key: Int64, value: Int64 }
            db { sales: Sale[] }
            service S {
                rpc totals() -> RegionTotal[] { db.sales.sumBy(|s: Sale| { s.region }, |s: Sale| { s.amount }) }
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn aggregate_by_accepts_a_truncated_timestamp_as_group_key() {
        // GRAMMAR.md §3.157: cierra el límite que §3.65 dejaba abierto --
        // agrupar por un Timestamp truncado a día/mes/año, no el
        // Timestamp exacto (que seguiría dando un grupo por fila).
        let src = r#"
            type Sale = { id: Int, at: Timestamp, amount: Int }
            type DayTotal = { key: Timestamp, value: Int }
            db { sales: Sale[] }
            service S {
                rpc byDay() -> DayTotal[] { db.sales.sumBy(|s: Sale| { s.at.truncateToDay() }, |s: Sale| { s.amount }) }
                rpc byMonth() -> DayTotal[] { db.sales.sumBy(|s: Sale| { s.at.truncateToMonth() }, |s: Sale| { s.amount }) }
                rpc byYear() -> DayTotal[] { db.sales.sumBy(|s: Sale| { s.at.truncateToYear() }, |s: Sale| { s.amount }) }
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn aggregate_by_still_rejects_an_untruncated_timestamp_as_group_key() {
        // El límite original de §3.65 sigue en pie para el caso SIN
        // truncar -- solo `.truncateToDay/Month/Year()` habilita Timestamp
        // como clave, nunca el campo crudo.
        let src = r#"
            type Sale = { id: Int, at: Timestamp, amount: Int }
            db { sales: Sale[] }
            fn f() -> Int {
                let rows = db.sales.sumBy(|s: Sale| { s.at }, |s: Sale| { s.amount });
                rows.length()
            }
        "#;
        let msg = format!("{:?}", check_source(src).unwrap_err());
        assert!(msg.contains("Timestamp truncado") || msg.contains("truncateTo"), "{msg}");
    }

    #[test]
    fn aggregate_by_rejects_truncate_to_x_on_a_non_timestamp_field() {
        let src = r#"
            type Sale = { id: Int, amount: Int }
            db { sales: Sale[] }
            fn f() -> Int {
                let rows = db.sales.sumBy(|s: Sale| { s.amount.truncateToDay() }, |s: Sale| { s.amount });
                rows.length()
            }
        "#;
        let msg = format!("{:?}", check_source(src).unwrap_err());
        assert!(msg.contains("solo es válido sobre un campo Timestamp"), "{msg}");
    }

    #[test]
    fn aggregate_by_rejects_a_key_optional_field_but_accepts_a_nullable_one() {
        // GRAMMAR.md §3.231: `planId?: String` (JSON) sigue rechazado...
        let src = r#"
            type Order = { id: Int, planId?: String, amountCents: Int }
            db { orders: Order[] }
            fn f() -> Int {
                let rows = db.orders.sumBy(|o: Order| { o.planId }, |o: Order| { o.amountCents });
                rows.length()
            }
        "#;
        let msg = format!("{:?}", check_source(src).unwrap_err());
        assert!(msg.contains("opcional por clave"), "{msg}");
        // ...y `planId: String?` agrupa, con la clave del resultado `String?`
        // (por eso `ByPlan` la declara así -- con `key: String` no tiparía).
        let src = r#"
            type Order = { id: Int, planId: String?, amountCents: Int }
            type ByPlan = { key: String?, value: Int }
            db { orders: Order[] }
            fn f() -> ByPlan[] { db.orders.sumBy(|o: Order| { o.planId }, |o: Order| { o.amountCents }) }
            fn g() -> Int { db.orders.countBy(|o: Order| { o.planId }).filter(|r: ByPlan| { r.key == null }).length() }
        "#;
        check_source(src).unwrap_or_else(|e| panic!("{e:?}"));
        let src = r#"
            type Order = { id: Int, planId: String?, amountCents: Int }
            type ByPlan = { key: String, value: Int }
            db { orders: Order[] }
            fn f() -> ByPlan[] { db.orders.sumBy(|o: Order| { o.planId }, |o: Order| { o.amountCents }) }
        "#;
        assert!(check_source(src).is_err(), "la clave nullable no puede tipar como String pelado");
    }

    #[test]
    fn count_by_takes_exactly_one_argument_the_others_take_exactly_two() {
        let src = r#"
            type Order = { id: Int, planId: String, amountCents: Int }
            db { orders: Order[] }
            fn f() -> Int {
                let rows = db.orders.countBy(|o: Order| { o.planId }, |o: Order| { o.amountCents });
                rows.length()
            }
        "#;
        let msg = format!("{:?}", check_source(src).unwrap_err());
        assert!(msg.contains("'countBy' toma exactamente 1"), "{msg}");

        let src2 = r#"
            type Order = { id: Int, planId: String, amountCents: Int }
            db { orders: Order[] }
            fn f() -> Int {
                let rows = db.orders.sumBy(|o: Order| { o.planId });
                rows.length()
            }
        "#;
        let msg2 = format!("{:?}", check_source(src2).unwrap_err());
        assert!(msg2.contains("'sumBy' toma exactamente 2"), "{msg2}");
    }

    /// GRAMMAR.md §3.95: `countWhere` es un `Int`, mismo contrato de tipos
    /// que `findWhere`/`deleteWhere` (`fn(T) -> Bool`, exactamente 1
    /// argumento) -- la diferencia entre los tres es de EJECUCIÓN
    /// (`runtime/db.rs::count_where_conjunction`), invisible al checker.
    #[test]
    fn count_where_takes_a_predicate_and_returns_int() {
        let src = r#"
            type Review = { id: Int, productId: Int, rating: Int }
            db { reviews: Review[] }
            fn f(productId: Int) -> Int {
                db.reviews.countWhere(|r: Review| { r.productId == productId })
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));

        let wrong_arity = r#"
            type Review = { id: Int, productId: Int }
            db { reviews: Review[] }
            fn f() -> Int {
                db.reviews.countWhere()
            }
        "#;
        let msg = format!("{:?}", check_source(wrong_arity).unwrap_err());
        assert!(msg.contains("'countWhere' toma exactamente 1"), "{msg}");

        let wrong_return = r#"
            type Review = { id: Int, productId: Int }
            db { reviews: Review[] }
            fn f(productId: Int) -> Review[] {
                db.reviews.countWhere(|r: Review| { r.productId == productId })
            }
        "#;
        assert!(check_source(wrong_return).is_err());

        let wrong_predicate_type = r#"
            type Review = { id: Int, productId: Int }
            db { reviews: Review[] }
            fn f() -> Int {
                db.reviews.countWhere(|r: Review| { r.productId })
            }
        "#;
        assert!(check_source(wrong_predicate_type).is_err(), "el predicado tiene que devolver Bool, no Int");
    }

    // ---- `@check(...)` sobre un campo (GRAMMAR.md §3.96) ----

    #[test]
    fn check_min_max_and_range_all_type_check_on_numeric_fields() {
        let src = r#"
            type Review = {
                id: Int,
                @check(range, 1, 5) rating: Int,
                @check(min, 0) helpfulVotes: Int64,
                @check(max, 100.0) discount: Float,
                @check(range, 0, 5) optionalScore: Int?
            }
            db { reviews: Review[] }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn check_rejects_a_non_numeric_field() {
        let src = r#"type Review = { id: Int, @check(range, 1, 5) title: String }"#;
        let msg = format!("{:?}", check_source(src).unwrap_err());
        assert!(msg.contains("solo aplica sobre"), "{msg}");
    }

    #[test]
    fn check_range_rejects_a_min_greater_than_max() {
        let src = r#"type Review = { id: Int, @check(range, 5, 1) rating: Int }"#;
        let msg = format!("{:?}", check_source(src).unwrap_err());
        assert!(msg.contains("mayor que el máximo"), "{msg}");
    }

    #[test]
    fn check_min_length_and_max_length_type_check_on_string_fields() {
        let src = r#"
            type Review = {
                id: Int,
                @check(minLength, 1) title: String,
                @check(maxLength, 280) comment: String,
                @check(minLength, 3) optionalTag: String?
            }
            db { reviews: Review[] }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn check_min_length_rejects_a_non_string_field() {
        let src = r#"type Review = { id: Int, @check(minLength, 1) rating: Int }"#;
        let msg = format!("{:?}", check_source(src).unwrap_err());
        assert!(msg.contains("solo aplica sobre `String`"), "{msg}");
    }

    #[test]
    fn check_min_length_rejects_a_negative_or_fractional_length() {
        let msg = format!("{:?}", check_source(r#"type Review = { id: Int, @check(minLength, -1) title: String }"#).unwrap_err());
        assert!(msg.contains("entero no negativo"), "{msg}");
        let msg = format!("{:?}", check_source(r#"type Review = { id: Int, @check(maxLength, 1.5) title: String }"#).unwrap_err());
        assert!(msg.contains("entero no negativo"), "{msg}");
    }

    #[test]
    fn check_rejects_an_unknown_kind() {
        let tokens = tokenize(r#"type Review = { id: Int, @check(evenNumber) rating: Int }"#).unwrap();
        let err = parse(tokens).expect_err("'@check(evenNumber)' debe rechazarse en el parser");
        assert!(format!("{err:?}").contains("desconocido"), "{err:?}");
    }

    #[test]
    fn check_rejects_a_second_check_on_the_same_field() {
        let tokens = tokenize(r#"type Review = { id: Int, @check(min, 0) @check(max, 5) rating: Int }"#).unwrap();
        let err = parse(tokens).expect_err("dos '@check' en el mismo campo debe rechazarse");
        assert!(format!("{err:?}").contains("repetido"), "{err:?}");
    }

    #[test]
    fn avg_by_always_returns_float_even_summing_an_int_field() {
        let src = r#"
            type Order = { id: Int, planId: String, amountCents: Int }
            type StringFloat = { key: String, value: Float }
            db { orders: Order[] }
            service S {
                rpc avg() -> StringFloat[] { db.orders.avgBy(|o: Order| { o.planId }, |o: Order| { o.amountCents }) }
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    /// `@deprecated("...")` sobre un rpc tipa limpio y coexiste con otras
    /// anotaciones (GRAMMAR.md §3.71) -- es una dimensión ortogonal, igual
    /// que `@rate_limit`.
    #[test]
    fn deprecated_on_an_rpc_typechecks_and_combines_with_auth() {
        let src = r#"
            enum Role { Admin }
            service S {
                @requires(Role.Admin)
                @deprecated("usa panelV2 en su lugar")
                rpc panel() -> Int { 1 }
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn deprecated_twice_on_the_same_rpc_is_rejected() {
        let src = r#"
            service S {
                @deprecated("motivo uno")
                @deprecated("motivo dos")
                rpc old() -> Int { 1 }
            }
        "#;
        let errs = check_source(src).expect_err("dos @deprecated en el mismo rpc debe rechazarse");
        assert!(errs.iter().any(|e| e.message.contains("más de una vez")), "{errs:?}");
    }

    #[test]
    fn deprecated_with_an_empty_reason_on_an_rpc_is_rejected() {
        let src = r#"
            service S {
                @deprecated("")
                rpc old() -> Int { 1 }
            }
        "#;
        let errs = check_source(src).expect_err("motivo vacío debe rechazarse");
        assert!(errs.iter().any(|e| e.message.contains("no puede estar vacío")), "{errs:?}");
    }

    /// `@deprecated` sobre un campo de struct (GRAMMAR.md §3.71) es
    /// puramente informativo para el checker -- no cambia si el struct
    /// tipa, ni la subtipificación estructural (dos structs iguales salvo
    /// el `@deprecated` de un campo siguen siendo el mismo tipo).
    #[test]
    fn deprecated_on_a_struct_field_typechecks_and_does_not_affect_structural_equality() {
        let src = r#"
            type Old = { id: Int, @deprecated("usa email") legacyContact: String }
            type New = { id: Int, legacyContact: String }
            db { olds: Old[] }
            fn takesNew(n: New) -> Int { n.id }
            fn f() -> Int {
                let row = db.olds.find(1);
                match row {
                    o: Old => takesNew(o),
                    null => 0,
                }
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    /// El motivo vacío se rechaza en el PARSER para campos (ver
    /// `parse_field_annotations` en parser.rs -- la validación de forma no
    /// depende de ningún tipo resuelto), así que este test parsea directo
    /// en vez de pasar por `check_source`.
    #[test]
    fn deprecated_with_an_empty_reason_on_a_field_is_rejected() {
        let src = r#"type Old = { id: Int, @deprecated("") legacy: String }"#;
        let tokens = tokenize(src).unwrap_or_else(|e| panic!("{e}"));
        let err = parse(tokens).expect_err("motivo vacío en campo debe rechazarse");
        assert!(format!("{err:?}").contains("no puede estar vacío"), "{err:?}");
    }

    #[test]
    fn an_unknown_annotation_on_a_field_is_rejected() {
        let src = r#"type Old = { id: Int, @authenticated legacy: String }"#;
        let tokens = tokenize(src).unwrap_or_else(|e| panic!("{e}"));
        let err = parse(tokens).expect_err("una anotación de rpc sobre un campo debe rechazarse");
        assert!(format!("{err:?}").contains("anotación desconocida"), "{err:?}");
    }

    // ---- `@validate(...)` sobre un campo (GRAMMAR.md §3.73) ----

    #[test]
    fn validate_email_on_a_string_field_typechecks() {
        let src = r#"type Signup = { @validate(email) email: String }"#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn validate_regex_on_a_string_field_typechecks() {
        let src = r#"type Order = { @validate(regex, "^[A-Z]{3}$") sku: String }"#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    /// `String?` también es válido -- `@validate` no exige que el campo sea
    /// requerido, solo que sea texto.
    #[test]
    fn validate_on_an_optional_string_field_typechecks() {
        let src = r#"type Signup = { @validate(email) email?: String }"#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn validate_on_a_non_string_field_is_rejected() {
        let src = r#"type Signup = { @validate(email) age: Int }"#;
        let errs = check_source(src).expect_err("@validate sobre un Int debe rechazarse");
        assert!(errs.iter().any(|e| e.message.contains("solo aplica sobre")), "{errs:?}");
    }

    #[test]
    fn validate_regex_with_an_invalid_pattern_is_rejected() {
        let src = r#"type Order = { @validate(regex, "[unclosed") sku: String }"#;
        let errs = check_source(src).expect_err("un patrón regex inválido debe rechazarse");
        assert!(errs.iter().any(|e| e.message.contains("patrón inválido")), "{errs:?}");
    }

    #[test]
    fn a_second_validate_on_the_same_field_is_a_parse_error() {
        let src = r#"type Signup = { @validate(email) @validate(email) email: String }"#;
        let tokens = tokenize(src).unwrap_or_else(|e| panic!("{e}"));
        let err = parse(tokens).expect_err("dos @validate en el mismo campo debe rechazarse");
        assert!(format!("{err:?}").contains("repetido"), "{err:?}");
    }

    #[test]
    fn an_unknown_validate_kind_is_a_parse_error() {
        let src = r#"type Signup = { @validate(minLength, 3) name: String }"#;
        let tokens = tokenize(src).unwrap_or_else(|e| panic!("{e}"));
        let err = parse(tokens).expect_err("una forma de @validate desconocida debe rechazarse");
        assert!(format!("{err:?}").contains("desconocido"), "{err:?}");
    }

    /// `@validate` también funciona sobre el campo de una variante de enum
    /// -- comparte `Field` con `type`, y `check_field_validators` se llama
    /// para las dos formas (ver `check_program_full`).
    #[test]
    fn validate_on_an_enum_variant_field_typechecks() {
        let src = r#"enum Event { SignedUp { @validate(email) email: String } }"#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    // ---- valores por defecto en campos de struct (GRAMMAR.md §3.74) ----

    #[test]
    fn a_field_default_that_matches_the_field_type_typechecks() {
        let src = r#"type Task = { title: String, status: String = "pending" }"#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn a_field_default_of_the_wrong_type_is_rejected() {
        let src = "type Task = { title: String, retries: Int = \"tres\" }";
        let errs = check_source(src).expect_err("un default de tipo equivocado debe rechazarse");
        assert!(errs.iter().any(|e| e.message.contains("se esperaba")), "{errs:?}");
    }

    /// Un campo CON default puede omitirse del literal igual que uno `?:`
    /// -- sin romper la exigencia de que uno SIN default (y sin `?`) sigue
    /// siendo requerido.
    #[test]
    fn omitting_a_field_with_a_default_typechecks_but_omitting_one_without_it_does_not() {
        let src = r#"
            type Task = { title: String, status: String = "pending" }
            fn f() -> Task { Task { title: "comprar leche" } }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));

        let src2 = r#"
            type Task = { title: String, status: String }
            fn f() -> Task { Task { status: "pending" } }
        "#;
        let errs = check_source(src2).expect_err("sin default, omitir 'title' debe rechazarse");
        assert!(errs.iter().any(|e| e.message.contains("falta el campo requerido")), "{errs:?}");
    }

    #[test]
    fn a_default_on_an_enum_variant_field_typechecks() {
        let src = r#"enum Event { Created { status: String = "new" } }"#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    // ---- db.<c>.upsert(matchFn, insertValue, updateFn) (GRAMMAR.md §3.75) ----

    #[test]
    fn upsert_with_the_right_shapes_typechecks() {
        let src = r#"
            type Counter = { id: Int, name: String, count: Int }
            type NewCounter = { name: String, count: Int }
            db { counters: Counter[] }
            service S {
                rpc bump(name: String) -> Counter {
                    db.counters.upsert(
                        |c: Counter| { c.name == name },
                        NewCounter { name: name, count: 1 },
                        |c: Counter| { NewCounter { name: c.name, count: c.count + 1 } }
                    )
                }
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    /// `updateFn` tiene que devolver `Omit<T,"id">` COMPLETO, no `Patch<T>`
    /// -- ver el porqué en `check_db_method`. Un `updateFn` que devuelve un
    /// tipo distinto (acá, `Int`) se rechaza como cualquier otro argumento
    /// de tipo función equivocado.
    #[test]
    fn upsert_rejects_an_update_fn_with_the_wrong_return_shape() {
        let src = r#"
            type Counter = { id: Int, name: String, count: Int }
            type NewCounter = { name: String, count: Int }
            db { counters: Counter[] }
            service S {
                rpc bump(name: String) -> Counter {
                    db.counters.upsert(
                        |c: Counter| { c.name == name },
                        NewCounter { name: name, count: 1 },
                        |c: Counter| { c.count }
                    )
                }
            }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn upsert_with_the_wrong_number_of_arguments_is_rejected() {
        let src = r#"
            type Counter = { id: Int, name: String, count: Int }
            db { counters: Counter[] }
            service S {
                rpc bump(name: String) -> Counter {
                    db.counters.upsert(|c: Counter| { c.name == name })
                }
            }
        "#;
        let errs = check_source(src).expect_err("upsert con menos de 3 argumentos debe rechazarse");
        assert!(errs.iter().any(|e| e.message.contains("toma exactamente 3 argumentos")), "{errs:?}");
    }

    // ---- db.<c>.insertMany(items) (GRAMMAR.md §3.76) ----

    #[test]
    fn insert_many_with_a_list_of_the_insertable_shape_typechecks() {
        let src = r#"
            type Task = { id: Int, title: String }
            type NewTask = { title: String }
            db { tasks: Task[] }
            service S {
                rpc seed() -> Task[] {
                    db.tasks.insertMany([NewTask { title: "a" }, NewTask { title: "b" }])
                }
            }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn insert_many_rejects_a_list_of_the_wrong_shape() {
        let src = r#"
            type Task = { id: Int, title: String }
            db { tasks: Task[] }
            service S {
                rpc seed() -> Task[] { db.tasks.insertMany([1, 2, 3]) }
            }
        "#;
        assert!(check_source(src).is_err());
    }

    #[test]
    fn insert_many_with_the_wrong_number_of_arguments_is_rejected() {
        let src = r#"
            type Task = { id: Int, title: String }
            db { tasks: Task[] }
            service S {
                rpc seed() -> Task[] { db.tasks.insertMany() }
            }
        "#;
        let errs = check_source(src).expect_err("insertMany sin argumentos debe rechazarse");
        assert!(errs.iter().any(|e| e.message.contains("toma exactamente 1 argumento")), "{errs:?}");
    }

    // ---- createdAt/updatedAt automáticos: `= now()` + `@autoUpdate` (GRAMMAR.md §3.77) ----

    #[test]
    fn auto_update_on_a_timestamp_field_typechecks() {
        let src = r#"type Task = { id: Int, @autoUpdate updatedAt: Timestamp = now() }"#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn auto_update_on_a_non_timestamp_field_is_rejected() {
        let src = "type Task = { id: Int, @autoUpdate title: String }";
        let errs = check_source(src).expect_err("@autoUpdate sobre un String debe rechazarse");
        assert!(errs.iter().any(|e| e.message.contains("solo aplica sobre")), "{errs:?}");
    }

    #[test]
    fn auto_update_does_not_require_a_default() {
        // @autoUpdate y `= now()` son ortogonales -- el primero solo importa
        // en applyPatch, así que un campo requerido sin default también es válido.
        let src = "type Task = { id: Int, @autoUpdate updatedAt: Timestamp }";
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn a_second_auto_update_on_the_same_field_is_a_parse_error() {
        let src = "type Task = { id: Int, @autoUpdate @autoUpdate updatedAt: Timestamp }";
        let tokens = tokenize(src).unwrap_or_else(|e| panic!("{e}"));
        let err = parse(tokens).expect_err("dos @autoUpdate en el mismo campo debe rechazarse");
        assert!(format!("{err:?}").contains("repetido"), "{err:?}");
    }

    // ---- soft-delete nativo: `@softDelete` (GRAMMAR.md §3.78) ----

    #[test]
    fn soft_delete_on_a_timestamp_optional_field_typechecks() {
        let src = "type Task = { id: Int, @softDelete deletedAt: Timestamp? = null }";
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn soft_delete_on_a_required_timestamp_is_rejected() {
        // `Timestamp` (sin `?`) no puede representar "no borrado" -- tiene
        // que ser `Timestamp?`.
        let src = "type Task = { id: Int, @softDelete deletedAt: Timestamp }";
        let errs = check_source(src).expect_err("@softDelete sobre Timestamp requerido debe rechazarse");
        assert!(errs.iter().any(|e| e.message.contains("solo aplica sobre")), "{errs:?}");
    }

    #[test]
    fn soft_delete_on_a_non_timestamp_field_is_rejected() {
        let src = "type Task = { id: Int, @softDelete deletedAt: String? }";
        let errs = check_source(src).expect_err("@softDelete sobre String? debe rechazarse");
        assert!(errs.iter().any(|e| e.message.contains("solo aplica sobre")), "{errs:?}");
    }

    #[test]
    fn two_soft_delete_fields_on_the_same_struct_is_rejected() {
        let src = "type Task = { id: Int, @softDelete a: Timestamp? = null, @softDelete b: Timestamp? = null }";
        let errs = check_source(src).expect_err("dos @softDelete en el mismo struct debe rechazarse");
        assert!(errs.iter().any(|e| e.message.contains("más de un campo con '@softDelete'")), "{errs:?}");
    }

    #[test]
    fn a_second_soft_delete_annotation_on_the_same_field_is_a_parse_error() {
        let src = "type Task = { id: Int, @softDelete @softDelete deletedAt: Timestamp? = null }";
        let tokens = tokenize(src).unwrap_or_else(|e| panic!("{e}"));
        let err = parse(tokens).expect_err("dos @softDelete en el mismo campo debe rechazarse");
        assert!(format!("{err:?}").contains("repetido"), "{err:?}");
    }

    // ---- cifrado de campo a nivel de columna: `@encrypted` (GRAMMAR.md §3.191) ----

    #[test]
    fn encrypted_on_a_string_field_typechecks() {
        let src = "type User = { id: Int, @encrypted ssn: String }";
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn encrypted_on_an_optional_string_field_typechecks() {
        let src = "type User = { id: Int, @encrypted ssn: String? }";
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn encrypted_on_a_non_string_field_is_rejected() {
        let src = "type User = { id: Int, @encrypted age: Int }";
        let errs = check_source(src).expect_err("@encrypted sobre un Int debe rechazarse");
        assert!(errs.iter().any(|e| e.message.contains("solo aplica sobre")), "{errs:?}");
    }

    #[test]
    fn encrypted_combined_with_unique_on_the_same_field_is_rejected() {
        // El nonce aleatorio hace que el ciphertext sea distinto en cada
        // escritura -- un UNIQUE sobre esa columna sería siempre "único",
        // incluso para el mismo valor en texto plano. Garantía falsa, no
        // solo redundante.
        let src = "type User = { id: Int, @encrypted @unique ssn: String }";
        let errs = check_source(src).expect_err("@encrypted + @unique en el mismo campo debe rechazarse");
        assert!(errs.iter().any(|e| e.message.contains("incompatible con '@index'/'@unique'")), "{errs:?}");
    }

    #[test]
    fn encrypted_combined_with_index_on_the_same_field_is_rejected() {
        let src = "type User = { id: Int, @encrypted @index ssn: String }";
        let errs = check_source(src).expect_err("@encrypted + @index en el mismo campo debe rechazarse");
        assert!(errs.iter().any(|e| e.message.contains("incompatible con '@index'/'@unique'")), "{errs:?}");
    }

    #[test]
    fn a_second_encrypted_annotation_on_the_same_field_is_a_parse_error() {
        let src = "type User = { id: Int, @encrypted @encrypted ssn: String }";
        let tokens = tokenize(src).unwrap_or_else(|e| panic!("{e}"));
        let err = parse(tokens).expect_err("dos '@encrypted' en el mismo campo debe rechazarse");
        assert!(format!("{err:?}").contains("repetido"), "{err:?}");
    }

    // ---- índices declarativos: `@index`/`@unique` (GRAMMAR.md §3.80) ----

    #[test]
    fn index_and_unique_annotations_typecheck_on_any_field_type() {
        // A diferencia de `@autoUpdate`/`@softDelete`, ninguno de los dos
        // exige un tipo particular -- un índice SQL tiene sentido sobre
        // casi cualquier columna.
        let src = "type User = { id: Int, @unique email: String, @index age: Int? }";
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn a_second_index_annotation_on_the_same_field_is_a_parse_error() {
        let src = "type User = { id: Int, @index @index email: String }";
        let tokens = tokenize(src).unwrap_or_else(|e| panic!("{e}"));
        let err = parse(tokens).expect_err("dos '@index' en el mismo campo debe rechazarse");
        assert!(format!("{err:?}").contains("repetido"), "{err:?}");
    }

    #[test]
    fn a_second_unique_annotation_on_the_same_field_is_a_parse_error() {
        let src = "type User = { id: Int, @unique @unique email: String }";
        let tokens = tokenize(src).unwrap_or_else(|e| panic!("{e}"));
        let err = parse(tokens).expect_err("dos '@unique' en el mismo campo debe rechazarse");
        assert!(format!("{err:?}").contains("repetido"), "{err:?}");
    }

    #[test]
    fn combining_index_and_unique_on_the_same_field_is_a_parse_error() {
        // Los dos piden un índice -- `@unique` además una restricción de
        // unicidad -- combinarlos sería redundante, rechazado por forma
        // (no un error del checker).
        let src = "type User = { id: Int, @index @unique email: String }";
        let tokens = tokenize(src).unwrap_or_else(|e| panic!("{e}"));
        let err = parse(tokens).expect_err("'@index' + '@unique' en el mismo campo debe rechazarse");
        assert!(format!("{err:?}").contains("repetido"), "{err:?}");
    }

    // ---- `@unique(campo1, campo2, ...)` a nivel de `type` (GRAMMAR.md §3.155) ----

    #[test]
    fn composite_unique_with_real_fields_typechecks() {
        let src = r#"
            @unique(profileId, slug)
            type Product = { id: Int, profileId: Int, slug: String }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn composite_unique_naming_more_than_two_fields_typechecks() {
        let src = r#"
            @unique(a, b, c)
            type T = { id: Int, a: Int, b: Int, c: Int }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn composite_unique_naming_an_unknown_field_is_rejected() {
        let src = r#"
            @unique(profileId, doesNotExist)
            type Product = { id: Int, profileId: Int, slug: String }
        "#;
        let result = check_source(src);
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("doesNotExist"), "{msg}");
    }

    #[test]
    fn composite_unique_with_fewer_than_two_fields_is_rejected() {
        let src = r#"
            @unique(profileId)
            type Product = { id: Int, profileId: Int }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "un solo campo en '@unique(...)' a nivel de type debe rechazarse -- usá '@unique' de campo");
    }

    #[test]
    fn composite_unique_repeating_the_same_field_twice_is_rejected() {
        let src = r#"
            @unique(profileId, profileId)
            type Product = { id: Int, profileId: Int }
        "#;
        let result = check_source(src);
        assert!(result.is_err());
    }

    #[test]
    fn composite_unique_on_a_non_struct_type_is_rejected() {
        let src = "@unique(a, b) type NotAStruct = Int[];";
        let result = check_source(src);
        assert!(result.is_err(), "'@unique(...)' sobre un alias que no es struct debe rechazarse");
    }

    #[test]
    fn declaring_the_exact_same_composite_unique_twice_is_rejected() {
        let src = r#"
            @unique(a, b)
            @unique(b, a)
            type T = { id: Int, a: Int, b: Int }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "el MISMO conjunto de campos (sin importar el orden) declarado dos veces debe rechazarse por redundante");
    }

    #[test]
    fn two_composite_uniques_with_different_field_sets_both_typecheck() {
        let src = r#"
            @unique(a, b)
            @unique(a, c)
            type T = { id: Int, a: Int, b: Int, c: Int }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    // ---- `@check(<expr>)` a nivel de `type` (GRAMMAR.md §3.173) ----

    #[test]
    fn type_level_check_comparing_two_fields_typechecks() {
        let src = r#"
            @check(endDate > startDate)
            type Booking = { id: Int, startDate: Timestamp, endDate: Timestamp }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn type_level_check_with_and_or_and_arithmetic_typechecks() {
        let src = r#"
            @check(discountPrice <= price && (bonus + price) > 0)
            type Product = { id: Int, price: Int, discountPrice: Int, bonus: Int }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn type_level_check_referencing_an_unknown_field_is_rejected() {
        let src = r#"
            @check(endDate > doesNotExist)
            type Booking = { id: Int, startDate: Timestamp, endDate: Timestamp }
        "#;
        let result = check_source(src);
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("doesNotExist"), "{msg}");
    }

    #[test]
    fn type_level_check_with_mismatched_types_is_rejected() {
        let src = r#"
            @check(name > age)
            type Person = { id: Int, name: String, age: Int }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "comparar String con Int tiene que fallar, mismo criterio que cualquier otro '<'/'>'");
    }

    #[test]
    fn type_level_check_that_does_not_typecheck_to_bool_is_rejected() {
        let src = r#"
            @check(price + bonus)
            type Product = { id: Int, price: Int, bonus: Int }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "'@check(...)' de tipo tiene que tipar a Bool, no a Int");
    }

    #[test]
    fn type_level_check_calling_a_function_is_rejected() {
        let src = r#"
            fn isValid(x: Int) -> Bool { x > 0 }
            @check(isValid(price))
            type Product = { id: Int, price: Int }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "una llamada dentro de '@check(...)' de tipo no puede traducirse a un CHECK de SQL -- tiene que rechazarse");
    }

    #[test]
    fn type_level_check_accessing_db_is_rejected() {
        let src = r#"
            type Order = { id: Int }
            db { orders: Order[] }
            @check(db.orders.count() > 0)
            type Product = { id: Int }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "acceso a 'db' dentro de '@check(...)' de tipo tiene que rechazarse -- no puede evaluarse dentro de un CHECK de SQL");
    }

    #[test]
    fn type_level_check_on_a_non_struct_type_is_rejected() {
        let src = "@check(true) type NotAStruct = Int[];";
        let result = check_source(src);
        assert!(result.is_err(), "'@check(...)' sobre un alias que no es struct debe rechazarse");
    }

    #[test]
    fn type_level_check_coexists_with_composite_unique_on_the_same_type() {
        let src = r#"
            @unique(startDate, room)
            @check(endDate > startDate)
            type Booking = { id: Int, room: String, startDate: Timestamp, endDate: Timestamp }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    // ---- `@unique(...) where <expr>` a nivel de `type` (GRAMMAR.md §3.174) ----

    /// Caso real motivador (GRAMMAR.md §3.174, citado desde el schema Drizzle
    /// de Glowapp): la condición referencia `status`, un campo que NO
    /// integra el conjunto único (`userId`/`appointmentDate`/`startTime`).
    #[test]
    fn conditional_composite_unique_with_real_fields_typechecks() {
        let src = r#"
            @unique(userId, appointmentDate, startTime) where status != "cancelled"
            type Appointment = { id: Int, userId: Int, appointmentDate: String, startTime: String, status: String }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    #[test]
    fn conditional_composite_unique_referencing_an_unknown_field_in_the_condition_is_rejected() {
        let src = r#"
            @unique(a, b) where doesNotExist != "x"
            type T = { id: Int, a: Int, b: Int }
        "#;
        let result = check_source(src);
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("doesNotExist"), "{msg}");
    }

    #[test]
    fn conditional_composite_unique_whose_condition_calls_a_function_is_rejected() {
        let src = r#"
            fn isActive(s: String) -> Bool { s != "cancelled" }
            @unique(a, b) where isActive(status)
            type T = { id: Int, a: Int, b: Int, status: String }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "una llamada dentro de 'where <expr>' no puede traducirse a un CHECK de SQL -- tiene que rechazarse");
    }

    #[test]
    fn conditional_composite_unique_whose_condition_does_not_typecheck_to_bool_is_rejected() {
        let src = r#"
            @unique(a, b) where a + b
            type T = { id: Int, a: Int, b: Int }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "'where <expr>' tiene que tipar a Bool, no a Int");
    }

    /// Dos `@unique` con el MISMO conjunto de campos pero condiciones
    /// DISTINTAS son dos constraints parciales distintos -- no un
    /// duplicado, a diferencia de dos `@unique` sin condición sobre el
    /// mismo conjunto (`declaring_the_exact_same_composite_unique_twice_is_rejected`).
    #[test]
    fn two_conditional_composite_uniques_with_the_same_fields_but_different_conditions_both_typecheck() {
        let src = r#"
            @unique(a, b) where status == "x"
            @unique(a, b) where status == "y"
            type T = { id: Int, a: Int, b: Int, status: String }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }

    /// A diferencia del test de arriba, el MISMO conjunto de campos con la
    /// MISMA condición sí es redundante.
    #[test]
    fn two_conditional_composite_uniques_with_the_same_fields_and_the_same_condition_are_rejected_as_redundant() {
        let src = r#"
            @unique(a, b) where status == "x"
            @unique(a, b) where status == "x"
            type T = { id: Int, a: Int, b: Int, status: String }
        "#;
        let result = check_source(src);
        assert!(result.is_err(), "mismo conjunto de campos Y misma condición -- tiene que rechazarse por redundante");
    }

    /// Y un `@unique` CON condición no puede confundirse con uno SIN
    /// condición sobre el mismo conjunto de campos -- son dos constraints
    /// distintos (uno total, uno parcial), ninguno redundante con el otro.
    #[test]
    fn a_conditional_and_an_unconditional_composite_unique_over_the_same_fields_both_typecheck() {
        let src = r#"
            @unique(a, b)
            @unique(a, b) where status == "x"
            type T = { id: Int, a: Int, b: Int, status: String }
        "#;
        assert!(check_source(src).is_ok(), "{:?}", check_source(src));
    }
}

/// GRAMMAR.md §3.230 (PLAN.md §9.19 ítem 5): `db.<c>.orderBy`/`orderByDesc`
/// devuelven una consulta ordenada que solo admite lecturas, y
/// `List<T>.sortBy`/`sortByDesc` exigen una clave con orden total. Los
/// errores se prueban por SUBSTRING del mensaje real, porque ese texto es
/// lo único que un programador ve.
#[cfg(test)]
mod order_by_tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::parse;

    const BASE: &str = "type Event = { id: Int, kind: String, amount: Int, at: Timestamp?, tags: String[], note?: String }\ntype NewEvent = { kind: String, amount: Int, at: Timestamp?, tags: String[] }\ndb { events: Event[] }\n";

    fn check(body: &str) -> Result<(), Vec<CheckError>> {
        let src = format!("{BASE}service S {{ {body} }}");
        let tokens = tokenize(&src).unwrap_or_else(|e| panic!("{e}"));
        let program = parse(tokens).unwrap_or_else(|e| panic!("{e:?}"));
        Checker::check_program(&program)
    }

    fn first_error(body: &str) -> String {
        check(body).expect_err("tenía que fallar el checker").iter().map(|e| format!("{e:?}")).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn ordered_reads_and_in_memory_sorts_type_check() {
        check(
            "rpc newest(n: Int) -> Event[] { db.events.orderByDesc(|e: Event| { e.at }).page(n, 0) }
             rpc byKind(k: String) -> Event[] { db.events.orderBy(|e: Event| { e.kind }).orderByDesc(|e: Event| { e.amount }).findWhere(|e: Event| { e.kind == k }) }
             rpc everything() -> Event[] { db.events.orderBy(|e: Event| { e.amount }).all() }
             rpc sorted() -> Event[] { db.events.all().sortBy(|e: Event| { e.amount }) }
             rpc sortedNullable() -> Event[] { db.events.all().sortByDesc(|e: Event| { e.at }) }
             rpc sortedStr() -> Event[] { db.events.all().sortBy(|e: Event| { e.kind }) }",
        )
        .unwrap_or_else(|errs| panic!("{errs:?}"));
    }

    #[test]
    fn ordering_by_a_json_field_or_a_key_optional_field_is_rejected() {
        let msg = first_error("rpc bad() -> Event[] { db.events.orderBy(|e: Event| { e.tags }).all() }");
        assert!(msg.contains("solo se puede ordenar por") && msg.contains("'tags'"), "{msg}");
        let msg = first_error("rpc bad() -> Event[] { db.events.orderBy(|e: Event| { e.note }).all() }");
        assert!(msg.contains("opcional por clave"), "{msg}");
        let msg = first_error("rpc bad() -> Event[] { db.events.orderBy(|e: Event| { e.amount + 1 }).all() }");
        assert!(msg.contains("selector de campo de orden"), "{msg}");
    }

    #[test]
    fn an_ordered_query_only_accepts_reads_and_never_page_after() {
        let msg = first_error("rpc bad() -> Event[] { db.events.orderBy(|e: Event| { e.amount }).pageAfter(null, 5) }");
        assert!(msg.contains("no se puede combinar con 'orderBy'"), "{msg}");
        let msg = first_error("rpc bad() -> Event { db.events.orderBy(|e: Event| { e.amount }).insert(NewEvent { kind: \"a\", amount: 1, at: null, tags: [] }) }");
        assert!(msg.contains("no existe sobre una consulta ordenada"), "{msg}");
        // La consulta en sí nunca es un valor de rpc: sin `.all()` no tipa.
        assert!(check("rpc bad() -> Event[] { db.events.orderBy(|e: Event| { e.amount }) }").is_err());
    }

    #[test]
    fn ordering_by_an_encrypted_field_is_a_compile_error() {
        let src = "type Person = { id: Int, @encrypted ssn: String }\ndb { people: Person[] }\nservice S { rpc bad() -> Person[] { db.people.orderBy(|p: Person| { p.ssn }).all() } }";
        let tokens = tokenize(src).unwrap_or_else(|e| panic!("{e}"));
        let program = parse(tokens).unwrap_or_else(|e| panic!("{e:?}"));
        let msg = Checker::check_program(&program).expect_err("tenía que fallar").iter().map(|e| format!("{e:?}")).collect::<Vec<_>>().join("\n");
        assert!(msg.contains("@encrypted") && msg.contains("ordenar"), "{msg}");
    }

    #[test]
    fn sort_by_needs_a_totally_ordered_key() {
        let msg = first_error("rpc bad() -> Event[] { db.events.all().sortBy(|e: Event| { e.tags }) }");
        assert!(msg.contains("la clave de orden es"), "{msg}");
    }
}

/// GRAMMAR.md §3.232 (PLAN.md §9.19 ítem 7): `@hidden`.
#[cfg(test)]
mod hidden_tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::parse;

    fn check(src: &str) -> Result<Checker, String> {
        let tokens = tokenize(src).map_err(|e| format!("{e}"))?;
        let program = parse(tokens).map_err(|e| format!("{e:?}"))?;
        Checker::check_program(&program).map_err(|e| format!("{e:?}"))?;
        Ok(Checker::build_symbols(&program).0)
    }

    #[test]
    fn hidden_fields_are_indexed_and_readable_inside_an_rpc() {
        let checker = check(
            "type User = { id: Int, email: String, @hidden passwordHash: String }
             type NewUser = { email: String, passwordHash: String }
             db { users: User[] }
             service S {
               rpc create(email: String, hash: String) -> User { db.users.insert(NewUser { email: email, passwordHash: hash }) }
               rpc withHash(h: String) -> User[] { db.users.all().filter(|u: User| { u.passwordHash == h }) }
               stream live() -> User { while true { db.users.subscribe() } }
             }",
        )
        .unwrap_or_else(|e| panic!("{e}"));
        assert!(checker.hidden_fields["User"].contains("passwordHash"));
        assert!(!checker.hidden_fields.contains_key("NewUser"));
    }

    #[test]
    fn hidden_id_and_hidden_types_as_params_are_rejected() {
        let msg = check("type User = { @hidden id: Int, email: String }\ndb { users: User[] }").err().expect("tenía que fallar");
        assert!(msg.contains("'@hidden' sobre 'id'"), "{msg}");
        let msg = check(
            "type User = { id: Int, @hidden passwordHash: String }
             type Wrapper = { users: User[] }
             service S { rpc bad(w: Wrapper) -> Int { 1 } }",
        )
        .err().expect("tenía que fallar");
        assert!(msg.contains("'User', un type con campos '@hidden'") && msg.contains("'w'"), "{msg}");
        let msg = check("type User = { id: Int, @hidden @hidden x: Int }").err().expect("tenía que fallar");
        assert!(msg.contains("repetido"), "{msg}");
    }
}

/// GRAMMAR.md §3.234 (PLAN.md §9.20 Eje G ítem 2): el bloque `ai { }`.
#[cfg(test)]
mod ai_block_tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::parse;

    fn check(src: &str) -> Result<Checker, String> {
        let tokens = tokenize(src).map_err(|e| format!("{e}"))?;
        let program = parse(tokens).map_err(|e| format!("{e:?}"))?;
        Checker::check_program(&program).map_err(|e| format!("{e:?}"))?;
        Ok(Checker::build_symbols(&program).0)
    }

    #[test]
    fn an_ai_block_declares_aliases_in_order_and_ai_stays_a_plain_identifier_elsewhere() {
        let checker = check(
            "ai { router: \"qwen2.5:0.5b\", coder: \"./qwen2.5-coder-7b.gguf\", }
             type Note = { id: Int, ai: String }
             db { notes: Note[] }
             service S { rpc ai(ai: String) -> String { let ai2 = ai; ai2 } }",
        )
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            checker.ai_models(),
            vec![("router".to_string(), "qwen2.5:0.5b".to_string()), ("coder".to_string(), "./qwen2.5-coder-7b.gguf".to_string())]
        );
    }

    #[test]
    fn duplicate_aliases_empty_specs_and_non_string_specs_are_rejected() {
        let msg = check("ai { router: \"a:b\" }\nai { router: \"c:d\" }").err().expect("tenía que fallar");
        assert!(msg.contains("ya está declarado"), "{msg}");
        let msg = check("ai { router: \"  \" }").err().expect("tenía que fallar");
        assert!(msg.contains("spec vacía"), "{msg}");
        let msg = check("ai { router: 42 }").err().expect("tenía que fallar");
        assert!(msg.contains("tiene que ser un string"), "{msg}");
    }
}

/// GRAMMAR.md §3.235 (PLAN.md §9.20 Eje G ítem 3): `ai.generate`/`ai.chat`/
/// `ai.models` y el tipo pre-sembrado `AiMessage`.
#[cfg(test)]
mod ai_builtin_tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::parse;

    fn check(src: &str) -> Result<(), String> {
        let tokens = tokenize(src).map_err(|e| format!("{e}"))?;
        let program = parse(tokens).map_err(|e| format!("{e:?}"))?;
        Checker::check_program(&program).map_err(|e| format!("{e:?}"))
    }

    #[test]
    fn ai_builtins_type_check_with_the_seeded_message_type_and_a_structural_one() {
        check(
            "ai { router: \"qwen2.5:0.5b\" }
             type Turn = { role: String, content: String, extra: Int }
             service S {
               rpc ask(p: String) -> String { ai.generate(\"router\", p, 64) }
               rpc chat(q: String) -> String { ai.chat(\"router\", [AiMessage { role: \"user\", content: q }], 32) }
               rpc chat2(q: String) -> String { ai.chat(\"router\", [Turn { role: \"user\", content: q, extra: 1 }], 32) }
               rpc models() -> String[] { ai.models() }
             }",
        )
        .unwrap_or_else(|e| panic!("{e}"));
    }

    #[test]
    fn ai_builtins_reject_wrong_arity_and_types() {
        let msg = check("service S { rpc a() -> String { ai.generate(\"r\", \"p\") } }").expect_err("tenía que fallar");
        assert!(msg.contains("'ai.generate' toma exactamente 3 argumentos"), "{msg}");
        let msg = check("service S { rpc a() -> String { ai.chat(\"r\", [\"hola\"], 8) } }").expect_err("tenía que fallar");
        assert!(msg.contains("AiMessage") || msg.contains("role"), "{msg}");
        let msg = check("service S { rpc a() -> Int { ai.generate(\"r\", \"p\", 8) } }").expect_err("tenía que fallar");
        assert!(msg.contains("String") && msg.contains("Int"), "{msg}");
    }
}

/// GRAMMAR.md §3.236: `ai.stream` como cuerpo de un `stream -> AiToken`.
#[cfg(test)]
mod ai_stream_tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::parse;

    fn check(src: &str) -> Result<(), String> {
        let tokens = tokenize(src).map_err(|e| format!("{e}"))?;
        let program = parse(tokens).map_err(|e| format!("{e:?}"))?;
        Checker::check_program(&program).map_err(|e| format!("{e:?}"))
    }

    #[test]
    fn ai_stream_types_as_a_token_stream_and_is_recognized_as_the_whole_body() {
        let src = "ai { router: \"qwen2.5:0.5b\" }
             service S {
               stream reply(q: String) -> AiToken { ai.stream(\"router\", [AiMessage { role: \"user\", content: q }], 64) }
               rpc all(q: String) -> AiToken[] { ai.stream(\"router\", [AiMessage { role: \"user\", content: q }], 8) }
             }";
        check(src).unwrap_or_else(|e| panic!("{e}"));
        let program = parse(tokenize(src).unwrap()).unwrap();
        assert!(crate::runtime::ai_stream_member(&program, "S", "reply"));
        assert!(!crate::runtime::ai_stream_member(&program, "S", "all"), "un rpc no es un stream");
    }

    #[test]
    fn ai_stream_needs_the_token_shape_and_three_arguments() {
        let msg = check("ai { r: \"x\" } service S { stream reply() -> String { ai.stream(\"r\", [], 8) } }").expect_err("tenía que fallar");
        assert!(msg.contains("AiToken") || msg.contains("token"), "{msg}");
        let msg = check("ai { r: \"x\" } service S { stream reply() -> AiToken { ai.stream(\"r\", []) } }").expect_err("tenía que fallar");
        assert!(msg.contains("'ai.stream' toma exactamente 3 argumentos"), "{msg}");
    }
}

/// GRAMMAR.md §3.239: `@index(campo1, campo2, ...)` compuesto a nivel de type.
#[cfg(test)]
mod composite_index_tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::parse;

    fn check(src: &str) -> Result<(), String> {
        let tokens = tokenize(src).map_err(|e| format!("{e}"))?;
        let program = parse(tokens).map_err(|e| format!("{e:?}"))?;
        Checker::check_program(&program).map_err(|e| format!("{e:?}"))
    }

    #[test]
    fn composite_index_shares_the_unique_rules_and_can_be_partial() {
        check(
            "@index(active, nextRun)\n@index(kind, createdAt) where active == true\n@unique(kind, slug)\n type Task = { id: Int, active: Bool, nextRun: Int, kind: String, createdAt: Int, slug: String }\n db { tasks: Task[] }",
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let msg = check("@index(active)\n type T = { id: Int, active: Bool }").expect_err("tenía que fallar");
        assert!(msg.contains("'@index(...)' a nivel de type necesita al menos 2 campos"), "{msg}");
        let msg = check("@index(active, nope)\n type T = { id: Int, active: Bool }").expect_err("tenía que fallar");
        assert!(msg.contains("'@index(...)' nombra 'nope'"), "{msg}");
        let msg = check("@unique(a, b)\n@index(a, b)\n type T = { id: Int, a: Int, b: Int }").expect_err("tenía que fallar");
        assert!(msg.contains("redundantes") && msg.contains("un UNIQUE ya indexa"), "{msg}");
    }
}
