// El backend PostgreSQL, contra un PostgreSQL DE VERDAD.
//
// Estos tests NO corren si no hay una base: piden la URL en `LINK_TEST_PG_URL`
// y se saltean si no está. Es deliberado -- que `cargo test` falle en la
// máquina de alguien que no tiene Postgres levantado sería ruido, y que pasen
// en verde sin haber tocado una base sería mentira. En CI la variable SÍ está
// (ver el job `postgres` en .github/workflows/ci.yml), así que ahí se ejecutan
// de verdad en cada push.
//
// Por qué existe este archivo: hasta esta ronda `runtime/postgres.rs` solo
// GENERABA texto SQL. El README hablaba de un "adaptador PostgreSQL
// enterprise" mientras `linkc serve` usaba SQLite siempre, sin excepción. Un
// test de generación de DDL no detecta esa diferencia: solo compara strings.
// Lo único que la detecta es escribir y leer filas reales contra el motor real,
// a través del servidor real.
//
// Para correrlos a mano:
//   docker run --rm -e POSTGRES_PASSWORD=link -p 5432:5432 postgres:16
//   LINK_TEST_PG_URL=postgres://postgres:link@localhost/postgres cargo test --test pg_integration

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

/// Un programa con un campo de cada familia: nativo requerido, nativo nullable,
/// opcional-por-clave, enum simple (columna TEXT), struct anidado (JSONB) y el
/// caso de tres estados `campo?: T?`.
const PROGRAM: &str = r#"
type Meta = { source: String, score: Int }

type Lead = {
  id: Int,
  email: String,
  name: String?,
  phone?: String,
  status: Status,
  score: Float,
  contacted: Bool,
  meta: Meta,
  note?: String?,
  createdAt: Timestamp,
}

type NewLead = {
  email: String,
  name: String?,
  status: Status,
  score: Float,
  contacted: Bool,
  meta: Meta,
  createdAt: Timestamp,
}

enum Status { New, Contacted, Won }

db { leads: Lead[], }

service Leads {
  rpc list() -> Lead[] { db.leads.all() }
  rpc get(id: Int) -> Lead? { db.leads.find(id) }
  rpc total() -> Int { db.leads.count() }
  rpc remove(id: Int) -> Bool { db.leads.delete(id) }
  rpc update(id: Int, patch: Patch<Lead>) -> Lead { db.leads.applyPatch(id, patch) }

  rpc create(email: String, score: Float) -> Lead {
    db.leads.insert(NewLead {
      email: email,
      name: null,
      status: Status.New {},
      score: score,
      contacted: false,
      meta: Meta { source: "test", score: 7 },
      createdAt: now(),
    })
  }

  rpc pending() -> Lead[] {
    db.leads.findWhere(|l: Lead| { !l.contacted })
  }
}
"#;

fn pg_url() -> Option<String> {
    let url = std::env::var("LINK_TEST_PG_URL").ok().filter(|v| !v.trim().is_empty());
    // Un test que se saltea en silencio es peor que uno que no existe: pasa en
    // verde sin haber probado nada. En CI, donde la base SIEMPRE tiene que
    // estar, `LINK_TEST_PG_REQUIRED` convierte el salteo en una falla.
    assert!(
        !(url.is_none() && std::env::var("LINK_TEST_PG_REQUIRED").is_ok()),
        "LINK_TEST_PG_REQUIRED está definida pero LINK_TEST_PG_URL no: el job de PostgreSQL \
         habría pasado en verde sin conectarse a ninguna base"
    );
    url
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "linkc-pg-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn program(&self) -> PathBuf {
        let path = self.0.join("leads.link");
        std::fs::write(&path, PROGRAM).unwrap();
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Cada test arranca con la tabla borrada: la base de CI es compartida entre
/// tests del mismo job, y un `count()` no significa nada si quedaron filas de
/// otro. Borrar la tabla ejercita además la creación desde cero en cada test.
fn reset_schema(url: &str) {
    let mut client =
        postgres::Client::connect(url, postgres::NoTls).expect("conectar a la base de PostgreSQL de test");
    client.batch_execute("DROP TABLE IF EXISTS \"leads\";").expect("limpiar el esquema");
}

fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0)).expect("puerto efímero").local_addr().unwrap().port()
}

