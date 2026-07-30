// Resolución de módulos multi-archivo (GRAMMAR.md §2.1) + package manager
// mínimo por rutas locales. `import_decl` ya se parseaba desde el principio
// de la sesión, pero no tenía ningún efecto -- este archivo es lo que lo
// conecta de verdad.
//
// Reglas de diseño deliberadas (no son detalles menores):
//
// - Un import se valida contra los ítems NATIVOS del archivo importado --
//   nunca contra su cierre ya fusionado con SUS PROPIOS imports. Si no fuera
//   así, "A importa X de B, B importa X de C" terminaría implementando
//   re-exports por accidente (decisión explícita: no soportarlos en v0).
// - Sin visibilidad real (`pub`/privado): el Program final que llega al
//   checker es la unión plana de los ítems nativos de TODO archivo
//   alcanzado transitivamente -- el import valida "¿existe ese nombre en
//   ESE archivo puntual?" pero no oculta nada de los demás archivos del
//   cierre. Implementar visibilidad real es una extensión más grande
//   (necesitaría un scoping propio en el checker, que hoy no tiene ningún
//   concepto de "de qué archivo vino este símbolo") -- correctamente fuera
//   de alcance acá.
// - `from` que empieza con "./" o "../" es una ruta relativa al archivo que
//   importa. Un nombre pelado se busca en `dependencies` de un `link.json`
//   en el directorio del archivo de entrada (la raíz del proyecto) -- sin
//   buscar hacia arriba en el árbol de directorios (eso es útil para
//   monorepos, un caso más avanzado que v0 no necesita).
// - Sin lockfile: con dependencias puramente por ruta no hay versión ni
//   conflicto que "lockear" todavía.

use crate::ast::{Item, Program};
use crate::lexer;
use crate::parser;
use crate::token::Span;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

/// Antes, un error de lexer/parser en un archivo importado se envolvía con
/// `.map_err(|e| err(e.to_string()))` -- perdía el `Span` estructurado
/// (`e.to_string()` ya lo había renderizado a texto) Y no anteponía el
/// archivo, a diferencia de los demás mensajes de este módulo (que sí usan
/// `canon.display()`). Con imports multi-archivo, un typo en un archivo
/// importado no decía en cuál.
///
/// `Syntax.errors` es un `Vec` a propósito, aunque hoy SIEMPRE tenga
/// exactamente 1 elemento (lexer y parser todavía devuelven un solo error
/// cada uno) -- cuando el parser gane recuperación de errores (varios
/// errores en una sola pasada), este archivo solo necesita empujar más
/// elementos al mismo `Vec`, sin volver a rediseñar esta forma.
#[derive(Debug)]
pub enum LoadError {
    /// IO, ciclos de imports, dependencia desconocida, manifest inválido --
    /// sin posición en un archivo fuente, se muestran como antes.
    Other(String),
    /// Error léxico o de sintaxis en un archivo concreto del cierre
    /// transitivo. `path` es la ruta CANONICALIZADA (misma convención que
    /// ya usan los demás mensajes de este archivo).
    Syntax { path: PathBuf, errors: Vec<(Span, String)> },
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Other(msg) => write!(f, "error de módulos: {msg}"),
            LoadError::Syntax { path, errors } => {
                for (i, (span, message)) in errors.iter().enumerate() {
                    if i > 0 {
                        writeln!(f)?;
                    }
                    write!(f, "error de módulos: '{}':{}:{}: {message}", display_path(path), span.line, span.col)?;
                }
                Ok(())
            }
        }
    }
}

