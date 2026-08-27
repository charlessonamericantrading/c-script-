// Árbol de sintaxis abstracta — refleja 1:1 las producciones de GRAMMAR.md §2.
//
// Cada `Expr`/`Stmt` viaja envuelto en `Spanned<T>` (definido debajo), y una
// decena de tipos de declaración (`TypeDecl`, `FnDecl`, `RpcDecl`, `Block`,
// `MatchArm`, etc.) cargan además un campo `span: Span` plano -- lo que hace
// falta para que un error del type checker señale una posición real del
// código fuente, no solo los de sintaxis (que ya podían antes, vía el span
// de cada `Token`). `Pattern`/`TypeExpr`/`Param`/`Field`/`Variant` quedan
// deliberadamente SIN span propio por ahora.

use crate::token::Span;

/// Envoltorio genérico que asocia un `Span` a un nodo del AST. La igualdad
/// (implementada A MANO, no derivada) ignora el span a propósito: dos
/// expresiones son "la misma" si representan el mismo árbol, sin importar de
/// dónde del archivo vinieron. Esto es lo que deja a los sitios que ya
/// comparan `Expr`/`Stmt` a mano (tests) sin cambios de comportamiento, y es
/// lo correcto de cara a un LSP real futuro (reanálisis incremental: "¿esta
/// declaración cambió?").
#[derive(Debug, Clone)]
pub struct Spanned<T> {
    pub node: T,
    pub span: Span,
}

impl<T: PartialEq> PartialEq for Spanned<T> {
    fn eq(&self, other: &Self) -> bool {
        self.node == other.node
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Import(ImportDecl),
    Type(TypeDecl),
    Enum(EnumDecl),
    Service(ServiceDecl),
    Const(ConstDecl),
    Fn(FnDecl),
    Db(DbDecl),
    Test(TestDecl),
}

/// Bloque de test integrado `test "nombre" { ... }` (PLAN.md §5, Eje 2)
#[derive(Debug, Clone, PartialEq)]
pub struct TestDecl {
    pub name: String,
    pub name_span: Option<Span>,
    pub body: Block,
    pub span: Span,
}

/// `db { users: User[], posts: Post[] }` (GRAMMAR.md §2.1) -- "DB tipada"
/// v0. `db` NO es palabra reservada (así una variable/campo llamado `db`
/// sigue siendo válido en cualquier otro lado) -- se reconoce por texto
/// ("db" seguido de `{`) solo en posición de ítem de nivel superior, igual
/// que un contextual keyword. Reusa `field_list`/`type_expr` tal cual --
/// sin gramática nueva para el tipo de cada colección, el checker exige
/// que cada uno resuelva a un struct con un campo `id: Int` (no la
/// gramática, ver checker.rs).
#[derive(Debug, Clone)]
pub struct DbDecl {
    pub collections: Vec<Field>,
    pub span: Span,
}

impl PartialEq for DbDecl {
    fn eq(&self, other: &Self) -> bool {
        self.collections == other.collections
    }
}

#[derive(Debug, Clone)]
pub struct ImportDecl {
    /// Los nombres entre llaves de `import { A, B } from "./x.link";`.
    /// **VACÍO** para la forma "solo por efecto" (`import "./x.link";`,
    /// GRAMMAR.md §3.161) -- cargar el módulo por lo que APORTA al programa
    /// (típicamente un `service`, el único ítem que no se puede nombrar en
    /// un import) en vez de por un nombre puntual que este archivo use.
    /// `modules.rs` valida "¿existe este nombre en ese archivo?" por cada
    /// elemento, así que una lista vacía naturalmente no valida nada -- el
    /// archivo igual se carga y sus ítems se fusionan, que es todo el punto.
    pub names: Vec<String>,
    pub from: String,
    pub span: Span,
}

impl PartialEq for ImportDecl {
    fn eq(&self, other: &Self) -> bool {
        self.names == other.names && self.from == other.from
    }
}

#[derive(Debug, Clone)]
pub struct TypeDecl {
    pub name: String,
    pub type_params: Vec<String>,
    pub ty: TypeExpr,
    /// `@unique(campo1, campo2, ...)` antes de `type` (GRAMMAR.md §3.155) --
    /// vacío para la inmensa mayoría de los `type`, que no declaran ningún
    /// constraint compuesto. Enum APARTE de `FieldAnnotation`/`Annotation`
    /// (mismo criterio que esos dos: cada punto de anclaje tiene su propio
    /// enum chico, en vez de reusar uno más grande que obligaría a rechazar
    /// en el checker combinaciones que el parser ya podría haber
    /// descartado por forma).
    pub annotations: Vec<TypeAnnotation>,
    pub span: Span,
}

impl PartialEq for TypeDecl {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.type_params == other.type_params
            && self.ty == other.ty
            && self.annotations == other.annotations
    }
}

/// Anotaciones que un `type` (no un campo suelto) admite -- ver la doc de
/// `TypeDecl::annotations`.
#[derive(Debug, Clone, PartialEq)]
pub enum TypeAnnotation {
    /// `@unique(campo1, campo2, ...)` (GRAMMAR.md §3.155): constraint
    /// UNIQUE COMPUESTO sobre varios campos a la vez -- complementa, nunca
    /// reemplaza, el `@unique` de un solo campo ya existente
    /// (`FieldAnnotation::Index { unique: true }`, §3.80). Al menos 2
    /// nombres -- un solo campo ya tiene su propia forma, más simple,
    /// arriba (el checker rechaza menos de 2). Identificadores sueltos,
    /// mismo criterio sintáctico que `Annotation::Invalidates`.
    Unique(Vec<String>),
}

#[derive(Debug, Clone)]
pub struct EnumDecl {
    pub name: String,
    pub type_params: Vec<String>,
    pub variants: Vec<Variant>,
    pub span: Span,
}

impl PartialEq for EnumDecl {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.type_params == other.type_params && self.variants == other.variants
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    pub name: String,
    /// `None` = variante unitaria (`Admin`). `Some(vec![])` es sintácticamente
    /// posible (`Foo {}`) aunque poco útil; se acepta sin caso especial.
    pub fields: Option<Vec<Field>>,
}

/// `name_span` cubre SOLO el identificador del nombre del campo (GRAMMAR.md
/// §3.22, cierra el límite que §3.21 dejó documentado) -- mismo criterio
/// que `TypeExpr::Named`'s propio `Span`: es lo que le permite al LSP
/// distinguir "el cursor está sobre el NOMBRE de un campo" de "está sobre
/// un USO de tipo", que antes eran indistinguibles y hacían que un campo
/// homónimo a un `type`/`enum` existente (`type Point = {...}; type Shape =
/// { Point: Int }`) saltara mal al pedir goto-def sobre el nombre de campo.
/// `PartialEq` es manual (no derive) para IGNORAR `name_span`, `annotations`
/// y `default`, mismo motivo que `TypeExpr` ya no deriva `PartialEq`: dos
/// `Field` estructuralmente iguales en offsets distintos (o con distinto
/// texto de `@deprecated`, o un default distinto pero del mismo tipo, todo
/// puramente informativo/de conveniencia) deben seguir siendo `==`
/// (lo usa, entre otros, `TypeExpr::Struct`'s propio `PartialEq` al comparar
/// `Vec<Field>`, y por lo tanto la subtipificación estructural -- ver
/// GRAMMAR.md §3.71: marcar un campo deprecado no lo saca de esa comparación).
#[derive(Debug, Clone)]
pub struct Field {
    pub name: String,
    pub optional: bool, // el `?` ANTES de `:` (x?: T) — distinto de Optional(T) en TypeExpr
    pub ty: TypeExpr,
    pub name_span: Span,
    /// `@deprecated(...)`/`@validate(...)` antes del campo, si hay
    /// (GRAMMAR.md §3.71 y §3.73). A diferencia de `RpcDecl.annotations`, un
    /// campo solo admite ESTAS dos -- no tiene sentido `@authenticated`/
    /// `@route`/etc. sobre un campo de struct, así que en vez de reusar
    /// `Vec<Annotation>` (que obligaría a validar en el checker qué
    /// variantes son válidas acá) el parser directamente solo sabe parsear
    /// `@deprecated`/`@validate` en esta posición y rechaza cualquier otro
    /// nombre ahí mismo (ver `parse_field` en parser.rs).
    pub annotations: Vec<FieldAnnotation>,
    /// `= expr` después del tipo, si hay (GRAMMAR.md §3.74) -- mismo lugar
    /// y mismo mecanismo que `Param::default` (parámetros de función/rpc,
    /// §2.2), no una anotación (`@algo(...)`) como `@deprecated`/`@validate`:
    /// es sintaxis del propio campo, `nombre: Tipo = valor`, exactamente
    /// igual que un parámetro. Un campo CON default puede omitirse de un
    /// literal `Struct { ... }` igual que uno `?:` -- ver
    /// `Checker::check_fields_against` -- aunque a diferencia de `x?: T` el
    /// tipo del campo NO cambia a `Optional`, sigue siendo el declarado.
    pub default: Option<Spanned<Expr>>,
}

