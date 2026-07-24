// Emisor de contrato: el único pass compartido que produce tanto el
// `.d.ts` como el `client.ts` (PLAN.md §3.3) — así el servidor y el cliente
// no pueden divergir, porque ambos salen del mismo `render_type`.
//
// Sigue la tabla de mapeo de GRAMMAR.md §4 al pie de la letra. `fn` no se
// emite: es lógica interna del backend, no parte del contrato (GRAMMAR.md
// nota sobre fn_decl en §2.1).

use crate::ast::*;
use crate::checker::Checker;
use crate::types::Type;

pub fn emit_contract(program: &Program) -> Result<String, String> {
    let (checker, errors) = Checker::build_symbols(program);
    if let Some(e) = errors.into_iter().next() {
        return Err(e.to_string());
    }

    let mut out = String::new();
    out.push_str("// Generado automáticamente por linkc — no editar a mano.\n\n");
    out.push_str("export type Result<T, E> = { type: \"Ok\"; value: T } | { type: \"Err\"; error: E };\n");
    // Partial<T> de TS YA implementa la semántica de Patch<T> de GRAMMAR.md §3.4:
    // un campo `x: T?` (=> `x: T | null`) se vuelve `x?: T | null` (omitir = no
    // tocar, null = limpiar, valor = fijar); un campo `x?: T` se queda `x?: T`
    // (no se puede limpiar, coherente con que nunca fue nullable). No hace
    // falta un mapped type a mano.
    out.push_str("export type Patch<T> = Partial<T>;\n\n");

    for item in &program.items {
        match item {
            Item::Type(t) => emit_type_decl(&mut out, t, &checker)?,
            Item::Enum(e) => emit_enum_decl(&mut out, e, &checker)?,
            _ => {}
        }
    }
    for item in &program.items {
        if let Item::Service(s) = item {
            emit_service_interface(&mut out, s, &checker)?;
        }
    }
    Ok(out)
}

pub fn emit_client(program: &Program) -> Result<String, String> {
    let (checker, errors) = Checker::build_symbols(program);
    if let Some(e) = errors.into_iter().next() {
        return Err(e.to_string());
    }

    let mut out = String::new();
    out.push_str("// Generado automáticamente por linkc — no editar a mano.\n\n");

    let Some(service) = program.items.iter().find_map(|i| match i {
        Item::Service(s) => Some(s),
        _ => None,
    }) else {
        return Ok(out); // sin service declarado, no hay cliente que generar
    };

    // Primera pasada: resolver todas las firmas para (a) saber qué importar
    // de "./contract" y (b) no resolver dos veces lo mismo en la segunda.
    let mut imported_names = std::collections::BTreeSet::new();
    let mut resolved: Vec<(&RpcDecl, bool, Vec<Type>, Type)> = Vec::new();
    for m in &service.members {
        let (rpc, is_stream) = match m {
            Member::Rpc(r) => (r, false),
            Member::Stream(r) => (r, true),
        };
        let mut param_tys = Vec::new();
        for p in &rpc.params {
            let ty = checker.resolve_type(&p.ty).map_err(|e| e.to_string())?;
            collect_type_names(&ty, &mut imported_names);
            param_tys.push(ty);
        }
        let ret_ty = checker.resolve_type(&rpc.return_type).map_err(|e| e.to_string())?;
        collect_type_names(&ret_ty, &mut imported_names);
        resolved.push((rpc, is_stream, param_tys, ret_ty));
    }
    imported_names.insert(format!("{}Client", service.name));

    out.push_str(&format!(
        "import type {{ {} }} from \"./contract\";\n\n",
        imported_names.into_iter().collect::<Vec<_>>().join(", ")
    ));
    // Errores de transporte vs de dominio (GRAMMAR.md §3.5): esta excepción es
    // SOLO para fallos de infraestructura (red, 5xx, timeout) — los errores de
    // dominio que un rpc declaró en su Result<T,E> siempre vuelven como valor,
    // nunca se lanzan.
    out.push_str("export class LinkTransportError extends Error {}\n\n");

    out.push_str(&format!("class {name}ClientImpl implements {name}Client {{\n", name = service.name));
    // Constructor explícito, no "parameter property" (`private x: T` en la
    // firma) -- esa azúcar de TS no la entienden strip-only transpilers
    // (soporte nativo de Node, esbuild en modo transform), y el código
    // generado debería ser legible por el mayor número posible de toolchains.
    out.push_str("  private baseUrl: string;\n");
    out.push_str("  constructor(baseUrl: string) {\n    this.baseUrl = baseUrl;\n  }\n\n");

    for (rpc, is_stream, param_tys, ret_ty) in &resolved {
        if *is_stream {
            // Streaming real (SSE/WS) es Fase 1 (PLAN.md §4) — el método
            // queda en la interfaz del contrato pero no implementado acá.
            out.push_str(&format!(
                "  async *{name}(): AsyncIterable<unknown> {{\n    throw new Error(\"streaming no implementado en el MVP (Fase 0)\");\n  }}\n\n",
                name = rpc.name
            ));
            continue;
        }

        let params: Vec<String> = rpc
            .params
            .iter()
            .zip(param_tys)
            .map(|(p, ty)| {
                format!(
                    "{}{}: {}",
                    p.name,
                    if p.default.is_some() { "?" } else { "" },
                    render_type(ty)
                )
            })
            .collect();
        let arg_names: Vec<&str> = rpc.params.iter().map(|p| p.name.as_str()).collect();

        out.push_str(&format!(
            "  async {}({}): Promise<{}> {{\n",
            rpc.name,
            params.join(", "),
            render_type(ret_ty)
        ));
        out.push_str(&format!(
            "    const res = await fetch(`${{this.baseUrl}}/{}/{}`, {{\n",
            service.name, rpc.name
        ));
        out.push_str("      method: \"POST\",\n");
        out.push_str("      headers: { \"Content-Type\": \"application/json\" },\n");
        out.push_str(&format!("      body: JSON.stringify({{ {} }}),\n", arg_names.join(", ")));
        out.push_str("    });\n");
        out.push_str("    if (!res.ok) throw new LinkTransportError(`HTTP ${res.status}`);\n");
        out.push_str("    return res.json();\n");
        out.push_str("  }\n\n");
    }
    out.push_str("}\n\n");

    out.push_str(&format!(
        "export function create{name}Client(baseUrl: string): {name}Client {{\n  return new {name}ClientImpl(baseUrl);\n}}\n",
        name = service.name
    ));

    Ok(out)
}