/// `fs::canonicalize` en Windows antepone el prefijo extendido `\\?\`
/// (`\\?\C:\...`, o `\\?\UNC\...` para una ruta de red) -- hace falta para
/// soportar rutas de más de 260 caracteres, pero no es lo que nadie
/// escribió en la terminal y no aporta nada en un mensaje de error. Se pela
/// SOLO para texto que un humano lee (`Display` de `LoadError`, diagnósticos
/// de `main.rs`) -- la ruta canónica en sí (clave de overlay, comparaciones)
/// se deja intacta en todos los demás lugares. No-op en cualquier otro OS,
/// ya que ahí `canonicalize` nunca produce este prefijo.
pub fn display_path(path: &Path) -> String {
    let s = path.display().to_string();
    s.strip_prefix(r"\\?\UNC\")
        .map(|rest| format!(r"\\{rest}"))
        .or_else(|| s.strip_prefix(r"\\?\").map(str::to_string))
        .unwrap_or(s)
}

fn err(msg: impl Into<String>) -> LoadError {
    LoadError::Other(msg.into())
}

fn canonicalize(path: &Path) -> Result<PathBuf, LoadError> {
    fs::canonicalize(path).map_err(|e| err(format!("no se pudo resolver '{}': {e}", path.display())))
}

/// Carga `entry` y todo su cierre transitivo de imports, devolviendo un
/// `Program` con los ítems de TODOS los archivos alcanzados (sin los
/// `Item::Import` ya resueltos -- el checker no necesita verlos) más la
/// lista de archivos físicos tocados, en el orden en que se leyeron por
/// primera vez (la reutiliza `link dev` para saber qué observar).
pub fn load_program(entry: &Path) -> Result<(Program, Vec<PathBuf>), LoadError> {
    load_program_with_overlay(entry, &HashMap::new())
}

/// Igual que `load_program`, pero cualquier archivo cuya ruta CANONICALIZADA
/// aparezca en `overlay` se lee de ahí (el buffer en memoria de un editor,
/// potencialmente con cambios sin guardar) en vez de `fs::read_to_string` --
/// el seam que el LSP (GRAMMAR.md, protocolo LSP) necesita para chequear el
/// contenido REAL de un documento abierto, no el de disco, que puede estar
/// desactualizado. `load_program` es simplemente el caso `overlay` vacío,
/// así que los 4 call sites existentes (`cmd_check`/`build_once`/`cmd_serve`/
/// los tests de este archivo) no cambian ni de firma ni de comportamiento.
pub fn load_program_with_overlay(entry: &Path, overlay: &HashMap<PathBuf, String>) -> Result<(Program, Vec<PathBuf>), LoadError> {
    let canon_entry = canonicalize(entry)?;
    let project_root = canon_entry.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();
    let mut loader = Loader {
        native_items: HashMap::new(),
        touched: Vec::new(),
        project_root,
        manifest: None,
        overlay,
    };
    let mut on_stack = HashSet::new();
    let mut done = HashSet::new();
    let mut merged = Vec::new();
    loader.visit(&canon_entry, &mut on_stack, &mut done, &mut merged)?;
    Ok((Program { items: merged }, loader.touched))
}

struct Loader<'a> {
    /// Ítems NATIVOS de cada archivo (los que declara él mismo, sin sus
    /// imports resueltos) -- cachea para no re-lexear/parsear el mismo
    /// archivo dos veces en un caso diamante (A y C importan D).
    native_items: HashMap<PathBuf, Vec<Item>>,
    touched: Vec<PathBuf>,
    project_root: PathBuf,
    /// `dependencies` de link.json, resuelto una sola vez y cacheado. `None`
    /// = todavía no se intentó leer; `Some(vacío)` = no hay link.json (o no
    /// declara nada), que es el caso común de un proyecto sin dependencias.
    manifest: Option<HashMap<String, String>>,
    /// Ruta canonicalizada -> contenido en memoria, consultado ANTES que
    /// disco (ver `load_program_with_overlay`). Prestado, no clonado -- el
    /// LSP arma este mapa una vez por re-chequeo a partir de sus documentos
    /// abiertos; clonarlo acá duplicaría el texto de cada archivo abierto
    /// sin ninguna necesidad.
    overlay: &'a HashMap<PathBuf, String>,
}

impl Loader<'_> {
    fn load_native(&mut self, canon: &Path) -> Result<Vec<Item>, LoadError> {
        if let Some(items) = self.native_items.get(canon) {
            return Ok(items.clone());
        }
        let source = match self.overlay.get(canon) {
            Some(text) => text.clone(),
            None => fs::read_to_string(canon).map_err(|e| err(format!("no se pudo leer '{}': {e}", canon.display())))?,
        };
        let tokens = lexer::tokenize(&source).map_err(|e| LoadError::Syntax {
            path: canon.to_path_buf(),
            errors: vec![(e.span, e.message)],
        })?;
        let program = parser::parse(tokens).map_err(|errs| LoadError::Syntax {
            path: canon.to_path_buf(),
            errors: errs.into_iter().map(|e| (e.span, e.message)).collect(),
        })?;
        self.touched.push(canon.to_path_buf());
        self.native_items.insert(canon.to_path_buf(), program.items.clone());
        Ok(program.items)
    }

    fn resolve_import_target(&mut self, from: &str, base_dir: &Path) -> Result<PathBuf, LoadError> {
        if from.starts_with("./") || from.starts_with("../") {
            return canonicalize(&base_dir.join(from));
        }
        if self.manifest.is_none() {
            let manifest_path = self.project_root.join("link.json");
            let deps = if manifest_path.exists() { read_manifest(&manifest_path)? } else { HashMap::new() };
            self.manifest = Some(deps);
        }
        let deps = self.manifest.as_ref().expect("se acaba de asignar arriba");
        let dep_path = deps.get(from).ok_or_else(|| {
            err(format!("'{from}' no es una ruta relativa ('./' o '../') ni una dependencia en link.json"))
        })?;
        canonicalize(&self.project_root.join(dep_path))
    }

    fn visit(
        &mut self,
        canon: &Path,
        on_stack: &mut HashSet<PathBuf>,
        done: &mut HashSet<PathBuf>,
        merged: &mut Vec<Item>,
    ) -> Result<(), LoadError> {
        if done.contains(canon) {
            return Ok(()); // caso diamante: ya se procesó por otro camino
        }
        if !on_stack.insert(canon.to_path_buf()) {
            return Err(err(format!("ciclo de imports detectado en '{}'", canon.display())));
        }

        let items = self.load_native(canon)?;
        let base_dir = canon.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();

        for item in &items {
            if let Item::Import(imp) = item {
                let target = self.resolve_import_target(&imp.from, &base_dir)?;
                // Se valida contra los ítems NATIVOS del archivo destino --
                // nunca contra su cierre ya fusionado (ver nota de arriba).
                let target_native = self.load_native(&target)?;
                for name in &imp.names {
                    if !name_exists_in_namespaces(&target_native, name) {
                        return Err(err(format!(
                            "'{name}' no existe en '{}' (import desde '{}')",
                            target.display(),
                            canon.display()
                        )));
                    }
                }
                self.visit(&target, on_stack, done, merged)?;
            }
        }

        for item in items {
            if !matches!(item, Item::Import(_)) {
                merged.push(item);
            }
        }
        on_stack.remove(canon);
        done.insert(canon.to_path_buf());
        Ok(())
    }
}

/// `types`/`enums`/`fns`/`consts` son namespaces independientes hoy (el
/// checker los guarda en HashMaps separados) -- un import busca en los
/// cuatro y alcanza con que matchee en UNO. `Item::Service` queda afuera
/// a propósito: no es algo que se referencie por nombre en ningún lado del
/// lenguaje hoy, así que "importarlo" no tiene un significado real todavía.
fn name_exists_in_namespaces(items: &[Item], name: &str) -> bool {
    items.iter().any(|item| match item {
        Item::Type(t) => t.name == name,
        Item::Enum(e) => e.name == name,
        Item::Fn(f) => f.name == name,
        Item::Const(c) => c.name == name,
        _ => false,
    })
}

fn read_manifest(path: &Path) -> Result<HashMap<String, String>, LoadError> {
    let content = fs::read_to_string(path).map_err(|e| err(format!("no se pudo leer '{}': {e}", path.display())))?;
    let json: serde_json::Value =
        serde_json::from_str(&content).map_err(|e| err(format!("'{}' no es JSON válido: {e}", path.display())))?;
    let mut map = HashMap::new();
    if let Some(deps) = json.get("dependencies").and_then(|d| d.as_object()) {
        for (k, v) in deps {
            let path_str = v.as_str().ok_or_else(|| {
                err(format!("dependencies.{k} en '{}' debe ser un string (ruta)", path.display()))
            })?;
            map.insert(k.clone(), path_str.to_string());
        }
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Directorio temporal aislado por test -- se limpia solo al salir de
    /// scope (Drop). Evita que tests en paralelo (cargo test los corre así
    /// por default) choquen entre sí escribiendo archivos.
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "cscript-modules-test-{tag}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
        fn write(&self, rel: &str, contents: &str) -> PathBuf {
            let path = self.0.join(rel);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            let mut f = fs::File::create(&path).unwrap();
            f.write_all(contents.as_bytes()).unwrap();
            path
        }
        fn path(&self, rel: &str) -> PathBuf {
            self.0.join(rel)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn basic_two_file_import_merges_both_files_items() {
        let dir = TempDir::new("basic");
        dir.write("b.link", "type Point = { x: Int, y: Int }");
        dir.write(
            "a.link",
            r#"
            import { Point } from "./b.link";
            fn origin() -> Point { Point { x: 0, y: 0 } }
        "#,
        );
        let (program, touched) = load_program(&dir.path("a.link")).unwrap();
        assert_eq!(program.items.len(), 2); // Point + origin, sin el Import
        assert_eq!(touched.len(), 2);
    }

    #[test]
    fn direct_cycle_is_rejected_with_a_clear_error() {
        // Cada archivo tiene que declarar algo real para que la validación
        // de "el nombre importado existe" pase en ambos lados -- si no, el
        // error que se dispara primero es "no existe", no el ciclo (el
        // chequeo de existencia corre ANTES de recursar en el import).
        let dir = TempDir::new("cycle");
        dir.write("a.link", r#"type X = { n: Int } import { Y } from "./b.link";"#);
        dir.write("b.link", r#"type Y = { n: Int } import { X } from "./a.link";"#);
        let result = load_program(&dir.path("a.link"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("ciclo"));
    }

    #[test]
    fn diamond_dependency_is_loaded_only_once() {
        let dir = TempDir::new("diamond");
        dir.write("d.link", "type Shared = { value: Int }");
        dir.write(
            "b.link",
            r#"
            import { Shared } from "./d.link";
            fn use_in_b() -> Shared { Shared { value: 1 } }
        "#,
        );
        dir.write(
            "c.link",
            r#"
            import { Shared } from "./d.link";
            fn use_in_c() -> Shared { Shared { value: 2 } }
        "#,
        );
        dir.write(
            "a.link",
            r#"
            import { use_in_b } from "./b.link";
            import { use_in_c } from "./c.link";
        "#,
        );
        let (program, touched) = load_program(&dir.path("a.link")).unwrap();
        // Shared (de d.link, una sola vez) + use_in_b + use_in_c = 3 ítems,
        // NO 4 (que sería Shared duplicado si d.link se cargara dos veces).
        assert_eq!(program.items.len(), 3);
        assert_eq!(touched.len(), 4); // a, b, c, d -- cada archivo físico, una vez
    }

    #[test]
    fn importing_a_name_that_only_exists_via_the_target_own_imports_is_rejected() {
        // La regla anti-re-export: B importa X de C, pero X NO es un ítem
        // NATIVO de B -- que A intente "import { X } from './b.link'" debe
        // fallar, aunque X sea alcanzable transitivamente a través de B.
        let dir = TempDir::new("no_reexport");
        dir.write("c.link", "type X = { n: Int }");
        dir.write("b.link", r#"import { X } from "./c.link";"#);
        dir.write("a.link", r#"import { X } from "./b.link";"#);
        let result = load_program(&dir.path("a.link"));
        assert!(result.is_err(), "b.link no declara X nativamente, no debería poder re-exportarlo");
    }

    #[test]
    fn importing_an_unknown_name_is_rejected() {
        let dir = TempDir::new("unknown_name");
        dir.write("b.link", "type Point = { x: Int }");
        dir.write("a.link", r#"import { NoExiste } from "./b.link";"#);
        let result = load_program(&dir.path("a.link"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("NoExiste"));
    }

    #[test]
    fn syntax_error_in_an_imported_file_names_that_file() {
        // Bug real (antes de esta ronda): un error de sintaxis en un
        // archivo IMPORTADO se reportaba sin decir en cuál -- `.to_string()`
        // ya había perdido el Span estructurado y el mensaje no anteponía
        // ningún path, a diferencia de los demás errores de este módulo.
        let dir = TempDir::new("syntax_error_named_file");
        dir.write("b.link", "type Point = { x Int }"); // falta ':' -- error de sintaxis real
        dir.write("a.link", r#"import { Point } from "./b.link";"#);
        let result = load_program(&dir.path("a.link"));
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("b.link"), "el mensaje debería nombrar el archivo con el error: {msg}");
        assert!(msg.contains(':'), "debería incluir línea:columna: {msg}");
    }

    #[test]
    fn multiple_syntax_errors_in_an_imported_file_are_all_surfaced() {
        // Confirma que las dos rondas componen: `LoadError::Syntax.errors`
        // se diseñó como Vec en la ronda anterior específicamente para que
        // la recuperación de errores del parser (esta ronda) no necesitara
        // rediseñarlo -- acá el archivo importado tiene 2 errores reales de
        // sintaxis, independientes entre sí, y ambos tienen que aparecer.
        let dir = TempDir::new("multi_syntax_error_named_file");
        dir.write("b.link", "fn a(*) -> Int { 1 } fn b(*) -> Int { 2 }");
        dir.write("a.link", r#"import { a } from "./b.link";"#);
        let result = load_program(&dir.path("a.link"));
        let err = result.unwrap_err();
        let LoadError::Syntax { errors, .. } = &err else {
            panic!("se esperaba LoadError::Syntax, fue {err:?}");
        };
        assert_eq!(errors.len(), 2, "{err}");
    }

    #[test]
    fn bare_name_import_resolves_via_link_json() {
        let dir = TempDir::new("manifest");
        dir.write("libs/shapes.link", "type Circle = { r: Int }");
        dir.write("link.json", r#"{ "dependencies": { "shapes": "./libs/shapes.link" } }"#);
        dir.write(
            "a.link",
            r#"
            import { Circle } from "shapes";
            fn unit_circle() -> Circle { Circle { r: 1 } }
        "#,
        );
        let (program, _) = load_program(&dir.path("a.link")).unwrap();
        assert_eq!(program.items.len(), 2);
    }
}
