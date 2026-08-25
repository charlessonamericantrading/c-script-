// Emisor de contrato: el único pass compartido que produce tanto el
// `.d.ts` como el `client.ts` (PLAN.md §3.3) — así el servidor y el cliente
// no pueden divergir, porque ambos salen del mismo `render_type`.
//
// Sigue la tabla de mapeo de GRAMMAR.md §4 al pie de la letra. `fn` no se
// emite: es lógica interna del backend, no parte del contrato (GRAMMAR.md
// nota sobre fn_decl en §2.1).

use super::validators_emit;
use crate::ast::*;
use crate::checker::Checker;
use crate::types::Type;

pub fn emit_contract(program: &Program) -> Result<String, String> {
    let (checker, errors) = Checker::build_symbols(program);
    if let Some(e) = errors.into_iter().next() {
        return Err(e.to_string());
    }

    let mut out = String::new();
    out.push_str(&format!("// Generado automáticamente por linkc v{} — no editar a mano.\n\n", crate::VERSION));
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

/// `const X: T = v` (GRAMMAR.md §4) -- va a `client.ts`, NO a
/// `contract.d.ts`.
///
/// Un `.d.ts` es un archivo de declaraciones AMBIENTALES: describe tipos y
/// firmas, no lleva código. TypeScript rechaza cualquier inicializador ahí
/// con `TS1039: Initializers are not allowed in ambient contexts`, así que
/// emitir `export const MAX: number = 3;` en el contrato hacía que
/// CUALQUIER programa con un `const` produjera un contrato que no compila
/// (bug real de la auditoría; el demo no tiene ningún `const`, por eso
/// nunca se notó). Un `const` es un VALOR, y los valores viven en el
/// módulo real -- de ahí que se emita acá.
fn emit_const_decl(out: &mut String, c: &ConstDecl, checker: &Checker) -> Result<(), String> {
    let ty = checker.resolve_type(&c.ty).map_err(|e| e.to_string())?;
    let value = render_const_value(&c.value.node, checker)?;
    out.push_str(&format!("export const {}: {} = {};\n\n", c.name, render_type(&ty), value));
    Ok(())
}

/// Solo expresiones con forma de literal -- lo único que este emisor puede
/// bajar a código TS estático. `Call`/`FieldAccess`/`Match`/etc. son
/// computaciones en runtime (necesitan `db`, o simplemente no tienen un
/// valor fijo hasta que corren) y no tienen ningún equivalente como
/// constante de módulo en TS -- por eso un `const` con ese tipo de valor
/// es un error del checker, no algo que el emisor intente adivinar.
fn render_const_value(e: &Expr, checker: &Checker) -> Result<String, String> {
    match e {
        Expr::Int(n) => Ok(n.to_string()),
        Expr::Float(n) => Ok(n.to_string()),
        Expr::Str(s) => Ok(format!("{s:?}")),
        Expr::Bool(b) => Ok(b.to_string()),
        Expr::Null => Ok("null".to_string()),
        Expr::ArrayLit(items) => {
            let parts: Vec<String> =
                items.iter().map(|i| render_const_value(&i.node, checker)).collect::<Result<_, _>>()?;
            Ok(format!("[{}]", parts.join(", ")))
        }
        Expr::TupleLit(items) => {
            let parts: Vec<String> =
                items.iter().map(|i| render_const_value(&i.node, checker)).collect::<Result<_, _>>()?;
            Ok(format!("[{}]", parts.join(", ")))
        }
        Expr::StructLit { name, variant, fields } => {
            let mut parts: Vec<String> = Vec::new();
            if let Some(v) = variant {
                // Un enum SIMPLE (todas sus variantes unitarias) se emite
                // como unión de literales string, así que su valor es el
                // string pelado -- no `{ type: "..." }`. Es exactamente la
                // misma distinción `all_unit` que ya hacen `emit_enum_decl`
                // (acá al lado) y `value_to_json` (runtime/mod.rs); este
                // tercer lugar se la había perdido, y emitía un valor que
                // ni siquiera era asignable al tipo que el propio contrato
                // declaraba dos líneas más arriba.
                if is_simple_enum(name, checker) {
                    return Ok(format!("{v:?}"));
                }
                parts.push(format!("type: {v:?}"));
            }
            for (k, fe) in fields {
                parts.push(format!("{k}: {}", render_const_value(&fe.node, checker)?));
            }
            Ok(format!("{{ {} }}", parts.join(", ")))
        }
        other => Err(format!(
            "el valor de un 'const' tiene que ser un literal (número, string, bool, null, array, tupla o \
             struct/variant literal) -- se encontró {other:?}, que es una computación en runtime sin ningún \
             equivalente como constante estática de TS"
        )),
    }
}

pub fn emit_client(program: &Program) -> Result<String, String> {
    let (checker, errors) = Checker::build_symbols(program);
    if let Some(e) = errors.into_iter().next() {
        return Err(e.to_string());
    }

    let mut out = String::new();
    out.push_str(&format!("// Generado automáticamente por linkc v{} — no editar a mano.\n\n", crate::VERSION));

    // Recolecta TODOS los `service` del programa, no el primero: un programa
    // con mas de un `service` (GRAMMAR.md no lo limita a uno, a diferencia de
    // `db{}`) generaba client.ts con una sola clase -- `emit_hooks`, la
    // funcion hermana un poco mas abajo en este mismo archivo, ya resolvia
    // esto con el mismo patron de recoleccion multi-servicio; aca faltaba.
    // `contract.d.ts` (interfaz `{Name}Client` por servicio) y `hooks.ts`
    // nunca tuvieron el bug -- solo esta funcion se detenia en el primero.
    let mut imported_names = std::collections::BTreeSet::new();
    let mut validator_names = std::collections::BTreeSet::new();
    let mut services: Vec<(&ServiceDecl, Vec<(&RpcDecl, bool, Vec<Type>, Type)>)> = Vec::new();
    for item in &program.items {
        let Item::Service(service) = item else { continue };
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
            validators_emit::collect_validator_names(&ret_ty, &mut validator_names);
            resolved.push((rpc, is_stream, param_tys, ret_ty));
        }
        imported_names.insert(format!("{}Client", service.name));
        services.push((service, resolved));
    }
    if services.is_empty() {
        return Ok(out); // sin ningun service declarado, no hay cliente que generar
    }
    // Los `const` se emiten en ESTE archivo (ver `emit_const_decl`), así que
    // su tipo declarado también hay que importarlo -- `const DEF: Role = ...`
    // no compila si `Role` no está en scope acá.
    for item in &program.items {
        if let Item::Const(c) = item {
            let ty = checker.resolve_type(&c.ty).map_err(|e| e.to_string())?;
            collect_type_names(&ty, &mut imported_names);
        }
    }

    out.push_str(&format!(
        "import type {{ {} }} from \"./contract\";\n",
        imported_names.into_iter().collect::<Vec<_>>().join(", ")
    ));
    if !validator_names.is_empty() {
        out.push_str(&format!(
            // `.ts` explícito, a diferencia del import de "./contract" de
            // arriba: ese es `import type` (se borra del todo al compilar,
            // Node nunca lo resuelve en runtime); este es un import de
            // VALOR real -- el loader ESM nativo de Node exige la
            // extensión explícita para resolverlo (mismo motivo que
            // frontend/src/main.ts ya importa "../../gen/client.ts" así).
            "import {{ {} }} from \"./validators.ts\";\n",
            validator_names.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    out.push('\n');
    // Errores de transporte vs de dominio (GRAMMAR.md §3.5): esta excepción es
    // SOLO para fallos de infraestructura (red, 5xx, timeout) — los errores de
    // dominio que un rpc declaró en su Result<T,E> siempre vuelven como valor,
    // nunca se lanzan.
    out.push_str("export class LinkTransportError extends Error {}\n\n");
    // Distinta de las dos anteriores: ni un error de dominio declarado
    // (Result<T,E>) ni un fallo de transporte -- "el servidor respondió 200
    // pero el payload no matchea el contrato", su propia categoría (GRAMMAR.md
    // §3.5 ya traza exactamente esta línea divisoria para las otras dos).
    out.push_str("export class LinkValidationError extends Error {\n");
    out.push_str("  rpcName: string;\n");
    out.push_str("  received: unknown;\n");
    out.push_str("  constructor(rpcName: string, received: unknown) {\n");
    out.push_str("    super(`la respuesta de '${rpcName}' no matchea el contrato declarado`);\n");
    out.push_str("    this.rpcName = rpcName;\n");
    out.push_str("    this.received = received;\n");
    out.push_str("  }\n");
    out.push_str("}\n\n");

    out.push_str("export function isOk<T, E>(result: { ok: true; value: T } | { ok: false; error: E }): result is { ok: true; value: T } {\n");
    out.push_str("  return result.ok === true;\n");
    out.push_str("}\n\n");
    out.push_str("export function isErr<T, E>(result: { ok: true; value: T } | { ok: false; error: E }): result is { ok: false; error: E } {\n");
    out.push_str("  return result.ok === false;\n");
    out.push_str("}\n\n");

    // Los `const` viven acá, no en contract.d.ts -- ver `emit_const_decl`:
    // un .d.ts es ambiental y TypeScript rechaza los inicializadores.
    for item in &program.items {
        if let Item::Const(c) = item {
            emit_const_decl(&mut out, c, &checker)?;
        }
    }

    // Un `class {Name}ClientImpl` + `create{Name}Client` por cada service
    // recolectado arriba -- antes esta función entera corría UNA sola vez
    // para el primer (y único) `service` que encontraba.
    for (service, resolved) in &services {
    out.push_str(&format!("class {name}ClientImpl implements {name}Client {{\n", name = service.name));
    // Constructor explícito, no "parameter property" (`private x: T` en la
    // firma) -- esa azúcar de TS no la entienden strip-only transpilers
    // (soporte nativo de Node, esbuild en modo transform), y el código
    // generado debería ser legible por el mayor número posible de toolchains.
    out.push_str("  private baseUrl: string;\n");
    // Auth v0 (GRAMMAR.md §3.14): estado MUTABLE de instancia, no un
    // parámetro por-llamada -- correcto para "una instancia de cliente = un
    // usuario/sesión activa" (igual que la mayoría de SDKs generados
    // reales), pero significa que una única instancia COMPARTIDA entre
    // requests concurrentes de usuarios DISTINTOS (ej. un backend-for-
    // frontend Node reusando un solo cliente módulo-level) puede pisarse el
    // token entre requests. Para ese caso: instanciar un cliente por
    // request/usuario, no compartir uno mutable.
    out.push_str("  private token: string | null = null;\n");
    out.push_str("  constructor(baseUrl: string) {\n    this.baseUrl = baseUrl;\n  }\n\n");
    out.push_str("  setToken(token: string | null): void {\n    this.token = token;\n  }\n\n");

    for (rpc, is_stream, param_tys, ret_ty) in resolved {
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
        let check = validators_emit::render_check_expr(ret_ty, "json");

        if *is_stream {
            // Alcance v0 explícito (GRAMMAR.md §3.13): el servidor manda la
            // secuencia YA CALCULADA entera como eventos SSE (server.rs,
            // `serve_stream`); acá del lado del cliente sólo hace falta
            // parsear ese framing (`data: ...\n\n`) e ir devolviendo cada
            // elemento -- ya validado, igual que un rpc normal -- a medida
            // que llega. No usa EventSource (es GET-only, sin body) porque
            // el resto del contrato ya asume POST+JSON body para args; en
            // cambio lee el ReadableStream nativo de fetch() a mano.
            out.push_str(&format!(
                "  async *{}({}): AsyncIterable<{}> {{\n",
                rpc.name,
                params.join(", "),
                render_type(ret_ty)
            ));
            push_fetch_call(&mut out, &service.name, &rpc.name, &arg_names);
            out.push_str("    if (!res.body) throw new LinkTransportError(\"el servidor no devolvió un body de stream\");\n");
            out.push_str("    const reader = res.body.getReader();\n");
            out.push_str("    const decoder = new TextDecoder();\n");
            out.push_str("    let buffer = \"\";\n");
            out.push_str("    while (true) {\n");
            out.push_str("      const { done, value } = await reader.read();\n");
            out.push_str("      if (done) break;\n");
            out.push_str("      buffer += decoder.decode(value, { stream: true });\n");
            out.push_str("      let sep: number;\n");
            out.push_str("      while ((sep = buffer.indexOf(\"\\n\\n\")) !== -1) {\n");
            out.push_str("        const frame = buffer.slice(0, sep);\n");
            out.push_str("        buffer = buffer.slice(sep + 2);\n");
            out.push_str("        if (!frame.startsWith(\"data: \")) continue;\n");
            out.push_str("        const json: unknown = JSON.parse(frame.slice(6));\n");
            out.push_str(&format!(
                "        if (!({check})) throw new LinkValidationError(\"{}\", json);\n",
                rpc.name
            ));
            out.push_str(&format!("        yield json as {};\n", render_type(ret_ty)));
            out.push_str("      }\n");
            out.push_str("    }\n");
            out.push_str("  }\n\n");
            continue;
        }

        out.push_str(&format!(
            "  async {}({}): Promise<{}> {{\n",
            rpc.name,
            params.join(", "),
            render_type(ret_ty)
        ));
        // `@route` (GRAMMAR.md §3.37) es un alias ADEMÁS de esta dirección,
        // nunca un reemplazo -- el cliente generado sigue llamando siempre a
        // /Service/rpc; la URL linda es para un crawler, no para código.
        if let Some(pattern) = rpc.route() {
            out.push_str(&format!("    // También accesible en: {pattern}\n"));
        }
        push_fetch_call(&mut out, &service.name, &rpc.name, &arg_names);

        // Un rpc con `@content_type` responde el String tal cual, sin comillas
        // de JSON alrededor (GRAMMAR.md §3.35) -- llamar a `res.json()` acá
        // reventaría con un SyntaxError sobre el primer `<` del HTML. Tampoco
        // corre el validador: no hay JSON que validar, y el checker ya exigió
        // que el retorno sea String.
        if let Some(ct) = rpc.content_type() {
            out.push_str(&format!("    // Content-Type declarado: {ct}\n"));
            out.push_str("    return await res.text();\n");
            out.push_str("  }\n\n");
            continue;
        }

        out.push_str("    const json: unknown = await res.json();\n");
        out.push_str(&format!(
            "    if (!({check})) throw new LinkValidationError(\"{}\", json);\n",
            rpc.name
        ));
        out.push_str(&format!("    return json as {};\n", render_type(ret_ty)));
        out.push_str("  }\n\n");
    }
    out.push_str("}\n\n");

    out.push_str(&format!(
        "export function create{name}Client(baseUrl: string): {name}Client {{\n  return new {name}ClientImpl(baseUrl);\n}}\n\n",
        name = service.name
    ));
    }

    Ok(out)
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

/// Mismo heurístico "nombre por forma" que separa el hook `Query` del
/// `Mutation` en `emit_hooks` (GRAMMAR.md §3.124) -- un rpc sin parámetros
/// también cuenta como Query, no hay forma más segura de mutar sin nada que
/// pasarle.
fn is_query_rpc(rpc: &RpcDecl) -> bool {
    rpc.name.starts_with("get")
        || rpc.name.starts_with("list")
        || rpc.name.starts_with("find")
        || rpc.name.starts_with("search")
        || rpc.name.starts_with("read")
        || rpc.name.starts_with("fetch")
        || rpc.params.is_empty()
}

pub fn emit_hooks(program: &Program) -> Result<String, String> {
    let (checker, errors) = Checker::build_symbols(program);
    if let Some(e) = errors.into_iter().next() {
        return Err(e.to_string());
    }

    let mut imported_types = std::collections::BTreeSet::new();
    let mut services = Vec::new();

    for item in &program.items {
        let Item::Service(service) = item else { continue };
        let mut members = Vec::new();
        for m in &service.members {
            let (rpc, is_stream) = match m {
                Member::Rpc(r) => (r, false),
                Member::Stream(r) => (r, true),
            };
            let mut param_tys = Vec::new();
            for p in &rpc.params {
                let ty = checker.resolve_type(&p.ty).map_err(|e| e.to_string())?;
                collect_type_names(&ty, &mut imported_types);
                param_tys.push(ty);
            }
            let ret_ty = checker.resolve_type(&rpc.return_type).map_err(|e| e.to_string())?;
            collect_type_names(&ret_ty, &mut imported_types);
            members.push((rpc, is_stream, param_tys, ret_ty));
        }
        imported_types.insert(format!("{}Client", service.name));
        services.push((service, members));
    }

    // GRAMMAR.md §3.124: si HAY algún rpc que va a generar un hook de Query,
    // el archivo entero necesita `useSyncExternalStore` (el cache
    // compartido) -- calculado ACÁ, antes de armar la línea de import,
    // porque `noUnusedLocals` (la config real de `examples/taskboard/
    // frontend/tsconfig.json`, entre otras) rechaza un import sin usar: si
    // el programa no declara ningún Query (todo streams/mutations), sumar
    // `useSyncExternalStore` de todos modos rompería el build de cualquiera
    // que compile con esa opción prendida.
    let has_any_query = services.iter().any(|(_, members)| members.iter().any(|(rpc, is_stream, _, _)| !is_stream && is_query_rpc(rpc)));

    let mut out = String::new();
    out.push_str(&format!("// Generado automáticamente por linkc v{} — no editar a mano.\n\n", crate::VERSION));
    if has_any_query {
        out.push_str("import { useState, useEffect, useCallback, useRef, useSyncExternalStore } from \"react\";\n");
    } else {
        out.push_str("import { useState, useEffect, useCallback, useRef } from \"react\";\n");
    }
    if !imported_types.is_empty() {
        out.push_str(&format!(
            "import type {{ {} }} from \"./contract\";\n\n",
            imported_types.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }

    out.push_str("export interface QueryState<T> {\n");
    out.push_str("  data: T | null;\n");
    out.push_str("  loading: boolean;\n");
    out.push_str("  error: Error | null;\n");
    out.push_str("  refetch: () => Promise<T | null>;\n");
    out.push_str("}\n\n");

    out.push_str("export interface MutationState<T> {\n");
    out.push_str("  data: T | null;\n");
    out.push_str("  loading: boolean;\n");
    out.push_str("  error: Error | null;\n");
    out.push_str("  reset: () => void;\n");
    out.push_str("}\n\n");

    out.push_str("export interface SubscriptionState<T> {\n");
    out.push_str("  data: T[];\n");
    out.push_str("  latest: T | null;\n");
    out.push_str("  isConnected: boolean;\n");
    out.push_str("  error: Error | null;\n");
    out.push_str("}\n\n");

    // Cache compartido entre TODAS las instancias de un mismo hook de Query
    // (GRAMMAR.md §3.124) -- dos componentes llamando a
    // `use{Servicio}{Rpc}Query` con los MISMOS parámetros comparten una
    // sola entrada: una sola request en vuelo (dedupe, ninguno de los dos
    // dispara su propio fetch por separado) y el resultado de UNO actualiza
    // a los DOS a la vez (`useSyncExternalStore`, la forma que React 18
    // documenta para suscribirse a un store externo al árbol de
    // componentes). Alcance: cache por rpc+parámetros, NO por instancia de
    // `client` -- si la misma app usara dos clients distintos contra el
    // mismo rpc comparten cache igual; el caso real (`examples/taskboard/
    // frontend` y el resto de los ejemplos) siempre tiene un client único
    // por app. Sin invalidación automática tras una `Mutation` todavía --
    // `refetch()` de una instancia sigue actualizando a todas las que
    // comparten esa misma clave, pero nada dispara eso solo porque una
    // mutación relacionada tuvo éxito (necesitaría una forma de declarar
    // qué Query invalida cada Mutation, que no existe hoy).
    if has_any_query {
        out.push_str("type QueryCacheState<T> = { data: T | null; loading: boolean; error: Error | null };\n\n");
        out.push_str("type QueryCacheEntry<T> = {\n");
        out.push_str("  state: QueryCacheState<T>;\n");
        out.push_str("  promise: Promise<T> | null;\n");
        out.push_str("  listeners: Set<() => void>;\n");
        out.push_str("};\n\n");
        out.push_str("const queryCache = new Map<string, QueryCacheEntry<unknown>>();\n\n");
        out.push_str("function getQueryCacheEntry<T>(key: string): QueryCacheEntry<T> {\n");
        out.push_str("  let entry = queryCache.get(key) as QueryCacheEntry<T> | undefined;\n");
        out.push_str("  if (!entry) {\n");
        out.push_str("    entry = { state: { data: null, loading: false, error: null }, promise: null, listeners: new Set() };\n");
        out.push_str("    queryCache.set(key, entry as QueryCacheEntry<unknown>);\n");
        out.push_str("  }\n");
        out.push_str("  return entry;\n");
        out.push_str("}\n\n");
        out.push_str("function setQueryCacheState<T>(entry: QueryCacheEntry<T>, patch: Partial<QueryCacheState<T>>): void {\n");
        out.push_str("  entry.state = { ...entry.state, ...patch };\n");
        out.push_str("  entry.listeners.forEach((listener) => listener());\n");
        out.push_str("}\n\n");
    }

    for (service, members) in services {
        for (rpc, is_stream, param_tys, ret_ty) in members {
            let cap_rpc = capitalize(&rpc.name);
            let ret_str = render_type(&ret_ty);
            let params_typed: Vec<String> = rpc
                .params
                .iter()
                .zip(&param_tys)
                .map(|(p, ty)| {
                    format!(
                        "{}{}: {}",
                        p.name,
                        if p.default.is_some() { "?" } else { "" },
                        render_type(ty)
                    )
                })
                .collect();
            let param_names: Vec<&str> = rpc.params.iter().map(|p| p.name.as_str()).collect();

            if is_stream {
                let params_sig = if params_typed.is_empty() {
                    format!("client: {}Client", service.name)
                } else {
                    format!("client: {}Client, {}", service.name, params_typed.join(", "))
                };
                let deps = if param_names.is_empty() {
                    "client".to_string()
                } else {
                    format!("client, {}", param_names.join(", "))
                };

                out.push_str(&format!(
                    "export function use{service}{cap_rpc}({params_sig}): SubscriptionState<{ret_str}> {{\n",
                    service = service.name
                ));
                out.push_str(&format!("  const [data, setData] = useState<{}[]>([]);\n", ret_str));
                out.push_str(&format!("  const [latest, setLatest] = useState<{} | null>(null);\n", ret_str));
                out.push_str("  const [isConnected, setIsConnected] = useState(false);\n");
                out.push_str("  const [error, setError] = useState<Error | null>(null);\n\n");
                out.push_str("  useEffect(() => {\n");
                out.push_str("    let cancelled = false;\n");
                out.push_str("    async function run() {\n");
                out.push_str("      try {\n");
                out.push_str("        setIsConnected(true);\n");
                out.push_str("        setError(null);\n");
                out.push_str(&format!("        for await (const item of client.{}({})) {{\n", rpc.name, param_names.join(", ")));
                out.push_str("          if (cancelled) break;\n");
                out.push_str("          setLatest(item);\n");
                out.push_str("          setData((prev) => [...prev, item]);\n");
                out.push_str("        }\n");
                out.push_str("      } catch (err) {\n");
                out.push_str("        if (!cancelled) setError(err instanceof Error ? err : new Error(String(err)));\n");
                out.push_str("      } finally {\n");
                out.push_str("        if (!cancelled) setIsConnected(false);\n");
                out.push_str("      }\n");
                out.push_str("    }\n");
                out.push_str("    run();\n");
                out.push_str("    return () => { cancelled = true; };\n");
                out.push_str(&format!("  }}, [{}]);\n\n", deps));
                out.push_str("  return { data, latest, isConnected, error };\n");
                out.push_str("}\n\n");
            } else {
                if is_query_rpc(rpc) {
                    let params_sig = if params_typed.is_empty() {
                        format!("client: {}Client, options?: {{ enabled?: boolean }}", service.name)
                    } else {
                        format!("client: {}Client, {}, options?: {{ enabled?: boolean }}", service.name, params_typed.join(", "))
                    };
                    let deps = if param_names.is_empty() {
                        "client".to_string()
                    } else {
                        format!("client, {}", param_names.join(", "))
                    };
                    // La clave incluye rpc+parámetros (no `client`, ver el
                    // comentario sobre el cache más arriba) -- `JSON.stringify`
                    // sobre el array de parámetros posicionales, así que dos
                    // instancias del hook con los MISMOS argumentos siempre
                    // caen en la MISMA entrada del `Map`, sin importar en qué
                    // componente estén montadas.
                    let cache_key_expr =
                        format!("\"{}.{}(\" + JSON.stringify([{}]) + \")\"", service.name, rpc.name, param_names.join(", "));

                    out.push_str(&format!(
                        "export function use{service}{cap_rpc}Query({params_sig}): QueryState<{ret_str}> {{\n",
                        service = service.name
                    ));
                    out.push_str("  const enabled = options?.enabled ?? true;\n");
                    out.push_str(&format!("  const cacheKey = {cache_key_expr};\n"));
                    // `getQueryCacheEntry` siempre devuelve el MISMO objeto
                    // para la MISMA clave (lo cachea el `Map`) -- por eso
                    // `entry` es una referencia estable entre renders
                    // mientras `cacheKey` no cambie, sin necesitar un
                    // `useMemo` propio para eso.
                    out.push_str(&format!("  const entry = getQueryCacheEntry<{ret_str}>(cacheKey);\n\n"));
                    out.push_str("  const subscribe = useCallback((onStoreChange: () => void) => {\n");
                    out.push_str("    entry.listeners.add(onStoreChange);\n");
                    out.push_str("    return () => { entry.listeners.delete(onStoreChange); };\n");
                    out.push_str("  }, [entry]);\n");
                    out.push_str("  const getSnapshot = useCallback(() => entry.state, [entry]);\n");
                    // `useSyncExternalStore` (React 18): la forma real de
                    // suscribirse a un store FUERA del árbol de componentes
                    // sin roturas de consistencia entre renders concurrentes
                    // -- es lo que hace que dos instancias de este hook con
                    // la MISMA `cacheKey` reciban la MISMA `data`/`loading`/
                    // `error` y se actualicen juntas cuando cualquiera de
                    // las dos llama a `refetch`.
                    out.push_str("  const state = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);\n\n");
                    out.push_str(&format!("  const refetch = useCallback(async (): Promise<{ret_str} | null> => {{\n"));
                    // Dedupe real: si YA hay una request en vuelo para esta
                    // clave (disparada por esta instancia, por OTRA
                    // instancia del mismo hook, o por el `useEffect` de
                    // abajo), todas comparten esa MISMA promesa en vez de
                    // que cada `refetch()` dispare su propio fetch --
                    // `entry.promise` es el punto de sincronización.
                    out.push_str("    if (!entry.promise) {\n");
                    out.push_str("      setQueryCacheState(entry, { loading: true, error: null });\n");
                    out.push_str(&format!("      entry.promise = client.{}({})\n", rpc.name, param_names.join(", ")));
                    out.push_str("        .then((res) => {\n");
                    out.push_str("          setQueryCacheState(entry, { data: res, loading: false });\n");
                    out.push_str("          return res;\n");
                    out.push_str("        })\n");
                    out.push_str("        .catch((err) => {\n");
                    out.push_str("          const e = err instanceof Error ? err : new Error(String(err));\n");
                    out.push_str("          setQueryCacheState(entry, { error: e, loading: false });\n");
                    out.push_str("          throw e;\n");
                    out.push_str("        })\n");
                    out.push_str("        .finally(() => {\n");
                    out.push_str("          entry.promise = null;\n");
                    out.push_str("        });\n");
                    out.push_str("    }\n");
                    out.push_str("    try {\n");
                    out.push_str("      return await entry.promise;\n");
                    out.push_str("    } catch {\n");
                    out.push_str("      return null;\n");
                    out.push_str("    }\n");
                    out.push_str(&format!("  }}, [entry, {deps}]);\n\n"));
                    out.push_str("  useEffect(() => {\n");
                    // `state.data === null && !state.loading` -- solo dispara
                    // un fetch si esta entrada del cache está genuinamente
                    // VACÍA (nadie la pidió todavía) y nada más ya la está
                    // pidiendo; si otra instancia ya la cacheó o ya hay una
                    // request en vuelo, este efecto no hace nada -- ahí está
                    // el dedupe entre MONTAJES, no solo entre llamadas.
                    out.push_str("    if (enabled && state.data === null && !state.loading && !entry.promise) {\n");
                    out.push_str("      refetch();\n");
                    out.push_str("    }\n");
                    out.push_str("  }, [enabled, refetch, entry, state.data, state.loading]);\n\n");
                    out.push_str("  return { data: state.data, loading: state.loading, error: state.error, refetch };\n");
                    out.push_str("}\n\n");
                }

                // Mutation hook
                out.push_str(&format!(
                    "export function use{service}{cap_rpc}Mutation(client: {service}Client): MutationState<{ret_str}> & {{\n  mutate: ({params}) => Promise<{ret_str}>;\n}} {{\n",
                    service = service.name,
                    params = params_typed.join(", ")
                ));
                out.push_str(&format!("  const [data, setData] = useState<{} | null>(null);\n", ret_str));
                out.push_str("  const [loading, setLoading] = useState(false);\n");
                out.push_str("  const [error, setError] = useState<Error | null>(null);\n");
                // Misma guarda de "solo la respuesta más reciente gana" que
                // el hook de Query -- un doble click en un botón de submit
                // dispara dos `mutate()` casi juntos, y sin esto la
                // respuesta más LENTA de las dos puede resolver después y
                // pisar `data`/`error` con el resultado de la llamada vieja.
                out.push_str("  const requestIdRef = useRef(0);\n\n");
                out.push_str(&format!(
                    "  const mutate = useCallback(async ({}): Promise<{}> => {{\n",
                    params_typed.join(", "),
                    ret_str
                ));
                out.push_str("    const requestId = ++requestIdRef.current;\n");
                out.push_str("    setLoading(true);\n");
                out.push_str("    setError(null);\n");
                out.push_str("    try {\n");
                out.push_str(&format!("      const res = await client.{}({});\n", rpc.name, param_names.join(", ")));
                out.push_str("      if (requestIdRef.current === requestId) setData(res);\n");
                out.push_str("      return res;\n");
                out.push_str("    } catch (err) {\n");
                out.push_str("      const e = err instanceof Error ? err : new Error(String(err));\n");
                out.push_str("      if (requestIdRef.current === requestId) setError(e);\n");
                out.push_str("      throw e;\n");
                out.push_str("    } finally {\n");
                out.push_str("      if (requestIdRef.current === requestId) setLoading(false);\n");
                out.push_str("    }\n");
                out.push_str("  }, [client]);\n\n");
                out.push_str("  const reset = useCallback(() => {\n");
                // Invalida cualquier `mutate()` que siga en vuelo -- sin
                // esto, una respuesta tardía de ANTES del reset podría
                // llegar después y pisar el estado recién limpiado con el
                // resultado de la llamada vieja.
                out.push_str("    requestIdRef.current++;\n");
                out.push_str("    setData(null);\n");
                out.push_str("    setLoading(false);\n");
                out.push_str("    setError(null);\n");
                out.push_str("  }, []);\n\n");
                out.push_str("  return { mutate, data, loading, error, reset };\n");
                out.push_str("}\n\n");
            }
        }
    }

    Ok(out)
}

/// El `fetch()` + chequeo de status es idéntico para un rpc normal y para
/// un stream (ambos mandan los mismos args por POST+JSON body) -- lo único
/// que difiere entre los dos es qué se hace DESPUÉS con `res` (un solo
/// `res.json()` vs. leer `res.body` incrementalmente), así que solo esta
/// parte vale la pena compartir.
fn push_fetch_call(out: &mut String, service_name: &str, rpc_name: &str, arg_names: &[&str]) {
    out.push_str(&format!(
        "    const res = await fetch(`${{this.baseUrl}}/{service_name}/{rpc_name}`, {{\n"
    ));
    out.push_str("      method: \"POST\",\n");
    // Auth v0 (GRAMMAR.md §3.14): el header Bearer se agrega SOLO si hay un
    // token seteado -- el server decide caso por caso (vía @authenticated/
    // @requires) si lo exige; el cliente lo manda siempre que lo tenga,
    // para cualquier rpc, sin necesidad de que el codegen sepa cuáles
    // realmente lo necesitan.
    out.push_str(
        "      headers: { \"Content-Type\": \"application/json\", ...(this.token ? { Authorization: `Bearer ${this.token}` } : {}) },\n",
    );
    out.push_str(&format!("      body: JSON.stringify({{ {} }}),\n", arg_names.join(", ")));
    out.push_str("    });\n");
    out.push_str("    if (!res.ok) throw new LinkTransportError(`HTTP ${res.status}`);\n");
}

/// `<T, U>` para una declaración genérica, o "" si no tiene type_params.
fn type_params_suffix(type_params: &[String]) -> String {
    if type_params.is_empty() {
        String::new()
    } else {
        format!("<{}>", type_params.join(", "))
    }
}

/// Resuelve el tipo de un campo dentro de una declaración que puede ser
/// genérica: con type_params, usa `resolve_type_abstract` (T se queda como
/// T, ver GRAMMAR.md §3.6) -- TS ya tiene genéricos nativos, así que la
/// declaración se emite tal cual, sin monomorfizar.
fn resolve_field_ty(checker: &Checker, ty: &TypeExpr, type_params: &[String]) -> Result<Type, String> {
    if type_params.is_empty() {
        checker.resolve_type(ty).map_err(|e| e.to_string())
    } else {
        checker.resolve_type_abstract(ty, type_params).map_err(|e| e.to_string())
    }
}

/// Emite (o no) el bloque JSDoc de un rpc: docstring `///` (GRAMMAR.md
/// §3.72) y/o `@deprecated` (§3.71), indentado 2 espacios. `None`/`None` no
/// emite nada -- mismo comportamiento que antes de que cualquiera de los dos
/// existiera. Con las dos cosas presentes, el docstring va primero como
/// texto libre y `@deprecated` como su propia línea de tag JSDoc, DENTRO del
/// mismo bloque (no dos comentarios separados) -- así es como cualquier
/// editor que entienda JSDoc espera verlos combinados.
fn push_rpc_jsdoc(out: &mut String, doc: Option<&str>, deprecated: Option<&str>) {
    match (doc, deprecated) {
        (None, None) => {}
        (Some(d), None) => {
            out.push_str("  /**\n");
            for line in d.lines() {
                out.push_str(&format!("   * {}\n", jsdoc_escape(line)));
            }
            out.push_str("   */\n");
        }
        (None, Some(reason)) => {
            out.push_str(&format!("  /** @deprecated {} */\n", jsdoc_escape(reason)));
        }
        (Some(d), Some(reason)) => {
            out.push_str("  /**\n");
            for line in d.lines() {
                out.push_str(&format!("   * {}\n", jsdoc_escape(line)));
            }
            out.push_str(&format!("   * @deprecated {}\n", jsdoc_escape(reason)));
            out.push_str("   */\n");
        }
    }
}

/// Emite (o no) el bloque JSDoc de un campo: `@deprecated` (§3.71) y/o
/// `@validate` (§3.73), indentado 2 espacios. Mismo criterio que
/// `push_rpc_jsdoc`: un solo bloque combinado, nunca dos comentarios
/// separados, y nada si el campo no lleva ninguna de las dos anotaciones.
/// `@validate` no tiene un tag JSDoc estándar -- se documenta como texto
/// libre ("Formato: ...") en vez de inventar uno propio que ningún editor
/// vaya a reconocer especialmente.
fn push_field_jsdoc(out: &mut String, deprecated: Option<&str>, validator: Option<&FieldValidator>) {
    let format_line = validator.map(|v| match v {
        FieldValidator::Email => "Formato: email".to_string(),
        FieldValidator::Regex(pattern) => format!("Formato: coincide con /{}/", jsdoc_escape(pattern)),
    });
    match (format_line, deprecated) {
        (None, None) => {}
        (Some(f), None) => {
            out.push_str(&format!("  /** {f} */\n"));
        }
        (None, Some(reason)) => {
            out.push_str(&format!("  /** @deprecated {} */\n", jsdoc_escape(reason)));
        }
        (Some(f), Some(reason)) => {
            out.push_str("  /**\n");
            out.push_str(&format!("   * {f}\n"));
            out.push_str(&format!("   * @deprecated {}\n", jsdoc_escape(reason)));
            out.push_str("   */\n");
        }
    }
}

/// Un motivo de `@deprecated` es texto libre de usuario -- si contuviera
/// literalmente `*/` cerraría el comentario JSDoc antes de tiempo y
/// corrompería el `.d.ts` generado. `*<wbr>/` con un espacio no es válido
/// JS, así que separar los dos caracteres alcanza para neutralizarlo sin
/// perder legibilidad.
fn jsdoc_escape(reason: &str) -> String {
    reason.replace("*/", "* /")
}

fn emit_type_decl(out: &mut String, t: &TypeDecl, checker: &Checker) -> Result<(), String> {
    let generics = type_params_suffix(&t.type_params);
    match &t.ty {
        TypeExpr::Struct(fields) => {
            out.push_str(&format!("export interface {}{} {{\n", t.name, generics));
            for f in fields {
                let ty = resolve_field_ty(checker, &f.ty, &t.type_params)?;
                push_field_jsdoc(out, f.deprecated(), f.validator());
                out.push_str(&format!(
                    "  {}{}: {};\n",
                    f.name,
                    // Un campo con `= default` (GRAMMAR.md §3.74) puede
                    // omitirse de un literal armado del lado TS igual que
                    // uno `?:` -- mismo criterio que ya usa
                    // `emit_service_interface` para un parámetro de rpc con
                    // default (`p.default.is_some()` -> `?` en la firma).
                    if f.optional || f.default.is_some() { "?" } else { "" },
                    render_type(&ty)
                ));
            }
            out.push_str("}\n\n");
        }
        other => {
            let ty = resolve_field_ty(checker, other, &t.type_params)?;
            out.push_str(&format!("export type {}{} = {};\n\n", t.name, generics, render_type(&ty)));
        }
    }
    Ok(())
}

/// ¿`name` es un enum "simple" (todas sus variantes unitarias)? Esa es la
/// distinción que decide entre "string plano" y "objeto con tag `type`"
/// en TODOS lados: la firma emitida (`emit_enum_decl`), el valor
/// serializado (`runtime/mod.rs::value_to_json`) y el valor de un `const`
/// (`render_const_value`). Que los tres coincidan no es opcional.
fn is_simple_enum(name: &str, checker: &Checker) -> bool {
    checker
        .enums
        .get(name)
        .is_some_and(|e| e.variants.iter().all(|v| v.fields.is_none()))
}

fn emit_enum_decl(out: &mut String, e: &EnumDecl, checker: &Checker) -> Result<(), String> {
    let generics = type_params_suffix(&e.type_params);
    let all_unit = e.variants.iter().all(|v| v.fields.is_none());
    if all_unit {
        // enum simple -> unión de literales string (GRAMMAR.md §4). Un
        // enum así no puede USAR su parámetro de tipo (no hay campos donde
        // meter T), pero la sintaxis lo permite -- y si se declaró, hay que
        // CONSERVARLO en la firma emitida: `render_type` sigue produciendo
        // `G<number>` para una instanciación, así que emitir `type G = ...`
        // a secas daba `TS2315: Type 'G' is not generic` en los tres
        // archivos generados. Un parámetro de tipo sin usar es TS válido;
        // referenciar como genérico algo que no lo es, no.
        let variants: Vec<String> = e.variants.iter().map(|v| format!("\"{}\"", v.name)).collect();
        out.push_str(&format!("export type {}{} = {};\n\n", e.name, generics, variants.join(" | ")));
        return Ok(());
    }
    // ADT -> unión discriminada con tag `type` (GRAMMAR.md §4)
    out.push_str(&format!("export type {}{} =\n", e.name, generics));
    for v in &e.variants {
        let mut parts = vec![format!("type: \"{}\"", v.name)];
        if let Some(fields) = &v.fields {
            for f in fields {
                let ty = resolve_field_ty(checker, &f.ty, &e.type_params)?;
                parts.push(format!(
                    "{}{}: {}",
                    f.name,
                    if f.optional || f.default.is_some() { "?" } else { "" },
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
        push_rpc_jsdoc(out, rpc.doc.as_deref(), rpc.deprecated());
        out.push_str(&format!("  {}({}): {};\n", rpc.name, params.join(", "), ret_str));
    }
    // Auth v0 (GRAMMAR.md §3.14): parte de la interfaz pública, no solo de
    // `{Service}ClientImpl` -- sin esto, algo tipado como `{Service}Client`
    // (lo que devuelve `create{Service}Client`) no podría llamar
    // `.setToken(...)`, solo la clase concreta podría.
    out.push_str("  setToken(token: string | null): void;\n");
    out.push_str("}\n\n");
    Ok(())
}

/// Type resuelto -> string TypeScript, siguiendo GRAMMAR.md §4 al pie de la letra.
pub(crate) fn render_type(ty: &Type) -> String {
    match ty {
        Type::Int | Type::Float => "number".to_string(),
        // NO "bigint" -- ver GRAMMAR.md §3.30. El wire ya manda un string
        // (json_to_typed_value/value_to_json en runtime/mod.rs) para no
        // perder precisión; emitir el mismo `string` acá evita necesitar
        // cualquier glue de (de)serialización nueva en el cliente generado
        // (push_fetch_call/emit_client no tienen hoy ningún punto de
        // conversión dirigido por tipo, y agregarlo sería arquitectura
        // nueva, no una extensión del patrón de Int). Quien necesite
        // aritmética real hace `BigInt(x)` a mano.
        Type::Int64 => "string".to_string(),
        // String ISO-8601 plano, no branded -- GRAMMAR.md §3.31: el mismo
        // criterio minimalista que el resto del proyecto, revisar branding
        // si aparece un caso real que lo pida.
        Type::Timestamp => "string".to_string(),
        Type::String => "string".to_string(),
        // Sin brand nominal (`string & { __uuid: true }`) -- mismo criterio
        // minimalista que Timestamp arriba: TypeScript no necesita distinguir
        // un Uuid de un string cualquiera para que el contrato sea útil, la
        // validación real (GRAMMAR.md §3.70) vive en validators.ts, no en el
        // sistema de tipos de TS.
        Type::Uuid => "string".to_string(),
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
        // Instanciación de un genérico DECLARADO POR EL USUARIO (GRAMMAR.md
        // §3.6) -- a diferencia de Result/Patch/Map, TS ya soporta genéricos
        // nativos: alcanza con emitir el nombre + args, sin expandir la
        // estructura inline (eso ya lo hace TS al instanciar `Box<number>`).
        Type::Generic(name, args) => {
            format!("{name}<{}>", args.iter().map(render_type).collect::<Vec<_>>().join(", "))
        }
        // Solo aparece al emitir la declaración ABSTRACTA de un genérico
        // (`interface Box<T>`, ver resolve_type_abstract) -- se renderiza
        // como el nombre literal del parámetro de tipo.
        Type::TypeParam(name) => name.clone(),
        Type::Dynamic => "unknown".to_string(),
        // Cada miembro pasa por render_type_atom (no render_type) por la
        // misma razón que List en su elemento: un miembro Function (`=>`)
        // sin paréntesis rompe la precedencia de TS dentro de un `|`
        // (ver la nota de render_type_atom más abajo).
        Type::Union(members) => members
            .iter()
            .map(render_type_atom)
            .collect::<Vec<_>>()
            .join(" | "),
        // `db`/`db.<coleccion>`/`auth`/`Service`/`math`/`crypto`/`http`/`json`/`base64` son internos del checker
        Type::Db | Type::DbCollection(_) | Type::Auth | Type::Service(_) | Type::Math | Type::Crypto | Type::Http | Type::Json | Type::Base64 | Type::Env | Type::Request | Type::Smtp | Type::Response => {
            unreachable!("Type::Db/DbCollection/Auth/Service/Math/Crypto/Http/Json/Base64 nunca aparece en un TypeExpr real")
        }
    }
}

/// Nombres de tipos declarados (structs/enums) y builtins (Result/Patch)
/// referenciados por `ty`, para saber qué importar de "./contract" en
/// client.ts. Los tipos estructurales (Optional/List/Tuple/Function) no
/// tienen nombre propio — solo se recorren para encontrar los que sí.
pub(crate) fn collect_type_names(ty: &Type, names: &mut std::collections::BTreeSet<String>) {
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
        // A diferencia de Result/Patch/Map (builtins de TS), un genérico
        // DECLARADO POR EL USUARIO (`Box<T>`) sí se emite como su propia
        // interface/type -- necesita import, igual que un struct/enum normal.
        Type::Generic(name, args) => {
            names.insert(name.clone());
            for a in args {
                collect_type_names(a, names);
            }
        }
        Type::Union(members) => {
            for m in members {
                collect_type_names(m, names);
            }
        }
        Type::Int | Type::Int64 | Type::Timestamp | Type::Float | Type::String | Type::Uuid | Type::Bool | Type::Void | Type::Null | Type::Dynamic | Type::TypeParam(_) => {}
        Type::Db | Type::DbCollection(_) | Type::Auth | Type::Service(_) | Type::Math | Type::Crypto | Type::Http | Type::Json | Type::Base64 | Type::Env | Type::Request | Type::Smtp | Type::Response => {
            unreachable!("Type::Db/DbCollection/Auth/Service/Math/Crypto/Http/Json/Base64 nunca aparece en un TypeExpr real")
        }
    }
}

/// `T[]` con `T = A | null` daría `A | null[]` — que TS parsea como `A |
/// (null[])`, no como `(A | null)[]`. Se envuelve en paréntesis cualquier
/// tipo cuya forma renderizada use `|` o `=>` en su nivel superior.
fn render_type_atom(ty: &Type) -> String {
    match ty {
        Type::Optional(_) | Type::Function(_, _) | Type::Union(_) => format!("({})", render_type(ty)),
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
        let program = parse(tokens).unwrap_or_else(|e| panic!("{e:?}"));
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

    /// Regresión: `emit_client` usaba `.find_map()` sobre `program.items` y
    /// se detenía en el primer `Item::Service` -- un programa con más de un
    /// `service` (GRAMMAR.md no lo limita a uno) generaba `client.ts` con
    /// una sola clase, silenciosamente, sin ningún error. `contract.d.ts`
    /// (interfaz `{Name}Client` por servicio, chequeado abajo) y `hooks.ts`
    /// (misma función hermana, mismo archivo) ya iteraban todos los
    /// servicios correctamente -- solo esta función se quedaba corta.
    #[test]
    fn emit_client_covers_every_service_not_just_the_first() {
        let src = r#"
            type A = { id: Int, x: Int }
            type B = { id: Int, y: Int }
            db { as: A[], bs: B[] }
            service SvcA { rpc list() -> A[] { db.as.all() } }
            service SvcB { rpc list() -> B[] { db.bs.all() } }
            service SvcC { rpc list() -> B[] { db.bs.all() } }
        "#;
        let (contract, client) = emit_both(src);

        // Las tres interfaces ya se emitían bien -- confirma que el bug era
        // solo de emit_client, no del checker ni de emit_contract.
        assert!(contract.contains("export interface SvcAClient"));
        assert!(contract.contains("export interface SvcBClient"));
        assert!(contract.contains("export interface SvcCClient"));

        for name in ["SvcA", "SvcB", "SvcC"] {
            assert!(
                client.contains(&format!("class {name}ClientImpl implements {name}Client")),
                "falta la clase de '{name}' en client.ts:\n{client}"
            );
            assert!(
                client.contains(&format!("export function create{name}Client(baseUrl: string): {name}Client")),
                "falta la factory de '{name}' en client.ts:\n{client}"
            );
        }

        // Los helpers compartidos (transporte, validación, Result guards)
        // se emiten UNA sola vez, no una copia por servicio.
        assert_eq!(client.matches("export class LinkTransportError").count(), 1);
        assert_eq!(client.matches("export class LinkValidationError").count(), 1);
        assert_eq!(client.matches("export function isOk").count(), 1);
    }

    /// `linkc --version` / cada archivo generado (PLAN.md §9.7, GRAMMAR.md
    /// §3.83): el header de `contract.d.ts`/`client.ts`/`hooks.ts` queda
    /// estampado con `crate::VERSION` -- la MISMA constante que
    /// `linkc --version` imprime, así que nunca pueden desincronizarse.
    #[test]
    fn contract_client_and_hooks_headers_are_stamped_with_the_compiler_version() {
        let src = "type Item = { id: Int } db { items: Item[] } service S { rpc list() -> Item[] { db.items.all() } }";
        let (contract, client) = emit_both(src);
        let expected = format!("// Generado automáticamente por linkc v{} — no editar a mano.", crate::VERSION);
        assert!(contract.starts_with(&expected), "{contract}");
        assert!(client.starts_with(&expected), "{client}");

        let tokens = tokenize(src).unwrap_or_else(|e| panic!("{e}"));
        let program = parse(tokens).unwrap_or_else(|e| panic!("{e:?}"));
        let hooks = emit_hooks(&program).unwrap_or_else(|e| panic!("{e}"));
        assert!(hooks.starts_with(&expected), "{hooks}");
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
    fn int64_emits_as_ts_string_not_number_or_bigint() {
        // GRAMMAR.md §3.30: string en el wire y en TS, para no perder
        // precisión -- ni "number" (perdería precisión arriba de 2^53) ni
        // "bigint" (necesitaría glue de (de)serialización nueva en el
        // cliente que esta ronda deliberadamente no construye).
        let src = r#"
            type Counter = { id: Int, big: Int64 }
            service S { rpc get() -> Counter { db.thing.get() } }
        "#;
        let (contract, _) = emit_both(src);
        assert!(contract.contains("big: string;"), "contrato real: {contract}");
    }

    #[test]
    fn timestamp_emits_as_plain_ts_string_not_branded() {
        let src = r#"
            type Event = { at: Timestamp }
            service S { rpc get() -> Event { db.thing.get() } }
        "#;
        let (contract, _) = emit_both(src);
        assert!(contract.contains("at: string;"), "contrato real: {contract}");
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

    #[test]
    fn const_decl_is_emitted_into_the_client_not_the_ambient_contract() {
        // Un .d.ts es un archivo de declaraciones AMBIENTALES: TypeScript
        // rechaza cualquier inicializador ahí (TS1039), así que emitir el
        // const en el contrato hacía que ningún programa con un `const`
        // produjera un contrato compilable -- bug de la auditoría. El valor
        // vive en client.ts, que sí es un módulo real.
        let src = r#"
            const MAX_RETRIES: Int = 3;
            service S { rpc ping() -> Int { 1 } }
        "#;
        let (contract, client) = emit_both(src);
        assert!(
            !contract.contains("MAX_RETRIES"),
            "un const no puede ir al .d.ts: los inicializadores son ilegales en contexto ambiental"
        );
        assert!(client.contains("export const MAX_RETRIES: number = 3;"), "{client}");
    }

    #[test]
    fn a_const_of_a_simple_enum_uses_the_bare_string_form() {
        // Misma distinción all_unit que ya hacen emit_enum_decl y
        // value_to_json: un enum simple ES un string en el wire y en el
        // tipo emitido, así que `{ type: "Admin" }` no era ni siquiera
        // asignable al `type Role = "Admin" | "Member"` de dos líneas
        // más arriba en el propio contrato generado.
        let src = r#"
            enum Role { Admin, Member }
            enum Shape { Circle { r: Int }, Square { s: Int } }
            const DEF: Role = Role.Admin {};
            const SH: Shape = Shape.Circle { r: 1 };
            service S { rpc ping() -> Int { 1 } }
        "#;
        let (_, client) = emit_both(src);
        assert!(client.contains(r#"export const DEF: Role = "Admin";"#), "{client}");
        // Un ADT sí conserva el tag `type`.
        assert!(client.contains(r#"export const SH: Shape = { type: "Circle", r: 1 };"#), "{client}");
        // Y el tipo del const tiene que estar importado para que compile.
        assert!(client.contains("Role"), "el tipo de un const emitido acá tiene que importarse");
    }

    #[test]
    fn an_all_unit_generic_enum_keeps_its_type_parameters() {
        // `render_type` sigue produciendo `G<number>` para una
        // instanciación, así que emitir `type G = ...` sin parámetros daba
        // TS2315 ("Type 'G' is not generic") en los tres archivos.
        let src = r#"
            enum G<T> { A, B }
            service S { rpc g() -> G<Int> { G.A {} } }
        "#;
        let (contract, _) = emit_both(src);
        assert!(contract.contains(r#"export type G<T> = "A" | "B";"#), "{contract}");
    }

    #[test]
    fn const_with_a_non_literal_value_is_rejected_by_the_emitter() {
        let src = r#"
            fn compute() -> Int { 1 + 1 }
            const TOTAL: Int = compute();
            service S { rpc ping() -> Int { 1 } }
        "#;
        let tokens = tokenize(src).unwrap_or_else(|e| panic!("{e}"));
        let program = parse(tokens).unwrap_or_else(|e| panic!("{e:?}"));
        let result = emit_client(&program);
        assert!(result.is_err(), "un const cuyo valor es una llamada no tiene forma de literal TS");
    }

    #[test]
    fn user_generic_struct_emits_real_ts_generic_not_monomorphized() {
        // La declaración se emite UNA vez, como genérico real de TS -- no
        // una interface por cada instanciación usada (eso es cosa del
        // checker/runtime internos, GRAMMAR.md §3.6).
        let src = r#"
            type Box<T> = { value: T }
            service S {
                rpc get() -> Box<Int> { db.thing.get() }
            }
        "#;
        let (contract, _) = emit_both(src);
        assert!(contract.contains("export interface Box<T> {"));
        assert!(contract.contains("value: T;"));
        assert!(contract.contains("get(): Promise<Box<number>>;"));
    }

    #[test]
    fn union_field_renders_as_ts_pipe_type() {
        let src = "type Event = { payload: Int | String }";
        let (contract, _) = emit_both(src);
        assert!(contract.contains("payload: number | string;"));
    }

    #[test]
    fn union_inside_list_gets_parenthesized_to_avoid_ts_ambiguity() {
        // `(Int | String)[]` es sintaxis real del lenguaje (agrupación pura
        // + postfix `[]`, GRAMMAR.md §2.2) -- sin paréntesis en la salida,
        // `number | string[]` significa en TS `number | (string[])`, no
        // `(number | string)[]` (ver la nota en render_type_atom).
        let src = "type Basket = { items: (Int | String)[] }";
        let (contract, _) = emit_both(src);
        assert!(contract.contains("items: (number | string)[];"));
    }

    #[test]
    fn user_generic_enum_emits_discriminated_union_with_type_param() {
        let src = r#"
            enum Option<T> { Some { value: T }, None }
            service S {
                rpc get() -> Option<String> { db.thing.get() }
            }
        "#;
        let (contract, _) = emit_both(src);
        assert!(contract.contains("export type Option<T> ="));
        assert!(contract.contains("| { type: \"Some\"; value: T }"));
        assert!(contract.contains("get(): Promise<Option<string>>;"));
    }

    // ---- auth v0 (GRAMMAR.md §3.14) ----

    #[test]
    fn client_interface_and_impl_both_expose_set_token() {
        // `setToken` tiene que estar en la INTERFAZ (contract.d.ts), no
        // solo en la clase concreta -- si no, algo tipado como
        // `{Service}Client` (lo que devuelve `create{Service}Client`) no
        // podría llamarlo.
        let src = r#"
            enum Role { Admin, Member }
            service S {
                @authenticated
                rpc me() -> Int { 1 }
            }
        "#;
        let (contract, client) = emit_both(src);
        assert!(contract.contains("setToken(token: string | null): void;"), "{contract}");
        assert!(client.contains("private token: string | null = null;"), "{client}");
        assert!(client.contains("setToken(token: string | null): void {"), "{client}");
    }

    #[test]
    fn every_fetch_call_conditionally_attaches_the_bearer_header() {
        // El cliente manda el header si tiene token seteado para
        // CUALQUIER rpc, sin importar si tiene @authenticated/@requires --
        // es el server el que decide caso por caso si lo exige.
        let src = r#"
            enum Role { Admin }
            service S {
                @requires(Role.Admin)
                rpc deleteThing(id: Int) -> Void { }

                stream watch() -> Int { [] }
            }
        "#;
        let (_, client) = emit_both(src);
        let auth_header = "...(this.token ? { Authorization: `Bearer ${this.token}` } : {})";
        let count = client.matches(auth_header).count();
        assert_eq!(count, 2, "esperaba el header condicional en el rpc normal Y en el stream: {client}");
    }

    #[test]
    fn emit_hooks_generates_queries_mutations_and_subscriptions() {
        let src = r#"
            type User = { id: Int, name: String }
            service Users {
                rpc list() -> User[] { [] }
                rpc create(name: String) -> User { User { id: 1, name: name } }
                stream watch() -> User { [] }
            }
        "#;
        let program = crate::parser::parse(crate::lexer::tokenize(src).unwrap()).unwrap();
        let hooks = emit_hooks(&program).expect("hooks generation");
        assert!(hooks.contains("export function useUsersListQuery"), "{hooks}");
        assert!(hooks.contains("export function useUsersCreateMutation"), "{hooks}");
        assert!(hooks.contains("export function useUsersWatch("), "{hooks}");
        assert!(hooks.contains("for await (const item of client.watch())"), "{hooks}");
    }

    /// El hook de Query (`use{Servicio}{Rpc}Query`) comparte cache entre
    /// TODAS sus instancias (GRAMMAR.md §3.124) -- `useSyncExternalStore`
    /// sobre una entrada de un `Map` global, clave por rpc+parámetros. Esto
    /// también resuelve el problema de la respuesta fuera de orden (ej. un
    /// buscador llamando al hook por cada letra tipeada): la clave de una
    /// request vieja Y una nueva son DISTINTAS entradas del cache (params
    /// distintos), así que nunca se pisan entre sí -- y dos instancias con
    /// los MISMOS parámetros comparten una sola request en vuelo en vez de
    /// disparar un fetch cada una.
    #[test]
    fn query_hook_shares_a_cache_entry_keyed_by_rpc_and_params() {
        let src = r#"
            type User = { id: Int, name: String }
            service Users {
                rpc get(id: Int) -> User { User { id: id, name: "x" } }
            }
        "#;
        let program = crate::parser::parse(crate::lexer::tokenize(src).unwrap()).unwrap();
        let hooks = emit_hooks(&program).expect("hooks generation");
        assert!(
            hooks.contains("import { useState, useEffect, useCallback, useRef, useSyncExternalStore } from \"react\";"),
            "{hooks}"
        );
        // Infraestructura de cache emitida UNA sola vez, no por hook.
        assert_eq!(hooks.matches("const queryCache = new Map").count(), 1, "{hooks}");
        assert_eq!(hooks.matches("function getQueryCacheEntry").count(), 1, "{hooks}");
        assert_eq!(hooks.matches("function setQueryCacheState").count(), 1, "{hooks}");
        // Clave del cache: rpc + parámetros serializados, NO el `client`.
        assert!(hooks.contains("const cacheKey = \"Users.get(\" + JSON.stringify([id]) + \")\";"), "{hooks}");
        assert!(hooks.contains("const entry = getQueryCacheEntry<User>(cacheKey);"), "{hooks}");
        assert!(hooks.contains("const state = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);"), "{hooks}");
        // Dedupe real: una sola request en vuelo por entrada, compartida
        // entre quien la disparó y cualquier otra instancia/llamada.
        assert!(hooks.contains("if (!entry.promise) {"), "{hooks}");
        assert!(hooks.contains("entry.promise = client.get(id)"), "{hooks}");
        // La forma pública del hook (lo que un componente consume) sigue
        // siendo exactamente `QueryState<T>` -- el cambio es interno.
        assert!(hooks.contains("export function useUsersGetQuery(client: UsersClient, id: number, options?: { enabled?: boolean }): QueryState<User> {"), "{hooks}");
        assert!(hooks.contains("return { data: state.data, loading: state.loading, error: state.error, refetch };"), "{hooks}");
    }

    /// Un programa SIN ningún rpc que genere un hook de Query (todo
    /// mutations) no debe sumar `useSyncExternalStore` al import ni la
    /// infraestructura de cache -- un import/`const`/`function` de nivel
    /// superior sin usar rompe cualquier build con `noUnusedLocals`
    /// (`examples/taskboard/frontend/tsconfig.json`, entre otras).
    #[test]
    fn a_program_with_only_mutations_does_not_emit_the_query_cache_infrastructure() {
        let src = r#"
            type User = { id: Int, name: String }
            service Users {
                rpc create(name: String) -> User { User { id: 1, name: name } }
            }
        "#;
        let program = crate::parser::parse(crate::lexer::tokenize(src).unwrap()).unwrap();
        let hooks = emit_hooks(&program).expect("hooks generation");
        assert!(!hooks.contains("useSyncExternalStore"), "{hooks}");
        assert!(!hooks.contains("queryCache"), "{hooks}");
        assert!(!hooks.contains("getQueryCacheEntry"), "{hooks}");
        assert!(hooks.contains("import { useState, useEffect, useCallback, useRef } from \"react\";"), "{hooks}");
    }

    /// Misma guarda que el hook de Query, pero para Mutation -- un doble
    /// click en un botón de submit dispara dos `mutate()` casi juntos, y
    /// `reset()` también invalida cualquier `mutate()` en vuelo (si no,
    /// una respuesta tardía de ANTES del reset podría pisar el estado
    /// recién limpiado).
    #[test]
    fn mutation_hook_guards_against_a_stale_response_and_reset_invalidates_in_flight_requests() {
        let src = r#"
            type User = { id: Int, name: String }
            service Users {
                rpc create(name: String) -> User { User { id: 1, name: name } }
            }
        "#;
        let program = crate::parser::parse(crate::lexer::tokenize(src).unwrap()).unwrap();
        let hooks = emit_hooks(&program).expect("hooks generation");
        assert!(hooks.contains("export function useUsersCreateMutation"), "{hooks}");
        assert!(hooks.contains("const requestIdRef = useRef(0);"), "{hooks}");
        assert!(hooks.contains("const requestId = ++requestIdRef.current;"), "{hooks}");
        assert!(hooks.contains("if (requestIdRef.current === requestId) setData(res);"), "{hooks}");
        let reset_block = hooks.split("const reset = useCallback(() => {").nth(1).expect("bloque de reset");
        assert!(reset_block.trim_start().starts_with("requestIdRef.current++;"), "{hooks}");
    }

    /// `@deprecated("...")` sobre un campo se propaga como comentario JSDoc
    /// justo antes del campo en `contract.d.ts` (GRAMMAR.md §3.71) -- un
    /// campo sin la anotación no gana ningún comentario.
    #[test]
    fn deprecated_field_gets_a_jsdoc_comment_in_the_contract() {
        let src = r#"
            type Lead = { id: Int, @deprecated("usa email en su lugar") legacyPhone: String, email: String }
        "#;
        let (contract, _) = emit_both(src);
        assert!(
            contract.contains("/** @deprecated usa email en su lugar */\n  legacyPhone:"),
            "{contract}"
        );
        // `email` no está deprecado -- no debe llevar comentario adelante.
        assert!(!contract.contains("*/\n  email:"), "{contract}");
    }

    /// `@deprecated("...")` sobre un rpc se propaga como comentario JSDoc
    /// justo antes de la firma del método en la interfaz `{Service}Client`.
    #[test]
    fn deprecated_rpc_gets_a_jsdoc_comment_on_the_client_method() {
        let src = r#"
            service S {
                @deprecated("usa listV2")
                rpc list() -> Int { 1 }
                rpc listV2() -> Int { 2 }
            }
        "#;
        let (contract, _) = emit_both(src);
        assert!(contract.contains("/** @deprecated usa listV2 */\n  list()"), "{contract}");
        assert!(!contract.contains("*/\n  listV2("), "{contract}");
    }

    /// Un motivo con `*/` literal no puede cerrar el comentario JSDoc antes
    /// de tiempo -- ver `jsdoc_escape`.
    #[test]
    fn a_deprecated_reason_containing_close_comment_is_escaped() {
        let src = r#"type T = { id: Int, @deprecated("viejo */ ignorar esto") x: String }"#;
        let (contract, _) = emit_both(src);
        assert!(!contract.contains("viejo */ ignorar"), "{contract}");
        assert!(contract.contains("viejo * / ignorar"), "{contract}");
    }

    /// Un docstring `///` sobre un rpc (GRAMMAR.md §3.72) se propaga como
    /// bloque JSDoc multilínea justo antes de la firma del método en la
    /// interfaz `{Service}Client`.
    #[test]
    fn a_docstring_on_an_rpc_becomes_a_multiline_jsdoc_block() {
        let src = r#"
            service Tasks {
                /// Crea una tarea nueva.
                /// El titulo no puede estar vacio.
                rpc create(title: String) -> Int { 1 }
            }
        "#;
        let (contract, _) = emit_both(src);
        assert!(
            contract.contains("  /**\n   * Crea una tarea nueva.\n   * El titulo no puede estar vacio.\n   */\n  create("),
            "{contract}"
        );
    }

    /// Docstring Y `@deprecated` juntos: un solo bloque JSDoc, con
    /// `@deprecated` como su propia línea de tag al final -- no dos
    /// comentarios separados.
    #[test]
    fn a_docstring_and_deprecated_combine_into_one_jsdoc_block() {
        let src = r#"
            service Tasks {
                /// Lista todas las tareas.
                @deprecated("usa listV2")
                rpc list() -> Int { 1 }
            }
        "#;
        let (contract, _) = emit_both(src);
        assert!(
            contract.contains("  /**\n   * Lista todas las tareas.\n   * @deprecated usa listV2\n   */\n  list("),
            "{contract}"
        );
    }

    /// Un rpc sin docstring ni `@deprecated` no gana ningún comentario --
    /// comportamiento sin cambios respecto de antes de esta ronda.
    #[test]
    fn an_rpc_with_neither_doc_nor_deprecated_gets_no_jsdoc() {
        let src = r#"
            service Tasks {
                rpc list() -> Int { 1 }
            }
        "#;
        let (contract, _) = emit_both(src);
        assert!(!contract.contains("/**"), "{contract}");
        assert!(!contract.contains("/*"), "{contract}");
    }

    /// `@validate(email)`/`@validate(regex, "...")` sobre un campo (GRAMMAR.md
    /// §3.73) se propagan como comentario informativo -- sin tag JSDoc
    /// estándar propio, texto libre "Formato: ...".
    #[test]
    fn validate_email_and_regex_get_an_informative_jsdoc_comment() {
        let src = r#"
            type Signup = {
                @validate(email) email: String,
                @validate(regex, "^[A-Z]{3}$") code: String,
            }
        "#;
        let (contract, _) = emit_both(src);
        assert!(contract.contains("/** Formato: email */\n  email:"), "{contract}");
        assert!(contract.contains("/** Formato: coincide con /^[A-Z]{3}$/ */\n  code:"), "{contract}");
    }

    /// `@validate` Y `@deprecated` juntos combinan en un solo bloque JSDoc.
    #[test]
    fn validate_and_deprecated_on_the_same_field_combine_into_one_block() {
        let src = r#"type Signup = { @validate(email) @deprecated("usa emailV2") email: String }"#;
        let (contract, _) = emit_both(src);
        assert!(
            contract.contains("  /**\n   * Formato: email\n   * @deprecated usa emailV2\n   */\n  email:"),
            "{contract}"
        );
    }

    /// Un campo con `= default` (GRAMMAR.md §3.74) se emite opcional (`?`)
    /// en la interfaz -- puede omitirse igual que uno `?:` -- mismo criterio
    /// que ya usa un parámetro de rpc con default en su firma TS.
    #[test]
    fn a_field_with_a_default_is_emitted_as_optional_in_the_interface() {
        let src = r#"type Task = { title: String, status: String = "pending" }"#;
        let (contract, _) = emit_both(src);
        assert!(contract.contains("title: string;"), "{contract}");
        assert!(contract.contains("status?: string;"), "{contract}");
    }

    /// Sin default (ni `?:`), el campo sigue siendo requerido en la
    /// interfaz -- este test evita que el cambio de arriba se vuelva "todo
    /// campo es opcional" por accidente.
    #[test]
    fn a_field_without_a_default_stays_required_in_the_interface() {
        let src = "type Task = { status: String }";
        let (contract, _) = emit_both(src);
        assert!(contract.contains("status: string;"), "{contract}");
        assert!(!contract.contains("status?:"), "{contract}");
    }
}