struct Serve {
    child: Child,
    port: u16,
}

impl Serve {
    fn start(src: &PathBuf, url: &str) -> Self {
        let port = free_port();
        let child = Command::new(env!("CARGO_BIN_EXE_linkc"))
            .arg("serve")
            .arg(src)
            .arg(port.to_string())
            .env("LINK_DATABASE_URL", url)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("iniciar 'linkc serve'");

        // Un round-trip HTTP completo es la única señal confiable de que el
        // servidor ya está sirviendo (mismo criterio que tests/server_http.rs).
        for _ in 0..300 {
            if ureq::get(&format!("http://127.0.0.1:{port}/health")).call().is_ok() {
                return Serve { child, port };
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("'linkc serve' no respondió a tiempo en el puerto {port}");
    }

    fn rpc(&self, method: &str, body: &str) -> serde_json::Value {
        let text = ureq::post(&format!("http://127.0.0.1:{}/{method}", self.port))
            .set("Content-Type", "application/json")
            .send_string(body)
            .unwrap_or_else(|e| panic!("{method} falló: {e}"))
            .into_string()
            .expect("leer el body");
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{method} no devolvió JSON ({e}): {text}"))
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn the_crud_surface_works_against_a_real_postgres() {
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    reset_schema(&url);
    let temp = TempDir::new("crud");
    let src = temp.program();
    let server = Serve::start(&src, &url);

    let a = server.rpc("Leads/create", r#"{"email":"ada@example.com","score":9.5}"#);
    assert!(a["id"].as_i64().unwrap() > 0, "el insert asigna id: {a}");
    assert_eq!(a["email"], "ada@example.com", "el texto vuelve igual");
    assert_eq!(a["score"], 9.5, "el float vuelve igual");
    assert_eq!(a["contacted"], false, "el booleano vuelve igual");
    assert_eq!(a["status"], "New", "un enum simple cruza como su nombre de variante");
    assert_eq!(a["meta"]["source"], "test", "el struct anidado vuelve entero");
    assert_eq!(a["meta"]["score"], 7);
    assert_eq!(a["name"], serde_json::Value::Null, "String? nulo vuelve como null");
    assert!(a.get("phone").is_none(), "un opcional-por-clave ausente no aparece en el JSON");

    let b = server.rpc("Leads/create", r#"{"email":"grace@example.com","score":8.0}"#);
    assert!(b["id"].as_i64().unwrap() > a["id"].as_i64().unwrap(), "los ids avanzan");

    assert_eq!(server.rpc("Leads/total", "{}"), 2, "count cuenta lo que hay");
    assert_eq!(server.rpc("Leads/list", "{}").as_array().unwrap().len(), 2);
    assert_eq!(
        server.rpc("Leads/pending", "{}").as_array().unwrap().len(),
        2,
        "findWhere evalúa el predicado sobre filas reales"
    );

    // applyPatch: solo los campos presentes en el patch se tocan.
    let id_a = a["id"].as_i64().unwrap();
    let patched = server.rpc("Leads/update", &format!(r#"{{"id":{id_a},"patch":{{"contacted":true,"name":"Ada"}}}}"#));
    assert_eq!(patched["contacted"], true, "el patch aplicó");
    assert_eq!(patched["name"], "Ada", "y el nullable ahora tiene valor");
    assert_eq!(patched["email"], "ada@example.com", "lo que el patch no nombra no se toca");
    assert_eq!(patched["score"], 9.5);
    assert_eq!(server.rpc("Leads/pending", "{}").as_array().unwrap().len(), 1, "y se ve en el filtro");

    assert_eq!(server.rpc("Leads/remove", &format!(r#"{{"id":{id_a}}}"#)), true);
    assert_eq!(server.rpc("Leads/total", "{}"), 1, "delete se nota en el count");
    assert_eq!(server.rpc("Leads/get", &format!(r#"{{"id":{id_a}}}"#)), serde_json::Value::Null);
    assert_eq!(
        server.rpc("Leads/remove", &format!(r#"{{"id":{id_a}}}"#)),
        false,
        "borrar dos veces devuelve false"
    );
}

#[test]
fn rows_survive_a_restart_and_are_readable_from_plain_sql() {
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida");
        return;
    };
    reset_schema(&url);
    let temp = TempDir::new("persist");
    let src = temp.program();

    let id = {
        let server = Serve::start(&src, &url);
        let row = server.rpc("Leads/create", r#"{"email":"persistida@example.com","score":3.25}"#);
        row["id"].as_i64().unwrap()
    }; // el servidor muere acá

    // Otro proceso, misma base: la fila sigue estando. Con SQLite esto ya
    // funcionaba contra un archivo; el punto es que ahora funciona contra la
    // base que el equipo ya administra.
    let server = Serve::start(&src, &url);
    let again = server.rpc("Leads/get", &format!(r#"{{"id":{id}}}"#));
    assert_eq!(again["email"], "persistida@example.com", "la fila sobrevivió al reinicio");
    assert_eq!(again["score"], 3.25);

    // Y ahora desde SQL a secas: lo que quedó en la base tiene que ser legible
    // sin pasar por c-script. Si el esquema fuera un blob opaco, "usá tu
    // Postgres de siempre" no sería cierto.
    let mut client = postgres::Client::connect(&url, postgres::NoTls).expect("conectar");
    let rows = client
        .query("SELECT \"email\", \"status\", \"score\", \"contacted\", \"meta\" FROM \"leads\" ORDER BY \"id\"", &[])
        .expect("consultar la tabla desde SQL plano");
    assert_eq!(rows.len(), 1);

    let email: String = rows[0].get(0);
    let status: String = rows[0].get(1);
    let score: f64 = rows[0].get(2);
    let contacted: bool = rows[0].get(3);
    let meta: serde_json::Value = rows[0].get(4);

    assert_eq!(email, "persistida@example.com");
    // El enum simple se guarda como el nombre de la variante en texto plano,
    // legible a ojo desde psql -- no como un número ni envuelto en JSON.
    assert_eq!(status, "New");
    assert_eq!(score, 3.25);
    assert!(!contacted);
    assert_eq!(meta["source"], serde_json::json!("test"));

    // El struct anidado es JSONB de verdad, consultable con los operadores de
    // Postgres -- no un string con JSON adentro.
    let by_json = client
        .query("SELECT count(*) FROM \"leads\" WHERE \"meta\"->>'source' = $1", &[&"test"])
        .expect("consultar por dentro del JSONB");
    assert_eq!(by_json[0].get::<_, i64>(0), 1, "el JSONB es consultable como tal");
}

#[test]
fn the_runtime_creates_the_same_schema_that_linkc_build_emits() {
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida");
        return;
    };
    reset_schema(&url);
    let temp = TempDir::new("schema");
    let src = temp.program();

    // El runtime crea las tablas al conectarse.
    let _server = Serve::start(&src, &url);

    // Y `linkc build` emite el schema.postgres.sql que el proyecto documenta.
    // Los dos tienen que decir lo mismo: si divergen, alguien migra su base con
    // un DDL que no es el que su servidor espera -- la clase de discrepancia
    // entre capas que este repo ya encontró varias veces (GRAMMAR.md §3.9).
    let out_dir = temp.0.join("gen");
    let build = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("build")
        .arg(&src)
        .arg(&out_dir)
        .output()
        .expect("ejecutar linkc build");
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let emitted = std::fs::read_to_string(out_dir.join("schema.postgres.sql")).expect("schema.postgres.sql");

    let mut client = postgres::Client::connect(&url, postgres::NoTls).expect("conectar");
    let cols = client
        .query(
            "SELECT column_name, data_type, is_nullable FROM information_schema.columns \
             WHERE table_name = 'leads' ORDER BY ordinal_position",
            &[],
        )
        .expect("leer el esquema real");
    let real: Vec<(String, String, String)> = cols.iter().map(|r| (r.get(0), r.get(1), r.get(2))).collect();
    let find = |name: &str| -> (String, String) {
        let (_, ty, nullable) = real
            .iter()
            .find(|(c, _, _)| c == name)
            .unwrap_or_else(|| panic!("la tabla real no tiene la columna '{name}': {real:?}"));
        (ty.clone(), nullable.clone())
    };

    assert_eq!(find("id").0, "bigint", "id es BIGSERIAL -> bigint");
    assert_eq!(find("email"), ("text".to_string(), "NO".to_string()), "String requerido");
    // `name: String?` es una columna TEXT que admite NULL, no un JSONB: ese era
    // el bug del generador de DDL -- sin desenvolver el Optional, cualquier
    // campo nullable se declaraba JSONB.
    assert_eq!(find("name"), ("text".to_string(), "YES".to_string()), "String? sigue siendo texto");
    assert_eq!(find("score").0, "double precision");
    assert_eq!(find("contacted").0, "boolean");
    assert_eq!(find("status").0, "text", "un enum simple es texto legible");
    assert_eq!(find("meta").0, "jsonb", "un struct anidado es JSONB");
    // `note?: String?` necesita tres estados (ausente / null / valor), que una
    // columna nativa nullable no puede representar: va a JSONB a propósito.
    assert_eq!(find("note").0, "jsonb", "el caso de tres estados va a JSONB");

    for (name, ty) in [("email", "TEXT"), ("score", "DOUBLE PRECISION"), ("contacted", "BOOLEAN"), ("meta", "JSONB")] {
        assert!(
            emitted.contains(&format!("\"{name}\" {ty}")),
            "schema.postgres.sql debe declarar {name} como {ty}:\n{emitted}"
        );
    }
}

#[test]
fn a_new_field_is_added_to_an_existing_table_without_losing_rows() {
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida");
        return;
    };
    reset_schema(&url);
    let temp = TempDir::new("migrate");

    // Versión 1 del programa, con datos adentro.
    let v1 = temp.0.join("leads.link");
    std::fs::write(&v1, PROGRAM).unwrap();
    let id = {
        let server = Serve::start(&v1, &url);
        server.rpc("Leads/create", r#"{"email":"vieja@example.com","score":1.0}"#)["id"]
            .as_i64()
            .unwrap()
    };

    // Versión 2: el programa gana un campo. La tabla ya existe y tiene filas.
    let v2 = PROGRAM.replace(
        "  note?: String?,\n  createdAt: Timestamp,\n}\n\ntype NewLead",
        "  note?: String?,\n  owner?: String,\n  createdAt: Timestamp,\n}\n\ntype NewLead",
    );
    assert_ne!(v2, PROGRAM, "el reemplazo del campo nuevo tiene que haber aplicado");
    std::fs::write(&v1, &v2).unwrap();

    let server = Serve::start(&v1, &url);
    let row = server.rpc("Leads/get", &format!(r#"{{"id":{id}}}"#));
    assert_eq!(row["email"], "vieja@example.com", "la fila anterior sigue ahí después de migrar");

    let mut client = postgres::Client::connect(&url, postgres::NoTls).expect("conectar");
    let cols = client
        .query(
            "SELECT column_name, is_nullable FROM information_schema.columns WHERE table_name = 'leads' AND column_name = 'owner'",
            &[],
        )
        .expect("buscar la columna nueva");
    assert_eq!(cols.len(), 1, "la columna nueva se agregó sola");
    // Se agrega SIEMPRE nullable, aunque el tipo sea requerido: no hay valor
    // que poner en las filas que ya existían. Es un límite documentado.
    assert_eq!(cols[0].get::<_, String>(1), "YES", "la columna migrada admite NULL");
}

#[test]
fn a_bad_connection_url_fails_with_a_message_instead_of_a_panic() {
    // Este no necesita base: prueba justamente el camino en que no hay ninguna.
    let temp = TempDir::new("badurl");
    let src = temp.program();

    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&src)
        .arg(free_port().to_string())
        .arg("--db")
        .arg("postgres://nadie:nada@127.0.0.1:1/no_existe")
        .output()
        .expect("ejecutar linkc serve");

    assert!(!out.status.success(), "una URL inválida no puede terminar en éxito");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no se pudo conectar a PostgreSQL"),
        "el error tiene que explicar qué pasó: {stderr}"
    );
    assert!(
        !stderr.contains("panicked at"),
        "una base caída es una condición operativa normal, no un bug del compilador: {stderr}"
    );
}
