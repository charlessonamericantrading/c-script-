// Generador de unidad systemd para producción (`linkc systemd`), a la par
// de `linkc docker` (docker.rs) -- mismo criterio para quien despliega
// contra una VM/bare metal en vez de un contenedor: un archivo listo para
// copiar a /etc/systemd/system/ y activar con `systemctl enable --now`, sin
// tener que armar la unidad a mano adivinando las opciones de hardening
// correctas.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Genera `<nombre>.service` en `out_dir` -- a diferencia de `linkc docker`
/// (puerto siempre 3000 dentro de la plantilla), acá el puerto es un
/// argumento real: `linkc serve` no tiene un puerto por default, así que la
/// unidad tampoco puede inventarse uno.
pub fn generate_systemd_unit(source_file: &str, port: u16, out_dir: &Path) -> Result<PathBuf, io::Error> {
    fs::create_dir_all(out_dir)?;

    let app_name = Path::new(source_file).file_stem().and_then(|s| s.to_str()).unwrap_or("app");
    let source_file_name = Path::new(source_file).file_name().and_then(|s| s.to_str()).unwrap_or(source_file);

    let unit_path = out_dir.join(format!("{app_name}.service"));
    let unit_content = format!(
        r#"[Unit]
Description=c-script (Link) service -- {app_name}
After=network.target

[Service]
Type=simple
WorkingDirectory=/opt/{app_name}
ExecStart=/usr/local/bin/linkc serve {source_file_name} {port}
Restart=on-failure
RestartSec=5
User=linkuser
Group=linkuser
Environment=LINK_ENV=production
# Descomentá y ajustá esta línea para correr contra PostgreSQL en vez del
# SQLite embebido -- la variable real que `linkc serve` lee es
# LINK_DATABASE_URL (GRAMMAR.md §3.36).
#Environment=LINK_DATABASE_URL=postgres://link_user:secret_password@localhost:5432/{app_name}_db

# Hardening mínimo -- el proceso no necesita ni privilegios de root ni
# escritura fuera de su propio directorio de trabajo (SQLite embebido vive
# ahí adentro).
NoNewPrivileges=true
ProtectSystem=strict
ReadWritePaths=/opt/{app_name}
PrivateTmp=true

[Install]
WantedBy=multi-user.target
"#
    );
    fs::write(&unit_path, unit_content)?;
    Ok(unit_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_systemd_unit_creates_a_service_file_with_the_real_port_and_variable() {
        let temp_dir = std::env::temp_dir().join(format!("link_systemd_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);

        let unit_path = generate_systemd_unit("users_api.link", 4200, &temp_dir).unwrap();
        assert_eq!(unit_path, temp_dir.join("users_api.service"));

        let unit = fs::read_to_string(&unit_path).unwrap();
        assert!(unit.contains("ExecStart=/usr/local/bin/linkc serve users_api.link 4200"));
        assert!(unit.contains("Description=c-script (Link) service -- users_api"));
        assert!(unit.contains("WantedBy=multi-user.target"));
        // Mismo motivo que el test de docker.rs: la variable real que
        // `linkc serve` lee es LINK_DATABASE_URL, no DATABASE_URL/
        // DATABASE_PATH -- ofrecer la equivocada deja a alguien pensando
        // que está en Postgres mientras el proceso sigue en SQLite.
        assert!(unit.contains("LINK_DATABASE_URL"), "la unidad debe nombrar la variable real");
        assert!(!unit.contains("DATABASE_PATH"));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn generate_systemd_unit_derives_the_app_name_from_the_source_files_stem() {
        let temp_dir = std::env::temp_dir().join(format!("link_systemd_test2_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);

        let unit_path = generate_systemd_unit("backend/main.link", 8080, &temp_dir).unwrap();
        assert_eq!(unit_path, temp_dir.join("main.service"));
        let unit = fs::read_to_string(&unit_path).unwrap();
        // El `ExecStart` referencia el archivo tal cual se lo pasaron
        // (nombre base, sin el directorio) -- `WorkingDirectory` es lo que
        // ubica ese archivo en el despliegue real, no una ruta absoluta acá.
        assert!(unit.contains("ExecStart=/usr/local/bin/linkc serve main.link 8080"));

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