fn emit_type_decl(out: &mut String, t: &TypeDecl, checker: &Checker) -> Result<(), String> {
    if !t.type_params.is_empty() {
        return Err(format!(
            "'{}' es genérico — emisión de type/enum genéricos declarados por el usuario aún no soportada (PLAN.md §3.6)",
            t.name
        ));
    }
    match &t.ty {
        TypeExpr::Struct(fields) => {
            out.push_str(&format!("export interface {} {{\n", t.name));
            for f in fields {
                let ty = checker.resolve_type(&f.ty).map_err(|e| e.to_string())?;
                out.push_str(&format!(
                    "  {}{}: {};\n",
                    f.name,
                    if f.optional { "?" } else { "" },
                    render_type(&ty)
                ));
            }
            out.push_str("}\n\n");
        }
        other => {
            let ty = checker.resolve_type(other).map_err(|e| e.to_string())?;
            out.push_str(&format!("export type {} = {};\n\n", t.name, render_type(&ty)));
        }
    }
    Ok(())
}

fn emit_enum_decl(out: &mut String, e: &EnumDecl, checker: &Checker) -> Result<(), String> {
    if !e.type_params.is_empty() {
        return Err(format!(
            "'{}' es genérico — emisión de enums genéricos aún no soportada (PLAN.md §3.6)",
            e.name
        ));
    }
    let all_unit = e.variants.iter().all(|v| v.fields.is_none());
    if all_unit {
        // enum simple -> unión de literales string (GRAMMAR.md §4)
        let variants: Vec<String> = e.variants.iter().map(|v| format!("\"{}\"", v.name)).collect();
        out.push_str(&format!("export type {} = {};\n\n", e.name, variants.join(" | ")));
        return Ok(());
    }
    // ADT -> unión discriminada con tag `type` (GRAMMAR.md §4)
    out.push_str(&format!("export type {} =\n", e.name));
    for v in &e.variants {
        let mut parts = vec![format!("type: \"{}\"", v.name)];
        if let Some(fields) = &v.fields {
            for f in fields {
                let ty = checker.resolve_type(&f.ty).map_err(|e| e.to_string())?;
                parts.push(format!(
                    "{}{}: {}",
                    f.name,
                    if f.optional { "?" } else { "" },
                    render_type(&ty)
                ));
            }
        }
        out.push_str(&format!("  | {{ {} }}\n", parts.join("; ")));
    }
    out.push_str(";\n\n");
    Ok(())
}

