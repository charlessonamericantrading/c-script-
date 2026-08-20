//! Parseo, matching y detección de conflictos de patrones `@route("/blog/:slug")`
//! (GRAMMAR.md §3.37, extendido a múltiples parámetros en §3.42).
//!
//! Un único parser acá, usado TANTO por el checker (validar en compilación)
//! COMO por el servidor (despachar en runtime) -- la razón de que exista este
//! módulo aparte en vez de tener la lógica duplicada en `checker.rs` y
//! `runtime/server.rs` es exactamente la que este proyecto ya viene
//! documentando desde GRAMMAR.md §3.9: dos capas que implementan la misma
//! regla por separado terminan divergiendo. Acá hay una sola fuente de
//! verdad de qué es un patrón válido, qué significa que matchee, y cuándo dos
//! patrones distintos son indistinguibles al despachar una request real.
//!
//! v0 (GRAMMAR.md §3.37) permitía como mucho UN segmento parametrizado, y
//! tenía que ser el último. §3.42 lo generaliza a cualquier cantidad de
//! parámetros, en cualquier posición (`/blog/:categoria/:slug`) -- lo único
//! que sigue acotado a propósito es la detección de conflictos: dos rutas
//! entran en conflicto si podrían matchear el MISMO path real y ninguna es
//! estrictamente más específica que la otra (ver `conflicts_with`).

/// Un segmento de un patrón de ruta ya parseado: literal (tiene que
/// matchear texto exacto) o parámetro (captura lo que sea que haya ahí).
#[derive(Debug, Clone, PartialEq)]
pub enum Segment {
    Literal(String),
    Param(String),
}

/// Un patrón de ruta ya parseado y validado en su FORMA (no valida que el rpc
/// tenga los parámetros correspondientes -- eso lo hace el checker, que es
/// quien tiene el `Vec<Param>` del rpc a mano).
#[derive(Debug, Clone, PartialEq)]
pub struct RoutePattern {
    pub segments: Vec<Segment>,
}

impl RoutePattern {
    /// Cuántos segmentos tiene esta ruta en total -- lo que un path real
    /// necesita tener para siquiera candidatear a matchear.
    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }

    /// Los nombres de parámetro, en el ORDEN en que aparecen en la ruta --
    /// el orden que importa para capturar valores en `matches`.
    pub fn param_names(&self) -> Vec<&str> {
        self.segments
            .iter()
            .filter_map(|s| match s {
                Segment::Param(name) => Some(name.as_str()),
                Segment::Literal(_) => None,
            })
            .collect()
    }

    /// Cuántos segmentos son literales -- más literales significa más
    /// específico. Una ruta con más segmentos fijos gana sobre una que deja
    /// más lugares libres, cuando las dos podrían matchear el mismo path
    /// real (`conflicts_with`, más abajo, y `resolve_route` en
    /// `runtime/server.rs`, que ordena la tabla de rutas por esto).
    pub fn specificity(&self) -> usize {
        self.segments.iter().filter(|s| matches!(s, Segment::Literal(_))).count()
    }

    /// Si ALGÚN path real podría matchear tanto a `self` como a `other`.
    /// Alcanza con encontrar una sola posición donde los dos sean literales
    /// con texto DISTINTO para probar que nunca se cruzan -- sin importar
    /// qué haya en el resto del patrón (un parámetro acepta cualquier
    /// texto, así que nunca por sí solo separa dos rutas).
    fn overlap_possible(&self, other: &RoutePattern) -> bool {
        self.segments.len() == other.segments.len()
            && self.segments.iter().zip(other.segments.iter()).all(|(a, b)| match (a, b) {
                (Segment::Literal(x), Segment::Literal(y)) => x == y,
                _ => true,
            })
    }

    /// Si `self` y `other` compiten de forma AMBIGUA al despachar una
    /// request real: pueden matchear el MISMO path (`overlap_possible`) Y
    /// ninguna es estrictamente más específica que la otra
    /// (`specificity()` empatada). Con especificidad distinta no hay
    /// conflicto -- la más específica gana siempre, es una regla
    /// determinística (mismo criterio que cualquier router HTTP común:
    /// `/blog/featured` le gana a `/blog/:slug`, y eso se generaliza acá a
    /// "más segmentos literales fijos gana").
    ///
    /// Ejemplo del caso que esto existe para atrapar: `/blog/:categoria/latest`
    /// y `/blog/featured/:slug` NO tienen la misma forma (posición 0 es
    /// parámetro en una y literal en la otra, y viceversa en la posición 1),
    /// pero las dos podrían matchear `/blog/featured/latest` -- y las dos
    /// tienen exactamente UN segmento literal, así que ninguna le gana a la
    /// otra. Eso es un conflicto real, aunque las formas sean distintas.
    pub fn conflicts_with(&self, other: &RoutePattern) -> bool {
        self.overlap_possible(other) && self.specificity() == other.specificity()
    }

    /// Intenta matchear los segmentos de un path real (ya partido por "/",
    /// sin el "" inicial de un path que empieza con "/"). Devuelve los
    /// valores crudos (sin decodificar, sin tipar) capturados por cada
    /// parámetro, EN EL ORDEN en que los parámetros aparecen en el patrón
    /// (el mismo orden que `param_names()`).
    pub fn matches<'p>(&self, path_segments: &[&'p str]) -> Option<Vec<&'p str>> {
        if path_segments.len() != self.segments.len() {
            return None;
        }
        let mut captured = Vec::new();
        for (seg, actual) in self.segments.iter().zip(path_segments.iter()) {
            match seg {
                Segment::Literal(expected) => {
                    if expected != actual {
                        return None;
                    }
                }
                Segment::Param(_) => captured.push(*actual),
            }
        }
        Some(captured)
    }
}