impl Field {
    /// El motivo declarado con `@deprecated("...")`, si hay (GRAMMAR.md §3.71).
    pub fn deprecated(&self) -> Option<&str> {
        self.annotations.iter().find_map(|a| match a {
            FieldAnnotation::Deprecated(reason) => Some(reason.as_str()),
            _ => None,
        })
    }

    /// El validador declarado con `@validate(...)`, si hay (GRAMMAR.md §3.73).
    /// A lo sumo uno por campo -- el parser ya lo exige (ver `parse_field`).
    pub fn validator(&self) -> Option<&FieldValidator> {
        self.annotations.iter().find_map(|a| match a {
            FieldAnnotation::Validate(v) => Some(v),
            _ => None,
        })
    }

    /// ¿Lleva `@autoUpdate`? (GRAMMAR.md §3.77) -- un campo `Timestamp` así
    /// marcado se pisa a `now()` en CADA `applyPatch`/`upsert`-actualización
    /// sobre la fila, sin importar qué traiga el patch para ese campo.
    pub fn auto_update(&self) -> bool {
        self.annotations.iter().any(|a| matches!(a, FieldAnnotation::AutoUpdate))
    }

    /// ¿Lleva `@softDelete`? (GRAMMAR.md §3.78) -- un campo `Timestamp?` así
    /// marcado convierte `delete` en un `UPDATE` que lo fija a `now()` en
    /// vez de borrar la fila, y toda lectura (`all`/`find`/`page`/etc.)
    /// filtra automáticamente las filas donde no es `null`.
    pub fn soft_delete(&self) -> bool {
        self.annotations.iter().any(|a| matches!(a, FieldAnnotation::SoftDelete))
    }

    /// ¿Lleva `@index` o `@unique`? (GRAMMAR.md §3.80) -- `true` para
    /// `unique` en el segundo elemento si es `@unique`, `false` para
    /// `@index` plano. A lo sumo uno de los dos por campo -- el parser ya
    /// lo exige (ver `parse_field_annotations`).
    pub fn index(&self) -> Option<bool> {
        self.annotations.iter().find_map(|a| match a {
            FieldAnnotation::Index { unique } => Some(*unique),
            _ => None,
        })
    }

    /// El constraint declarado con `@check(...)`, si hay (GRAMMAR.md §3.96).
    /// A lo sumo uno por campo -- el parser ya lo exige (ver
    /// `parse_field_annotations`).
    pub fn check(&self) -> Option<&FieldCheck> {
        self.annotations.iter().find_map(|a| match a {
            FieldAnnotation::Check(c) => Some(c),
            _ => None,
        })
    }
}

impl PartialEq for Field {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.optional == other.optional && self.ty == other.ty
    }
}

/// Anotaciones que un campo de `struct` admite -- deliberadamente un enum
/// APARTE de `Annotation` (el de `RpcDecl`), no un subconjunto reusado: un
/// campo nunca necesita `@authenticated`/`@route`/`@rate_limit`/etc., así
/// que un enum propio, más chico, evita que el checker tenga que rechazar
/// en runtime combinaciones que el parser ya podría haber descartado por
/// forma (ver `parse_field` en parser.rs).
#[derive(Debug, Clone, PartialEq)]
pub enum FieldAnnotation {
    /// `@deprecated("usa X en su lugar")` -- ver GRAMMAR.md §3.71.
    Deprecated(String),
    /// `@validate(email)` o `@validate(regex, "...")` -- ver GRAMMAR.md §3.73.
    Validate(FieldValidator),
    /// `@autoUpdate` (sin paréntesis, ver `parse_field_annotations`) -- solo
    /// sobre un campo `Timestamp`. Ver GRAMMAR.md §3.77.
    AutoUpdate,
    /// `@softDelete` (sin paréntesis) -- solo sobre un campo `Timestamp?`.
    /// Ver GRAMMAR.md §3.78.
    SoftDelete,
    /// `@index` (`unique: false`) o `@unique` (`unique: true`), sin
    /// paréntesis -- índice de un solo campo (GRAMMAR.md §3.80). Un
    /// índice/constraint COMPUESTO (de varios campos) queda afuera de esta
    /// ronda -- necesitaría una anotación a nivel de `type`, no de campo,
    /// que hoy no existe (`TypeDecl` no tiene `annotations`).
    Index { unique: bool },
    /// `@check(min, N)`/`@check(max, N)`/`@check(range, N, M)` -- GRAMMAR.md
    /// §3.96, restricción numérica de nivel de BASE (no solo del lado
    /// aplicación) sobre un campo `Int`/`Int64`/`Float` (requerido u
    /// opcional). Solo el caso de rango numérico simple -- ver "Límites
    /// honestos" de §3.96 para lo que un `@check` de otros motores (una
    /// expresión booleana arbitraria, comparar dos campos entre sí) todavía
    /// no cubre.
    Check(FieldCheck),
}

/// Las tres formas de `@check(...)` (GRAMMAR.md §3.96) -- mismo criterio de
/// "kind + argumento(s)" que `FieldValidator`, ampliable sin romper esta
/// forma. Los límites se guardan como `f64` sin importar si el campo es
/// `Int`/`Int64` o `Float` -- comparar un valor entero contra un límite de
/// punto flotante es exacto para cualquier magnitud realista (un `Int64`
/// gigantesco que se saliera del rango exacto de `f64` de todos modos no
/// tendría sentido como límite humano de un `@check`).
#[derive(Debug, Clone, PartialEq)]
pub enum FieldCheck {
    /// `@check(min, N)` -- el valor tiene que ser `>= N`, sin techo.
    Min(f64),
    /// `@check(max, N)` -- el valor tiene que ser `<= N`, sin piso.
    Max(f64),
    /// `@check(range, N, M)` -- el valor tiene que estar en `[N, M]`
    /// (los dos límites inclusive). `N` tiene que ser `<= M` -- validado en
    /// el checker (GRAMMAR.md §3.96), no acá: el parser no resuelve tipos,
    /// así que no puede decidir todavía si esto es un error real.
    Range(f64, f64),
    /// `@check(minLength, N)` (GRAMMAR.md §3.146) -- sobre `String`/
    /// `String?`, no numérico: la CANTIDAD de caracteres tiene que ser
    /// `>= N`. `@check(minLength, 1)` es la forma de expresar "no vacío".
    /// `f64` por el mismo motivo que `Min`/`Max`/`Range` -- el checker
    /// valida que sea un entero no negativo, acá el parser solo guarda el
    /// número tal cual se escribió.
    MinLength(f64),
    /// `@check(maxLength, N)` -- la cantidad de caracteres tiene que ser
    /// `<= N`.
    MaxLength(f64),
}

