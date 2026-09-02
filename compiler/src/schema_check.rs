//! Compatibilidad de TIPOS entre lo que un `.link` declara y lo que una base
//! PostgreSQL ya existente tiene de verdad (GRAMMAR.md §3.229, PLAN.md §9.19
//! ítem 4).
//!
//! Hasta esta ronda, `linkc doctor`, `linkc db inspect` y `linkc migrate
//! --dry-run` miraban solo si la tabla y las COLUMNAS existían -- nunca de
//! qué tipo eran. Una columna `uuid[]` declarada como `String[]`, o un
//! `integer` declarado como `Bool`, pasaban los tres con "0 errores / nada
//! que migrar", y el fallo aparecía recién al leer una fila real en
//! producción (así descubrió el CRM Nexus los arrays de §3.228, en dos
//! servicios distintos). Esta tabla de compatibilidad es la MISMA que
//! `runtime/store.rs` aplica al leer (`postgres_cell` y sus fallbacks) y al
//! escribir (`Cell::to_sql` por tipo de columna), escrita una vez como dato
//! para que las tres herramientas avisen ANTES de la primera fila.
//!
//! Solo PostgreSQL: SQLite tiene tipado dinámico y `check_schema_matches`
//! (db.rs) ya compara el DDL exacto al conectar.

use crate::ast::Program;
use crate::checker::Checker;
use crate::runtime::db::{encrypted_fields_by_collection, ColumnPlan};
use crate::runtime::simple_enum_names;
use crate::runtime::store::{Backend, Cell, ColumnKind};
use crate::types::Type;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Va a fallar al leer o escribir una fila real: tipo que el runtime no
    /// decodifica, o array de un elemento sin soporte.
    Error,
    /// Funciona pero con un borde que conviene saber: una columna nullable
    /// detrás de un campo requerido (§3.68 da un error limpio por fila).
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnIssue {
    pub collection: String,
    pub column: String,
    pub severity: Severity,
    pub message: String,
}

impl ColumnIssue {
    pub fn render(&self) -> String {
        let tag = match self.severity {
            Severity::Error => "ERROR",
            Severity::Warning => "AVISO",
        };
        format!("[{tag}] {}.{}: {}", self.collection, self.column, self.message)
    }
}

/// Lo que `information_schema.columns` dice de una columna física.
#[derive(Debug, Clone)]
pub struct PhysicalColumn {
    pub name: String,
    /// `data_type`: `integer`, `text`, `ARRAY`, `USER-DEFINED`, ...
    pub data_type: String,
    /// `udt_name`: para `ARRAY` es el elemento con `_` adelante (`_int4`);
    /// para un enum de Postgres, su nombre.
    pub udt_name: String,
    pub nullable: bool,
}

fn declared_list_element(ty: &Type) -> Option<&Type> {
    match ty {
        Type::Optional(inner) => declared_list_element(inner),
        Type::List(elem) => Some(elem),
        _ => None,
    }
}

fn effective(ty: &Type) -> &Type {
    match ty {
        Type::Optional(inner) => effective(inner),
        other => other,
    }
}