fn emit_service_interface(out: &mut String, s: &ServiceDecl, checker: &Checker) -> Result<(), String> {
    out.push_str(&format!("export interface {}Client {{\n", s.name));
    for m in &s.members {
        let (rpc, is_stream) = match m {
            Member::Rpc(r) => (r, false),
            Member::Stream(r) => (r, true),
        };
        let mut params = Vec::new();
        for p in &rpc.params {
            let ty = checker.resolve_type(&p.ty).map_err(|e| e.to_string())?;
            // parámetro con default -> opcional en la firma TS (GRAMMAR.md §4)
            params.push(format!(
                "{}{}: {}",
                p.name,
                if p.default.is_some() { "?" } else { "" },
                render_type(&ty)
            ));
        }
        let ret_ty = checker.resolve_type(&rpc.return_type).map_err(|e| e.to_string())?;
        let ret_str = if is_stream {
            format!("AsyncIterable<{}>", render_type(&ret_ty))
        } else {
            format!("Promise<{}>", render_type(&ret_ty))
        };
        out.push_str(&format!("  {}({}): {};\n", rpc.name, params.join(", "), ret_str));
    }
    out.push_str("}\n\n");
    Ok(())
}

/// Type resuelto -> string TypeScript, siguiendo GRAMMAR.md §4 al pie de la letra.
fn render_type(ty: &Type) -> String {
    match ty {
        Type::Int | Type::Float => "number".to_string(),
        Type::String => "string".to_string(),
        Type::Bool => "boolean".to_string(),
        Type::Void => "void".to_string(),
        Type::Null => "null".to_string(),
        Type::Optional(inner) => format!("{} | null", render_type(inner)),
        Type::List(inner) => format!("{}[]", render_type_atom(inner)),
        Type::Tuple(items) => format!(
            "[{}]",
            items.iter().map(render_type).collect::<Vec<_>>().join(", ")
        ),
        Type::Function(params, ret) => {
            let ps: Vec<String> = params
                .iter()
                .enumerate()
                .map(|(i, p)| format!("arg{i}: {}", render_type(p)))
                .collect();
            format!("({}) => {}", ps.join(", "), render_type(ret))
        }
        Type::Struct { name: Some(n), .. } => n.clone(),
        Type::Struct { name: None, fields } => {
            let fs: Vec<String> = fields
                .iter()
                .map(|f| {
                    format!(
                        "{}{}: {}",
                        f.name,
                        if f.optional { "?" } else { "" },
                        render_type(&f.ty)
                    )
                })
                .collect();
            format!("{{ {} }}", fs.join("; "))
        }
        Type::Enum(name) => name.clone(),
        Type::ResultOf(a, b) => format!("Result<{}, {}>", render_type(a), render_type(b)),
        Type::PatchOf(inner) => format!("Patch<{}>", render_type(inner)),
        // `Record<K,V>` es un utility type NATIVO de TS -- a diferencia de
        // Result/Patch, no hace falta definirlo en el preámbulo del
        // contrato ni importarlo (ver collect_type_names).
        Type::MapOf(k, v) => format!("Record<{}, {}>", render_type(k), render_type(v)),
        Type::Dynamic => "unknown".to_string(),
    }
}

/// Nombres de tipos declarados (structs/enums) y builtins (Result/Patch)
/// referenciados por `ty`, para saber qué importar de "./contract" en
/// client.ts. Los tipos estructurales (Optional/List/Tuple/Function) no
/// tienen nombre propio — solo se recorren para encontrar los que sí.
fn collect_type_names(ty: &Type, names: &mut std::collections::BTreeSet<String>) {
    match ty {
        Type::Struct { name: Some(n), .. } => {
            names.insert(n.clone());
        }
        Type::Struct { name: None, fields } => {
            for f in fields {
                collect_type_names(&f.ty, names);
            }
        }
        Type::Enum(n) => {
            names.insert(n.clone());
        }
        Type::ResultOf(a, b) => {
            names.insert("Result".to_string());
            collect_type_names(a, names);
            collect_type_names(b, names);
        }
        Type::PatchOf(inner) => {
            names.insert("Patch".to_string());
            collect_type_names(inner, names);
        }
        // Record<K,V> es nativo de TS -- no se agrega "Map"/"Record" a los
        // imports, solo se recorre K y V por si referencian algo propio.
        Type::MapOf(k, v) => {
            collect_type_names(k, names);
            collect_type_names(v, names);
        }
        Type::Optional(inner) | Type::List(inner) => collect_type_names(inner, names),
        Type::Tuple(items) => {
            for i in items {
                collect_type_names(i, names);
            }
        }
        Type::Function(params, ret) => {
            for p in params {
                collect_type_names(p, names);
            }
            collect_type_names(ret, names);
        }
        Type::Int | Type::Float | Type::String | Type::Bool | Type::Void | Type::Null | Type::Dynamic => {}
    }
}

