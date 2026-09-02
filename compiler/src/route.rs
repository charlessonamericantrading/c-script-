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
/// matchear texto exacto), parámetro (captura UN segmento) o catch-all
/// (captura CERO o más segmentos restantes, unidos con "/" -- GRAMMAR.md
/// §3.42, ronda catch-all).
#[derive(Debug, Clone, PartialEq)]
pub enum Segment {
    Literal(String),
    Param(String),
    CatchAll(String),
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
    /// el orden que importa para capturar valores en `matches`. Incluye el
    /// catch-all si lo hay (siempre el último, por construcción de
    /// `parse_route_pattern`) -- se bindea a un parámetro del rpc exactamente
    /// igual que un `Param` normal, solo que como `String` únicamente.
    pub fn param_names(&self) -> Vec<&str> {
        self.segments
            .iter()
            .filter_map(|s| match s {
                Segment::Param(name) | Segment::CatchAll(name) => Some(name.as_str()),
                Segment::Literal(_) => None,
            })
            .collect()
    }

    /// El nombre del segmento catch-all de esta ruta, si tiene uno --
    /// `parse_route_pattern` garantiza que si existe, es el ÚLTIMO segmento.
    /// Lo usa el checker para exigir que ese parámetro del rpc sea `String`
    /// (nunca `Int`: el texto capturado puede contener "/").
    pub fn catchall_name(&self) -> Option<&str> {
        match self.segments.last() {
            Some(Segment::CatchAll(name)) => Some(name.as_str()),
            _ => None,
        }
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
    /// Cuántos segmentos FIJOS tiene el patrón antes de un eventual
    /// catch-all -- el total si no lo tiene, `total - 1` si lo tiene (el
    /// catch-all mismo no es un segmento fijo, es "cero o más").
    fn fixed_len(&self) -> usize {
        match self.segments.last() {
            Some(Segment::CatchAll(_)) => self.segments.len() - 1,
            _ => self.segments.len(),
        }
    }

    fn has_catchall(&self) -> bool {
        matches!(self.segments.last(), Some(Segment::CatchAll(_)))
    }

    fn overlap_possible(&self, other: &RoutePattern) -> bool {
        if !self.has_catchall() && !other.has_catchall() {
            return self.segments.len() == other.segments.len()
                && self.segments.iter().zip(other.segments.iter()).all(|(a, b)| match (a, b) {
                    (Segment::Literal(x), Segment::Literal(y)) => x == y,
                    _ => true,
                });
        }
        // Al menos uno de los dos tiene catch-all: ese lado se puede
        // estirar para cubrir cualquier cantidad de segmentos restantes,
        // así que ya no hace falta que las dos rutas tengan la MISMA
        // longitud total -- alcanza con que el lado sin catch-all (si lo
        // hay) tenga al menos tantos segmentos como el prefijo fijo del
        // otro, y que los prefijos fijos compartidos (hasta el más corto de
        // los dos) sean compatibles. Conservador a propósito: prefiere un
        // falso positivo (marcar conflicto donde en la práctica nunca
        // chocarían) a dejar pasar una ambigüedad real -- mismo criterio
        // que el resto de este archivo.
        let self_fixed = self.fixed_len();
        let other_fixed = other.fixed_len();
        if !self.has_catchall() && self.segments.len() < other_fixed {
            return false;
        }
        if !other.has_catchall() && other.segments.len() < self_fixed {
            return false;
        }
        let common = self_fixed.min(other_fixed);
        self.segments[..common].iter().zip(other.segments[..common].iter()).all(|(a, b)| match (a, b) {
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
    /// (el mismo orden que `param_names()`). Sin catch-all, requiere la
    /// MISMA cantidad de segmentos. Con catch-all, alcanza con que el path
    /// tenga al menos los segmentos fijos de antes -- el catch-all se lleva
    /// TODO lo que sobre (cero o más segmentos), unido con "/" en una sola
    /// captura (por eso el resultado es `Vec<String>` y no `Vec<&str>`: un
    /// `&str` prestado no puede representar varios segmentos originales
    /// unidos por algo que no estaba en el string de entrada).
    pub fn matches(&self, path_segments: &[&str]) -> Option<Vec<String>> {
        let fixed_len = self.fixed_len();
        if self.has_catchall() {
            if path_segments.len() < fixed_len {
                return None;
            }
        } else if path_segments.len() != fixed_len {
            return None;
        }
        let mut captured = Vec::new();
        for (seg, actual) in self.segments[..fixed_len].iter().zip(path_segments[..fixed_len].iter()) {
            match seg {
                Segment::Literal(expected) => {
                    if expected != actual {
                        return None;
                    }
                }
                Segment::Param(_) => captured.push((*actual).to_string()),
                Segment::CatchAll(_) => unreachable!("catch-all nunca cae dentro del prefijo fijo"),
            }
        }
        if self.has_catchall() {
            captured.push(path_segments[fixed_len..].join("/"));
        }
        Some(captured)
    }
}

/// Un `@route` es ESTÁTICO si su patrón no tiene ningún `:param` ni
/// catch-all -- o sea, si nombra exactamente UNA URL -- y PÚBLICO si el rpc
/// no lleva `@authenticated`/`@requires`. Solo esos pueden ir a un sitemap
/// (GRAMMAR.md §3.222): una ruta con parámetro nombra infinitas URLs que
/// solo el programa sabe cuáles existen, y una ruta con auth no debe
/// anunciarse a un crawler que nunca va a poder verla. Orden de
/// declaración. Compartida entre `Db` (para `staticRoutes()` en runtime) y
/// `linkc build` (para `routes.json`), así las dos vistas coinciden.
pub fn static_public_routes(program: &crate::ast::Program) -> Vec<String> {
    let mut out = Vec::new();
    for item in &program.items {
        let crate::ast::Item::Service(s) = item else { continue };
        for m in &s.members {
            let crate::ast::Member::Rpc(r) = m else { continue };
            let Some(raw) = r.route() else { continue };
            if raw.contains(':') || r.auth().is_some() {
                continue;
            }
            out.push(raw.to_string());
        }
    }
    out
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
    let last_index = parts.len() - 1;
    for (i, part) in parts.iter().enumerate() {
        if let Some(rest) = part.strip_prefix(':') {
            if let Some(name) = rest.strip_suffix('*') {
                // Catch-all (§3.42, ronda catch-all): solo tiene sentido al
                // final -- captura "lo que sobre", así que cualquier
                // segmento después de él sería inalcanzable siempre.
                if i != last_index {
                    return Err(format!(
                        "'{raw}': el segmento catch-all ':{name}*' solo puede ser el último segmento de la ruta (hay {} segmento(s) después)",
                        last_index - i
                    ));
                }
                if !is_valid_param_name(name) {
                    return Err(format!(
                        "'{raw}': ':{name}*' no es un nombre de catch-all válido (tiene que ser un identificador antes del '*': empezar con letra o '_', seguido de letras/dígitos/'_')"
                    ));
                }
                if seen_params.contains(&name) {
                    return Err(format!(
                        "'{raw}': ':{name}' aparece más de una vez -- cada parámetro de la ruta necesita un nombre distinto"
                    ));
                }
                seen_params.push(name);
                segments.push(Segment::CatchAll(name.to_string()));
                continue;
            }
            let name = rest;
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
        assert_eq!(r.matches(&["blog", "hola-mundo"]), Some(vec!["hola-mundo".to_string()]));
        assert_eq!(r.matches(&["blog"]), None);
        assert_eq!(r.matches(&["otra", "cosa"]), None);
    }

    #[test]
    fn matching_multiple_params_captures_them_in_pattern_order() {
        let r = parse_route_pattern("/blog/:categoria/:slug").unwrap();
        assert_eq!(r.matches(&["blog", "rust", "hola-mundo"]), Some(vec!["rust".to_string(), "hola-mundo".to_string()]));
        assert_eq!(r.matches(&["blog", "rust"]), None, "faltan segmentos");
    }

    #[test]
    fn parses_a_trailing_catchall() {
        let r = parse_route_pattern("/docs/:rest*").unwrap();
        assert_eq!(r.segments, vec![Segment::Literal("docs".to_string()), Segment::CatchAll("rest".to_string())]);
        assert_eq!(r.param_names(), vec!["rest"]);
        assert_eq!(r.catchall_name(), Some("rest"));
    }

    #[test]
    fn rejects_a_catchall_that_is_not_the_last_segment() {
        let e = parse_route_pattern("/docs/:rest*/more").unwrap_err();
        assert!(e.contains("último segmento"), "mensaje inesperado: {e}");
    }

    #[test]
    fn rejects_an_invalid_catchall_name() {
        assert!(parse_route_pattern("/docs/:*").is_err());
        assert!(parse_route_pattern("/docs/:123*").is_err());
    }

    #[test]
    fn a_catchall_repeated_with_a_normal_param_is_still_rejected_as_duplicate() {
        let e = parse_route_pattern("/:slug/x/:slug*").unwrap_err();
        assert!(e.contains("más de una vez"), "mensaje inesperado: {e}");
    }

    #[test]
    fn catchall_matches_zero_one_or_many_trailing_segments() {
        let r = parse_route_pattern("/docs/:rest*").unwrap();
        assert_eq!(r.matches(&["docs"]), Some(vec!["".to_string()]), "cero segmentos restantes -> string vacío");
        assert_eq!(r.matches(&["docs", "intro"]), Some(vec!["intro".to_string()]));
        assert_eq!(r.matches(&["docs", "api", "v2", "users"]), Some(vec!["api/v2/users".to_string()]));
        assert_eq!(r.matches(&["other"]), None, "el prefijo fijo tiene que matchear igual");
    }

    #[test]
    fn a_lone_catchall_matches_any_path_including_the_root_segment() {
        let r = parse_route_pattern("/:rest*").unwrap();
        assert_eq!(r.matches(&["a", "b", "c"]), Some(vec!["a/b/c".to_string()]));
        assert_eq!(r.matches(&["a"]), Some(vec!["a".to_string()]));
    }

    #[test]
    fn a_literal_route_is_more_specific_than_an_overlapping_catchall() {
        let literal = parse_route_pattern("/docs/changelog").unwrap();
        let catchall = parse_route_pattern("/docs/:rest*").unwrap();
        assert!(literal.specificity() > catchall.specificity());
        // Podrían matchear el mismo path ("/docs/changelog"), pero la
        // especificidad distinta significa que NO es un conflicto real --
        // la literal gana determinísticamente, mismo criterio que un
        // literal contra un `:param` normal.
        assert!(!literal.conflicts_with(&catchall));
    }

    #[test]
    fn two_catchalls_with_different_fixed_prefixes_conflict_if_the_shorter_prefix_is_compatible() {
        let a = parse_route_pattern("/docs/:rest*").unwrap();
        let b = parse_route_pattern("/docs/:section/:rest*").unwrap();
        // Ambos tienen 1 segmento literal fijo ("docs") -- empatados en
        // especificidad, y ambos podrían matchear "/docs/x/y": conflicto.
        assert!(a.conflicts_with(&b));
        assert!(b.conflicts_with(&a));
    }

    #[test]
    fn a_catchall_does_not_conflict_with_an_incompatible_literal_prefix() {
        let a = parse_route_pattern("/docs/:rest*").unwrap();
        let b = parse_route_pattern("/blog/:rest*").unwrap();
        assert!(!a.conflicts_with(&b), "prefijos literales distintos ('docs' vs 'blog') nunca se cruzan");
    }
}