/// La regla pura, sin base: ¿el campo declarado (su plan de columna) puede
/// leer y escribir esta columna física? `None` = compatible. Es la tabla
/// de `runtime/store.rs` en forma de dato -- si una rama nueva se agrega
/// allá, se agrega acá.
pub(crate) fn check_column(plan: &ColumnPlan, physical: &PhysicalColumn) -> Option<(Severity, String)> {
    let dt = physical.data_type.to_ascii_lowercase();
    let udt = physical.udt_name.to_ascii_lowercase();
    let declared_name = plan.field.ty.to_string();

    let int_like = matches!(dt.as_str(), "bigint" | "integer" | "smallint");
    let text_like = matches!(dt.as_str(), "text" | "character varying" | "character" | "citext");

    // Un campo `@encrypted` viaja como texto cifrado sea cual sea su tipo
    // declarado (§3.191): la columna tiene que ser texto.
    if plan.encrypted {
        return if text_like || dt == "bytea" {
            None
        } else {
            Some((Severity::Error, format!("campo @encrypted (se guarda como texto cifrado) sobre una columna '{dt}' -- tiene que ser text/varchar")))
        };
    }

    let kind = plan.kind();
    let compatible = match kind {
        ColumnKind::Int => int_like,
        ColumnKind::Timestamp => int_like || matches!(dt.as_str(), "timestamp without time zone" | "timestamp with time zone" | "date"),
        ColumnKind::Float => matches!(dt.as_str(), "double precision" | "real" | "numeric"),
        ColumnKind::Decimal => dt == "numeric",
        ColumnKind::Bool => dt == "boolean",
        ColumnKind::Uuid => dt == "uuid" || text_like,
        ColumnKind::Text => text_like || matches!(dt.as_str(), "uuid" | "inet" | "cidr" | "json" | "jsonb"),
        ColumnKind::Json => {
            if dt == "array" {
                // GRAMMAR.md §3.228: solo una lista declarada puede leer un
                // array nativo, y solo de los elementos que store.rs conoce.
                let Some(elem) = declared_list_element(&plan.field.ty) else {
                    return Some((
                        Severity::Error,
                        format!("la columna es un array nativo ('{}[]') pero el campo se declara como {declared_name}, que se guarda como JSON -- declaralo como lista del tipo del elemento", udt.trim_start_matches('_')),
                    ));
                };
                let ok = match effective(elem) {
                    Type::Int | Type::Int64 => matches!(udt.as_str(), "_int2" | "_int4" | "_int8"),
                    Type::String | Type::Uuid | Type::Enum(_) => matches!(udt.as_str(), "_text" | "_varchar" | "_bpchar" | "_citext"),
                    Type::Bool => udt == "_bool",
                    Type::Float => matches!(udt.as_str(), "_float4" | "_float8"),
                    _ => false,
                };
                if !ok {
                    return Some((
                        Severity::Error,
                        format!(
                            "array nativo de '{}' declarado como {declared_name} -- c-script lee/escribe arrays de enteros, texto, booleanos y flotantes (integer[]/bigint[]/text[]/boolean[]/double precision[]), con el tipo de elemento correspondiente (GRAMMAR.md §3.228)",
                            udt.trim_start_matches('_')
                        ),
                    ));
                }
                true
            } else {
                matches!(dt.as_str(), "json" | "jsonb") || text_like
            }
        }
    };
    if !compatible {
        let kind_name = match kind {
            ColumnKind::Json => "JSON (jsonb/json/text)",
            ColumnKind::Int => "un entero (bigint/integer/smallint)",
            ColumnKind::Timestamp => "bigint de milisegundos, timestamp, timestamptz o date",
            ColumnKind::Float => "double precision/real/numeric",
            ColumnKind::Decimal => "numeric",
            ColumnKind::Bool => "boolean",
            ColumnKind::Uuid => "uuid o text",
            ColumnKind::Text => "text/varchar (o uuid/inet/cidr/json/jsonb)",
        };
        let shown = if dt == "user-defined" || dt == "array" { format!("{dt} ({udt})") } else { dt.clone() };
        return Some((
            Severity::Error,
            format!("declarado como {declared_name}, que espera {kind_name}; la columna es '{shown}' -- va a fallar al leer o escribir una fila real"),
        ));
    }
    if physical.nullable && plan_not_null(plan) {
        return Some((
            Severity::Warning,
            format!("declarado como {declared_name} (requerido) pero la columna admite NULL -- una fila con NULL da un error limpio al leerla (GRAMMAR.md §3.68); declaralo como {declared_name}? si puede faltar"),
        ));
    }
    None
}

fn plan_not_null(plan: &ColumnPlan) -> bool {
    !plan.field.optional && !matches!(plan.field.ty, Type::Optional(_))
}

