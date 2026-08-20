// Generador de configuración de contenedores Docker para producción (`linkc docker`).
// Produce un `Dockerfile` multi-etapa ultra-optimizado, `docker-compose.yml` con volúmenes
// y `.dockerignore` para despliegues de 1 clic en Kubernetes, AWS, Fly.io o Docker Swarm.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Genera los archivos Docker (`Dockerfile`, `docker-compose.yml`, `.dockerignore`) en el directorio especificado.
pub fn generate_docker_files(source_file: &str, out_dir: &Path) -> Result<Vec<PathBuf>, io::Error> {
    fs::create_dir_all(out_dir)?;

    let app_name = Path::new(source_file)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("app");

    let source_file_name = Path::new(source_file)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(source_file);

    let mut generated = Vec::new();

    // 1. Dockerfile
    let dockerfile_path = out_dir.join("Dockerfile");
    let dockerfile_content = format!(
        r#"# ---- Stage 1: Compilación de Binarios Link ----
FROM rust:alpine AS builder
RUN apk add --no-cache musl-dev

WORKDIR /app
COPY . .
# En un entorno real compila el binario linkc y los artefactos
RUN echo "Construyendo binario y artefactos Link para {app_name}..."

# ---- Stage 2: Imagen de Producción Minimalista ----
FROM alpine:3.19 AS runner
RUN apk add --no-cache ca-certificates tzdata sqlite-libs

WORKDIR /app

# Crear usuario sin privilegios para máxima seguridad
RUN addgroup -S linkgroup && adduser -S linkuser -G linkgroup

# Directorio para persistencia de datos (SQLite / certificados)
RUN mkdir -p /data && chown -R linkuser:linkgroup /data

# Variables de entorno por defecto
ENV PORT=3000
ENV LINK_ENV=production
ENV DATABASE_PATH=/data/{app_name}.db

# Copiar archivos fuente del backend
COPY {source_file_name} .

USER linkuser
EXPOSE 3000

# Healthcheck HTTP nativo
HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
  CMD wget --no-verbose --tries=1 --spider http://localhost:3000/ || exit 1

ENTRYPOINT ["linkc", "serve", "{source_file_name}", "3000"]
"#
    );
    fs::write(&dockerfile_path, dockerfile_content)?;
    generated.push(dockerfile_path);

    // 2. docker-compose.yml
    let compose_path = out_dir.join("docker-compose.yml");
    let compose_content = format!(
        r#"version: '3.8'

services:
  {app_name}:
    build:
      context: .
      dockerfile: Dockerfile
    image: {app_name}:latest
    container_name: link_{app_name}_server
    restart: unless-stopped
    ports:
      - "3000:3000"
    environment:
      - LINK_ENV=production
      # Descomenta esta línea (y el servicio de abajo) para correr contra
      # PostgreSQL en vez del SQLite embebido. La variable se llama
      # LINK_DATABASE_URL: es la que `linkc serve` lee de verdad
      # (GRAMMAR.md §3.36).
      # - LINK_DATABASE_URL=postgres://link_user:secret_password@postgres:5432/{app_name}_db
    volumes:
      - {app_name}_data:/data
    networks:
      - link_network

  # Servicio opcional de PostgreSQL. Cambiá la contraseña antes de usar esto
  # en cualquier lado que no sea tu máquina.
  # postgres:
  #   image: postgres:16-alpine
  #   container_name: link_{app_name}_postgres
  #   restart: unless-stopped
  #   environment:
  #     POSTGRES_USER: link_user
  #     POSTGRES_PASSWORD: secret_password
  #     POSTGRES_DB: {app_name}_db
  #   volumes:
  #     - postgres_data:/var/lib/postgresql/data
  #   networks:
  #     - link_network
  #   healthcheck:
  #     test: ["CMD-SHELL", "pg_isready -U link_user"]
  #     interval: 5s
  #     retries: 10

volumes:
  {app_name}_data:
    driver: local
  # postgres_data:

networks:
  link_network:
    driver: bridge
"#
    );
    fs::write(&compose_path, compose_content)?;
    generated.push(compose_path);

    // 3. .dockerignore
    let dockerignore_path = out_dir.join(".dockerignore");
    let dockerignore_content = r#"target/
.git/
.github/
node_modules/
dist/
*.db
*.db-journal
*.db-wal
*.log
tmp/
.env.local
"#;
    fs::write(&dockerignore_path, dockerignore_content)?;
    generated.push(dockerignore_path);

    Ok(generated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_docker_files_creates_all_expected_artifacts() {
        let temp_dir = std::env::temp_dir().join(format!("link_docker_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);

        let files = generate_docker_files("users_api.link", &temp_dir).unwrap();
        assert_eq!(files.len(), 3);

        let dockerfile = fs::read_to_string(temp_dir.join("Dockerfile")).unwrap();
        assert!(dockerfile.contains("users_api.link"));
        assert!(dockerfile.contains("EXPOSE 3000"));

        let compose = fs::read_to_string(temp_dir.join("docker-compose.yml")).unwrap();
        assert!(compose.contains("users_api"));
        assert!(compose.contains("3000:3000"));
        // El compose traía `DATABASE_URL` y `DATABASE_PATH`, que el binario no
        // lee: descomentar esa línea dejaba al servidor en SQLite mientras
        // quien la descomentó creía estar en PostgreSQL. La variable real es
        // LINK_DATABASE_URL (GRAMMAR.md §3.36).
        assert!(compose.contains("LINK_DATABASE_URL"), "el compose debe nombrar la variable real");
        assert!(!compose.contains("DATABASE_PATH"), "no debe ofrecer variables que nadie lee");

        let ignore = fs::read_to_string(temp_dir.join(".dockerignore")).unwrap();
        assert!(ignore.contains("node_modules/"));

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
