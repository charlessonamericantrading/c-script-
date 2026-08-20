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
const PROGRAM_TEMPLATE: &str = r#"
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

db { COLLECTION: Lead[], }

service Leads {
  rpc list() -> Lead[] { db.COLLECTION.all() }
  rpc get(id: Int) -> Lead? { db.COLLECTION.find(id) }
  rpc total() -> Int { db.COLLECTION.count() }
  rpc remove(id: Int) -> Bool { db.COLLECTION.delete(id) }
  rpc update(id: Int, patch: Patch<Lead>) -> Lead { db.COLLECTION.applyPatch(id, patch) }

  rpc create(email: String, score: Float) -> Lead {
    db.COLLECTION.insert(NewLead {
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
    db.COLLECTION.findWhere(|l: Lead| { !l.contacted })
  }
}
"#;

/// El programa con la colección (y por lo tanto la tabla) que pide el test.
fn program_for(collection: &str) -> String {
    PROGRAM_TEMPLATE.replace("COLLECTION", collection)
}

/// PostgreSQL no serializa bien el DDL concurrente: dos conexiones haciendo
/// `CREATE TABLE IF NOT EXISTS` a la vez pueden chocar en los catálogos del
/// sistema. Cada test usa su propia tabla, y además el arranque (drop + create)
/// se serializa acá -- `cargo test` corre los tests en paralelo por defecto y
/// eso no se cambia solo para esto.
static SETUP: std::sync::Mutex<()> = std::sync::Mutex::new(());

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

    fn program(&self, collection: &str) -> PathBuf {
        let path = self.0.join("app.link");
        std::fs::write(&path, program_for(collection)).unwrap();
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
fn reset_schema(url: &str, collection: &str) {
    let mut client =
        postgres::Client::connect(url, postgres::NoTls).expect("conectar a la base de PostgreSQL de test");
    client
        .batch_execute(&format!("DROP TABLE IF EXISTS \"{collection}\";"))
        .expect("limpiar el esquema");
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
        // La salida del servidor va a archivos, no a /dev/null: cuando esto
        // falla, el motivo lo tiene el proceso hijo, y tirarlo obliga a
        // adivinar. (Primera versión de este test: 3 fallos en CI que solo
        // decían "no respondió a tiempo".)
        let log_dir = src.parent().expect("el .link vive en algún directorio");
        let out_path = log_dir.join(format!("serve-{port}.out"));
        let err_path = log_dir.join(format!("serve-{port}.err"));
        let child = Command::new(env!("CARGO_BIN_EXE_linkc"))
            .arg("serve")
            .arg(src)
            .arg(port.to_string())
            .env("LINK_DATABASE_URL", url)
            .stdout(std::fs::File::create(&out_path).expect("crear el log de stdout"))
            .stderr(std::fs::File::create(&err_path).expect("crear el log de stderr"))
            .spawn()
            .expect("iniciar 'linkc serve'");

        // Un round-trip HTTP completo es la única señal confiable de que el
        // servidor ya está sirviendo (mismo criterio que tests/server_http.rs).
        let mut server = Serve { child, port };
        for _ in 0..300 {
            if ureq::get(&format!("http://127.0.0.1:{port}/health")).call().is_ok() {
                return server;
            }
            // Si el proceso ya murió, no tiene sentido seguir esperando.
            if let Ok(Some(status)) = server.child.try_wait() {
                panic!(
                    "'linkc serve' salió con {status} antes de escuchar:\nstdout: {}\nstderr: {}",
                    std::fs::read_to_string(&out_path).unwrap_or_default(),
                    std::fs::read_to_string(&err_path).unwrap_or_default()
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!(
            "'linkc serve' no respondió a tiempo en el puerto {port}\nstdout: {}\nstderr: {}",
            std::fs::read_to_string(&out_path).unwrap_or_default(),
            std::fs::read_to_string(&err_path).unwrap_or_default()
        );
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
    const COLLECTION: &str = "leads_crud";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);
    let temp = TempDir::new("crud");
    let src = temp.program(COLLECTION);
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
    const COLLECTION: &str = "leads_persist";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);
    let temp = TempDir::new("persist");
    let src = temp.program(COLLECTION);

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
        .query(&format!("SELECT \"email\", \"status\", \"score\", \"contacted\", \"meta\" FROM \"{COLLECTION}\" ORDER BY \"id\""), &[])
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
        .query(&format!("SELECT count(*) FROM \"{COLLECTION}\" WHERE \"meta\"->>'source' = $1"), &[&"test"])
        .expect("consultar por dentro del JSONB");
    assert_eq!(by_json[0].get::<_, i64>(0), 1, "el JSONB es consultable como tal");
}

#[test]
fn the_runtime_creates_the_same_schema_that_linkc_build_emits() {
    const COLLECTION: &str = "leads_schema";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);
    let temp = TempDir::new("schema");
    let src = temp.program(COLLECTION);

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
             WHERE table_name = $1 ORDER BY ordinal_position",
            &[&COLLECTION],
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
    const COLLECTION: &str = "leads_migrate";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);
    let temp = TempDir::new("migrate");

    // Versión 1 del programa, con datos adentro.
    let v1 = temp.0.join("app.link");
    let v1_src = program_for(COLLECTION);
    std::fs::write(&v1, &v1_src).unwrap();
    let id = {
        let server = Serve::start(&v1, &url);
        server.rpc("Leads/create", r#"{"email":"vieja@example.com","score":1.0}"#)["id"]
            .as_i64()
            .unwrap()
    };

    // Versión 2: el programa gana un campo. La tabla ya existe y tiene filas.
    let v2 = v1_src.replace(
        "  note?: String?,\n  createdAt: Timestamp,\n}\n\ntype NewLead",
        "  note?: String?,\n  owner?: String,\n  createdAt: Timestamp,\n}\n\ntype NewLead",
    );
    assert_ne!(v2, v1_src, "el reemplazo del campo nuevo tiene que haber aplicado");
    std::fs::write(&v1, &v2).unwrap();

    let server = Serve::start(&v1, &url);
    let row = server.rpc("Leads/get", &format!(r#"{{"id":{id}}}"#));
    assert_eq!(row["email"], "vieja@example.com", "la fila anterior sigue ahí después de migrar");

    let mut client = postgres::Client::connect(&url, postgres::NoTls).expect("conectar");
    let cols = client
        .query(
            "SELECT column_name, is_nullable FROM information_schema.columns WHERE table_name = $1 AND column_name = 'owner'",
            &[&COLLECTION],
        )
        .expect("buscar la columna nueva");
    assert_eq!(cols.len(), 1, "la columna nueva se agregó sola");
    // Se agrega SIEMPRE nullable, aunque el tipo sea requerido: no hay valor
    // que poner en las filas que ya existían. Es un límite documentado.
    assert_eq!(cols[0].get::<_, String>(1), "YES", "la columna migrada admite NULL");
}

#[test]
fn a_preexisting_table_with_a_non_integer_id_fails_at_connect_not_at_first_insert() {
    // Encontrado en producción real (migrando desde un backend que ya usaba
    // UUID como clave primaria): una tabla creada por OTRO sistema, con
    // `id UUID`, dejaba pasar `CREATE TABLE IF NOT EXISTS` sin ninguna queja
    // -- es un no-op sobre una tabla que ya existe, nunca mira sus columnas.
    // El primer `db.<col>.insert(...)` recién ahí explotaba: `RETURNING "id"`
    // leído como `i64` contra una columna `uuid` -- y como `handle_rpc` corre
    // sincrónico en el hilo principal del accept-loop (server.rs), eso no
    // tiraba abajo solo esa request, tiraba abajo el PROCESO ENTERO.
    //
    // Este test fija que ahora se rechaza ANTES de aceptar la primera
    // conexión, con un mensaje que dice qué pasó -- nunca con un panic.
    const COLLECTION: &str = "leads_uuid_id";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let mut client = postgres::Client::connect(&url, postgres::NoTls).expect("conectar");
    client
        .batch_execute(&format!(
            "CREATE TABLE \"{COLLECTION}\" (\"id\" UUID PRIMARY KEY DEFAULT gen_random_uuid(), \"email\" TEXT NOT NULL)"
        ))
        .expect("crear la tabla preexistente con id UUID (simula una migrada desde otro backend)");

    let temp = TempDir::new("uuid-id");
    let src = temp.program(COLLECTION);

    // Arranque directo -- a diferencia de Serve::start, que esperaría el
    // puerto en vano: esto tiene que fallar YA, antes de que el servidor
    // llegue a escuchar nada.
    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&src)
        .arg(free_port().to_string())
        .env("LINK_DATABASE_URL", &url)
        .output()
        .expect("ejecutar linkc serve");

    assert!(!out.status.success(), "una tabla con id no entero no puede terminar en éxito");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("uuid"), "el error tiene que nombrar el tipo real encontrado: {stderr}");
    assert!(stderr.contains(COLLECTION), "el error tiene que nombrar la tabla: {stderr}");
    assert!(
        !stderr.contains("panicked at"),
        "tiene que fallar LIMPIO al conectar, no crashear el proceso en el primer insert: {stderr}"
    );
    // Confirma que el rechazo pasa ANTES de aceptar una conexión, no que el
    // accept-loop murió después de arrancar bien.
    assert!(!stderr.contains("escuchando en"), "no debió llegar a anunciar que estaba sirviendo: {stderr}");
}

#[test]
fn a_bad_connection_url_fails_with_a_message_instead_of_a_panic() {
    // Este no necesita base: prueba justamente el camino en que no hay ninguna.
    let temp = TempDir::new("badurl");
    let src = temp.program("leads_badurl");

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
