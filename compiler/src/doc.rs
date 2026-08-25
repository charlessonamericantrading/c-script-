// Generador de documentación HTML estática para programas Link (`linkc doc`).
// Produce un portal técnico moderno, responsivo y autónomo con la especificación completa
// de tipos, servicios, RPCs, anotaciones de autenticación y funciones.

use crate::ast::*;
use std::path::Path;

/// Genera el documento HTML completo a partir de un `Program` parseado.
pub fn generate_html(program: &Program, file_name: &str) -> String {
    let title = Path::new(file_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(file_name);

    let mut types_html = String::new();
    let mut enums_html = String::new();
    let mut services_html = String::new();
    let mut fns_html = String::new();
    let mut consts_html = String::new();

    let mut type_nav = String::new();
    let mut enum_nav = String::new();
    let mut service_nav = String::new();
    let mut fn_nav = String::new();

    for item in &program.items {
        match item {
            Item::Type(t) => {
                let name = &t.name;
                type_nav.push_str(&format!("<li><a href=\"#type-{}\">{}</a></li>", name, name));
                types_html.push_str(&render_type_decl(t));
            }
            Item::Enum(e) => {
                let name = &e.name;
                enum_nav.push_str(&format!("<li><a href=\"#enum-{}\">{}</a></li>", name, name));
                enums_html.push_str(&render_enum_decl(e));
            }
            Item::Service(s) => {
                let name = &s.name;
                service_nav.push_str(&format!("<li><a href=\"#service-{}\">{}</a></li>", name, name));
                services_html.push_str(&render_service(s));
            }
            Item::Fn(f) => {
                let name = &f.name;
                fn_nav.push_str(&format!("<li><a href=\"#fn-{}\">{}()</a></li>", name, name));
                fns_html.push_str(&render_fn(f));
            }
            Item::Const(c) => {
                consts_html.push_str(&render_const(c));
            }
            _ => {}
        }
    }

    let service_nav_section = nav_section("Servicios", &service_nav);
    let type_nav_section = nav_section("Tipos y Structs", &type_nav);
    let enum_nav_section = nav_section("Enums", &enum_nav);
    let fn_nav_section = nav_section("Funciones", &fn_nav);

    let services_section = wrap_section("Servicios y Endpoints RPC", &services_html);
    let types_section = wrap_section("Definiciones de Tipos", &types_html);
    let enums_section = wrap_section("Enums", &enums_html);
    let fns_section = wrap_section("Funciones", &fns_html);
    let consts_section = wrap_section("Constantes", &consts_html);

    format!(
        r#"<!DOCTYPE html>
<html lang="es">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>{title} — Documentación API (Link)</title>
  <style>
    :root {{
      --bg: #0d1117;
      --card-bg: #161b22;
      --border: #30363d;
      --text: #c9d1d9;
      --heading: #f0f6fc;
      --accent: #58a6ff;
      --accent-badge: rgba(56, 139, 253, 0.15);
      --auth-badge: rgba(210, 153, 34, 0.15);
      --auth-text: #e3b341;
      --tag-bg: #21262d;
      --code-bg: #090d13;
      --font-mono: ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, monospace;
      --font-sans: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
    }}
    * {{ box-sizing: border-box; margin: 0; padding: 0; }}
    body {{
      background-color: var(--bg);
      color: var(--text);
      font-family: var(--font-sans);
      line-height: 1.6;
      display: flex;
      min-height: 100vh;
    }}
    sidebar {{
      width: 280px;
      background: var(--card-bg);
      border-right: 1px solid var(--border);
      padding: 2rem 1.5rem;
      position: sticky;
      top: 0;
      height: 100vh;
      overflow-y: auto;
    }}
    sidebar h2 {{
      font-size: 1.1rem;
      color: var(--heading);
      margin-bottom: 0.5rem;
      text-transform: uppercase;
      letter-spacing: 0.05em;
    }}
    sidebar .brand {{
      font-size: 1.4rem;
      font-weight: 700;
      color: var(--accent);
      margin-bottom: 1.5rem;
      display: flex;
      align-items: center;
      gap: 0.5rem;
    }}
    sidebar ul {{ list-style: none; margin-bottom: 1.5rem; }}
    sidebar li {{ margin-bottom: 0.35rem; }}
    sidebar a {{
      color: var(--text);
      text-decoration: none;
      font-size: 0.9rem;
      transition: color 0.15s;
    }}
    sidebar a:hover {{ color: var(--accent); }}
    main {{
      flex: 1;
      max-width: 900px;
      padding: 3rem 2.5rem;
    }}
    header.main-header {{
      margin-bottom: 3rem;
      padding-bottom: 1.5rem;
      border-bottom: 1px solid var(--border);
    }}
    header.main-header h1 {{
      font-size: 2.2rem;
      color: var(--heading);
      margin-bottom: 0.5rem;
    }}
    .badge {{
      display: inline-block;
      padding: 0.2rem 0.6rem;
      border-radius: 9999px;
      font-size: 0.75rem;
      font-weight: 600;
      background: var(--accent-badge);
      color: var(--accent);
      border: 1px solid rgba(56, 139, 253, 0.3);
    }}
    .auth-badge {{
      background: var(--auth-badge);
      color: var(--auth-text);
      border: 1px solid rgba(227, 179, 65, 0.3);
    }}
    section {{ margin-bottom: 3.5rem; }}
    h2.section-title {{
      font-size: 1.5rem;
      color: var(--heading);
      margin-bottom: 1.5rem;
      padding-bottom: 0.5rem;
      border-bottom: 1px solid var(--border);
    }}
    .card {{
      background: var(--card-bg);
      border: 1px solid var(--border);
      border-radius: 8px;
      padding: 1.5rem;
      margin-bottom: 1.5rem;
    }}
    .card-header {{
      display: flex;
      align-items: center;
      justify-content: space-between;
      margin-bottom: 1rem;
    }}
    .card-title {{
      font-size: 1.2rem;
      font-weight: 600;
      color: var(--heading);
      font-family: var(--font-mono);
    }}
    pre.code-block {{
      background: var(--code-bg);
      padding: 1rem;
      border-radius: 6px;
      overflow-x: auto;
      font-family: var(--font-mono);
      font-size: 0.88rem;
      border: 1px solid var(--border);
      margin-top: 0.75rem;
    }}
    table.params-table {{
      width: 100%;
      border-collapse: collapse;
      margin-top: 1rem;
      font-size: 0.9rem;
    }}
    table.params-table th, table.params-table td {{
      padding: 0.6rem 0.8rem;
      text-align: left;
      border-bottom: 1px solid var(--border);
    }}
    table.params-table th {{
      color: var(--heading);
      font-weight: 600;
      background: var(--tag-bg);
    }}
    .type-pill {{
      font-family: var(--font-mono);
      font-size: 0.8rem;
      color: var(--accent);
    }}
  </style>
</head>
<body>
  <sidebar>
    <div class="brand">⚡ Link Docs</div>
    {service_nav_section}
    {type_nav_section}
    {enum_nav_section}
    {fn_nav_section}
  </sidebar>
  <main>
    <header class="main-header">
      <span class="badge">Link v1.0 Contrato API</span>
      <h1>{title}</h1>
      <p>Documentación técnica autogenerada por <code>linkc doc</code>.</p>
    </header>

    {services_section}
    {types_section}
    {enums_section}
    {fns_section}
    {consts_section}
  </main>
</body>
</html>
"#
    )
}

fn nav_section(title: &str, items: &str) -> String {
    if items.is_empty() {
        return String::new();
    }
    format!("<h2>{title}</h2><ul>{items}</ul>")
}

fn wrap_section(title: &str, content: &str) -> String {
    if content.is_empty() {
        return String::new();
    }
    format!(r#"<section><h2 class="section-title">{title}</h2>{content}</section>"#)
}

fn render_type_decl(t: &TypeDecl) -> String {
    let name = &t.name;
    let mut fields_table = String::new();
    match &t.ty {
        TypeExpr::Struct(fields) => {
            fields_table.push_str(r#"<table class="params-table"><thead><tr><th>Campo</th><th>Tipo</th><th>Requerido</th></tr></thead><tbody>"#);
            for f in fields {
                let req = if f.optional { "Opcional" } else { "Requerido" };
                let ty_str = format!("{:?}", f.ty);
                fields_table.push_str(&format!(
                    r#"<tr><td><code>{}</code></td><td><span class="type-pill">{}</span></td><td>{}</td></tr>"#,
                    f.name, ty_str, req
                ));
            }
            fields_table.push_str("</tbody></table>");
        }
        _ => {}
    }
    format!(
        r#"<div class="card" id="type-{name}">
  <div class="card-header">
    <div class="card-title">type {name}</div>
  </div>
  {fields_table}
</div>"#
    )
}

fn render_enum_decl(e: &EnumDecl) -> String {
    let name = &e.name;
    let mut variants_html = String::new();
    for v in &e.variants {
        variants_html.push_str(&format!(r#"<li><code>{}</code></li>"#, v.name));
    }
    format!(
        r#"<div class="card" id="enum-{name}">
  <div class="card-header">
    <div class="card-title">enum {name}</div>
  </div>
  <p style="margin-bottom:0.5rem; font-weight:500;">Variantes:</p>
  <ul style="padding-left: 1.5rem; font-family: var(--font-mono); font-size: 0.9rem;">
    {variants_html}
  </ul>
</div>"#
    )
}

fn render_service(s: &ServiceDecl) -> String {
    let name = &s.name;
    let mut rpcs_html = String::new();
    for member in &s.members {
        match member {
            Member::Rpc(r) => {
                let rpc_name = &r.name;
                let auth_badge = match r.auth() {
                    Some(Annotation::Requires { enum_name, variant_names }) => {
                        let roles = variant_names.iter().map(|v| format!("{enum_name}.{v}")).collect::<Vec<_>>().join(" | ");
                        format!(r#"<span class="badge auth-badge">🔒 @requires({roles})</span>"#)
                    }
                    Some(Annotation::Authenticated) => {
                        r#"<span class="badge auth-badge">🔒 @authenticated</span>"#.to_string()
                    }
                    // `auth()` nunca devuelve ContentType, Route, RateLimit,
                    // Deprecated, CacheControl, Example, Invalidates,
                    // Infinite ni Idempotent; el brazo existe para que
                    // agregar una anotación nueva rompa acá y no pase de
                    // largo mostrando "Público" por descarte.
                    Some(Annotation::ContentType(_))
                    | Some(Annotation::Route(_))
                    | Some(Annotation::RateLimit { .. })
                    | Some(Annotation::Deprecated(_))
                    | Some(Annotation::CacheControl(_))
                    | Some(Annotation::Example { .. })
                    | Some(Annotation::Invalidates(_))
                    | Some(Annotation::Infinite { .. })
                    | Some(Annotation::Idempotent)
                    | Some(Annotation::Cache(_))
                    | Some(Annotation::Cors(_))
                    | None => r#"<span class="badge">🌐 Público</span>"#.to_string(),
                };
                let rate_limit_badge = match r.rate_limit() {
                    Some((spec, Some(key_param))) => format!(r#"<span class="badge">⏱️ @rate_limit("{spec}", key: {key_param})</span>"#),
                    Some((spec, None)) => format!(r#"<span class="badge">⏱️ @rate_limit("{spec}")</span>"#),
                    None => String::new(),
                };
                let content_type_badge = match r.content_type() {
                    Some(ct) => format!(r#"<span class="badge">📄 {ct}</span>"#),
                    None => String::new(),
                };
                let route_badge = match r.route() {
                    Some(pattern) => format!(r#"<span class="badge">🔗 {pattern}</span>"#),
                    None => String::new(),
                };
                let deprecated_badge = match r.deprecated() {
                    Some(reason) => format!(r#"<span class="badge" style="background:#7a1f1f;">⚠️ deprecated: {reason}</span>"#),
                    None => String::new(),
                };
                let auth_badge = format!("{auth_badge}{content_type_badge}{route_badge}{rate_limit_badge}{deprecated_badge}");

                let mut params_str = Vec::new();
                let mut params_table = String::new();
                if !r.params.is_empty() {
                    params_table.push_str(r#"<table class="params-table"><thead><tr><th>Parámetro</th><th>Tipo</th></tr></thead><tbody>"#);
                    for p in &r.params {
                        params_str.push(format!("{}: {:?}", p.name, p.ty));
                        params_table.push_str(&format!(
                            r#"<tr><td><code>{}</code></td><td><span class="type-pill">{:?}</span></td></tr>"#,
                            p.name, p.ty
                        ));
                    }
                    params_table.push_str("</tbody></table>");
                }

                let ret_ty = format!("{:?}", r.return_type);
                let signature = format!("rpc {rpc_name}({}) -> {ret_ty}", params_str.join(", "));

                rpcs_html.push_str(&format!(
                    r#"<div style="margin-top: 1.5rem; border-top: 1px solid var(--border); padding-top: 1rem;">
  <div style="display:flex; justify-content:space-between; align-items:center; margin-bottom:0.5rem;">
    <h4 style="font-family:var(--font-mono); color:var(--heading);">{name}.{rpc_name}</h4>
    {auth_badge}
  </div>
  <pre class="code-block">{signature}</pre>
  {params_table}
</div>"#
                ));
            }
            Member::Stream(st) => {
                let stream_name = &st.name;
                let signature = format!("stream {stream_name}() -> {:?}", st.return_type);
                rpcs_html.push_str(&format!(
                    r#"<div style="margin-top: 1.5rem; border-top: 1px solid var(--border); padding-top: 1rem;">
  <div style="display:flex; justify-content:space-between; align-items:center; margin-bottom:0.5rem;">
    <h4 style="font-family:var(--font-mono); color:var(--heading);">{name}.{stream_name} (SSE Stream)</h4>
    <span class="badge">📡 Realtime</span>
  </div>
  <pre class="code-block">{signature}</pre>
</div>"#
                ));
            }
        }
    }

    format!(
        r#"<div class="card" id="service-{name}">
  <div class="card-header">
    <div class="card-title">service {name}</div>
  </div>
  {rpcs_html}
</div>"#
    )
}

fn render_fn(f: &FnDecl) -> String {
    let name = &f.name;
    let params: Vec<String> = f.params.iter().map(|p| format!("{}: {:?}", p.name, p.ty)).collect();
    let ret_ty = format!("{:?}", f.return_type);
    let signature = format!("fn {name}({}) -> {ret_ty}", params.join(", "));
    format!(
        r#"<div class="card" id="fn-{name}">
  <div class="card-header">
    <div class="card-title">{name}</div>
  </div>
  <pre class="code-block">{signature}</pre>
</div>"#
    )
}

fn render_const(c: &ConstDecl) -> String {
    let name = &c.name;
    format!(
        r#"<div class="card">
  <div class="card-header">
    <div class="card-title">const {name}: {:?}</div>
  </div>
</div>"#,
        c.ty
    )
}
