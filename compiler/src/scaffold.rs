// `linkc new <nombre>`: crea un proyecto mínimo a partir de las plantillas en
// ../templates/ -- un main.link autocontenido (sin `db`, para no necesitar
// explicar seed data en el primer minuto) más un frontend/ calcado del que
// ya funciona en la raíz de este repo.

use std::fs;
use std::path::Path;
use std::process::ExitCode;

const MAIN_LINK: &str = include_str!("../templates/main.link");
const FRONTEND_PACKAGE_JSON: &str = include_str!("../templates/frontend/package.json");
const FRONTEND_TSCONFIG: &str = include_str!("../templates/frontend/tsconfig.json");
const FRONTEND_MAIN_TS: &str = include_str!("../templates/frontend/src/main.ts");
const PROJECT_README: &str = include_str!("../templates/PROJECT_README.md");

/// Rutas relativas + contenido de cada archivo del scaffold, con
/// `__PROJECT_NAME__` ya sustituido -- separado de `new_project` para poder
/// testear la generación de contenido sin tocar el filesystem ni el CWD
/// del proceso (`cargo test` corre tests en paralelo; nada acá debería
/// depender del directorio actual).
fn project_files(name: &str) -> [(&'static str, String); 5] {
    [
        ("main.link", MAIN_LINK.to_string()),
        ("frontend/package.json", FRONTEND_PACKAGE_JSON.replace("__PROJECT_NAME__", name)),
        ("frontend/tsconfig.json", FRONTEND_TSCONFIG.to_string()),
        ("frontend/src/main.ts", FRONTEND_MAIN_TS.to_string()),
        ("README.md", PROJECT_README.replace("__PROJECT_NAME__", name)),
    ]
}

pub fn new_project(name: &str) -> ExitCode {
    if name.is_empty() || name.contains(['/', '\\']) || name == ".." {
        eprintln!("nombre de proyecto inválido: '{name}' (no puede estar vacío, ni contener '/' o '\\', ni ser '..')");
        return ExitCode::FAILURE;
    }
    if fs::metadata(name).is_ok() {
        eprintln!("ya existe algo en '{name}' -- elegí otro nombre o borralo primero");
        return ExitCode::FAILURE;
    }

    let files = project_files(name);
    for (rel, _) in &files {
        let path = Path::new(name).join(rel);
        if let Some(parent) = path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                eprintln!("no se pudo crear {}: {e}", parent.display());
                return ExitCode::FAILURE;
            }
        }
    }
    for (rel, contents) in &files {
        let path = Path::new(name).join(rel);
        if let Err(e) = fs::write(&path, contents) {
            eprintln!("no se pudo escribir {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    }

    println!("OK: proyecto '{name}' creado");
    println!("Próximos pasos:");
    println!("  cd {name}");
    println!("  linkc build main.link gen");
    println!("  cd frontend && npm install && npx tsc --noEmit");
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::parse;

    #[test]
    fn project_name_is_substituted_into_package_json_and_readme() {
        let files = project_files("my-app");
        let package_json = files.iter().find(|(rel, _)| *rel == "frontend/package.json").unwrap();
        assert!(package_json.1.contains("\"my-app-frontend\""));
        assert!(!package_json.1.contains("__PROJECT_NAME__"));

        let readme = files.iter().find(|(rel, _)| *rel == "README.md").unwrap();
        assert!(readme.1.contains("my-app"));
        assert!(!readme.1.contains("__PROJECT_NAME__"));
    }

    #[test]
    fn scaffolded_main_link_type_checks() {
        let files = project_files("my-app");
        let main_link = &files.iter().find(|(rel, _)| *rel == "main.link").unwrap().1;
        let tokens = tokenize(main_link).unwrap_or_else(|e| panic!("{e}"));
        let program = parse(tokens).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(
            crate::checker::Checker::check_program(&program).is_ok(),
            "la plantilla de main.link debería tipar limpio"
        );
    }
}