/// Las dos formas de `@validate(...)` que un campo `String`/`String?` admite
/// (GRAMMAR.md §3.73). Ampliable a futuro (`@validate(minLength, N)`, etc.)
/// sin romper esta forma -- cada variante nueva es un nombre nuevo dentro
/// del mismo paréntesis, no una anotación nueva.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldValidator {
    /// `@validate(email)` -- forma general de dirección de email (no RFC
    /// 5322 completo, ver GRAMMAR.md §3.73 "Límites honestos").
    Email,
    /// `@validate(regex, "^[A-Z]{3}$")` -- el patrón tal cual, sin parsear
    /// (el parser solo lo guarda como string; el checker lo compila con la
    /// crate `regex` para dar el error de sintaxis en compilación, no en el
    /// primer request real).
    Regex(String),
}

#[derive(Debug, Clone)]
pub struct ConstDecl {
    pub name: String,
    pub ty: TypeExpr,
    pub value: Spanned<Expr>,
    pub span: Span,
}

impl PartialEq for ConstDecl {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.ty == other.ty && self.value == other.value
    }
}

#[derive(Debug, Clone)]
pub struct ServiceDecl {
    pub name: String,
    pub members: Vec<Member>,
    pub span: Span,
}

impl PartialEq for ServiceDecl {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.members == other.members
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Member {
    Rpc(RpcDecl),
    Stream(RpcDecl),
}

/// El span cubre la FIRMA (desde el nombre hasta el return type, incluyendo
/// la `@annotation` si hay una -- ver `parse_member`/`parse_rpc_like` en
/// parser.rs), NO el cuerpo: `body` (un `Block`) ya carga su propio span
/// preciso por statement/expresión, así que un error DENTRO del cuerpo nunca
/// necesita caer de vuelta al span de todo el rpc.
#[derive(Debug, Clone)]
pub struct RpcDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: TypeExpr,
    pub body: Block,
    pub annotations: Vec<Annotation>,
    /// El docstring `///` que precede al rpc/stream (incluyendo cualquier
    /// `@annotation` en el medio), si hay (GRAMMAR.md §3.72) -- se propaga
    /// como `description` a `openapi.json`. Ignorado en `PartialEq`, mismo
    /// criterio que `Field::deprecated` (ast.rs): es metadata puramente
    /// informativa, no cambia qué declara el rpc.
    pub doc: Option<String>,
    pub span: Span,
}

impl PartialEq for RpcDecl {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.params == other.params
            && self.return_type == other.return_type
            && self.body == other.body
            && self.annotations == other.annotations
    }
}

impl RpcDecl {
    /// La anotación de auth, si hay (GRAMMAR.md §3.14). Sigue siendo a lo sumo
    /// UNA anotación de auth por rpc -- pero desde §3.49, `@requires` en sí
    /// puede nombrar varios roles con `|` (`Role.Admin | Role.Agent`), así
    /// que "un solo rol por endpoint" ya no es una restricción real. Lo que
    /// el checker permite combinar es auth con `@content_type`, que es una
    /// dimensión distinta (§3.35).
    pub fn auth(&self) -> Option<&Annotation> {
        self.annotations
            .iter()
            .find(|a| matches!(a, Annotation::Authenticated | Annotation::Requires { .. }))
    }

    /// El Content-Type declarado con `@content_type("...")`, si hay. Cuando
    /// está, la respuesta es el `String` que devuelve el rpc tal cual, no un
    /// JSON (GRAMMAR.md §3.35).
    pub fn content_type(&self) -> Option<&str> {
        self.annotations.iter().find_map(|a| match a {
            Annotation::ContentType(ct) => Some(ct.as_str()),
            _ => None,
        })
    }

    /// El patrón de ruta declarado con `@route("/blog/:slug")`, si hay --
    /// texto crudo, sin parsear (GRAMMAR.md §3.37). El checker es quien
    /// valida la forma y arma el binding contra los parámetros del rpc;
    /// acá es solo el string tal como se escribió.
    pub fn route(&self) -> Option<&str> {
        self.annotations.iter().find_map(|a| match a {
            Annotation::Route(pattern) => Some(pattern.as_str()),
            _ => None,
        })
    }

    /// `(spec, key_param)` de `@rate_limit("20/1m")` o `@rate_limit("20/1m",
    /// key: email)`, si hay -- `spec` es texto crudo, sin parsear (GRAMMAR.md
    /// §3.39); `key_param` es el nombre de un parámetro adicional que se
    /// combina con la IP del cliente para la clave del bucket (§3.142), o
    /// `None` si el rate limit es solo-IP (comportamiento de siempre). El
    /// checker valida el formato de `spec` y que `key_param`, si está,
    /// nombre un parámetro real de tipo `String`/`Int`.
    pub fn rate_limit(&self) -> Option<(&str, Option<&str>)> {
        self.annotations.iter().find_map(|a| match a {
            Annotation::RateLimit { spec, key_param } => Some((spec.as_str(), key_param.as_deref())),
            _ => None,
        })
    }

    /// El motivo declarado con `@deprecated("...")`, si hay (GRAMMAR.md
    /// §3.71).
    pub fn deprecated(&self) -> Option<&str> {
        self.annotations.iter().find_map(|a| match a {
            Annotation::Deprecated(reason) => Some(reason.as_str()),
            _ => None,
        })
    }

    /// El valor declarado con `@cache_control("...")`, si hay -- texto
    /// crudo, sin parsear (GRAMMAR.md §3.113). Mismo criterio que
    /// `rate_limit()`: el checker valida que no esté vacío, nunca la
    /// gramática interna de `Cache-Control` (`public`, `max-age=N`, etc. --
    /// eso es responsabilidad de HTTP, no de c-script).
    pub fn cache_control(&self) -> Option<&str> {
        self.annotations.iter().find_map(|a| match a {
            Annotation::CacheControl(value) => Some(value.as_str()),
            _ => None,
        })
    }

    /// El `@example(request: ..., response: ...)`, si hay (GRAMMAR.md
    /// §3.119) -- las dos mitades son independientes, `None` para la que no
    /// se declaró.
    pub fn example(&self) -> Option<(Option<&Spanned<Expr>>, Option<&Spanned<Expr>>)> {
        self.annotations.iter().find_map(|a| match a {
            Annotation::Example { request, response } => Some((request.as_deref(), response.as_deref())),
            _ => None,
        })
    }

    /// Nombres de `@invalidates(rpc1, rpc2, ...)`, si hay (GRAMMAR.md
    /// §3.125) -- rpcs de la MISMA `service` cuyo cache de Query se limpia
    /// después de que este rpc (un Mutation) tiene éxito.
    pub fn invalidates(&self) -> Option<&[String]> {
        self.annotations.iter().find_map(|a| match a {
            Annotation::Invalidates(names) => Some(names.as_slice()),
            _ => None,
        })
    }

    /// `(cursor_param, limit_param)` de `@infinite(cursor, limit)`, si hay
    /// (GRAMMAR.md §3.134).
    pub fn infinite(&self) -> Option<(&str, &str)> {
        self.annotations.iter().find_map(|a| match a {
            Annotation::Infinite { cursor_param, limit_param } => Some((cursor_param.as_str(), limit_param.as_str())),
            _ => None,
        })
    }

    /// `true` si este rpc declaró `@idempotent` (GRAMMAR.md §3.140) -- el
    /// server (`runtime::idempotency`) recuerda el resultado de la primera
    /// ejecución exitosa por `Idempotency-Key` y lo repite en un reintento
    /// con la misma clave, en vez de correr el cuerpo de nuevo.
    pub fn idempotent(&self) -> bool {
        self.annotations.iter().any(|a| matches!(a, Annotation::Idempotent))
    }

