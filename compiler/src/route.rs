//! Parseo y matching de patrones `@route("/blog/:slug")` (GRAMMAR.md §3.37).
//!
//! Un único parser acá, usado TANTO por el checker (validar en compilación)
//! COMO por el servidor (despachar en runtime) -- la razón de que exista este
//! módulo aparte en vez de tener la lógica duplicada en `checker.rs` y
//! `runtime/server.rs` es exactamente la que este proyecto ya viene
//! documentando desde GRAMMAR.md §3.9: dos capas que implementan la misma
//! regla por separado terminan divergiendo. Acá hay una sola fuente de
//! verdad de qué es un patrón válido y qué significa que matchee un path
//! real.
//!
//! Alcance v0, a propósito acotado: como mucho UN segmento parametrizado, y
//! tiene que ser el ÚLTIMO (`/producto/:id`, nunca `/:categoria/:slug`). Es
//! justo lo que necesita el caso que motivó esto -- una URL humana por
//! página, típico de contenido SEO -- sin abrir todavía la complejidad real
//! de resolver ambigüedad entre patrones con MÁS de un segmento dinámico.

/// Un patrón de ruta ya parseado y validado en su FORMA (no valida que el rpc
/// tenga el parámetro correspondiente -- eso lo hace el checker, que es quien
/// tiene el `Vec<Param>` del rpc a mano).
#[derive(Debug, Clone, PartialEq)]
pub struct RoutePattern {
    /// Segmentos literales, en orden, ANTES del posible parámetro final.
    /// Puede estar vacío (`@route("/:slug")`, un solo segmento dinámico).
    pub literal_prefix: Vec<String>,
    /// Nombre del parámetro si el último segmento es `:nombre`; `None` si
    /// la ruta es puramente literal (ej. `@route("/sitemap.xml")`).
    pub param_name: Option<String>,
}

impl RoutePattern {
    /// Cuántos segmentos tiene esta ruta en total -- lo que un path real
    /// necesita tener para siquiera candidatear a matchear.
    pub fn segment_count(&self) -> usize {
        self.literal_prefix.len() + usize::from(self.param_name.is_some())
    }

    /// Compara SOLO la forma (segmentos literales + si termina en param),
    /// ignorando el NOMBRE del parámetro -- dos rutas con esta misma forma
    /// son indistinguibles al despachar una request real, sin importar
    /// cómo se llame el parámetro de cada una (`:slug` vs `:id`). Es
    /// exactamente el criterio de conflicto que usa el checker.
    pub fn same_shape(&self, other: &RoutePattern) -> bool {
        self.literal_prefix == other.literal_prefix && self.param_name.is_some() == other.param_name.is_some()
    }

    /// Intenta matchear los segmentos de un path real (ya partido por "/",
    /// sin el "" inicial de un path que empieza con "/"). Devuelve el valor
    /// crudo (sin decodificar, sin tipar) del segmento capturado por el
    /// parámetro, si esta ruta tiene uno.
    pub fn matches<'p>(&self, path_segments: &[&'p str]) -> Option<Option<&'p str>> {
        if path_segments.len() != self.segment_count() {
            return None;
        }
        for (expected, actual) in self.literal_prefix.iter().zip(path_segments.iter()) {
            if expected != actual {
                return None;
            }
        }
        match &self.param_name {
            Some(_) => Some(path_segments.last().copied()),
            None => Some(None),
        }
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
/// - A LO SUMO el último segmento puede ser un parámetro -- ninguno antes.
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

    let mut literal_prefix = Vec::with_capacity(parts.len());
    let mut param_name = None;
    for (i, part) in parts.iter().enumerate() {
        let is_last = i == parts.len() - 1;
        if let Some(name) = part.strip_prefix(':') {
            if !is_valid_param_name(name) {
                return Err(format!(
                    "'{raw}': ':{name}' no es un nombre de parámetro válido (tiene que ser un identificador: empezar con letra o '_', seguido de letras/dígitos/'_')"
                ));
            }
            if !is_last {
                return Err(format!(
                    "'{raw}': solo el ÚLTIMO segmento puede ser un parámetro (':{name}' está en el medio) -- v0 no soporta más de un segmento dinámico por ruta, ver GRAMMAR.md §3.37"
                ));
            }
            param_name = Some(name.to_string());
        } else {
            literal_prefix.push((*part).to_string());
        }
    }

    Ok(RoutePattern { literal_prefix, param_name })
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
        assert_eq!(r.literal_prefix, vec!["sitemap.xml"]);
        assert_eq!(r.param_name, None);
        assert_eq!(r.segment_count(), 1);
    }

    #[test]
    fn parses_a_trailing_param() {
        let r = parse_route_pattern("/blog/:slug").unwrap();
        assert_eq!(r.literal_prefix, vec!["blog"]);
        assert_eq!(r.param_name, Some("slug".to_string()));
        assert_eq!(r.segment_count(), 2);
    }

    #[test]
    fn a_lone_param_has_an_empty_prefix() {
        let r = parse_route_pattern("/:slug").unwrap();
        assert_eq!(r.literal_prefix, Vec::<String>::new());
        assert_eq!(r.param_name, Some("slug".to_string()));
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
    fn rejects_a_param_that_is_not_the_last_segment() {
        let e = parse_route_pattern("/:categoria/:slug").unwrap_err();
        assert!(e.contains("ÚLTIMO"), "mensaje inesperado: {e}");
    }

    #[test]
    fn rejects_an_invalid_param_name() {
        assert!(parse_route_pattern("/blog/:123").is_err());
        assert!(parse_route_pattern("/blog/:").is_err());
    }

    #[test]
    fn same_shape_ignores_the_param_name() {
        let a = parse_route_pattern("/blog/:slug").unwrap();
        let b = parse_route_pattern("/blog/:id").unwrap();
        assert!(a.same_shape(&b));
    }

    #[test]
    fn same_shape_distinguishes_literal_from_param_at_the_same_position() {
        let literal = parse_route_pattern("/blog/featured").unwrap();
        let param = parse_route_pattern("/blog/:slug").unwrap();
        assert!(!literal.same_shape(&param));
    }

    #[test]
    fn matching_a_literal_route_requires_an_exact_match() {
        let r = parse_route_pattern("/sitemap.xml").unwrap();
        assert_eq!(r.matches(&["sitemap.xml"]), Some(None));
        assert_eq!(r.matches(&["robots.txt"]), None);
        assert_eq!(r.matches(&["sitemap.xml", "extra"]), None);
    }

    #[test]
    fn matching_a_param_route_captures_the_last_segment() {
        let r = parse_route_pattern("/blog/:slug").unwrap();
        assert_eq!(r.matches(&["blog", "hola-mundo"]), Some(Some("hola-mundo")));
        assert_eq!(r.matches(&["blog"]), None);
        assert_eq!(r.matches(&["otra", "cosa"]), None);
    }
}