/// Parsea el texto crudo de `@route("...")`. Reglas (todas producen un
/// mensaje de error pensado para leerse tal cual, sin que el caller le
/// agregue contexto):
///
/// - Tiene que empezar con `/`.
/// - Sin segmentos vacíos (`//`, o una barra final que no sea la raíz).
/// - Un segmento que empieza con `:` es un parámetro; el nombre después de
///   `:` tiene que ser un identificador válido (letra/`_` inicial,
///   alfanumérico/`_` después).
/// - Ningún nombre de parámetro puede repetirse DENTRO de la misma ruta.
pub fn parse_route_pattern(raw: &str) -> Result<RoutePattern, String> {
    let Some(rest) = raw.strip_prefix('/') else {
        return Err(format!("'{raw}' tiene que empezar con '/' (ej. '/blog/:slug')"));
    };
    if rest.is_empty() {
        return Err("'/' sola no es una ruta válida -- hace falta al menos un segmento".to_string());
    }
    let parts: Vec<&str> = rest.split('/').collect();
    if parts.iter().any(|s| s.is_empty()) {
        return Err(format!(
            "'{raw}' tiene un segmento vacío (una '//' en el medio, o una '/' final de sobra)"
        ));
    }

    let mut segments = Vec::with_capacity(parts.len());
    let mut seen_params: Vec<&str> = Vec::new();
    for part in &parts {
        if let Some(name) = part.strip_prefix(':') {
            if !is_valid_param_name(name) {
                return Err(format!(
                    "'{raw}': ':{name}' no es un nombre de parámetro válido (tiene que ser un identificador: empezar con letra o '_', seguido de letras/dígitos/'_')"
                ));
            }
            if seen_params.contains(&name) {
                return Err(format!(
                    "'{raw}': ':{name}' aparece más de una vez -- cada parámetro de la ruta necesita un nombre distinto"
                ));
            }
            seen_params.push(name);
            segments.push(Segment::Param(name.to_string()));
        } else {
            segments.push(Segment::Literal((*part).to_string()));
        }
    }

    Ok(RoutePattern { segments })
}