/// `T[]` con `T = A | null` daría `A | null[]` — que TS parsea como `A |
/// (null[])`, no como `(A | null)[]`. Se envuelve en paréntesis cualquier
/// tipo cuya forma renderizada use `|` o `=>` en su nivel superior.
fn render_type_atom(ty: &Type) -> String {
    match ty {
        Type::Optional(_) | Type::Function(_, _) => format!("({})", render_type(ty)),
        _ => render_type(ty),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::parse;

    fn emit_both(src: &str) -> (String, String) {
        let tokens = tokenize(src).unwrap_or_else(|e| panic!("{e}"));
        let program = parse(tokens).unwrap_or_else(|e| panic!("{e}"));
        let contract = emit_contract(&program).unwrap_or_else(|e| panic!("{e}"));
        let client = emit_client(&program).unwrap_or_else(|e| panic!("{e}"));
        (contract, client)
    }

    fn users_demo_src() -> String {
        std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/users.link"),
        )
        .expect("no se pudo leer examples/users.link")
    }

    #[test]
    fn struct_emits_interface_with_correct_optionality() {
        let (contract, _) = emit_both(&users_demo_src());
        assert!(contract.contains("export interface User {"));
        assert!(contract.contains("bio?: string;")); // x?: T -- clave ausente
        assert!(contract.contains("deletedAt: string | null;")); // x: T? -- clave presente, valor null
        assert!(contract.contains("role: Role;"));
    }

    #[test]
    fn simple_enum_emits_string_union() {
        let (contract, _) = emit_both(&users_demo_src());
        assert!(contract.contains("export type Role = \"Admin\" | \"Member\" | \"Guest\";"));
    }

    #[test]
    fn adt_enum_emits_discriminated_union() {
        let (contract, _) = emit_both(&users_demo_src());
        assert!(contract.contains("export type ValidationError ="));
        assert!(contract.contains("| { type: \"InvalidEmail\"; field: string }"));
        assert!(contract.contains("| { type: \"TooShort\"; field: string; min: number }"));
    }

    #[test]
    fn fn_declarations_are_not_part_of_the_contract() {
        let (contract, _) = emit_both(&users_demo_src());
        assert!(!contract.contains("validate"));
    }

    #[test]
    fn service_interface_and_rpc_signatures() {
        let (contract, _) = emit_both(&users_demo_src());
        assert!(contract.contains("export interface UsersClient {"));
        assert!(contract.contains("list(limit?: number): Promise<User[]>;")); // default -> opcional
        assert!(contract.contains("getById(id: number): Promise<User | null>;"));
        assert!(contract.contains("create(input: NewUser): Promise<Result<User, ValidationError>>;"));
    }

    #[test]
    fn client_never_throws_for_declared_result_and_wraps_transport_errors() {
        let (_, client) = emit_both(&users_demo_src());
        assert!(client.contains("class LinkTransportError extends Error {}"));
        assert!(client.contains("if (!res.ok) throw new LinkTransportError"));
        assert!(client.contains("class UsersClientImpl implements UsersClient"));
        assert!(client.contains("export function createUsersClient(baseUrl: string): UsersClient"));
    }

    #[test]
    fn client_imports_every_type_it_references_not_just_the_client_interface() {
        // Bug real encontrado a mano: client.ts usaba User/NewUser/Result/
        // ValidationError en sus firmas sin importarlos — no habría compilado.
        let (_, client) = emit_both(&users_demo_src());
        let import_line = client.lines().find(|l| l.starts_with("import type")).expect("falta la línea de import");
        for name in ["User", "NewUser", "Result", "ValidationError", "Patch", "UsersClient"] {
            assert!(
                import_line.contains(name),
                "el import de client.ts debería incluir '{name}': {import_line}"
            );
        }
    }

    #[test]
    fn patch_of_user_renders_as_utility_type_reference() {
        let (contract, client) = emit_both(&users_demo_src());
        assert!(contract.contains("update(id: number, patch: Patch<User>): Promise<User>;"));
        assert!(client.contains("async update(id: number, patch: Patch<User>): Promise<User>"));
    }

    #[test]
    fn list_of_optional_gets_parenthesized() {
        let src = "type A = { xs: Int?[] }"; // List(Optional(Int)) -- ver GRAMMAR.md §2.2
        let (contract, _) = emit_both(src);
        assert!(
            contract.contains("xs: (number | null)[];"),
            "se esperaban paréntesis alrededor de 'number | null': {contract}"
        );
    }

    #[test]
    fn patch_is_just_partial() {
        let (contract, _) = emit_both(&users_demo_src());
        assert!(contract.contains("export type Patch<T> = Partial<T>;"));
    }

    #[test]
    fn map_renders_as_native_record_without_needing_an_import() {
        let src = "type Config = { flags: Map<String, Bool> }";
        let (contract, _) = emit_both(src);
        assert!(contract.contains("flags: Record<string, boolean>;"));
        // Record es nativo de TS -- no debería aparecer en ningún import
        assert!(!contract.contains("import"));
    }
}