    /// La duración cruda de `@cache("60s")`, si hay (GRAMMAR.md §3.144) --
    /// texto sin parsear, mismo criterio que `rate_limit()`/`cache_control()`:
    /// el checker valida el formato, acá es solo el string tal como se
    /// escribió.
    pub fn cache(&self) -> Option<&str> {
        self.annotations.iter().find_map(|a| match a {
            Annotation::Cache(ttl) => Some(ttl.as_str()),
            _ => None,
        })
    }

    /// El valor crudo de `@cors("...")`, si hay (GRAMMAR.md §3.147) --
    /// texto sin parsear, mismo criterio que `rate_limit()`/`cache()`.
    pub fn cors(&self) -> Option<&str> {
        self.annotations.iter().find_map(|a| match a {
            Annotation::Cors(v) => Some(v.as_str()),
            _ => None,
        })
    }

    /// El valor crudo de `@cron("5m")`, si hay (GRAMMAR.md §3.159) -- texto
    /// sin parsear, mismo criterio que `cache()`. El checker ya garantizó
    /// que si esto es `Some`, es la ÚNICA anotación del rpc.
    pub fn cron(&self) -> Option<&str> {
        self.annotations.iter().find_map(|a| match a {
            Annotation::Cron(v) => Some(v.as_str()),
            _ => None,
        })
    }

    /// Mismo heurístico "nombre por forma" en UN solo lugar -- lo usan
    /// `codegen::ts_emit::emit_hooks` (para decidir si un rpc genera un
    /// hook `use...Query`) Y `checker::check_invalidates_annotation` (para
    /// validar que `@invalidates` solo nombre rpcs que de verdad tienen una
    /// entrada de cache que invalidar, GRAMMAR.md §3.125) -- duplicarlo en
    /// los dos lugares habría sido exactamente la clase de divergencia
    /// entre dos copias del mismo código que este proyecto evita desde
    /// GRAMMAR.md §3.9. Un rpc sin parámetros también cuenta como Query, no
    /// hay forma más segura de mutar sin nada que pasarle.
    pub fn looks_like_a_query(&self) -> bool {
        self.name.starts_with("get")
            || self.name.starts_with("list")
            || self.name.starts_with("find")
            || self.name.starts_with("search")
            || self.name.starts_with("read")
            || self.name.starts_with("fetch")
            || self.params.is_empty()
    }
}

/// Anotaciones de un rpc/stream. Se permiten varias, pero no cualquier
/// combinación: el checker rechaza dos de auth, dos de `@content_type`, dos
/// de `@route`, y tanto `@content_type` como `@route` sobre un `stream`.
#[derive(Debug, Clone, PartialEq)]
pub enum Annotation {
    Authenticated,
    /// `@requires(Role.Admin)` o, desde §3.49, `@requires(Role.Admin |
    /// Role.Agent)` -- `variant_names` tiene siempre al menos 1 elemento
    /// (el parser no acepta paréntesis vacíos), y todos vienen del MISMO
    /// `enum_name` (el parser ya lo exige -- mezclar dos enums distintos en
    /// un solo `@requires` no tendría significado: una sesión tiene un rol
    /// de UN enum, nunca de dos a la vez).
    Requires { enum_name: String, variant_names: Vec<String> },
    /// `@content_type("text/html; charset=utf-8")` -- ver GRAMMAR.md §3.35.
    ContentType(String),
    /// `@route("/blog/:slug")` -- URL alternativa, amigable para crawlers,
    /// que convive con el `/Servicio/rpc` de siempre (nunca lo reemplaza).
    /// Ver GRAMMAR.md §3.37.
    Route(String),
    /// `@rate_limit("20/1m")` -- como mucho N requests por ventana de
    /// tiempo, por (ip del cliente, servicio, rpc). Ver GRAMMAR.md §3.39.
    /// Con `key: <param>` (GRAMMAR.md §3.142) -- `@rate_limit("5/1m", key:
    /// email)` -- la clave del bucket combina la IP CON el valor de ese
    /// parámetro, para el caso real "limitar por IP+email, no solo IP" (un
    /// abuso que rota de IP reusando el mismo email evade un límite
    /// solo-IP).
    RateLimit { spec: String, key_param: Option<String> },
    /// `@deprecated("usa X en su lugar")` sobre un rpc/stream -- el texto se
    /// propaga tal cual como comentario JSDoc `@deprecated` sobre el método
    /// correspondiente en el `.d.ts` generado (GRAMMAR.md §3.71). No cambia
    /// nada en runtime: sigue funcionando igual, es puramente informativo
    /// para quien consume el contrato generado.
    Deprecated(String),
    /// `@cache_control("public, max-age=3600")` -- header `Cache-Control`
    /// declarativo en la respuesta de ÉXITO de un rpc normal (nunca un
    /// `stream`, GRAMMAR.md §3.113). Texto crudo, tal cual el valor de
    /// HTTP: c-script no valida su gramática interna.
    CacheControl(String),
    /// `@example(request: <expr>, response: <expr>)` (GRAMMAR.md §3.119) --
    /// a diferencia del resto de las anotaciones, sus valores son
    /// EXPRESIONES de c-script (típicamente un `StructLit`), no `String`
    /// crudo: el checker las tipa contra la forma real del rpc (`request`
    /// contra sus parámetros, `response` contra su `return_type`), así que
    /// un ejemplo desincronizado del contrato es un error de compilación,
    /// no un blob de JSON que puede mentir en silencio. Restringidas a
    /// expresiones LITERALES (`is_literal_expr`, checker.rs) -- un ejemplo
    /// es un valor fijo, no algo recalculado en cada build. Al menos una de
    /// las dos siempre está presente (`@example()` vacío es un error del
    /// parser); ambas son independientes -- un rpc sin parámetros solo
    /// puede declarar `response`.
    Example { request: Option<Box<Spanned<Expr>>>, response: Option<Box<Spanned<Expr>>> },
    /// `@invalidates(list, search)` (GRAMMAR.md §3.125) -- nombres de rpcs
    /// de la MISMA `service` (identificadores sueltos, no `Enum.Variante`
    /// como `@requires`) cuyo cache de Query se limpia en el frontend
    /// generado después de que ESTE rpc (siempre un Mutation en la
    /// práctica) resuelve con éxito. El checker valida que cada nombre sea
    /// un rpc real del mismo service que además genere un hook de Query
    /// (`RpcDecl::looks_like_a_query`) -- nombrar algo que no tiene cache
    /// de Query no invalidaría nada.
    Invalidates(Vec<String>),
    /// `@infinite(cursor, limit)` (GRAMMAR.md §3.134) -- nombra los DOS
    /// parámetros de ESTE rpc que juegan el rol de cursor de continuación y
    /// tamaño de página, para generar un hook de scroll infinito
    /// (`use{Servicio}{Rpc}Infinite`) en vez del hook de Query normal. Los
    /// dos son identificadores sueltos (como `@invalidates`, no
    /// `Enum.Variante`). El checker exige que `cursor` sea un parámetro
    /// real de tipo `Int?` y `limit` uno de tipo `Int` -- mismas firmas que
    /// `db.<c>.pageAfter(cursor: Int?, limit: Int)` (§3.61), el único
    /// mecanismo de paginación por cursor que el lenguaje ya tiene -- y que
    /// el retorno sea `T[]` con `T` teniendo un campo `id: Int` (el cursor
    /// siguiente es el `id` del último elemento de la página, mismo
    /// criterio que `pageAfter` usa puertas adentro).
    Infinite { cursor_param: String, limit_param: String },
    /// `@idempotent` (GRAMMAR.md §3.140) -- sin argumentos, como
    /// `@authenticated`. Marca un rpc como elegible para deduplicación por
    /// `Idempotency-Key`: si el caller manda ese header, el servidor
    /// recuerda el resultado de la primera ejecución exitosa y lo repite en
    /// un reintento con la MISMA clave, sin volver a correr el cuerpo. Un
    /// caller que no manda el header no ve ningún cambio de comportamiento
    /// -- la deduplicación es opt-in por REQUEST, no forzada por el rpc.
    Idempotent,
    /// `@cache("60s")` (GRAMMAR.md §3.144) -- cachea el resultado de una
    /// ejecución EXITOSA en el servidor, keyeado por (service, rpc,
    /// argumentos), por la duración dada (`Ns`/`Nm`/`Nh`/`Nd`, mismo formato
    /// que `--session-ttl`). Para lecturas costosas y poco cambiantes --
    /// dimensión ORTOGONAL a `@cache_control` (que solo le dice al CLIENTE
    /// cuánto puede cachear; esto cachea del lado del SERVIDOR, ahorrando la
    /// ejecución real del cuerpo). Alcance v0: sin invalidación cruzada con
    /// `@invalidates` (esa es una cache de CLIENTE separada) -- una entrada
    /// expira sola por tiempo, nunca antes.
    Cache(String),
    /// `@cors("https://a.com, https://b.com")` o `@cors("*")` (GRAMMAR.md
    /// §3.147) -- override de CORS por rpc/stream, mismo formato
    /// separado-por-comas que `LINK_CORS_ORIGINS` (main.rs). Reemplaza
    /// ENTERO al `--cors-origin`/`LINK_CORS_ORIGINS` global para ESTE
    /// endpoint puntual (nunca lo combina) -- el caso real: la API entera
    /// detrás de un allowlist, salvo un endpoint público (un widget, un
    /// sitemap) que necesita otro origen o `*`.
    Cors(String),
    /// `@cron("5m")` (GRAMMAR.md §3.159) -- tarea recurrente nativa dentro
    /// de `linkc serve`: el rpc corre solo, cada `Ns`/`Nm`/`Nh`/`Nd`
    /// (mismo formato que `@cache`/`--session-ttl`), en su propio hilo,
    /// nunca alcanzable vía HTTP. El checker exige que sea la ÚNICA
    /// anotación del rpc (sin `@route`/`@authenticated`/`@rate_limit`/etc.
    /// -- ninguna tiene sentido sobre algo que nunca recibe una request
    /// real), sin parámetros, y retorno `Void`.
    Cron(String),
}

