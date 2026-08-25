use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-docker-pg-test-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let path = std::env::temp_dir().join(unique);
        fs::create_dir_all(&path).expect("no se pudo crear tempdir para test");
        Self(path)
    }

    fn write(&self, relative: &str, content: &str) -> PathBuf {
        let full = self.0.join(relative);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&full, content).unwrap();
        full
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn linkc_docker_generates_valid_dockerfile_and_compose_yml() {
    let temp = TempDir::new("docker-test");
    let src = r#"
    type User = { id: Int, name: String }
    db { users: User[] }
    service UserService {
        rpc list() -> User[] { db.users.findMany() }
    }
    "#;
    let file = temp.write("main.link", src);
    let out_dir = temp.0.join("docker_output");

    let res = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("docker")
        .arg(&file)
        .arg(&out_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();

    assert!(res.status.success());

    let dockerfile = fs::read_to_string(out_dir.join("Dockerfile")).unwrap();
    assert!(dockerfile.contains("FROM alpine:3.19 AS runner"));
    assert!(dockerfile.contains("EXPOSE 3000"));
    assert!(dockerfile.contains("HEALTHCHECK"));

    let compose = fs::read_to_string(out_dir.join("docker-compose.yml")).unwrap();
    assert!(compose.contains("3000:3000"));
    assert!(compose.contains("postgres"));

    let ignore = fs::read_to_string(out_dir.join(".dockerignore")).unwrap();
    assert!(ignore.contains("node_modules/"));
}

/// `linkc systemd` (GRAMMAR.md §3.120, PLAN.md §9.7) -- mismo criterio de
/// test que `linkc docker` arriba, contra el binario real.
#[test]
fn linkc_systemd_generates_a_valid_service_file_with_the_real_port() {
    let temp = TempDir::new("systemd-test");
    let src = r#"
    type User = { id: Int, name: String }
    db { users: User[] }
    service UserService {
        rpc list() -> User[] { db.users.findMany() }
    }
    "#;
    let file = temp.write("main.link", src);
    let out_dir = temp.0.join("systemd_output");

    let res = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("systemd")
        .arg(&file)
        .arg("4200")
        .arg(&out_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();

    assert!(res.status.success(), "{}", String::from_utf8_lossy(&res.stderr));

    let unit = fs::read_to_string(out_dir.join("main.service")).unwrap();
    assert!(unit.contains("ExecStart=/usr/local/bin/linkc serve main.link 4200"));
    assert!(unit.contains("[Install]"));
    assert!(unit.contains("WantedBy=multi-user.target"));
}

#[test]
fn linkc_systemd_rejects_an_invalid_port() {
    let temp = TempDir::new("systemd-bad-port-test");
    let file = temp.write("main.link", "service S { rpc f() -> Int { 1 } }");

    let res = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("systemd")
        .arg(&file)
        .arg("no-es-un-puerto")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();

    assert!(!res.status.success());
    assert!(String::from_utf8_lossy(&res.stderr).contains("puerto inválido"));
}

/// `linkc pm2-config` (GRAMMAR.md §3.121, PLAN.md §9.7) -- mismo criterio
/// de test que `linkc docker`/`linkc systemd`, contra el binario real. Usa
/// `-o` explícito, la forma de uso que PLAN.md documenta.
#[test]
fn linkc_pm2_config_generates_a_valid_ecosystem_json_with_the_real_port() {
    let temp = TempDir::new("pm2-test");
    let file = temp.write("main.link", "service S { rpc f() -> Int { 1 } }");
    let out_path = temp.0.join("ecosystem.json");

    let res = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("pm2-config")
        .arg(&file)
        .arg("4200")
        .arg("-o")
        .arg(&out_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();

    assert!(res.status.success(), "{}", String::from_utf8_lossy(&res.stderr));

    let content = fs::read_to_string(&out_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&content).expect("debe ser JSON válido");
    assert_eq!(json["apps"][0]["name"], "main");
    assert_eq!(json["apps"][0]["args"][1], "main.link");
    assert_eq!(json["apps"][0]["args"][2], "4200");
}

/// Sin `-o`, el default es `./ecosystem.json` en el directorio actual --
/// se corre desde el propio tempdir para no ensuciar el repo real.
#[test]
fn linkc_pm2_config_defaults_the_output_path_to_ecosystem_json() {
    let temp = TempDir::new("pm2-default-test");
    let file = temp.write("main.link", "service S { rpc f() -> Int { 1 } }");

    let res = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("pm2-config")
        .arg(&file)
        .arg("4200")
        .current_dir(&temp.0)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();

    assert!(res.status.success(), "{}", String::from_utf8_lossy(&res.stderr));
    assert!(temp.0.join("ecosystem.json").exists());
}

#[test]
fn linkc_build_emits_postgres_ddl_when_db_is_declared() {
    let temp = TempDir::new("pg-build-test");
    let src = r#"
    type Post = { id: Int, title: String, published: Bool, author_id: Int, tags: String[] }
    db { posts: Post[] }
    service Blog {
        rpc get(id: Int) -> Post? { db.posts.find(id) }
    }
    "#;
    let file = temp.write("app.link", src);
    let out_dir = temp.0.join("generated");

    let res = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("build")
        .arg(&file)
        .arg(&out_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&res.stderr);
    let stdout = String::from_utf8_lossy(&res.stdout);
    assert!(res.status.success(), "stdout: {stdout}\nstderr: {stderr}");

    let pg_sql_file = out_dir.join("schema.postgres.sql");
    assert!(pg_sql_file.exists());
    let sql = fs::read_to_string(&pg_sql_file).unwrap();
    assert!(sql.contains("CREATE TABLE IF NOT EXISTS \"posts\""));
    assert!(sql.contains("\"id\" BIGSERIAL PRIMARY KEY"));
    assert!(sql.contains("\"title\" TEXT NOT NULL"));
    assert!(sql.contains("\"published\" BOOLEAN NOT NULL"));
    assert!(sql.contains("\"tags\" JSONB NOT NULL"));
}