fn is_valid_param_name(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_purely_literal_route() {
        let r = parse_route_pattern("/sitemap.xml").unwrap();
        assert_eq!(r.segments, vec![Segment::Literal("sitemap.xml".to_string())]);
        assert_eq!(r.segment_count(), 1);
        assert!(r.param_names().is_empty());
    }

    #[test]
    fn parses_a_trailing_param() {
        let r = parse_route_pattern("/blog/:slug").unwrap();
        assert_eq!(r.segments, vec![Segment::Literal("blog".to_string()), Segment::Param("slug".to_string())]);
        assert_eq!(r.segment_count(), 2);
        assert_eq!(r.param_names(), vec!["slug"]);
    }

    #[test]
    fn a_lone_param_has_no_literal_segments() {
        let r = parse_route_pattern("/:slug").unwrap();
        assert_eq!(r.segments, vec![Segment::Param("slug".to_string())]);
        assert_eq!(r.param_names(), vec!["slug"]);
    }

    #[test]
    fn parses_multiple_params_in_any_position() {
        let r = parse_route_pattern("/blog/:categoria/:slug").unwrap();
        assert_eq!(r.param_names(), vec!["categoria", "slug"]);
        let r = parse_route_pattern("/:categoria/blog/:slug/comentarios").unwrap();
        assert_eq!(r.param_names(), vec!["categoria", "slug"]);
        assert_eq!(r.segment_count(), 4);
    }

    #[test]
    fn rejects_missing_leading_slash() {
        assert!(parse_route_pattern("blog/:slug").is_err());
    }

    #[test]
    fn rejects_empty_segments() {
        assert!(parse_route_pattern("/blog//featured").is_err());
        assert!(parse_route_pattern("/blog/").is_err());
    }

    #[test]
    fn rejects_a_repeated_param_name_in_the_same_route() {
        let e = parse_route_pattern("/:slug/comentarios/:slug").unwrap_err();
        assert!(e.contains("más de una vez"), "mensaje inesperado: {e}");
    }

    #[test]
    fn rejects_an_invalid_param_name() {
        assert!(parse_route_pattern("/blog/:123").is_err());
        assert!(parse_route_pattern("/blog/:").is_err());
    }

    #[test]
    fn identical_patterns_conflict() {
        let a = parse_route_pattern("/blog/:slug").unwrap();
        let b = parse_route_pattern("/blog/:id").unwrap();
        assert!(a.conflicts_with(&b), "mismo nombre de parámetro no importa, la FORMA es la misma");
    }

    #[test]
    fn a_literal_route_never_conflicts_with_a_param_route_at_the_same_position() {
        let literal = parse_route_pattern("/blog/featured").unwrap();
        let param = parse_route_pattern("/blog/:slug").unwrap();
        assert!(!literal.conflicts_with(&param), "el literal es más específico, gana determinísticamente");
    }

    #[test]
    fn two_purely_literal_routes_with_different_text_never_conflict() {
        let a = parse_route_pattern("/sitemap.xml").unwrap();
        let b = parse_route_pattern("/robots.txt").unwrap();
        assert!(!a.conflicts_with(&b), "un solo segmento literal distinto ya prueba que nunca se cruzan");
    }

    #[test]
    fn cross_position_ambiguity_between_differently_shaped_multi_param_routes_is_a_conflict() {
        // El caso que motivó `conflicts_with` en vez de una comparación de
        // forma exacta: ninguna de las dos tiene la MISMA forma que la
        // otra, pero las dos matchean `/blog/featured/latest` y ninguna es
        // más específica (1 segmento literal cada una).
        let a = parse_route_pattern("/blog/:categoria/latest").unwrap();
        let b = parse_route_pattern("/blog/featured/:slug").unwrap();
        assert!(a.conflicts_with(&b));
        assert!(b.conflicts_with(&a), "conflicts_with tiene que ser simétrico");
    }

    #[test]
    fn a_more_specific_multi_segment_route_does_not_conflict_with_a_fully_dynamic_one() {
        let generic = parse_route_pattern("/blog/:categoria/:slug").unwrap();
        let specific = parse_route_pattern("/blog/featured/:slug").unwrap();
        assert!(!generic.conflicts_with(&specific), "1 literal contra 0 literales: la específica gana sola");
    }

    #[test]
    fn matching_a_literal_route_requires_an_exact_match() {
        let r = parse_route_pattern("/sitemap.xml").unwrap();
        assert_eq!(r.matches(&["sitemap.xml"]), Some(vec![]));
        assert_eq!(r.matches(&["robots.txt"]), None);
        assert_eq!(r.matches(&["sitemap.xml", "extra"]), None);
    }

    #[test]
    fn matching_a_param_route_captures_the_segment() {
        let r = parse_route_pattern("/blog/:slug").unwrap();
        assert_eq!(r.matches(&["blog", "hola-mundo"]), Some(vec!["hola-mundo"]));
        assert_eq!(r.matches(&["blog"]), None);
        assert_eq!(r.matches(&["otra", "cosa"]), None);
    }

    #[test]
    fn matching_multiple_params_captures_them_in_pattern_order() {
        let r = parse_route_pattern("/blog/:categoria/:slug").unwrap();
        assert_eq!(r.matches(&["blog", "rust", "hola-mundo"]), Some(vec!["rust", "hola-mundo"]));
        assert_eq!(r.matches(&["blog", "rust"]), None, "faltan segmentos");
    }
}
