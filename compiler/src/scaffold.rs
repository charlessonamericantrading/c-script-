// `linkc new <nombre> [--template <nextjs|vite|minimal>]`: Scaffoldea proyectos
// de producción completos con backend Link y frontends modernos (Next.js 14, Vite+React o Minimal).

use std::fs;
use std::path::Path;
use std::process::ExitCode;

const MAIN_LINK: &str = include_str!("../templates/main.link");
const FRONTEND_PACKAGE_JSON: &str = include_str!("../templates/frontend/package.json");
const FRONTEND_TSCONFIG: &str = include_str!("../templates/frontend/tsconfig.json");
const FRONTEND_MAIN_TS: &str = include_str!("../templates/frontend/src/main.ts");
const PROJECT_README: &str = include_str!("../templates/PROJECT_README.md");

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Template {
    Minimal,
    Nextjs,
    Vite,
}

impl Template {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "minimal" | "default" => Some(Template::Minimal),
            "next" | "nextjs" => Some(Template::Nextjs),
            "vite" | "react" => Some(Template::Vite),
            _ => None,
        }
    }
}

pub fn project_files(name: &str, template: Template) -> Vec<(String, String)> {
    let mut files = vec![
        ("main.link".to_string(), MAIN_LINK.to_string()),
        ("README.md".to_string(), PROJECT_README.replace("__PROJECT_NAME__", name)),
    ];

    match template {
        Template::Minimal => {
            files.push(("frontend/package.json".to_string(), FRONTEND_PACKAGE_JSON.replace("__PROJECT_NAME__", name)));
            files.push(("frontend/tsconfig.json".to_string(), FRONTEND_TSCONFIG.to_string()));
            files.push(("frontend/src/main.ts".to_string(), FRONTEND_MAIN_TS.to_string()));
        }
        Template::Nextjs => {
            let pkg = format!(
                r#"{{
  "name": "{name}-web",
  "version": "0.1.0",
  "private": true,
  "scripts": {{
    "dev": "next dev",
    "build": "next build",
    "start": "next start",
    "lint": "next lint"
  }},
  "dependencies": {{
    "next": "14.2.3",
    "react": "^18.3.1",
    "react-dom": "^18.3.1"
  }},
  "devDependencies": {{
    "@types/node": "^20",
    "@types/react": "^18",
    "@types/react-dom": "^18",
    "typescript": "^5"
  }}
}}
"#
            );
            let tsconfig = r#"{
  "compilerOptions": {
    "target": "es5",
    "lib": ["dom", "dom.iterable", "esnext"],
    "allowJs": true,
    "skipLibCheck": true,
    "strict": true,
    "noEmit": true,
    "esModuleInterop": true,
    "module": "esnext",
    "moduleResolution": "bundler",
    "resolveJsonModule": true,
    "isolatedModules": true,
    "jsx": "preserve",
    "incremental": true,
    "plugins": [{ "name": "next" }],
    "paths": { "@/*": ["./*"] }
  },
  "include": ["next-env.d.ts", "**/*.ts", "**/*.tsx", ".next/types/**/*.ts"],
  "exclude": ["node_modules"]
}
"#;
            let page = format!(
                r#"'use client';
import {{ createClient }} from '../gen/client';

const client = createClient('http://localhost:3000');

export default function Home() {{
  return (
    <main style={{{{ padding: '3rem', fontFamily: 'system-ui, sans-serif' }}}}>
      <h1>🚀 {name}</h1>
      <p>Next.js 14 App Router + Link Backend Tipado End-to-End</p>
    </main>
  );
}}
"#
            );
            files.push(("web/package.json".to_string(), pkg));
            files.push(("web/tsconfig.json".to_string(), tsconfig.to_string()));
            files.push(("web/app/page.tsx".to_string(), page));
        }
        Template::Vite => {
            let pkg = format!(
                r#"{{
  "name": "{name}-vite",
  "private": true,
  "version": "0.0.0",
  "type": "module",
  "scripts": {{
    "dev": "vite",
    "build": "tsc && vite build",
    "preview": "vite preview"
  }},
  "dependencies": {{
    "react": "^18.3.1",
    "react-dom": "^18.3.1"
  }},
  "devDependencies": {{
    "@types/react": "^18.3.3",
    "@types/react-dom": "^18.3.0",
    "@vitejs/plugin-react": "^4.3.0",
    "typescript": "^5.4.5",
    "vite": "^5.2.11"
  }}
}}
"#
            );
            let app_tsx = format!(
                r#"import React from 'react';

export function App() {{
  return (
    <div style={{{{ padding: '2rem', fontFamily: 'sans-serif' }}}}>
      <h1>⚡ {name} (Vite + React)</h1>
      <p>Conectado a Link Backend con Type Safety completa.</p>
    </div>
  );
}}
"#
            );
            files.push(("frontend/package.json".to_string(), pkg));
            files.push(("frontend/src/App.tsx".to_string(), app_tsx));
        }
    }

    files
}

pub fn new_project_with_template(name: &str, template: Template) -> ExitCode {
    if name.is_empty() || name.contains(['/', '\\']) || name == ".." {
        eprintln!("nombre de proyecto inválido: '{name}' (no puede estar vacío, ni contener '/' o '\\', ni ser '..')");
        return ExitCode::FAILURE;
    }
    if fs::metadata(name).is_ok() {
        eprintln!("ya existe algo en '{name}' -- elegí otro nombre o borralo primero");
        return ExitCode::FAILURE;
    }

    let files = project_files(name, template);
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

    println!("OK: proyecto '{name}' creado con plantilla {:?}", template);
    println!("Próximos pasos:");
    println!("  cd {name}");
    println!("  linkc build main.link gen");
    println!("  linkc serve main.link 3000");
    ExitCode::SUCCESS
}

pub fn new_project(name: &str) -> ExitCode {
    new_project_with_template(name, Template::Minimal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::parse;

    #[test]
    fn project_name_is_substituted_into_package_json_and_readme() {
        let files = project_files("my-app", Template::Minimal);
        let package_json = files.iter().find(|(rel, _)| rel == "frontend/package.json").unwrap();
        assert!(package_json.1.contains("\"my-app-frontend\""));
        assert!(!package_json.1.contains("__PROJECT_NAME__"));

        let readme = files.iter().find(|(rel, _)| rel == "README.md").unwrap();
        assert!(readme.1.contains("my-app"));
        assert!(!readme.1.contains("__PROJECT_NAME__"));
    }

    #[test]
    fn nextjs_template_creates_app_router_files() {
        let files = project_files("acme", Template::Nextjs);
        assert!(files.iter().any(|(r, _)| r == "web/app/page.tsx"));
        assert!(files.iter().any(|(r, _)| r == "web/package.json"));
    }

    #[test]
    fn scaffolded_main_link_type_checks() {
        let files = project_files("my-app", Template::Minimal);
        let main_link = &files.iter().find(|(rel, _)| rel == "main.link").unwrap().1;
        let tokens = tokenize(main_link).unwrap_or_else(|e| panic!("{e}"));
        let program = parse(tokens).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(
            crate::checker::Checker::check_program(&program).is_ok(),
            "la plantilla de main.link debería tipar limpio"
        );
    }
}