/// `name_span`: mismo criterio y mismo motivo que `Field::name_span` (ver
/// su doc) -- distingue el NOMBRE de un parámetro de un USO de tipo para
/// goto-def (GRAMMAR.md §3.22). `PartialEq` manual por la misma razón:
/// ignora `name_span` para que dos `Param` estructuralmente iguales en
/// offsets distintos sigan siendo `==` (lo usa `FnDecl`/`RpcDecl`'s propio
/// `PartialEq` al comparar `Vec<Param>`).
#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub ty: TypeExpr,
    pub default: Option<Spanned<Expr>>,
    pub name_span: Span,
}

impl PartialEq for Param {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.ty == other.ty && self.default == other.default
    }
}

/// Mismo criterio que `RpcDecl`: el span cubre la firma, no `body`.
#[derive(Debug, Clone)]
pub struct FnDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: TypeExpr,
    pub body: Block,
    pub span: Span,
}

impl PartialEq for FnDecl {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.params == other.params
            && self.return_type == other.return_type
            && self.body == other.body
    }
}

#[derive(Debug, Clone)]
pub enum TypeExpr {
    /// `identifier [type_args]` — incluye tipos primitivos (Int, String, ...),
    /// nombres de type/enum declarados, y genéricos instanciados (Result<A,B>).
    /// El `Span` cubre solo el identificador (no los `type_args`) -- es lo
    /// que permite al LSP resolver goto-definición de un nombre de tipo
    /// escrito en una firma (GRAMMAR.md §3.21). Ninguna otra variante lleva
    /// span propio a propósito: son combinadores sintácticos sin un
    /// identificador escrito al que alguien pediría saltar (el `Int` dentro
    /// de `Int[]` ya es su propio `Named` anidado).
    Named(String, Vec<TypeExpr>, Span),
    /// `{ field_list }`
    Struct(Vec<Field>),
    /// `{ K: V }`
    Map(Box<TypeExpr>, Box<TypeExpr>),
    /// `(A, B, ...)` — requiere ≥2 elementos tras desambiguar con agrupación (§2.2)
    Tuple(Vec<TypeExpr>),
    /// `(A, B) -> C`
    Function(Vec<TypeExpr>, Box<TypeExpr>),
    /// postfix `?`
    Optional(Box<TypeExpr>),
    /// postfix `[]`
    List(Box<TypeExpr>),
    /// `A | B | C`
    Union(Vec<TypeExpr>),
}

