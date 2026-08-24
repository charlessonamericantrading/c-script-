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

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
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

  rpc page(limit: Int, offset: Int) -> Lead[] {
    db.COLLECTION.page(limit, offset)
  }

  rpc pageAfter(cursor: Int?, limit: Int) -> Lead[] {
    db.COLLECTION.pageAfter(cursor, limit)
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

    fn write(&self, name: &str, content: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, content).unwrap();
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
        Self::start_with_args(src, url, &[])
    }

    /// Igual que `start`, más flags extra de `linkc serve` (ej.
    /// `--adopt-existing`, GRAMMAR.md §3.67) -- parámetro nuevo, no un
    /// tercer método: mismo criterio que `Db::new_with_options` en
    /// `runtime/db.rs`.
    fn start_with_args(src: &PathBuf, url: &str, extra_args: &[&str]) -> Self {
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
            .args(extra_args)
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

    /// Como `rpc`, pero sin panickear -- para el test de reconexión
    /// (`a_dropped_connection_self_heals_without_a_process_restart`), que
    /// necesita poder ver una request FALLAR (mientras la conexión a
    /// Postgres sigue cortada) sin abortar el test. `linkc serve` sigue
    /// respondiendo HTTP normalmente incluso cuando la query interna a
    /// Postgres falla -- un 5xx con `{"error": "..."}"`, no un timeout ni
    /// una conexión rechazada -- así que `Err` acá es "el status no fue
    /// 2xx", con el mensaje de error del body si lo trae.
    /// `GET /health` (GRAMMAR.md §3.87) -- devuelve (status, body JSON) sin
    /// panickear sobre un 503, a diferencia de `rpc`: el test de health
    /// check necesita ver ESE status para probar el punto entero de la
    /// feature.
    fn health(&self) -> (u16, serde_json::Value) {
        let (status, text) = match ureq::get(&format!("http://127.0.0.1:{}/health", self.port)).call() {
            Ok(r) => (r.status(), r.into_string().expect("leer el body")),
            Err(ureq::Error::Status(status, r)) => (status, r.into_string().unwrap_or_default()),
            Err(e) => panic!("/health falló de red: {e}"),
        };
        (status, serde_json::from_str(&text).unwrap_or_else(|e| panic!("/health no devolvió JSON ({e}): {text}")))
    }

    fn try_rpc(&self, method: &str, body: &str) -> Result<serde_json::Value, String> {
        let request = ureq::post(&format!("http://127.0.0.1:{}/{method}", self.port))
            .set("Content-Type", "application/json")
            .send_string(body);
        match request {
            Ok(r) => {
                let text = r.into_string().map_err(|e| e.to_string())?;
                serde_json::from_str(&text).map_err(|e| format!("{method} no devolvió JSON ({e}): {text}"))
            }
            Err(ureq::Error::Status(status, r)) => {
                let text = r.into_string().unwrap_or_default();
                Err(format!("{method} devolvió {status}: {text}"))
            }
            Err(e) => Err(e.to_string()),
        }
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
fn page_pushes_limit_offset_to_real_sql_against_postgres() {
    // GRAMMAR.md §3.48: la razón de ser de `page` es que `LIMIT`/`OFFSET`
    // viajen DENTRO del SQL -- para una tabla grande, no cuesta O(tabla
    // entera) como `all().take(n)` en el lenguaje. `all()` ya prueba el
    // camino sin paginar contra SQLite/Postgres; este test prueba el camino
    // CON LIMIT/OFFSET, específicamente contra Postgres (mismo criterio de
    // "los dos backends por separado" que ya usa el resto de este archivo).
    const COLLECTION: &str = "leads_page";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);
    let temp = TempDir::new("page");
    let src = temp.program(COLLECTION);
    let server = Serve::start(&src, &url);

    let mut ids = Vec::new();
    for i in 0..5 {
        let row = server.rpc("Leads/create", &format!(r#"{{"email":"lead{i}@example.com","score":1.0}}"#));
        ids.push(row["id"].as_i64().unwrap());
    }

    let page1 = server.rpc("Leads/page", r#"{"limit":2,"offset":0}"#);
    let page1_ids: Vec<i64> = page1.as_array().unwrap().iter().map(|r| r["id"].as_i64().unwrap()).collect();
    assert_eq!(page1_ids, ids[0..2], "primera página: los primeros 2 por id");

    let page2 = server.rpc("Leads/page", r#"{"limit":2,"offset":2}"#);
    let page2_ids: Vec<i64> = page2.as_array().unwrap().iter().map(|r| r["id"].as_i64().unwrap()).collect();
    assert_eq!(page2_ids, ids[2..4], "segunda página: sigue donde terminó la primera, sin solaparse");

    let last_page = server.rpc("Leads/page", r#"{"limit":2,"offset":4}"#);
    let last_page_ids: Vec<i64> = last_page.as_array().unwrap().iter().map(|r| r["id"].as_i64().unwrap()).collect();
    assert_eq!(last_page_ids, ids[4..5], "última página parcial: solo lo que queda, no un error");

    let past_the_end = server.rpc("Leads/page", r#"{"limit":2,"offset":100}"#);
    assert_eq!(past_the_end.as_array().unwrap().len(), 0, "offset más allá del final: lista vacía, no error");

    let err = server.try_rpc("Leads/page", r#"{"limit":2,"offset":-1}"#);
    assert!(err.is_err(), "offset negativo tiene que fallar, no mandarse tal cual al SQL de Postgres");
}

#[test]
fn page_after_pushes_a_cursor_predicate_to_real_sql_against_postgres() {
    // GRAMMAR.md §3.61: mismo espíritu que el test de arriba, pero para el
    // cursor -- confirma que `WHERE "id" > cursor` corre de verdad contra
    // Postgres, no solo contra SQLite (`db.rs`, unit test).
    const COLLECTION: &str = "leads_page_after";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);
    let temp = TempDir::new("page-after");
    let src = temp.program(COLLECTION);
    let server = Serve::start(&src, &url);

    let mut ids = Vec::new();
    for i in 0..5 {
        let row = server.rpc("Leads/create", &format!(r#"{{"email":"lead{i}@example.com","score":1.0}}"#));
        ids.push(row["id"].as_i64().unwrap());
    }

    let page1 = server.rpc("Leads/pageAfter", r#"{"cursor":null,"limit":2}"#);
    let page1_ids: Vec<i64> = page1.as_array().unwrap().iter().map(|r| r["id"].as_i64().unwrap()).collect();
    assert_eq!(page1_ids, ids[0..2], "primera página: cursor null trae desde el principio");

    let page2 = server.rpc("Leads/pageAfter", &format!(r#"{{"cursor":{},"limit":2}}"#, ids[1]));
    let page2_ids: Vec<i64> = page2.as_array().unwrap().iter().map(|r| r["id"].as_i64().unwrap()).collect();
    assert_eq!(page2_ids, ids[2..4], "segunda página: sigue justo después del cursor");

    let past_the_end = server.rpc("Leads/pageAfter", &format!(r#"{{"cursor":{},"limit":2}}"#, ids[4]));
    assert_eq!(past_the_end.as_array().unwrap().len(), 0, "cursor en el último id: lista vacía, no error");

    let err = server.try_rpc("Leads/pageAfter", r#"{"cursor":null,"limit":-1}"#);
    assert!(err.is_err(), "limit negativo tiene que fallar, no mandarse tal cual al SQL de Postgres");
}

const AGGREGATE_PROGRAM: &str = r#"
enum Plan { Free, Pro, Enterprise }
type Order = { id: Int, planId: String, plan: Plan, amountCents: Int }
type StringInt = { key: String, value: Int }
type PlanInt = { key: Plan, value: Int }

db { COLLECTION: Order[], }

service Orders {
  rpc create(planId: String, plan: Plan, amountCents: Int) -> Order {
    db.COLLECTION.insert(Order { id: 0, planId: planId, plan: plan, amountCents: amountCents })
  }

  rpc revenueByPlanId() -> StringInt[] {
    db.COLLECTION.sumBy(|o: Order| { o.planId }, |o: Order| { o.amountCents })
  }

  rpc countByPlan() -> PlanInt[] {
    db.COLLECTION.countBy(|o: Order| { o.plan })
  }
}
"#;

#[test]
fn aggregate_by_pushes_group_by_to_real_sql_against_postgres() {
    // GRAMMAR.md §3.52: mismo criterio que `page` (arriba) -- `GROUP BY`
    // real tiene que dar el mismo resultado en los dos backends, SQLite
    // (ya probado en runtime/mod.rs) y Postgres acá. Incluye agrupar por
    // un campo ENUM (`countByPlan`), que tiene que devolver el enum real
    // como key en los dos backends por igual.
    const COLLECTION: &str = "orders_aggregate";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);
    let temp = TempDir::new("aggregate");
    let src = temp.write("app.link", &AGGREGATE_PROGRAM.replace("COLLECTION", COLLECTION));
    let server = Serve::start(&src, &url);

    server.rpc("Orders/create", r#"{"planId":"pro","plan":"Pro","amountCents":2000}"#);
    server.rpc("Orders/create", r#"{"planId":"pro","plan":"Pro","amountCents":2000}"#);
    server.rpc("Orders/create", r#"{"planId":"free","plan":"Free","amountCents":0}"#);
    server.rpc("Orders/create", r#"{"planId":"ent","plan":"Enterprise","amountCents":10000}"#);
    server.rpc("Orders/create", r#"{"planId":"ent","plan":"Enterprise","amountCents":15000}"#);

    let revenue = server.rpc("Orders/revenueByPlanId", "{}");
    let mut by_key: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for row in revenue.as_array().unwrap() {
        by_key.insert(row["key"].as_str().unwrap().to_string(), row["value"].as_i64().unwrap());
    }
    assert_eq!(by_key.get("pro"), Some(&4000), "{by_key:?}");
    assert_eq!(by_key.get("free"), Some(&0), "{by_key:?}");
    assert_eq!(by_key.get("ent"), Some(&25000), "{by_key:?}");

    let counts = server.rpc("Orders/countByPlan", "{}");
    let mut count_by_key: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for row in counts.as_array().unwrap() {
        count_by_key.insert(row["key"].as_str().unwrap().to_string(), row["value"].as_i64().unwrap());
    }
    assert_eq!(count_by_key.get("Pro"), Some(&2), "la key es el nombre de variante real: {count_by_key:?}");
    assert_eq!(count_by_key.get("Free"), Some(&1), "{count_by_key:?}");
    assert_eq!(count_by_key.get("Enterprise"), Some(&2), "{count_by_key:?}");
}

const INT64_AGGREGATE_PROGRAM: &str = r#"
type Sale = { id: Int, region: Int64, amount: Int64 }
type RegionTotal = { key: Int64, value: Int64 }

db { COLLECTION: Sale[], }

service Sales {
  rpc create(region: Int64, amount: Int64) -> Sale {
    db.COLLECTION.insert(Sale { id: 0, region: region, amount: amount })
  }

  rpc totalByRegion() -> RegionTotal[] {
    db.COLLECTION.sumBy(|s: Sale| { s.region }, |s: Sale| { s.amount })
  }
}
"#;

#[test]
fn aggregation_by_int64_key_and_value_pushes_group_by_to_real_sql_against_postgres() {
    // GRAMMAR.md §3.65: antes de esta ronda, Int64 estaba rechazado como key
    // Y como value en sumBy/etc. -- este test es el lado Postgres del que ya
    // existe contra SQLite (runtime/mod.rs). Importa especialmente contra
    // Postgres porque Int64 viaja como STRING en el JSON (§3.30, para no
    // perder precisión arriba de 2^53) -- si `scalar_cell_to_value` hubiera
    // seguido etiquetando el resultado como `Value::Int`, esto habría
    // serializado como número, no como string, y roto cualquier cliente que
    // esperara la forma documentada.
    const COLLECTION: &str = "sales_int64_aggregate";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);
    let temp = TempDir::new("int64-aggregate");
    let src = temp.write("app.link", &INT64_AGGREGATE_PROGRAM.replace("COLLECTION", COLLECTION));
    let server = Serve::start(&src, &url);

    server.rpc("Sales/create", r#"{"region":"1","amount":"500"}"#);
    server.rpc("Sales/create", r#"{"region":"1","amount":"700"}"#);
    server.rpc("Sales/create", r#"{"region":"2","amount":"300"}"#);

    let totals = server.rpc("Sales/totalByRegion", "{}");
    let rows = totals.as_array().unwrap();
    assert_eq!(rows.len(), 2, "una fila por region distinta: {rows:?}");
    for row in rows {
        assert!(row["key"].is_string(), "Int64 viaja como string en el JSON: {row:?}");
        assert!(row["value"].is_string(), "el VALUE agregado también, si de verdad es Int64: {row:?}");
    }
    let by_key: std::collections::HashMap<String, i64> =
        rows.iter().map(|r| (r["key"].as_str().unwrap().to_string(), r["value"].as_str().unwrap().parse().unwrap())).collect();
    assert_eq!(by_key.get("1"), Some(&1200), "{by_key:?}");
    assert_eq!(by_key.get("2"), Some(&300), "{by_key:?}");
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
fn a_preexisting_table_with_a_32_bit_serial_id_accepts_inserts_and_reads() {
    // GRAMMAR.md §3.58: `validate_existing_id_column` (arriba) ya aceptaba
    // "integer"/"smallint" además de "bigint" al CONECTAR -- pero
    // `insert_returning_id`/`postgres_cell` (runtime/store.rs) exigían el OID
    // EXACTO int8. Una tabla real con "id" `SERIAL` (int4, típico al migrar
    // desde otro backend que no usaba BIGSERIAL) pasaba la conexión sin
    // queja y fallaba en el primer insert -- el mismo desacuerdo entre capas
    // que §3.9 viene documentando desde v1.0, solo que acá ninguna de las
    // dos capas panickeaba, así que quedó sin test hasta esta ronda. Crea la
    // tabla A MANO con "id" SERIAL (no BIGSERIAL) y confirma insert + get +
    // list de punta a punta contra Postgres real.
    const COLLECTION: &str = "items_serial_id";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let mut client = postgres::Client::connect(&url, postgres::NoTls).expect("conectar");
    client
        .batch_execute(&format!("CREATE TABLE \"{COLLECTION}\" (\"id\" SERIAL PRIMARY KEY, \"name\" TEXT NOT NULL)"))
        .expect("crear la tabla preexistente con id SERIAL (int4, no BIGSERIAL)");

    let temp = TempDir::new("serial-id");
    let src = temp.write(
        "app.link",
        &format!(
            r#"
type Item = {{ id: Int, name: String }}
type NewItem = {{ name: String }}
db {{ {COLLECTION}: Item[] }}
service Items {{
  rpc create(name: String) -> Item {{ db.{COLLECTION}.insert(NewItem {{ name: name }}) }}
  rpc list() -> Item[] {{ db.{COLLECTION}.all() }}
  rpc get(id: Int) -> Item? {{ db.{COLLECTION}.find(id) }}
}}
"#
        ),
    );

    let server = Serve::start(&src, &url);

    let created = server.rpc("Items/create", r#"{"name":"primero"}"#);
    let id = created["id"].as_i64().expect("insert devuelve un id numérico, no un error de decodificación");
    assert_eq!(created["name"], "primero");

    let fetched = server.rpc("Items/get", &format!(r#"{{"id":{id}}}"#));
    assert_eq!(fetched["name"], "primero", "leer de vuelta la fila insertada por id");

    let listed = server.rpc("Items/list", "{}");
    assert_eq!(listed.as_array().map(|a| a.len()), Some(1), "list() también decodifica el id de 32 bits");
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

/// Agrega un query param a una URL de conexión que hoy no trae ninguno
/// (`LINK_TEST_PG_URL` en CI es justo así) -- alcanza para lo que necesitan
/// los tests de acá abajo, no es un parser de URL genérico.
fn with_query_param(url: &str, param: &str) -> String {
    if url.contains('?') {
        format!("{url}&{param}")
    } else {
        format!("{url}?{param}")
    }
}

#[test]
fn sslmode_disable_still_connects_in_plaintext() {
    // Regresión explícita del comportamiento de ANTES de GRAMMAR.md §3.40:
    // quien pide texto plano a propósito (`sslmode=disable`) lo sigue
    // teniendo, sin que el intento de TLS oportunista de la nueva versión
    // se interponga.
    const COLLECTION: &str = "leads_sslmode_disable";
    let Some(base_url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&base_url, COLLECTION);

    let url = with_query_param(&base_url, "sslmode=disable");
    let temp = TempDir::new("sslmode-disable");
    let src = temp.program(COLLECTION);
    let server = Serve::start(&src, &url);

    let created = server.rpc("Leads/create", r#"{"email":"plain@example.com","score":1.0}"#);
    assert_eq!(created["email"], "plain@example.com", "body: {created:?}");
}

#[test]
fn default_sslmode_still_connects_against_a_server_without_tls_configured() {
    // El otro lado de la misma regresión: SIN pedir `sslmode` explícito, el
    // default pasó de "nunca cifrar" a "intentar TLS, seguir en texto plano
    // si el servidor no lo ofrece" (`SslMode::Prefer`, GRAMMAR.md §3.40). El
    // Postgres de CI no tiene TLS configurado -- si el fallback a texto
    // plano no funcionara, ESTE test (y de hecho todos los demás de este
    // archivo, que usan la URL tal cual) fallarían al conectar.
    const COLLECTION: &str = "leads_sslmode_default";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let temp = TempDir::new("sslmode-default");
    let src = temp.program(COLLECTION);
    let server = Serve::start(&src, &url);

    let created = server.rpc("Leads/create", r#"{"email":"default@example.com","score":1.0}"#);
    assert_eq!(created["email"], "default@example.com", "body: {created:?}");
}

#[test]
fn a_dropped_connection_self_heals_without_a_process_restart() {
    // Antes de GRAMMAR.md §3.40: `Backend::Postgres` guardaba UN
    // `postgres::Client` para toda la vida del proceso, sin reemplazarlo
    // nunca -- una conexión cortada (red inestable, el propio Postgres
    // reiniciando, un firewall cerrando conexiones ociosas) dejaba CADA
    // request siguiente fallando hasta un reinicio manual de `linkc serve`.
    //
    // Este test corta la conexión de verdad (`pg_terminate_backend` desde
    // una conexión administrativa aparte, identificando el backend del
    // servidor por `application_name`) y prueba que, SIN reiniciar el
    // proceso, el servidor vuelve a servir solo.
    //
    // No se asume que la PRIMERA request después del corte falle: es una
    // carrera entre cuándo el cliente interno nota la conexión cerrada y
    // cuándo llega esa request, y afirmar sobre el resultado exacto de esa
    // carrera sería un test frágil por diseño. Lo que se prueba es la
    // propiedad que de verdad importa: que se recupera SOLO, en un plazo
    // razonable, sin ayuda humana.
    const COLLECTION: &str = "leads_reconnect";
    const APP_NAME: &str = "linkc_reconnect_test";
    let Some(base_url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&base_url, COLLECTION);

    let url = with_query_param(&base_url, &format!("application_name={APP_NAME}"));
    let temp = TempDir::new("reconnect");
    let src = temp.program(COLLECTION);
    let server = Serve::start(&src, &url);

    // La conexión funciona antes de tocar nada.
    let first = server.rpc("Leads/create", r#"{"email":"before@example.com","score":1.0}"#);
    assert_eq!(first["email"], "before@example.com", "body: {first:?}");

    // Cortar la conexión del SERVIDOR (identificada por application_name),
    // desde una conexión administrativa aparte -- nunca la del propio test
    // runner, que usa otra conexión distinta para esto.
    let mut admin = postgres::Client::connect(&base_url, postgres::NoTls).expect("conectar como admin");
    let terminated = admin
        .execute(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
             WHERE application_name = $1 AND pid <> pg_backend_pid()",
            &[&APP_NAME],
        )
        .expect("ejecutar pg_terminate_backend");
    assert!(terminated > 0, "no se encontró la conexión del servidor por application_name -- ¿cambió cómo se identifica?");

    // Reintentar hasta que vuelva a andar, sin reiniciar `server` (mismo
    // proceso, mismo PID todo el tiempo).
    let mut last_err = String::new();
    let mut recovered = false;
    for _ in 0..100 {
        match server.try_rpc("Leads/create", r#"{"email":"after@example.com","score":2.0}"#) {
            Ok(created) if created["email"] == "after@example.com" => {
                recovered = true;
                break;
            }
            Ok(other) => last_err = format!("respuesta inesperada: {other:?}"),
            Err(e) => last_err = e,
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(recovered, "el servidor nunca se recuperó de la conexión cortada (último error: {last_err})");

    // La conexión de verdad quedó sana, no solo esa única request: los
    // datos de ANTES del corte siguen ahí, y una lectura normal funciona.
    let all = server.rpc("Leads/list", "{}");
    let emails: Vec<&str> = all.as_array().expect("list debe devolver un array").iter().map(|l| l["email"].as_str().unwrap()).collect();
    assert!(emails.contains(&"before@example.com"), "la fila de antes del corte debe seguir ahí: {emails:?}");
    assert!(emails.contains(&"after@example.com"), "la fila de después de reconectar debe estar: {emails:?}");
}

// GRAMMAR.md §3.87: `/health` hace un `SELECT 1` real contra la base, en vez
// de un 200 fijo sin importar nada -- reusa exactamente la misma técnica que
// `a_dropped_connection_self_heals_without_a_process_restart` (arriba) para
// cortar la conexión de verdad con `pg_terminate_backend`.

#[test]
fn health_check_reports_503_while_postgres_is_down_and_recovers_on_its_own() {
    const COLLECTION: &str = "leads_health";
    const APP_NAME: &str = "linkc_health_test";
    let Some(base_url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&base_url, COLLECTION);

    let url = with_query_param(&base_url, &format!("application_name={APP_NAME}"));
    let temp = TempDir::new("health");
    let src = temp.program(COLLECTION);
    let server = Serve::start(&src, &url);

    // Sana antes de tocar nada.
    let (status, body) = server.health();
    assert_eq!(status, 200, "body: {body:?}");
    assert_eq!(body["status"], "ok");
    assert_eq!(body["database"], "ok");

    // Cortar la conexión del SERVIDOR (identificada por application_name),
    // desde una conexión administrativa aparte -- mismo mecanismo exacto
    // que el test de reconexión.
    let mut admin = postgres::Client::connect(&base_url, postgres::NoTls).expect("conectar como admin");
    let terminated = admin
        .execute(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
             WHERE application_name = $1 AND pid <> pg_backend_pid()",
            &[&APP_NAME],
        )
        .expect("ejecutar pg_terminate_backend");
    assert!(terminated > 0, "no se encontró la conexión del servidor por application_name");

    // La PRIMERA request tras el corte todavía ve la conexión rota --
    // `with_reconnect` la reemplaza DESPUÉS de fallar, no antes -- así que
    // /health tiene que reportar 503 al menos una vez antes de recuperarse.
    // No se asume que sea EXACTAMENTE la primera (carrera real con cuándo
    // el cliente interno nota el corte), mismo criterio que el test de
    // reconexión: se prueba la propiedad, no el timing exacto.
    let mut saw_503 = false;
    let mut recovered = false;
    for _ in 0..100 {
        let (status, body) = server.health();
        if status == 503 {
            saw_503 = true;
            assert_eq!(body["status"], "error", "body: {body:?}");
            assert_ne!(body["database"], "ok", "body: {body:?}");
        } else if saw_503 && status == 200 {
            recovered = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(saw_503, "/health nunca reportó la caída -- ¿se está devolviendo 200 fijo de nuevo?");
    assert!(recovered, "/health debería recuperarse solo, sin reiniciar el proceso");

    let (status, body) = server.health();
    assert_eq!(status, 200, "body: {body:?}");
    assert_eq!(body["database"], "ok");
}

// GRAMMAR.md §3.44: LISTEN/NOTIFY entre DOS instancias de `linkc serve`
// contra la misma base -- un `stream` conectado a la instancia A tiene que
// ver una escritura que llegó por la instancia B, no solo las propias.

const PUSH_PROGRAM: &str = r#"
type Item = { id: Int, name: String }

db { COLLECTION: Item[], }

service Items {
  rpc create(name: String) -> Item {
    db.COLLECTION.insert(Item { id: 0, name: name })
  }

  stream watchAll() -> Item {
    while true {
      db.COLLECTION.subscribe()
    }
  }
}
"#;

/// Lee eventos SSE de un `stream` conectado a mano por un `TcpStream` --
/// `linkc serve` los manda con `Transfer-Encoding: chunked`, y CADA chunk es
/// EXACTAMENTE un evento (`write_chunk` en runtime/server.rs nunca parte un
/// evento en dos chunks ni junta dos eventos en uno), así que alcanza con
/// leer un chunk a la vez -- no hace falta un parser de SSE de verdad.
struct StreamClient {
    reader: BufReader<TcpStream>,
}

impl StreamClient {
    fn connect(port: u16, path: &str) -> Self {
        let stream = TcpStream::connect(("127.0.0.1", port)).expect("conectar al stream");
        // Sin esto, una lectura sin eventos pendientes bloquearía para
        // siempre -- `next_event` depende de que un timeout se resuelva
        // como "no llegó nada", no como un cuelgue del test.
        stream.set_read_timeout(Some(Duration::from_secs(10))).expect("fijar read timeout");
        let body = "{}";
        let request = format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let mut stream = stream;
        stream.write_all(request.as_bytes()).expect("escribir request");
        stream.flush().ok();

        let mut reader = BufReader::new(stream);
        let mut status_line = String::new();
        reader.read_line(&mut status_line).expect("línea de estado del stream");
        assert!(status_line.contains("200"), "el stream no arrancó bien: {status_line}");
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).expect("header del stream");
            if line.trim().is_empty() {
                break;
            }
        }
        StreamClient { reader }
    }

    /// Un evento (ya parseado como JSON), o `None` si no llegó ninguno
    /// dentro del `read_timeout` fijado en `connect`.
    fn next_event(&mut self) -> Option<serde_json::Value> {
        let mut size_line = String::new();
        self.reader.read_line(&mut size_line).ok()?;
        let size = usize::from_str_radix(size_line.trim(), 16).ok()?;
        if size == 0 {
            return None;
        }
        let mut buf = vec![0u8; size];
        self.reader.read_exact(&mut buf).ok()?;
        let mut crlf = [0u8; 2];
        self.reader.read_exact(&mut crlf).ok()?;
        let chunk = String::from_utf8_lossy(&buf);
        let data = chunk.strip_prefix("data: ")?.trim_end_matches(['\n', '\r']);
        serde_json::from_str(data).ok()
    }
}

#[test]
fn a_write_on_one_instance_pushes_to_a_stream_connected_to_another() {
    const COLLECTION: &str = "items_cross_instance";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let temp = TempDir::new("cross-instance");
    let src = temp.write("app.link", &PUSH_PROGRAM.replace("COLLECTION", COLLECTION));

    // Dos procesos `linkc serve` DISTINTOS, mismo programa, misma base.
    let instance_a = Serve::start(&src, &url);
    let instance_b = Serve::start(&src, &url);

    // Conectado a A -- tabla recién limpiada, así que la foto inicial (el
    // primer evento que un `stream` manda siempre, GRAMMAR.md §3.16) está
    // vacía y el próximo evento real que llegue es la escritura de abajo.
    let mut watcher = StreamClient::connect(instance_a.port, "/Items/watchAll");

    // La escritura entra por B.
    let created = instance_b.rpc("Items/create", r#"{"name":"desde-B"}"#);
    assert_eq!(created["name"], "desde-B", "body: {created:?}");

    // A tiene que verla igual -- vía LISTEN/NOTIFY, no porque comparta
    // memoria con B (son procesos separados).
    let event = watcher.next_event().expect("la instancia A debió recibir el push de la instancia B");
    assert_eq!(event["name"], "desde-B", "evento recibido: {event:?}");
    assert_eq!(event["id"], created["id"], "evento recibido: {event:?}");
}

// GRAMMAR.md §3.66: `linkc introspect <db-url>` genera un .link de partida
// desde una base PostgreSQL YA EXISTENTE -- estos tests crean una tabla A
// MANO (simulando un sistema adoptado, no generado por linkc) y confirman
// que lo que sale de introspect compila Y conecta de verdad contra esa
// MISMA tabla.

#[test]
fn introspect_generates_a_link_file_that_actually_works_against_the_real_table() {
    const COLLECTION: &str = "legacy_customers";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let mut client = postgres::Client::connect(&url, postgres::NoTls).expect("conectar");
    client
        .batch_execute(&format!(
            "CREATE TABLE \"{COLLECTION}\" (\
                \"id\" BIGSERIAL PRIMARY KEY, \
                \"name\" TEXT NOT NULL, \
                \"nickname\" TEXT, \
                \"active\" BOOLEAN NOT NULL, \
                \"balance\" DOUBLE PRECISION NOT NULL, \
                \"signup_date\" DATE NOT NULL\
            )"
        ))
        .expect("crear la tabla 'legacy' a mano, como si ya existiera de otro sistema");
    client
        .execute(
            &format!(
                "INSERT INTO \"{COLLECTION}\" (name, nickname, active, balance, signup_date) \
                 VALUES ('Ada', NULL, true, 42.5, '2026-08-24'::date)"
            ),
            &[],
        )
        .expect("sembrar una fila real");

    let output =
        Command::new(env!("CARGO_BIN_EXE_linkc")).arg("introspect").arg(&url).output().expect("ejecutar linkc introspect");
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    let generated = String::from_utf8_lossy(&output.stdout).to_string();

    assert!(generated.contains("type LegacyCustomers"), "{generated}");
    assert!(generated.contains("id: Int,"), "{generated}");
    assert!(generated.contains("name: String,"), "{generated}");
    assert!(generated.contains("nickname: String?,"), "nullable tiene que salir como String?: {generated}");
    assert!(generated.contains("active: Bool,"), "{generated}");
    assert!(generated.contains("balance: Float,"), "{generated}");
    // GRAMMAR.md §3.91: un `date` nativo mapea a `Timestamp`, sin
    // advertencia -- y decodifica de verdad más abajo, no solo "parece"
    // el tipo correcto en el .link generado.
    assert!(generated.contains("signup_date: Timestamp,"), "{generated}");
    assert!(generated.contains(&format!("{COLLECTION}: LegacyCustomers[]")), "{generated}");

    // Verificación fuerte: el .link generado no solo "parece" correcto --
    // compila Y conecta de verdad contra la MISMA tabla, leyendo el dato ya
    // sembrado antes de que `linkc` supiera que esta tabla existía.
    let mut full_program = generated;
    full_program
        .push_str(&format!("\nservice Check {{\n  rpc list() -> LegacyCustomers[] {{ db.{COLLECTION}.all() }}\n}}\n"));
    let temp = TempDir::new("introspect-roundtrip");
    let src = temp.write("app.link", &full_program);
    let server = Serve::start(&src, &url);

    let rows = server.rpc("Check/list", "{}");
    let arr = rows.as_array().unwrap();
    assert_eq!(arr.len(), 1, "{arr:?}");
    assert_eq!(arr[0]["name"], "Ada");
    assert_eq!(arr[0]["nickname"], serde_json::Value::Null);
    assert_eq!(arr[0]["active"], true);
    assert_eq!(arr[0]["balance"], 42.5);
    assert_eq!(arr[0]["signup_date"], "2026-08-24T00:00:00.000Z", "{arr:?}");
}

#[test]
fn introspect_warns_about_columns_it_cannot_map_with_confidence() {
    const COLLECTION: &str = "legacy_events";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let mut client = postgres::Client::connect(&url, postgres::NoTls).expect("conectar");
    client
        .batch_execute(&format!(
            "CREATE TABLE \"{COLLECTION}\" (\
                \"id\" BIGSERIAL PRIMARY KEY, \
                \"payload\" JSONB NOT NULL, \
                \"opens_at\" TIME NOT NULL\
            )"
        ))
        .expect("crear la tabla con columnas 'raras' a mano");

    let output =
        Command::new(env!("CARGO_BIN_EXE_linkc")).arg("introspect").arg(&url).output().expect("ejecutar linkc introspect");
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    let generated = String::from_utf8_lossy(&output.stdout).to_string();
    let warnings = String::from_utf8_lossy(&output.stderr).to_string();

    // Sigue emitiendo un tipo VÁLIDO -- nunca omite la columna -- pero avisa.
    assert!(generated.contains("payload: String,"), "{generated}");
    // El nombre del campo es el nombre REAL de la columna SQL, snake_case
    // incluido -- c-script no tiene ningún mecanismo de alias campo->columna
    // (el nombre del campo ES el nombre de columna que usa insert/find/etc.,
    // runtime/db.rs), así que "prolijizarlo" a camelCase acá rompería la
    // conexión real con la tabla.
    // `TIME` (sin fecha) sigue sin mapeo exacto -- a diferencia de
    // `date`/`timestamp`/`timestamptz`, que desde GRAMMAR.md §3.91 mapean
    // a `Timestamp` sin advertencia (ver
    // `introspect_generates_a_link_file_that_actually_works_against_the_real_table`).
    assert!(generated.contains("opens_at: String,"), "{generated}");
    assert!(warnings.contains("payload") && warnings.to_lowercase().contains("jsonb"), "stderr: {warnings}");
    assert!(warnings.contains("opens_at") && warnings.to_lowercase().contains("time"), "stderr: {warnings}");
}

// ---- modo adopción (`--adopt-existing`/`LINK_ADOPT_EXISTING`, GRAMMAR.md §3.67) ----

#[test]
fn adopt_existing_never_runs_ddl_and_ignores_an_unmodeled_column_against_real_postgres() {
    // Simula el caso real que motiva esto: una tabla de producción con una
    // columna que este programa nunca va a modelar, y (a propósito) SIN
    // otorgarle a este test ningún permiso de CREATE/ALTER que --adopt-existing
    // no debería necesitar -- si el connect ejecutara CUALQUIER DDL contra
    // esta tabla, este test lo notaría por construcción: la tabla ya existe
    // con exactamente las columnas de abajo, `linkc serve` solo puede
    // arrancar en verde si de verdad no toca nada.
    const COLLECTION: &str = "legacy_customers_adopt";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let mut client = postgres::Client::connect(&url, postgres::NoTls).expect("conectar");
    client
        .batch_execute(&format!(
            "CREATE TABLE \"{COLLECTION}\" (\
                \"id\" BIGSERIAL PRIMARY KEY, \
                \"name\" TEXT NOT NULL, \
                \"legacy_note\" TEXT\
            )"
        ))
        .expect("crear la tabla legacy a mano, con una columna que el .link de abajo no va a declarar");
    client
        .execute(&format!("INSERT INTO \"{COLLECTION}\" (name, legacy_note) VALUES ($1, $2)"), &[&"Ada", &"columna sin modelar"])
        .expect("sembrar una fila preexistente");

    let temp = TempDir::new("adopt-extra-column");
    let src = temp.write(
        "app.link",
        &format!(
            r#"
type Item = {{ id: Int, name: String }}
db {{ {COLLECTION}: Item[] }}
service Items {{ rpc list() -> Item[] {{ db.{COLLECTION}.all() }} }}
"#
        ),
    );

    let server = Serve::start_with_args(&src, &url, &["--adopt-existing"]);
    let listed = server.rpc("Items/list", "{}");
    let rows = listed.as_array().expect("se esperaba una lista");
    assert_eq!(rows.len(), 1, "la fila sembrada a mano tiene que seguir ahí -- adoptar no crea ni vacía la tabla");
    assert_eq!(rows[0]["name"], "Ada");
    assert!(rows[0].get("legacy_note").is_none(), "una columna no declarada no debe filtrarse a la respuesta");
}

// GRAMMAR.md §3.91: un campo `Timestamp` decodifica correctamente contra
// una columna `date`/`timestamp`/`timestamptz` NATIVA de Postgres, no solo
// contra el `BIGINT` propio de c-script -- encontrado auditando un reporte
// de adopción real (MyFinance): antes de esta ronda, una tabla YA
// EXISTENTE con columnas de fecha nativas (el caso normal al adoptar un
// sistema en producción) fallaba al leer la primera fila real, sin importar
// si el campo se declaraba `Timestamp` (ninguno de los anchos de entero de
// `postgres_int_cell` matchea el OID de un tipo temporal nativo) o `String`
// (el wire binario de un `timestamp` tampoco es texto UTF-8 válido).
#[test]
fn a_timestamp_field_decodes_a_native_postgres_date_and_timestamptz_column() {
    const COLLECTION: &str = "facturas_fecha_nativa";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let mut client = postgres::Client::connect(&url, postgres::NoTls).expect("conectar");
    client
        .batch_execute(&format!(
            "CREATE TABLE \"{COLLECTION}\" (\
                \"id\" BIGSERIAL PRIMARY KEY, \
                \"fecha_emision\" date NOT NULL, \
                \"created_at\" timestamptz NOT NULL, \
                \"updated_at\" timestamp NOT NULL\
            )"
        ))
        .expect("crear la tabla legacy a mano, con columnas de fecha NATIVAS de Postgres");
    // Sembrada con SQL crudo -- exactamente como llegan los datos reales de
    // un sistema que YA estaba en producción antes de adoptarlo, nunca
    // escritos por el propio c-script.
    client
        .execute(
            &format!(
                "INSERT INTO \"{COLLECTION}\" (fecha_emision, created_at, updated_at) VALUES \
                 ('2026-08-24'::date, '2026-08-24T14:30:00Z'::timestamptz, '2026-08-24T14:30:00'::timestamp)"
            ),
            &[],
        )
        .expect("sembrar una fila con SQL crudo, columnas de fecha nativas");

    let temp = TempDir::new("native-timestamp");
    let src = temp.write(
        "app.link",
        &format!(
            r#"
type Factura = {{ id: Int, fechaEmision: Timestamp, createdAt: Timestamp, updatedAt: Timestamp }}
db {{ {COLLECTION}: Factura[] }}
service Facturas {{ rpc list() -> Factura[] {{ db.{COLLECTION}.all() }} }}
"#
        ),
    );

    let server = Serve::start_with_args(&src, &url, &["--adopt-existing"]);
    let listed = server.rpc("Facturas/list", "{}");
    let rows = listed.as_array().expect("se esperaba una lista");
    assert_eq!(rows.len(), 1, "body: {listed:?}");
    assert_eq!(rows[0]["fechaEmision"], "2026-08-24T00:00:00.000Z", "date nativo: {rows:?}");
    assert_eq!(rows[0]["createdAt"], "2026-08-24T14:30:00.000Z", "timestamptz nativo: {rows:?}");
    assert_eq!(rows[0]["updatedAt"], "2026-08-24T14:30:00.000Z", "timestamp (sin tz) nativo: {rows:?}");
}

#[test]
fn adopt_existing_fails_fast_against_real_postgres_when_a_declared_column_is_missing() {
    const COLLECTION: &str = "legacy_items_missing_col";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let mut client = postgres::Client::connect(&url, postgres::NoTls).expect("conectar");
    client
        .batch_execute(&format!("CREATE TABLE \"{COLLECTION}\" (\"id\" BIGSERIAL PRIMARY KEY, \"name\" TEXT NOT NULL)"))
        .expect("crear la tabla SIN 'note'");

    let temp = TempDir::new("adopt-missing-column");
    // "note?" opcional: en modo normal, connect_postgres la agregaría con
    // `ADD COLUMN ... NULL` sin drama. En modo adopción tiene que fallar
    // igual -- el punto es no ejecutar NINGÚN DDL, ni siquiera uno no
    // destructivo.
    let src = temp.write(
        "app.link",
        &format!(
            r#"
type Item = {{ id: Int, name: String, note?: String }}
db {{ {COLLECTION}: Item[] }}
service Items {{ rpc list() -> Item[] {{ db.{COLLECTION}.all() }} }}
"#
        ),
    );

    let out = Command::new(env!("CARGO_BIN_EXE_linkc"))
        .arg("serve")
        .arg(&src)
        .arg(free_port().to_string())
        .arg("--adopt-existing")
        .env("LINK_DATABASE_URL", &url)
        .output()
        .expect("ejecutar linkc serve");

    assert!(!out.status.success(), "una columna declarada faltante tiene que fallar incluso si es opcional");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("note"), "el error tiene que nombrar la columna faltante: {stderr}");
    assert!(!stderr.contains("panicked at"), "tiene que fallar LIMPIO, no crashear el proceso: {stderr}");
}

// ---- matriz de comportamiento de auto-migrate: caso PostgreSQL real que
// SQLite no puede reproducir por construcción (PLAN.md §9.1.1) ----

#[test]
fn a_row_with_null_in_a_column_the_link_now_declares_required_fails_that_one_read_without_killing_the_server() {
    // `connect_postgres` SIEMPRE agrega una columna nueva NULLABLE (nunca
    // puede saber qué backfillear en filas viejas), sin importar si el
    // campo es requerido en el `.link` actual -- documentado como límite
    // real desde antes de esta ronda. Antes de esta ronda, una fila vieja
    // con NULL ahí decodificaba en SILENCIO a `Value::Null`: el cliente
    // tipado recibía `null` en un campo que el contrato TypeScript declara
    // `string` (no `string | null`), sin ningún error en ningún lado --
    // exactamente la clase de "los dos extremos no están de acuerdo" que
    // este proyecto viene evitando desde §3.9. Este test crea esa fila a
    // mano (simulando datos de antes de una migración real) y confirma que
    // ahora sale un error de runtime limpio -- nunca un panic que tumbe el
    // proceso entero -- Y que el servidor sigue respondiendo normalmente a
    // la request SIGUIENTE contra una fila sin ese problema.
    const COLLECTION: &str = "items_null_required";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let mut client = postgres::Client::connect(&url, postgres::NoTls).expect("conectar");
    client
        .batch_execute(&format!(
            "CREATE TABLE \"{COLLECTION}\" (\"id\" BIGSERIAL PRIMARY KEY, \"name\" TEXT NOT NULL, \"note\" TEXT)"
        ))
        .expect("crear la tabla con 'note' NULLABLE -- como quedaría tras un ADD COLUMN real");
    client
        .execute(&format!("INSERT INTO \"{COLLECTION}\" (name, note) VALUES ($1, NULL)"), &[&"fila-vieja-sin-note"])
        .expect("sembrar una fila con NULL en 'note', como una fila insertada antes de declarar el campo requerido");
    client
        .execute(&format!("INSERT INTO \"{COLLECTION}\" (name, note) VALUES ($1, $2)"), &[&"fila-nueva-con-note", &"tiene valor"])
        .expect("sembrar una segunda fila SIN el problema");

    let temp = TempDir::new("null-required");
    // "note" declarado REQUERIDO -- la columna física sigue nullable.
    let src = temp.write(
        "app.link",
        &format!(
            r#"
type Item = {{ id: Int, name: String, note: String }}
db {{ {COLLECTION}: Item[] }}
service Items {{
  rpc list() -> Item[] {{ db.{COLLECTION}.all() }}
  rpc get(id: Int) -> Item? {{ db.{COLLECTION}.find(id) }}
}}
"#
        ),
    );
    let server = Serve::start(&src, &url);

    // `list()` (que trae las dos filas) tiene que fallar limpio -- ninguna
    // fila con un problema puede filtrarse a un `Item[]` parcialmente
    // decodificado.
    let list_err = server.try_rpc("Items/list", "{}").expect_err("una fila con NULL en un campo requerido no puede listarse en éxito");
    assert!(list_err.contains("note"), "el error tiene que nombrar el campo: {list_err}");
    assert!(!list_err.contains("panicked at"), "tiene que ser un error de runtime limpio, no un panic: {list_err}");

    // El servidor sigue vivo: una request contra la fila SIN el problema
    // responde 200 normal -- confirma que el error de arriba fue un 5xx de
    // ESA request, no un panic que tiró abajo el proceso entero.
    let good = server.rpc("Items/get", r#"{"id":2}"#);
    assert_eq!(good["name"], "fila-nueva-con-note");
    assert_eq!(good["note"], "tiene valor");
}

// ---- dos `.link` distintos declarando la misma colección contra la misma
// base (PLAN.md §9.1, ítem "colisión de colección") ----

#[test]
fn two_different_link_files_declaring_disjoint_columns_of_the_same_table_can_read_each_others_rows_but_not_always_write() {
    // A diferencia de SQLite (`check_schema_matches`, §3.17, exige
    // coincidencia EXACTA), PostgreSQL nunca compara el schema completo --
    // solo valida "id" y agrega SUS PROPIAS columnas declaradas. Dos `.link`
    // con columnas DISTINTAS (sin nombres en común) sobre la misma tabla
    // conviven para LECTURA sin ningún error -- pero un INSERT desde el
    // segundo `.link` puede fallar si el primero dejó una columna NOT NULL
    // que el segundo no conoce y por lo tanto nunca provee.
    const COLLECTION: &str = "items_two_links_disjoint";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let temp = TempDir::new("two-links-disjoint");
    let link_a = temp.write(
        "a.link",
        &format!(
            r#"
type Item = {{ id: Int, name: String }}
db {{ {COLLECTION}: Item[] }}
service Items {{ rpc create(name: String) -> Item {{ db.{COLLECTION}.insert(Item {{ id: 0, name: name }}) }} rpc get(id: Int) -> Item? {{ db.{COLLECTION}.find(id) }} }}
"#
        ),
    );
    let server_a = Serve::start(&link_a, &url);
    let created = server_a.rpc("Items/create", r#"{"name":"desde A"}"#);
    let id = created["id"].as_i64().unwrap();
    drop(server_a);

    // "b.link" declara la MISMA colección con una columna distinta ("price",
    // que "a.link" nunca mencionó) -- se espera que conecte sin queja y le
    // agregue "price" (nullable) a la tabla que "a.link" ya creó.
    let link_b = temp.write(
        "b.link",
        &format!(
            r#"
type Item = {{ id: Int, price: Float? }}
db {{ {COLLECTION}: Item[] }}
service Items {{ rpc get(id: Int) -> Item? {{ db.{COLLECTION}.find(id) }} rpc create(price: Float?) -> Item {{ db.{COLLECTION}.insert(Item {{ id: 0, price: price }}) }} }}
"#
        ),
    );
    let server_b = Serve::start(&link_b, &url);
    // "b.link" ve la fila que "a.link" insertó (mismo "id"), con "price" en null.
    let seen_by_b = server_b.rpc("Items/get", &format!(r#"{{"id":{id}}}"#));
    assert_eq!(seen_by_b["price"], serde_json::Value::Null, "'price' nunca se escribió -- tiene que verse null, no faltar la fila");

    // Pero "b.link" NO puede insertar una fila propia: la tabla física tiene
    // "name" NOT NULL (de "a.link"), y el INSERT de "b.link" nunca la
    // menciona -- Postgres rechaza el INSERT, con un error limpio, nunca un
    // panic que tumbe el proceso.
    let insert_err =
        server_b.try_rpc("Items/create", r#"{"price":9.99}"#).expect_err("insertar sin 'name' tiene que violar el NOT NULL físico");
    assert!(!insert_err.contains("panicked at"), "tiene que ser un error de runtime limpio, no un panic: {insert_err}");
    drop(server_b);

    // "a.link" sigue viendo SU columna ("name") intacta -- nunca vio "price",
    // y el servidor sigue funcionando después del error de arriba.
    let server_a2 = Serve::start(&link_a, &url);
    let seen_by_a = server_a2.rpc("Items/get", &format!(r#"{{"id":{id}}}"#));
    assert_eq!(seen_by_a["name"], "desde A");
    assert!(seen_by_a.get("price").is_none(), "'a.link' no declara 'price' -- no debe aparecer en su respuesta");
}

#[test]
fn two_different_link_files_disagreeing_on_a_shared_columns_type_fails_cleanly_not_with_a_panic() {
    // El caso peligroso: los dos `.link` declaran un campo con el MISMO
    // nombre pero tipos distintos. `ADD COLUMN IF NOT EXISTS` es un no-op
    // sobre una columna que ya existe -- el segundo `.link` "cree" que la
    // columna es de su propio tipo, pero físicamente sigue siendo la del
    // primero. Este test confirma que el desacuerdo se descubre en el
    // primer INSERT/SELECT real (un error de tipo limpio del driver de
    // Postgres, propagado como RuntimeError), nunca en un panic que tumbe
    // el proceso.
    const COLLECTION: &str = "items_two_links_type_conflict";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let temp = TempDir::new("two-links-conflict");
    let link_a = temp.write(
        "a.link",
        &format!(
            r#"
type Item = {{ id: Int, count: Int }}
db {{ {COLLECTION}: Item[] }}
service Items {{ rpc create(count: Int) -> Item {{ db.{COLLECTION}.insert(Item {{ id: 0, count: count }}) }} }}
"#
        ),
    );
    let server_a = Serve::start(&link_a, &url);
    server_a.rpc("Items/create", r#"{"count":5}"#);
    drop(server_a);

    // "count" ya existe como INTEGER -- "b.link" lo declara String.
    let link_b = temp.write(
        "b.link",
        &format!(
            r#"
type Item = {{ id: Int, count: String }}
db {{ {COLLECTION}: Item[] }}
service Items {{ rpc list() -> Item[] {{ db.{COLLECTION}.all() }} }}
"#
        ),
    );
    let server_b = Serve::start(&link_b, &url);
    let err = server_b.try_rpc("Items/list", "{}").expect_err("leer una columna INTEGER real como String tiene que fallar limpio");
    assert!(!err.contains("panicked at"), "tiene que ser un error de runtime limpio, no un panic que tumbe el proceso: {err}");
}

/// Mismo host/base, credenciales distintas -- para probar el DDL generado
/// con un rol restringido, sin depender de que la URL de test tenga un
/// formato particular más allá de `postgres://user:pass@resto`.
fn with_credentials(url: &str, user: &str, password: &str) -> String {
    let after_scheme = url.strip_prefix("postgres://").or_else(|| url.strip_prefix("postgresql://")).expect("URL postgres:// esperada");
    let host_and_rest = after_scheme.split_once('@').map(|(_, rest)| rest).unwrap_or(after_scheme);
    format!("postgres://{user}:{password}@{host_and_rest}")
}

#[test]
fn generated_ddl_applies_cleanly_as_a_role_without_superuser_or_createrole() {
    // PLAN.md §9.1: el reporte de adopción preguntaba si `CREATE EXTENSION
    // "pgcrypto"` (que ya se sacó del DDL generado, ver postgres_emit.rs)
    // necesitaba superusuario en un proveedor gestionado (Neon/RDS/Supabase),
    // donde la app casi nunca corre como superusuario. Este test no asume la
    // respuesta -- crea un rol restringido de verdad (NOSUPERUSER,
    // NOCREATEROLE, NOCREATEDB, sin ningún privilegio más allá de crear
    // tablas en el schema `public`) y aplica el `schema.postgres.sql`
    // generado CONECTADO COMO ESE ROL, confirmando que aplica limpio.
    const COLLECTION: &str = "items_restricted_role";
    let Some(url) = pg_url() else {
        eprintln!("saltado: LINK_TEST_PG_URL no está definida (en CI sí lo está)");
        return;
    };
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    reset_schema(&url, COLLECTION);

    let mut admin = postgres::Client::connect(&url, postgres::NoTls).expect("conectar como el rol de test (admin en CI)");
    let role = "linkc_test_restricted_role";
    let password = "linkc-test-only";
    admin.batch_execute(&format!("DROP ROLE IF EXISTS {role};")).ok();
    let Ok(_) = admin.batch_execute(&format!(
        "CREATE ROLE {role} WITH LOGIN PASSWORD '{password}' NOSUPERUSER NOCREATEDB NOCREATEROLE; \
         GRANT CREATE, USAGE ON SCHEMA public TO {role};"
    )) else {
        eprintln!("saltado: el rol de LINK_TEST_PG_URL no tiene permiso de crear roles -- no se puede simular esta restricción acá");
        return;
    };

    let temp = TempDir::new("restricted-role");
    let src = temp.write(
        "app.link",
        &format!("type Item = {{ id: Int, name: String }}\ndb {{ {COLLECTION}: Item[] }}"),
    );
    let out_dir = temp.0.join("gen");
    let build = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("build").arg(&src).arg(&out_dir).output().expect("ejecutar linkc build");
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let emitted = std::fs::read_to_string(out_dir.join("schema.postgres.sql")).expect("schema.postgres.sql");
    assert!(!emitted.to_lowercase().contains("extension"), "el DDL emitido no debería pedir ninguna extensión: {emitted}");

    let restricted_url = with_credentials(&url, role, password);
    let mut restricted = postgres::Client::connect(&restricted_url, postgres::NoTls)
        .expect("conectar con el rol restringido -- si esto falla, el problema es la conexión, no el DDL");
    restricted
        .batch_execute(&emitted)
        .expect("un rol SIN superusuario/createrole tiene que poder aplicar el schema completo generado, sin CREATE EXTENSION de por medio");

    admin.batch_execute(&format!("DROP TABLE IF EXISTS \"{COLLECTION}\"; DROP ROLE IF EXISTS {role};")).ok();
}
