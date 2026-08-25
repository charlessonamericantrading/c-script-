// Generador de configuración PM2 para producción (`linkc pm2-config`), a la
// par de `linkc docker`/`linkc systemd` -- mismo criterio para quien ya usa
// PM2 como supervisor de procesos en vez de contenedores o una unidad
// systemd.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Genera un `ecosystem.json` (formato NATIVO que `pm2 start
/// ecosystem.json` entiende sin conversión, no el `ecosystem.config.js`
/// alternativo) en `out_path` -- a diferencia de `linkc docker`/`linkc
/// systemd` (un directorio de salida con nombre fijo por archivo), acá el
/// CALLER elige el nombre completo del archivo de salida (flag `-o`),
/// porque un `ecosystem.json` de PM2 suele vivir junto a otros ecosystems
/// del mismo repo, no en un directorio propio.
///
/// `--restart-backoff 30s` va como argumento de `linkc serve` DENTRO de
/// `args`, no como `restart_delay` del lado de PM2 -- GRAMMAR.md §3.92
/// documenta que ese flag nativo existe justamente para REEMPLAZAR la
/// mitigación externa de PM2 (una espera fija) por un backoff exponencial
/// real ante un fallo de conexión a la base; `autorestart: true` sigue
/// siendo responsabilidad de PM2 (reinicio de PROCESO ante un crash), las
/// dos cosas son complementarias, no redundantes -- mismo criterio que
/// `Restart=on-failure` + `RestartSec` en `linkc systemd`.
///
/// Sin `LINK_DATABASE_URL` en el `env` generado, a diferencia de la
/// variable comentada que `linkc docker`/`linkc systemd` sí dejan como
/// referencia -- JSON no tiene comentarios, así que un placeholder acá
/// sería un valor REAL que PM2 pasaría al proceso, apuntando a una base
/// falsa en vez de quedar inerte como en las otras dos plantillas.
pub fn generate_pm2_config(source_file: &str, port: u16, out_path: &Path) -> Result<PathBuf, io::Error> {
    if let Some(parent) = out_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    let app_name = Path::new(source_file).file_stem().and_then(|s| s.to_str()).unwrap_or("app");
    let source_file_name = Path::new(source_file).file_name().and_then(|s| s.to_str()).unwrap_or(source_file);

    let config = format!(
        r#"{{
  "apps": [
    {{
      "name": "{app_name}",
      "script": "linkc",
      "interpreter": "none",
      "args": ["serve", "{source_file_name}", "{port}", "--restart-backoff", "30s"],
      "cwd": ".",
      "instances": 1,
      "exec_mode": "fork",
      "autorestart": true,
      "watch": false,
      "env": {{
        "LINK_ENV": "production"
      }}
    }}
  ]
}}
"#
    );
    fs::write(out_path, config)?;
    Ok(out_path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_pm2_config_produces_valid_json_with_the_real_port_and_no_fake_db_url() {
        let temp_dir = std::env::temp_dir().join(format!("link_pm2_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        let out_path = temp_dir.join("ecosystem.json");

        let returned = generate_pm2_config("users_api.link", 4200, &out_path).unwrap();
        assert_eq!(returned, out_path);

        let content = fs::read_to_string(&out_path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).expect("debe ser JSON válido");
        let app = &json["apps"][0];
        assert_eq!(app["name"], "users_api");
        assert_eq!(app["args"], serde_json::json!(["serve", "users_api.link", "4200", "--restart-backoff", "30s"]));
        assert_eq!(app["autorestart"], true);
        // Ninguna variable de conexión falsa -- JSON no tiene comentarios,
        // así que a diferencia de docker.rs/systemd.rs un placeholder acá
        // sería un valor REAL, no una referencia inerte.
        assert!(!content.contains("LINK_DATABASE_URL"));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn generate_pm2_config_derives_the_app_name_from_the_source_files_stem() {
        let temp_dir = std::env::temp_dir().join(format!("link_pm2_test2_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        let out_path = temp_dir.join("ecosystem.json");

        generate_pm2_config("backend/main.link", 8080, &out_path).unwrap();
        let content = fs::read_to_string(&out_path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(json["apps"][0]["name"], "main");
        assert_eq!(json["apps"][0]["args"][1], "main.link");

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