/// A mano, no derivado: el `Span` de `Named` se ignora a propósito (mismo
/// criterio que `Spanned<T>`, arriba) -- dos `TypeExpr` que describen el
/// mismo tipo siguen siendo iguales sin importar en qué offset del archivo
/// se haya escrito cada uno.
impl PartialEq for TypeExpr {
    fn eq(&self, other: &Self) -> bool {
        use TypeExpr::*;
        match (self, other) {
            (Named(n1, a1, _), Named(n2, a2, _)) => n1 == n2 && a1 == a2,
            (Struct(f1), Struct(f2)) => f1 == f2,
            (Map(k1, v1), Map(k2, v2)) => k1 == k2 && v1 == v2,
            (Tuple(t1), Tuple(t2)) => t1 == t2,
            (Function(p1, r1), Function(p2, r2)) => p1 == p2 && r1 == r2,
            (Optional(a), Optional(b)) => a == b,
            (List(a), List(b)) => a == b,
            (Union(a), Union(b)) => a == b,
            _ => false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Block {
    pub stmts: Vec<Spanned<Stmt>>,
    pub tail: Option<Box<Spanned<Expr>>>,
    pub span: Span,
}

impl PartialEq for Block {
    fn eq(&self, other: &Self) -> bool {
        self.stmts == other.stmts && self.tail == other.tail
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Let {
        name: String,
        mutable: bool,
        ty: Option<TypeExpr>,
        value: Spanned<Expr>,
    },
    Return(Option<Spanned<Expr>>),
    Expr(Spanned<Expr>),
    /// `x = expr;` -- solo variables simples (no `obj.field = ...` ni
    /// `arr[i] = ...` todavía). El checker exige que `x` haya sido
    /// declarada con `mut` (GRAMMAR.md §2.3).
    Assign {
        name: String,
        value: Spanned<Expr>,
    },
    /// `while cond { body }` (GRAMMAR.md §3.15) -- a propósito NUNCA un
    /// `Expr`: no hay `break <valor>` en v0 (ni tipo `Never`/bottom para lo
    /// que valdría un loop que nunca hace break), así que un loop no tiene
    /// ningún valor que producir. `cond` se re-evalúa en cada iteración;
    /// `body` corre por efecto solamente -- se chequea contra `Type::Void`,
    /// igual que un `if`/`match` en posición de sentencia (checker.rs::
    /// check_stmt). Un `return` alcanzable desde `body` se RECHAZA
    /// explícitamente en v0 (reusa `block_has_return`) en vez de heredar en
    /// silencio el bug ya existente de `if`/`match` como sentencia (un
    /// `return` ahí es un no-op silencioso en runtime, no solo mal tipado --
    /// ver la nota extensa en checker.rs::check_stmt).
    While {
        cond: Spanned<Expr>,
        body: Block,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Null,
    Ident(String),
    FieldAccess {
        base: Box<Spanned<Expr>>,
        field: String,
    },
    Call {
        callee: Box<Spanned<Expr>>,
        args: Vec<Spanned<Expr>>,
    },
    /// `[e1, e2, ...]` -- vacío (`[]`) solo es válido en modo chequeo,
    /// ver checker.rs (no se puede sintetizar un elemento de la nada).
    ArrayLit(Vec<Spanned<Expr>>),
    /// `base[index]` -- postfix, ver GRAMMAR.md §2.3.
    Index {
        base: Box<Spanned<Expr>>,
        index: Box<Spanned<Expr>>,
    },
    /// `(e1, e2, ...)` -- distinto de `Paren` (agrupación) por la misma
    /// regla de la coma obligatoria que ya usa el nivel de tipos (§2.2).
    TupleLit(Vec<Spanned<Expr>>),
    /// `base.0`, `base.1`, ... -- acceso posicional. Un solo nivel: `t.0.1`
    /// NO encadena (ver nota del lexer en GRAMMAR.md §2.3), es una
    /// limitación conocida, no un error silencioso.
    TupleIndex {
        base: Box<Spanned<Expr>>,
        index: usize,
    },
    /// `Nombre { campos }` o `Enum.Variante { campos }` (GRAMMAR.md §2.3 struct_or_variant_lit).
    StructLit {
        name: String,
        variant: Option<String>,
        fields: Vec<(String, Spanned<Expr>)>,
    },
    Match {
        scrutinee: Box<Spanned<Expr>>,
        arms: Vec<MatchArm>,
    },
    If {
        cond: Box<Spanned<Expr>>,
        then_block: Block,
        else_block: Block,
    },
    Binary {
        op: BinaryOp,
        left: Box<Spanned<Expr>>,
        right: Box<Spanned<Expr>>,
    },
    Unary {
        op: UnaryOp,
        operand: Box<Spanned<Expr>>,
    },
    Paren(Box<Spanned<Expr>>),
    /// `|params| { block }` (GRAMMAR.md §3.10) -- SIEMPRE un bloque con
    /// llaves, nunca una expresión suelta (no existe "bloque como
    /// expresión general" en el lenguaje; esto reusa `Block` tal cual, sin
    /// inventar ese concepto). Sin params implica `||`, que lexea como un
    /// solo token `PipePipe` distinto de `Pipe` -- alcance v0 deliberado:
    /// closures de 0 parámetros no tienen consumidor real todavía
    /// (`.map`/`.filter` siempre pasan 1 argumento), así que no se soportan.
    Closure {
        params: Vec<ClosureParam>,
        body: Block,
    },
    /// `transaction { ... }` (GRAMMAR.md §3.154) -- una expresión de
    /// bloque, misma forma que el cuerpo de un `if`/`match`: retorna el
    /// valor de la última sentencia, de modo CHEQUEO nada más (no se puede
    /// sintetizar sin contexto, mismo motivo que `Expr::If`/`Expr::Match`
    /// en checker.rs). Envuelve TODAS las escrituras a `db` adentro en una
    /// transacción SQL real -- `COMMIT` si el bloque termina de correr
    /// normal, `ROLLBACK` si cualquier error de runtime se propaga desde
    /// adentro (`panic`, una violación de `@check`/`@unique`, etc.). No se
    /// puede anidar, y no admite `return` en su cuerpo (mismo límite que
    /// `while`, GRAMMAR.md §3.15) -- las dos reglas se verifican en
    /// checker.rs, no acá.
    Transaction(Block),
}

/// `nombre` o `nombre: Tipo` dentro de `|...|`. Sin anotación solo es válido
/// cuando el closure se chequea (⇐) contra un `Type::Function` ya conocido
/// (ej. el callback de `.filter`/`.map`) -- `checker.rs::synth_expr` exige
/// que TODOS estén anotados si el closure se sintetiza sin ese contexto.
#[derive(Debug, Clone, PartialEq)]
pub struct ClosureParam {
    pub name: String,
    pub ty: Option<TypeExpr>,
}

/// GRAMMAR.md §3.7 — sin coerción implícita entre variantes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    And,
    Or,
    /// `a ?? b` (GRAMMAR.md §3.9): si `a` (un `T?`) no es `null`, el valor
    /// desenvuelto; si no, `b` (de tipo `T`). Azúcar sobre exactamente el
    /// `match` de narrowing de arriba -- existe aparte porque escribir
    /// `match x { v: Item => v, null => default }` para el caso más común
    /// ("dame un default") es ceremonia real que no aporta nada.
    Coalesce,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pattern: Pattern,
    /// `pattern if guard => body` (GRAMMAR.md §3.3). Un arm con guard NUNCA
    /// descarta exhaustividad por sí solo -- la condición podría ser falsa
    /// en runtime, así que el checker lo trata como si no cubriera nada.
    pub guard: Option<Spanned<Expr>>,
    pub body: MatchArmBody,
    pub span: Span,
}

impl PartialEq for MatchArm {
    fn eq(&self, other: &Self) -> bool {
        self.pattern == other.pattern && self.guard == other.guard && self.body == other.body
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum MatchArmBody {
    Expr(Spanned<Expr>),
    Block(Block),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    /// binding simple, incluye `_` (wildcard)
    Bind(String),
    /// `1`, `"texto"`, `true`/`false`, `null` (GRAMMAR.md §3.3) --
    /// deliberadamente SIN Float (comparar floats por igualdad exacta es la
    /// misma trampa que Rust terminó prohibiendo en sus propios patrones).
    /// `null` solo es válido contra un escrutinio `T?` -- narrowing real de
    /// un opcional, GRAMMAR.md §3.9: `match x { v: Item => ..., null => ... }`.
    Literal(LiteralPattern),
    /// `Enum.Variante { campo: patrón, ... }` — la variante unitaria sin
    /// llaves (`Enum.Variante`) se representa con `fields: None`.
    Variant {
        enum_name: String,
        variant_name: String,
        fields: Option<Vec<FieldPattern>>,
    },
    /// `P1 | P2 | ...` (GRAMMAR.md §3.3). Alcance v0: ninguna alternativa
    /// puede introducir bindings -- exigir que las N alternativas liguen
    /// exactamente las mismas variables del mismo tipo (la regla real de
    /// Rust) es la parte cara de or-patterns; acá se prohíbe bindear del
    /// todo dentro de un `Or` (el checker lo rechaza explícitamente), que
    /// cubre el caso común (combinar variantes/literales con un mismo
    /// cuerpo) sin la complejidad de reconciliar bindings entre ramas.
    Or(Vec<Pattern>),
    /// `nombre: Tipo` -- narrowing de un valor de tipo unión a su miembro
    /// concreto (GRAMMAR.md §3.9), ej. `i: Int` o `u: User`. Reusa el `:`
    /// que ya significa "nombre tiene este tipo declarado" en todos lados
    /// (let, params, campos de struct) en vez de inventar puntuación nueva
    /// (`is`/`as`). Nombre primero, tipo segundo -- mismo orden que
    /// `Param`/`Field`/`FieldPattern`. El tipo se parsea como un tipo
    /// POSTFIJO (parser.rs::parse_postfix_type), nunca uno con `|` de nivel
    /// superior -- así un `|` que sigue queda para el propio or-pattern que
    /// lo rodea (`i: Int | s: String`), no se lo come la anotación.
    Type(String, TypeExpr),
}

#[derive(Debug, Clone, PartialEq)]
pub enum LiteralPattern {
    Int(i64),
    Str(String),
    Bool(bool),
    Null,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldPattern {
    pub name: String,
    /// El shorthand `x` (sin `: patrón`) se expande en el parser a
    /// `Pattern::Bind(x)`, así el resto del compilador no necesita conocer
    /// la abreviatura — ya llega desugared.
    pub pattern: Pattern,
}

/// Reconoce el ÚNICO shape de cuerpo de un `stream` que dispara push real
/// v0 (GRAMMAR.md §3.16): `while true { db.<coleccion>.subscribe() }`,
/// nada más -- ni una sentencia antes, ni un tail después (`while` nunca
/// produce uno, ver `Stmt::While` arriba, así que el cuerpo entero tiene
/// que ser esa única sentencia). Cualquier otra forma (otro método,
/// argumentos, sentencias de más, una condición que no sea el literal
/// `true`) devuelve `None` -- ese `stream` sigue el camino de siempre
/// (`List<T>` ya calculada).
///
/// Vive acá, no en checker.rs ni en runtime, para que ambos lo llamen sin
/// que ninguno dependa del otro: `checker.rs::check_rpc` lo usa para
/// decidir si valida este shape especial en vez del cuerpo normal, y
/// `runtime::live_subscribe_collection` (sibling de `is_stream_member`)
/// lo usa para que `server.rs` decida el routing ANTES de invocar
/// `invoke_rpc_with_sessions` -- ese cuerpo nunca llega a `eval_block`.
pub fn recognize_live_subscribe(body: &Block) -> Option<&str> {
    let [stmt] = body.stmts.as_slice() else { return None };
    if body.tail.is_some() {
        return None;
    }
    let Stmt::While { cond, body: loop_body } = &stmt.node else { return None };
    if !matches!(cond.node, Expr::Bool(true)) {
        return None;
    }
    if !loop_body.stmts.is_empty() {
        return None;
    }
    let Expr::Call { callee, args } = &loop_body.tail.as_ref()?.node else { return None };
    if !args.is_empty() {
        return None;
    }
    let Expr::FieldAccess { base, field } = &callee.node else { return None };
    if field != "subscribe" {
        return None;
    }
    let Expr::FieldAccess { base: db_ident, field: collection } = &base.node else { return None };
    matches!(&db_ident.node, Expr::Ident(n) if n == "db").then(|| collection.as_str())
}

/// Reconoce `|item: T| item.campo` -- el ÚNICO shape de closure que
/// `sumBy`/`countBy`/`avgBy`/`maxBy`/`minBy` (GRAMMAR.md §3.52) aceptan
/// como selector: un acceso de campo simple sobre el propio parámetro,
/// nada más -- ni una expresión derivada (`item.campo + 1`), ni una
/// llamada a método, ni acceso anidado (`item.campo.otro`), ni más de un
/// parámetro. Cualquier otra forma devuelve `None` -- esos casos no se
/// pueden traducir a una columna SQL real (no hay forma de "empujar" una
/// expresión c-script arbitraria a SQL), así que el checker los rechaza
/// con un mensaje claro en vez de intentar adivinar.
///
/// `param_names`, no `Vec<ClosureParam>`: el nombre es lo único que hace
/// falta acá, y así la misma función sirve tanto para `Expr::Closure`
/// (checker, params con anotación de tipo) como para `Value::Closure`
/// (runtime, params ya reducidos a `Vec<String>`) sin que ninguno de los
/// dos tenga que convertir su propia representación a la del otro.
pub fn recognize_field_selector<'a>(param_names: &[String], body: &'a Block) -> Option<&'a str> {
    let [param] = param_names else { return None };
    if !body.stmts.is_empty() {
        return None;
    }
    let Expr::FieldAccess { base, field } = &body.tail.as_ref()?.node else { return None };
    matches!(&base.node, Expr::Ident(n) if n == param).then(|| field.as_str())
}

/// Granularidad de truncado de fecha para el selector de AGRUPACIÓN de
/// `sumBy`/`countBy`/`avgBy`/`maxBy`/`minBy` (GRAMMAR.md §3.157) -- el
/// límite que §3.65 dejaba abierto a propósito ("sin truncado de fechas").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeGranularity {
    Day,
    Month,
    Year,
}

/// Reconoce el selector de CLAVE de agrupación: `|item: T| item.campo`
/// (como `recognize_field_selector`, sin truncar) o `|item: T|
/// item.campo.truncateToDay/Month/Year()` (GRAMMAR.md §3.157). Es la
/// ÚNICA posición de todo el lenguaje donde un método existe sobre un
/// `Timestamp` -- §3.31 sigue prohibiendo cualquier otro uso; esto NUNCA
/// evalúa el método de verdad, solo lo reconoce sintácticamente, mismo
/// espíritu que `recognize_live_subscribe` con `db.<c>.subscribe()`.
/// Nunca se usa para el selector de VALOR (`field_selector` sola alcanza
/// ahí) -- solo `check_aggregate_by`/`select_grouped` llaman a esta.
pub fn recognize_group_key_selector<'a>(param_names: &[String], body: &'a Block) -> Option<(&'a str, Option<TimeGranularity>)> {
    let [param] = param_names else { return None };
    if !body.stmts.is_empty() {
        return None;
    }
    let tail = &body.tail.as_ref()?.node;
    if let Expr::FieldAccess { base, field } = tail {
        return matches!(&base.node, Expr::Ident(n) if n == param).then(|| (field.as_str(), None));
    }
    let Expr::Call { callee, args } = tail else { return None };
    if !args.is_empty() {
        return None;
    }
    let Expr::FieldAccess { base: receiver, field: method } = &callee.node else { return None };
    let granularity = match method.as_str() {
        "truncateToDay" => TimeGranularity::Day,
        "truncateToMonth" => TimeGranularity::Month,
        "truncateToYear" => TimeGranularity::Year,
        _ => return None,
    };
    let Expr::FieldAccess { base, field } = &receiver.node else { return None };
    matches!(&base.node, Expr::Ident(n) if n == param).then(|| (field.as_str(), Some(granularity)))
}

/// El lado "valor" de una hoja de predicado (ver `recognize_predicate_expr`
/// abajo). Normalmente una expresión del código fuente sin evaluar todavía
/// (mismo criterio que la versión de un solo operador) -- pero `!item.campo`
/// se reconoce como una hoja completa (`item.campo == false`) sin que exista
/// ningún literal `false` en el código fuente al que apuntar, y lo mismo
/// para `item.campo` solo (`item.campo == true`) -- por eso esta forma
/// también admite un booleano SINTETIZADO, no tomado del AST. `Field` (§3.171)
/// es el tercer caso: el otro lado no es un valor a bindear sino OTRO campo
/// del mismo parámetro (`item.endDate > item.startDate`) -- el SQL generado
/// compara dos columnas entre sí, sin placeholder.
pub enum PredicateOperand<'a> {
    Expr(&'a Spanned<Expr>),
    Bool(bool),
    Field(&'a str),
}

/// Árbol de un predicado pusheable -- GRAMMAR.md §3.95/§3.108/§3.109
/// (una hoja, luego una conjunción `&&` de varias) y ahora también §3.170
/// (`||` combinando condiciones, en cualquier profundidad de anidamiento
/// con `&&`, respetando la precedencia real del lenguaje -- `&&` liga más
/// fuerte que `||`, mismo criterio que `parser.rs::parse_or_expr`/
/// `parse_and_expr`). `And`/`Or` nunca anidan el MISMO tipo directamente
/// (`recognize_predicate_expr` los aplana) -- una cadena `a && b && c`
/// sigue siendo un solo `And` de 3 hojas, no `And(And(a,b),c)`, así que el
/// SQL generado para el caso puro de siempre no cambia de forma.
pub enum PredicateExpr<'a> {
    Leaf(&'a str, BinaryOp, PredicateOperand<'a>),
    And(Vec<PredicateExpr<'a>>),
    Or(Vec<PredicateExpr<'a>>),
}

/// Reconoce `|item: T| item.campo OP valor` (en cualquier orden -- `valor
/// OP item.campo` también) para `OP` en `==`/`!=`/`<`/`<=`/`>`/`>=`, una
/// conjunción de varias hojas así unidas con `&&`, y AHORA TAMBIÉN `||`
/// combinándolas en cualquier profundidad (`item.a == v1 && item.b == v2 ||
/// item.c == v3`, respetando la precedencia real del lenguaje) --
/// GRAMMAR.md §3.95 (`==`, v1.59.0), §3.108 (los otros cinco operadores),
/// §3.109 (conjunción de N hojas), §3.170 (`||`), §3.171 (`item.campoA OP
/// item.campoB`, los cuatro relacionales solamente -- ver el comentario en
/// el brazo `Lt | LtEq | Gt | GtEq` de este mismo reconocedor para por qué
/// `==`/`!=` entre dos campos queda deliberadamente afuera). Incluye
/// `!item.campo`/`item.campo` sueltos como hojas booleanas. El ÚNICO shape
/// de predicado
/// que `countWhere`/`findWhere`/`upsert` (su `matchFn`) empujan a SQL en
/// vez de traer la colección entera a memoria y evaluar el predicado fila
/// por fila. Cada hoja trae el nombre del campo, el operador (ya
/// "enderezado" -- ver abajo) y el lado "valor" sin evaluar -- el caller
/// (`runtime/mod.rs`, que sí tiene acceso al `Env` capturado del closure)
/// decide si cada uno es lo bastante simple como para confiar en el
/// resultado (un literal, o un `Ident` que resuelve en ese `Env`) sin
/// tener que reimplementar un evaluador de expresiones acá.
///
/// Cuando el campo de una hoja de comparación aparece del lado DERECHO (`5
/// < item.campo`), el operador se invierte (`Lt` -> `Gt`) para que el
/// caller SIEMPRE reciba "campo OP valor" con el campo a la izquierda, sin
/// tener que manejar los dos órdenes por separado en el sitio de
/// generación de SQL.
///
/// Mismo criterio conservador que `recognize_field_selector`: un campo
/// derivado, una comparación `==`/`!=` entre DOS campos del propio
/// parámetro (los cuatro relacionales sí se reconocen, ver §3.171 arriba), o
/// `!(...)` negando algo que no sea una hoja de campo suelta (`!(a && b)`,
/// De Morgan) hace fallar TODO el reconocimiento (`None`) -- el caller cae
/// al camino interpretado de siempre, correcto en cualquier caso, más
/// lento solo en ese caso puntual. `deleteWhere` tampoco gana este atajo
/// (mismo motivo que la versión de un solo operador: publicar cada fila
/// borrada a `stream` complica un `DELETE ... WHERE` de una sola sentencia).
pub fn recognize_predicate_expr<'a>(param_names: &[String], body: &'a Block) -> Option<PredicateExpr<'a>> {
    let [param] = param_names else { return None };
    if !body.stmts.is_empty() {
        return None;
    }
    recognize_predicate_tree(param, body.tail.as_ref()?)
}

fn strip_parens(mut e: &Spanned<Expr>) -> &Spanned<Expr> {
    while let Expr::Paren(inner) = &e.node {
        e = inner;
    }
    e
}

fn field_of_param<'a>(param: &str, e: &'a Spanned<Expr>) -> Option<&'a str> {
    let Expr::FieldAccess { base, field } = &e.node else { return None };
    matches!(&base.node, Expr::Ident(n) if n == param).then(|| field.as_str())
}

/// `a && b` combinados: si CUALQUIERA de los dos ya es un `And`, sus hojas
/// se aplanan adentro del nuevo -- así `a && b && c` (que el parser arma
/// como `And(And(a,b), c)`, asociatividad izquierda) sigue devolviendo un
/// único `And` de 3 elementos, no un árbol anidado -- el SQL generado para
/// el caso puro de siempre no gana paréntesis de más.
fn merge_and<'a>(l: PredicateExpr<'a>, r: PredicateExpr<'a>) -> PredicateExpr<'a> {
    let mut items = Vec::new();
    match l {
        PredicateExpr::And(v) => items.extend(v),
        other => items.push(other),
    }
    match r {
        PredicateExpr::And(v) => items.extend(v),
        other => items.push(other),
    }
    PredicateExpr::And(items)
}

/// Como `merge_and`, para `||`.
fn merge_or<'a>(l: PredicateExpr<'a>, r: PredicateExpr<'a>) -> PredicateExpr<'a> {
    let mut items = Vec::new();
    match l {
        PredicateExpr::Or(v) => items.extend(v),
        other => items.push(other),
    }
    match r {
        PredicateExpr::Or(v) => items.extend(v),
        other => items.push(other),
    }
    PredicateExpr::Or(items)
}

fn recognize_predicate_tree<'a>(param: &str, expr: &'a Spanned<Expr>) -> Option<PredicateExpr<'a>> {
    let expr = strip_parens(expr);
    match &expr.node {
        Expr::Binary { op: BinaryOp::And, left, right } => {
            let l = recognize_predicate_tree(param, left)?;
            let r = recognize_predicate_tree(param, right)?;
            Some(merge_and(l, r))
        }
        Expr::Binary { op: BinaryOp::Or, left, right } => {
            let l = recognize_predicate_tree(param, left)?;
            let r = recognize_predicate_tree(param, right)?;
            Some(merge_or(l, r))
        }
        Expr::Binary { op, left, right }
            if matches!(op, BinaryOp::Eq | BinaryOp::NotEq | BinaryOp::Lt | BinaryOp::LtEq | BinaryOp::Gt | BinaryOp::GtEq) =>
        {
            let left = strip_parens(left);
            let right = strip_parens(right);
            // `item.endDate > item.startDate` -- comparación entre DOS
            // campos del propio parámetro, GRAMMAR.md §3.171. Acotado a los
            // cuatro operadores relacionales a propósito: el checker
            // (checker.rs::synth_binary, brazo `Lt | LtEq | Gt | GtEq`) solo
            // los tipa cuando ambos lados son Int/Int64/Float/Timestamp SIN
            // envolver en `Optional` -- y un campo no-opcional siempre es
            // `NOT NULL` en la columna real (ver postgres_emit.rs, el test
            // que confirma que desenvolver `Optional` no cuela un `NOT NULL`
            // de más) -- así que esta forma nunca puede toparse con el
            // problema de NULL-seguridad que si tiene `==`/`!=` (donde el
            // checker sí permite comparar dos `T?`, y `NULL = NULL` en SQL
            // no es `true` como en el camino interpretado). Por eso `==`/
            // `!=` entre dos campos NO se reconoce acá -- cae al camino
            // interpretado de siempre (ya lo hacía antes de este cambio,
            // sin que hiciera falta ningún chequeo nuevo: el lado derecho
            // queda envuelto en `PredicateOperand::Expr` de un
            // `FieldAccess`, que `evaluate_predicate_tree` ya rechaza por no
            // ser un literal ni un `Ident`).
            if matches!(op, BinaryOp::Lt | BinaryOp::LtEq | BinaryOp::Gt | BinaryOp::GtEq) {
                if let (Some(lf), Some(rf)) = (field_of_param(param, left), field_of_param(param, right)) {
                    return Some(PredicateExpr::Leaf(lf, *op, PredicateOperand::Field(rf)));
                }
            }
            if let Some(field) = field_of_param(param, left) {
                return Some(PredicateExpr::Leaf(field, *op, PredicateOperand::Expr(right)));
            }
            if let Some(field) = field_of_param(param, right) {
                return Some(PredicateExpr::Leaf(field, flip_comparison_operator(*op), PredicateOperand::Expr(left)));
            }
            None
        }
        Expr::Unary { op: UnaryOp::Not, operand } => {
            let field = field_of_param(param, strip_parens(operand))?;
            Some(PredicateExpr::Leaf(field, BinaryOp::Eq, PredicateOperand::Bool(false)))
        }
        _ => {
            let field = field_of_param(param, expr)?;
            Some(PredicateExpr::Leaf(field, BinaryOp::Eq, PredicateOperand::Bool(true)))
        }
    }
}

/// `a OP b` <=> `b flip(OP) a` -- usado cuando el campo del predicado
/// pusheable aparece del lado derecho de la comparación. `==`/`!=` son
/// simétricos (sin cambio); los cuatro relacionales se invierten cruzado
/// (`<` <-> `>`, `<=` <-> `>=`).
fn flip_comparison_operator(op: BinaryOp) -> BinaryOp {
    match op {
        BinaryOp::Lt => BinaryOp::Gt,
        BinaryOp::LtEq => BinaryOp::GtEq,
        BinaryOp::Gt => BinaryOp::Lt,
        BinaryOp::GtEq => BinaryOp::LtEq,
        other => other,
    }
}