/// Los planes de columna de cada colección, exactamente como `Db::new` los
/// construye (misma `ColumnPlan::for_field`, mismos enums simples, mismos
/// `@encrypted`), sin abrir ninguna base.
pub(crate) fn column_plans(program: &Program) -> Result<Vec<(String, Vec<ColumnPlan>)>, String> {
    let (checker, errors) = Checker::build_symbols(program);
    if let Some(e) = errors.into_iter().next() {
        return Err(format!("programa inválido: {e}"));
    }
    let simple_enums = simple_enum_names(program);
    let encrypted = encrypted_fields_by_collection(program, &checker);
    let mut out = Vec::new();
    for (name, element_ty) in checker.db_collections() {
        let Type::Struct { fields, .. } = element_ty else { continue };
        let encrypted_here = encrypted.get(name).cloned().unwrap_or_default();
        let plans: Vec<ColumnPlan> = fields
            .iter()
            .filter(|f| f.name != "id")
            .map(|f| ColumnPlan::for_field(f.clone(), &simple_enums, encrypted_here.contains(&f.name)))
            .collect();
        out.push((name.clone(), plans));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Las columnas físicas de una tabla, `None` si la tabla no existe.
pub(crate) fn physical_columns(backend: &Backend, collection: &str) -> Result<Option<Vec<PhysicalColumn>>, String> {
    let sql = format!(
        "SELECT column_name, data_type, udt_name, is_nullable FROM information_schema.columns \
         WHERE table_name = {} AND table_schema = ANY(current_schemas(false)) ORDER BY ordinal_position",
        backend.placeholder(1)
    );
    let rows = backend
        .query(&sql, &[Cell::Text(collection.to_string())], &[ColumnKind::Text, ColumnKind::Text, ColumnKind::Text, ColumnKind::Text])
        .map_err(|e| format!("no se pudo leer los tipos de '{collection}': {e}"))?;
    if rows.is_empty() {
        return Ok(None);
    }
    let text = |c: &Cell| if let Cell::Text(s) = c { s.clone() } else { String::new() };
    Ok(Some(
        rows.iter()
            .map(|r| PhysicalColumn {
                name: text(&r[0]),
                data_type: text(&r[1]),
                udt_name: text(&r[2]),
                nullable: text(&r[3]) == "YES",
            })
            .collect(),
    ))
}

/// Todos los problemas de tipo del programa contra la base conectada, en
/// orden de colección y columna. Una tabla que no existe no genera nada
/// acá (eso ya lo reportan `migrate`/`inspect` como "no existe todavía").
pub(crate) fn check_program(program: &Program, backend: &Backend) -> Result<Vec<ColumnIssue>, String> {
    let mut issues = Vec::new();
    for (collection, plans) in column_plans(program)? {
        let Some(physical) = physical_columns(backend, &collection)? else { continue };
        let by_name: HashMap<&str, &PhysicalColumn> = physical.iter().map(|c| (c.name.as_str(), c)).collect();
        for plan in &plans {
            let Some(col) = by_name.get(plan.field.name.as_str()) else { continue };
            if let Some((severity, message)) = check_column(plan, col) {
                issues.push(ColumnIssue { collection: collection.clone(), column: plan.field.name.clone(), severity, message });
            }
        }
    }
    Ok(issues)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::parse;

    fn plans_for(src: &str) -> Vec<ColumnPlan> {
        let program = parse(tokenize(src).expect("lexer")).expect("parser");
        let mut all = column_plans(&program).expect("plans");
        assert_eq!(all.len(), 1);
        all.remove(0).1
    }

    fn physical(data_type: &str, udt: &str, nullable: bool) -> PhysicalColumn {
        PhysicalColumn { name: "x".into(), data_type: data_type.into(), udt_name: udt.into(), nullable }
    }

    fn plan(src_field: &str) -> ColumnPlan {
        plans_for(&format!("type T = {{ id: Int, x: {src_field} }}\ndb {{ ts: T[] }}\n")).remove(0)
    }

    #[test]
    fn scalars_accept_their_native_columns_and_reject_the_rest() {
        assert!(check_column(&plan("Int"), &physical("integer", "int4", false)).is_none());
        assert!(check_column(&plan("Int"), &physical("bigint", "int8", false)).is_none());
        assert!(check_column(&plan("Int"), &physical("text", "text", false)).is_some());
        assert!(check_column(&plan("Bool"), &physical("boolean", "bool", false)).is_none());
        assert!(check_column(&plan("Bool"), &physical("integer", "int4", false)).is_some(), "un Bool no lee un integer");
        assert!(check_column(&plan("String"), &physical("uuid", "uuid", false)).is_none(), "§3.179");
        assert!(check_column(&plan("String"), &physical("jsonb", "jsonb", false)).is_none(), "§3.187");
        assert!(check_column(&plan("String"), &physical("integer", "int4", false)).is_some());
        assert!(check_column(&plan("Timestamp"), &physical("timestamp with time zone", "timestamptz", false)).is_none(), "§3.182");
        assert!(check_column(&plan("Decimal"), &physical("numeric", "numeric", false)).is_none());
        assert!(check_column(&plan("Float"), &physical("numeric", "numeric", false)).is_none());
        assert!(check_column(&plan("Uuid"), &physical("uuid", "uuid", false)).is_none());
    }

    #[test]
    fn lists_accept_supported_native_arrays_and_json_and_reject_other_arrays() {
        assert!(check_column(&plan("Int[]"), &physical("ARRAY", "_int4", false)).is_none());
        assert!(check_column(&plan("Int[]"), &physical("ARRAY", "_int8", false)).is_none());
        assert!(check_column(&plan("String[]"), &physical("ARRAY", "_text", false)).is_none());
        assert!(check_column(&plan("Bool[]"), &physical("ARRAY", "_bool", false)).is_none());
        assert!(check_column(&plan("Int[]"), &physical("jsonb", "jsonb", false)).is_none(), "una lista en JSON sigue siendo válida");
        let (sev, msg) = check_column(&plan("String[]"), &physical("ARRAY", "_uuid", false)).expect("uuid[] no se lee");
        assert_eq!(sev, Severity::Error);
        assert!(msg.contains("array nativo de 'uuid'"), "{msg}");
        let (sev, _) = check_column(&plan("Int[]"), &physical("ARRAY", "_text", false)).expect("Int[] contra text[]");
        assert_eq!(sev, Severity::Error);
        let (_, msg) = check_column(&plan("String"), &physical("ARRAY", "_int4", false)).expect("String contra un array");
        assert!(msg.contains("declarado como String"), "{msg}");
    }

    #[test]
    fn a_nullable_column_behind_a_required_field_is_a_warning_not_an_error() {
        let (sev, msg) = check_column(&plan("Int"), &physical("integer", "int4", true)).expect("aviso");
        assert_eq!(sev, Severity::Warning);
        assert!(msg.contains("§3.68"), "{msg}");
        assert!(check_column(&plan("Int?"), &physical("integer", "int4", true)).is_none());
    }

    #[test]
    fn an_encrypted_field_needs_a_text_column_whatever_its_declared_type() {
        let p = plans_for("type T = { id: Int, @encrypted x: Int }\ndb { ts: T[] }\n").remove(0);
        assert!(p.encrypted);
        assert!(check_column(&p, &physical("text", "text", false)).is_none());
        assert!(check_column(&p, &physical("integer", "int4", false)).is_some());
    }
}
